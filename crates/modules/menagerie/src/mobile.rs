//! Menagerie sur le téléphone : les services gérés (en marche ou non, leurs ports), les ports ouverts du poste (arrêter
//! le processus), les conteneurs Docker groupés par projet compose comme sur le PC (démarrer, arrêter, relancer,
//! journal), chaque partie dans un bandeau pliable, et une recherche.
//!
//! Tout vient de ce que Menagerie expose publiquement : son état enregistré (`App::st`) et le dernier relevé de son
//! moniteur (`App::monitor`). Chaque action passe par Menagerie, comme son bouton sur le PC : un service par
//! `start_service`…, un conteneur par `docker_action` (annoncée sur le PC, ses boutons attendent sa fin, son résultat
//! est suivi par `docker_job_of`), un programme du poste par `kill_program`. Seul le journal d'un conteneur est lu ici,
//! dans un fil.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use menagerie::app::{App, DockerJob};
use menagerie::docker::{self, Container, Docker};
use menagerie::monitor::Snapshot;
use menagerie::persist::ManagedSpec;
use menagerie::ports::{Category, Detected};
use suite_sdk::mobile::ui::{Act, Band, Row, Screen, Tone};

/// Un journal de conteneur est relu au plus toutes les 5 s tant qu'il est regardé.
const LOGS_EVERY: Duration = Duration::from_secs(5);
/// Fin du journal montrée (caractères).
const LOGS_MAX: usize = 12_000;
/// Le résultat d'une action Docker reste affiché une minute dans l'accueil.
const JOB_SHOWN: Duration = Duration::from_secs(60);
/// Au-delà, le bandeau des ports ouverts est plié au départ, sauf si l'un est visible du réseau.
const PORTS_OPEN: usize = 6;

/// Le libellé d'une action Docker.
fn verb_label(verb: &str) -> Option<&'static str> {
    match verb {
        "start" => Some("Démarrer"),
        "stop" => Some("Arrêter"),
        "restart" => Some("Relancer"),
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
            let label = verb_label(v)?;
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
}

/// Ce que Menagerie dit d'une action sur le conteneur `name` ; `None` : finie depuis plus d'une minute.
fn job_of(name: &str, j: &DockerJob) -> Option<Job> {
    match j {
        DockerJob::Running => Some(Job { text: format!("{name} : en cours…"), tone: Tone::Info }),
        DockerJob::Done(r, at) if at.elapsed() < JOB_SHOWN => Some(match r {
            Ok(t) => Job { text: t.clone(), tone: Tone::Ok },
            Err(t) => Job { text: t.clone(), tone: Tone::Danger },
        }),
        DockerJob::Done(..) => None,
    }
}

/// Les actions Docker de Menagerie, lancées d'ici ou du PC : les conteneurs qui en attendent la fin, et ce que les
/// dernières ont donné (de la plus ancienne à la plus récente).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Jobs {
    pub busy: HashSet<String>,
    pub done: Vec<Job>,
}

fn jobs(app: &App) -> Jobs {
    let busy = app.docker_jobs().iter().filter(|(_, j)| **j == DockerJob::Running).map(|(id, _)| id.clone()).collect();
    let mut done: Vec<(&Instant, Job)> = app
        .docker_jobs()
        .values()
        .filter_map(|j| match j {
            DockerJob::Done(_, at) => Some((at, job_of("", j)?)),
            DockerJob::Running => None,
        })
        .collect();
    done.sort_by_key(|(at, _)| **at);
    // Une action sur un projet compose finit pour tous ses conteneurs à la fois : dite une fois.
    done.dedup_by(|a, b| a.0 == b.0 && a.1 == b.1);
    Jobs { busy, done: done.into_iter().map(|(_, j)| j).collect() }
}

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

/// Une ligne de conteneur ; `busy` : une action de Menagerie est en cours sur lui (ses actions attendent sa fin,
/// comme ses boutons sur le PC).
fn container_row(c: &Container, busy: bool) -> Row {
    let mut row = Row::new(&c.name)
        .key(format!("docker/{}", c.id))
        .subtitle(format!("{} · {}", c.image, c.status))
        .detail(ports_text(&c.ports))
        .glyph(if c.running() { "●" } else { "○" })
        .open(format!("docker/{}", c.id));
    if c.running() {
        row = row.tone(Tone::Ok);
    }
    if busy {
        row.progress(None)
    } else {
        row.acts = container_acts(c);
        row
    }
}

