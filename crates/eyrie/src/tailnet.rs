//! Tailscale de la suite (synchro mobile) : **un seul nœud** pour la suite, au nom générique (`suite-<poste>`), état
//! dans `%LOCALAPPDATA%\EyrieSuite\Eyrie\tailnet`.
//!
//! Le compagnon d'Eyrie (`eyrie-tailnet.exe`, en Go avec tsnet, dossier `compagnon/`) rejoint le tailnet et relaie
//! les connexions du téléphone (port 4747) vers le pont mobile d'Eyrie : Eyrie lui donne l'adresse locale du pont et
//! un jeton par son entrée standard, une ligne JSON à chaque changement (`{"relais":"127.0.0.1:…","jeton":"…"}`, ou
//! `{"relais":""}` : plus rien). Il annonce son état par lignes JSON sur sa sortie et s'arrête quand Eyrie ferme son
//! entrée. Le compagnon de Fawkes (`fawkes-tailnet.exe`) sert de secours : le nœud seul, sans relais.
//!
//! Un seul compagnon par nœud (son verrou, [`veille::lock_name`]). Quand le verrou est pris, c'est d'ordinaire le
//! veilleur (`--veille`, « Démarrer Eyrie depuis le téléphone », voir [`crate::veille`]) : Eyrie ne lance rien, elle
//! s'y rattache par son tube, qui parle comme l'entrée et la sortie du compagnon, et le quitte avec `{"relais":""}`
//! (le nœud reste sur le tailnet). `{"arret":true}` l'arrête : réglage coupé, Tailscale coupé, autre nom de nœud.
//! Verrou pris sans tube : un veilleur démarre ou s'arrête, Eyrie réessaie quelques secondes ; au-delà, un autre
//! compagnon tient le nœud, et Eyrie ne lance pas celui de Fawkes à côté. Tout cela se décide dans un fil, jamais
//! dans celui de l'interface ; ce qui touche au système passe par [`Os`].

use std::ffi::OsString;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use eframe::egui::{self, Color32};
use suite_fawkes::tailnet::{self as fawkes_tailnet, TailnetState};

use crate::i18n::tr;
use crate::persist::{MobileSettings, TailnetSettings};
use crate::veille::{self, Attach, Os, Pipe, Process, Spawned, Veille};

/// Le compagnon d'Eyrie, qui sait relayer le téléphone.
pub const COMPANION: &str = "eyrie-tailnet.exe";

/// Verrou pris sans tube : le temps laissé à un veilleur pour ouvrir son tube ; après un `{"arret":true}`, pour
/// lâcher le nœud.
const PATIENCE: Duration = Duration::from_secs(5);
const HALT_PATIENCE: Duration = Duration::from_secs(20);
/// Le temps laissé au veilleur qu'Eyrie vient de lancer pour prendre le nœud.
const STANDBY_START: Duration = Duration::from_secs(10);
/// Entre deux essais d'ouvrir le tube.
const RETRY: Duration = Duration::from_millis(200);
/// Un veilleur arrêté de l'extérieur n'est pas relancé tout de suite : `tools\compagnon.ps1` l'arrête avant de
/// recompiler son exécutable, qu'un compagnon relancé verrouillerait.
const STANDBY_GONE: Duration = Duration::from_secs(30);

/// Le compagnon que tient Eyrie.
enum Conn {
    /// Lancé par Eyrie : son entrée fermée, il s'arrête.
    Child { stdin: Box<dyn Write + Send>, process: Box<dyn Process> },
    /// Le veilleur, par son tube : il survit à Eyrie.
    Standby { pipe: Box<dyn Write + Send> },
}

/// Comment Eyrie lâche son compagnon.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Leave {
    /// Eyrie part : un compagnon lancé par elle s'arrête, le veilleur garde le nœud (`{"relais":""}`).
    Detach,
    /// Le veilleur s'arrête aussi (`{"arret":true}`).
    Halt,
}

#[derive(Default)]
struct Slot {
    conn: Option<Conn>,
    /// Eyrie ne veut plus de ce lien : ce que le fil obtient ensuite est lâché aussitôt.
    gone: Option<Leave>,
}

