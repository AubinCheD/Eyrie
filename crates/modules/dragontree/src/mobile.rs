//! DragonTree sur le téléphone : les dépôts de ses réglages (onglets ouverts, favoris, récents), leur branche, leur
//! avance ou retard et leurs modifications ; le détail d'un dépôt (modifications, branches locales, worktrees) ; fetch,
//! pull, push et « ouvrir sur le PC ».
//!
//! L'état de l'application n'est pas public : le téléphone lit les réglages de DragonTree (le fichier, sans le
//! réécrire) et interroge git lui-même par `dragontree-git`, toujours dans un fil (git est lent) : l'état des dépôts
//! est gardé et relu toutes les 30 s tant que le téléphone regarde, ou sur demande. Un dépôt est désigné par une
//! empreinte de son chemin (`depot/<clé>`) : pas de chemin Windows dans un chemin d'écran, et la même clé quand la
//! liste change d'ordre. « Ouvrir sur le PC » passe par le canal d'instance de DragonTree, comme `dragontree.exe
//! <dépôt>` lancé pendant que la suite tourne.

use std::collections::{HashMap, HashSet};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant, SystemTime};

use dragontree_app::settings::{Settings, path_key};
use dragontree_app::single_instance::{self, Forwarded};
use dragontree_git::{FileState, Git, HeadState, NetOptions, Repo, StatusEntry, StatusSnapshot};
use suite_sdk::mobile::ui::{Act, Row, Screen, Tone};

/// Dépôts montrés.
const MAX_REPOS: usize = 20;
/// Modifications montrées dans le détail d'un dépôt.
const MAX_PATHS: usize = 50;
/// L'état des dépôts est relu au plus toutes les 30 s tant que le téléphone regarde.
const SWEEP_EVERY: Duration = Duration::from_secs(30);
const SETTINGS_EVERY: Duration = Duration::from_secs(5);
/// Le résultat d'un fetch, d'un pull ou d'un push reste affiché une minute dans l'accueil.
const JOB_SHOWN: Duration = Duration::from_secs(60);

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

/// L'état d'un dépôt, relevé par git.
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

/// Une branche locale.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Branch {
    pub name: String,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    pub gone: bool,
    pub head: bool,
}

/// Un worktree.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Tree {
    pub path: PathBuf,
    pub branch: Option<String>,
    pub main: bool,
    pub current: bool,
    pub prunable: bool,
}

/// Le détail d'un dépôt, relevé quand son écran est ouvert.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Details {
    /// Les premières modifications : (lettre d'état, chemin, conflit).
    pub changes: Vec<(char, String, bool)>,
    pub branches: Vec<Branch>,
    pub worktrees: Vec<Tree>,
}

/// Ce qu'a donné (ou fait en ce moment) un fetch, un pull ou un push.
#[derive(Clone, Debug, PartialEq)]
pub struct Job {
    pub text: String,
    /// `Info` : en cours.
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

fn letter(e: &StatusEntry) -> char {
    if e.conflict.is_some() {
        return 'U';
    }
    let state = if e.worktree != FileState::Unmodified { e.worktree } else { e.index };
    match state {
        FileState::Modified => 'M',
        FileState::Added => 'A',
        FileState::Deleted => 'D',
        FileState::Renamed => 'R',
        FileState::Copied => 'C',
        FileState::TypeChanged => 'T',
        FileState::Untracked => '?',
        FileState::Ignored => '!',
        FileState::Unmodified => ' ',
    }
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

/// L'accueil : les dépôts par origine ; `sweeping` : leur état est en cours de relevé.
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
            row = row.act(Act::new(format!("pc:{key}"), "Ouvrir sur le PC").glyph("🖥️"));
            s.row(row);
        }
    }
    s
}

