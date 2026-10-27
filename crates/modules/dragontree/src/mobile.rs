//! DragonTree sur le téléphone : les dépôts de ses réglages (onglets ouverts, favoris, récents), leur branche, leur
//! avance ou retard et leurs modifications ; fetch, pull, push et « ouvrir sur le PC » depuis la liste ; et, dans un
//! dépôt, un client git presque complet (`depot.rs` : modifications et diff, indexer, valider, historique, branches,
//! fusion, rebase, conflits, remises, étiquettes, worktrees, opération en cours).
//!
//! **Tout passe par DragonTree, sur le PC.** Un dépôt regardé depuis le téléphone est ouvert dans un onglet de
//! DragonTree (en fond : l'onglet affiché ne change pas). Ce que montre le téléphone est ce que DragonTree en a lu
//! (état, branches, historique, remises, worktrees, opération en cours), et chaque opération est lancée par DragonTree
//! comme son bouton (`DragonTreeApp::queue_operation`) : sa file d'opérations, le compte associé au dépôt, sa
//! progression et ses notifications sur le PC ; une fusion arrêtée sur un conflit s'affiche dans son bandeau et se
//! résout ici ou là-bas. Ce qui tourne dans DragonTree, lancé d'ici ou du PC, et ce qu'il a donné, est montré ici. Le
//! téléphone ne voit jamais git ni les identifiants.
//!
//! Lu à part, dans un fil, avec le dépôt ouvert par DragonTree : ce qu'il ne garde pas (le diff d'un fichier, le
//! détail d'un commit, le dernier message pour corriger le commit) ; et l'état rapide des dépôts de la liste qui ne
//! sont pas ouverts dans un onglet (lecture seule). Un dépôt est désigné par une empreinte de son chemin
//! (`depot/<clé>`) : pas de chemin Windows dans un chemin d'écran, et la même clé quand la liste change d'ordre.

use std::collections::{HashMap, HashSet};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant, SystemTime};

use dragontree_app::DragonTreeApp;
use dragontree_app::repo::{Refresh, RemoteOp, RepoTab};
use dragontree_app::settings::{Settings, path_key};
use dragontree_git::{CommitInfo, DiffOptions, FileChange, FileState, Git, Oid, Repo, StatusEntry, StatusSnapshot};
use suite_sdk::mobile::ui::{Act, Block, Row, Screen, Tone};

use crate::depot::{self, RepoView, file_key};

/// Dépôts montrés.
const MAX_REPOS: usize = 20;
/// L'état des dépôts qui ne sont pas ouverts dans DragonTree est relu au plus toutes les 30 s tant que le téléphone
/// regarde la liste.
const SWEEP_EVERY: Duration = Duration::from_secs(30);
const SETTINGS_EVERY: Duration = Duration::from_secs(5);
/// Le résultat d'une opération reste affiché une minute.
const JOB_SHOWN: Duration = Duration::from_secs(60);
/// DragonTree ne suit les changements sur disque que dans l'onglet affiché : un dépôt regardé depuis le téléphone lui
/// est fait relire au plus toutes les 20 s.
const REFRESH_EVERY: Duration = Duration::from_secs(20);
/// Le diff d'un fichier du dossier de travail est relu au plus toutes les 15 s tant qu'il est regardé.
const DIFF_EVERY: Duration = Duration::from_secs(15);
/// Une opération attend au plus 30 s que DragonTree ait ouvert son dépôt.
const OPEN_WAIT: Duration = Duration::from_secs(30);
/// Une erreur de git montrée est coupée au-delà (caractères).
const ERROR_MAX: usize = 400;

/// D'où vient un dépôt de la liste.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    Tab,
    Favorite,
    Recent,
}

impl Origin {
    fn title(self) -> &'static str {
        match self {
            Origin::Tab => "Onglets ouverts",
            Origin::Favorite => "Favoris",
            Origin::Recent => "Récents",
        }
    }
}

/// L'état d'un dépôt : celui que DragonTree a lu, ou relevé ici pour un dépôt qu'il n'a pas ouvert.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RepoState {
    /// `None` : HEAD détachée.
    pub branch: Option<String>,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    pub changes: usize,
    pub error: Option<String>,
}

/// Ce que fait DragonTree sur un dépôt (`Info` : en cours), ou ce qu'a donné sa dernière opération.
#[derive(Clone, Debug, PartialEq)]
pub struct Job {
    pub text: String,
    pub tone: Tone,
    pub at: Instant,
}

/// Un dépôt de la liste, avec ce qu'on en sait.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub path: PathBuf,
    pub origin: Origin,
    /// Le dépôt de l'onglet actif de DragonTree.
    pub active: bool,
    pub state: Option<RepoState>,
    pub job: Option<Job>,
}

/// Empreinte courte d'un dépôt (de sa clé de chemin, insensible à la casse comme Windows).
pub fn repo_key(path: &Path) -> String {
    let mut h = DefaultHasher::new();
    path_key(path).hash(&mut h);
    format!("{:x}", h.finish() & 0xffff_ffff)
}

fn repo_name(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.display().to_string())
}

fn sync_text(ahead: u32, behind: u32) -> Option<String> {
    match (ahead, behind) {
        (0, 0) => None,
        (a, 0) => Some(format!("↑{a}")),
        (0, b) => Some(format!("↓{b}")),
        (a, b) => Some(format!("↑{a} ↓{b}")),
    }
}

fn changes_text(n: usize) -> String {
    match n {
        0 => "propre".into(),
        1 => "1 modification".into(),
        n => format!("{n} modifications"),
    }
}