/// Ce que partagent Eyrie et le fil qui lance le compagnon ou se rattache au veilleur.
struct Link {
    state: Mutex<TailnetState>,
    slot: Mutex<Slot>,
    /// Plus rien à attendre de ce lien : compagnon sorti, tube fermé, nœud tenu par un autre.
    ended: AtomicBool,
    /// Le lien est le tube d'un veilleur.
    standby: AtomicBool,
    /// Pourquoi le veilleur voulu ne tourne pas (le compagnon lancé par Eyrie le remplace).
    fallback: Mutex<Option<String>>,
    ctx: egui::Context,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Link {
    fn new(ctx: egui::Context) -> Link {
        Link {
            state: Mutex::new(TailnetState::Starting),
            slot: Mutex::default(),
            ended: AtomicBool::new(false),
            standby: AtomicBool::new(false),
            fallback: Mutex::new(None),
            ctx,
        }
    }

    fn set(&self, s: TailnetState) {
        *lock(&self.state) = s;
        self.ctx.request_repaint();
    }

    fn gone(&self) -> bool {
        lock(&self.slot).gone.is_some()
    }

    fn end(&self) {
        self.ended.store(true, Ordering::Relaxed);
        self.ctx.request_repaint();
    }

    fn fail_end(&self, msg: impl Into<String>) {
        if !self.gone() {
            self.set(TailnetState::Failed(msg.into()));
        }
        self.end();
    }

    /// Le compagnon obtenu devient celui d'Eyrie ; si elle n'en veut plus entre-temps, il est lâché (faux).
    fn install(&self, conn: Conn) -> bool {
        let leave = {
            let mut slot = lock(&self.slot);
            match slot.gone {
                None => {
                    slot.conn = Some(conn);
                    None
                }
                Some(leave) => Some((conn, leave)),
            }
        };
        self.ctx.request_repaint();
        match leave {
            None => true,
            Some((conn, leave)) => {
                close(conn, leave);
                false
            }
        }
    }

    /// Les lignes d'état d'un compagnon (sa sortie) ou du veilleur (son tube), jusqu'à la fin.
    fn read_states(&self, from: Box<dyn Read + Send>) -> bool {
        let mut heard = false;
        for line in BufReader::new(from).lines() {
            let Ok(line) = line else { break };
            heard = true;
            if let Some(s) = fawkes_tailnet::parse_line(&line) {
                self.set(s);
            }
        }
        heard
    }
}

/// Écrit une consigne ; faux si le compagnon ne la reçoit plus.
fn send(w: &mut dyn Write, line: &str) -> bool {
    writeln!(w, "{line}").and_then(|_| w.flush()).is_ok()
}

/// Lâche un compagnon. Lancé par Eyrie : son entrée fermée, il se déconnecte proprement ; arrêté de force au bout de
/// 3 s. Le veilleur : il garde le nœud sans plus rien relayer, ou s'arrête.
fn close(conn: Conn, leave: Leave) {
    match conn {
        Conn::Child { stdin, mut process } => {
            drop(stdin);
            let deadline = Instant::now() + Duration::from_secs(3);
            while Instant::now() < deadline {
                if !process.running() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            process.kill();
        }
        Conn::Standby { mut pipe } => {
            let line = match leave {
                Leave::Detach => relay_line(&None),
                Leave::Halt => serde_json::json!({ "arret": true }).to_string(),
            };
            send(&mut *pipe, &line);
        }
    }
}

/// Ce que le fil d'un lien doit faire.
struct Plan {
    os: Arc<dyn Os>,
    exe: Option<PathBuf>,
    node: PathBuf,
    hostname: String,
    /// Les arguments du veilleur, s'il est voulu : lancé si personne ne tient le nœud.
    standby: Option<Vec<OsString>>,
    /// Combien attendre un tube quand le verrou est pris.
    patience: Duration,
    /// Seulement arrêter le veilleur qui tiendrait le nœud (Tailscale coupé).
    halt_only: bool,
}

impl Plan {
    fn run(self, link: Arc<Link>) {
        let (lock_name, pipe) = (veille::lock_name(&self.node), veille::pipe_name(&self.node));
        let Some(exe) = self.exe.clone() else {
            if !self.halt_only {
                link.fail_end(format!("{COMPANION} introuvable : compilez-le avec tools\\compagnon.ps1."));
            }
            return link.end();
        };
        let mut deadline = Instant::now() + self.patience;
        // Le veilleur qu'Eyrie vient de lancer, et quand.
        let mut launched: Option<(Box<dyn Process>, u64)> = None;
        let mut tried = false;
        loop {
            if link.gone() {
                return link.end();
            }
            if self.os.node_held(&lock_name) {
                match self.os.attach(&pipe, &exe) {
                    Ok(mut p) if self.halt_only => {
                        send(&mut *p.writer, &serde_json::json!({ "arret": true }).to_string());
                        return link.end();
                    }
                    Ok(p) => return self.serve_standby(p, &link),
                    Err(Attach::Absent) if Instant::now() < deadline => {}
                    Err(Attach::Absent) if self.halt_only => return link.end(),
                    Err(Attach::Absent) => {
                        return link.fail_end(
                            "nœud déjà tenu par un autre compagnon, sans tube de veilleur : Eyrie n'en lance pas un second",
                        );
                    }
                    Err(Attach::Refused(e)) => {
                        return link.fail_end(format!("nœud tenu par un compagnon qu'Eyrie ne reconnaît pas : {e}"));
                    }
                }
                std::thread::sleep(RETRY);
                continue;
            }
            if self.halt_only {
                return link.end();
            }
            if let Some(args) = &self.standby
                && !tried
            {
                tried = true;
                match self.os.spawn_standby(&exe, args) {
                    Ok(p) => {
                        launched = Some((p, now()));
                        deadline = Instant::now() + STANDBY_START;
                        continue;
                    }
                    Err(e) => *lock(&link.fallback) = Some(format!("lancement du veilleur : {e}")),
                }
            }
            if let Some((p, at)) = &mut launched {
                if p.running() && Instant::now() < deadline {
                    std::thread::sleep(RETRY);
                    continue;
                }
                // Sorti sans prendre le nœud (ou trop lent) : pourquoi, s'il l'a écrit.
                let why = veille::read_failure(&self.node)
                    .filter(|f| f.at + 5 >= *at)
                    .map(|f| f.message)
                    .unwrap_or_else(|| "le veilleur s'est arrêté sans prendre le nœud".into());
                if p.running() {
                    p.kill();
                }
                *lock(&link.fallback) = Some(why);
            }
            return self.spawn_companion(&exe, &link);
        }
    }

    /// Rattachée au veilleur : ses lignes d'état jusqu'à ce qu'il ferme le tube.
    fn serve_standby(&self, p: Pipe, link: &Link) {
        link.standby.store(true, Ordering::Relaxed);
        if !link.install(Conn::Standby { pipe: p.writer }) {
            return link.end();
        }
        let heard = link.read_states(p.reader);
        if !link.gone() {
            let log = self.node.join("journal.txt");
            link.set(TailnetState::Failed(if heard {
                format!("le veilleur s'est arrêté, Eyrie reprend le nœud (détails : {})", log.display())
            } else {
                // Un client qu'il ne reconnaît pas est fermé sans un mot.
                format!(
                    "le veilleur a refusé Eyrie : il ne sert que l'eyrie.exe rangé à côté de lui (détails : {})",
                    log.display()
                )
            }));
        }
        link.end();
    }

    fn spawn_companion(&self, exe: &Path, link: &Link) {
        let own = is_own(exe);
        let mut args: Vec<OsString> = vec![
            "--dir".into(),
            self.node.clone().into(),
            "--hostname".into(),
            self.hostname.clone().into(),
            "--version".into(),
            env!("CARGO_PKG_VERSION").into(),
        ];
        if own {
            args.extend(["--port".into(), suite_mobile::PORT.to_string().into()]);
        }
        let Spawned { stdin, stdout, process } = match self.os.spawn_companion(exe, &args) {
            Ok(s) => s,
            Err(e) => return link.fail_end(format!("lancement de {} : {e}", exe.display())),
        };
        if !link.install(Conn::Child { stdin, process }) {
            return link.end();
        }
        link.read_states(stdout);
        // Sortie fermée : le compagnon s'est arrêté (ou Eyrie l'a arrêté).
        let log = self.node.join("journal.txt");
        {
            let mut s = lock(&link.state);
            if !matches!(*s, TailnetState::Failed(_)) && !link.gone() {
                *s = TailnetState::Failed(format!(
                    "le compagnon s'est arrêté, Eyrie le relance (détails : {})",
                    log.display()
                ));
            }
        }
        link.end();
    }
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn is_own(exe: &Path) -> bool {
    exe.file_name().is_some_and(|n| n.eq_ignore_ascii_case(COMPANION))
}

struct Running {
    link: Arc<Link>,
    hostname: String,
    /// Le veilleur était voulu au lancement de ce lien.
    standby: bool,
    /// Le compagnon d'Eyrie (relais) plutôt que celui de Fawkes.
    own: bool,
    /// Dernière consigne de relais écrite.
    relay: Option<Option<(String, String)>>,
}

pub struct Tailnet {
    /// Le système (verrou, tube, processus, registre, DPAPI) ; un faux dans les essais.
    pub os: Arc<dyn Os>,
    running: Option<Running>,
    /// Quand le compagnon en marche s'est arrêté de lui-même (il sera relancé).
    exited: Option<Instant>,
    /// Relances depuis la dernière fois que le nœud était connecté.
    restarts: u32,
    /// Le dernier lien a arrêté le veilleur : le suivant lui laisse le temps de lâcher le nœud.
    halted: bool,
    /// Le dossier du nœud, une fois connu ([`node_dir`] déplace un ancien dossier).
    node: Option<PathBuf>,
    /// L'Eyrie de tous les jours (dossier par défaut) : seule elle inscrit le veilleur à l'ouverture de session.
    pub main: bool,
    /// Le réglage du veilleur : valeur Run, `veille.dpapi`, clé.
    pub veille: Veille,
    /// Tailscale coupé : un veilleur resté seul sur le nœud a été arrêté (une fois).
    orphan_checked: bool,
}

/// Délai avant de relancer un compagnon arrêté de lui-même, selon les relances déjà faites : 5 s, puis le double à
/// chaque fois, 2 min au plus.
fn restart_delay(restarts: u32) -> Duration {
    Duration::from_secs(5 << restarts.min(5)).min(Duration::from_secs(120))
}

/// Où trouver le compagnon : `EYRIE_TAILNET_EXE`, `eyrie-tailnet.exe` à côté d'`eyrie.exe` ou compilé dans ce dépôt ;
/// sinon, en secours, celui de Fawkes (`FAWKES_TAILNET_EXE`, à côté, ou compilé dans le dépôt voisin).
pub fn find_companion() -> Option<PathBuf> {
    for var in ["EYRIE_TAILNET_EXE", "FAWKES_TAILNET_EXE"] {
        if let Some(p) = std::env::var_os(var).filter(|p| !p.is_empty()) {
            return Some(PathBuf::from(p));
        }
    }
    let mut candidates = Vec::new();
    let beside = std::env::current_exe().ok().and_then(|e| e.parent().map(Path::to_path_buf));
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    for (name, repo) in [
        (COMPANION, manifest.join("../../target")),
        (fawkes_tailnet::COMPANION, manifest.join("../../../Fawkes/target")),
    ] {
        if let Some(dir) = &beside {
            candidates.push(dir.join(name));
        }
        for profile in ["release", "debug"] {
            let built = repo.join(profile).join(name);
            // Un chemin de compilation, seulement s'il est dans le profil de l'utilisateur : ailleurs (`D:\build\…`,
            // `C:\agent\…`), un autre compte du poste pourrait créer ce chemin et y déposer un faux compagnon,
            // qu'Eyrie lancerait sous le compte de l'utilisateur.
            if dirs::home_dir().is_some_and(|home| inside(&built, &home)) {
                candidates.push(built);
            }
        }
    }
    candidates.into_iter().find(|p| p.is_file())
}

/// `path` est-il dans `dir` (sans `..`, casse ignorée comme sous Windows) ?
fn inside(path: &Path, dir: &Path) -> bool {
    let norm = |p: &Path| {
        let mut out = Vec::new();
        for c in p.components() {
            match c {
                std::path::Component::ParentDir => {
                    out.pop();
                }
                std::path::Component::CurDir => {}
                c => out.push(c.as_os_str().to_string_lossy().to_lowercase()),
            }
        }
        out
    };
    norm(path).starts_with(&norm(dir))
}

/// État du nœud (sa clé privée comprise) d'Eyrie dont le dossier est `dir` : dans `%LOCALAPPDATA%\EyrieSuite\Eyrie`,
/// jamais dans le profil itinérant (`%APPDATA%`), qu'une synchronisation ou une sauvegarde copierait ailleurs — qui
/// aurait la clé pourrait se faire passer pour ce poste sur le tailnet. L'ancien dossier y est déplacé. Un Eyrie sur
/// un autre dossier (essais) garde le sien.
fn node_dir(dir: &Path) -> PathBuf {
    let old = dir.join("tailnet");
    if dir != crate::persist::default_data_dir() {
        return old;
    }
    suite_accounts::moved(&old, &suite_accounts::local_suite_dir().join("Eyrie").join("tailnet"))
}

pub fn hostname(settings: &TailnetSettings) -> String {
    let chosen = if settings.hostname.trim().is_empty() {
        format!("suite-{}", fawkes_tailnet::computer_name())
    } else {
        settings.hostname.clone()
    };
    fawkes_tailnet::hostname_for(&chosen, &fawkes_tailnet::computer_name())
}

/// La consigne de relais écrite au compagnon.
fn relay_line(relay: &Option<(String, String)>) -> String {
    match relay {
        Some((addr, token)) => serde_json::json!({ "relais": addr, "jeton": token }).to_string(),
        None => serde_json::json!({ "relais": "" }).to_string(),
    }
}

impl Tailnet {
    pub fn new(dir: &Path) -> Self {
        // Les essais n'approchent ni le vrai registre, ni le vrai nœud.
        #[cfg(test)]
        let os: Arc<dyn Os> = Arc::new(veille::fake::Fake::default());
        #[cfg(not(test))]
        let os: Arc<dyn Os> = Arc::new(veille::System);
        Tailnet {
            os,
            running: None,
            exited: None,
            restarts: 0,
            halted: false,
            node: None,
            main: dir == crate::persist::default_data_dir(),
            veille: Veille::default(),
            orphan_checked: false,
        }
    }

    fn node(&mut self, dir: &Path) -> PathBuf {
        self.node.get_or_insert_with(|| node_dir(dir)).clone()
    }

    /// Le dossier du nœud, s'il est déjà connu (page des réglages : `veille-echec.json`).
    pub fn node_path(&self) -> Option<&Path> {
        self.node.as_deref()
    }

    /// Démarre, arrête ou redémarre le compagnon selon les réglages, et le relance s'il s'est arrêté de lui-même ; lui
    /// donne la consigne de relais du pont mobile. Applique le réglage du veilleur s'il a changé.
    pub fn poll(
        &mut self,
        settings: &TailnetSettings,
        mobile: &MobileSettings,
        dir: &Path,
        ctx: &egui::Context,
        relay: Option<(String, String)>,
    ) {
        let want = veille::Want {
            on: settings.enabled && mobile.wake,
            active: mobile.enabled,
            main: self.main,
            hostname: hostname(settings),
            node: self.node(dir),
            mobile: crate::mobile::store::mobile_dir(dir),
            instance: veille::instance_lock(dir),
        };
        let os = self.os.clone();
        self.veille.apply(&*os, &want);

        if let Some(r) = &self.running
            && r.link.ended.load(Ordering::Relaxed)
        {
            let since = *self.exited.get_or_insert_with(Instant::now);
            let mut delay = restart_delay(self.restarts);
            if r.link.standby.load(Ordering::Relaxed) {
                delay = delay.max(STANDBY_GONE);
            }
            if since.elapsed() >= delay {
                self.restarts += 1;
                // Relancé juste après, comme un compagnon voulu mais absent.
                self.release(Leave::Detach);
            }
        }
        if matches!(self.state(), Some(TailnetState::Running { .. })) {
            self.restarts = 0;
        }

        let standby = self.veille.standby();
        let wanted = settings.enabled.then(|| (want.hostname.clone(), standby.is_some()));
        let attached = self.running.as_ref().is_some_and(|r| r.link.standby.load(Ordering::Relaxed));
        // Un veilleur trouvé sur le nœud est celui qu'on veut maintenant : il reste.
        if self.standby_attached()
            && let (Some(r), Some((h, true))) = (&mut self.running, &wanted)
            && r.hostname == *h
        {
            r.standby = true;
        }
        let current = self.running.as_ref().map(|r| (r.hostname.clone(), r.standby));
        // Un veilleur dont on ne veut pas (réglage coupé) s'arrête ; parti de lui-même, il attend la relance. Celui
        // qu'une autre eyrie.exe a inscrit, et que cette Eyrie ne peut pas lancer, est voulu aussi : il reste.
        let kept = want.on && self.veille.kept.is_some();
        let unwanted = self.standby_attached() && !kept && !wanted.as_ref().is_some_and(|(_, s)| *s);
        if wanted != current || unwanted {
            self.release(if attached { Leave::Halt } else { Leave::Detach });
            if let Some((h, _)) = wanted {
                self.launch(h, standby, &want.node, ctx, false);
            }
        }
        if !settings.enabled && !self.orphan_checked && self.running.is_none() {
            // Tailscale coupé : un veilleur resté sur le nœud (Eyrie arrêtée avant de l'arrêter) s'arrête.
            self.orphan_checked = true;
            self.launch(String::new(), None, &want.node, ctx, true);
        } else if settings.enabled {
            self.orphan_checked = false;
        }

        if let Some(r) = &mut self.running
            && r.own
            && r.relay.as_ref() != Some(&relay)
        {
            let line = relay_line(&relay);
            let mut slot = lock(&r.link.slot);
            let written = match slot.conn.as_mut() {
                Some(Conn::Child { stdin, .. }) => send(&mut **stdin, &line),
                Some(Conn::Standby { pipe }) => send(&mut **pipe, &line),
                None => false,
            };
            if written {
                r.relay = Some(relay);
            }
        }
    }

    /// Lance le fil d'un lien : rattachement au veilleur, ou lancement du veilleur ou du compagnon. `halt_only` : le
    /// fil ne fait qu'arrêter un veilleur trouvé sur le nœud, et n'est pas gardé.
    fn launch(
        &mut self,
        hostname: String,
        standby: Option<(PathBuf, Vec<OsString>)>,
        node: &Path,
        ctx: &egui::Context,
        halt_only: bool,
    ) {
        if !halt_only {
            let _ = std::fs::create_dir_all(node);
        }
        let wants_standby = standby.is_some();
        let (exe, args) = match standby {
            Some((exe, args)) => (Some(exe), Some(args)),
            None => (self.os.companion(), None),
        };
        let own = exe.as_deref().is_some_and(is_own);
        let plan = Plan {
            os: self.os.clone(),
            exe,
            node: node.to_path_buf(),
            hostname: hostname.clone(),
            standby: args,
            patience: if std::mem::take(&mut self.halted) { HALT_PATIENCE } else { PATIENCE },
            halt_only,
        };
        let link = Arc::new(Link::new(ctx.clone()));
        let l = link.clone();
        if let Err(e) = std::thread::Builder::new().name("tailnet".into()).spawn(move || plan.run(l)) {
            link.fail_end(format!("fil du compagnon : {e}"));
        }
        if !halt_only {
            self.running = Some(Running { link, hostname, standby: wants_standby, own, relay: None });
        }
    }

    /// Lâche le lien en cours.
    fn release(&mut self, leave: Leave) {
        self.exited = None;
        let Some(r) = self.running.take() else { return };
        let conn = {
            let mut slot = lock(&r.link.slot);
            slot.gone = Some(leave);
            slot.conn.take()
        };
        if leave == Leave::Halt {
            self.halted = true;
        }
        if let Some(c) = conn {
            close(c, leave);
        }
    }

    pub fn state(&self) -> Option<TailnetState> {
        self.running.as_ref().map(|r| lock(&r.link.state).clone())
    }

    /// Le compagnon en marche sait relayer le téléphone, et relaie.
    pub fn relays(&self) -> bool {
        self.running.as_ref().is_some_and(|r| r.own && matches!(r.relay, Some(Some(_))))
    }

    /// Le compagnon en marche est celui d'Eyrie.
    pub fn own_companion(&self) -> Option<bool> {
        self.running.as_ref().map(|r| r.own)
    }

    /// Eyrie est rattachée au veilleur (il tient le nœud, et le gardera quand elle se fermera).
    pub fn standby_attached(&self) -> bool {
        self.running
            .as_ref()
            .is_some_and(|r| r.link.standby.load(Ordering::Relaxed) && !r.link.ended.load(Ordering::Relaxed))
    }

    /// Pourquoi le veilleur voulu ne tourne pas (le compagnon lancé par Eyrie le remplace).
    pub fn standby_fallback(&self) -> Option<String> {
        self.running.as_ref().filter(|r| r.standby).and_then(|r| lock(&r.link.fallback).clone())
    }

    /// La clé dont chaque téléphone tire son secret de démarrage (`Welcome.wake`) : seulement pour un veilleur voulu
    /// et possible.
    pub fn wake_key(&self) -> Option<[u8; 32]> {
        self.veille.key()
    }

    /// « Réessayer » le veilleur : réglage réappliqué, et un compagnon qui le remplace est relancé.
    pub fn retry_standby(&mut self) {
        self.veille.retry();
        if self.running.as_ref().is_some_and(|r| r.standby && !r.link.standby.load(Ordering::Relaxed)) {
            self.release(Leave::Detach);
        }
    }

    /// Eyrie part : un compagnon lancé par elle s'arrête (son entrée fermée, de force au bout de 3 s) ; le veilleur
    /// garde le nœud, sans plus rien relayer.
    pub fn stop(&mut self) {
        self.release(Leave::Detach);
    }

    pub fn status_line(&self, p: &suite_sdk::theme::Palette) -> (String, Color32) {
        match self.state() {
            None => (format!("⛓ {}", tr("Tailscale coupé")), p.text_dim),
            Some(TailnetState::Starting) => (format!("⛓ {}", tr("Tailscale : démarrage")), p.text_muted),
            Some(TailnetState::NeedsLogin(_)) => (format!("⛓ {}", tr("Tailscale : connexion à faire")), p.warn),
            Some(TailnetState::Running { name, .. }) => (format!("⛓ {name}"), p.ok),
            Some(TailnetState::Failed(_)) => (format!("⛓ {}", tr("Tailscale : erreur")), p.danger),
        }
    }
}

impl Drop for Tailnet {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::veille::fake::Fake;

    #[test]
    fn default_node_name_is_generic() {
        let h = hostname(&TailnetSettings { enabled: true, hostname: String::new() });
        assert!(h.starts_with("suite"), "{h}");
        assert_eq!(hostname(&TailnetSettings { enabled: true, hostname: "Mon Poste".into() }), "mon-poste");
    }

    #[test]
    fn a_stopped_companion_is_restarted_ever_more_slowly() {
        let delays: Vec<u64> = (0..8).map(|n| restart_delay(n).as_secs()).collect();
        assert_eq!(delays, [5, 10, 20, 40, 80, 120, 120, 120]);
    }

    #[test]
    fn an_eyrie_on_another_folder_keeps_its_node_there() {
        let dir = Path::new(r"C:\Temp\eyrie-essai");
        assert_eq!(node_dir(dir), dir.join("tailnet"));
    }

    /// Le compagnon compilé dans un dépôt n'est cherché que dans le profil de l'utilisateur.
    #[test]
    fn the_built_companion_is_looked_for_in_the_profile_only() {
        let home = Path::new(r"C:\Users\aubin");
        assert!(inside(Path::new(r"C:\Users\Aubin\DEV\Eyrie\crates\eyrie\../../../Fawkes/target/release/x.exe"), home));
        assert!(!inside(Path::new(r"D:\build\Eyrie\crates\eyrie\../../../Fawkes/target/release/x.exe"), home));
        assert!(!inside(Path::new(r"C:\Users\aubin\..\autre\Fawkes\x.exe"), home));
        assert!(!inside(Path::new(r"C:\Users\aubinette\x.exe"), home));
    }

    #[test]
    fn the_relay_order_is_one_json_line() {
        let on = relay_line(&Some(("127.0.0.1:5000".into(), "abc".into())));
        let v: serde_json::Value = serde_json::from_str(&on).unwrap();
        assert_eq!((v["relais"].as_str(), v["jeton"].as_str()), (Some("127.0.0.1:5000"), Some("abc")));
        assert!(!on.contains('\n'));
        let off: serde_json::Value = serde_json::from_str(&relay_line(&None)).unwrap();
        assert_eq!(off["relais"], "");
    }

    /// Un Tailnet sur un faux système, dans un dossier d'essai, comme l'Eyrie de tous les jours : le compagnon d'Eyrie
    /// est à côté d'`eyrie.exe`.
    struct Bench {
        t: Tailnet,
        fake: Arc<Fake>,
        dir: tempfile::TempDir,
        ctx: egui::Context,
        settings: TailnetSettings,
        mobile: MobileSettings,
    }

    const RELAY: (&str, &str) = ("127.0.0.1:5000", "jeton");

    impl Bench {
        fn new() -> Bench {
            let fake = Arc::new(Fake::default());
            let dir = tempfile::tempdir().unwrap();
            let mut t = Tailnet::new(dir.path());
            t.os = fake.clone();
            t.main = true;
            *fake.companion.lock().unwrap() = Some(dir.path().join(COMPANION));
            *fake.eyrie.lock().unwrap() = Some(dir.path().join("eyrie.exe"));
            Bench {
                t,
                fake,
                dir,
                ctx: egui::Context::default(),
                settings: TailnetSettings { enabled: true, hostname: "essai".into() },
                mobile: MobileSettings { enabled: true, ..Default::default() },
            }
        }

        fn poll(&mut self) {
            let relay = Some((RELAY.0.to_string(), RELAY.1.to_string()));
            self.t.poll(&self.settings, &self.mobile, self.dir.path(), &self.ctx, relay);
        }

        /// Fait tourner `poll` jusqu'à ce que `done` dise oui.
        fn until(&mut self, mut done: impl FnMut(&mut Bench) -> bool) {
            let deadline = Instant::now() + Duration::from_secs(20);
            while !done(self) {
                assert!(Instant::now() < deadline, "délai dépassé : {:?}", self.t.state());
                self.poll();
                std::thread::sleep(Duration::from_millis(10));
            }
        }

        fn launches(&self) -> (usize, usize) {
            (self.fake.companion_launches.lock().unwrap().len(), self.fake.standby_launches.lock().unwrap().len())
        }

        /// `veille.dpapi`, déchiffré.
        fn config(&self) -> Option<veille::Config> {
            let sealed = std::fs::read(self.dir.path().join("mobile").join(veille::CONFIG)).ok()?;
            serde_json::from_slice(&self.fake.unseal(&sealed).unwrap()).ok()
        }
    }

    fn relay_json() -> String {
        relay_line(&Some((RELAY.0.into(), RELAY.1.into())))
    }

    const RUNNING: &str = r#"{"state":"running","name":"essai.ts.net","ip":"100.1.2.3"}"#;

    /// Le nœud tenu par le veilleur voulu : Eyrie ne lance rien, elle s'y rattache, lit son état, lui donne le relais ;
    /// en partant, elle lui laisse le nœud (`{"relais":""}`).
    #[test]
    fn eyrie_attaches_to_the_standby_holding_the_node_and_leaves_it_up() {
        let mut b = Bench::new();
        b.mobile.wake = true;
        b.fake.standby_up();
        b.poll();
        let mut pipe = b.fake.pipe();
        pipe.say(RUNNING);
        b.until(|b| matches!(b.t.state(), Some(TailnetState::Running { .. })) && b.t.relays());
        assert_eq!(pipe.line().as_deref(), Some(relay_json().as_str()));
        assert!(b.t.standby_attached());
        assert_eq!(b.launches(), (0, 0), "rien n'est lancé");
        b.t.stop();
        assert_eq!(pipe.line().as_deref(), Some(r#"{"relais":""}"#));
        assert_eq!(pipe.line(), None, "tube lâché");
    }

    /// Verrou pris sans tube : Eyrie réessaie quelques secondes, puis dit qu'un autre compagnon tient le nœud, sans
    /// lancer le sien ni celui de Fawkes à côté ; le veilleur enfin prêt, elle s'y rattache à la relance.
    #[test]
    fn a_node_held_without_a_pipe_is_never_doubled() {
        let mut b = Bench::new();
        *b.fake.held.lock().unwrap() = true;
        let started = Instant::now();
        b.poll();
        b.until(|b| matches!(b.t.state(), Some(TailnetState::Failed(_))));
        assert!(started.elapsed() >= PATIENCE - Duration::from_millis(500), "réessayé quelques secondes");
        let Some(TailnetState::Failed(e)) = b.t.state() else { unreachable!() };
        assert!(e.contains("déjà tenu"), "{e}");
        assert_eq!(b.launches(), (0, 0));
        b.fake.standby_up();
        b.t.exited = Some(Instant::now() - Duration::from_secs(600));
        b.poll();
        let _pipe = b.fake.pipe();
        assert_eq!(b.launches(), (0, 0));
    }

    /// Un tube tenu par un autre exécutable : refusé, rien n'est lancé.
    #[test]
    fn a_foreign_pipe_is_refused() {
        let mut b = Bench::new();
        b.fake.standby_up();
        *b.fake.foreign.lock().unwrap() = Some(r"le tube est tenu par C:\autre.exe".into());
        b.poll();
        b.until(|b| matches!(b.t.state(), Some(TailnetState::Failed(_))));
        let Some(TailnetState::Failed(e)) = b.t.state() else { unreachable!() };
        assert!(e.contains("autre.exe"), "{e}");
        assert_eq!(b.launches(), (0, 0));
    }

    /// Personne sur le nœud, pas de veilleur voulu : le compagnon est lancé, avec le relais ; rien n'est inscrit à
    /// l'ouverture de session ni écrit pour le veilleur.
    #[test]
    fn without_the_setting_the_companion_is_spawned() {
        let mut b = Bench::new();
        b.poll();
        let mut child = b.fake.child();
        let (exe, args) = b.fake.companion_launches.lock().unwrap()[0].clone();
        assert_eq!(exe, b.dir.path().join(COMPANION));
        assert!(args.contains(&"--port".into()) && !args.contains(&"--veille".into()), "{args:?}");
        b.until(|b| b.t.relays());
        assert_eq!(child.line().as_deref(), Some(relay_json().as_str()));
        assert_eq!(*b.fake.run.lock().unwrap(), None);
        assert_eq!(b.config(), None);
        assert_eq!(b.t.wake_key(), None);
        b.t.stop();
        assert_eq!(child.line(), None, "entrée fermée : le compagnon s'arrête");
    }

    /// « Démarrer Eyrie depuis le téléphone » allumé : le compagnon d'Eyrie s'arrête, le veilleur est inscrit à
    /// l'ouverture de session, lancé détaché et rattaché, son réglage écrit, et les téléphones ont une clé. L'accès
    /// coupé : `actif: false`, le veilleur reste. Coupé : il s'arrête, la valeur Run est retirée, plus de clé, et le
    /// compagnon d'Eyrie reprend le nœud une fois libéré.
    #[test]
    fn toggling_the_setting_switches_between_companion_and_standby() {
        let mut b = Bench::new();
        b.poll();
        let mut child = b.fake.child();
        b.until(|b| b.t.relays());
        assert_eq!(child.line().as_deref(), Some(relay_json().as_str()));

        b.mobile.wake = true;
        b.poll();
        assert_eq!(child.line(), None, "le compagnon d'Eyrie s'arrête");
        let mut pipe = b.fake.pipe();
        let exe = b.dir.path().join(COMPANION);
        let (spawned, args) = b.fake.standby_launches.lock().unwrap()[0].clone();
        assert_eq!(spawned, exe);
        let node = b.dir.path().join("tailnet");
        let mobile = b.dir.path().join("mobile");
        assert_eq!(args, veille::standby_args(&node, &mobile, "essai"));
        pipe.say(RUNNING);
        b.until(|b| b.t.standby_attached() && b.t.relays());
        assert_eq!(pipe.line().as_deref(), Some(relay_json().as_str()));
        assert_eq!(*b.fake.run.lock().unwrap(), Some(veille::run_line(&exe, &node, &mobile, "essai")));
        let c = b.config().expect("réglage du veilleur écrit");
        assert!(c.actif && c.reduite && c.v == 1);
        assert_eq!(c.instance, veille::instance_lock(b.dir.path()));
        let key = b.t.wake_key().expect("une clé pour les téléphones");
        assert_eq!(crate::mobile::store::hex(&key), c.cle);
        assert_eq!(b.launches(), (1, 1));

        // L'accès depuis le téléphone coupé : le veilleur reste, mais refuse de démarrer Eyrie ; même clé.
        b.mobile.enabled = false;
        b.poll();
        let off = b.config().unwrap();
        assert!(!off.actif);
        assert_eq!(off.cle, c.cle);
        assert!(b.t.standby_attached());

        // Coupé.
        b.mobile.enabled = true;
        b.mobile.wake = false;
        b.poll();
        assert_eq!(pipe.line().as_deref(), Some(r#"{"arret":true}"#));
        assert_eq!(*b.fake.run.lock().unwrap(), None, "valeur Run retirée");
        assert!(!b.config().unwrap().actif);
        assert_eq!(b.t.wake_key(), None);
        // Le veilleur lâche le nœud un peu plus tard : pas de second compagnon d'ici là.
        std::thread::sleep(Duration::from_millis(600));
        b.poll();
        assert_eq!(b.launches(), (1, 1));
        b.fake.standby_down();
        let mut child = b.fake.child();
        b.until(|b| b.t.relays());
        assert_eq!(child.line().as_deref(), Some(relay_json().as_str()));
        assert_eq!(b.launches(), (2, 1));
    }

    /// Un veilleur trouvé sur le nœud alors que le réglage est coupé : arrêté, puis le compagnon d'Eyrie.
    #[test]
    fn a_standby_found_with_the_setting_off_is_stopped() {
        let mut b = Bench::new();
        b.fake.standby_up();
        b.poll();
        let mut pipe = b.fake.pipe();
        b.until(|b| b.t.running.is_none() || !b.t.running.as_ref().unwrap().link.standby.load(Ordering::Relaxed));
        assert_eq!(pipe.line().as_deref(), Some(r#"{"arret":true}"#));
        b.fake.standby_down();
        let _child = b.fake.child();
        assert_eq!(b.launches(), (1, 0));
    }

    /// Le dossier d'Eyrie n'est pas à l'abri des autres comptes : ni valeur Run, ni veilleur, ni clé ; la raison est
    /// gardée pour la page des réglages.
    #[test]
    fn an_unsafe_place_refuses_the_standby() {
        let mut b = Bench::new();
        *b.fake.unsafe_place.lock().unwrap() = Some(r"C:\Outils peut être modifié par un autre compte".into());
        b.mobile.wake = true;
        b.poll();
        assert!(b.t.veille.problem.as_deref().is_some_and(|p| p.contains("Outils")), "{:?}", b.t.veille.problem);
        assert_eq!(*b.fake.run.lock().unwrap(), None);
        assert_eq!(b.config(), None);
        assert_eq!(b.t.wake_key(), None);
        let _child = b.fake.child();
        assert_eq!(b.launches(), (1, 0));
    }

    /// Une Eyrie sur le dossier par défaut, mais rangée ailleurs que le veilleur inscrit (celle de `target\debug` à côté
    /// de celle de `target\release`), ne peut pas le lancer : elle ne le désinscrit pas, ne l'arrête pas, ne coupe pas
    /// son réglage, et les téléphones gardent leur clé. L'accès coupé le suit ; le réglage coupé le retire.
    #[test]
    fn an_eyrie_that_cannot_host_the_standby_keeps_the_registered_one() {
        let mut b = Bench::new();
        let exe = b.dir.path().join(COMPANION);
        let (node, mobile) = (b.dir.path().join("tailnet"), b.dir.path().join("mobile"));
        let line = veille::run_line(&exe, &node, &mobile, "essai");
        *b.fake.run.lock().unwrap() = Some(line.clone());
        let written = veille::Config {
            v: 1,
            actif: true,
            cle: "07".repeat(32),
            instance: veille::instance_lock(b.dir.path()),
            reduite: true,
        };
        std::fs::create_dir_all(&mobile).unwrap();
        let sealed = b.fake.seal(&serde_json::to_vec(&written).unwrap()).unwrap();
        std::fs::write(mobile.join(veille::CONFIG), sealed).unwrap();
        *b.fake.eyrie.lock().unwrap() = Some(b.dir.path().join("debug").join("eyrie.exe"));
        b.fake.standby_up();
        b.mobile.wake = true;
        b.poll();
        assert!(b.t.veille.problem.as_deref().is_some_and(|p| p.contains("rangé à côté")), "{:?}", b.t.veille.problem);
        assert_eq!(b.t.veille.kept.as_deref(), Some(exe.as_path()));
        assert_eq!((*b.fake.run_writes.lock().unwrap(), b.fake.run.lock().unwrap().clone()), (0, Some(line.clone())));
        assert_eq!(b.config(), Some(written.clone()), "réglage du veilleur inchangé");
        assert_eq!(b.t.wake_key(), Some([7; 32]));

        // Le veilleur sur le nœud n'est pas arrêté : en partant, cette Eyrie lui laisse le nœud.
        let mut pipe = b.fake.pipe();
        pipe.say(RUNNING);
        b.until(|b| b.t.relays());
        assert_eq!(pipe.line().as_deref(), Some(relay_json().as_str()));
        for _ in 0..20 {
            b.poll();
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(b.launches(), (0, 0));

        // L'accès depuis le téléphone coupé : le veilleur gardé refuse de démarrer Eyrie, comme partout.
        b.mobile.enabled = false;
        b.poll();
        assert_eq!(b.config(), Some(veille::Config { actif: false, ..written }));
        assert_eq!(*b.fake.run_writes.lock().unwrap(), 0);

        b.t.stop();
        assert_eq!(pipe.line().as_deref(), Some(r#"{"relais":""}"#), "pas d'arrêt");

        // Le réglage coupé : plus voulu, il est désinscrit.
        b.mobile.wake = false;
        b.poll();
        assert_eq!(*b.fake.run.lock().unwrap(), None);
        assert_eq!(b.t.wake_key(), None);
    }

    /// Une valeur Run qui lance un compagnon d'un dossier exposé aux autres comptes n'est pas gardée.
    #[test]
    fn a_registered_standby_in_an_unsafe_place_is_not_kept() {
        let mut b = Bench::new();
        let exe = b.dir.path().join(COMPANION);
        let line = veille::run_line(&exe, &b.dir.path().join("tailnet"), &b.dir.path().join("mobile"), "essai");
        *b.fake.run.lock().unwrap() = Some(line);
        *b.fake.unsafe_place.lock().unwrap() = Some(r"C:\Outils peut être modifié par un autre compte".into());
        b.mobile.wake = true;
        b.poll();
        assert_eq!(b.t.veille.kept, None);
        assert_eq!(*b.fake.run.lock().unwrap(), None, "valeur Run retirée");
        assert_eq!(b.t.wake_key(), None);
    }

    /// Une Eyrie d'essai (autre dossier) ne touche jamais la valeur Run du compte, même pour la retirer.
    #[test]
    fn a_trial_eyrie_never_touches_the_run_value() {
        let mut b = Bench::new();
        b.t.main = false;
        *b.fake.run.lock().unwrap() = Some("celle de l'Eyrie de tous les jours".into());
        b.poll();
        b.mobile.wake = true;
        b.poll();
        assert_eq!(*b.fake.run_writes.lock().unwrap(), 0);
        assert!(b.t.veille.problem.as_deref().is_some_and(|p| p.contains("dossier par défaut")));
        assert_eq!(b.t.wake_key(), None);
    }

    /// Le veilleur lancé par Eyrie sort sans prendre le nœud : Eyrie dit pourquoi (`veille-echec.json`) et lance son
    /// compagnon à la place.
    #[test]
    fn a_standby_that_fails_is_replaced_by_the_companion() {
        let mut b = Bench::new();
        *b.fake.standby_fails.lock().unwrap() = true;
        let node = b.dir.path().join("tailnet");
        std::fs::create_dir_all(&node).unwrap();
        let failure = format!("{{\"at\":{},\"message\":\"tube de contrôle : refusé\"}}\n", now());
        std::fs::write(node.join(veille::FAILURE), failure).unwrap();
        b.mobile.wake = true;
        b.poll();
        let _child = b.fake.child();
        assert_eq!(b.launches(), (1, 1));
        assert_eq!(b.t.standby_fallback().as_deref(), Some("tube de contrôle : refusé"));
        assert!(b.fake.run.lock().unwrap().is_some(), "le veilleur reste inscrit pour la prochaine session");
    }

    /// Tailscale coupé avec un veilleur resté sur le nœud : il est arrêté, une fois.
    #[test]
    fn with_tailscale_off_an_orphan_standby_is_stopped() {
        let mut b = Bench::new();
        b.settings.enabled = false;
        b.fake.standby_up();
        b.poll();
        let mut pipe = b.fake.pipe();
        assert_eq!(pipe.line().as_deref(), Some(r#"{"arret":true}"#));
        b.fake.standby_up();
        b.poll();
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(*b.fake.attaches.lock().unwrap(), 1, "une seule fois");
        assert!(b.t.state().is_none());
    }
}