/// Le détail d'un dépôt.
pub fn depot(e: &Entry, details: Option<&Details>) -> Screen {
    let key = repo_key(&e.path);
    let mut s = Screen::new(repo_name(&e.path)).subtitle(e.path.display().to_string());
    let busy = e.job.as_ref().is_some_and(|j| j.tone == Tone::Info);
    if let Some(j) = &e.job {
        s.note(&j.text, j.tone);
    }
    match &e.state {
        None => {
            s.empty("Lecture de l'état du dépôt…");
        }
        Some(st) if st.error.is_some() => {
            s.note(st.error.clone().unwrap_or_default(), Tone::Danger);
        }
        Some(st) => {
            s.row(Row::new("Branche").detail(st.branch.clone().unwrap_or_else(|| "HEAD détachée".into())));
            s.row(Row::new("Branche suivie").detail(st.upstream.clone().unwrap_or_else(|| "aucune".into())));
            s.row(Row::new("Avance, retard").detail(sync_text(st.ahead, st.behind).unwrap_or_else(|| "à jour".into())));
        }
    }
    let mut acts = if busy { Vec::new() } else { net_acts(&key) };
    acts.push(Act::new(format!("pc:{key}"), "Ouvrir sur le PC").glyph("🖥️"));
    s.buttons(acts);
    let Some(d) = details else {
        s.empty("Lecture du détail…");
        return s;
    };
    let total = e.state.as_ref().map_or(d.changes.len(), |st| st.changes);
    s.section("Modifications", total.to_string());
    if d.changes.is_empty() {
        s.empty("Aucune modification.");
    }
    for (l, path, conflict) in &d.changes {
        let mut row = Row::new(path).glyph(l.to_string());
        if *conflict {
            row = row.tone(Tone::Danger).badge("conflit");
        }
        s.row(row);
    }
    if total > d.changes.len() {
        s.note(format!("Et {} autres : voir sur le PC.", total - d.changes.len()), Tone::Info);
    }
    s.section("Branches locales", d.branches.len().to_string());
    for b in &d.branches {
        let mut sub = match (&b.upstream, b.gone) {
            (Some(u), true) => format!("{u} (supprimée)"),
            (Some(u), false) => u.clone(),
            (None, _) => "non suivie".into(),
        };
        if let Some(t) = sync_text(b.ahead, b.behind) {
            sub = format!("{sub} · {t}");
        }
        let mut row = Row::new(&b.name).subtitle(sub).glyph("🌿");
        if b.head {
            row = row.badge("courante").tone(Tone::Ok);
        }
        s.row(row);
    }
    if d.worktrees.len() > 1 {
        s.section("Worktrees", d.worktrees.len().to_string());
        for w in &d.worktrees {
            let mut row = Row::new(repo_name(&w.path))
                .subtitle(w.branch.clone().unwrap_or_else(|| "HEAD détachée".into()))
                .detail(if w.main { "principal" } else { "" })
                .glyph("🌳");
            if w.current {
                row = row.badge("celui-ci");
            }
            if w.prunable {
                row = row.tone(Tone::Warn).subtitle("dossier disparu");
            }
            s.row(row);
        }
    }
    s
}

// --- Le travail de fond ---

/// Ce que les fils de git ont relevé ou fait, lu par l'interface.
#[derive(Default)]
struct Work {
    states: HashMap<String, RepoState>,
    details: HashMap<String, (Instant, Details)>,
    inspecting: HashSet<String>,
    jobs: HashMap<String, Job>,
    sweep_at: Option<Instant>,
    sweeping: bool,
}

type Shared = Arc<Mutex<Work>>;

fn lock(w: &Shared) -> MutexGuard<'_, Work> {
    w.lock().unwrap_or_else(|e| e.into_inner())
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

fn open(git: &Git, path: &Path) -> Result<Repo, String> {
    if !path.is_dir() {
        return Err("dossier introuvable".into());
    }
    Repo::open(git, path).map_err(|e| e.to_string())
}

fn state_of(status: Result<&StatusSnapshot, String>) -> RepoState {
    match status {
        Ok(s) => RepoState {
            branch: s.branch.clone(),
            upstream: s.upstream.clone(),
            ahead: s.ahead,
            behind: s.behind,
            changes: s.entries.len(),
            error: None,
        },
        Err(e) => RepoState { error: Some(e), ..Default::default() },
    }
}

fn read_state(git: &Result<Git, String>, path: &Path) -> RepoState {
    let status = git.clone().and_then(|g| open(&g, path)).and_then(|r| r.status().map_err(|e| e.to_string()));
    state_of(status.as_ref().map_err(String::clone))
}