/// Les conteneurs groupés comme sur le PC : un groupe par projet compose (par nom), les autres à la fin ; dans un
/// groupe, ceux qui tournent d'abord.
fn by_project(list: &[Container]) -> Vec<(Option<&str>, Vec<&Container>)> {
    let mut groups: Vec<(Option<&str>, Vec<&Container>)> = Vec::new();
    for c in list {
        match groups.iter_mut().find(|(p, _)| *p == c.project.as_deref()) {
            Some((_, v)) => v.push(c),
            None => groups.push((c.project.as_deref(), vec![c])),
        }
    }
    groups.sort_by_key(|(p, _)| (p.is_none(), p.map(str::to_lowercase)));
    for (_, v) in &mut groups {
        v.sort_by_key(|c| !c.running());
    }
    groups
}

/// L'accueil : les services gérés, les ports ouverts (pliés au-delà de `PORTS_OPEN`), un bandeau par projet compose
/// et ce que font les actions Docker.
pub fn home(
    managed: &[ManagedSpec],
    live: &BTreeMap<u64, Live>,
    names: &BTreeMap<String, String>,
    snap: &Snapshot,
    jobs: &Jobs,
) -> Screen {
    let mut s = Screen::new("Menagerie").search("Chercher un service, un port, un conteneur");
    if snap.scans == 0 {
        s.subtitle = "Premier relevé du poste en cours…".into();
    }
    if let Some(Docker::Running(list)) = &snap.docker {
        let busy: Vec<&str> = list.iter().filter(|c| jobs.busy.contains(&c.id)).map(|c| c.name.as_str()).collect();
        match busy.len() {
            0 => {}
            1 => {
                s.progress(format!("Docker : {} en cours…", busy[0]), None, None);
            }
            n => {
                s.progress(format!("Docker : {n} conteneurs en cours…"), None, None);
            }
        }
    }
    for j in &jobs.done {
        s.note(&j.text, j.tone);
    }

    // Services gérés : en marche si le processus noté tourne toujours (même numéro, même heure de création).
    let mut owned: HashSet<u32> = HashSet::new();
    let failed = managed.iter().filter(|m| live.get(&m.id).is_some_and(|l| l.failure)).count();
    let mut band = Band::new("services", "Services gérés").aside(managed.len().to_string()).glyph("⚙️");
    if failed > 0 {
        band = band
            .badge(if failed == 1 { "1 planté".to_string() } else { format!("{failed} plantés") })
            .tone(Tone::Danger);
    }
    s.band(band);
    if managed.is_empty() {
        s.empty("Aucun service géré.");
    }
    for spec in managed {
        let tree = managed_tree(spec, snap);
        let state = live.get(&spec.id);
        let mut row = Row::new(&spec.name)
            .key(format!("service/{}", spec.id))
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
    let public = open.iter().filter(|d| d.public).count();
    let unfold = open.len() <= PORTS_OPEN || public > 0;
    let mut band = Band::new("ports", "Ports ouverts").aside(open.len().to_string()).glyph("🔁").open(unfold);
    if public > 0 {
        band = band.badge(format!("{public} visible(s) du réseau")).tone(Tone::Warn);
    }
    s.band(band);
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
        let mut row = Row::new(name)
            .key(format!("port/{}:{}", d.proc.pid, d.proc.created))
            .subtitle(sub)
            .detail(format!("n° {}", d.proc.pid))
            .glyph("🔁");
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

    // Docker : un bandeau par projet compose, comme sur le PC.
    let docker = Band::new("docker", "Docker").glyph("🐳");
    match &snap.docker {
        None => {
            s.band(docker);
            s.empty("Relevé en cours…");
        }
        Some(Docker::Absent) => {
            s.band(docker);
            s.empty("Docker n'est pas installé.");
        }
        Some(Docker::Stopped(why)) => {
            s.band(docker.tone(Tone::Warn));
            s.note(format!("Docker ne répond pas : {why}"), Tone::Warn);
        }
        Some(Docker::Error(why)) => {
            s.band(docker.tone(Tone::Danger));
            s.note(format!("Docker : {why}"), Tone::Danger);
        }
        Some(Docker::Running(list)) if list.is_empty() => {
            s.band(docker.aside("0"));
            s.empty("Aucun conteneur.");
        }
        Some(Docker::Running(list)) => {
            let groups = by_project(list);
            let several = groups.len() > 1 || groups[0].0.is_some();
            for (project, members) in groups {
                let up = members.iter().filter(|c| c.running()).count();
                let band = match project {
                    Some(p) => Band::new(format!("projet/{p}"), p).aside("compose"),
                    None if several => Band::new("autres-conteneurs", "Autres conteneurs"),
                    None => Band::new("docker", "Docker"),
                };
                s.band(band.glyph("🐳").badge(format!("{up} / {}", members.len())).open(up > 0 || !several));
                for c in members {
                    s.row(container_row(c, jobs.busy.contains(&c.id)));
                }
            }
        }
    }
    s
}

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

/// Un conteneur : son état, ses actions, la fin de son journal (`None` : lecture en cours).
pub fn container(c: &Container, job: Option<&Job>, logs: Option<&Result<String, String>>) -> Screen {
    let mut s = Screen::new(&c.name).subtitle(format!("{} · {}", c.image, c.status));
    match job {
        Some(j) if j.tone == Tone::Info => {
            s.progress(&j.text, None, None);
        }
        Some(j) => {
            s.note(&j.text, j.tone);
        }
        None => {}
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

/// Ce que le téléphone garde entre deux appels : les journaux des conteneurs, lus dans un fil.
#[derive(Default)]
pub struct Phone {
    logs: HashMap<String, (Instant, Result<String, String>)>,
    reading: HashSet<String>,
}

pub type Shared = Arc<Mutex<Phone>>;

fn lock(p: &Shared) -> MutexGuard<'_, Phone> {
    p.lock().unwrap_or_else(|e| e.into_inner())
}

fn docker_cmd(app: &App) -> String {
    let cmd = app.st.docker_cmd.trim();
    if cmd.is_empty() { "docker".into() } else { cmd.to_string() }
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
        return Some(home(&app.st.managed, &live(app), &app.st.names, &snap, &jobs(app)));
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
    let job = app.docker_job_of(id).and_then(|j| job_of(&c.name, j));
    Some(container(c, job.as_ref(), p.logs.get(id).map(|(_, l)| l)))
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
            let snap = app.monitor().snapshot();
            let name = snap
                .services
                .iter()
                .find(|d| d.proc.pid == pid && d.proc.created == created)
                .map(|d| d.proc.name.clone())
                .unwrap_or_else(|| format!("Processus n° {pid}"));
            app.kill_program(pid, created, &name)?;
            Ok(Some(format!("{name} arrêté")))
        }
        "docker" => {
            let (verb, id) = rest.split_once(':').ok_or("action inconnue")?;
            let name = app.docker_action(verb, id)?;
            // Le journal est relu après l'action.
            lock(phone).logs.remove(id);
            Ok(Some(format!("Envoyé à Docker : {name}")))
        }
        _ => Err("action inconnue".into()),
    }
}