/// « main · ↑2 ↓1 · 3 modifications ».
fn state_text(s: &RepoState) -> String {
    if let Some(e) = &s.error {
        return e.clone();
    }
    let mut parts = vec![s.branch.clone().unwrap_or_else(|| "HEAD détachée".into())];
    parts.extend(sync_text(s.ahead, s.behind));
    parts.push(changes_text(s.changes));
    parts.join(" · ")
}

fn state_tone(s: &RepoState) -> Tone {
    if s.error.is_some() {
        Tone::Danger
    } else if s.behind > 0 || s.changes > 0 {
        Tone::Warn
    } else {
        Tone::Ok
    }
}

fn net_acts(key: &str) -> Vec<Act> {
    vec![
        Act::new(format!("fetch:{key}"), "Fetch").glyph("⬇️"),
        Act::new(format!("pull:{key}"), "Pull").glyph("↓"),
        Act::new(format!("push:{key}"), "Push")
            .glyph("↑")
            .confirm("Pousser la branche courante vers sa branche distante ?"),
    ]
}

/// L'accueil : les dépôts par origine ; `sweeping` : l'état de ceux que DragonTree n'a pas ouverts est en cours de
/// relevé.
pub fn home(entries: &[Entry], sweeping: bool) -> Screen {
    let mut s = Screen::new("DragonTree").act(Act::new("actualiser", "Actualiser").glyph("🔄"));
    if sweeping {
        s.subtitle = "Relevé de l'état des dépôts…".into();
    }
    let mut jobs: Vec<(&Entry, &Job)> = entries
        .iter()
        .filter_map(|e| Some((e, e.job.as_ref()?)))
        .filter(|(_, j)| j.tone != Tone::Info && j.at.elapsed() < JOB_SHOWN)
        .collect();
    jobs.sort_by_key(|(_, j)| j.at);
    for (e, j) in jobs {
        s.note(format!("{} : {}", repo_name(&e.path), j.text), j.tone);
    }
    if entries.is_empty() {
        s.empty("Aucun dépôt : ouvrez-en un dans DragonTree, sur le PC.");
    }
    for origin in [Origin::Tab, Origin::Favorite, Origin::Recent] {
        let list: Vec<&Entry> = entries.iter().filter(|e| e.origin == origin).collect();
        if list.is_empty() {
            continue;
        }
        s.section(origin.title(), list.len().to_string());
        for e in list {
            let key = repo_key(&e.path);
            let mut row = Row::new(repo_name(&e.path)).glyph("🌿").open(format!("depot/{key}"));
            row = match &e.state {
                Some(st) => row.subtitle(state_text(st)).tone(state_tone(st)),
                None => row.subtitle("lecture…"),
            };
            if e.job.as_ref().is_some_and(|j| j.tone == Tone::Info) {
                row = row.badge("en cours");
            } else if e.active {
                row = row.badge("actif");
            }
            row.acts = net_acts(&key);
            row = row.act(Act::new(format!("pc:{key}"), "Afficher dans DragonTree sur le PC").glyph("🖥️"));
            s.row(row);
        }
    }
    s
}

// --- Ce que DragonTree sait ---

fn short_error(e: &str) -> String {
    let e = e.trim();
    if e.chars().count() <= ERROR_MAX {
        return e.to_string();
    }
    let cut: String = e.chars().take(ERROR_MAX).collect();
    format!("{cut}…")
}

fn state_of(s: &StatusSnapshot) -> RepoState {
    RepoState {
        branch: s.branch.clone(),
        upstream: s.upstream.clone(),
        ahead: s.ahead,
        behind: s.behind,
        changes: s.entries.iter().filter(|e| e.worktree != FileState::Ignored).count(),
        error: None,
    }
}

/// L'état du dépôt tel que DragonTree l'a lu ; `None` : pas encore lu.
fn tab_state(t: &RepoTab) -> Option<RepoState> {
    if let Some(e) = &t.open_error {
        return Some(RepoState { error: Some(e.to_string()), ..Default::default() });
    }
    t.status.as_deref().map(state_of)
}

/// L'opération visible en cours dans l'onglet (lancée d'ici ou du PC), sinon ce qu'a donné la dernière dans la minute.
fn tab_job(t: &RepoTab) -> Option<Job> {
    if let Some(op) = t.running.iter().find(|o| !o.silent) {
        let text = if op.waiting {
            format!("{} : attend la fin du fetch automatique…", op.title)
        } else if let Some(p) = op.progress.as_ref().and_then(|p| p.percent) {
            format!("{} en cours… {p} %", op.title)
        } else {
            format!("{} en cours…", op.title)
        };
        return Some(Job { text, tone: Tone::Info, at: Instant::now() });
    }
    let o = t.outcomes().last().filter(|o| o.at.elapsed() < JOB_SHOWN)?;
    Some(match &o.result {
        Ok(Some(msg)) => Job { text: msg.clone(), tone: Tone::Ok, at: o.at },
        Ok(None) => Job { text: format!("{} : terminé", o.title), tone: Tone::Ok, at: o.at },
        Err(e) => Job { text: format!("{} a échoué : {}", o.title, short_error(e)), tone: Tone::Danger, at: o.at },
    })
}

/// L'historique tel que DragonTree l'affiche ; `None` : en cours de lecture.
fn log_of(t: &RepoTab) -> Option<Result<Vec<dragontree_git::Commit>, String>> {
    if let Some(e) = &t.history.error {
        return Some(Err(e.to_string()));
    }
    if t.history.commits.is_empty() && (t.history.loading || t.refs.is_none()) {
        return None;
    }
    Some(Ok(t.history.commits.iter().take(depot::LOG_COUNT).cloned().collect()))
}

// --- Ce qui est lu à part ---

