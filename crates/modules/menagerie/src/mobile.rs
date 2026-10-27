//! Menagerie sur le téléphone : les services gérés (en marche ou non, leurs ports), les ports ouverts du poste (arrêter
//! le processus), les conteneurs Docker (démarrer, arrêter, relancer, journal).
//!
//! Tout vient de ce que Menagerie expose publiquement : son état enregistré (`App::st`) et le dernier relevé de son
//! moniteur (`App::monitor`). Docker peut mettre des secondes à répondre : ses commandes tournent dans un fil, leur
//! résultat s'affiche ensuite, et le moniteur est relancé pour un relevé frais.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use menagerie::app::App;
use menagerie::docker::{self, Container, Docker};
use menagerie::monitor::Snapshot;
use menagerie::persist::ManagedSpec;
use menagerie::ports::{Category, Detected};
use suite_sdk::mobile::ui::{Act, Row, Screen, Tone};

/// Un journal de conteneur est relu au plus toutes les 5 s tant qu'il est regardé.
const LOGS_EVERY: Duration = Duration::from_secs(5);
/// Fin du journal montrée (caractères).
const LOGS_MAX: usize = 12_000;
/// Le résultat d'une action Docker reste affiché une minute dans l'accueil.
const JOB_SHOWN: Duration = Duration::from_secs(60);

/// Une action Docker : son libellé, ce qu'elle a fait.
fn verb_labels(verb: &str) -> Option<(&'static str, &'static str)> {
    match verb {
        "start" => Some(("Démarrer", "démarré")),
        "stop" => Some(("Arrêter", "arrêté")),
        "restart" => Some(("Relancer", "relancé")),
        _ => None,
    }
}

fn ports_text(ports: &[u16]) -> String {
    ports.iter().map(|p| format!(":{p}")).collect::<Vec<_>>().join(" ")
}

/// Les deux derniers éléments d'un chemin (`…\dev\contoso`).
fn short_path(p: &str) -> String {
    let parts: Vec<&str> = p.split(['\\', '/']).filter(|s| !s.is_empty()).collect();
    match parts.len() {
        0 | 1 => p.to_string(),
        n => format!("…\\{}\\{}", parts[n - 2], parts[n - 1]),
    }
}

/// Le service lancé par Menagerie tourne-t-il, et quels processus en font partie (lui et ses descendants) ?
fn managed_tree(spec: &ManagedSpec, snap: &Snapshot) -> Option<HashSet<u32>> {
    let r = spec.run?;
    let alive = snap.tree.get(&r.pid).is_some_and(|(_, created)| *created == r.created);
    alive.then(|| menagerie::ports::descendants(r.pid, r.created, &snap.tree).into_iter().map(|(p, _)| p).collect())
}

fn container_acts(c: &Container) -> Vec<Act> {
    let verbs: &[&str] = if c.running() { &["stop", "restart"] } else { &["start"] };
    verbs
        .iter()
        .filter_map(|v| {
            let (label, _) = verb_labels(v)?;
            let act = Act::new(format!("docker:{v}:{}", c.id), label);
            Some(match *v {
                "stop" => act.glyph("⏹️").danger(),
                "restart" => act.glyph("🔄"),
                _ => act.glyph("▶️").primary(),
            })
        })
        .collect()
}

/// Ce qu'une action Docker a donné, ou qu'elle est en cours.
#[derive(Clone, Debug, PartialEq)]
pub struct Job {
    pub text: String,
    /// `Info` : en cours.
    pub tone: Tone,
    pub at: Instant,
}

/// L'accueil ; `jobs` : les actions Docker récentes, par conteneur.
/// Ce que Menagerie sait d'un service géré qu'il supervise : son état (« en cours », « planté »…).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Live {
    pub label: String,
    /// Lancé (ou en relance) : « Arrêter » et « Relancer » ; sinon « Démarrer ».
    pub active: bool,
    pub failure: bool,
}