#[cfg(test)]
mod tests {
    use menagerie::persist::RunRecord;
    use menagerie::ports::ProcInfo;
    use suite_sdk::mobile::ui::{Block, Fold, PROGRESS_UNKNOWN};

    use super::*;

    fn rows(s: &Screen) -> Vec<&Row> {
        s.blocks
            .iter()
            .filter_map(|b| match b {
                Block::Row(r) => Some(r),
                _ => None,
            })
            .collect()
    }

    /// Les bandeaux d'un écran : (clé, pliage, pastille, ton).
    fn bands(s: &Screen) -> Vec<(&str, Fold, &str, Option<Tone>)> {
        s.blocks
            .iter()
            .filter_map(|b| match b {
                Block::Section { key, fold, badge, tone, .. } => Some((key.as_str(), *fold, badge.as_str(), *tone)),
                _ => None,
            })
            .collect()
    }

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
        let s = home(&managed, &BTreeMap::new(), &BTreeMap::new(), &snap, &Jobs::default());
        let rows = rows(&s);
        assert_eq!((rows[0].title.as_str(), rows[0].detail.as_str(), rows[0].tone), ("web", ":3000", Some(Tone::Ok)));
        assert_eq!((rows[1].title.as_str(), rows[1].detail.as_str()), ("api", "arrêté"));
        // Ports ouverts : ni node (au service géré), ni les composants de Windows.
        assert_eq!(rows[2].title, "dotnet — contoso-web");
        assert_eq!(rows.len(), 5);
        // Sans projet compose, un seul bandeau Docker, ceux qui tournent d'abord.
        assert_eq!(
            bands(&s),
            [
                ("services", Fold::Open, "", None),
                ("ports", Fold::Open, "", None),
                ("docker", Fold::Open, "1 / 2", None)
            ]
        );
        assert_eq!((rows[3].title.as_str(), rows[4].title.as_str()), ("db", "cache"));
        assert!(!s.search.is_empty());
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