fn read_details(git: &Result<Git, String>, path: &Path) -> (RepoState, Details) {
    let repo = match git.clone().and_then(|g| open(&g, path)) {
        Ok(r) => r,
        Err(e) => return (RepoState { error: Some(e), ..Default::default() }, Details::default()),
    };
    let status = repo.status().map_err(|e| e.to_string());
    let state = state_of(status.as_ref().map_err(String::clone));
    let mut d = Details::default();
    if let Ok(s) = &status {
        d.changes =
            s.entries.iter().take(MAX_PATHS).map(|e| (letter(e), e.path.clone(), e.conflict.is_some())).collect();
    }
    if let Ok(refs) = repo.refs() {
        let head = match &refs.head {
            HeadState::Branch(b) | HeadState::Unborn(b) => Some(b.clone()),
            HeadState::Detached(_) => None,
        };
        d.branches = refs
            .locals
            .iter()
            .map(|b| Branch {
                name: b.name.clone(),
                upstream: b.upstream.clone(),
                ahead: b.ahead,
                behind: b.behind,
                gone: b.upstream_gone,
                head: b.is_head || head.as_deref() == Some(b.name.as_str()),
            })
            .collect();
    }
    if let Ok(list) = repo.worktrees() {
        d.worktrees = list
            .into_iter()
            .map(|w| Tree {
                path: w.path,
                branch: w.branch,
                main: w.is_main,
                current: w.is_current,
                prunable: w.prunable,
            })
            .collect();
    }
    (state, d)
}