/// Les actions d'un service géré, selon son état.
fn service_acts(id: u64, name: &str, live: Option<&Live>) -> Vec<Act> {
    if live.is_some_and(|l| l.active) {
        vec![
            Act::new(format!("service:relancer:{id}"), "Relancer").glyph("🔄"),
            Act::new(format!("service:arreter:{id}"), "Arrêter")
                .glyph("⏹️")
                .danger()
                .confirm(format!("Arrêter {name} ?")),
        ]
    } else {
        vec![Act::new(format!("service:demarrer:{id}"), "Démarrer").glyph("▶️")]
    }
}

pub fn home(
    managed: &[ManagedSpec],
    live: &BTreeMap<u64, Live>,
    names: &BTreeMap<String, String>,
    snap: &Snapshot,
    jobs: &[&Job],
) -> Screen {
    let mut s = Screen::new("Menagerie");
    if snap.scans == 0 {
        s.subtitle = "Premier relevé du poste en cours…".into();
    }
    for j in jobs {
        s.note(&j.text, j.tone);
    }

    // Services gérés : en marche si le processus noté tourne toujours (même numéro, même heure de création).
    let mut owned: HashSet<u32> = HashSet::new();
    s.section("Services gérés", managed.len().to_string());
    if managed.is_empty() {
        s.empty("Aucun service géré.");
    }
    for spec in managed {
        let tree = managed_tree(spec, snap);
        let state = live.get(&spec.id);
        let mut row = Row::new(&spec.name)
            .subtitle(format!("{} · {}", spec.command, short_path(&spec.dir)))
            .open(format!("service/{}", spec.id));
        row.acts = service_acts(spec.id, &spec.name, state);
        row = match &tree {
            Some(pids) => {
                let mut ports: Vec<u16> = snap
                    .services
                    .iter()
                    .filter(|d| pids.contains(&d.proc.pid))
                    .flat_map(|d| d.ports.iter().copied())
                    .collect();
                ports.sort_unstable();
                ports.dedup();
                owned.extend(pids);
                let detail = if ports.is_empty() { "en marche".to_string() } else { ports_text(&ports) };
                row.detail(detail).tone(Tone::Ok).glyph("●")
            }
            None => match state {
                Some(l) if l.failure => row.detail(&l.label).tone(Tone::Danger).glyph("⚠️"),
                Some(l) if l.active => row.detail(&l.label).tone(Tone::Info).glyph("●"),
                _ => row.detail("arrêté").glyph("⚪"),
            },
        };
        s.row(row);
    }

    // Ports ouverts par les programmes de développement (hors services gérés, déjà montrés).
    let mut open: Vec<&Detected> =
        snap.services.iter().filter(|d| d.category == Category::Dev && !owned.contains(&d.proc.pid)).collect();
    open.sort_by_key(|d| d.ports.first().copied().unwrap_or(0));
    s.section("Ports ouverts", open.len().to_string());
    if open.is_empty() {
        s.empty(if snap.scans == 0 {
            "Relevé en cours…"
        } else {
            "Aucun port ouvert par un programme de développement."
        });
    }
    for d in open {
        let name = names.get(&d.key()).cloned().unwrap_or_else(|| d.auto_name());
        let mut sub = ports_text(&d.ports);
        if d.public {
            sub = format!("{sub} · visible du réseau");
        }
        let mut row = Row::new(name).subtitle(sub).detail(format!("n° {}", d.proc.pid)).glyph("🔁");
        if d.public {
            row = row.tone(Tone::Warn);
        }
        row = row.act(
            Act::new(format!("tuer:{}:{}", d.proc.pid, d.proc.created), "Arrêter le processus")
                .glyph("⏹️")
                .danger()
                .confirm(format!("Arrêter « {} » (n° {}) et ses sous-processus ?", d.proc.name, d.proc.pid)),
        );
        s.row(row);
    }

    // Docker.
    match &snap.docker {
        None => {
            s.section("Docker", "");
            s.empty("Relevé en cours…");
        }
        Some(Docker::Absent) => {
            s.section("Docker", "");
            s.empty("Docker n'est pas installé.");
        }
        Some(Docker::Stopped(why)) => {
            s.section("Docker", "");
            s.note(format!("Docker ne répond pas : {why}"), Tone::Warn);
        }
        Some(Docker::Error(why)) => {
            s.section("Docker", "");
            s.note(format!("Docker : {why}"), Tone::Danger);
        }
        Some(Docker::Running(list)) => {
            let up = list.iter().filter(|c| c.running()).count();
            s.section("Docker", format!("{up} / {}", list.len()));
            if list.is_empty() {
                s.empty("Aucun conteneur.");
            }
            for c in list {
                let mut sub = format!("{} · {}", c.image, c.status);
                if let Some(p) = &c.project {
                    sub = format!("{p} · {sub}");
                }
                let mut row = Row::new(&c.name)
                    .subtitle(sub)
                    .detail(ports_text(&c.ports))
                    .glyph(if c.running() { "●" } else { "○" })
                    .open(format!("docker/{}", c.id));
                if c.running() {
                    row = row.tone(Tone::Ok);
                }
                row.acts = container_acts(c);
                s.row(row);
            }
        }
    }
    s
}

