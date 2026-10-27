//! Patronus sur le téléphone : les tâches (lancer, arrêter), la boîte de réception, le détail d'une exécution (journal,
//! marquer lu, archiver, appliquer le diff, supprimer le worktree), la pause et le budget du mois.
//!
//! Tout passe par le moteur public de Patronus (`App::engine`), verrouillé le temps de relever ce qu'il faut : les
//! écrans sont mis en forme sans le verrou, git (diff, worktree) tourne dans un fil et son résultat est reporté dans le
//! moteur à l'image suivante (`after_logic`), comme le fait la fenêtre de Patronus.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::{NaiveDate, NaiveDateTime};
use patronus_app::app::App;
use patronus_core::actions::{self, ApplyPlan};
use patronus_core::engine::Engine;
use patronus_core::exec::Context;
use patronus_core::run::{Run, RunStore, Status, now};
use patronus_core::task::Loaded;
use suite_sdk::Cx;
use suite_sdk::mobile::ui::{Act, Row, Screen, Tone};

/// Exécutions montrées dans la boîte de réception.
const INBOX: usize = 30;
/// Fin du journal montrée dans le détail d'une exécution (octets).
const JOURNAL: u64 = 4000;
/// Le coût du mois se lit dans un fichier : relu au plus toutes les 10 s.
const COST_EVERY: Duration = Duration::from_secs(10);

/// Une tâche, relevée sous le verrou du moteur.
#[derive(Clone, Debug, PartialEq)]
pub struct TaskLine {
    pub id: String,
    pub name: String,
    pub active: bool,
    /// Ce qui l'empêche de partir (fichier illisible, réglage invalide).
    pub problem: Option<String>,
    pub next: Option<NaiveDateTime>,
    pub running: bool,
    /// Un départ attend la fin de l'exécution en cours.
    pub pending: bool,
    pub last: Option<(Status, NaiveDateTime)>,
}

/// Ce que l'accueil montre, relevé sous le verrou du moteur.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Snapshot {
    pub paused: bool,
    pub month_cost: f64,
    pub budget: Option<f64>,
    pub running: usize,
    pub unread: usize,
    pub tasks: Vec<TaskLine>,
    /// Non archivées, non lues d'abord puis les plus récentes.
    pub runs: Vec<Run>,
}

impl TaskLine {
    pub fn read(e: &Engine, l: &Loaded) -> TaskLine {
        let (name, active, problem) = match &l.task {
            Ok(t) => (t.nom.clone(), t.active, l.problems.first().cloned()),
            Err(why) => (l.id.clone(), false, Some(why.clone())),
        };
        TaskLine {
            id: l.id.clone(),
            name,
            active,
            problem,
            next: e.next_run(&l.id),
            running: e.is_running(&l.id),
            pending: e.is_pending(&l.id),
            last: e.last_run(&l.id).map(|r| (r.statut, r.debut)),
        }
    }
}

impl Snapshot {
    pub fn read(e: &Engine, month_cost: f64) -> Snapshot {
        let tasks = e.loaded.iter().map(|l| TaskLine::read(e, l)).collect();
        // Les plus récentes d'abord (ordre du moteur), les non lues en tête ; seules celles montrées sont copiées.
        let shown = || e.runs.iter().filter(|r| !r.archive);
        let runs = shown().filter(|r| unread(r)).chain(shown().filter(|r| !unread(r))).take(INBOX).cloned().collect();
        Snapshot {
            paused: e.paused(),
            month_cost,
            budget: e.monthly_budget(),
            running: e.running_count(),
            unread: e.unread_count(),
            tasks,
            runs,
        }
    }
}

fn unread(r: &Run) -> bool {
    !r.lu && r.statut != Status::EnCours
}

pub fn status_tone(s: Status) -> Tone {
    match s {
        Status::EnCours => Tone::Info,
        Status::Reussie => Tone::Ok,
        Status::Echouee | Status::Impossible => Tone::Danger,
        _ => Tone::Warn,
    }
}

/// « aujourd'hui 14:05 », « hier 08:00 », « 12/10 09:30 ».
pub fn when(at: NaiveDateTime, today: NaiveDate) -> String {
    let day = at.date();
    let prefix = if day == today {
        "aujourd'hui".to_string()
    } else if today.pred_opt() == Some(day) {
        "hier".to_string()
    } else if today.succ_opt() == Some(day) {
        "demain".to_string()
    } else {
        at.format("%d/%m").to_string()
    };
    format!("{prefix} {}", at.format("%H:%M"))
}