/// Ce qui a été lu à part pour un dépôt, avec le dépôt ouvert par DragonTree.
#[derive(Default)]
struct Reads {
    /// Le relevé d'état de DragonTree sur lequel le dernier message et les diffs du dossier de travail ont été lus.
    status_seen: usize,
    last_message: Option<String>,
    /// Commit (identifiant complet) → ses informations et ses fichiers.
    commits: HashMap<String, Result<(CommitInfo, Vec<FileChange>), String>>,
    /// Diff rendu : `w:<fichier>` (dossier de travail) ou `c:<commit>:<fichier>`.
    diffs: HashMap<String, Result<Vec<(u8, String)>, String>>,
    /// Lecture → quand elle a été faite ; en cours : absente de `loaded`, présente dans `loading`.
    loaded: HashMap<String, Instant>,
    loading: HashSet<String>,
}

/// Par dépôt (clé).
type SharedReads = Arc<Mutex<HashMap<String, Reads>>>;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Ce qu'un écran du dépôt a besoin de lire à part.
#[derive(Clone, Debug)]
enum Load {
    LastMessage,
    Commit(String),
    /// Diff d'un fichier du dossier de travail (indexé et non indexé).
    WorkDiff(String, StatusEntry),
    /// Diff d'un fichier d'un commit.
    CommitDiff(String, String),
}

impl Load {
    fn key(&self) -> String {
        match self {
            Load::LastMessage => "message".into(),
            Load::Commit(id) => format!("commit:{id}"),
            Load::WorkDiff(f, _) => format!("w:{f}"),
            Load::CommitDiff(id, f) => format!("c:{id}:{f}"),
        }
    }

    fn every(&self) -> Option<Duration> {
        match self {
            Load::WorkDiff(..) => Some(DIFF_EVERY),
            // Relus quand l'état lu par DragonTree change ; un commit ne change pas.
            Load::LastMessage | Load::Commit(_) | Load::CommitDiff(..) => None,
        }
    }
}

fn oid(hex: &str) -> Result<Oid, String> {
    Oid::from_hex(hex).ok_or_else(|| "identifiant de commit illisible".into())
}

/// Lit `what` (dans un fil) et le range dans `r`.
fn read(repo: &Repo, what: &Load, known: Option<&(CommitInfo, Vec<FileChange>)>, r: &mut Reads) {
    let opts = DiffOptions::default();
    match what {
        Load::LastMessage => r.last_message = Some(repo.last_commit_message().unwrap_or_default()),
        Load::Commit(id) => {
            let res = oid(id).and_then(|o| {
                let info = repo.commit_info(o).map_err(|e| e.to_string())?;
                let files = repo.commit_files(o, info.parents.first().copied()).map_err(|e| e.to_string())?;
                Ok((info, files))
            });
            r.commits.insert(id.clone(), res);
        }
        Load::WorkDiff(f, e) => {
            let staged = e.conflict.is_none() && !matches!(e.index, FileState::Unmodified | FileState::Untracked);
            let unstaged = !matches!(e.worktree, FileState::Unmodified | FileState::Ignored);
            let untracked = e.worktree == FileState::Untracked;
            let mut parts = Vec::new();
            let mut error = None;
            for (wanted, cached, title) in [(staged, true, "Indexé"), (unstaged, false, "Non indexé")] {
                if !wanted {
                    continue;
                }
                match repo.diff_worktree_file(&e.path, cached, untracked && !cached, &opts) {
                    Ok(d) => {
                        parts.push((suite_sdk::mobile::ui::DIFF_HEADER, format!("── {title} ──")));
                        parts.extend(depot::render_diff(&d));
                    }
                    Err(err) => error = Some(err.to_string()),
                }
            }
            let res = match (parts.is_empty(), error) {
                (true, Some(e)) => Err(e),
                _ => Ok(parts),
            };
            r.diffs.insert(format!("w:{f}"), res);
        }
        Load::CommitDiff(id, f) => {
            let Some((info, files)) = known else { return };
            let Some(change) = files.iter().find(|c| file_key(&c.path) == *f) else { return };
            let res = oid(id).and_then(|o| {
                repo.diff_commit_file(o, info.parents.first().copied(), change, &opts)
                    .map(|d| depot::render_diff(&d))
                    .map_err(|e| e.to_string())
            });
            r.diffs.insert(format!("c:{id}:{f}"), res);
        }
    }
}

// --- Les dépôts que DragonTree n'a pas ouverts ---

/// L'état rapide des dépôts de la liste qui ne sont pas dans un onglet de DragonTree.
#[derive(Default)]
struct Sweep {
    states: HashMap<String, RepoState>,
    at: Option<Instant>,
    running: bool,
}

/// L'exécutable git, cherché une fois (celui des réglages de DragonTree d'abord).
type GitCell = Arc<OnceLock<Result<Git, String>>>;

fn git(cell: &GitCell, configured: Option<PathBuf>) -> Result<Git, String> {
    cell.get_or_init(|| {
        configured
            .map(Git::with_exe)
            .filter(|g| g.version().is_ok())
            .map(Ok)
            .unwrap_or_else(|| Git::discover().map_err(|e| e.to_string()))
    })
    .clone()
}

fn read_state(git: &Result<Git, String>, path: &Path) -> RepoState {
    if !path.is_dir() {
        return RepoState { error: Some("dossier introuvable".into()), ..Default::default() };
    }
    let status = git
        .clone()
        .and_then(|g| Repo::open(&g, path).map_err(|e| e.to_string()))
        .and_then(|r| r.status().map_err(|e| e.to_string()));
    match status {
        Ok(s) => state_of(&s),
        Err(e) => RepoState { error: Some(e), ..Default::default() },
    }
}