/// Un conteneur : son état, ses actions, la fin de son journal (`None` : lecture en cours).
/// Un service géré : son état, ses actions, la fin de son journal.
pub fn service(spec: &ManagedSpec, live: Option<&Live>, log: &str) -> Screen {
    let label = live.map(|l| l.label.clone()).unwrap_or_else(|| "arrêté".into());
    let mut s = Screen::new(&spec.name).subtitle(label);
    s.acts = service_acts(spec.id, &spec.name, live);
    if live.is_some_and(|l| l.failure) {
        s.note("Ce service s'est arrêté sur une erreur : voir son journal.", Tone::Danger);
    }
    s.row(Row::new("Commande").subtitle(&spec.command));
    s.row(Row::new("Dossier").subtitle(&spec.dir));
    if let Some(url) = &spec.health_url {
        s.row(Row::new("Santé").subtitle(url));
    }
    s.section("Journal", "");
    if log.trim().is_empty() {
        s.empty("Rien pour l'instant.");
    } else {
        s.text(tail(log), true);
    }
    s
}

pub fn container(c: &Container, job: Option<&Job>, logs: Option<&Result<String, String>>) -> Screen {
    let mut s = Screen::new(&c.name).subtitle(format!("{} · {}", c.image, c.status));
    if let Some(j) = job {
        s.note(&j.text, j.tone);
    }
    if !c.ports.is_empty() {
        s.row(Row::new("Ports").detail(ports_text(&c.ports)));
    }
    if let Some(p) = &c.project {
        s.row(Row::new("Projet").detail(p));
    }
    if job.is_none_or(|j| j.tone != Tone::Info) {
        s.buttons(container_acts(c));
    }
    s.section("Journal", "fin");
    match logs {
        None => s.empty("Lecture du journal…"),
        Some(Err(e)) => s.note(format!("Journal illisible : {e}"), Tone::Danger),
        Some(Ok(text)) if text.trim().is_empty() => s.empty("Journal vide."),
        Some(Ok(text)) => s.text(text, true),
    };
    s
}

/// La fin d'un journal : sans couleurs ANSI, `LOGS_MAX` caractères au plus, depuis une ligne entière.
fn tail(text: &str) -> String {
    let text = menagerie::supervisor::strip_ansi(text);
    let text = text.trim_end();
    let count = text.chars().count();
    if count <= LOGS_MAX {
        return text.to_string();
    }
    let cut: String = text.chars().skip(count - LOGS_MAX).collect();
    cut.split_once('\n').map_or(cut.clone(), |(_, rest)| rest.to_string())
}

/// Ce que le téléphone garde entre deux appels, partagé avec les fils Docker.
#[derive(Default)]
pub struct Phone {
    /// Par conteneur : la dernière action et ce qu'elle a donné.
    jobs: HashMap<String, Job>,
    logs: HashMap<String, (Instant, Result<String, String>)>,
    reading: HashSet<String>,
    /// Une action Docker a fini : relancer le moniteur pour un relevé frais.
    poke: bool,
}

pub type Shared = Arc<Mutex<Phone>>;

fn lock(p: &Shared) -> MutexGuard<'_, Phone> {
    p.lock().unwrap_or_else(|e| e.into_inner())
}

fn docker_cmd(app: &App) -> String {
    let cmd = app.st.docker_cmd.trim();
    if cmd.is_empty() { "docker".into() } else { cmd.to_string() }
}