/// « 12,34 $ ».
pub fn money(usd: f64) -> String {
    format!("{usd:.2} $").replace('.', ",")
}

fn duration(secs: i64) -> String {
    match secs {
        s if s < 60 => format!("{s} s"),
        s if s < 3600 => format!("{} min {:02} s", s / 60, s % 60),
        s => format!("{} h {:02} min", s / 3600, (s % 3600) / 60),
    }
}

fn plural(n: usize, one: &str, many: &str) -> String {
    if n == 1 { format!("1 {one}") } else { format!("{n} {many}") }
}

fn task_acts(t: &TaskLine) -> Vec<Act> {
    let mut acts = vec![Act::new(format!("lancer:{}", t.id), "Lancer").glyph("▶️").primary()];
    if t.running || t.pending {
        acts.push(Act::new(format!("arreter:{}", t.id), "Arrêter").glyph("⏹️").danger());
    }
    acts
}

fn task_state(t: &TaskLine, today: NaiveDate) -> String {
    if t.running {
        return if t.pending { "en cours, relancée à la fin".into() } else { "en cours".into() };
    }
    if let Some(p) = &t.problem {
        return format!("ne peut pas partir : {p}");
    }
    let next = match (t.active, t.next) {
        (false, _) => "inactive : à la main seulement".to_string(),
        (true, Some(at)) => format!("prochaine : {}", when(at, today)),
        (true, None) => "aucune échéance".to_string(),
    };
    match t.last {
        Some((s, at)) => format!("{next} · dernière {} ({})", s.label(), when(at, today)),
        None => next,
    }
}

fn task_row(t: &TaskLine, today: NaiveDate) -> Row {
    let tone = if t.running {
        Some(Tone::Info)
    } else if t.problem.is_some() {
        Some(Tone::Danger)
    } else {
        t.last.map(|(s, _)| status_tone(s))
    };
    let mut row = Row::new(&t.name)
        .subtitle(task_state(t, today))
        .glyph(if t.running { "◐" } else { "✦" })
        .open(format!("tache/{}", t.id));
    if let Some(tone) = tone {
        row = row.tone(tone);
    }
    if t.running {
        row = row.badge("en cours");
    }
    row.acts = task_acts(t);
    row
}

fn run_row(r: &Run, today: NaiveDate) -> Row {
    let mut sub = format!("{} · {}", r.statut.label(), when(r.debut, today));
    if let Some(c) = &r.cout {
        sub = format!("{sub} · {}", money(c.usd));
    }
    let mut row = Row::new(&r.tache_nom).subtitle(sub).tone(status_tone(r.statut)).open(format!("run/{}", r.id));
    if unread(r) {
        row = row.badge("non lu");
    }
    row
}

/// L'accueil.
pub fn home(s: &Snapshot, today: NaiveDate) -> Screen {
    let mut parts = Vec::new();
    if s.running > 0 {
        parts.push(format!("{} en cours", s.running));
    }
    if s.unread > 0 {
        parts.push(plural(s.unread, "non lu", "non lus"));
    }
    let mut screen = Screen::new("Patronus").subtitle(parts.join(" · "));
    if s.running > 0 {
        screen = screen.act(
            Act::new("tout_arreter", "Tout arrêter")
                .glyph("⏹️")
                .danger()
                .confirm("Arrêter toutes les exécutions en cours ?"),
        );
    }
    if s.paused {
        screen.note("En pause : aucune tâche ne part d'elle-même.", Tone::Warn);
        screen.buttons(vec![Act::new("reprendre", "Reprendre").glyph("▶️").primary()]);
    } else {
        screen = screen.act(Act::new("pause", "Mettre en pause").glyph("⏸️"));
    }
    match s.budget {
        Some(b) => {
            let ratio = s.month_cost / b;
            let tone = if ratio >= 1.0 {
                Tone::Danger
            } else if ratio >= 0.8 {
                Tone::Warn
            } else {
                Tone::Ok
            };
            screen.meter("Budget du mois", ratio as f32, format!("{} sur {}", money(s.month_cost), money(b)), tone);
        }
        None if s.month_cost > 0.0 => {
            screen.row(Row::new("Coût du mois").subtitle("sessions Claude").detail(money(s.month_cost)));
        }
        None => {}
    }
    screen.section("Tâches", s.tasks.len().to_string());
    if s.tasks.is_empty() {
        screen.empty("Aucune tâche : créez-en une sur le PC.");
    }
    for t in &s.tasks {
        screen.row(task_row(t, today));
    }
    screen.section(
        "Boîte de réception",
        if s.unread > 0 { plural(s.unread, "non lu", "non lus") } else { String::new() },
    );
    if s.runs.is_empty() {
        screen.empty("Aucun résultat.");
    }
    for r in &s.runs {
        screen.row(run_row(r, today));
    }
    screen
}