/// Fetch, pull (avance rapide seulement) ou push de la branche courante, sans fenêtre d'authentification.
fn net(git: &Result<Git, String>, path: &Path, op: &str) -> Result<String, String> {
    let repo = git.clone().and_then(|g| open(&g, path))?;
    let n = NetOptions { non_interactive: true, ..Default::default() };
    let err = |e: dragontree_git::GitError| e.to_string();
    if op == "fetch" {
        repo.fetch(None, false, &n, &mut |_| {}).map_err(err)?;
        return Ok("fetch terminé".into());
    }
    let s = repo.status().map_err(err)?;
    let branch = s.branch.ok_or("HEAD détachée : rien à tirer ni à pousser.")?;
    let upstream = s.upstream.ok_or("Pas de branche distante suivie : publiez-la depuis le PC.")?;
    // `origin/feature/x` : le remote est le plus long nom de remote qui préfixe la branche suivie.
    let remotes = repo.remotes().map_err(err)?;
    let remote = remotes
        .iter()
        .filter(|r| upstream.starts_with(&format!("{}/", r.name)))
        .max_by_key(|r| r.name.len())
        .ok_or("Remote de la branche suivie introuvable.")?;
    let remote_branch = &upstream[remote.name.len() + 1..];
    match op {
        "pull" => {
            if s.ahead > 0 && s.behind > 0 {
                return Err("Branches divergentes : à régler sur le PC.".into());
            }
            if s.behind == 0 {
                return Ok("déjà à jour".into());
            }
            repo.pull(&remote.name, remote_branch, false, &n, &mut |_| {}).map_err(err)?;
            Ok(format!("pull terminé ({} commit(s))", s.behind))
        }
        "push" => {
            if s.behind > 0 {
                return Err("En retard sur la branche distante : faites d'abord un pull.".into());
            }
            if s.ahead == 0 {
                return Ok("rien à pousser".into());
            }
            repo.push(&remote.name, &branch, remote_branch, false, false, &n, &mut |_| {}).map_err(err)?;
            Ok(format!("push terminé ({} commit(s))", s.ahead))
        }
        _ => Err("action inconnue".into()),
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

/// Ce que le téléphone garde entre deux appels.
pub struct Phone {
    settings_path: PathBuf,
    /// Clé du profil de DragonTree : son canal d'instance (« ouvrir sur le PC »).
    instance: String,
    checked: Option<Instant>,
    stamp: Option<SystemTime>,
    lists: Lists,
    work: Shared,
    git: GitCell,
}

impl Phone {
    pub fn new(settings_path: PathBuf, instance: String) -> Phone {
        Phone {
            settings_path,
            instance,
            checked: None,
            stamp: None,
            lists: Lists::default(),
            work: Shared::default(),
            git: GitCell::default(),
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

    /// Relève l'état de `paths` dans un fil, s'il est dû (`force` : tout de suite).
    fn sweep(&self, paths: Vec<PathBuf>, force: bool) {
        let mut w = lock(&self.work);
        if w.sweeping || (!force && w.sweep_at.is_some_and(|t| t.elapsed() < SWEEP_EVERY)) {
            return;
        }
        w.sweeping = true;
        drop(w);
        let (work, cell, configured) = (self.work.clone(), self.git.clone(), self.lists.git_path.clone());
        std::thread::spawn(move || {
            let git = git(&cell, configured);
            for p in paths {
                let state = read_state(&git, &p);
                lock(&work).states.insert(repo_key(&p), state);
            }
            let mut w = lock(&work);
            w.sweeping = false;
            w.sweep_at = Some(Instant::now());
        });
    }

    /// Relève le détail d'un dépôt dans un fil, s'il est dû.
    fn inspect(&self, path: &Path, force: bool) {
        let key = repo_key(path);
        let mut w = lock(&self.work);
        let fresh = w.details.get(&key).is_some_and(|(t, _)| t.elapsed() < SWEEP_EVERY);
        if (fresh && !force) || !w.inspecting.insert(key.clone()) {
            return;
        }
        drop(w);
        let (work, cell, configured, path) =
            (self.work.clone(), self.git.clone(), self.lists.git_path.clone(), path.to_path_buf());
        std::thread::spawn(move || {
            let (state, details) = read_details(&git(&cell, configured), &path);
            let mut w = lock(&work);
            w.inspecting.remove(&key);
            w.states.insert(key.clone(), state);
            w.details.insert(key, (Instant::now(), details));
        });
    }

    fn entry(&self, w: &Work, (path, origin, active): (PathBuf, Origin, bool)) -> Entry {
        let key = repo_key(&path);
        Entry { state: w.states.get(&key).cloned(), job: w.jobs.get(&key).cloned(), path, origin, active }
    }

    pub fn screen(&mut self, path: &str, active: Option<&Path>) -> Option<Screen> {
        let repos = self.repos(active);
        if path.is_empty() {
            self.sweep(repos.iter().map(|r| r.0.clone()).collect(), false);
            let w = lock(&self.work);
            let entries: Vec<Entry> = repos.into_iter().map(|r| self.entry(&w, r)).collect();
            return Some(home(&entries, w.sweeping));
        }
        let key = path.strip_prefix("depot/")?;
        let repo = repos.into_iter().find(|r| repo_key(&r.0) == key)?;
        self.inspect(&repo.0, false);
        let w = lock(&self.work);
        let details = w.details.get(key).map(|(_, d)| d.clone());
        let e = self.entry(&w, repo);
        Some(depot(&e, details.as_ref()))
    }

    /// Une action ; `Ok(Some(dépôt))` pour « ouvrir sur le PC » : le dépôt transmis à DragonTree.
    pub fn action(&mut self, action: &str, active: Option<&Path>) -> Result<(String, Option<PathBuf>), String> {
        let repos = self.repos(active);
        if action == "actualiser" {
            self.sweep(repos.into_iter().map(|r| r.0).collect(), true);
            return Ok(("Actualisation lancée".into(), None));
        }
        let (op, key) = action.split_once(':').ok_or("action inconnue")?;
        let path = repos
            .into_iter()
            .find(|r| repo_key(&r.0) == key)
            .map(|r| r.0)
            .ok_or("Ce dépôt n'est plus dans la liste.")?;
        let name = repo_name(&path);
        if op == "pc" {
            let (instance, target) = (self.instance.clone(), path.clone());
            // Le canal d'instance est servi par un fil de DragonTree, dans ce même processus : jamais attendu ici.
            std::thread::spawn(move || {
                let _ = single_instance::send(&instance, &Forwarded { open: vec![target] }, Duration::from_secs(3));
            });
            return Ok((format!("{name} ouvert sur le PC"), Some(path)));
        }
        let label = match op {
            "fetch" => "Fetch",
            "pull" => "Pull",
            "push" => "Push",
            _ => return Err("action inconnue".into()),
        };
        let mut w = lock(&self.work);
        if w.jobs.get(key).is_some_and(|j| j.tone == Tone::Info) {
            return Err(format!("{name} : une opération est déjà en cours."));
        }
        w.jobs
            .insert(key.to_string(), Job { text: format!("{label} en cours…"), tone: Tone::Info, at: Instant::now() });
        drop(w);
        let (work, cell, configured, op, key) =
            (self.work.clone(), self.git.clone(), self.lists.git_path.clone(), op.to_string(), key.to_string());
        std::thread::spawn(move || {
            let git = git(&cell, configured);
            let done = net(&git, &path, &op);
            let (state, details) = read_details(&git, &path);
            let job = match done {
                Ok(text) => Job { text, tone: Tone::Ok, at: Instant::now() },
                Err(text) => Job { text, tone: Tone::Danger, at: Instant::now() },
            };
            let mut w = lock(&work);
            w.jobs.insert(key.clone(), job);
            w.states.insert(key.clone(), state);
            w.details.insert(key, (Instant::now(), details));
        });
        Ok((format!("{label} lancé : {name}"), None))
    }
}

#[cfg(test)]
mod tests {
    use suite_sdk::mobile::ui::Block;

    use super::*;

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

    #[test]
    fn a_repo_shows_its_changes_branches_and_worktrees() {
        let e = entry(r"C:\dev\ozmo", Origin::Tab, Some(state("main", 0, 0, 60)));
        let d = Details {
            changes: (0..MAX_PATHS).map(|i| ('M', format!("src/f{i}.rs"), i == 0)).collect(),
            branches: vec![
                Branch { name: "main".into(), upstream: Some("origin/main".into()), head: true, ..Default::default() },
                Branch {
                    name: "vieille".into(),
                    upstream: Some("origin/vieille".into()),
                    gone: true,
                    ..Default::default()
                },
            ],
            worktrees: vec![
                Tree {
                    path: r"C:\dev\ozmo".into(),
                    branch: Some("main".into()),
                    main: true,
                    current: true,
                    prunable: false,
                },
                Tree { path: r"C:\dev\ozmo-wt".into(), branch: Some("essai".into()), ..Default::default() },
            ],
        };
        let s = depot(&e, Some(&d));
        assert!(s.blocks.contains(&Block::Section { title: "Modifications".into(), aside: "60".into() }));
        assert!(
            s.blocks.iter().any(|b| matches!(b, Block::Note { text, .. } if text == "Et 10 autres : voir sur le PC."))
        );
        assert!(s.blocks.iter().any(|b| matches!(b, Block::Row(r) if r.subtitle == "origin/vieille (supprimée)")));
        assert!(s.blocks.contains(&Block::Section { title: "Worktrees".into(), aside: "2".into() }));
        assert_eq!(s.action_ids().len(), 4, "fetch, pull, push, ouvrir sur le PC");
        let mut busy = e.clone();
        busy.job = Some(Job { text: "Push en cours…".into(), tone: Tone::Info, at: Instant::now() });
        assert_eq!(depot(&busy, None).action_ids().len(), 1, "pendant une opération : seulement ouvrir sur le PC");
    }

    #[test]
    fn an_operation_runs_in_a_thread_and_reports_its_result() {
        let dir = std::env::temp_dir().join(format!("suite-dragontree-telephone-{}", std::process::id()));
        let repo = dir.join("pas-un-depot");
        std::fs::create_dir_all(&repo).unwrap();
        let settings = Settings { recent_repos: vec![repo.clone()], ..Default::default() };
        let file = dir.join("settings.json");
        std::fs::write(&file, serde_json::to_vec(&settings).unwrap()).unwrap();
        let mut phone = Phone::new(file, "essai".into());
        let key = repo_key(&repo);
        assert!(phone.action("fetch:inconnu", None).is_err());
        assert_eq!(phone.action(&format!("fetch:{key}"), None).map(|r| r.1), Ok(None));
        assert!(phone.action(&format!("fetch:{key}"), None).is_err(), "une seule opération à la fois");
        let deadline = Instant::now() + Duration::from_secs(20);
        while lock(&phone.work).jobs[&key].tone == Tone::Info && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(lock(&phone.work).jobs[&key].tone, Tone::Danger, "pas un dépôt git");
        let s = phone.screen(&format!("depot/{key}"), None).unwrap();
        assert!(s.blocks.iter().any(|b| matches!(b, Block::Note { tone: Tone::Danger, .. })));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
