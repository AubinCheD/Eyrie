//! DragonTree sur le téléphone : tous ses dépôts (`liste.rs` : ses onglets, ses favoris, un bandeau par dossier racine
//! analysé, ses récents), leur branche, leur avance ou retard et leurs modifications ; fetch, pull, push et « afficher
//! sur le PC » depuis la liste ; et, dans un dépôt, un client git presque complet (`depot.rs` : modifications et diff,
//! indexer, valider, historique, branches, fusion, rebase, conflits, remises, étiquettes, worktrees, opération en
//! cours).
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
//! sont pas ouverts dans un onglet (lecture seule, comme le relevé de sa barre latérale ; celui de sa liste en
//! attendant). La liste est celle que DragonTree a en mémoire (`repo_directory`) ; ses réglages d'affichage (ordre de
//! l'historique, portée du graphe), son git et son dossier de clonage sont relus dans son fichier quand il change,
//! jamais écrits. Un dépôt est désigné par une empreinte de son chemin (`depot/<clé>`) : pas de chemin Windows dans un
//! chemin d'écran, et la même clé quand la liste change d'ordre.
//!
//! DragonTree vit même masqué dans Eyrie (`background_tick`, appelé par `lib.rs`) : le dépôt que regarde le téléphone
//! est son « foyer distant » (`set_remote_focus`), relu comme l'onglet affiché quand il change sur le disque. Une
//! lecture ratée de DragonTree (état, branches) se dit sur le téléphone.

use std::collections::{HashMap, HashSet};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime};

use dragontree_app::DragonTreeApp;
use dragontree_app::repo::{Refresh, RemoteOp, RepoTab};
use dragontree_app::repo_list::{QuickStatus, RepoDirectory};
use dragontree_app::settings::{Settings, long_path_key, path_key};
use dragontree_git::discovery::repo_head;
use dragontree_git::{
    CancelToken, Commit, CommitInfo, DiffOptions, FileChange, FileState, Git, GraphLayout, HeadState, LogOptions,
    LogOrder, LogScope, Oid, Repo, StatusEntry, StatusSnapshot,
};
use suite_sdk::mobile::msg::OPEN_PREFIX;
use suite_sdk::mobile::ui::{Act, Block, FOLDER_REPO, FolderInput, GraphCell, PROGRESS_UNKNOWN, Place, Screen, Tone};

use crate::branches;
use crate::depot::{self, RepoView, file_key};
use crate::historique::{self, LogPage, LogView, Marks};
use crate::liste::{self, Entry, JOB_SHOWN, Job, Listed, RepoState, repo_key};

/// L'état des dépôts que DragonTree n'a pas ouverts est relu au plus toutes les 30 s tant que le téléphone regarde la
/// liste ; ceux des bandeaux pliés au départ, toutes les 5 min.
const SWEEP_EVERY: Duration = Duration::from_secs(30);
const SWEEP_FOLDED_EVERY: Duration = Duration::from_secs(300);
/// Les réglages de DragonTree sont relus (s'ils ont changé) au plus toutes les 5 s.
const SETTINGS_EVERY: Duration = Duration::from_secs(5);
/// Le téléphone demande l'écran qu'il montre toutes les 300 ms : un dépôt dont aucun écran n'a été demandé depuis 3 s
/// n'est plus regardé (DragonTree n'a plus de foyer distant).
const FOCUS_FOR: Duration = Duration::from_secs(3);
/// Un dépôt de nouveau regardé après 30 s sans l'être voit ses branches relues, comme DragonTree quand sa fenêtre
/// revient au premier plan.
const LOOK_AGAIN: Duration = Duration::from_secs(30);
/// Le diff d'un fichier du dossier de travail est relu au plus toutes les 15 s tant qu'il est regardé.
const DIFF_EVERY: Duration = Duration::from_secs(15);
/// Une opération attend au plus 30 s que DragonTree ait ouvert son dépôt.
const OPEN_WAIT: Duration = Duration::from_secs(30);
/// Dossiers de dépôts connus proposés en raccourcis du sélecteur, au plus.
const PLACES_OF_REPOS: usize = 6;
/// Au-delà, un dépôt que DragonTree n'a toujours pas lu le dit (lecture longue, ou ratée).
const SLOW_AFTER: Duration = Duration::from_secs(15);
/// Une erreur de git montrée est coupée au-delà (caractères).
const ERROR_MAX: usize = 400;

/// Annuler l'opération en cours, comme le bouton Annuler de sa barre de progression (une fenêtre d'identification
/// restée ouverte sur le PC, un fetch qui ne répond pas…).
fn stop_act() -> Act {
    Act::new("stopper", "Annuler l'opération")
        .glyph("⏹️")
        .danger()
        .confirm("Annuler l'opération en cours dans DragonTree ?")
}

