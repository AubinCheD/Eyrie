//! DragonTree sur le téléphone : les dépôts de ses réglages (onglets ouverts, favoris, récents), leur branche, leur
//! avance ou retard et leurs modifications ; fetch, pull, push et « ouvrir sur le PC » depuis la liste ; et, dans un
//! dépôt, un client git presque complet (`depot.rs` : modifications et diff, indexer, valider, historique, branches,
//! fusion, rebase, remises, étiquettes, worktrees, opération en cours).
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
use dragontree_git::{
    CancelToken, DiffOptions, FileState, Git, LogOptions, LogScope, MergeMode, NetOptions, Oid, Repo, ResetMode,
    StatusSnapshot,
};
use suite_sdk::mobile::ui::{Act, Block, Row, Screen, Tone};

use crate::depot::{self, Op, RepoView, file_key};

/// Dépôts montrés.
const MAX_REPOS: usize = 20;
/// L'état des dépôts est relu au plus toutes les 30 s tant que le téléphone regarde.
const SWEEP_EVERY: Duration = Duration::from_secs(30);
const SETTINGS_EVERY: Duration = Duration::from_secs(5);
/// Le résultat d'une opération reste affiché une minute.
const JOB_SHOWN: Duration = Duration::from_secs(60);
/// Ce qu'un écran du dépôt relit tant qu'il est regardé : l'état, l'historique, un diff.
const OVERVIEW_EVERY: Duration = Duration::from_secs(20);
const LOG_EVERY: Duration = Duration::from_secs(60);
const DIFF_EVERY: Duration = Duration::from_secs(15);

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

// --- Le travail de fond ---

/// Ce que les fils de git ont relevé ou fait, lu par l'interface.
#[derive(Default)]
struct Work {
    states: HashMap<String, RepoState>,
    /// Ce qui est relevé de chaque dépôt ouvert sur le téléphone.
    views: HashMap<String, RepoView>,
    /// (dépôt, relevé) → quand il a été fait ; en cours : absent de `loaded`, présent dans `loading`.
    loaded: HashMap<(String, String), Instant>,
    loading: HashSet<(String, String)>,
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

/// Ce qu'un écran du dépôt a besoin de relever.
#[derive(Clone, Debug, PartialEq)]
enum Load {
    /// État, branches, remises, worktrees, remotes, opération en cours, dernier message.
    Overview,
    Log,
    Commit(String),
    /// Diff d'un fichier du dossier de travail (indexé et non indexé).
    WorkDiff(String),
    /// Diff d'un fichier d'un commit.
    CommitDiff(String, String),
}

impl Load {
    fn key(&self) -> String {
        match self {
            Load::Overview => "etat".into(),
            Load::Log => "historique".into(),
            Load::Commit(id) => format!("commit:{id}"),
            Load::WorkDiff(f) => format!("w:{f}"),
            Load::CommitDiff(id, f) => format!("c:{id}:{f}"),
        }
    }

