//! Tailscale de la suite (synchro mobile) : **un seul nœud** pour la suite, au nom générique (`suite-<poste>`), état
//! dans `%LOCALAPPDATA%\EyrieSuite\Eyrie\tailnet`.
//!
//! Le compagnon d'Eyrie (`eyrie-tailnet.exe`, en Go avec tsnet, dossier `compagnon/`) rejoint le tailnet et relaie
//! les connexions du téléphone (port 4747) vers le pont mobile d'Eyrie : Eyrie lui donne l'adresse locale du pont et
//! un jeton par son entrée standard, une ligne JSON à chaque changement (`{"relais":"127.0.0.1:…","jeton":"…"}`, ou
//! `{"relais":""}` : plus rien). Il annonce son état par lignes JSON sur sa sortie et s'arrête quand Eyrie ferme son
//! entrée. Le compagnon de Fawkes (`fawkes-tailnet.exe`) sert de secours : le nœud seul, sans relais.

use std::io::{BufRead, BufReader, Write};
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eframe::egui::{self, Color32};
use suite_fawkes::tailnet::{self as fawkes_tailnet, TailnetState};

use crate::i18n::tr;
use crate::persist::TailnetSettings;

/// Le compagnon d'Eyrie, qui sait relayer le téléphone.
pub const COMPANION: &str = "eyrie-tailnet.exe";

struct Running {
    /// `None` : le compagnon n'a pas pu être lancé (son état dit pourquoi).
    child: Option<Child>,
    stdin: Option<ChildStdin>,
    state: Arc<Mutex<TailnetState>>,
    hostname: String,
    /// Le compagnon d'Eyrie (relais) plutôt que celui de Fawkes.
    own: bool,
    /// Dernière consigne de relais écrite.
    relay: Option<Option<(String, String)>>,
}

pub struct Tailnet {
    running: Option<Running>,
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
    pub fn new(_dir: &Path) -> Self {
        Tailnet { running: None }
    }

    /// Démarre, arrête ou redémarre le compagnon selon les réglages ; lui donne la consigne de relais du pont mobile.
    pub fn poll(
        &mut self,
        settings: &TailnetSettings,
        dir: &Path,
        ctx: &egui::Context,
        relay: Option<(String, String)>,
    ) {
        let wanted = settings.enabled.then(|| hostname(settings));
        let current = self.running.as_ref().map(|r| r.hostname.clone());
        if wanted != current {
            self.stop();
            if let Some(h) = wanted {
                let state_dir = node_dir(dir);
                let _ = std::fs::create_dir_all(&state_dir);
                self.running = Some(start(&state_dir, &h, ctx.clone()));
            }
        }
        if let Some(r) = &mut self.running
            && r.own
            && r.relay.as_ref() != Some(&relay)
        {
            let line = relay_line(&relay);
            let written = r.stdin.as_mut().is_some_and(|s| writeln!(s, "{line}").and_then(|_| s.flush()).is_ok());
            if written {
                r.relay = Some(relay);
            }
        }
    }

    pub fn state(&self) -> Option<TailnetState> {
        self.running.as_ref().map(|r| r.state.lock().unwrap_or_else(|e| e.into_inner()).clone())
    }

    /// Le compagnon en marche sait relayer le téléphone, et relaie.
    pub fn relays(&self) -> bool {
        self.running.as_ref().is_some_and(|r| r.own && matches!(r.relay, Some(Some(_))))
    }

    /// Le compagnon en marche est celui d'Eyrie.
    pub fn own_companion(&self) -> Option<bool> {
        self.running.as_ref().map(|r| r.own)
    }

    /// Ferme l'entrée du compagnon, qui se déconnecte proprement ; l'arrête de force au bout de 3 s.
    pub fn stop(&mut self) {
        let Some(mut r) = self.running.take() else { return };
        drop(r.stdin.take());
        let Some(mut child) = r.child.take() else { return };
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            if matches!(child.try_wait(), Ok(Some(_))) {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = child.kill();
        let _ = child.wait();
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

fn start(dir: &Path, hostname: &str, ctx: egui::Context) -> Running {
    let state = Arc::new(Mutex::new(TailnetState::Starting));
    let fail = |msg: String| {
        *state.lock().unwrap_or_else(|e| e.into_inner()) = TailnetState::Failed(msg);
    };
    let Some(exe) = find_companion() else {
        fail(format!("{COMPANION} introuvable : compilez-le avec tools\\compagnon.ps1."));
        return Running { child: None, stdin: None, state, hostname: hostname.to_string(), own: false, relay: None };
    };
    let own = exe.file_name().is_some_and(|n| n.eq_ignore_ascii_case(COMPANION));
    let mut cmd = Command::new(&exe);
    cmd.arg("--dir").arg(dir).args(["--hostname", hostname, "--version", env!("CARGO_PKG_VERSION")]);
    if own {
        cmd.args(["--port", &suite_mobile::PORT.to_string()]);
    }
    cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            fail(format!("lancement de {} : {e}", exe.display()));
            return Running { child: None, stdin: None, state, hostname: hostname.to_string(), own, relay: None };
        }
    };
    if let Some(out) = child.stdout.take() {
        let log = dir.join("journal.txt");
        let state = state.clone();
        let _ = std::thread::Builder::new().name("tailnet".into()).spawn(move || {
            for line in BufReader::new(out).lines() {
                let Ok(line) = line else { break };
                if let Some(s) = fawkes_tailnet::parse_line(&line) {
                    *state.lock().unwrap_or_else(|e| e.into_inner()) = s;
                    ctx.request_repaint();
                }
            }
            // Sortie fermée : le compagnon s'est arrêté (ou Eyrie l'a arrêté).
            let mut s = state.lock().unwrap_or_else(|e| e.into_inner());
            if !matches!(*s, TailnetState::Failed(_)) {
                *s = TailnetState::Failed(format!("le compagnon s'est arrêté (détails : {})", log.display()));
            }
            ctx.request_repaint();
        });
    }
    let stdin = child.stdin.take();
    Running { child: Some(child), stdin, state, hostname: hostname.to_string(), own, relay: None }
}

impl Drop for Tailnet {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_node_name_is_generic() {
        let h = hostname(&TailnetSettings { enabled: true, hostname: String::new() });
        assert!(h.starts_with("suite"), "{h}");
        assert_eq!(hostname(&TailnetSettings { enabled: true, hostname: "Mon Poste".into() }), "mon-poste");
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
}