/// Les dépôts des réglages de DragonTree.
#[derive(Clone, Debug, Default, PartialEq)]
struct Lists {
    tabs: Vec<PathBuf>,
    favorites: Vec<PathBuf>,
    recent: Vec<PathBuf>,
    git_path: Option<PathBuf>,
}

/// La liste montrée : l'onglet actif, les onglets ouverts, les favoris, les récents ; chacun une fois.
fn repo_list(lists: &Lists, active: Option<&Path>) -> Vec<(PathBuf, Origin, bool)> {
    let active_key = active.map(path_key);
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    let tabs = active.into_iter().map(Path::to_path_buf).chain(lists.tabs.iter().cloned());
    let all = tabs
        .map(|p| (p, Origin::Tab))
        .chain(lists.favorites.iter().cloned().map(|p| (p, Origin::Favorite)))
        .chain(lists.recent.iter().cloned().map(|p| (p, Origin::Recent)));
    for (p, origin) in all {
        let key = path_key(&p);
        if seen.insert(key.clone()) {
            out.push((p, origin, active_key.as_ref() == Some(&key)));
        }
        if out.len() == MAX_REPOS {
            break;
        }
    }
    out
}

/// Une opération du téléphone qui attend que DragonTree ait ouvert son dépôt.
struct Waiting {
    path: PathBuf,
    op: RemoteOp,
    label: String,
    at: Instant,
}

/// Ce que le téléphone garde entre deux appels.
pub struct Phone {
    settings_path: PathBuf,
    checked: Option<Instant>,
    stamp: Option<SystemTime>,
    lists: Lists,
    sweep: Arc<Mutex<Sweep>>,
    git: GitCell,
    reads: SharedReads,
    /// Par dépôt : quand DragonTree l'a relu à la demande du téléphone.
    refreshed: HashMap<String, Instant>,
    waiting: HashMap<String, Waiting>,
    /// Par dépôt : une opération qui n'a pas pu partir (dépôt impossible à ouvrir).
    failed: HashMap<String, Job>,
}

impl Phone {
    pub fn new(settings_path: PathBuf) -> Phone {
        Phone {
            settings_path,
            checked: None,
            stamp: None,
            lists: Lists::default(),
            sweep: Arc::default(),
            git: GitCell::default(),
            reads: SharedReads::default(),
            refreshed: HashMap::new(),
            waiting: HashMap::new(),
            failed: HashMap::new(),
        }
    }

    /// Relit les réglages de DragonTree s'ils ont changé (au plus toutes les 5 s), sans jamais les réécrire.
    fn update(&mut self) {
        if self.checked.is_some_and(|t| t.elapsed() < SETTINGS_EVERY) {
            return;
        }
        self.checked = Some(Instant::now());
        let stamp = std::fs::metadata(&self.settings_path).and_then(|m| m.modified()).ok();
        if stamp == self.stamp {
            return;
        }
        self.stamp = stamp;
        let settings: Option<Settings> =
            std::fs::read(&self.settings_path).ok().and_then(|b| serde_json::from_slice(&b).ok());
        if let Some(s) = settings {
            self.lists =
                Lists { tabs: s.open_tabs, favorites: s.favorites, recent: s.recent_repos, git_path: s.git_path };
        }
    }

    fn repos(&mut self, active: Option<&Path>) -> Vec<(PathBuf, Origin, bool)> {
        self.update();
        repo_list(&self.lists, active)
    }

    /// Relève l'état de `paths` (dépôts que DragonTree n'a pas ouverts) dans un fil, s'il est dû (`force` : tout de
    /// suite).
    fn sweep(&self, paths: Vec<PathBuf>, force: bool) {
        let mut s = lock(&self.sweep);
        if paths.is_empty() || s.running || (!force && s.at.is_some_and(|t| t.elapsed() < SWEEP_EVERY)) {
            return;
        }
        s.running = true;
        drop(s);
        let (sweep, cell, configured) = (self.sweep.clone(), self.git.clone(), self.lists.git_path.clone());
        std::thread::spawn(move || {
            let git = git(&cell, configured);
            for p in paths {
                let state = read_state(&git, &p);
                lock(&sweep).states.insert(repo_key(&p), state);
            }
            let mut s = lock(&sweep);
            s.running = false;
            s.at = Some(Instant::now());
        });
    }

    /// Ce qui se passe sur le dépôt `key` : une opération d'ici qui attend son ouverture, ou qui n'a pas pu partir ;
    /// sinon ce que fait DragonTree, ou ce qu'il a donné.
    fn job(&self, key: &str, tab: Option<&RepoTab>) -> Option<Job> {
        if let Some(w) = self.waiting.get(key) {
            let text = format!("{} : ouverture du dépôt dans DragonTree…", w.label);
            return Some(Job { text, tone: Tone::Info, at: w.at });
        }
        let failed = self.failed.get(key).filter(|j| j.at.elapsed() < JOB_SHOWN).cloned();
        let running = tab.and_then(tab_job);
        match (failed, running) {
            (Some(f), Some(r)) if r.tone == Tone::Info || r.at > f.at => Some(r),
            (Some(f), _) => Some(f),
            (None, r) => r,
        }
    }

    fn entry(&self, app: &DragonTreeApp, (path, origin, active): (PathBuf, Origin, bool)) -> Entry {
        let key = repo_key(&path);
        let tab = app.repo_tab(&path);
        let state = match tab {
            Some(t) => tab_state(t),
            None => lock(&self.sweep).states.get(&key).cloned(),
        };
        Entry { state, job: self.job(&key, tab), path, origin, active }
    }