/// À chaque image : un relevé frais après une action Docker.
pub fn after_logic(app: &mut App, phone: &Shared) {
    if std::mem::take(&mut lock(phone).poke) {
        app.monitor().poke();
    }
}

/// L'état des services que Menagerie supervise.
fn live(app: &App) -> BTreeMap<u64, Live> {
    app.managed()
        .iter()
        .map(|m| {
            let active = m.is_active() || m.status == menagerie::supervisor::Status::Stopping;
            (m.spec.id, Live { label: m.status.label(), active, failure: m.status.is_failure() })
        })
        .collect()
}

pub fn screen(app: &mut App, phone: &Shared, path: &str) -> Option<Screen> {
    let snap = app.monitor().snapshot();
    if path.is_empty() {
        let p = lock(phone);
        let mut jobs: Vec<&Job> = p.jobs.values().filter(|j| j.at.elapsed() < JOB_SHOWN).collect();
        jobs.sort_by_key(|j| j.at);
        return Some(home(&app.st.managed, &live(app), &app.st.names, &snap, &jobs));
    }
    if let Some(id) = path.strip_prefix("service/") {
        let id: u64 = id.parse().ok()?;
        let spec = app.st.managed.iter().find(|s| s.id == id)?.clone();
        let log = app.managed().iter().find(|m| m.spec.id == id).map(|m| m.log_text()).unwrap_or_default();
        return Some(service(&spec, live(app).get(&id), &log));
    }
    let id = path.strip_prefix("docker/")?;
    let Some(Docker::Running(list)) = &snap.docker else { return None };
    let c = list.iter().find(|c| c.id == id)?;
    let mut p = lock(phone);
    let fresh = p.logs.get(id).is_some_and(|(at, _)| at.elapsed() < LOGS_EVERY);
    if !fresh && p.reading.insert(id.to_string()) {
        let (cmd, cid, shared) = (docker_cmd(app), id.to_string(), phone.clone());
        std::thread::spawn(move || {
            let logs = docker::logs(&cmd, &cid).map(|t| tail(&t));
            let mut p = lock(&shared);
            p.reading.remove(&cid);
            p.logs.insert(cid, (Instant::now(), logs));
        });
    }
    Some(container(c, p.jobs.get(id), p.logs.get(id).map(|(_, l)| l)))
}

pub fn action(app: &mut App, phone: &Shared, action: &str) -> Result<Option<String>, String> {
    let (kind, rest) = action.split_once(':').ok_or("action inconnue")?;
    match kind {
        "service" => {
            let (verb, id) = rest.split_once(':').ok_or("action inconnue")?;
            let id: u64 = id.parse().map_err(|_| "action inconnue")?;
            let name = app.st.managed.iter().find(|s| s.id == id).map(|s| s.name.clone()).ok_or("service inconnu")?;
            let done = match verb {
                "demarrer" => app.start_service(id).map(|_| "démarré"),
                "arreter" => app.stop_service(id).map(|_| "arrêt en cours"),
                "relancer" => app.restart_service(id).map(|_| "relancé"),
                _ => return Err("action inconnue".into()),
            }?;
            Ok(Some(format!("{name} : {done}")))
        }
        "tuer" => {
            let (pid, created) = rest.split_once(':').ok_or("action inconnue")?;
            let pid: u32 = pid.parse().map_err(|_| "action inconnue")?;
            let created: i64 = created.parse().map_err(|_| "action inconnue")?;
            menagerie::supervisor::kill_external(pid, created)?;
            app.monitor().poke();
            Ok(Some("Processus arrêté".into()))
        }
        "docker" => {
            let (verb, id) = rest.split_once(':').ok_or("action inconnue")?;
            let (label, done) = verb_labels(verb).ok_or("action inconnue")?;
            let snap = app.monitor().snapshot();
            let name = match &snap.docker {
                Some(Docker::Running(list)) => list.iter().find(|c| c.id == id).map(|c| c.name.clone()),
                _ => None,
            }
            .ok_or("Ce conteneur n'existe plus.")?;
            let mut p = lock(phone);
            if p.jobs.get(id).is_some_and(|j| j.tone == Tone::Info) {
                return Err(format!("{name} : une action est déjà en cours."));
            }
            p.jobs.insert(
                id.to_string(),
                Job { text: format!("{name} : {label}…"), tone: Tone::Info, at: Instant::now() },
            );
            p.logs.remove(id);
            drop(p);
            let (cmd, verb, cid, shared) = (docker_cmd(app), verb.to_string(), id.to_string(), phone.clone());
            let reply = format!("Envoyé à Docker : {name}");
            std::thread::spawn(move || {
                let job = match docker::action(&cmd, &verb, std::slice::from_ref(&cid)) {
                    Ok(()) => Job { text: format!("{name} : {done}"), tone: Tone::Ok, at: Instant::now() },
                    Err(e) => Job { text: format!("{name} : {e}"), tone: Tone::Danger, at: Instant::now() },
                };
                let mut p = lock(&shared);
                p.jobs.insert(cid, job);
                p.poke = true;
            });
            Ok(Some(reply))
        }
        _ => Err("action inconnue".into()),
    }
}