/// Une tâche et ses dernières exécutions.
pub fn task(t: &TaskLine, runs: &[Run], today: NaiveDate) -> Screen {
    let mut s = Screen::new(&t.name).subtitle(task_state(t, today));
    if let Some(p) = &t.problem {
        s.note(format!("Elle ne peut pas partir : {p}"), Tone::Danger);
    }
    s.buttons(task_acts(t));
    s.section("Exécutions", runs.len().to_string());
    if runs.is_empty() {
        s.empty("Pas encore d'exécution.");
    }
    for r in runs {
        s.row(run_row(r, today));
    }
    s
}

/// Une exécution : son résumé, la fin de son journal et ce qu'on peut en faire ; `job` : une action menée dans un
/// fil (en cours, ou son résultat).
pub fn run(r: &Run, journal: &str, has_diff: bool, job: Option<&(String, Tone)>, today: NaiveDate) -> Screen {
    let mut s = Screen::new(&r.tache_nom).subtitle(format!("{} · {}", r.statut.label(), when(r.debut, today)));
    if let Some((text, tone)) = job {
        s.note(text, *tone);
    }
    let mut info = vec![("Déclencheur".to_string(), r.declencheur.clone())];
    if let Some(d) = r.duration_secs() {
        info.push(("Durée".into(), duration(d)));
    }
    if let Some(c) = &r.cout {
        let estimated = if c.estime { " (estimé)" } else { "" };
        info.push(("Coût".into(), format!("{}{estimated}", money(c.usd))));
    }
    if let Some(at) = r.diff_applique {
        info.push(("Diff appliqué".into(), when(at, today)));
    }
    for (k, v) in info {
        s.row(Row::new(k).detail(v));
    }
    if let Some(d) = &r.depot {
        s.row(Row::new("Dépôt").subtitle(d.display().to_string()));
    }
    if let Some(w) = &r.worktree {
        s.row(Row::new("Worktree gardé").subtitle(w.display().to_string()));
    }
    if !r.resume.trim().is_empty() {
        s.section("Résumé", "");
        s.text(r.resume.trim(), false);
    }
    let finished = r.statut != Status::EnCours;
    let busy = job.is_some_and(|(_, t)| *t == Tone::Info);
    let mut acts = Vec::new();
    if finished {
        acts.push(if r.lu {
            Act::new("non_lu", "Marquer non lu").glyph("●")
        } else {
            Act::new("lu", "Marquer lu").glyph("✅").primary()
        });
        acts.push(Act::new("archiver", "Archiver").glyph("🗄️"));
    }
    if finished && has_diff && r.diff_applique.is_none() && r.depot.is_some() && !busy {
        acts.push(
            Act::new("diff", "Appliquer le diff")
                .glyph("⬇️")
                .confirm("Appliquer le diff dans la copie de travail du dépôt (sans commit) ?"),
        );
    }
    if finished && r.worktree.is_some() && !busy {
        acts.push(
            Act::new("worktree", "Supprimer le worktree")
                .glyph("🗑")
                .danger()
                .confirm("Supprimer le worktree gardé ? Le diff reste dans le résultat."),
        );
    }
    acts.extend(
        r.liens
            .iter()
            .filter(|l| l.starts_with("https://") || l.starts_with("http://"))
            .take(3)
            .map(|l| Act::link("Ouvrir le lien", l)),
    );
    if !acts.is_empty() {
        s.buttons(acts);
    }
    s.section("Journal", "fin");
    if journal.trim().is_empty() {
        s.empty("Journal vide.");
    } else {
        s.text(journal.trim_end(), true);
    }
    s
}