    /// Le dépôt regardé est ouvert dans un onglet de DragonTree (en fond), et relu par lui au plus toutes les 20 s.
    fn watch(&mut self, app: &mut DragonTreeApp, repo: &Path) {
        let key = repo_key(repo);
        if app.repo_tab(repo).is_none() {
            // L'ouverture lit tout.
            app.open_repo_tab(repo);
            self.refreshed.insert(key, Instant::now());
            return;
        }
        if self.refreshed.get(&key).is_none_or(|t| t.elapsed() >= REFRESH_EVERY) {
            app.refresh_repo(repo, Refresh::REFS.with_extras());
            self.refreshed.insert(key, Instant::now());
        }
    }

    /// Lit `what` pour le dépôt `key` dans un fil, s'il est dû.
    fn want(&self, key: &str, repo: &Arc<Repo>, what: Load) {
        let k = what.key();
        let mut all = lock(&self.reads);
        let r = all.entry(key.to_string()).or_default();
        let fresh = r.loaded.get(&k).is_some_and(|t| what.every().is_none_or(|every| t.elapsed() < every));
        if fresh || !r.loading.insert(k.clone()) {
            return;
        }
        let known = match &what {
            Load::CommitDiff(id, _) => match r.commits.get(id) {
                Some(Ok(c)) => Some(c.clone()),
                _ => None,
            },
            _ => None,
        };
        drop(all);
        let (reads, repo, key) = (self.reads.clone(), repo.clone(), key.to_string());
        std::thread::spawn(move || {
            let mut fresh = Reads::default();
            read(&repo, &what, known.as_ref(), &mut fresh);
            let mut all = lock(&reads);
            let r = all.entry(key).or_default();
            r.loading.remove(&k);
            r.loaded.insert(k, Instant::now());
            if fresh.last_message.is_some() {
                r.last_message = fresh.last_message;
            }
            r.commits.extend(fresh.commits);
            r.diffs.extend(fresh.diffs);
        });
    }

    /// Ce que DragonTree a relu de nouveau rend périmés le dernier message et les diffs du dossier de travail : ils
    /// seront relus (les anciens restent montrés d'ici là).
    fn follow(&self, key: &str, t: &RepoTab) {
        let seen = t.status.as_ref().map_or(0, |s| Arc::as_ptr(s) as usize);
        let mut all = lock(&self.reads);
        let r = all.entry(key.to_string()).or_default();
        if r.status_seen != seen {
            r.status_seen = seen;
            r.loaded.retain(|k, _| k != "message" && !k.starts_with("w:"));
        }
    }

    /// Ce que DragonTree sait du dépôt, et ce qui en a été lu à part pour l'écran `parts`.
    fn view(&self, key: &str, t: &RepoTab, parts: &[&str]) -> RepoView {
        let status = match (&t.open_error, &t.status) {
            (Some(e), _) => Some(Err(e.to_string())),
            (None, Some(s)) => {
                // DragonTree lit aussi les fichiers ignorés ; le téléphone ne les montre pas.
                let mut s = (**s).clone();
                s.entries.retain(|e| e.worktree != FileState::Ignored);
                Some(Ok(s))
            }
            (None, None) => None,
        };
        let mut v = RepoView {
            status,
            refs: t.refs.as_deref().cloned(),
            stashes: t.stashes.to_vec(),
            worktrees: t.worktrees.to_vec(),
            remotes: t.remotes.iter().map(|r| r.name.clone()).collect(),
            op: t.status.is_some().then_some(t.op_state),
            ..Default::default()
        };
        if parts == ["historique"] {
            v.log = log_of(t);
        }
        let all = lock(&self.reads);
        let Some(r) = all.get(key) else { return v };
        v.last_message = r.last_message.clone().unwrap_or_default();
        let mut keep = |k: String| {
            if let Some(d) = r.diffs.get(&k) {
                v.diffs.insert(k, d.clone());
            }
        };
        match parts {
            ["fichier", f] => keep(format!("w:{f}")),
            ["commit", id, f] => keep(format!("c:{id}:{f}")),
            _ => {}
        }
        if let ["commit", id, ..] = parts
            && let Some(c) = r.commits.get(*id)
        {
            v.commits.insert(id.to_string(), c.clone());
        }
        v
    }

    pub fn screen(&mut self, app: &mut DragonTreeApp, path: &str) -> Option<Screen> {
        let repos = self.repos(app.active_repo_dir());
        if path.is_empty() {
            let closed = repos.iter().filter(|r| app.repo_tab(&r.0).is_none()).map(|r| r.0.clone()).collect();
            self.sweep(closed, false);
            let sweeping = lock(&self.sweep).running;
            let entries: Vec<Entry> = repos.into_iter().map(|r| self.entry(app, r)).collect();
            return Some(home(&entries, sweeping));
        }
        let rest = path.strip_prefix("depot/")?;
        let (key, sub) = rest.split_once('/').unwrap_or((rest, ""));
        let repo = repos.into_iter().find(|r| repo_key(&r.0) == key)?.0;
        self.watch(app, &repo);
        let t = app.repo_tab(&repo)?;
        let parts: Vec<&str> = sub.split('/').collect();
        self.follow(key, t);
        if let Some(handle) = &t.repo {
            match parts.as_slice() {
                [""] | ["modifs"] => self.want(key, handle, Load::LastMessage),
                ["fichier", f] => {
                    let entry = t.status.as_ref().and_then(|s| s.entries.iter().find(|e| file_key(&e.path) == *f));
                    if let Some(e) = entry {
                        self.want(key, handle, Load::WorkDiff(f.to_string(), e.clone()));
                    }
                }
                ["commit", id] => self.want(key, handle, Load::Commit(id.to_string())),
                ["commit", id, f] => {
                    self.want(key, handle, Load::Commit(id.to_string()));
                    self.want(key, handle, Load::CommitDiff(id.to_string(), f.to_string()));
                }
                _ => {}
            }
        }
        let view = self.view(key, t, &parts);
        let job = self.job(key, Some(t));
        let busy = job.as_ref().is_some_and(|j| j.tone == Tone::Info);
        let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
        let mut screen = match parts.as_slice() {
            [""] => {
                let j = job.as_ref().map(|j| (j.text.as_str(), j.tone));
                return Some(depot::overview(&repo_name(&repo), &repo.display().to_string(), &view, j, busy));
            }
            ["modifs"] => depot::changes(&view, busy),
            ["fichier", f] => depot::file(&view, f, busy)?,
            ["historique"] => depot::history(&view, now),
            ["commit", id] => depot::commit(&view, id, now, busy),
            ["commit", id, f] => depot::commit_file(&view, id, f)?,
            ["branches"] => depot::branches(&view, busy),
            ["remises"] => depot::stashes(&view, now, busy),
            ["etiquettes"] => depot::tags(&view, busy),
            ["worktrees"] => depot::worktrees(&view, busy),
            _ => return None,
        };
        // Ce que fait DragonTree, ou ce qu'a donné sa dernière opération, en tête de chaque écran du dépôt.
        if let Some(j) = job {
            screen.blocks.insert(0, Block::Note { text: j.text, tone: j.tone });
        }
        Some(screen)
    }