    fn every(&self) -> Option<Duration> {
        match self {
            Load::Overview => Some(OVERVIEW_EVERY),
            Load::Log => Some(LOG_EVERY),
            Load::WorkDiff(_) => Some(DIFF_EVERY),
            // Un commit ne change pas.
            Load::Commit(_) | Load::CommitDiff(..) => None,
        }
    }
}

fn read_overview(repo: &Repo, v: &mut RepoView) {
    v.status = Some(repo.status().map_err(|e| e.to_string()));
    v.refs = repo.refs().ok();
    v.stashes = repo.stashes().unwrap_or_default();
    v.worktrees = repo.worktrees().unwrap_or_default();
    v.remotes = repo.remotes().map(|r| r.into_iter().map(|r| r.name).collect()).unwrap_or_default();
    v.op = Some(repo.operation_state());
    v.last_message = repo.last_commit_message().unwrap_or_default();
}

fn oid(hex: &str) -> Result<Oid, String> {
    Oid::from_hex(hex).ok_or_else(|| "identifiant de commit illisible".into())
}

/// Relève `what` dans `v` (une copie de ce qu'on sait : les fichiers d'un commit, les modifications).
fn read(repo: &Repo, what: &Load, v: &mut RepoView) {
    let opts = DiffOptions::default();
    match what {
        Load::Overview => read_overview(repo, v),
        Load::Log => {
            let o = LogOptions { scope: LogScope::Head, max_count: Some(depot::LOG_COUNT), ..Default::default() };
            v.log = Some(repo.log(&o, &CancelToken::new()).map_err(|e| e.to_string()));
        }
        Load::Commit(id) => {
            let r = oid(id).and_then(|o| {
                let info = repo.commit_info(o).map_err(|e| e.to_string())?;
                let files = repo.commit_files(o, info.parents.first().copied()).map_err(|e| e.to_string())?;
                Ok((info, files))
            });
            v.commits.insert(id.clone(), r);
        }
        Load::WorkDiff(f) => {
            let Some(e) = v.entry(f).cloned() else { return };
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
            let r = match (parts.is_empty(), error) {
                (true, Some(e)) => Err(e),
                _ => Ok(parts),
            };
            v.diffs.insert(format!("w:{f}"), r);
        }
        Load::CommitDiff(id, f) => {
            let Some(Ok((info, files))) = v.commits.get(id) else { return };
            let Some(change) = files.iter().find(|c| file_key(&c.path) == *f).cloned() else { return };
            let parent = info.parents.first().copied();
            let r = oid(id).and_then(|o| {
                repo.diff_commit_file(o, parent, &change, &opts)
                    .map(|d| depot::render_diff(&d))
                    .map_err(|e| e.to_string())
            });
            v.diffs.insert(format!("c:{id}:{f}"), r);
        }
    }
}

/// Fetch, pull (avance rapide, ou rebase) ou push (forcé avec bail) de la branche courante, sans fenêtre
/// d'authentification.
fn net(repo: &Repo, op: &str) -> Result<String, String> {
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
        "pull" | "pull-rebase" => {
            let rebase = op == "pull-rebase";
            if s.ahead > 0 && s.behind > 0 && !rebase {
                return Err("Branches divergentes : faites un pull avec rebase, ou réglez-le sur le PC.".into());
            }
            if s.behind == 0 {
                return Ok("déjà à jour".into());
            }
            repo.pull(&remote.name, remote_branch, rebase, &n, &mut |_| {}).map_err(err)?;
            Ok(format!("pull terminé ({} commit(s))", s.behind))
        }
        "push" | "push-force" => {
            let force = op == "push-force";
            if s.behind > 0 && !force {
                return Err("En retard sur la branche distante : faites d'abord un pull.".into());
            }
            if s.ahead == 0 {
                return Ok("rien à pousser".into());
            }
            repo.push(&remote.name, &branch, remote_branch, false, force, &n, &mut |_| {}).map_err(err)?;
            Ok(format!("push terminé ({} commit(s))", s.ahead))
        }
        _ => Err("action inconnue".into()),
    }
}