/// Ce que fait DragonTree sur le dépôt, en tête de ses écrans : une opération en cours, avec sa progression et son
/// « Annuler », ou ce qu'a donné la dernière.
fn job_block(j: Job) -> Block {
    if !j.running() {
        return Block::Note { text: j.text, tone: j.tone, progress: None, acts: Vec::new() };
    }
    let acts = j.stoppable.then(stop_act).into_iter().collect();
    Block::Note { text: j.text, tone: Tone::Info, progress: Some(j.percent.unwrap_or(PROGRESS_UNKNOWN)), acts }
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

/// L'état du dépôt tel que DragonTree l'a lu ; `None` : pas encore lu. Une lecture ratée (ouverture, état, branches)
/// l'emporte : ce qui avait été lu avant n'est plus sûr.
fn tab_state(t: &RepoTab) -> Option<RepoState> {
    let failed = |e: String| Some(RepoState { error: Some(e), ..Default::default() });
    if let Some(e) = &t.open_error {
        return failed(e.to_string());
    }
    if let Some(e) = &t.status_error {
        return failed(format!("Lecture de l'état impossible : {}", short_error(&e.to_string())));
    }
    if let Some(e) = &t.refs_error {
        return failed(format!("Lecture des branches impossible : {}", short_error(&e.to_string())));
    }
    t.status.as_deref().map(state_of)
}

/// Un résultat d'opération (pas en cours).
fn done(text: String, tone: Tone, at: Instant) -> Job {
    Job { text, tone, at, percent: None, stoppable: false }
}

/// La progression montrée : arrondie à 5 %, pour qu'un écran ne reparte pas au téléphone à chaque pour cent.
pub fn rounded(percent: Option<u8>) -> Option<u8> {
    percent.map(|p| p.min(100) / 5 * 5)
}

/// L'opération visible en cours dans l'onglet (lancée d'ici ou du PC), sinon ce qu'a donné la dernière dans la minute.
fn tab_job(t: &RepoTab) -> Option<Job> {
    if let Some(op) = t.running.iter().find(|o| !o.silent) {
        let text = if op.waiting {
            format!("{} : attend la fin du fetch automatique…", op.title)
        } else {
            format!("{} en cours…", op.title)
        };
        let percent = rounded(op.progress.as_ref().and_then(|p| p.percent));
        return Some(Job { text, tone: Tone::Info, at: Instant::now(), percent, stoppable: true });
    }
    let o = t.outcomes().last().filter(|o| o.at.elapsed() < JOB_SHOWN)?;
    Some(match &o.result {
        Ok(Some(msg)) => done(msg.clone(), Tone::Ok, o.at),
        Ok(None) => done(format!("{} : terminé", o.title), Tone::Ok, o.at),
        Err(e) => done(format!("{} a échoué : {}", o.title, short_error(e)), Tone::Danger, o.at),
    })
}

/// La portée du graphe de DragonTree, dite au téléphone : ce que parcourt son onglet (réglages `settings`) et ce qu'il
/// replie.
fn scope_text(t: &RepoTab, settings: &Settings) -> String {
    let h = &t.history;
    let mut text = if h.current_branch_only {
        "Branche courante seulement (réglage de DragonTree pour ce dépôt)".to_string()
    } else if !h.filter_refs.is_empty() {
        let names: Vec<&str> = h
            .filter_refs
            .iter()
            .map(|r| r.trim_start_matches("refs/heads/").trim_start_matches("refs/remotes/"))
            .collect();
        format!("Branches choisies dans DragonTree : {}", names.join(", "))
    } else {
        match (settings.show_remote_branches, settings.show_tags) {
            (true, true) => "Toutes les branches, distantes et étiquettes comprises",
            (true, false) => "Toutes les branches, distantes comprises",
            (false, true) => "Toutes les branches locales, étiquettes comprises",
            (false, false) => "Toutes les branches locales",
        }
        .to_string()
    };
    if !h.folded.is_empty() {
        text.push_str(&format!(" · {} fusion(s) repliée(s)", h.folded.len()));
    }
    if h.loading {
        text.push_str(" · lecture en cours…");
    }
    text
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
    /// Historique d'une révision (`HEAD`, `refs/heads/x`) → combien de commits ont été demandés, et ce qui a été lu.
    logs: HashMap<String, (usize, Result<LogPage, String>)>,
    /// Les commits pointés par les branches et les étiquettes (leur date, leur titre).
    tips: Option<Result<Arc<HashMap<Oid, Commit>>, String>>,
    /// Branche locale (`refs/heads/x`) → ses commits à pousser et à tirer.
    pending: HashMap<String, branches::Pending>,
    /// Lecture → quand elle a été faite, et sur quoi ([`Load::stamp`]) ; en cours : présente dans `loading`.
    loaded: HashMap<String, (Instant, u64)>,
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
    /// Les `count` derniers commits d'une révision et leur graphe ; `topo` : dans l'ordre topologique (réglage de
    /// DragonTree) ; `tip` : le commit où elle est, d'après DragonTree.
    Log {
        rev: String,
        count: usize,
        topo: bool,
        tip: Option<Oid>,
    },
    /// Les commits pointés par les branches et les étiquettes (triés, sans doublon).
    Tips(Vec<Oid>),
    /// Ce qu'une branche locale a à pousser vers sa branche suivie, et à en tirer ; `tips` : où elles sont.
    Pending {
        local: String,
        upstream: String,
        tips: (Oid, Oid),
    },
}

impl Load {
    fn key(&self) -> String {
        match self {
            Load::LastMessage => "message".into(),
            Load::Commit(id) => format!("commit:{id}"),
            Load::WorkDiff(f, _) => format!("w:{f}"),
            Load::CommitDiff(id, f) => format!("c:{id}:{f}"),
            Load::Log { rev, count, .. } => format!("log:{rev}:{count}"),
            Load::Tips(_) => "tips".into(),
            Load::Pending { local, .. } => format!("pending:{local}"),
        }
    }

    fn every(&self) -> Option<Duration> {
        match self {
            Load::WorkDiff(..) => Some(DIFF_EVERY),
            // Relus quand l'état lu par DragonTree change, ou quand leurs pointes bougent (`stamp`) ; un commit ne
            // change pas.
            Load::LastMessage
            | Load::Commit(_)
            | Load::CommitDiff(..)
            | Load::Log { .. }
            | Load::Tips(_)
            | Load::Pending { .. } => None,
        }
    }

    /// Ce sur quoi la lecture est faite : les pointes qu'elle parcourt (et l'ordre d'un historique). Une lecture faite
    /// sur d'autres est périmée ; tant qu'elles ne bougent pas, elle ne l'est pas, même quand DragonTree relit ses
    /// branches (à chaque changement sur le disque d'un dépôt regardé).
    fn stamp(&self) -> u64 {
        let mut h = DefaultHasher::new();
        match self {
            Load::Log { tip, topo, .. } => (tip, topo).hash(&mut h),
            Load::Tips(oids) => oids.hash(&mut h),
            Load::Pending { tips, .. } => tips.hash(&mut h),
            Load::LastMessage | Load::Commit(_) | Load::WorkDiff(..) | Load::CommitDiff(..) => {}
        }
        h.finish()
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
        Load::Log { rev, count, topo, .. } => {
            // `rev` vient des branches lues par DragonTree (`HEAD`, `refs/…`) : jamais une option de git. Un commit de
            // plus que demandé dit s'il y a une suite.
            let opts = LogOptions {
                scope: LogScope::Revs(vec![rev.clone()]),
                order: if *topo { LogOrder::Topo } else { LogOrder::Date },
                max_count: Some(*count + 1),
                ..Default::default()
            };
            let res = repo.log(&opts, &CancelToken::new()).map(LogPage::of).map_err(|e| e.to_string());
            r.logs.insert(rev.clone(), (*count, res));
        }
        Load::Tips(oids) => {
            let res = repo.commits_of(oids, &CancelToken::new()).map_err(|e| e.to_string());
            r.tips = Some(res.map(|list| Arc::new(list.into_iter().map(|c| (c.id, c)).collect())));
        }
        Load::Pending { local, upstream, .. } => {
            // Des noms complets lus par DragonTree (`refs/…`) : jamais une option de git.
            let log = |from: &str, not: &str| {
                let opts = LogOptions {
                    scope: LogScope::Revs(vec![from.to_string(), format!("^{not}")]),
                    max_count: Some(branches::PENDING_SHOWN),
                    ..Default::default()
                };
                repo.log(&opts, &CancelToken::new())
            };
            let res = log(local, upstream).and_then(|push| Ok((push, log(upstream, local)?)));
            r.pending.insert(local.clone(), res.map_err(|e| e.to_string()));
        }
    }
}

// --- Les dépôts que DragonTree n'a pas ouverts ---

/// L'état rapide des dépôts de la liste qui ne sont pas dans un onglet de DragonTree, et quand il a été relevé.
#[derive(Default)]
struct Sweep {
    states: HashMap<String, (RepoState, Instant)>,
    running: bool,
}

/// L'exécutable git, gardé une fois trouvé (celui des réglages de DragonTree d'abord) ; un échec est retenté au relevé
/// suivant.
type GitCell = Arc<Mutex<Option<Git>>>;

fn git(cell: &GitCell, configured: Option<PathBuf>) -> Result<Git, String> {
    if let Some(g) = lock(cell).clone() {
        return Ok(g);
    }
    let found = configured
        .map(Git::with_exe)
        .filter(|g| g.version().is_ok())
        .map(Ok)
        .unwrap_or_else(|| Git::discover().map_err(|e| e.to_string()));
    if let Ok(g) = &found {
        *lock(cell) = Some(g.clone());
    }
    found
}

/// Un état rapide (celui de la barre latérale de DragonTree) et la branche `head`, pour la liste.
fn quick_state(q: QuickStatus, head: Option<HeadState>) -> RepoState {
    let branch = match head {
        Some(HeadState::Branch(b) | HeadState::Unborn(b)) => Some(b),
        _ => None,
    };
    let error = q.error.or_else(|| q.untrusted.then(|| "git refuse ce dépôt (safe.directory) : voir sur le PC".into()));
    RepoState { branch, upstream: None, ahead: q.ahead, behind: q.behind, changes: q.dirty as usize, error }
}

/// L'état rapide d'un dépôt, comme le relevé de la barre latérale de DragonTree (`quick_status` : rien de ce que
/// désigne la configuration du dépôt n'est lancé), et sa branche lue sur le disque.
fn read_state(git: &Result<Git, String>, path: &Path) -> RepoState {
    if !path.is_dir() {
        return RepoState { error: Some("dossier introuvable".into()), ..Default::default() };
    }
    let git = match git {
        Ok(g) => g,
        Err(e) => return RepoState { error: Some(e.clone()), ..Default::default() },
    };
    quick_state(dragontree_app::repo_list::quick_status(git, path), repo_head(path))
}

/// Des noms de dossiers où le téléphone ne fait jamais ouvrir de dépôt, où qu'ils soient (en minuscules).
const FORBIDDEN_NAMES: [&str; 3] = [".ssh", "$recycle.bin", "system volume information"];

/// Les dossiers où le téléphone ne fait pas ouvrir de dépôt (`docs/mobile.md` §5) : Windows, les programmes, les
/// données des applications (celles d'Eyrie et de la suite comprises), et `profile`, celui de DragonTree.
fn forbidden_zones(profile: Option<&Path>) -> Vec<PathBuf> {
    let vars = ["SystemRoot", "windir", "ProgramFiles", "ProgramFiles(x86)", "ProgramW6432", "ProgramData"];
    let vars = vars.into_iter().chain(["APPDATA", "LOCALAPPDATA"]);
    let mut zones: Vec<PathBuf> =
        vars.filter_map(|v| std::env::var_os(v).filter(|d| !d.is_empty())).map(PathBuf::from).collect();
    if let Some(home) = std::env::var_os("USERPROFILE").filter(|h| !h.is_empty()) {
        zones.push(Path::new(&home).join("AppData"));
    }
    zones.push(suite_accounts::suite_dir());
    zones.push(suite_accounts::local_suite_dir());
    zones.extend(profile.map(Path::to_path_buf));
    zones
}

/// Un chemin `X:\…` d'un disque : ni réseau, ni `\\?\` ou `\\.\`, ni flux (`:` après le lecteur), ni `..`, ni nom
/// réservé de Windows (`CON`, `NUL`, `COM1`…, extension comprise).
fn drive_path(text: &str) -> bool {
    let reserved = |name: &str| {
        let stem = name.split('.').next().unwrap_or_default().trim_end().to_ascii_uppercase();
        let numbered = |p: &str| stem.strip_prefix(p).is_some_and(|n| matches!(n.as_bytes(), [b'1'..=b'9']));
        matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL") || numbered("COM") || numbered("LPT")
    };
    let b = text.as_bytes();
    let drive = b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && matches!(b[2], b'\\' | b'/');
    drive
        && !text[2..].contains(':')
        && Path::new(text).components().all(|c| match c {
            Component::ParentDir => false,
            Component::Normal(n) => !reserved(&n.to_string_lossy()),
            _ => true,
        })
}

/// `dir` résolu (noms courts, jonctions, liens), sans le préfixe `\\?\` ; `None` : il n'existe pas.
fn resolved(dir: &Path) -> Option<PathBuf> {
    let full = std::fs::canonicalize(dir).ok()?;
    let text = full.to_string_lossy();
    Some(PathBuf::from(text.strip_prefix(r"\\?\").unwrap_or(&text)))
}

/// `p` est `zone` ou en dessous (sans casse, séparateurs confondus).
fn inside(p: &Path, zone: &Path) -> bool {
    let (p, z) = (path_key(p), path_key(zone));
    !z.is_empty() && (p == z || p.starts_with(&format!("{z}/")))
}

/// Un dossier de dépôt désigné depuis le téléphone, contrôlé ici même si le pont l'a fait avant (`docs/mobile.md` §5,
/// à venir) : un chemin `X:\…` d'un disque du PC ([`drive_path`]) ; résolu (noms courts, jonctions, liens), un dossier
/// local, hors des dossiers interdits `forbidden` et des [`FORBIDDEN_NAMES`] ; un dépôt git (un `.git`, dossier ou
/// fichier d'un worktree). Rend le chemin résolu : c'est lui que DragonTree ouvre, pas l'endroit où une jonction
/// changée entre-temps mènerait. (Le pont vérifie de plus que le lecteur est un disque fixe.)
fn check_repo_dir(dir: &Path, forbidden: &[PathBuf]) -> Result<PathBuf, String> {
    let refused = || "Chemin refusé : un dossier d'un disque du PC est attendu (C:\\…).".to_string();
    if !drive_path(&dir.to_string_lossy()) {
        return Err(refused());
    }
    let full = resolved(dir).ok_or_else(|| format!("{} n'est pas un dossier du PC.", dir.display()))?;
    // Un lecteur réseau se résout en `\\?\UNC\…`.
    if !drive_path(&full.to_string_lossy()) {
        return Err(refused());
    }
    for p in [dir, full.as_path()] {
        let named = p.components().any(|c| {
            matches!(c, Component::Normal(n) if FORBIDDEN_NAMES.contains(&n.to_string_lossy().to_lowercase().as_str()))
        });
        let zoned = forbidden.iter().any(|z| inside(p, z) || resolved(z).is_some_and(|z| inside(p, &z)));
        if named || zoned {
            return Err(format!("{} : dossier protégé, refusé depuis le téléphone.", dir.display()));
        }
    }
    if !full.is_dir() {
        return Err(format!("{} n'est pas un dossier du PC.", dir.display()));
    }
    if !full.join(".git").exists() {
        return Err(format!("{} n'est pas un dépôt git.", dir.display()));
    }
    Ok(full)
}

/// Ce qu'était le fichier des réglages de DragonTree quand il a été lu.
fn stamp_of(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// Combien de commits d'un historique lu à part montrer, et s'il y en a d'autres : `read` commits lus pour `asked`
/// demandés (un de plus est lu, qui dit s'il y a une suite), `count` demandés maintenant (plus que `asked` : une
/// lecture plus longue est en cours, la suite arrive). À 2 000, la suite est dite sur le PC.
fn page_of(read: usize, asked: usize, count: usize) -> (usize, bool) {
    let shown = read.min(asked).min(count);
    (shown, read > shown)
}

/// Un graphe découpé en lignes : le graphe, combien de lignes, et leurs parts.
type CutGraph = (Arc<GraphLayout>, usize, Arc<Vec<GraphCell>>);

/// Une opération du téléphone qui attend que DragonTree ait ouvert son dépôt.
struct Waiting {
    path: PathBuf,
    op: RemoteOp,
    label: String,
    at: Instant,
}

/// Ce que le téléphone garde entre deux appels.
pub struct Phone {
    /// Le fichier des réglages de DragonTree : ses réglages d'affichage, son git et son dossier de clonage (la liste
    /// des dépôts, elle, est lue en mémoire).
    settings_path: PathBuf,
    checked: Option<Instant>,
    /// Ce qu'était le fichier à sa dernière lecture réussie : un fichier illisible (en cours de remplacement par
    /// DragonTree) est relu la fois suivante, les réglages gardant ce qu'ils avaient.
    settings_stamp: Option<SystemTime>,
    settings: Settings,
    /// Dépôts que le téléphone a ouverts lui-même dans un onglet de DragonTree, pour les lire : ils restent rangés où
    /// ils étaient dans la liste.
    opened: HashSet<String>,
    /// Les dossiers où le téléphone ne fait pas ouvrir de dépôt ([`forbidden_zones`]).
    forbidden: Vec<PathBuf>,
    sweep: Arc<Mutex<Sweep>>,
    git: GitCell,
    reads: SharedReads,
    /// Par dépôt : quand le téléphone l'a regardé pour la dernière fois.
    watched: HashMap<String, Instant>,
    /// Quand DragonTree a reçu son foyer distant (le dépôt regardé) pour la dernière fois ; `None` : il n'en a pas.
    focus: Option<Instant>,
    /// Par dépôt : depuis quand le téléphone attend que DragonTree lise son état et ses branches ; oublié dès qu'il les
    /// a lus, ou qu'une lecture a échoué (elle se dit alors elle-même).
    since: HashMap<String, Instant>,
    /// Par chemin d'écran : les lignes demandées de sa liste par pages (« plus »).
    pages: HashMap<String, usize>,
    /// Par chemin d'écran : le graphe découpé en lignes, et de quoi il l'a été.
    cells: HashMap<String, CutGraph>,
    waiting: HashMap<String, Waiting>,
    /// Par dépôt : une opération qui n'a pas pu partir (dépôt impossible à ouvrir).
    failed: HashMap<String, Job>,
}

impl Phone {
    /// `settings_path` : les réglages de DragonTree.
    pub fn new(settings_path: PathBuf) -> Phone {
        Phone {
            forbidden: forbidden_zones(settings_path.parent()),
            settings_path,
            checked: None,
            settings_stamp: None,
            settings: Settings::default(),
            opened: HashSet::new(),
            sweep: Arc::default(),
            git: GitCell::default(),
            reads: SharedReads::default(),
            watched: HashMap::new(),
            focus: None,
            since: HashMap::new(),
            pages: HashMap::new(),
            cells: HashMap::new(),
            waiting: HashMap::new(),
            failed: HashMap::new(),
        }
    }

    /// Relit les réglages de DragonTree s'ils ont changé (au plus toutes les 5 s), sans jamais les réécrire : il les
    /// enregistre dès qu'ils changent, affiché ou masqué (`background_tick`). Une lecture ratée est refaite la fois
    /// suivante.
    fn update(&mut self) {
        if self.checked.is_some_and(|t| t.elapsed() < SETTINGS_EVERY) {
            return;
        }
        self.checked = Some(Instant::now());
        let stamp = stamp_of(&self.settings_path);
        if stamp != self.settings_stamp {
            let read: Option<Settings> =
                std::fs::read(&self.settings_path).ok().and_then(|b| serde_json::from_slice(&b).ok());
            if let Some(s) = read {
                self.settings = s;
                self.settings_stamp = stamp;
            }
        }
    }

    /// Tous les dépôts de la liste, rangés par bandeau.
    fn repos(&mut self, app: &DragonTreeApp) -> Vec<Listed> {
        self.repos_in(app, &app.repo_directory())
    }

    /// Tous les dépôts de `dir` (ce que DragonTree sait de ses dépôts), rangés par bandeau.
    fn repos_in(&mut self, app: &DragonTreeApp, dir: &RepoDirectory) -> Vec<Listed> {
        self.update();
        let opened = &self.opened;
        let open = |p: &Path| app.repo_tab(p).is_some() && !opened.contains(&repo_key(p));
        let list = liste::list(dir, open);
        // Un onglet que le téléphone avait ouvert, refermé depuis sur le PC : rouvert, il sera à l'utilisateur.
        for l in list.iter().filter(|l| app.repo_tab(&l.path).is_none()) {
            self.opened.remove(&repo_key(&l.path));
        }
        list
    }

    /// Le dépôt de clé `key` dans la liste.
    fn find(&mut self, app: &DragonTreeApp, key: &str) -> Option<Listed> {
        self.repos(app).into_iter().find(|r| repo_key(&r.path) == key)
    }

    /// Relève dans un fil l'état des dépôts `shown` (bandeaux ouverts au départ) qui ne l'ont pas été depuis 30 s, puis
    /// celui des dépôts `folded` (bandeaux pliés) qui ne l'ont pas été depuis 5 min ; `force` : tous, tout de suite.
    fn sweep(&self, shown: Vec<PathBuf>, folded: Vec<PathBuf>, force: bool) {
        let mut s = lock(&self.sweep);
        if s.running {
            return;
        }
        let due = |p: &PathBuf, every: Duration| {
            force || s.states.get(&repo_key(p)).is_none_or(|(_, at)| at.elapsed() >= every)
        };
        let paths: Vec<PathBuf> = shown
            .iter()
            .filter(|p| due(p, SWEEP_EVERY))
            .chain(folded.iter().filter(|p| due(p, SWEEP_FOLDED_EVERY)))
            .cloned()
            .collect();
        if paths.is_empty() {
            return;
        }
        s.running = true;
        drop(s);
        let (sweep, cell, configured) = (self.sweep.clone(), self.git.clone(), self.settings.git_path.clone());
        std::thread::spawn(move || {
            let git = git(&cell, configured);
            for p in paths {
                let state = read_state(&git, &p);
                lock(&sweep).states.insert(repo_key(&p), (state, Instant::now()));
            }
            lock(&sweep).running = false;
        });
    }

    /// Ce qui se passe sur le dépôt `key` : une opération d'ici qui attend son ouverture, ou qui n'a pas pu partir ;
    /// sinon ce que fait DragonTree, ou ce qu'il a donné.
    fn job(&self, key: &str, tab: Option<&RepoTab>) -> Option<Job> {
        if let Some(w) = self.waiting.get(key) {
            let text = format!("{} : ouverture du dépôt dans DragonTree…", w.label);
            return Some(Job { text, tone: Tone::Info, at: w.at, percent: None, stoppable: false });
        }
        let failed = self.failed.get(key).filter(|j| j.at.elapsed() < JOB_SHOWN).cloned();
        let running = tab.and_then(tab_job);
        match (failed, running) {
            (Some(f), Some(r)) if r.running() || r.at > f.at => Some(r),
            (Some(f), _) => Some(f),
            (None, r) => r,
        }
    }

    /// Le dépôt `repo` de la liste et ce qu'on en sait : l'état lu par son onglet, sinon le relevé fait ici, sinon
    /// celui de la liste de DragonTree en attendant ; la couleur de son onglet (`dir` : ce que DragonTree sait de ses
    /// dépôts).
    fn entry(&mut self, app: &DragonTreeApp, dir: &RepoDirectory, repo: Listed) -> Entry {
        let key = repo_key(&repo.path);
        let tab = app.repo_tab(&repo.path);
        let state = match tab {
            Some(t) => tab_state(t),
            None => lock(&self.sweep).states.get(&key).map(|(s, _)| s.clone()).or_else(|| {
                let (q, head) = app.repo_quick_status(&repo.path)?;
                Some(quick_state(q, head))
            }),
        };
        let color = dir.tab_colors.get(&path_key(&repo.path)).map(|c| {
            let c = c.color(&dragontree_app::theme::Palette::light());
            u32::from_be_bytes([0, c.r(), c.g(), c.b()])
        });
        let slow = tab.is_some_and(|t| self.slow(&key, t));
        Entry { state, job: self.job(&key, tab), color, slow, repo }
    }

    /// L'accueil : tous les dépôts ; l'état de ceux que DragonTree n'a pas ouverts est relevé à part, d'abord ceux des
    /// bandeaux ouverts au départ.
    fn home(&mut self, app: &DragonTreeApp) -> Screen {
        let dir = app.repo_directory();
        let repos = self.repos_in(app, &dir);
        let (mut shown, mut folded) = (Vec::new(), Vec::new());
        for r in repos.iter().filter(|r| app.repo_tab(&r.path).is_none()) {
            let in_band = repos.iter().filter(|o| o.group == r.group).count();
            if matches!(r.group, liste::Group::Root(_)) && in_band > liste::FOLD_ABOVE {
                folded.push(r.path.clone());
            } else {
                shown.push(r.path.clone());
            }
        }
        self.sweep(shown, folded, false);
        let busy = if dir.scanning {
            Some("Analyse des dossiers de DragonTree…")
        } else if dir.sweeping || lock(&self.sweep).running {
            Some("Relevé de l'état des dépôts…")
        } else {
            None
        };
        let entries: Vec<Entry> = repos.into_iter().map(|r| self.entry(app, &dir, r)).collect();
        let add = self.add_act(&entries, &dir);
        liste::home(&entries, busy, Some(add))
    }

    /// « Ajouter un dépôt existant… » : un dossier du PC, choisi dans le sélecteur du téléphone, qui doit être un dépôt
    /// git. Raccourcis : le dossier de clonage et les dossiers analysés de DragonTree, puis les dossiers où sont déjà
    /// ses dépôts ; ceux-ci sont marqués « déjà là ».
    fn add_act(&self, entries: &[Entry], dir: &RepoDirectory) -> Act {
        let mut places = Vec::new();
        let mut seen = HashSet::new();
        let mut place = |label: String, path: &Path, glyph: &str| {
            let fresh = seen.insert(path_key(path));
            if fresh {
                places.push(Place { label, path: path.display().to_string(), glyph: glyph.into() });
            }
            fresh
        };
        if let Some(clone_dir) = &self.settings.default_clone_dir {
            place("Dossier de clonage".into(), clone_dir, "📥");
        }
        for root in &dir.roots {
            place(format!("Dossier analysé : {}", liste::repo_name(root)), root, "📁");
        }
        let mut parents = 0;
        for p in entries.iter().filter_map(|e| e.repo.path.parent()) {
            if parents < PLACES_OF_REPOS && place(format!("Dépôts de {}", liste::repo_name(p)), p, "🗂️") {
                parents += 1;
            }
        }
        let known = entries.iter().map(|e| e.repo.path.display().to_string()).collect();
        let spec = FolderInput { want: FOLDER_REPO, places, known, ..Default::default() };
        Act::new("ajouter", "Ajouter un dépôt existant…")
            .glyph("➕")
            .input("Dépôt à ajouter", r"C:\…\mon-depot")
            .folder("Dépôt à ajouter", spec)
    }

    /// Ajoute à DragonTree le dépôt du dossier `input` (choisi sur le PC dans le sélecteur du téléphone, contrôlé par
    /// [`check_repo_dir`]) et l'ouvre sur le téléphone. DragonTree vérifie que git l'ouvre, l'ouvre dans un onglet de
    /// fond (l'onglet affiché sur le PC ne change pas) et le retient parmi ses récents (`add_repo` : dans la liste
    /// aussitôt, en mémoire) ; cet onglet est celui du téléphone. Déjà dans la liste (sous une autre forme du même
    /// chemin, peut-être) : le téléphone l'ouvre.
    fn add(&mut self, app: &mut DragonTreeApp, input: Option<&str>) -> Result<(String, bool), String> {
        let raw = input.map(str::trim).filter(|t| !t.is_empty()).ok_or("Aucun dossier choisi.")?;
        let dir = check_repo_dir(Path::new(raw), &self.forbidden)?;
        let long = long_path_key(&dir);
        if let Some(known) = self.repos(app).into_iter().find(|r| long_path_key(&r.path) == long) {
            return Ok((format!("{OPEN_PREFIX}depot/{}", repo_key(&known.path)), false));
        }
        let path = app
            .add_repo(&dir)
            .map_err(|e| format!("DragonTree n'ouvre pas {} : {}", dir.display(), short_error(&e)))?;
        let key = repo_key(&path);
        self.opened.insert(key.clone());
        Ok((format!("{OPEN_PREFIX}depot/{key}"), false))
    }

    /// Le dépôt regardé est ouvert dans un onglet de DragonTree (en fond) et devient son foyer distant : relu comme
    /// l'onglet affiché quand il change sur le disque. Regardé de nouveau après un moment, ses branches sont relues.
    fn watch(&mut self, app: &mut DragonTreeApp, repo: &Path) {
        let key = repo_key(repo);
        app.set_remote_focus(Some(repo));
        self.focus = Some(Instant::now());
        let again = self.watched.insert(key.clone(), Instant::now()).is_none_or(|t| t.elapsed() >= LOOK_AGAIN);
        if app.repo_tab(repo).is_none() {
            // L'ouverture lit tout.
            app.open_repo_tab(repo);
            self.opened.insert(key.clone());
            self.since.insert(key, Instant::now());
            return;
        }
        if again {
            app.refresh_repo(repo, Refresh::REFS);
        }
    }

    /// DragonTree n'a toujours pas lu l'état ou les branches de l'onglet `t` (dépôt `key`), sans erreur, 15 s après que
    /// le téléphone les attend : sa lecture est longue. Une lecture ratée se dit elle-même ; ce que le téléphone lit
    /// lui-même (un historique) n'y entre pas.
    fn slow(&mut self, key: &str, t: &RepoTab) -> bool {
        let failed = t.open_error.is_some() || t.status_error.is_some() || t.refs_error.is_some();
        if failed || (t.status.is_some() && t.refs.is_some()) {
            self.since.remove(key);
            return false;
        }
        self.since.entry(key.to_string()).or_insert_with(Instant::now).elapsed() >= SLOW_AFTER
    }

    /// Lit `what` pour le dépôt `key` dans un fil, s'il est dû.
    fn want(&self, key: &str, repo: &Arc<Repo>, what: Load) {
        let (k, stamp) = (what.key(), what.stamp());
        let mut all = lock(&self.reads);
        let r = all.entry(key.to_string()).or_default();
        let fresh = r
            .loaded
            .get(&k)
            .is_some_and(|(t, on)| *on == stamp && what.every().is_none_or(|every| t.elapsed() < every));
        if fresh || r.loading.contains(&k) {
            return;
        }
        // Le diff d'un fichier d'un commit attend le commit (ses fichiers, son parent) : demandé à un appel suivant.
        let known = match &what {
            Load::CommitDiff(id, _) => match r.commits.get(id) {
                Some(Ok(c)) => Some(c.clone()),
                _ => return,
            },
            _ => None,
        };
        r.loading.insert(k.clone());
        drop(all);
        let (reads, repo, key) = (self.reads.clone(), repo.clone(), key.to_string());
        std::thread::spawn(move || {
            let mut fresh = Reads::default();
            read(&repo, &what, known.as_ref(), &mut fresh);
            let mut all = lock(&reads);
            let r = all.entry(key).or_default();
            r.loading.remove(&k);
            r.loaded.insert(k, (Instant::now(), stamp));
            if fresh.last_message.is_some() {
                r.last_message = fresh.last_message;
            }
            r.commits.extend(fresh.commits);
            r.diffs.extend(fresh.diffs);
            // Deux lectures du même historique (« plus » touché pendant une lecture) : la plus longue gagne.
            for (rev, (count, page)) in fresh.logs {
                if r.logs.get(&rev).is_none_or(|(had, _)| *had <= count) {
                    r.logs.insert(rev, (count, page));
                }
            }
            r.pending.extend(fresh.pending);
            if fresh.tips.is_some() {
                r.tips = fresh.tips;
            }
        });
    }

    /// L'état que DragonTree a relu rend périmés le dernier message et les diffs du dossier de travail : ils seront
    /// relus, les anciens restant montrés d'ici là. (Les historiques et les pointes le sont quand leurs pointes
    /// bougent : [`Load::stamp`].)
    fn follow(&self, key: &str, t: &RepoTab) {
        let mut all = lock(&self.reads);
        let r = all.entry(key.to_string()).or_default();
        let status = t.status.as_ref().map_or(0, |s| Arc::as_ptr(s) as usize);
        if r.status_seen != status {
            r.status_seen = status;
            r.loaded.retain(|k, _| k != "message" && !k.starts_with("w:"));
        }
    }

    /// Le graphe des `rows` premières lignes de `graph`, gardé tant que DragonTree ne l'a pas refait (`path` : l'écran).
    fn cells(&mut self, path: &str, graph: &Arc<GraphLayout>, rows: usize) -> Arc<Vec<GraphCell>> {
        if let Some((g, n, c)) = self.cells.get(path)
            && Arc::ptr_eq(g, graph)
            && *n == rows
        {
            return c.clone();
        }
        let c = Arc::new(historique::cells(graph, rows));
        self.cells.insert(path.to_string(), (graph.clone(), rows, c.clone()));
        c
    }

    /// L'historique de `rev` lu à part, avec son graphe (`None` : pas encore lu) ; `count` : les commits demandés.
    fn log_view(
        &mut self,
        key: &str,
        path: &str,
        rev: &str,
        count: usize,
        t: &RepoTab,
    ) -> Option<Result<LogView, String>> {
        let read = lock(&self.reads).get(key).and_then(|r| r.logs.get(rev).cloned());
        let (asked, page) = match read? {
            (asked, Ok(page)) => (asked, page),
            (_, Err(e)) => return Some(Err(e)),
        };
        let (shown, more) = page_of(page.commits.len(), asked, count);
        let cells = self.cells(path, &page.graph, shown);
        // Le premier commit de l'historique de HEAD est HEAD, même avant que DragonTree ait lu les branches.
        let head = if rev == "HEAD" { page.commits.first().map(|c| c.id) } else { None };
        let rows = historique::rows(&page, &cells, t.refs.as_deref(), &Marks { head, ..Default::default() });
        Some(Ok(LogView { rows, more, scope: String::new() }))
    }

    /// Le graphe de DragonTree, tel qu'il l'affiche (sa portée, ses fusions repliées) ; `count` : les lignes demandées.
    fn graph_view(&mut self, path: &str, count: usize, t: &RepoTab) -> Option<Result<LogView, String>> {
        let h = &t.history;
        if let Some(e) = &h.error {
            return Some(Err(e.to_string()));
        }
        if h.commits.is_empty() && (h.loading || t.refs.is_none()) {
            return None;
        }
        let shown = count.min(h.commits.len()).min(h.graph.rows.len());
        let cells = self.cells(path, &h.graph, shown);
        let page = LogPage { commits: h.commits.clone(), graph: h.graph.clone() };
        let marks = Marks {
            head: None,
            lost: Some(&h.lost),
            stashes: t.stashes.iter().map(|s| s.oid).collect(),
            folded: Some(&h.folded),
        };
        let rows = historique::rows(&page, &cells, t.refs.as_deref(), &marks);
        let more = shown < h.commits.len() || !h.complete;
        Some(Ok(LogView { rows, more, scope: scope_text(t, &self.settings) }))
    }

    /// Ce que DragonTree sait du dépôt (partagé, sans copie), et ce qui en a été lu à part pour l'écran `parts`
    /// (chemin complet : `path`).
    fn view(&self, key: &str, t: &RepoTab, parts: &[&str], path: &str) -> RepoView {
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
            refs: t.refs.clone(),
            stashes: t.stashes.clone(),
            worktrees: t.worktrees.clone(),
            remotes: t.remotes.clone(),
            op: t.status.is_some().then_some(t.op_state),
            auth_error: t.fetch_auth_error().map(short_error),
            status_error: t.status_error.as_ref().map(|e| short_error(&e.to_string())),
            refs_error: t.refs_error.as_ref().map(|e| short_error(&e.to_string())),
            count: self.pages.get(path).copied().unwrap_or(depot::PAGE),
            ..Default::default()
        };
        let all = lock(&self.reads);
        let Some(r) = all.get(key) else { return v };
        v.last_message = r.last_message.clone().unwrap_or_default();
        if let Some(Ok(tips)) = &r.tips {
            v.tips = tips.clone();
        }
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
        if path.is_empty() {
            return Some(self.home(app));
        }
        let rest = path.strip_prefix("depot/")?;
        let (key, sub) = rest.split_once('/').unwrap_or((rest, ""));
        let listed = self.find(app, key)?;
        let repo = listed.path;
        self.watch(app, &repo);
        let t = app.repo_tab(&repo)?;
        let parts: Vec<&str> = sub.split('/').collect();
        let count = self.pages.get(path).copied().unwrap_or(depot::PAGE);
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
                ["historique"] => {
                    let topo = self.settings.topological_order;
                    let tip = t.refs.as_ref().and_then(|r| r.head_oid).or(t.status.as_ref().and_then(|s| s.head_oid));
                    self.want(key, handle, Load::Log { rev: "HEAD".into(), count, topo, tip });
                }
                _ => {}
            }
            // Les dates des pointes : pour ranger les branches et les étiquettes.
            if let (["branches", ..] | ["branche", _] | ["plus"] | ["etiquettes"], Some(refs)) =
                (parts.as_slice(), &t.refs)
            {
                self.want(key, handle, Load::Tips(branches::tip_oids(refs)));
            }
            if let (["branche", b], Some(refs)) = (parts.as_slice(), &t.refs)
                && let Some(branch) = branches::find(refs, b)
            {
                let topo = self.settings.topological_order;
                self.want(key, handle, Load::Log { rev: branch.full(), count, topo, tip: Some(branch.oid()) });
                if let branches::Branch::Local(l) = branch
                    && (l.ahead > 0 || l.behind > 0)
                    && let Some((upstream, at)) = branches::upstream_ref(refs, l)
                {
                    let tips = (l.oid, at);
                    self.want(key, handle, Load::Pending { local: branch.full(), upstream, tips });
                }
            }
        }
        let slow = self.slow(key, t);
        let mut view = RepoView { slow, ..self.view(key, t, &parts, path) };
        match parts.as_slice() {
            ["historique"] => view.log = self.log_view(key, path, "HEAD", count, t),
            ["historique", "tout"] => view.log = self.graph_view(path, count, t),
            ["branche", b] => {
                if let Some(full) = t.refs.as_deref().and_then(|r| branches::find(r, b)).map(|b| b.full()) {
                    view.log = self.log_view(key, path, &full, count, t);
                    view.pending = lock(&self.reads).get(key).and_then(|r| r.pending.get(&full).cloned());
                }
            }
            _ => {}
        }
        let job = self.job(key, Some(t));
        let busy = job.as_ref().is_some_and(Job::running);
        let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
        // Les chemins des sous-écrans sont absolus dans le module : le téléphone les demande tels quels.
        let base = format!("depot/{key}");
        let mut screen = match parts.as_slice() {
            [""] => depot::overview(&base, &listed.title, &repo.display().to_string(), &view, busy),
            ["modifs"] => depot::changes(&base, &view, busy),
            ["fichier", f] => depot::file(&view, f, busy)?,
            ["historique"] => historique::current(&base, &view, now),
            ["historique", "tout"] => historique::all(&base, &view, now),
            ["commit", id] => depot::commit(&base, &view, id, now, busy),
            ["commit", id, f] => depot::commit_file(&view, id, f)?,
            ["branches"] => branches::list(&base, &view, now, busy),
            ["branches", "g", g] => branches::group(&base, &view, g, now, busy)?,
            ["branche", b] => branches::detail(&base, &view, b, now, busy),
            ["plus"] => depot::more(&base, &view, now, busy),
            ["etiquettes"] => depot::tags(&base, &view, now, busy),
            _ => return None,
        };
        // Ce que fait DragonTree (sa progression, et son « Annuler »), ou ce qu'a donné sa dernière opération, puis ses
        // lectures ratées dont l'écran montre la lecture d'avant, en tête de chaque écran du dépôt.
        let stale = depot::stale_reads(&view).into_iter();
        let stale = stale.map(|text| Block::Note { text, tone: Tone::Warn, progress: None, acts: Vec::new() });
        let top: Vec<Block> = job.map(job_block).into_iter().chain(stale).collect();
        screen.blocks.splice(0..0, top);
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
        let in_repo = path.strip_prefix("depot/").map(|r| r.split_once('/').map_or(r, |(k, _)| k).to_string());
        if action == "ajouter" && in_repo.is_none() {
            return self.add(app, input);
        }
        if action == "actualiser" && in_repo.is_none() {
            let mut closed = Vec::new();
            for r in self.repos(app) {
                if app.repo_tab(&r.path).is_some() {
                    app.refresh_repo(&r.path, Refresh::REFS.with_extras());
                } else {
                    closed.push(r.path);
                }
            }
            self.sweep(closed, Vec::new(), true);
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
        let listed = self.find(app, &key).ok_or("Ce dépôt n'est plus dans la liste.")?;
        let (repo, name) = (listed.path, listed.title);
        if verb == "pc" {
            app.show_repo(&repo);
            // Affiché sur le PC : il est désormais à l'utilisateur, rangé parmi les dépôts ouverts.
            self.opened.remove(&key);
            return Ok((format!("{name} affiché dans DragonTree sur le PC"), true));
        }
        if verb == "stopper" {
            let running = app.repo_tab(&repo).and_then(|t| t.running.iter().find(|o| !o.silent).map(|o| o.id));
            let id = running.ok_or("Plus rien ne tourne sur ce dépôt.")?;
            app.cancel_operation(&repo, id);
            return Ok(("Annulation demandée à DragonTree".into(), false));
        }
        if verb == "actualiser" {
            if app.repo_tab(&repo).is_none() {
                app.open_repo_tab(&repo);
                self.opened.insert(key.clone());
            } else {
                app.refresh_repo(&repo, Refresh::ALL.with_extras());
            }
            self.since.insert(key.clone(), Instant::now());
            lock(&self.reads).remove(&key);
            return Ok(("DragonTree relit le dépôt…".into(), false));
        }
        if verb == "plus" {
            // La page suivante de la liste de l'écran (étiquettes, commits, branches).
            let n = self.pages.entry(path.to_string()).or_insert(depot::PAGE);
            *n = (*n + depot::PAGE).min(depot::PAGE_MAX);
            return Ok((String::new(), false));
        }
        // Le geste vient du dernier écran du dépôt, fait de ce que DragonTree en savait (`view` : sans ce qui est lu
        // à part, inutile ici).
        let view = app.repo_tab(&repo).map(|t| self.view(&key, t, &[], "")).unwrap_or_default();
        let (op, label) = depot::parse(&view, &verb, input)?;
        self.failed.remove(&key);
        if app.repo_tab(&repo).is_none() {
            app.open_repo_tab(&repo);
            self.opened.insert(key.clone());
        }
        let opening = app.repo_tab(&repo).is_some_and(|t| t.repo.is_none() && t.open_error.is_none());
        if opening {
            if self.waiting.contains_key(&key) {
                return Err(format!("{name} : une opération attend déjà l'ouverture du dépôt."));
            }
            self.waiting.insert(key, Waiting { path: repo, op, label: label.clone(), at: Instant::now() });
            return Ok((format!("{label} : ouverture de {name} dans DragonTree…"), false));
        }
        app.queue_operation(&repo, op)?;
        // Renommée depuis son détail : le téléphone ouvre celui du nouveau nom (il se remplit quand DragonTree a relu
        // les branches).
        if let (Some(("renommer", _)), Some(new)) = (verb.split_once(':'), input.map(str::trim))
            && path.contains("/branche/")
        {
            let renamed = branches::branch_key(&format!("refs/heads/{new}"));
            return Ok((format!("{OPEN_PREFIX}depot/{key}/branche/{renamed}"), false));
        }
        Ok((format!("{label} : lancé dans DragonTree"), false))
    }

    /// À chaque image : le dépôt que le téléphone ne regarde plus (aucun de ses écrans demandé depuis 3 s) cesse d'être
    /// le foyer distant de DragonTree ; les opérations qui attendaient l'ouverture de leur dépôt partent quand
    /// DragonTree l'a ouvert (ou sont abandonnées s'il n'y arrive pas).
    pub fn after_logic(&mut self, app: &mut DragonTreeApp) {
        if self.focus.is_some_and(|t| t.elapsed() >= FOCUS_FOR) {
            app.set_remote_focus(None);
            self.focus = None;
        }
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
                self.failed.insert(key, done(text, Tone::Danger, Instant::now()));
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

    /// « Ajouter un dépôt existant… » ouvre le sélecteur de dossiers du PC sur un dépôt git, avec en raccourcis le
    /// dossier de clonage, les dossiers analysés et six dossiers de dépôts connus au plus, sans doublon ; les dépôts
    /// connus y sont marqués.
    #[test]
    fn adding_a_repo_offers_the_pc_folders_of_dragontree() {
        let mut phone = Phone::new(PathBuf::from("absent.json"));
        phone.settings = Settings { default_clone_dir: Some(r"C:\dev".into()), ..Default::default() };
        let dir = RepoDirectory { roots: vec![r"C:\dev".into(), r"D:\sources".into()], ..Default::default() };
        let entries: Vec<Entry> = (0..9)
            .map(|i| {
                let path = PathBuf::from(format!(r"C:\p{}\depot{i}", i % 8));
                let repo = Listed {
                    title: liste::repo_name(&path),
                    path,
                    group: liste::Group::Recent,
                    kind: dragontree_git::discovery::RepoKind::Normal,
                    active: false,
                };
                Entry { repo, state: None, job: None, color: None, slow: false }
            })
            .collect();
        let add = phone.add_act(&entries, &dir);
        assert_eq!(add.id, "ajouter");
        let spec = add.input.as_ref().and_then(|i| i.folder.clone()).expect("un sélecteur de dossiers");
        assert_eq!(spec.want, FOLDER_REPO);
        let places: Vec<&str> = spec.places.iter().map(|p| p.path.as_str()).collect();
        assert_eq!(places, [r"C:\dev", r"D:\sources", r"C:\p0", r"C:\p1", r"C:\p2", r"C:\p3", r"C:\p4", r"C:\p5"]);
        assert_eq!(spec.known.len(), 9);
        let s = liste::home(&[], None, Some(add));
        assert!(s.action_ids().contains("ajouter"), "aussi en bouton quand la liste est vide : {s:?}");
    }

    /// Un chemin venu du téléphone : un dossier `X:\…` d'un disque, sans `..`, flux, nom réservé ni préfixe ; les
    /// dossiers de Windows et des données des applications sont refusés, même désignés autrement.
    #[test]
    fn a_folder_from_the_phone_is_checked_on_the_pc() {
        for good in [r"C:\dev\depot", "D:/sources/x", r"C:\dev\com10", r"C:\dev\console"] {
            assert!(drive_path(good), "{good}");
        }
        for bad in [
            r"\\serveur\partage",
            r"\\?\C:\dev",
            r"\\.\C:\dev",
            "C:relatif",
            r"C:\dev\..\x",
            r"C:\dev\depot:flux",
            r"C:\dev\nul",
            r"C:\dev\COM1.txt",
            "depot",
            "",
        ] {
            assert!(!drive_path(bad), "{bad}");
        }
        let zones = forbidden_zones(None);
        if let Some(windows) = std::env::var_os("SystemRoot") {
            let e = check_repo_dir(Path::new(&windows), &zones).unwrap_err();
            assert!(e.contains("protégé"), "{e}");
        }
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            let e = check_repo_dir(Path::new(&local), &zones).unwrap_err();
            assert!(e.contains("protégé"), "{e}");
        }
    }

    /// Un dépôt ajouté depuis le téléphone l'est par DragonTree, dans un onglet de fond (l'onglet du PC ne change pas),
    /// retenu parmi ses récents ; il entre tout de suite dans la liste, et le téléphone l'ouvre. Un dossier qui n'est
    /// pas un dépôt, un chemin relatif ou réseau, un dossier protégé (même par une jonction) sont refusés.
    #[test]
    fn an_existing_repo_is_added_by_dragontree_and_opened() {
        let Ok(git) = Git::discover() else { return };
        let dir = std::env::temp_dir().join(format!("suite-dragontree-ajout-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let repo = dir.join("nouveau");
        new_repo(&git, &repo);
        let profile = dir.join("profil");
        // Une racine analysée où n'est pas le dépôt à ajouter.
        let mut phone = phone_scanning(&profile, &dir.join("ailleurs").join("autre"));
        let mut h = dragontree(&profile);
        h.step();
        // Le dépôt est dans le dossier temporaire, sous les données des applications : seul `repo` est protégé ici.
        phone.forbidden = vec![repo.clone()];
        let link = dir.join("lien");
        let linked = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&link)
            .arg(&repo)
            .output()
            .is_ok_and(|o| o.status.success());
        for protected in [Some(&repo), linked.then_some(&link)].into_iter().flatten() {
            let e = phone.action(h.state_mut(), "", "ajouter", Some(&protected.display().to_string())).unwrap_err();
            assert!(e.contains("protégé"), "{}: {e}", protected.display());
        }
        phone.forbidden.clear();
        // DragonTree ouvre le chemin résolu (forme longue d'un dossier temporaire en noms courts).
        let key = repo_key(&resolved(&repo).unwrap());
        assert!(phone.screen(h.state_mut(), &format!("depot/{key}")).is_none(), "pas encore connu");
        let plain = dir.join("pas-un-depot");
        std::fs::create_dir_all(&plain).unwrap();
        for bad in [plain.display().to_string(), "nouveau".into(), r"\\serveur\partage\depot".into(), String::new()] {
            assert!(phone.action(h.state_mut(), "", "ajouter", Some(&bad)).is_err(), "{bad}");
        }
        let (open, show) = phone.action(h.state_mut(), "", "ajouter", Some(&repo.display().to_string())).unwrap();
        assert_eq!((open.as_str(), show), (format!("{OPEN_PREFIX}depot/{key}").as_str(), false));
        assert!(h.state().repo_tab(&resolved(&repo).unwrap()).is_some(), "ouvert par DragonTree");
        assert!(h.state().active_repo_dir().is_none(), "en fond : l'onglet du PC ne change pas");
        let recent = h.state().repo_directory().recent.first().map(|p| long_path_key(p));
        assert_eq!(recent, Some(long_path_key(&repo)), "retenu parmi ses récents");
        assert!(phone.opened.contains(&key), "un onglet du téléphone");
        if linked {
            // Par la jonction : le même dépôt, déjà dans la liste.
            let (again, _) = phone.action(h.state_mut(), "", "ajouter", Some(&link.display().to_string())).unwrap();
            assert_eq!(again, open);
        }
        let home = phone.screen(h.state_mut(), "").unwrap();
        assert!(home.opens().contains(&format!("depot/{key}")), "dans la liste tout de suite : {home:?}");
        assert_eq!(band_of(&home, &key), Some("Récents".into()), "{home:?}");
        let s = wait_screen(&mut h, &mut phone, &format!("depot/{key}"), "lisez-moi.txt");
        assert!(format!("{s:?}").contains("lisez-moi.txt"), "{s:?}");
        drop(h);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Une opération en cours se montre avec sa progression, arrondie à 5 % (un écran ne repart pas au téléphone à
    /// chaque pour cent), et le moyen de l'arrêter ; une opération qui attend l'ouverture du dépôt n'a ni l'un ni
    /// l'autre.
    #[test]
    fn a_running_operation_shows_its_rounded_progress_and_how_to_stop_it() {
        assert_eq!(
            (rounded(Some(47)), rounded(Some(100)), rounded(Some(250)), rounded(None)),
            (Some(45), Some(100), Some(100), None)
        );
        let job = |percent, stoppable| Job {
            text: "Fetch en cours…".into(),
            tone: Tone::Info,
            at: Instant::now(),
            percent,
            stoppable,
        };
        match job_block(job(rounded(Some(47)), true)) {
            Block::Note { progress: Some(45), acts, .. } => assert_eq!(acts[0].id, "stopper"),
            b => panic!("{b:?}"),
        }
        match job_block(job(None, false)) {
            Block::Note { progress: Some(PROGRESS_UNKNOWN), acts, .. } => assert!(acts.is_empty()),
            b => panic!("{b:?}"),
        }
        let failed = Job { tone: Tone::Danger, ..job(None, false) };
        assert!(matches!(job_block(failed), Block::Note { progress: None, tone: Tone::Danger, .. }));
    }

    /// Un historique est lu un commit plus loin que demandé : exactement 100 commits n'offrent pas un « plus » qui
    /// n'ajoute rien ; à 2 000, la suite est dite sur le PC ; pendant une lecture plus longue, la suite reste offerte.
    #[test]
    fn a_history_page_knows_whether_more_commits_follow() {
        assert_eq!(page_of(100, 100, 100), (100, false), "exactement 100 : rien de plus");
        assert_eq!(page_of(101, 100, 100), (100, true));
        assert_eq!(page_of(42, 100, 100), (42, false));
        assert_eq!(page_of(101, 100, 200), (100, true), "la suite arrive");
        assert_eq!(page_of(2001, 2000, 2000), (2000, true), "au-delà de 2 000 : sur le PC");
        assert!(historique::more_row(2000, true).is_some_and(|r| r.tap.is_none() && r.title.contains("sur le PC")));
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

    /// Le profil de DragonTree où `repo` n'est que sous son dossier parent analysé (dans la liste de son dernier scan,
    /// qu'il lit au démarrage avant de scanner de nouveau).
    fn phone_scanning(profile: &Path, repo: &Path) -> Phone {
        std::fs::create_dir_all(profile).unwrap();
        let file = profile.join("settings.json");
        let settings = Settings { scan_roots: vec![repo.parent().unwrap().to_path_buf()], ..Default::default() };
        std::fs::write(&file, serde_json::to_vec(&settings).unwrap()).unwrap();
        let found = dragontree_git::discovery::FoundRepo {
            path: repo.to_path_buf(),
            name: liste::repo_name(repo),
            root: repo.parent().unwrap().to_path_buf(),
            kind: dragontree_git::discovery::RepoKind::Normal,
        };
        std::fs::write(profile.join("repos-cache.json"), serde_json::to_vec(&[found]).unwrap()).unwrap();
        Phone::new(file)
    }

    /// Ses réglages d'affichage sont relus quand ils changent ; un fichier illisible (DragonTree en train de le
    /// remplacer) ne les perd pas et sera relu.
    #[test]
    fn a_bad_read_of_the_settings_keeps_them_and_is_retried() {
        let dir = std::env::temp_dir().join(format!("suite-dragontree-reglages-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("settings.json");
        let write = |topological_order: bool| {
            let s = Settings { topological_order, show_tags: false, ..Default::default() };
            std::fs::write(&file, serde_json::to_vec(&s).unwrap()).unwrap();
        };
        let mut phone = Phone::new(file.clone());
        let update = |phone: &mut Phone| {
            phone.checked = None;
            phone.update();
            std::thread::sleep(Duration::from_millis(20));
        };
        write(true);
        update(&mut phone);
        assert!(phone.settings.topological_order && !phone.settings.show_tags);
        std::fs::write(&file, b"{ en cours d'ecriture").unwrap();
        update(&mut phone);
        assert!(phone.settings.topological_order && !phone.settings.show_tags, "gardés");
        write(false);
        update(&mut phone);
        assert!(!phone.settings.topological_order, "relus");
        let _ = std::fs::remove_dir_all(&dir);
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
        assert!(phone.action(h.state_mut(), &format!("depot/{key}"), "stopper", None).is_err(), "rien ne tourne");
        let s = wait_screen(&mut h, &mut phone, &format!("depot/{key}"), "tone: Danger");
        assert!(
            s.blocks
                .iter()
                .any(|b| matches!(b, Block::Note { tone: Tone::Danger, text, .. } if text.starts_with("Fetch : "))),
            "pas un dépôt git : {s:?}"
        );
        // Chaque onglet dit pourquoi DragonTree ne l'a pas ouvert, au lieu de « Lecture… » sans fin.
        for tab in ["historique", "historique/tout", "branches", "plus"] {
            let s = phone.screen(h.state_mut(), &format!("depot/{key}/{tab}")).unwrap();
            let dangers = s.blocks.iter().filter(|b| matches!(b, Block::Note { tone: Tone::Danger, .. })).count();
            assert!(dangers >= 1 && !format!("{s:?}").contains("Lecture"), "{tab} : {s:?}");
        }
        drop(h);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Lance git dans `repo` (sans la configuration du système).
    fn sh(git: &Git, repo: &Path, args: &[&str]) {
        let ok = std::process::Command::new(git.exe())
            .args(args)
            .current_dir(repo)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .unwrap()
            .status
            .success();
        assert!(ok, "git {args:?}");
    }

    /// Un dépôt d'essai dans `repo` : un premier commit sur `main`, et `lisez-moi.txt` modifié depuis.
    fn new_repo(git: &Git, repo: &Path) {
        std::fs::create_dir_all(repo).unwrap();
        let sh = |args: &[&str]| sh(git, repo, args);
        sh(&["init", "-q", "-b", "main"]);
        sh(&["config", "user.name", "Essai"]);
        sh(&["config", "user.email", "essai@exemple.org"]);
        sh(&["config", "commit.gpgsign", "false"]);
        std::fs::write(repo.join("lisez-moi.txt"), "un\n").unwrap();
        sh(&["add", "."]);
        sh(&["commit", "-q", "-m", "Premier commit"]);
        std::fs::write(repo.join("lisez-moi.txt"), "un\ndeux\n").unwrap();
    }

    /// `n` commits vides de plus sur `main` (« Commit 0 »… « Commit n-1 »), d'un coup (`git fast-import`).
    fn more_commits(git: &Git, repo: &Path, n: usize) {
        use std::io::Write;
        let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_secs();
        let mut stream = String::new();
        for i in 0..n {
            let message = format!("Commit {i}");
            let at = now + i as u64;
            stream.push_str(&format!("commit refs/heads/main\ncommitter Essai <essai@exemple.org> {at} +0000\n"));
            stream.push_str(&format!("data {}\n{message}\n", message.len()));
            if i == 0 {
                stream.push_str("from refs/heads/main^0\n");
            }
            stream.push('\n');
        }
        let mut child = std::process::Command::new(git.exe())
            .args(["fast-import", "--quiet"])
            .current_dir(repo)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .stdin(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(stream.as_bytes()).unwrap();
        assert!(child.wait().unwrap().success(), "git fast-import");
    }

    /// L'historique par pages : 100 commits, puis « plus » en ajoute 100 (lus plus loin, à part), jusqu'au bout,
    /// où il n'y a plus de « plus » ; une branche d'exactement 100 commits n'en offre pas un qui n'ajouterait rien.
    #[test]
    fn the_history_grows_by_pages_to_its_end() {
        let Ok(git) = Git::discover() else { return };
        let dir = std::env::temp_dir().join(format!("suite-dragontree-pages-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let repo = dir.join("depot");
        new_repo(&git, &repo);
        // « Premier commit », puis « Commit 0 » à « Commit 248 » : 250 ; `cent` en a 100.
        more_commits(&git, &repo, 249);
        sh(&git, &repo, &["branch", "cent", "main~150"]);

        let profile = dir.join("profil");
        let mut phone = phone_for(&profile, &repo);
        let mut h = dragontree(&profile);
        let base = format!("depot/{}", repo_key(&repo));
        let path = format!("{base}/historique");
        let commits = |s: &Screen| rows(s).iter().filter(|r| r.graph.is_some()).count();
        let s = wait_screen(&mut h, &mut phone, &path, "\"Commit 248\"");
        assert_eq!(commits(&s), depot::PAGE, "{s:?}");
        assert!(s.action_ids().contains("plus"), "{s:?}");
        for (last, shown, more) in [("\"Commit 49\"", 200, true), ("\"Premier commit\"", 250, false)] {
            assert_eq!(phone.action(h.state_mut(), &path, "plus", None).unwrap(), (String::new(), false));
            let s = wait_screen(&mut h, &mut phone, &path, last);
            assert_eq!((commits(&s), s.action_ids().contains("plus")), (shown, more), "{s:?}");
        }
        let cent = format!("{base}/branche/{}", branches::branch_key("refs/heads/cent"));
        let d = wait_screen(&mut h, &mut phone, &cent, "\"Premier commit\"");
        assert_eq!(commits(&d), 100, "{d:?}");
        assert!(!d.action_ids().contains("plus"), "exactement 100 : pas de « plus » vide : {d:?}");
        drop(h);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Fait tourner DragonTree jusqu'à ce que l'écran `path` ait fini de lire ce qu'il montre (au plus 10 s).
    fn settle(h: &mut H, phone: &mut Phone, path: &str) -> Option<Screen> {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            h.step();
            phone.after_logic(h.state_mut());
            let s = phone.screen(h.state_mut(), path)?;
            if !format!("{s:?}").contains("Lecture") || Instant::now() > deadline {
                return Some(s);
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    /// Parcourt les écrans que le téléphone peut ouvrir depuis `start` (au plus `max`, et 4 de chaque sorte : détails
    /// de branche, commits…) : chaque chemin qu'un écran visité ouvre (ligne, onglet, action d'écran) doit exister sur
    /// le PC, visité ou non. Rend les chemins visités.
    fn walk(h: &mut H, phone: &mut Phone, start: &str, max: usize) -> Vec<String> {
        // `depot/<clé>/branche/<b>` : « branche », 4 parts.
        let kind = |p: &str| format!("{}:{}", p.split('/').nth(2).unwrap_or_default(), p.split('/').count());
        let mut kinds: HashMap<String, usize> = HashMap::new();
        let mut seen = std::collections::BTreeSet::new();
        let mut todo = std::collections::VecDeque::from([start.to_string()]);
        while let Some(path) = todo.pop_front() {
            if seen.len() >= max || seen.contains(&path) {
                continue;
            }
            let visits = kinds.entry(kind(&path)).or_default();
            if *visits >= 4 {
                continue;
            }
            *visits += 1;
            seen.insert(path.clone());
            let s = settle(h, phone, &path).unwrap_or_else(|| panic!("l'écran « {path} » n'existe pas sur le PC"));
            for open in s.opens() {
                assert!(
                    open.is_empty() || open.starts_with("depot/"),
                    "« {path} » ouvre « {open} », un chemin qui n'est pas absolu dans le module"
                );
                assert!(
                    phone.screen(h.state_mut(), &open).is_some(),
                    "« {path} » ouvre « {open} », que le PC ne trouve pas"
                );
                todo.push_back(open);
            }
        }
        seen.into_iter().collect()
    }

    /// Le défaut du 28/10 : les sous-écrans d'un dépôt ouvraient des chemins relatifs, que le PC ne trouvait pas.
    /// Depuis la liste, chaque écran atteignable existe, sur un vrai dépôt avec un commit, une modification, une
    /// seconde branche, une étiquette, une remise, et un remote de 32 branches (un bandeau coupé, son groupe entier,
    /// le détail d'une branche distante).
    #[test]
    fn every_screen_reachable_from_the_list_exists_on_the_pc() {
        let Ok(git) = Git::discover() else { return };
        let dir = std::env::temp_dir().join(format!("suite-dragontree-ecrans-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let repo = dir.join("depot");
        new_repo(&git, &repo);
        let sh = |args: &[&str]| sh(&git, &repo, args);
        // Assez d'étiquettes et de modifications pour que l'onglet Plus et l'onglet État en coupent la liste.
        for i in 0..11 {
            sh(&["tag", &format!("v{i}")]);
        }
        sh(&["branch", "feature/essai"]);
        std::fs::write(repo.join("remise.txt"), "de côté\n").unwrap();
        sh(&["add", "remise.txt"]);
        sh(&["stash", "push", "-q", "-m", "Une remise", "--", "remise.txt"]);
        for i in 0..31 {
            std::fs::write(repo.join(format!("neuf-{i}.txt")), "neuf\n").unwrap();
        }
        // Un remote nu où `main` est suivie, et 31 branches d'un même dossier : au-delà de 30, un bandeau par dossier,
        // coupé à 25 lignes, puis « 6 autres… » vers le groupe entier.
        let origin = dir.join("origin.git").display().to_string();
        sh(&["init", "-q", "--bare", &origin]);
        sh(&["remote", "add", "origin", &origin]);
        sh(&["push", "-q", "-u", "origin", "main"]);
        let many: Vec<String> = (0..31).map(|i| format!("main:refs/heads/equipe/eux/b{i}")).collect();
        let mut push = vec!["push", "-q", "origin"];
        push.extend(many.iter().map(String::as_str));
        sh(&push);
        sh(&["fetch", "-q", "origin"]);

        // Le dépôt n'est que dans la liste du scan de DragonTree (ni onglet, ni récent).
        let profile = dir.join("profil");
        let mut phone = phone_scanning(&profile, &repo);
        let mut h = dragontree(&profile);
        let base = format!("depot/{}", repo_key(&repo));
        let home = wait_screen(&mut h, &mut phone, "", "depot");
        assert!(home.opens().contains(&base), "{home:?}");
        wait_screen(&mut h, &mut phone, &base, "Non indexé");
        let seen = walk(&mut h, &mut phone, "", 80);
        let has = |p: &str| seen.iter().any(|s| s.starts_with(&format!("{base}/{p}")));
        for p in ["modifs", "fichier/", "historique", "commit/", "branches", "branche/", "plus", "etiquettes"] {
            assert!(has(p), "{p} visité : {seen:?}");
        }
        assert!(
            seen.iter().any(|s| s.starts_with(&format!("{base}/commit/")) && s.matches('/').count() == 4),
            "le diff d'un fichier d'un commit : {seen:?}"
        );
        assert!(has("branches/g/"), "le groupe entier d'un gros remote : {seen:?}");
        // Une branche distante : un bandeau plié de l'onglet Branches l'ouvre ; son détail la suit en local, la
        // fusionne ou rebase la courante dessus.
        let branches = settle(&mut h, &mut phone, &format!("{base}/branches")).unwrap();
        let theirs = format!("{base}/branche/{}", branches::branch_key("refs/remotes/origin/equipe/eux/b0"));
        assert!(branches.opens().contains(&theirs), "{branches:?}");
        assert!(format!("{branches:?}").contains("6 autres…"), "{branches:?}");
        let d = settle(&mut h, &mut phone, &theirs).unwrap();
        for id in ["suivre:origin/equipe/eux/b0", "fusionner:origin/equipe/eux/b0", "rebaser:origin/equipe/eux/b0"] {
            assert!(d.action_ids().contains(id), "{id} : {d:?}");
        }
        drop(h);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Le bandeau de la liste où est le dépôt `key`.
    fn band_of(home: &Screen, key: &str) -> Option<String> {
        let mut band = None;
        for b in &home.blocks {
            match b {
                Block::Section { title, .. } => band = Some(title.clone()),
                Block::Row(r) if r.key == key => return band,
                _ => {}
            }
        }
        None
    }

    /// La ligne du dépôt `key` dans la liste.
    fn row_in(home: &Screen, key: &str) -> Option<suite_sdk::mobile::ui::Row> {
        rows(home).into_iter().find(|r| r.key == key).cloned()
    }

    /// Le défaut du 28/10 : une lecture ratée restait « Lecture… » sans dire pourquoi, et un dépôt regardé n'était
    /// relu que toutes les 20 s. L'erreur de DragonTree se dit dans la liste et sur l'écran du dépôt (à la place de
    /// « Lecture… », ou en tête quand l'écran montre sa lecture d'avant) ; le dépôt regardé est relu dès qu'il change
    /// sur le disque, et plus quand le téléphone ne le regarde plus.
    #[test]
    fn a_read_error_of_dragontree_is_said_and_the_watched_repo_follows_the_disk() {
        let Ok(git) = Git::discover() else { return };
        let dir = std::env::temp_dir().join(format!("suite-dragontree-erreurs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let repo = dir.join("depot");
        new_repo(&git, &repo);
        // Un index illisible : DragonTree ouvre le dépôt, mais ne peut pas en lire l'état.
        let index = repo.join(".git").join("index");
        let index_was = std::fs::read(&index).unwrap();
        std::fs::write(&index, "illisible").unwrap();
        let profile = dir.join("profil");
        let mut phone = phone_for(&profile, &repo);
        let mut h = dragontree(&profile);
        let key = repo_key(&repo);
        let base = format!("depot/{key}");
        let s = wait_screen(&mut h, &mut phone, &base, "n'a pas pu lire l'état");
        let text = format!("{s:?}");
        assert!(text.contains("« Actualiser » relance la lecture") && !text.contains("Lecture du dépôt"), "{text}");
        assert!(s.blocks.iter().any(|b| matches!(b, Block::Note { tone: Tone::Danger, .. })), "{s:?}");
        // Même longtemps après : pas de « lecture longue », l'erreur est connue.
        phone.since.insert(key.clone(), Instant::now().checked_sub(SLOW_AFTER * 2).expect("un PC allumé"));
        let s = phone.screen(h.state_mut(), &base).unwrap();
        assert!(!format!("{s:?}").contains("très gros"), "{s:?}");
        let row = row_in(&phone.screen(h.state_mut(), "").unwrap(), &key).expect("dans la liste");
        assert!(row.subtitle.starts_with("Lecture de l'état impossible : "), "{row:?}");
        assert_eq!(row.tone, Some(Tone::Danger));

        // Réparé, puis « Actualiser » : lu.
        std::fs::write(&index, &index_was).unwrap();
        phone.action(h.state_mut(), &base, "actualiser", None).unwrap();
        let s = wait_screen(&mut h, &mut phone, &base, "lisez-moi.txt");
        assert!(!format!("{s:?}").contains("n'a pas pu"), "{s:?}");
        // Regardé depuis le téléphone, l'onglet de fond est relu dès que le dépôt change sur le disque.
        assert!(h.state().active_repo_dir().is_none(), "onglet de fond");
        std::fs::write(repo.join("neuf.txt"), "neuf\n").unwrap();
        let s = wait_screen(&mut h, &mut phone, &base, "neuf.txt");
        assert!(format!("{s:?}").contains("neuf.txt"), "relu sans « Actualiser » : {s:?}");
        // Une lecture ratée après une bonne : en tête, l'écran montre la lecture d'avant.
        let head = repo.join(".git").join("HEAD");
        let head_was = std::fs::read(&head).unwrap();
        std::fs::write(&head, "illisible\n").unwrap();
        let s = wait_screen(&mut h, &mut phone, &base, "n'a pas pu relire les branches");
        assert!(
            matches!(&s.blocks[0], Block::Note { tone: Tone::Warn, text, .. } if text.contains("lecture d'avant")),
            "{s:?}"
        );
        assert!(format!("{s:?}").contains("lisez-moi.txt"), "la lecture d'avant : {s:?}");
        std::fs::write(&head, &head_was).unwrap();
        let s = wait_gone(&mut h, &mut phone, &base, "n'a pas pu");
        assert!(!format!("{s:?}").contains("n'a pas pu"), "relu, réparé : {s:?}");

        // Plus regardé (aucun écran demandé depuis 3 s) : plus relu, il attend de nouveau qu'on le regarde.
        phone.focus = Instant::now().checked_sub(FOCUS_FOR * 2);
        phone.after_logic(h.state_mut());
        assert!(phone.focus.is_none());
        std::fs::write(repo.join("plus-regarde.txt"), "rien\n").unwrap();
        let deadline = Instant::now() + Duration::from_millis(1500);
        while Instant::now() < deadline {
            h.step();
            std::thread::sleep(Duration::from_millis(25));
        }
        let status = h.state().repo_tab(&repo).and_then(|t| t.status.clone()).unwrap();
        assert!(!status.entries.iter().any(|e| e.path == "plus-regarde.txt"), "plus regardé : pas relu");
        drop(h);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Eyrie où DragonTree est masqué : sa logique tourne à chaque image (`visible` faux), jamais son `ui`.
    struct HiddenInEyrie {
        module: Box<dyn suite_sdk::Module>,
        host: suite_sdk::Host,
    }

    impl eframe::App for HiddenInEyrie {
        fn logic(&mut self, ctx: &eframe::egui::Context, frame: &mut eframe::Frame) {
            let mut cx = suite_sdk::Cx { host: &self.host, frame, active: false, visible: false };
            self.module.logic(ctx, &mut cx);
        }

        fn ui(&mut self, _ui: &mut eframe::egui::Ui, _frame: &mut eframe::Frame) {}
    }

    impl HiddenInEyrie {
        fn screen(&mut self, path: &str) -> Option<Screen> {
            let mut frame = eframe::Frame::_new_kittest();
            let mut cx = suite_sdk::Cx { host: &self.host, frame: &mut frame, active: false, visible: false };
            self.module.mobile_screen(path, &mut cx)
        }
    }

    /// Le défaut du 28/10 : masqué dans Eyrie, DragonTree ne relisait rien et n'enregistrait rien (`ui` seul le
    /// faisait). Le module le fait vivre : l'onglet ouvert pour le téléphone est enregistré dans ses réglages, et le
    /// dépôt regardé est relu dès qu'il change sur le disque.
    #[test]
    fn hidden_in_eyrie_dragontree_still_follows_the_watched_repo() {
        let Ok(git) = Git::discover() else { return };
        let dir = std::env::temp_dir().join(format!("suite-dragontree-masque-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let repo = dir.join("depot");
        new_repo(&git, &repo);
        let profile = dir.join("profil");
        let file = profile.join("settings.json");
        drop(phone_for(&profile, &repo));
        dragontree_forges::secrets::use_vaults(
            Arc::new(suite_accounts::MemoryVault::default()),
            Arc::new(suite_accounts::MemoryVault::default()),
        );
        let (suite, trial) = (dir.join("suite"), profile.clone());
        let mut h = Harness::builder().with_size(eframe::egui::vec2(800.0, 600.0)).build_eframe(move |cc| {
            let host = suite_sdk::Host::new(cc.egui_ctx.clone(), suite, suite_sdk::theme::ThemeChoice::Graphite);
            host.set_accounts_dir(trial.join("comptes"));
            let bcx = suite_sdk::BuildCx { cc, host: host.clone(), data: suite_sdk::DataMode::Trial(trial) };
            let module = crate::build(&bcx).unwrap_or_else(|e| panic!("{e}"));
            HiddenInEyrie { module, host }
        });
        let base = format!("depot/{}", repo_key(&repo));
        let saved = || {
            let s: Option<Settings> = std::fs::read(&file).ok().and_then(|b| serde_json::from_slice(&b).ok());
            s.is_some_and(|s| s.open_tabs.iter().any(|p| path_key(p) == path_key(&repo)))
        };
        let wait = |h: &mut Harness<'static, HiddenInEyrie>, what: &str, cond: &dyn Fn(&Screen) -> bool| {
            let deadline = Instant::now() + Duration::from_secs(40);
            loop {
                h.step();
                let s = h.state_mut().screen(&base).expect("écran du dépôt");
                if cond(&s) {
                    return;
                }
                assert!(Instant::now() < deadline, "délai dépassé : {what} : {s:?}");
                std::thread::sleep(Duration::from_millis(25));
            }
        };
        wait(&mut h, "dépôt lu", &|s| format!("{s:?}").contains("lisez-moi.txt"));
        assert!(saved(), "l'onglet ouvert pour le téléphone est enregistré, DragonTree masqué");
        std::fs::write(repo.join("neuf.txt"), "neuf\n").unwrap();
        wait(&mut h, "neuf.txt relu", &|s| format!("{s:?}").contains("neuf.txt"));
        drop(h);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Fait tourner DragonTree jusqu'à ce que l'écran `path` ne contienne plus `text`.
    fn wait_gone(h: &mut H, phone: &mut Phone, path: &str, text: &str) -> Screen {
        let deadline = Instant::now() + Duration::from_secs(40);
        loop {
            h.step();
            phone.after_logic(h.state_mut());
            let s = phone.screen(h.state_mut(), path).expect("écran");
            if !format!("{s:?}").contains(text) || Instant::now() > deadline {
                return s;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    /// Un dépôt que le téléphone a ouvert pour le lire reste dans son bandeau ; son onglet refermé sur le PC, il n'est
    /// plus « au téléphone » : rouvert ensuite par l'utilisateur, il est parmi les dépôts ouverts sur le PC, même en
    /// fond.
    #[test]
    fn a_tab_closed_on_the_pc_is_no_longer_the_phones() {
        use eframe::egui::{Key, Modifiers};

        let Ok(git) = Git::discover() else { return };
        let dir = std::env::temp_dir().join(format!("suite-dragontree-onglet-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (repo, other) = (dir.join("depot"), dir.join("autre"));
        new_repo(&git, &repo);
        new_repo(&git, &other);
        let profile = dir.join("profil");
        std::fs::create_dir_all(&profile).unwrap();
        let settings = Settings { recent_repos: vec![repo.clone(), other.clone()], ..Default::default() };
        std::fs::write(profile.join("settings.json"), serde_json::to_vec(&settings).unwrap()).unwrap();
        let mut phone = Phone::new(profile.join("settings.json"));
        let mut h = dragontree(&profile);
        let key = repo_key(&repo);
        let band = |h: &mut H, phone: &mut Phone| band_of(&phone.screen(h.state_mut(), "").unwrap(), &key);
        wait_screen(&mut h, &mut phone, &format!("depot/{key}"), "lisez-moi.txt");
        assert_eq!(band(&mut h, &mut phone), Some("Récents".into()), "lu depuis le téléphone : il ne bouge pas");

        // L'utilisateur l'affiche sur le PC (son onglet), puis le ferme (Ctrl+W) ; le téléphone regarde sa liste.
        h.state_mut().show_repo(&repo);
        h.step();
        h.key_press_modifiers(Modifiers::COMMAND, Key::W);
        let deadline = Instant::now() + Duration::from_secs(10);
        while h.state().repo_tab(&repo).is_some() && Instant::now() < deadline {
            h.step();
            std::thread::sleep(Duration::from_millis(25));
        }
        assert!(h.state().repo_tab(&repo).is_none(), "onglet fermé");
        assert_eq!(band(&mut h, &mut phone), Some("Récents".into()));
        // Rouvert par l'utilisateur, puis un autre dépôt affiché : le sien reste ouvert, en fond.
        h.state_mut().show_repo(&repo);
        h.state_mut().show_repo(&other);
        h.step();
        assert_eq!(band(&mut h, &mut phone), Some("Ouverts sur le PC".into()));
        drop(h);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn rows(s: &Screen) -> Vec<&suite_sdk::mobile::ui::Row> {
        s.blocks
            .iter()
            .filter_map(|b| match b {
                Block::Row(r) => Some(r),
                _ => None,
            })
            .collect()
    }

    /// Le défaut du 28/10 : « Historique » montrait le haut du graphe de toutes les branches (des commits de
    /// collègues) en se disant celui de la branche courante. C'est celui de HEAD, avec son graphe et ses étiquettes ;
    /// le graphe de DragonTree, toutes branches, est à un toucher.
    #[test]
    fn the_history_is_the_current_branch_with_its_graph() {
        let Ok(git) = Git::discover() else { return };
        let dir = std::env::temp_dir().join(format!("suite-dragontree-historique-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let repo = dir.join("depot");
        new_repo(&git, &repo);
        let sh = |args: &[&str]| sh(&git, &repo, args);
        sh(&["stash", "push", "-q"]);
        sh(&["checkout", "-q", "-b", "autre"]);
        sh(&["commit", "-q", "--allow-empty", "-m", "Sur une autre branche"]);
        sh(&["checkout", "-q", "main"]);
        sh(&["commit", "-q", "--allow-empty", "-m", "Second commit"]);
        sh(&["tag", "v1"]);

        let profile = dir.join("profil");
        let mut phone = phone_for(&profile, &repo);
        let mut h = dragontree(&profile);
        let base = format!("depot/{}", repo_key(&repo));
        // Les étiquettes viennent des branches lues par DragonTree, qui peuvent arriver après l'historique.
        let s = wait_screen(&mut h, &mut phone, &format!("{base}/historique"), "\"v1\"");
        let text = format!("{s:?}");
        assert!(text.contains("Premier commit") && !text.contains("Sur une autre branche"), "HEAD seulement : {text}");
        assert_eq!(s.subtitle, "Branche courante : main");
        assert!(!s.search.is_empty() && s.tabs.iter().any(|t| t.path == format!("{base}/historique")));
        let commits: Vec<_> = rows(&s).into_iter().filter(|r| r.graph.is_some()).collect();
        assert_eq!(commits.len(), 2, "{s:?}");
        let head = commits[0];
        assert_eq!(head.graph.as_ref().map(|g| g.kind), Some(suite_sdk::mobile::ui::NODE_HEAD));
        assert!(head.chips.iter().any(|c| c.text == "main" && c.strong), "{head:?}");
        assert!(head.chips.iter().any(|c| c.text == "v1"), "{head:?}");
        assert!(
            !head.key.is_empty() && head.open.as_deref().is_some_and(|o| o.starts_with(&format!("{base}/commit/")))
        );
        assert!(!s.action_ids().contains("plus"), "deux commits : pas de suite");
        // Le diff d'un fichier d'un commit demandé d'emblée (le téléphone revient sur cet écran), avant le commit
        // lui-même : il arrive (le défaut : « Lecture du diff… » sans fin).
        let diff = format!("{base}/commit/{}/{}", commits[1].key, file_key("lisez-moi.txt"));
        let d = wait_screen(&mut h, &mut phone, &diff, "+un");
        assert!(format!("{d:?}").contains("+un"), "{d:?}");

        let all = wait_screen(&mut h, &mut phone, &format!("{base}/historique/tout"), "Sur une autre branche");
        assert!(format!("{all:?}").contains("Sur une autre branche"), "{all:?}");
        assert!(all.subtitle.starts_with("Toutes les branches"), "la vraie portée : {}", all.subtitle);
        assert!(
            all.tabs.iter().any(|t| t.path == format!("{base}/historique/tout")),
            "l'onglet Historique reste actif"
        );
        let lanes = rows(&all).iter().filter_map(|r| r.graph.as_ref()).map(|g| g.lanes).max();
        assert_eq!(lanes, Some(2), "deux branches, deux voies : {all:?}");

        // Toucher une branche ouvre son détail : son historique à elle, et la bascule en premier bouton.
        let list = wait_screen(&mut h, &mut phone, &format!("{base}/branches"), "autre");
        let other = branches::branch_key("refs/heads/autre");
        let row = rows(&list).into_iter().find(|r| r.title == "autre").expect("autre");
        assert_eq!(row.open, Some(format!("{base}/branche/{other}")));
        assert!(row.tap.is_none(), "pas de bascule d'un toucher");
        // DragonTree a lu le dépôt : l'historique que le téléphone lit encore à part n'est pas une lecture « peut-être
        // ratée », même dans un dépôt regardé depuis longtemps (le défaut : l'alerte venait 15 s après la première
        // visite, sur chaque historique pas encore lu).
        let key = repo_key(&repo);
        let long_ago = Instant::now().checked_sub(SLOW_AFTER * 2).expect("un PC allumé depuis plus de 30 s");
        phone.since.insert(key.clone(), long_ago);
        let unread = format!("log:refs/heads/autre:{}", depot::PAGE);
        lock(&phone.reads).entry(key.clone()).or_default().loading.insert(unread.clone());
        let d = phone.screen(h.state_mut(), &format!("{base}/branche/{other}")).unwrap();
        assert!(format!("{d:?}").contains("Lecture de l'historique"), "{d:?}");
        assert!(!d.blocks.iter().any(|b| matches!(b, Block::Note { tone: Tone::Warn, .. })), "pas d'alerte : {d:?}");
        lock(&phone.reads).get_mut(&key).unwrap().loading.remove(&unread);
        let d = wait_screen(&mut h, &mut phone, &format!("{base}/branche/{other}"), "Premier commit");
        assert!(format!("{d:?}").contains("Premier commit") && !format!("{d:?}").contains("Second commit"), "{d:?}");
        assert!(d.action_ids().contains("basculer:autre"));
        // Renommée depuis son détail : le téléphone ouvre le détail du nouveau nom.
        let (open, _) =
            phone.action(h.state_mut(), &format!("{base}/branche/{other}"), "renommer:autre", Some("autre2")).unwrap();
        let renamed = format!("{base}/branche/{}", branches::branch_key("refs/heads/autre2"));
        assert_eq!(open, format!("{OPEN_PREFIX}{renamed}"));
        let d = wait_screen(&mut h, &mut phone, &renamed, "Premier commit");
        assert_eq!(d.title, "autre2", "{d:?}");

        // DragonTree relit ses branches (à chaque changement sur le disque d'un dépôt regardé) : tant qu'elles n'ont
        // pas bougé, l'historique lu à part ne l'est pas de nouveau ; un commit de plus, il l'est.
        let log = format!("log:HEAD:{}", depot::PAGE);
        let read_at = |phone: &Phone| {
            let all = lock(&phone.reads);
            (all[&key].loaded.get(&log).map(|(at, _)| *at), all[&key].loading.contains(&log))
        };
        let reread = |h: &mut H| {
            let old = h.state().repo_tab(&repo).and_then(|t| t.refs.clone());
            h.state_mut().refresh_repo(&repo, Refresh::REFS.with_extras());
            let same = |h: &H| {
                let now = h.state().repo_tab(&repo).and_then(|t| t.refs.clone());
                now.zip(old.clone()).is_some_and(|(a, b)| Arc::ptr_eq(&a, &b))
            };
            let deadline = Instant::now() + Duration::from_secs(10);
            while same(h) && Instant::now() < deadline {
                h.step();
                std::thread::sleep(Duration::from_millis(25));
            }
            assert!(!same(h), "DragonTree a relu ses branches");
        };
        wait_screen(&mut h, &mut phone, &format!("{base}/historique"), "Second commit");
        let before = read_at(&phone);
        assert!(before.0.is_some(), "lu");
        reread(&mut h);
        phone.screen(h.state_mut(), &format!("{base}/historique")).unwrap();
        assert_eq!(read_at(&phone), before, "pas relu : ses pointes n'ont pas bougé");
        sh(&["commit", "-q", "--allow-empty", "-m", "Troisième commit"]);
        reread(&mut h);
        let s = wait_screen(&mut h, &mut phone, &format!("{base}/historique"), "Troisième commit");
        assert!(format!("{s:?}").contains("Troisième commit"), "relu : HEAD a bougé");
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
        new_repo(&git, &repo);

        let profile = dir.join("profil");
        let mut phone = phone_for(&profile, &repo);
        let mut h = dragontree(&profile);
        let key = repo_key(&repo);
        let base = format!("depot/{key}");
        let s = wait_screen(&mut h, &mut phone, &base, "lisez-moi.txt");
        assert!(format!("{s:?}").contains("Non indexé") && s.tabs.len() == 4, "{s:?}");
        assert!(h.state().repo_tab(&repo).is_some() && h.state().active_repo_dir().is_none(), "onglet de fond");
        // Ouvert par le téléphone pour le lire : la liste ne bouge pas, il reste dans les récents.
        assert_eq!(band_of(&phone.screen(h.state_mut(), "").unwrap(), &key), Some("Récents".into()));
        let f = file_key("lisez-moi.txt");
        let diff = wait_screen(&mut h, &mut phone, &format!("{base}/fichier/{f}"), "+deux");
        assert!(format!("{diff:?}").contains("+deux"), "{diff:?}");

        phone.action(h.state_mut(), &format!("{base}/modifs"), &format!("indexer:{f}"), None).unwrap();
        let s = wait_screen(&mut h, &mut phone, &format!("{base}/modifs"), "Valider…");
        assert!(s.action_ids().contains("valider"), "indexé par DragonTree : on peut valider");
        phone.action(h.state_mut(), &format!("{base}/modifs"), "valider", Some("Ajoute deux")).unwrap();
        // L'historique est lu à part : il peut montrer le commit avant que DragonTree ait fini l'opération.
        let log = wait_screen(&mut h, &mut phone, &format!("{base}/historique"), "Commit créé");
        assert!(format!("{log:?}").contains("Commit créé"), "le résultat de DragonTree : {log:?}");
        let log = wait_screen(&mut h, &mut phone, &format!("{base}/historique"), "Ajoute deux");
        assert!(format!("{log:?}").contains("Ajoute deux") && format!("{log:?}").contains("Premier commit"), "{log:?}");

        phone.action(h.state_mut(), &format!("{base}/branches"), "nouvelle-branche", Some("feature/essai")).unwrap();
        let b = wait_screen(&mut h, &mut phone, &format!("{base}/branches"), "Courante : feature/essai");
        assert!(b.action_ids().contains("basculer:main"), "{b:?}");
        if let Some(out) = std::env::var_os("SUITE_MOBILE_DUMP") {
            std::fs::write(repo.join("lisez-moi.txt"), "un\ndeux\ntrois\n").unwrap();
            std::fs::write(repo.join("nouveau.rs"), "fn main() {}\n").unwrap();
            phone.action(h.state_mut(), &base, "actualiser", None).unwrap();
            let out = Path::new(&out);
            for (name, path, wait) in [
                ("depot", base.clone(), "Non indexé"),
                ("modifs", format!("{base}/modifs"), "Non indexé"),
                ("diff", format!("{base}/fichier/{f}"), "+trois"),
                ("historique", format!("{base}/historique"), "Ajoute deux"),
                ("historique-tout", format!("{base}/historique/tout"), "Ajoute deux"),
                ("branches", format!("{base}/branches"), "feature/essai"),
                ("branche", format!("{base}/branche/{}", branches::branch_key("refs/heads/main")), "Premier commit"),
                ("plus", format!("{base}/plus"), "Remotes"),
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
        assert_eq!(band_of(&phone.screen(h.state_mut(), "").unwrap(), &key), Some("Ouverts sur le PC".into()));
        drop(h);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