    /// Une action de la liste (`fetch:<clé>`…) ou d'un écran du dépôt (`depot/<clé>/…`) ; vrai : DragonTree est à
    /// montrer sur le PC (« afficher dans DragonTree »).
    pub fn action(
        &mut self,
        app: &mut DragonTreeApp,
        path: &str,
        action: &str,
        input: Option<&str>,
    ) -> Result<(String, bool), String> {
        let repos = self.repos(app.active_repo_dir());
        let in_repo = path.strip_prefix("depot/").map(|r| r.split_once('/').map_or(r, |(k, _)| k).to_string());
        if action == "actualiser" && in_repo.is_none() {
            let mut closed = Vec::new();
            for (p, ..) in repos {
                if app.repo_tab(&p).is_some() {
                    app.refresh_repo(&p, Refresh::REFS.with_extras());
                } else {
                    closed.push(p);
                }
            }
            self.sweep(closed, true);
            return Ok(("Actualisation lancée".into(), false));
        }
        // Depuis la liste, l'action porte la clé du dépôt (`fetch:<clé>`) ; dans un écran du dépôt, le chemin.
        let (verb, key) = match &in_repo {
            Some(k) => (action.to_string(), k.clone()),
            None => {
                let (op, k) = action.split_once(':').ok_or("action inconnue")?;
                (op.to_string(), k.to_string())
            }
        };
        let repo = repos
            .into_iter()
            .find(|r| repo_key(&r.0) == key)
            .map(|r| r.0)
            .ok_or("Ce dépôt n'est plus dans la liste.")?;
        let name = repo_name(&repo);
        if verb == "pc" {
            app.show_repo(&repo);
            return Ok((format!("{name} affiché dans DragonTree sur le PC"), true));
        }
        if verb == "actualiser" {
            if app.repo_tab(&repo).is_none() {
                app.open_repo_tab(&repo);
            } else {
                app.refresh_repo(&repo, Refresh::ALL.with_extras());
            }
            self.refreshed.insert(key.clone(), Instant::now());
            lock(&self.reads).remove(&key);
            return Ok(("DragonTree relit le dépôt…".into(), false));
        }
        // Le geste vient du dernier écran du dépôt, fait de ce que DragonTree en savait (`view` : sans ce qui est lu
        // à part, inutile ici).
        let view = app.repo_tab(&repo).map(|t| self.view(&key, t, &[])).unwrap_or_default();
        let (op, label) = depot::parse(&view, &verb, input)?;
        self.failed.remove(&key);
        app.open_repo_tab(&repo);
        let opening = app.repo_tab(&repo).is_some_and(|t| t.repo.is_none() && t.open_error.is_none());
        if opening {
            if self.waiting.contains_key(&key) {
                return Err(format!("{name} : une opération attend déjà l'ouverture du dépôt."));
            }
            self.waiting.insert(key, Waiting { path: repo, op, label: label.clone(), at: Instant::now() });
            return Ok((format!("{label} : ouverture de {name} dans DragonTree…"), false));
        }
        app.queue_operation(&repo, op)?;
        Ok((format!("{label} : lancé dans DragonTree"), false))
    }