/// Exécute une opération du dépôt ; le texte dit ce qui s'est passé.
fn run(repo: &Repo, op: &Op) -> Result<String, String> {
    let e = |e: dragontree_git::GitError| e.to_string();
    let n = NetOptions { non_interactive: true, ..Default::default() };
    let branch_name = |name: &str| {
        if repo.is_valid_branch_name(name) {
            Ok(())
        } else {
            Err(format!("« {name} » n'est pas un nom de branche valide."))
        }
    };
    match op {
        Op::Fetch => net(repo, "fetch"),
        Op::Pull { rebase } => net(repo, if *rebase { "pull-rebase" } else { "pull" }),
        Op::Push { force } => net(repo, if *force { "push-force" } else { "push" }),
        Op::Stage(p) => repo.stage(p).map(|_| "indexé".into()).map_err(e),
        Op::Unstage(p) => repo.unstage(p).map(|_| "désindexé".into()).map_err(e),
        Op::Discard { tracked, untracked } => repo.discard(tracked, untracked).map(|_| "annulé".into()).map_err(e),
        Op::Commit { message, amend } => {
            repo.commit(message, *amend).map(|_| if *amend { "commit corrigé" } else { "validé" }.into()).map_err(e)
        }
        Op::StashSave(m) => repo.stash_save(m.as_deref(), true, false).map(|_| "mis de côté".into()).map_err(e),
        Op::StashApply(h) => repo.stash_apply_oid(oid(h)?).map(|_| "remise appliquée".into()).map_err(e),
        Op::StashPop(h) => repo.stash_pop_oid(oid(h)?).map(|_| "remise appliquée et retirée".into()).map_err(e),
        Op::StashDrop(h) => repo.stash_drop_oid(oid(h)?).map(|_| "remise jetée".into()).map_err(e),
        Op::Checkout(b) => repo.checkout(b).map(|_| format!("sur {b}")).map_err(e),
        Op::CheckoutDetached(id) => repo.checkout(id).map(|_| "HEAD détachée sur ce commit".into()).map_err(e),
        Op::Track { remote_branch, local } => {
            branch_name(local)?;
            repo.checkout_new_tracking(remote_branch, local).map(|_| format!("sur {local}")).map_err(e)
        }
        Op::CreateBranch { name, start } => {
            branch_name(name)?;
            repo.create_branch(name, start, true).map(|_| "créée et sortie".into()).map_err(e)
        }
        Op::RenameBranch { old, new } => {
            branch_name(new)?;
            repo.rename_branch(old, new).map(|_| format!("renommée en {new}")).map_err(e)
        }
        Op::DeleteBranch { name, force } => repo.delete_branch(name, *force).map(|_| "supprimée".into()).map_err(e),
        Op::PushBranch { name, remote } => repo
            .push(remote, name, name, true, false, &n, &mut |_| {})
            .map(|_| format!("poussée vers {remote}"))
            .map_err(e),
        Op::Merge(r) => repo.merge(r, MergeMode::Default).map(|_| "fusionné".into()).map_err(e),
        Op::Rebase(r) => repo.rebase(r).map(|_| "rebasé".into()).map_err(e),
        Op::CherryPick(h) => repo.cherry_pick(&[oid(h)?]).map(|_| "appliqué".into()).map_err(e),
        Op::Revert(h) => repo.revert(oid(h)?).map(|_| "commit d'annulation créé".into()).map_err(e),
        Op::Reset { rev, mode } => {
            let mode = match mode {
                depot::Reset::Soft => ResetMode::Soft,
                depot::Reset::Mixed => ResetMode::Mixed,
                depot::Reset::Hard => ResetMode::Hard,
            };
            repo.reset(rev, mode).map(|_| "branche ramenée".into()).map_err(e)
        }
        Op::Continue => repo.continue_op(repo.operation_state()).map(|_| "poursuivi".into()).map_err(e),
        Op::Abort => repo.abort_op(repo.operation_state()).map(|_| "abandonné".into()).map_err(e),
        Op::Skip => repo.skip_rebase_commit().map(|_| "commit passé".into()).map_err(e),
        Op::CreateTag { name, rev } => repo.create_tag(name, rev, None).map(|_| "étiquette créée".into()).map_err(e),
        Op::DeleteTag(name) => repo.delete_tag(name).map(|_| "étiquette supprimée".into()).map_err(e),
        Op::PushTag { name, remote } => {
            repo.push_tag(remote, name, &n, &mut |_| {}).map(|_| format!("poussée vers {remote}")).map_err(e)
        }
        Op::RemoveWorktree(p) => repo.remove_worktree(p, false).map(|_| "worktree supprimé".into()).map_err(e),
        Op::PruneWorktrees => repo.prune_worktrees().map(|_| "élagués".into()).map_err(e),
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

    /// Relève `what` pour le dépôt `path` dans un fil, s'il est dû (`force` : tout de suite).
    fn want(&self, path: &Path, what: Load, force: bool) {
        let key = (repo_key(path), what.key());
        let mut w = lock(&self.work);
        let fresh = w.loaded.get(&key).is_some_and(|t| what.every().is_none_or(|every| t.elapsed() < every));
        if (fresh && !force) || !w.loading.insert(key.clone()) {
            return;
        }
        let mut view = w.views.get(&key.0).cloned().unwrap_or_default();
        drop(w);
        let (work, cell, configured, path) =
            (self.work.clone(), self.git.clone(), self.lists.git_path.clone(), path.to_path_buf());
        std::thread::spawn(move || {
            let repo = git(&cell, configured).and_then(|g| open(&g, &path));
            match &repo {
                Ok(r) => read(r, &what, &mut view),
                Err(e) => view.status = Some(Err(e.clone())),
            }
            let mut w = lock(&work);
            w.loading.remove(&key);
            w.loaded.insert(key.clone(), Instant::now());
            let current = w.views.entry(key.0.clone()).or_default();
            // Seulement ce que ce relevé concerne : un autre relevé a pu écrire le reste entre-temps.
            match &what {
                Load::Overview => {
                    current.status = view.status.clone();
                    current.refs = view.refs;
                    current.stashes = view.stashes;
                    current.worktrees = view.worktrees;
                    current.remotes = view.remotes;
                    current.op = view.op;
                    current.last_message = view.last_message;
                }
                Load::Log => current.log = view.log,
                Load::Commit(id) => {
                    if let Some(c) = view.commits.remove(id) {
                        current.commits.insert(id.clone(), c);
                    }
                }
                Load::WorkDiff(_) | Load::CommitDiff(..) => {
                    let k = what.key();
                    if let Some(d) = view.diffs.remove(&k) {
                        current.diffs.insert(k, d);
                    }
                }
            }
            if let Load::Overview = what {
                let state = state_of(
                    view.status
                        .as_ref()
                        .map(|r| r.as_ref().map_err(String::clone))
                        .unwrap_or(Err("dépôt illisible".into())),
                );
                w.states.insert(key.0, state);
            }
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
        let rest = path.strip_prefix("depot/")?;
        let (key, sub) = rest.split_once('/').unwrap_or((rest, ""));
        let repo = repos.into_iter().find(|r| repo_key(&r.0) == key)?.0;
        self.want(&repo, Load::Overview, false);
        let parts: Vec<&str> = sub.split('/').collect();
        match parts.as_slice() {
            ["fichier", f] => self.want(&repo, Load::WorkDiff(f.to_string()), false),
            ["historique"] => self.want(&repo, Load::Log, false),
            ["commit", id] => self.want(&repo, Load::Commit(id.to_string()), false),
            ["commit", id, f] => {
                self.want(&repo, Load::Commit(id.to_string()), false);
                self.want(&repo, Load::CommitDiff(id.to_string(), f.to_string()), false);
            }
            _ => {}
        }
        let w = lock(&self.work);
        let view = w.views.get(key).cloned().unwrap_or_default();
        let job = w.jobs.get(key).filter(|j| j.at.elapsed() < JOB_SHOWN || j.tone == Tone::Info).cloned();
        drop(w);
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
        // Ce qu'a donné la dernière opération, en tête de chaque écran du dépôt.
        if let Some(j) = job {
            screen.blocks.insert(0, Block::Note { text: j.text, tone: j.tone });
        }
        Some(screen)
    }

    /// Une action de la liste (`fetch:<clé>`…) ou d'un écran du dépôt (`depot/<clé>/…`). `Ok(Some(dépôt))` pour
    /// « ouvrir sur le PC » : le dépôt transmis à DragonTree.
    pub fn action(
        &mut self,
        path: &str,
        action: &str,
        input: Option<&str>,
        active: Option<&Path>,
    ) -> Result<(String, Option<PathBuf>), String> {
        let repos = self.repos(active);
        let in_repo = path.strip_prefix("depot/").map(|r| r.split_once('/').map_or(r, |(k, _)| k).to_string());
        if action == "actualiser" && in_repo.is_none() {
            self.sweep(repos.into_iter().map(|r| r.0).collect(), true);
            return Ok(("Actualisation lancée".into(), None));
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
            let (instance, target) = (self.instance.clone(), repo.clone());
            // Le canal d'instance est servi par un fil de DragonTree, dans ce même processus : jamais attendu ici.
            std::thread::spawn(move || {
                let _ = single_instance::send(&instance, &Forwarded { open: vec![target] }, Duration::from_secs(3));
            });
            return Ok((format!("{name} ouvert sur le PC"), Some(repo)));
        }
        if verb == "actualiser" {
            self.want(&repo, Load::Overview, true);
            if lock(&self.work).loaded.contains_key(&(key.clone(), Load::Log.key())) {
                self.want(&repo, Load::Log, true);
            }
            return Ok(("Relecture du dépôt…".into(), None));
        }
        let view = lock(&self.work).views.get(&key).cloned().unwrap_or_default();
        let (op, label) = depot::parse(&view, &verb, input)?;
        let mut w = lock(&self.work);
        if w.jobs.get(&key).is_some_and(|j| j.tone == Tone::Info) {
            return Err(format!("{name} : une opération est déjà en cours."));
        }
        w.jobs.insert(key.clone(), Job { text: format!("{label} en cours…"), tone: Tone::Info, at: Instant::now() });
        drop(w);
        let (work, cell, configured) = (self.work.clone(), self.git.clone(), self.lists.git_path.clone());
        let log_shown = lock(&self.work).loaded.contains_key(&(key.clone(), Load::Log.key()));
        let reply = format!("{label} lancé : {name}");
        std::thread::spawn(move || {
            let repo = git(&cell, configured).and_then(|g| open(&g, &repo));
            let done = repo.as_ref().map_err(String::clone).and_then(|r| run(r, &op));
            // Après l'opération : l'état, l'historique s'il est montré ; les diffs du dossier de travail sont relus.
            let mut fresh = RepoView::default();
            if let Ok(r) = &repo {
                read_overview(r, &mut fresh);
                if log_shown {
                    read(r, &Load::Log, &mut fresh);
                }
            }
            let job = match done {
                Ok(text) => Job { text: format!("{label} : {text}"), tone: Tone::Ok, at: Instant::now() },
                Err(text) => Job { text: format!("{label} : {text}"), tone: Tone::Danger, at: Instant::now() },
            };
            let mut w = lock(&work);
            w.jobs.insert(key.clone(), job);
            if repo.is_ok() {
                w.states.insert(
                    key.clone(),
                    state_of(
                        fresh
                            .status
                            .as_ref()
                            .map(|r| r.as_ref().map_err(String::clone))
                            .unwrap_or(Err("dépôt illisible".into())),
                    ),
                );
                let current = w.views.entry(key.clone()).or_default();
                current.status = fresh.status;
                current.refs = fresh.refs;
                current.stashes = fresh.stashes;
                current.worktrees = fresh.worktrees;
                current.remotes = fresh.remotes;
                current.op = fresh.op;
                current.last_message = fresh.last_message;
                if log_shown {
                    current.log = fresh.log;
                }
                current.diffs.retain(|k, _| !k.starts_with("w:"));
                let now = Instant::now();
                w.loaded.insert((key.clone(), Load::Overview.key()), now);
                w.loaded.retain(|(k, what), _| !(k == &key && what.starts_with("w:")));
            }
        });
        Ok((reply, None))
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

    /// Attend la fin de l'opération en cours sur ce dépôt ; rend son ton.
    fn wait_job(phone: &Phone, key: &str) -> Tone {
        let deadline = Instant::now() + Duration::from_secs(30);
        while lock(&phone.work).jobs.get(key).is_none_or(|j| j.tone == Tone::Info) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        lock(&phone.work).jobs[key].tone
    }

    /// Attend qu'un écran contienne `text`.
    fn wait_screen(phone: &mut Phone, path: &str, text: &str) -> Screen {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let s = phone.screen(path, None).expect("écran");
            if format!("{s:?}").contains(text) || Instant::now() > deadline {
                return s;
            }
            std::thread::sleep(Duration::from_millis(30));
        }
    }

    fn phone_for(dir: &Path, repo: &Path) -> Phone {
        let settings = Settings { recent_repos: vec![repo.to_path_buf()], ..Default::default() };
        let file = dir.join("settings.json");
        std::fs::write(&file, serde_json::to_vec(&settings).unwrap()).unwrap();
        Phone::new(file, "essai".into())
    }

    #[test]
    fn an_operation_runs_in_a_thread_and_reports_its_result() {
        let dir = std::env::temp_dir().join(format!("suite-dragontree-telephone-{}", std::process::id()));
        let repo = dir.join("pas-un-depot");
        std::fs::create_dir_all(&repo).unwrap();
        let mut phone = phone_for(&dir, &repo);
        let key = repo_key(&repo);
        assert!(phone.action("", "fetch:inconnu", None, None).is_err());
        assert_eq!(phone.action("", &format!("fetch:{key}"), None, None).map(|r| r.1), Ok(None));
        assert!(phone.action("", &format!("fetch:{key}"), None, None).is_err(), "une seule opération à la fois");
        assert_eq!(wait_job(&phone, &key), Tone::Danger, "pas un dépôt git");
        let s = phone.screen(&format!("depot/{key}"), None).unwrap();
        assert!(s.blocks.iter().any(|b| matches!(b, Block::Note { tone: Tone::Danger, .. })));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Un vrai dépôt : modifier, voir le diff, indexer, valider, retrouver le commit, créer une branche.
    #[test]
    fn a_change_is_staged_committed_and_found_in_the_history() {
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
        std::fs::write(
            repo.join("lisez-moi.txt"),
            "un
",
        )
        .unwrap();
        sh(&["add", "."]);
        sh(&["commit", "-q", "-m", "Premier commit"]);
        std::fs::write(
            repo.join("lisez-moi.txt"),
            "un
deux
",
        )
        .unwrap();

        let mut phone = phone_for(&dir, &repo);
        let key = repo_key(&repo);
        let base = format!("depot/{key}");
        let s = wait_screen(&mut phone, &base, "1 fichier(s), dont 0 indexé(s)");
        assert!(format!("{s:?}").contains("1 fichier(s), dont 0 indexé(s)"), "{s:?}");
        let f = file_key("lisez-moi.txt");
        let diff = wait_screen(&mut phone, &format!("{base}/fichier/{f}"), "+deux");
        assert!(format!("{diff:?}").contains("+deux"), "{diff:?}");

        phone.action(&format!("{base}/modifs"), &format!("indexer:{f}"), None, None).unwrap();
        assert_eq!(wait_job(&phone, &key), Tone::Ok);
        let s = phone.screen(&format!("{base}/modifs"), None).unwrap();
        assert!(s.action_ids().contains("valider"), "indexé : on peut valider");
        phone.action(&format!("{base}/modifs"), "valider", Some("Ajoute deux"), None).unwrap();
        assert_eq!(wait_job(&phone, &key), Tone::Ok, "{:?}", lock(&phone.work).jobs[&key]);
        let log = wait_screen(&mut phone, &format!("{base}/historique"), "Ajoute deux");
        assert!(format!("{log:?}").contains("Ajoute deux") && format!("{log:?}").contains("Premier commit"));

        phone.action(&format!("{base}/branches"), "nouvelle-branche", Some("feature/essai"), None).unwrap();
        assert_eq!(wait_job(&phone, &key), Tone::Ok);
        let b = wait_screen(&mut phone, &format!("{base}/branches"), "Courante : feature/essai");
        if let Some(out) = std::env::var_os("SUITE_MOBILE_DUMP") {
            std::fs::write(
                repo.join("lisez-moi.txt"),
                "un
deux
trois
",
            )
            .unwrap();
            std::fs::write(
                repo.join("nouveau.rs"),
                "fn main() {}
",
            )
            .unwrap();
            phone.action(&base, "actualiser", None, None).unwrap();
            let out = Path::new(&out);
            for (name, path, wait) in [
                ("depot", base.clone(), "fichier(s)"),
                ("modifs", format!("{base}/modifs"), "Non indexé"),
                ("diff", format!("{base}/fichier/{f}"), "+trois"),
                ("historique", format!("{base}/historique"), "Ajoute deux"),
                ("branches", format!("{base}/branches"), "feature/essai"),
            ] {
                let s = wait_screen(&mut phone, &path, wait);
                std::fs::write(out.join(format!("{name}.json")), serde_json::to_vec(&s).unwrap()).unwrap();
            }
        }
        assert!(b.action_ids().contains("basculer:main"));
        assert!(phone.action(&format!("{base}/branches"), "nouvelle-branche", Some("pas valide..nom"), None).is_ok());
        assert_eq!(wait_job(&phone, &key), Tone::Danger, "nom de branche refusé");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