    /// Comme le PC : un bandeau par projet compose (par nom), « Autres conteneurs » à la fin, ceux qui tournent
    /// d'abord ; un projet tout arrêté est plié. Une action en cours se voit (barre, et plus d'action sur la ligne).
    #[test]
    fn containers_are_grouped_by_compose_project() {
        let in_project = |id: &str, name: &str, state: &str, p: &str| Container {
            project: Some(p.into()),
            ..container(id, name, state)
        };
        let snap = Snapshot {
            docker: Some(Docker::Running(vec![
                container("seul", "seul", "running"),
                in_project("w1", "web-db", "exited", "web"),
                in_project("w2", "web-app", "running", "web"),
                in_project("a1", "api-db", "exited", "Api"),
            ])),
            scans: 1,
            ..Default::default()
        };
        let jobs = Jobs { busy: HashSet::from(["w2".to_string()]), done: Vec::new() };
        let s = home(&[], &BTreeMap::new(), &BTreeMap::new(), &snap, &jobs);
        let docker: Vec<(&str, Fold, &str)> = bands(&s).into_iter().skip(2).map(|(k, f, b, _)| (k, f, b)).collect();
        assert_eq!(
            docker,
            [
                ("projet/Api", Fold::Closed, "0 / 1"),
                ("projet/web", Fold::Open, "1 / 2"),
                ("autres-conteneurs", Fold::Open, "1 / 1")
            ]
        );
        let titles: Vec<&str> = rows(&s).iter().map(|r| r.title.as_str()).collect();
        assert_eq!(titles, ["api-db", "web-app", "web-db", "seul"]);
        let busy = rows(&s)[1];
        assert_eq!((busy.progress, busy.acts.len()), (Some(PROGRESS_UNKNOWN), 0), "une action à la fois");
        assert!(
            matches!(&s.blocks[0], Block::Note { progress: Some(PROGRESS_UNKNOWN), text, .. } if text == "Docker : web-app en cours…")
        );
    }

    /// Plus de six ports : pliés ; un port visible du réseau met le bandeau en alerte, déplié.
    #[test]
    fn many_open_ports_fold_and_a_port_seen_from_the_network_warns() {
        let mut services: Vec<Detected> =
            (0..7).map(|i| detected(100 + i, "node", &[3000 + i as u16], Category::Dev)).collect();
        let s = home(
            &[],
            &BTreeMap::new(),
            &BTreeMap::new(),
            &Snapshot { services: services.clone(), scans: 1, ..Default::default() },
            &Jobs::default(),
        );
        assert_eq!(bands(&s)[1], ("ports", Fold::Closed, "", None));
        services[6].public = true;
        let s = home(
            &[],
            &BTreeMap::new(),
            &BTreeMap::new(),
            &Snapshot { services: services.clone(), scans: 1, ..Default::default() },
            &Jobs::default(),
        );
        assert_eq!(bands(&s)[1].1, Fold::Open, "l'alerte dépliée");
        services[6].public = false;
        services.truncate(2);
        services[1].public = true;
        let s = home(
            &[],
            &BTreeMap::new(),
            &BTreeMap::new(),
            &Snapshot { services, scans: 1, ..Default::default() },
            &Jobs::default(),
        );
        assert_eq!(bands(&s)[1], ("ports", Fold::Open, "1 visible(s) du réseau", Some(Tone::Warn)));
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
        let busy = job_of("db", &DockerJob::Running).unwrap();
        let s = super::container(&c, Some(&busy), None);
        assert!(s.action_ids().is_empty());
        assert!(s.blocks.contains(&Block::Empty { text: "Lecture du journal…".into() }));
        // Ce qu'a dit Menagerie, une minute durant.
        let done = DockerJob::Done(Err("Docker (db) : refusé".into()), Instant::now());
        assert_eq!(job_of("db", &done), Some(Job { text: "Docker (db) : refusé".into(), tone: Tone::Danger }));
        if let Some(old) = Instant::now().checked_sub(Duration::from_secs(120)) {
            assert_eq!(job_of("db", &DockerJob::Done(Ok("db : arrêté".into()), old)), None);
        }
    }

    /// Une action Docker passe par Menagerie, comme son bouton : en cours chez lui, une à la fois, et l'écran du
    /// conteneur le dit sans proposer d'autre action.
    #[test]
    fn a_docker_action_goes_through_menagerie() {
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
        assert!(action(&mut app, &phone, "docker:rm:abc").is_err(), "seuls démarrer, arrêter, relancer");
        assert!(action(&mut app, &phone, "tuer:pas-un-nombre:1").is_err());
        assert_eq!(action(&mut app, &phone, "docker:stop:abc"), Ok(Some("Envoyé à Docker : db".into())));
        assert_eq!(app.docker_job_of("abc"), Some(&DockerJob::Running), "en cours dans Menagerie");
        assert!(action(&mut app, &phone, "docker:restart:abc").is_err(), "une action à la fois");
        let s = screen(&mut app, &phone, "docker/abc").unwrap();
        assert!(s.action_ids().is_empty());
        assert!(s.blocks.iter().any(|b| matches!(
            b,
            Block::Note { tone: Tone::Info, progress: Some(PROGRESS_UNKNOWN), text, .. } if text == "db : en cours…"
        )));
        let home = screen(&mut app, &phone, "").unwrap();
        assert!(home.blocks.iter().any(|b| matches!(b, Block::Note { text, .. } if text == "Docker : db en cours…")));
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