/// Fin d'une action menée dans un fil (git), à reporter dans le moteur.
enum Done {
    Applied(String, Result<ApplyPlan, String>),
    Dropped(String, Result<(), String>),
}

/// Ce que le téléphone garde entre deux appels : le coût du mois lu, les actions git en cours et leurs résultats.
#[derive(Default)]
pub struct Phone {
    cost: Option<(Instant, f64)>,
    /// Par exécution : ce qui se passe ou s'est passé (texte et ton ; `Info` : en cours).
    jobs: HashMap<String, (String, Tone)>,
    done: Vec<Done>,
}

pub type Shared = Arc<Mutex<Phone>>;

fn lock(p: &Shared) -> std::sync::MutexGuard<'_, Phone> {
    p.lock().unwrap_or_else(|e| e.into_inner())
}

/// À chaque image : reporte dans le moteur ce que les fils ont fini.
pub fn after_logic(app: &mut App, phone: &Shared) {
    let done = std::mem::take(&mut lock(phone).done);
    if done.is_empty() {
        return;
    }
    let mut e = app.engine();
    let mut notes = Vec::new();
    for d in done {
        match d {
            Done::Applied(id, Ok(plan)) => {
                e.mark_diff_applied(&id, now());
                notes.push((id, (format!("Diff appliqué : {} fichier(s), sans commit.", plan.files.len()), Tone::Ok)));
            }
            Done::Applied(id, Err(why)) => notes.push((id, (format!("Diff non appliqué : {why}"), Tone::Danger))),
            Done::Dropped(id, Ok(())) => {
                e.forget_worktree(&id);
                notes.push((id, ("Worktree supprimé ; le diff reste dans le résultat.".into(), Tone::Ok)));
            }
            Done::Dropped(id, Err(why)) => notes.push((id, (format!("Worktree non supprimé : {why}"), Tone::Danger))),
        }
    }
    drop(e);
    lock(phone).jobs.extend(notes);
}

pub fn screen(app: &mut App, phone: &Shared, path: &str) -> Option<Screen> {
    let today = now().date();
    if path.is_empty() {
        let cached = lock(phone).cost.filter(|(at, _)| at.elapsed() < COST_EVERY).map(|(_, c)| c);
        let e = app.engine();
        let cost = cached.unwrap_or_else(|| e.month_cost(now()));
        let snap = Snapshot::read(&e, cost);
        drop(e);
        if cached.is_none() {
            lock(phone).cost = Some((Instant::now(), cost));
        }
        return Some(home(&snap, today));
    }
    if let Some(id) = path.strip_prefix("tache/") {
        let e = app.engine();
        let t = TaskLine::read(&e, e.loaded.iter().find(|l| l.id == id)?);
        let runs: Vec<Run> = e.runs.iter().filter(|r| r.tache == id && !r.archive).take(10).cloned().collect();
        drop(e);
        return Some(task(&t, &runs, today));
    }
    let id = path.strip_prefix("run/")?;
    let (r, store_dir) = {
        let e = app.engine();
        (e.runs.iter().find(|r| r.id == id).cloned()?, e.store.dir.clone())
    };
    // Hors du verrou du moteur : la fin du journal et la présence du diff.
    let store = RunStore::new(store_dir);
    let journal = store.read_journal_tail(id, JOURNAL);
    let has_diff = store.diff_path(id).exists();
    let job = lock(phone).jobs.get(id).cloned();
    Some(run(&r, &journal, has_diff, job.as_ref(), today))
}

/// Lance `work` dans un fil ; son résultat est reporté au moteur par `after_logic`.
fn background(phone: &Shared, id: &str, busy: &str, work: impl FnOnce() -> Done + Send + 'static) {
    lock(phone).jobs.insert(id.to_string(), (busy.to_string(), Tone::Info));
    let phone = phone.clone();
    std::thread::spawn(move || {
        let done = work();
        lock(&phone).done.push(done);
    });
}

pub fn action(app: &mut App, phone: &Shared, path: &str, action: &str, cx: &mut Cx) -> Result<Option<String>, String> {
    let result = act(&mut app.engine(), phone, path, action);
    cx.host.repaint();
    result
}