    /// À chaque image : les opérations qui attendaient l'ouverture de leur dépôt partent quand DragonTree l'a ouvert
    /// (ou sont abandonnées s'il n'y arrive pas).
    pub fn after_logic(&mut self, app: &mut DragonTreeApp) {
        if self.waiting.is_empty() {
            return;
        }
        let keys: Vec<String> = self.waiting.keys().cloned().collect();
        for key in keys {
            let w = &self.waiting[&key];
            let (ready, error) = match app.repo_tab(&w.path) {
                None => (false, Some("l'onglet du dépôt a été fermé dans DragonTree.".to_string())),
                Some(t) => (t.repo.is_some(), t.open_error.as_ref().map(|e| e.to_string())),
            };
            let error = error.or_else(|| {
                (!ready && w.at.elapsed() > OPEN_WAIT).then(|| "DragonTree n'a pas ouvert le dépôt.".into())
            });
            if !ready && error.is_none() {
                continue;
            }
            let Some(w) = self.waiting.remove(&key) else { continue };
            let error = error.or_else(|| app.queue_operation(&w.path, w.op).err());
            if let Some(e) = error {
                let text = format!("{} : {}", w.label, short_error(&e));
                self.failed.insert(key, Job { text, tone: Tone::Danger, at: Instant::now() });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use dragontree_app::StartOptions;
    use egui_kittest::Harness;
    use suite_sdk::mobile::ui::Block;

    use super::*;

    type H = Harness<'static, DragonTreeApp>;

    fn state(branch: &str, ahead: u32, behind: u32, changes: usize) -> RepoState {
        RepoState {
            branch: Some(branch.into()),
            upstream: Some(format!("origin/{branch}")),
            ahead,
            behind,
            changes,
            error: None,
        }
    }

    fn entry(path: &str, origin: Origin, state: Option<RepoState>) -> Entry {
        Entry { path: PathBuf::from(path), origin, active: false, state, job: None }
    }

    #[test]
    fn the_list_puts_the_active_tab_first_and_each_repo_once() {
        let lists = Lists {
            tabs: vec![r"C:\dev\ozmo".into(), r"C:\dev\diapaz".into()],
            favorites: vec![r"c:\dev\OZMO\".into(), r"C:\dev\ximi".into()],
            recent: vec![r"C:\dev\ximi".into(), r"C:\dev\walle".into()],
            git_path: None,
        };
        let list = repo_list(&lists, Some(Path::new(r"C:\dev\diapaz")));
        let names: Vec<(String, Origin, bool)> = list.iter().map(|(p, o, a)| (repo_name(p), *o, *a)).collect();
        assert_eq!(
            names,
            [
                ("diapaz".into(), Origin::Tab, true),
                ("ozmo".into(), Origin::Tab, false),
                ("ximi".into(), Origin::Favorite, false),
                ("walle".into(), Origin::Recent, false),
            ]
        );
        assert_eq!(repo_key(Path::new(r"C:\dev\ozmo")), repo_key(Path::new(r"c:/dev/OZMO/")), "même dossier");
    }

    #[test]
    fn the_home_shows_each_repo_state_and_its_actions() {
        let mut active = entry(r"C:\dev\ozmo", Origin::Tab, Some(state("main", 2, 1, 3)));
        active.active = true;
        let broken = entry(
            r"C:\dev\parti",
            Origin::Recent,
            Some(RepoState { error: Some("dossier introuvable".into()), ..Default::default() }),
        );
        let mut busy = entry(r"C:\dev\ximi", Origin::Favorite, None);
        busy.job = Some(Job { text: "Fetch en cours…".into(), tone: Tone::Info, at: Instant::now() });
        let s = home(&[active, busy, broken], false);
        let rows: Vec<&Row> = s
            .blocks
            .iter()
            .filter_map(|b| match b {
                Block::Row(r) => Some(r),
                _ => None,
            })
            .collect();
        assert_eq!((rows[0].subtitle.as_str(), rows[0].badge.as_str()), ("main · ↑2 ↓1 · 3 modifications", "actif"));
        assert_eq!(rows[0].tone, Some(Tone::Warn));
        assert_eq!((rows[1].subtitle.as_str(), rows[1].badge.as_str()), ("lecture…", "en cours"));
        assert_eq!((rows[2].subtitle.as_str(), rows[2].tone), ("dossier introuvable", Some(Tone::Danger)));
        let key = repo_key(Path::new(r"C:\dev\ozmo"));
        assert!(s.opens().contains(&format!("depot/{key}")));
        let ids = s.action_ids();
        for op in ["fetch", "pull", "push", "pc"] {
            assert!(ids.contains(&format!("{op}:{key}")), "{op}");
        }
        assert!(ids.contains("actualiser"));
        let push = s.all_acts().into_iter().find(|a| a.id == format!("push:{key}")).unwrap();
        assert!(push.confirm.is_some(), "push confirmé");
    }

    /// DragonTree dans un profil d'essai, sans le coffre de Windows.
    fn dragontree(profile: &Path) -> H {
        dragontree_forges::secrets::use_vaults(
            Arc::new(suite_accounts::MemoryVault::default()),
            Arc::new(suite_accounts::MemoryVault::default()),
        );
        let start = StartOptions {
            profile_dir: Some(profile.to_path_buf()),
            accounts_dir: Some(profile.join("comptes")),
            screenshot_after: Duration::ZERO,
            lang: Some(dragontree_i18n::Lang::Fr),
            ..Default::default()
        };
        Harness::builder()
            .with_size(eframe::egui::vec2(1200.0, 800.0))
            .build_eframe(move |cc| DragonTreeApp::new(cc, start))
    }

    /// Le profil de DragonTree avec `repo` dans ses récents, et le téléphone qui lit ses réglages.
    fn phone_for(profile: &Path, repo: &Path) -> Phone {
        std::fs::create_dir_all(profile).unwrap();
        let settings = Settings { recent_repos: vec![repo.to_path_buf()], ..Default::default() };
        let file = profile.join("settings.json");
        std::fs::write(&file, serde_json::to_vec(&settings).unwrap()).unwrap();
        Phone::new(file)
    }

    /// Fait tourner DragonTree jusqu'à ce que l'écran `path` contienne `text`.
    fn wait_screen(h: &mut H, phone: &mut Phone, path: &str, text: &str) -> Screen {
        let deadline = Instant::now() + Duration::from_secs(40);
        loop {
            h.step();
            phone.after_logic(h.state_mut());
            let s = phone.screen(h.state_mut(), path).expect("écran");
            if format!("{s:?}").contains(text) || Instant::now() > deadline {
                return s;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    #[test]
    fn an_operation_waits_for_dragontree_to_open_the_repo() {
        let dir = std::env::temp_dir().join(format!("suite-dragontree-telephone-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let repo = dir.join("pas-un-depot");
        std::fs::create_dir_all(&repo).unwrap();
        let mut phone = phone_for(&dir.join("profil"), &repo);
        let mut h = dragontree(&dir.join("profil"));
        h.step();
        let key = repo_key(&repo);
        assert!(phone.action(h.state_mut(), "", "fetch:inconnu", None).is_err());
        let (text, show) = phone.action(h.state_mut(), "", &format!("fetch:{key}"), None).unwrap();
        assert!(text.contains("ouverture") && !show, "{text}");
        assert!(h.state().repo_tab(&repo).is_some(), "ouvert dans un onglet de DragonTree");
        assert!(h.state().active_repo_dir().is_none(), "en fond");
        assert!(phone.action(h.state_mut(), "", &format!("fetch:{key}"), None).is_err(), "une seule en attente");
        let s = wait_screen(&mut h, &mut phone, &format!("depot/{key}"), "tone: Danger");
        assert!(
            s.blocks
                .iter()
                .any(|b| matches!(b, Block::Note { tone: Tone::Danger, text } if text.starts_with("Fetch : "))),
            "pas un dépôt git : {s:?}"
        );
        drop(h);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Un vrai dépôt, mené par DragonTree : modifier, voir le diff, indexer, valider, retrouver le commit dans son
    /// historique, créer une branche ; un nom refusé par git est dit.
    #[test]
    fn a_change_is_staged_and_committed_by_dragontree() {
        let Ok(git) = Git::discover() else { return };
        let dir = std::env::temp_dir().join(format!("suite-dragontree-git-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let repo = dir.join("depot");
        std::fs::create_dir_all(&repo).unwrap();
        let sh = |args: &[&str]| {
            let ok = std::process::Command::new(git.exe())
                .args(args)
                .current_dir(&repo)
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .output()
                .unwrap()
                .status
                .success();
            assert!(ok, "git {args:?}");
        };
        sh(&["init", "-q", "-b", "main"]);
        sh(&["config", "user.name", "Essai"]);
        sh(&["config", "user.email", "essai@exemple.org"]);
        sh(&["config", "commit.gpgsign", "false"]);
        std::fs::write(repo.join("lisez-moi.txt"), "un\n").unwrap();
        sh(&["add", "."]);
        sh(&["commit", "-q", "-m", "Premier commit"]);
        std::fs::write(repo.join("lisez-moi.txt"), "un\ndeux\n").unwrap();

        let profile = dir.join("profil");
        let mut phone = phone_for(&profile, &repo);
        let mut h = dragontree(&profile);
        let key = repo_key(&repo);
        let base = format!("depot/{key}");
        let s = wait_screen(&mut h, &mut phone, &base, "1 fichier(s), dont 0 indexé(s)");
        assert!(format!("{s:?}").contains("1 fichier(s), dont 0 indexé(s)"), "{s:?}");
        assert!(h.state().repo_tab(&repo).is_some() && h.state().active_repo_dir().is_none(), "onglet de fond");
        let f = file_key("lisez-moi.txt");
        let diff = wait_screen(&mut h, &mut phone, &format!("{base}/fichier/{f}"), "+deux");
        assert!(format!("{diff:?}").contains("+deux"), "{diff:?}");

        phone.action(h.state_mut(), &format!("{base}/modifs"), &format!("indexer:{f}"), None).unwrap();
        let s = wait_screen(&mut h, &mut phone, &format!("{base}/modifs"), "Valider…");
        assert!(s.action_ids().contains("valider"), "indexé par DragonTree : on peut valider");
        phone.action(h.state_mut(), &format!("{base}/modifs"), "valider", Some("Ajoute deux")).unwrap();
        let log = wait_screen(&mut h, &mut phone, &format!("{base}/historique"), "Ajoute deux");
        assert!(format!("{log:?}").contains("Ajoute deux") && format!("{log:?}").contains("Premier commit"), "{log:?}");
        assert!(format!("{log:?}").contains("Commit créé"), "le résultat de DragonTree : {log:?}");

        phone.action(h.state_mut(), &format!("{base}/branches"), "nouvelle-branche", Some("feature/essai")).unwrap();
        let b = wait_screen(&mut h, &mut phone, &format!("{base}/branches"), "Courante : feature/essai");
        assert!(b.action_ids().contains("basculer:main"), "{b:?}");
        if let Some(out) = std::env::var_os("SUITE_MOBILE_DUMP") {
            std::fs::write(repo.join("lisez-moi.txt"), "un\ndeux\ntrois\n").unwrap();
            std::fs::write(repo.join("nouveau.rs"), "fn main() {}\n").unwrap();
            phone.action(h.state_mut(), &base, "actualiser", None).unwrap();
            let out = Path::new(&out);
            for (name, path, wait) in [
                ("depot", base.clone(), "fichier(s)"),
                ("modifs", format!("{base}/modifs"), "Non indexé"),
                ("diff", format!("{base}/fichier/{f}"), "+trois"),
                ("historique", format!("{base}/historique"), "Ajoute deux"),
                ("branches", format!("{base}/branches"), "feature/essai"),
            ] {
                let s = wait_screen(&mut h, &mut phone, &path, wait);
                std::fs::write(out.join(format!("{name}.json")), serde_json::to_vec(&s).unwrap()).unwrap();
            }
        }
        phone.action(h.state_mut(), &format!("{base}/branches"), "nouvelle-branche", Some("pas valide..nom")).unwrap();
        let s = wait_screen(&mut h, &mut phone, &format!("{base}/branches"), "a échoué");
        assert!(s.blocks.iter().any(|b| matches!(b, Block::Note { tone: Tone::Danger, .. })), "nom refusé : {s:?}");

        // « Afficher dans DragonTree » : l'onglet du dépôt devient celui du PC.
        let (_, show) = phone.action(h.state_mut(), &base, "pc", None).unwrap();
        assert!(show);
        assert_eq!(h.state().active_repo_dir().map(path_key), Some(path_key(&repo)));
        drop(h);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