#[cfg(test)]
mod tests {
    use menagerie::persist::RunRecord;
    use menagerie::ports::ProcInfo;
    use suite_sdk::mobile::ui::Block;

    use super::*;

    fn detected(pid: u32, name: &str, ports: &[u16], category: Category) -> Detected {
        Detected {
            proc: ProcInfo { pid, name: name.into(), created: 1000 + pid as i64, ..Default::default() },
            ports: ports.to_vec(),
            public: false,
            folder: Some("contoso-web".into()),
            category,
        }
    }

    fn container(id: &str, name: &str, state: &str) -> Container {
        Container {
            id: id.into(),
            name: name.into(),
            image: "postgres:16".into(),
            state: state.into(),
            status: if state == "running" { "Up 3 hours".into() } else { "Exited (0)".into() },
            ports: vec![5432],
            project: None,
        }
    }

    fn spec(id: u64, name: &str, run: Option<RunRecord>) -> ManagedSpec {
        ManagedSpec {
            id,
            name: name.into(),
            dir: r"C:\dev\contoso-web".into(),
            command: "npm run dev".into(),
            policy: Default::default(),
            health_url: None,
            was_running: false,
            run,
        }
    }

    #[test]
    fn the_home_shows_managed_services_open_ports_and_containers() {
        let mut snap = Snapshot {
            services: vec![
                detected(20, "node", &[3000], Category::Dev),
                detected(30, "dotnet", &[5000], Category::Dev),
                detected(4, "System", &[445], Category::System),
            ],
            docker: Some(Docker::Running(vec![container("abc", "db", "running"), container("def", "cache", "exited")])),
            scans: 3,
            ..Default::default()
        };
        // Le service géré « web » (n° 10) a lancé node (n° 20) ; « api » a un processus noté qui ne tourne plus.
        snap.tree.insert(10, (1, 500));
        snap.tree.insert(20, (10, 1020));
        snap.tree.insert(30, (1, 1030));
        let managed = vec![
            spec(1, "web", Some(RunRecord { pid: 10, created: 500 })),
            spec(2, "api", Some(RunRecord { pid: 99, created: 7 })),
        ];
        let s = home(&managed, &BTreeMap::new(), &BTreeMap::new(), &snap, &[]);
        let rows: Vec<&Row> = s
            .blocks
            .iter()
            .filter_map(|b| match b {
                Block::Row(r) => Some(r),
                _ => None,
            })
            .collect();
        assert_eq!((rows[0].title.as_str(), rows[0].detail.as_str(), rows[0].tone), ("web", ":3000", Some(Tone::Ok)));
        assert_eq!((rows[1].title.as_str(), rows[1].detail.as_str()), ("api", "arrêté"));
        // Ports ouverts : ni node (au service géré), ni les composants de Windows.
        assert_eq!(rows[2].title, "dotnet — contoso-web");
        assert_eq!(rows.len(), 5);
        let ids: Vec<String> = s.action_ids().into_iter().collect();
        // Sans état vivant de Menagerie (essai), les services gérés proposent « Démarrer ».
        assert_eq!(
            ids,
            [
                "docker:restart:abc",
                "docker:start:def",
                "docker:stop:abc",
                "service:demarrer:1",
                "service:demarrer:2",
                "tuer:30:1030"
            ]
        );
        assert_eq!(s.opens().into_iter().collect::<Vec<_>>(), ["docker/abc", "docker/def", "service/1", "service/2"]);
    }