/// Une action du téléphone, sous le verrou du moteur (git part dans un fil, hors du verrou).
fn act(e: &mut Engine, phone: &Shared, path: &str, action: &str) -> Result<Option<String>, String> {
    if let Some(id) = path.strip_prefix("run/") {
        if e.runs.iter().all(|r| r.id != id) {
            return Err("Cette exécution n'existe plus.".into());
        }
        if lock(phone).jobs.get(id).is_some_and(|(_, t)| *t == Tone::Info) && matches!(action, "diff" | "worktree") {
            return Err("Une action est déjà en cours sur cette exécution.".into());
        }
        return match action {
            "lu" | "non_lu" => {
                e.mark_read(id, action == "lu");
                Ok(Some(if action == "lu" { "Marqué lu" } else { "Marqué non lu" }.into()))
            }
            "archiver" => {
                e.archive(id);
                Ok(Some("Archivé".into()))
            }
            // git peut prendre des secondes : dans un fil, hors du verrou du moteur.
            "diff" => {
                let (repo, patch) = e.diff_job(id)?;
                let rid = id.to_string();
                background(phone, id, "Application du diff…", move || {
                    let done = actions::check_apply(&repo, &patch)
                        .and_then(|plan| actions::apply(&repo, &patch).map(|_| plan));
                    Done::Applied(rid, done)
                });
                Ok(Some("Application du diff lancée".into()))
            }
            "worktree" => {
                let (wt, repo): (PathBuf, Option<PathBuf>) = e.worktree_job(id)?;
                let rid = id.to_string();
                background(phone, id, "Suppression du worktree…", move || {
                    Done::Dropped(rid, patronus_core::claude::discard_worktree(&wt, repo.as_deref()))
                });
                Ok(Some("Suppression du worktree lancée".into()))
            }
            _ => Err("action inconnue".into()),
        };
    }
    match action.split_once(':') {
        Some(("lancer", id)) => {
            if e.task(id).is_none() {
                return Err("Cette tâche n'existe plus ou ne peut pas être lue.".into());
            }
            let queued = e.is_running(id);
            e.launch(id, Context { trigger: "téléphone".into(), ..Default::default() }, now());
            Ok(Some(if queued { "Déjà en cours : relancée à la fin" } else { "Lancée" }.into()))
        }
        Some(("arreter", id)) => {
            if !e.is_running(id)
                && let Some(owner) = e.running_elsewhere(id)
            {
                return Err(format!(
                    "Menée par un autre processus de Patronus (n° {}) : elle ne s'arrête pas d'ici.",
                    owner.pid
                ));
            }
            e.stop(id);
            Ok(Some("Arrêt demandé".into()))
        }
        _ => match action {
            "tout_arreter" => {
                let n = e.running_count();
                e.stop_all();
                Ok(Some(format!("Arrêt demandé pour {n} exécution(s)")))
            }
            "pause" | "reprendre" => {
                e.set_paused(action == "pause", now());
                Ok(Some(if action == "pause" { "En pause" } else { "Reprise" }.into()))
            }
            _ => Err("action inconnue".into()),
        },
    }
}

#[cfg(test)]
mod tests {
    use patronus_core::run::Cost;
    use suite_sdk::mobile::ui::Block;

    use super::*;

