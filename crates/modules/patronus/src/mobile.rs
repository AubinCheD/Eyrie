//! Patronus sur le téléphone : l'accueil en trois bandeaux (ce qui tourne, la boîte de réception, les tâches), le détail
//! d'une tâche (lancer, arrêter) et d'une exécution (journal, marquer lu, archiver, appliquer le diff, supprimer le
//! worktree), la pause et le budget du mois ; un résultat se marque lu ou s'archive depuis son menu, et toute la boîte
//! se marque lue d'un geste, comme le bouton du PC.
//!
//! Tout passe par Patronus : son moteur public (`App::engine`), verrouillé le temps de relever ce qu'il faut (les
//! écrans sont mis en forme sans le verrou), et, pour appliquer le diff ou supprimer le worktree, les mêmes actions que
//! les boutons d'un résultat (`App::apply_diff`, `App::drop_worktree`) : la fenêtre de Patronus montre que git
//! travaille, annonce le résultat, et le téléphone le lit par `App::run_job`.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::{NaiveDate, NaiveDateTime};
use patronus_app::app::{App, RunJob};
use patronus_core::engine::Engine;
use patronus_core::exec::Context;
use patronus_core::run::{Run, RunStore, Status, now};
use patronus_core::task::Loaded;
use suite_sdk::Cx;
use suite_sdk::mobile::ui::{Act, Band, Row, Screen, Tone};

/// Exécutions montrées dans la boîte de réception.
const INBOX: usize = 30;
/// Fin du journal montrée dans le détail d'une exécution (octets).
const JOURNAL: u64 = 4000;
/// Le coût du mois se lit dans un fichier : relu au plus toutes les 10 s.
const COST_EVERY: Duration = Duration::from_secs(10);
/// Le résultat d'une action sur un résultat reste affiché une minute.
const JOB_SHOWN: Duration = Duration::from_secs(60);
/// Au-delà, le bandeau des tâches est plié au départ, sauf si l'une ne peut pas partir.
const TASKS_OPEN: usize = 6;

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
    /// Début de l'exécution en cours.
    pub since: Option<NaiveDateTime>,
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
            since: e.running_run_id(&l.id).and_then(|run| e.runs.iter().find(|r| r.id == run)).map(|r| r.debut),
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

/// Le glyphe d'un état d'exécution : réussie, en échec, en cours, arrêtée.
pub fn status_glyph(s: Status) -> &'static str {
    match s {
        Status::EnCours => "◐",
        Status::Reussie => "✓",
        Status::Echouee | Status::Impossible => "✗",
        Status::Arretee
        | Status::DureeDepassee
        | Status::ToursAtteints
        | Status::BudgetAtteint
        | Status::Interrompue => "⏹",
        Status::Manquee => "⏭",
    }
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