    #[test]
    fn a_running_service_offers_stop_and_restart_and_shows_its_log() {
        let spec = spec(7, "web", None);
        let running = Live { label: "en cours".into(), active: true, failure: false };
        let s = service(&spec, Some(&running), "prêt sur :3000\n");
        let ids: Vec<String> = s.action_ids().into_iter().collect();
        assert_eq!(ids, ["service:arreter:7", "service:relancer:7"]);
        assert!(s.blocks.iter().any(|b| matches!(b, Block::Text { text, mono: true } if text.contains(":3000"))));
        let crashed = Live { label: "planté (code 1)".into(), active: false, failure: true };
        let s = service(&spec, Some(&crashed), "");
        assert_eq!(s.action_ids().into_iter().collect::<Vec<_>>(), ["service:demarrer:7"]);
        assert!(s.blocks.iter().any(|b| matches!(b, Block::Note { tone: Tone::Danger, .. })));
    }

    #[test]
    fn a_container_shows_its_logs_and_hides_its_actions_while_one_runs() {
        let c = container("abc", "db", "running");
        let s = super::container(&c, None, Some(&Ok("prêt\n".into())));
        assert!(s.blocks.contains(&Block::Text { text: "prêt\n".into(), mono: true }));
        assert_eq!(s.action_ids().len(), 2);
        let busy = Job { text: "db : Arrêter…".into(), tone: Tone::Info, at: Instant::now() };
        let s = super::container(&c, Some(&busy), None);
        assert!(s.action_ids().is_empty());
        assert!(s.blocks.contains(&Block::Empty { text: "Lecture du journal…".into() }));
    }

    #[test]
    fn a_docker_action_runs_in_a_thread_and_its_result_is_shown() {
        let dir = std::env::temp_dir().join(format!("suite-menagerie-telephone-{}", std::process::id()));
        let snap = Snapshot {
            docker: Some(Docker::Running(vec![container("abc", "db", "running")])),
            scans: 1,
            ..Default::default()
        };
        // Une commande des conteneurs absente : rien n'est envoyé à un vrai Docker.
        let st = menagerie::persist::State { docker_cmd: "eyrie-docker-absent".into(), ..Default::default() };
        let dirs = menagerie::app::Dirs {
            data: dir.join("data"),
            logs: dir.join("logs"),
            relay: dir.join("relais.exe"),
            key: "essai".into(),
            create_key: false,
        };
        let mut app = App::with(st, Vec::new(), menagerie::monitor::Monitor::fixed(snap), dirs);
        let phone = Shared::default();
        assert!(action(&mut app, &phone, "docker:stop:inconnu").is_err());
        assert!(action(&mut app, &phone, "tuer:pas-un-nombre:1").is_err());
        assert_eq!(action(&mut app, &phone, "docker:stop:abc"), Ok(Some("Envoyé à Docker : db".into())));
        let deadline = Instant::now() + Duration::from_secs(5);
        while lock(&phone).jobs["abc"].tone == Tone::Info && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(lock(&phone).jobs["abc"].tone, Tone::Danger, "commande introuvable");
        assert!(lock(&phone).poke, "le moniteur sera relancé");
        let home = screen(&mut app, &phone, "").unwrap();
        assert!(matches!(&home.blocks[0], Block::Note { tone: Tone::Danger, text } if text.starts_with("db : ")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_long_log_keeps_its_end_without_colors() {
        let long: String = (0..2000).map(|i| format!("\x1b[32mligne {i}\x1b[0m\n")).collect();
        let t = tail(&long);
        assert!(t.chars().count() <= LOGS_MAX);
        assert!(t.ends_with("ligne 1999"));
        assert!(t.starts_with("ligne "), "depuis une ligne entière : {}", &t[..20]);
        assert!(!t.contains('\x1b'));
    }
}