    fn at(day: u32, h: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 10, day).unwrap().and_hms_opt(h, 0, 0).unwrap()
    }

    fn sample() -> Snapshot {
        let mut done = Run::new("r1".into(), "build", "Build du soir", "08:00", at(27, 8));
        done.statut = Status::Echouee;
        done.fin = Some(at(27, 9));
        done.cout = Some(Cost { usd: 0.5, ..Default::default() });
        let mut read = Run::new("r0".into(), "build", "Build du soir", "08:00", at(26, 8));
        read.statut = Status::Reussie;
        read.lu = true;
        Snapshot {
            paused: true,
            month_cost: 42.0,
            budget: Some(50.0),
            running: 1,
            unread: 1,
            tasks: vec![
                TaskLine {
                    id: "build".into(),
                    name: "Build du soir".into(),
                    active: true,
                    problem: None,
                    next: Some(at(27, 20)),
                    running: true,
                    pending: false,
                    last: Some((Status::Echouee, at(27, 8))),
                },
                TaskLine {
                    id: "casse".into(),
                    name: "casse".into(),
                    active: false,
                    problem: Some("fichier illisible".into()),
                    next: None,
                    running: false,
                    pending: false,
                    last: None,
                },
            ],
            runs: vec![done, read],
        }
    }

    #[test]
    fn the_home_shows_the_pause_the_budget_the_tasks_and_the_inbox() {
        let s = home(&sample(), at(27, 12).date());
        assert_eq!(s.subtitle, "1 en cours · 1 non lu");
        assert!(matches!(&s.blocks[0], Block::Note { tone: Tone::Warn, .. }), "bandeau de pause");
        assert!(s.blocks.contains(&Block::Meter {
            label: "Budget du mois".into(),
            value: 0.84,
            text: "42,00 $ sur 50,00 $".into(),
            tone: Tone::Warn,
        }));
        let ids: Vec<String> = s.action_ids().into_iter().collect();
        assert_eq!(ids, ["arreter:build", "lancer:build", "lancer:casse", "reprendre", "tout_arreter"]);
        assert_eq!(s.opens().into_iter().collect::<Vec<_>>(), ["run/r0", "run/r1", "tache/build", "tache/casse"]);
        let rows: Vec<&Row> = s
            .blocks
            .iter()
            .filter_map(|b| match b {
                Block::Row(r) => Some(r),
                _ => None,
            })
            .collect();
        assert_eq!(rows[1].tone, Some(Tone::Danger), "tâche qui ne peut pas partir");
        assert_eq!((rows[2].badge.as_str(), rows[2].tone), ("non lu", Some(Tone::Danger)));
        assert_eq!(rows[2].subtitle, "échouée · aujourd'hui 08:00 · 0,50 $");
    }

    #[test]
    fn a_finished_run_offers_its_actions_and_its_journal() {
        let mut r = sample().runs.remove(0);
        r.depot = Some(PathBuf::from(r"C:\dev\ozmo"));
        r.worktree = Some(PathBuf::from(r"C:\dev\ozmo-wt"));
        r.liens = vec!["https://exemple.org/pr/1".into(), "file:///c:/x".into()];
        let today = at(27, 12).date();
        let s = run(&r, "ligne 1\nligne 2\n", true, None, today);
        assert_eq!(s.action_ids().into_iter().collect::<Vec<_>>(), ["archiver", "diff", "lu", "worktree"]);
        assert!(s.blocks.contains(&Block::Text { text: "ligne 1\nligne 2".into(), mono: true }));
        let links = s.all_acts().into_iter().filter(|a| a.url.is_some()).count();
        assert_eq!(links, 1, "seuls les liens http(s)");
        // Pendant une action git : ni diff ni worktree, et ce qui se passe est dit.
        let busy = ("Application du diff…".to_string(), Tone::Info);
        let s = run(&r, "", true, Some(&busy), today);
        assert_eq!(s.action_ids().into_iter().collect::<Vec<_>>(), ["archiver", "lu"]);
        assert!(matches!(&s.blocks[0], Block::Note { tone: Tone::Info, .. }));
    }

    #[test]
    fn dates_read_like_the_pc() {
        let today = at(27, 12).date();
        assert_eq!(when(at(27, 8), today), "aujourd'hui 08:00");
        assert_eq!(when(at(26, 8), today), "hier 08:00");
        assert_eq!(when(at(28, 8), today), "demain 08:00");
        assert_eq!(when(at(12, 9), today), "12/10 09:00");
        assert_eq!(money(12.5), "12,50 $");
    }

    #[test]
    fn phone_actions_reach_the_engine() {
        let dir = std::env::temp_dir().join(format!("suite-patronus-telephone-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut e = Engine::open_headless(&dir, now());
        let phone = Shared::default();
        assert_eq!(act(&mut e, &phone, "", "pause"), Ok(Some("En pause".into())));
        assert!(e.paused());
        assert_eq!(act(&mut e, &phone, "", "reprendre"), Ok(Some("Reprise".into())));
        assert!(!e.paused());
        assert!(act(&mut e, &phone, "", "lancer:inconnue").is_err());
        assert!(act(&mut e, &phone, "run/inconnue", "lu").is_err());
        assert!(act(&mut e, &phone, "", "autre").is_err());
        drop(e);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