/// « depuis 4 min ».
fn since(start: NaiveDateTime, now: NaiveDateTime) -> String {
    match (now - start).num_seconds().max(0) {
        s if s < 60 => "depuis moins d'une minute".into(),
        s if s < 3600 => format!("depuis {} min", s / 60),
        s => format!("depuis {} h {:02}", s / 3600, (s % 3600) / 60),
    }
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

/// « Arrêter », confirmé partout où il est proposé (bouton visible, menu, écran de la tâche).
fn stop_act(t: &TaskLine) -> Act {
    Act::new(format!("arreter:{}", t.id), "Arrêter").glyph("⏹️").danger().confirm(format!("Arrêter « {} » ?", t.name))
}

fn task_acts(t: &TaskLine) -> Vec<Act> {
    let mut acts = vec![
        Act::new(format!("lancer:{}", t.id), "Lancer").glyph("▶️").primary(),
        // La note part dans le journal et à la fin de la consigne d'une session Claude (comme un déclencheur).
        Act::new(format!("lancer-note:{}", t.id), "Lancer avec une consigne…")
            .glyph("📝")
            .input("Ajoutée à la consigne de la tâche", "Ex. : regarde surtout le module de facturation")
            .multiline(),
    ];
    if t.running || t.pending {
        acts.push(stop_act(t));
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
        .key(format!("tache/{}", t.id))
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

/// Une tâche qui tourne, dans « En cours » : depuis quand, et « Arrêter » à portée de pouce.
fn running_row(t: &TaskLine, now: NaiveDateTime) -> Row {
    let mut sub = t.since.map(|at| since(at, now)).unwrap_or_else(|| "en cours".into());
    if t.pending {
        sub = format!("{sub} · relancée à la fin");
    }
    let mut row = Row::new(&t.name)
        .key(format!("cours/{}", t.id))
        .subtitle(sub)
        .glyph("◐")
        .tone(Tone::Info)
        .progress(None)
        .open(format!("tache/{}", t.id))
        .trailing(stop_act(t));
    row.acts = task_acts(t);
    row
}

/// L'ordre des tâches : celles qui ne peuvent pas partir, puis la prochaine échéance, les inactives à la fin.
fn task_order(t: &TaskLine) -> (u8, Option<NaiveDateTime>, String) {
    let rank = match (&t.problem, t.active, t.next) {
        (Some(_), ..) => 0,
        (None, true, Some(_)) => 1,
        (None, true, None) => 2,
        (None, false, _) => 3,
    };
    (rank, t.next, t.name.to_lowercase())
}

/// Un résultat : son état en glyphe, et son menu (lu ou non lu, archiver) pour trier la boîte sans l'ouvrir.
fn run_row(r: &Run, today: NaiveDate) -> Row {
    let mut sub = format!("{} · {}", r.statut.label(), when(r.debut, today));
    if let Some(c) = &r.cout {
        sub = format!("{sub} · {}", money(c.usd));
    }
    let mut row = Row::new(&r.tache_nom)
        .key(format!("run/{}", r.id))
        .subtitle(sub)
        .glyph(status_glyph(r.statut))
        .tone(status_tone(r.statut))
        .open(format!("run/{}", r.id));
    if r.statut == Status::EnCours {
        return row;
    }
    row = if r.lu {
        row.act(Act::new(format!("non_lu:{}", r.id), "Marquer non lu").glyph("●"))
    } else {
        row.badge("non lu").act(Act::new(format!("lu:{}", r.id), "Marquer lu").glyph("✅"))
    };
    row.act(Act::new(format!("archiver:{}", r.id), "Archiver").glyph("🗄️"))
}

/// L'accueil : ce qui tourne, la boîte de réception, puis les tâches (pliées au-delà de `TASKS_OPEN`, sauf si l'une
/// ne peut pas partir : l'alerte se voit dépliée).
pub fn home(s: &Snapshot, now: NaiveDateTime) -> Screen {
    let today = now.date();
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

    let running: Vec<&TaskLine> = s.tasks.iter().filter(|t| t.running).collect();
    if !running.is_empty() {
        screen.band(Band::new("en-cours", "En cours").aside(running.len().to_string()).glyph("◐").tone(Tone::Info));
        for t in running {
            screen.row(running_row(t, now));
        }
    }

    let mut inbox = Band::new("boite", "Boîte de réception").glyph("📥");
    if s.unread > 0 {
        inbox =
            inbox.badge(plural(s.unread, "non lu", "non lus")).act(Act::new("tout_lu", "Tout marquer lu").glyph("✅"));
    }
    screen.band(inbox);
    if s.runs.is_empty() {
        screen.empty("Aucun résultat.");
    }
    for r in &s.runs {
        screen.row(run_row(r, today));
    }

    let mut tasks: Vec<&TaskLine> = s.tasks.iter().collect();
    tasks.sort_by_key(|t| task_order(t));
    let blocked = tasks.iter().filter(|t| t.problem.is_some()).count();
    let open = s.tasks.len() <= TASKS_OPEN || blocked > 0;
    let mut band = Band::new("taches", "Tâches").aside(s.tasks.len().to_string()).glyph("✦").open(open);
    if blocked > 0 {
        band = band.badge(plural(blocked, "bloquée", "bloquées")).tone(Tone::Danger);
    }
    screen.band(band);
    if tasks.is_empty() {
        screen.empty("Aucune tâche : créez-en une sur le PC.");
    }
    for t in tasks {
        screen.row(task_row(t, today));
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
    // Le résumé dit l'essentiel : le journal se déplie à la demande.
    s.band(Band::new("journal", "Journal").aside("fin").glyph("📜").open(r.resume.trim().is_empty()));
    if journal.trim().is_empty() {
        s.empty("Journal vide.");
    } else {
        s.text(journal.trim_end(), true);
    }
    s
}

/// Ce que le téléphone garde entre deux appels : le coût du mois lu.
#[derive(Default)]
pub struct Phone {
    cost: Option<(Instant, f64)>,
}

pub type Shared = Arc<Mutex<Phone>>;

fn lock(p: &Shared) -> std::sync::MutexGuard<'_, Phone> {
    p.lock().unwrap_or_else(|e| e.into_inner())
}

/// Ce que Patronus dit de l'action sur un résultat (texte et ton ; `Info` : en cours) ; `None` : finie depuis plus
/// d'une minute.
fn job_note(j: &RunJob) -> Option<(String, Tone)> {
    match j {
        RunJob::Running(text) => Some((text.clone(), Tone::Info)),
        RunJob::Done(r, at) if at.elapsed() < JOB_SHOWN => Some(match r {
            Ok(t) => (t.clone(), Tone::Ok),
            Err(t) => (t.clone(), Tone::Danger),
        }),
        RunJob::Done(..) => None,
    }
}

pub fn screen(app: &mut App, phone: &Shared, path: &str) -> Option<Screen> {
    let now = now();
    let today = now.date();
    if path.is_empty() {
        let cached = lock(phone).cost.filter(|(at, _)| at.elapsed() < COST_EVERY).map(|(_, c)| c);
        let e = app.engine();
        let cost = cached.unwrap_or_else(|| e.month_cost(now));
        let snap = Snapshot::read(&e, cost);
        drop(e);
        if cached.is_none() {
            lock(phone).cost = Some((Instant::now(), cost));
        }
        return Some(home(&snap, now));
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
    let job = app.run_job(id).and_then(job_note);
    Some(run(&r, &journal, has_diff, job.as_ref(), today))
}

pub fn action(
    app: &mut App,
    path: &str,
    action: &str,
    input: Option<&str>,
    cx: &mut Cx,
) -> Result<Option<String>, String> {
    let result = match (path.strip_prefix("run/"), action) {
        (Some(id), "diff" | "worktree") => result_action(app, id, action),
        _ => act_with(&mut app.engine(), path, action, input),
    };
    cx.host.repaint();
    result
}

/// Appliquer le diff, supprimer le worktree : par Patronus, comme leurs boutons ; vérifié d'abord, pour répondre tout
/// de suite si c'est impossible.
fn result_action(app: &mut App, id: &str, action: &str) -> Result<Option<String>, String> {
    {
        let e = app.engine();
        if e.runs.iter().all(|r| r.id != id) {
            return Err("Cette exécution n'existe plus.".into());
        }
        if action == "diff" {
            e.diff_job(id)?;
        } else {
            e.worktree_job(id)?;
        }
    }
    if action == "diff" {
        app.apply_diff(id)?;
        Ok(Some("Application du diff lancée".into()))
    } else {
        app.drop_worktree(id)?;
        Ok(Some("Suppression du worktree lancée".into()))
    }
}

/// Une action du téléphone sur le moteur, sous son verrou.
#[cfg(test)]
fn act(e: &mut Engine, path: &str, action: &str) -> Result<Option<String>, String> {
    act_with(e, path, action, None)
}

fn act_with(e: &mut Engine, path: &str, action: &str, input: Option<&str>) -> Result<Option<String>, String> {
    if let Some(id) = path.strip_prefix("run/") {
        if e.runs.iter().all(|r| r.id != id) {
            return Err("Cette exécution n'existe plus.".into());
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
            _ => Err("action inconnue".into()),
        };
    }
    match action.split_once(':') {
        Some((verb @ ("lu" | "non_lu" | "archiver"), id)) => {
            if e.runs.iter().all(|r| r.id != id) {
                return Err("Cette exécution n'existe plus.".into());
            }
            if verb == "archiver" {
                e.archive(id);
                return Ok(Some("Archivé".into()));
            }
            e.mark_read(id, verb == "lu");
            Ok(Some(if verb == "lu" { "Marqué lu" } else { "Marqué non lu" }.into()))
        }
        Some((verb @ ("lancer" | "lancer-note"), id)) => {
            if e.task(id).is_none() {
                return Err("Cette tâche n'existe plus ou ne peut pas être lue.".into());
            }
            let queued = e.is_running(id);
            let notes =
                if verb == "lancer-note" { input.unwrap_or_default().trim().to_string() } else { String::new() };
            e.launch(id, Context { trigger: "téléphone".into(), notes, ..Default::default() }, now());
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
            "tout_lu" => {
                // Comme « Tout marquer comme lu » sur le PC : une boucle sur `mark_read`.
                let ids: Vec<String> =
                    e.runs.iter().filter(|r| unread(r) && !r.archive).map(|r| r.id.clone()).collect();
                for id in &ids {
                    e.mark_read(id, true);
                }
                Ok(Some(match ids.len() {
                    0 => "Rien à marquer".into(),
                    1 => "1 résultat marqué lu".into(),
                    n => format!("{n} résultats marqués lus"),
                }))
            }
            "tout_arreter" => {
                let n = e.running_count();
                e.stop_all();
                Ok(Some(match n {
                    0 => "Rien ne tourne".into(),
                    n => format!("Arrêt demandé pour {}", plural(n, "exécution", "exécutions")),
                }))
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
    use std::path::PathBuf;

    use patronus_core::run::Cost;
    use suite_sdk::mobile::ui::{Block, Fold};

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
                    since: Some(at(27, 12) - chrono::TimeDelta::minutes(4)),
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
                    since: None,
                    pending: false,
                    last: None,
                },
            ],
            runs: vec![done, read],
        }
    }

    fn rows(s: &Screen) -> Vec<&Row> {
        s.blocks
            .iter()
            .filter_map(|b| match b {
                Block::Row(r) => Some(r),
                _ => None,
            })
            .collect()
    }

    /// Les bandeaux d'un écran : (clé, pliage, pastille).
    fn bands(s: &Screen) -> Vec<(&str, Fold, &str)> {
        s.blocks
            .iter()
            .filter_map(|b| match b {
                Block::Section { key, fold, badge, .. } => Some((key.as_str(), *fold, badge.as_str())),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn the_home_shows_what_runs_the_inbox_and_the_tasks_in_bands() {
        let s = home(&sample(), at(27, 12));
        assert_eq!(s.subtitle, "1 en cours · 1 non lu");
        assert!(matches!(&s.blocks[0], Block::Note { tone: Tone::Warn, .. }), "bandeau de pause");
        assert!(s.blocks.contains(&Block::Meter {
            label: "Budget du mois".into(),
            value: 0.84,
            text: "42,00 $ sur 50,00 $".into(),
            tone: Tone::Warn,
        }));
        assert_eq!(
            bands(&s),
            [("en-cours", Fold::Open, ""), ("boite", Fold::Open, "1 non lu"), ("taches", Fold::Open, "1 bloquée")]
        );
        let ids: Vec<String> = s.action_ids().into_iter().collect();
        assert_eq!(
            ids,
            [
                "archiver:r0",
                "archiver:r1",
                "arreter:build",
                "lancer-note:build",
                "lancer-note:casse",
                "lancer:build",
                "lancer:casse",
                "lu:r1",
                "non_lu:r0",
                "reprendre",
                "tout_arreter",
                "tout_lu"
            ]
        );
        assert_eq!(s.opens().into_iter().collect::<Vec<_>>(), ["run/r0", "run/r1", "tache/build", "tache/casse"]);
        let rows = rows(&s);
        // En cours : depuis quand, une barre qui tourne, « Arrêter » visible (avec confirmation).
        assert_eq!((rows[0].key.as_str(), rows[0].subtitle.as_str()), ("cours/build", "depuis 4 min"));
        assert_eq!(rows[0].progress, Some(suite_sdk::mobile::ui::PROGRESS_UNKNOWN));
        assert!(rows[0].trailing.as_ref().is_some_and(|a| a.id == "arreter:build" && a.confirm.is_some()));
        // … et confirmé aussi depuis le menu de la ligne.
        let stop = rows[0].acts.iter().find(|a| a.id == "arreter:build");
        assert_eq!(stop.and_then(|a| a.confirm.as_deref()), Some("Arrêter « Build du soir » ?"));
        // La boîte : l'état en glyphe, non lus marqués.
        assert_eq!((rows[1].badge.as_str(), rows[1].tone, rows[1].glyph.as_str()), ("non lu", Some(Tone::Danger), "✗"));
        assert_eq!(rows[1].subtitle, "échouée · aujourd'hui 08:00 · 0,50 $");
        assert_eq!((rows[2].glyph.as_str(), rows[2].badge.as_str()), ("✓", ""));
        // Les tâches : celle qui ne peut pas partir d'abord.
        assert_eq!((rows[3].key.as_str(), rows[3].tone), ("tache/casse", Some(Tone::Danger)));
        assert_eq!(rows[4].key, "tache/build");
    }

    /// Les tâches : bloquées, puis par prochaine échéance, inactives à la fin ; pliées au-delà de six, sauf si l'une ne
    /// peut pas partir. Rien ne tourne : pas de bandeau « En cours », et une boîte lue n'offre pas « Tout marquer lu ».
    #[test]
    fn many_tasks_are_sorted_and_folded() {
        let task = |id: &str, active: bool, next: Option<NaiveDateTime>| TaskLine {
            id: id.into(),
            name: id.into(),
            active,
            problem: None,
            next,
            running: false,
            since: None,
            pending: false,
            last: None,
        };
        let mut tasks = vec![
            task("inactive", false, None),
            task("demain", true, Some(at(28, 8))),
            task("sans-echeance", true, None),
            task("ce-soir", true, Some(at(27, 20))),
        ];
        tasks.push(TaskLine { problem: Some("dossier absent".into()), ..task("bloquee", true, Some(at(27, 13))) });
        tasks.extend((0..3).map(|i| task(&format!("z{i}"), false, None)));
        let s = home(&Snapshot { tasks: tasks.clone(), ..Default::default() }, at(27, 12));
        assert_eq!(bands(&s), [("boite", Fold::Open, ""), ("taches", Fold::Open, "1 bloquée")], "l'alerte dépliée");
        assert!(!s.action_ids().contains("tout_lu"));
        let order: Vec<&str> = rows(&s).iter().map(|r| r.title.as_str()).collect();
        assert_eq!(order, ["bloquee", "ce-soir", "demain", "sans-echeance", "inactive", "z0", "z1", "z2"]);
        tasks.retain(|t| t.problem.is_none());
        tasks.push(task("z3", false, None));
        let s = home(&Snapshot { tasks, ..Default::default() }, at(27, 12));
        assert_eq!(bands(&s)[1], ("taches", Fold::Closed, ""), "rien d'alarmant : plié");
    }

    #[test]
    fn a_run_tells_how_long_it_has_been_running() {
        assert_eq!(since(at(27, 12), at(27, 12)), "depuis moins d'une minute");
        assert_eq!(since(at(27, 11), at(27, 12) - chrono::TimeDelta::minutes(1)), "depuis 59 min");
        assert_eq!(since(at(27, 9), at(27, 12) + chrono::TimeDelta::minutes(5)), "depuis 3 h 05");
        assert_eq!(status_glyph(Status::DureeDepassee), "⏹");
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
        assert_eq!(bands(&s), [("journal", Fold::Open, "")], "sans résumé, le journal est ouvert");
        r.resume = "Le build casse sur le module de facturation.".into();
        let s = run(&r, "ligne 1\n", true, None, today);
        assert_eq!(
            bands(&s),
            [("", Fold::No, ""), ("journal", Fold::Closed, "")],
            "le résumé d'abord, le journal plié"
        );
        let links = s.all_acts().into_iter().filter(|a| a.url.is_some()).count();
        assert_eq!(links, 1, "seuls les liens http(s)");
        // Pendant une action git : ni diff ni worktree, et ce qui se passe est dit.
        let busy = ("Application du diff…".to_string(), Tone::Info);
        let s = run(&r, "", true, Some(&busy), today);
        assert_eq!(s.action_ids().into_iter().collect::<Vec<_>>(), ["archiver", "lu"]);
        assert!(matches!(&s.blocks[0], Block::Note { tone: Tone::Info, .. }));
        // Ce que Patronus en dit : en cours, puis son résultat une minute durant.
        assert_eq!(job_note(&RunJob::Running("Application du diff…".into())), Some(busy));
        let failed = RunJob::Done(Err("Diff non appliqué : conflit".into()), Instant::now());
        assert_eq!(job_note(&failed), Some(("Diff non appliqué : conflit".into(), Tone::Danger)));
        if let Some(old) = Instant::now().checked_sub(Duration::from_secs(120)) {
            assert_eq!(job_note(&RunJob::Done(Ok("Worktree supprimé".into()), old)), None);
        }
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
        assert_eq!(act(&mut e, "", "tout_arreter"), Ok(Some("Rien ne tourne".into())));
        assert_eq!(act(&mut e, "", "pause"), Ok(Some("En pause".into())));
        assert!(e.paused());
        assert_eq!(act(&mut e, "", "reprendre"), Ok(Some("Reprise".into())));
        assert!(!e.paused());
        assert!(act(&mut e, "", "lancer:inconnue").is_err());
        assert!(act(&mut e, "run/inconnue", "lu").is_err());
        assert!(act(&mut e, "", "autre").is_err());
        // Le menu d'un résultat et « Tout marquer lu », comme les boutons du PC.
        assert!(act(&mut e, "", "lu:inconnue").is_err());
        assert_eq!(act(&mut e, "", "tout_lu"), Ok(Some("Rien à marquer".into())));
        for (id, statut) in [("r1", Status::Echouee), ("r2", Status::Reussie), ("r3", Status::EnCours)] {
            let mut r = Run::new(id.into(), "build", "Build du soir", "08:00", at(27, 8));
            r.statut = statut;
            e.runs.push(r);
        }
        assert_eq!(e.unread_count(), 2);
        assert_eq!(act(&mut e, "", "lu:r1"), Ok(Some("Marqué lu".into())));
        assert_eq!(e.unread_count(), 1);
        assert_eq!(act(&mut e, "tache/build", "non_lu:r1"), Ok(Some("Marqué non lu".into())));
        assert_eq!(act(&mut e, "", "tout_lu"), Ok(Some("2 résultats marqués lus".into())));
        assert_eq!(e.unread_count(), 0);
        assert!(!e.runs.iter().find(|r| r.id == "r3").unwrap().lu, "une exécution en cours n'est pas un résultat");
        assert_eq!(act(&mut e, "", "archiver:r2"), Ok(Some("Archivé".into())));
        assert!(e.runs.iter().find(|r| r.id == "r2").unwrap().archive);
        drop(e);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
