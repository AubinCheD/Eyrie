//! La liste des dépôts sur le téléphone : tous ceux de DragonTree, rangés comme dans sa barre latérale.
//!
//! - **Ouverts sur le PC** : l'onglet affiché, puis les autres onglets de DragonTree (sauf ceux que le téléphone a
//!   ouverts lui-même pour lire un dépôt : la liste ne bouge pas parce qu'on a regardé un dépôt) ;
//! - **Épinglés** : ses favoris ;
//! - un bandeau par **dossier racine** analysé (la liste de son dernier scan), plié au-delà de 15 dépôts ;
//! - **Récents** : ceux qui ne sont sous aucune racine.
//!
//! Chaque dépôt n'y est qu'une fois, dans le premier bandeau qui le prend ; ceux que DragonTree masque n'y sont pas,
//! sauf ouverts ou épinglés. Deux dépôts du même nom sont distingués par leur dossier parent (« Ozmo · Ozmo2 »).
//! Ce que DragonTree a lu d'un dépôt ouvert sert son état ; pour les autres, un relevé rapide fait à part.

use std::collections::{HashMap, HashSet};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use dragontree_app::settings::{Settings, path_key};
use dragontree_git::discovery::{FoundRepo, RepoKind};
use suite_sdk::mobile::ui::{Act, Band, Row, Screen, Tone};

/// Un bandeau de dossier racine est plié au départ au-delà de ce nombre de dépôts.
pub const FOLD_ABOVE: usize = 15;
/// Le résultat d'une opération reste affiché une minute.
pub const JOB_SHOWN: Duration = Duration::from_secs(60);

/// Où un dépôt est rangé dans la liste.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Group {
    /// Ouvert dans un onglet de DragonTree sur le PC.
    Open,
    /// Un favori de DragonTree.
    Pinned,
    /// Trouvé par le scan sous ce dossier racine.
    Root(PathBuf),
    Recent,
}

/// Un dépôt de la liste.
#[derive(Clone, Debug, PartialEq)]
pub struct Listed {
    pub path: PathBuf,
    pub group: Group,
    /// Son nom, suivi de son dossier parent s'il a un homonyme dans la liste.
    pub title: String,
    pub kind: RepoKind,
    /// Le dépôt de l'onglet affiché de DragonTree.
    pub active: bool,
}

/// Empreinte courte d'un dépôt (de sa clé de chemin, insensible à la casse comme Windows) : ce qui le désigne dans
/// un chemin d'écran (`depot/<clé>`) et dans une action, et l'identité de sa ligne.
pub fn repo_key(path: &Path) -> String {
    short_hash(&path_key(path))
}

/// Empreinte courte d'un texte (un dossier racine, une branche).
pub fn short_hash(text: &str) -> String {
    let mut h = DefaultHasher::new();
    text.hash(&mut h);
    format!("{:x}", h.finish() & 0xffff_ffff)
}

pub fn repo_name(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.display().to_string())
}

/// Les `depth` derniers dossiers au-dessus de `path` (« Ozmo2 », puis « DEV/Ozmo2 »…) ; `None` : il n'y en a pas
/// autant.
fn parents(path: &Path, depth: usize) -> Option<String> {
    let names: Vec<String> = path
        .parent()?
        .components()
        .filter_map(|c| match c {
            std::path::Component::Normal(n) => Some(n.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect();
    (names.len() >= depth).then(|| names[names.len() - depth..].join("/"))
}

/// Le titre de chaque dépôt : son nom, et pour les homonymes (sans casse) leur dossier parent, en remontant d'un
/// dossier tant qu'il en reste deux pareils ; en dernier recours, le chemin du parent.
pub fn titles(paths: &[&Path]) -> Vec<String> {
    let names: Vec<String> = paths.iter().map(|p| repo_name(p)).collect();
    let mut same: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, n) in names.iter().enumerate() {
        same.entry(n.to_lowercase()).or_default().push(i);
    }
    let mut out = names.clone();
    for group in same.values().filter(|g| g.len() > 1) {
        let mut pending: Vec<usize> = group.clone();
        for depth in 1..=3 {
            let hints: Vec<(usize, Option<String>)> = pending.iter().map(|&i| (i, parents(paths[i], depth))).collect();
            let mut taken: HashMap<String, usize> = HashMap::new();
            for (_, h) in &hints {
                if let Some(h) = h {
                    *taken.entry(h.to_lowercase()).or_default() += 1;
                }
            }
            pending.clear();
            for (i, h) in hints {
                match h {
                    Some(h) if taken[&h.to_lowercase()] == 1 => out[i] = format!("{} · {h}", names[i]),
                    _ => pending.push(i),
                }
            }
        }
        for i in pending {
            let parent = paths[i].parent().map(|p| p.display().to_string()).unwrap_or_default();
            out[i] = format!("{} · {parent}", names[i]);
        }
    }
    out
}

/// Les dépôts de DragonTree : `settings` (ses onglets, favoris, récents, dépôts masqués, racines), `found` (la liste
/// de son dernier scan), `added` (ajoutés depuis le téléphone, le plus récent en tête), l'onglet affiché `active`,
/// et `open` : le dépôt est ouvert dans un onglet de DragonTree par l'utilisateur (pas par le téléphone).
pub fn list(
    settings: &Settings,
    found: &[FoundRepo],
    added: &[PathBuf],
    active: Option<&Path>,
    open: impl Fn(&Path) -> bool,
) -> Vec<Listed> {
    let key_set = |paths: &[PathBuf]| paths.iter().map(|p| path_key(p)).collect::<HashSet<String>>();
    let hidden = key_set(&settings.hidden_repos);
    let favorites = key_set(&settings.favorites);
    let active_key = active.map(path_key);
    let found_by_key: HashMap<String, &FoundRepo> = found.iter().map(|f| (path_key(&f.path), f)).collect();
    // Les racines dans l'ordre des réglages, puis celles d'un scan plus ancien qui n'y sont plus.
    let mut roots: Vec<PathBuf> = settings.scan_roots.clone();
    for f in found {
        if !roots.iter().any(|r| path_key(r) == path_key(&f.root)) {
            roots.push(f.root.clone());
        }
    }
    let mut scanned: Vec<&FoundRepo> = found.iter().collect();
    scanned.sort_by_key(|f| {
        let root = roots.iter().position(|r| path_key(r) == path_key(&f.root)).unwrap_or(usize::MAX);
        (root, f.relative_parent().to_lowercase(), f.name.to_lowercase())
    });
    let candidates = active
        .map(Path::to_path_buf)
        .into_iter()
        .chain(settings.open_tabs.iter().cloned())
        .chain(settings.favorites.iter().cloned())
        .chain(scanned.iter().map(|f| f.path.clone()))
        .chain(added.iter().cloned())
        .chain(settings.recent_repos.iter().cloned());
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for path in candidates {
        let key = path_key(&path);
        if !seen.insert(key.clone()) {
            continue;
        }
        let is_active = active_key.as_ref() == Some(&key);
        let scan = found_by_key.get(&key);
        let group = if is_active || open(&path) {
            Group::Open
        } else if favorites.contains(&key) {
            Group::Pinned
        } else if hidden.contains(&key) {
            continue;
        } else if let Some(f) = scan {
            Group::Root(f.root.clone())
        } else {
            Group::Recent
        };
        let kind = scan.map_or(RepoKind::Normal, |f| f.kind);
        out.push(Listed { title: String::new(), path, group, kind, active: is_active });
    }
    // Rangés par bandeau : ouverts (l'affiché d'abord), épinglés, racines dans leur ordre et dans celui du scan, récents.
    let order: HashMap<String, usize> = scanned.iter().enumerate().map(|(i, f)| (path_key(&f.path), i)).collect();
    let rank = |l: &Listed| match &l.group {
        Group::Open => (0, 0),
        Group::Pinned => (1, 0),
        Group::Root(r) => (
            2 + roots.iter().position(|x| path_key(x) == path_key(r)).unwrap_or(roots.len()),
            order.get(&path_key(&l.path)).copied().unwrap_or(0),
        ),
        Group::Recent => (usize::MAX, 0),
    };
    out.sort_by_key(rank);
    let paths: Vec<&Path> = out.iter().map(|l| l.path.as_path()).collect();
    let titles = titles(&paths);
    for (l, t) in out.iter_mut().zip(titles) {
        l.title = t;
    }
    out
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

/// Ce que fait DragonTree sur un dépôt (`Info` : en cours, avec sa progression), ou ce qu'a donné sa dernière
/// opération.
#[derive(Clone, Debug, PartialEq)]
pub struct Job {
    pub text: String,
    pub tone: Tone,
    pub at: Instant,
    /// En cours : la progression en pour cent, arrondie à 5 (`None` : inconnue).
    pub percent: Option<u8>,
    /// En cours, et DragonTree sait l'arrêter (son bouton Annuler).
    pub stoppable: bool,
}

impl Job {
    pub fn running(&self) -> bool {
        self.tone == Tone::Info
    }
}

/// Un dépôt de la liste, avec ce qu'on en sait.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub repo: Listed,
    pub state: Option<RepoState>,
    pub job: Option<Job>,
    /// Couleur de son onglet dans DragonTree (0xRRGGBB).
    pub color: Option<u32>,
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
pub fn state_text(s: &RepoState) -> String {
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

fn kind_glyph(kind: RepoKind) -> &'static str {
    match kind {
        RepoKind::Normal => "🌿",
        RepoKind::Worktree => "🌳",
        RepoKind::Submodule => "📦",
    }
}

fn band_of(group: &Group, count: usize) -> Band {
    let band = match group {
        Group::Open => Band::new("ouverts", "Ouverts sur le PC").glyph("🖥️"),
        Group::Pinned => Band::new("epingles", "Épinglés").glyph("📌"),
        Group::Root(root) => Band::new(format!("racine/{}", short_hash(&path_key(root))), repo_name(root))
            .glyph("📁")
            .open(count <= FOLD_ABOVE),
        Group::Recent => Band::new("recents", "Récents").glyph("🕘"),
    };
    band.aside(count.to_string())
}

fn row_of(e: &Entry) -> Row {
    let key = repo_key(&e.repo.path);
    let mut row = Row::new(&e.repo.title).key(&key).glyph(kind_glyph(e.repo.kind)).open(format!("depot/{key}"));
    row = match &e.state {
        Some(st) => row.subtitle(state_text(st)).tone(state_tone(st)),
        None => row.subtitle("lecture…"),
    };
    if let Some(c) = e.color {
        row = row.color(c);
    }
    match &e.job {
        Some(j) if j.running() => row = row.badge("en cours").progress(j.percent),
        _ if e.repo.active => row = row.badge("affiché"),
        _ => {}
    }
    row.acts = net_acts(&key);
    row.act(Act::new(format!("pc:{key}"), "Afficher dans DragonTree sur le PC").glyph("🖥️"))
}

/// L'accueil : les dépôts par bandeau ; `sweeping` : l'état de ceux que DragonTree n'a pas ouverts est en cours de
/// relevé ; `add` : l'action « Ajouter un dépôt existant… ».
pub fn home(entries: &[Entry], sweeping: bool, add: Option<Act>) -> Screen {
    let mut s = Screen::new("DragonTree").search("Chercher un dépôt");
    s.acts.extend(add.clone());
    s.acts.push(Act::new("actualiser", "Actualiser").glyph("🔄"));
    if sweeping {
        s.subtitle = "Relevé de l'état des dépôts…".into();
    }
    let mut jobs: Vec<(&Entry, &Job)> = entries
        .iter()
        .filter_map(|e| Some((e, e.job.as_ref()?)))
        .filter(|(_, j)| !j.running() && j.at.elapsed() < JOB_SHOWN)
        .collect();
    jobs.sort_by_key(|(_, j)| j.at);
    for (e, j) in jobs {
        s.note(format!("{} : {}", e.repo.title, j.text), j.tone);
    }
    if entries.is_empty() {
        s.empty("Aucun dépôt : ajoutez-en un, ou ouvrez-en un dans DragonTree sur le PC.");
        if let Some(a) = add {
            s.buttons(vec![a.primary()]);
        }
    }
    let mut i = 0;
    while i < entries.len() {
        let group = &entries[i].repo.group;
        let n = entries[i..].iter().take_while(|e| e.repo.group == *group).count();
        s.band(band_of(group, n));
        for e in &entries[i..i + n] {
            s.row(row_of(e));
        }
        i += n;
    }
    s
}

#[cfg(test)]
mod tests {
    use suite_sdk::mobile::ui::{Block, Fold};

    use super::*;

    fn found(root: &str, rel: &str) -> FoundRepo {
        let path = Path::new(root).join(rel);
        FoundRepo { name: repo_name(&path), path, root: root.into(), kind: RepoKind::Normal }
    }

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

    fn listed(path: &str, group: Group) -> Listed {
        let path = PathBuf::from(path);
        Listed { title: repo_name(&path), path, group, kind: RepoKind::Normal, active: false }
    }

    fn entry(path: &str, group: Group, state: Option<RepoState>) -> Entry {
        Entry { repo: listed(path, group), state, job: None, color: None }
    }

    /// Les dépôts de DragonTree : l'onglet affiché et les onglets, les favoris, un bandeau par racine (le scan), les
    /// récents hors racines ; chacun une fois ; les masqués absents ; les homonymes distingués par leur parent.
    #[test]
    fn the_list_has_every_repo_of_dragontree_once_and_by_band() {
        let settings = Settings {
            open_tabs: vec![r"C:\dev\Ozmo2\Ozmo".into(), r"C:\dev\diapaz".into()],
            favorites: vec![r"c:\dev\XIMI\".into()],
            recent_repos: vec![r"C:\dev\ximi".into(), r"C:\ailleurs\walle".into(), r"C:\dev\cache".into()],
            hidden_repos: vec![r"C:\dev\cache".into()],
            scan_roots: vec![r"C:\dev".into()],
            ..Default::default()
        };
        let scan = [
            found(r"C:\dev", "ximi"),
            found(r"C:\dev", r"Ozmo2\Ozmo"),
            found(r"C:\dev", r"Survey2\Ozmo"),
            found(r"C:\dev", "diapaz"),
            found(r"C:\dev", "cache"),
            found(r"C:\dev", "cockpit"),
        ];
        let added = [PathBuf::from(r"D:\neuf")];
        // Diapaz est ouvert par le téléphone seulement : il reste dans sa racine.
        let open = |p: &Path| path_key(p) == path_key(Path::new(r"C:\dev\Ozmo2\Ozmo"));
        let l = list(&settings, &scan, &added, Some(Path::new(r"C:\dev\walle")), open);
        let got: Vec<(&str, Group, bool)> = l.iter().map(|l| (l.title.as_str(), l.group.clone(), l.active)).collect();
        let root = Group::Root(r"C:\dev".into());
        assert_eq!(
            got,
            [
                ("walle · dev", Group::Open, true),
                ("Ozmo · Ozmo2", Group::Open, false),
                ("XIMI", Group::Pinned, false),
                ("cockpit", root.clone(), false),
                ("diapaz", root.clone(), false),
                ("Ozmo · Survey2", root, false),
                ("neuf", Group::Recent, false),
                ("walle · ailleurs", Group::Recent, false),
            ]
        );
    }

    #[test]
    fn homonyms_climb_up_their_parents_until_they_differ() {
        let p = [r"C:\a\x\Ozmo", r"C:\b\x\Ozmo", r"C:\c\y\ozmo", r"C:\dev\Eyrie"].map(PathBuf::from);
        let refs: Vec<&Path> = p.iter().map(PathBuf::as_path).collect();
        assert_eq!(titles(&refs), ["Ozmo · a/x", "Ozmo · b/x", "ozmo · y", "Eyrie"]);
        assert_eq!(repo_key(Path::new(r"C:\dev\ozmo")), repo_key(Path::new(r"c:/dev/OZMO/")), "même dossier");
    }

    #[test]
    fn the_home_shows_bands_each_repo_state_and_its_actions() {
        let mut shown = entry(r"C:\dev\ozmo", Group::Open, Some(state("main", 2, 1, 3)));
        shown.repo.active = true;
        let root = Group::Root(r"C:\dev".into());
        let mut busy = entry(r"C:\dev\ximi", root.clone(), None);
        busy.job = Some(Job {
            text: "Fetch en cours…".into(),
            tone: Tone::Info,
            at: Instant::now(),
            percent: Some(40),
            stoppable: true,
        });
        let broken = entry(
            r"C:\dev\parti",
            Group::Recent,
            Some(RepoState { error: Some("dossier introuvable".into()), ..Default::default() }),
        );
        let s = home(&[shown, busy, broken], false, None);
        assert!(!s.search.is_empty(), "une recherche");
        let bands: Vec<(&str, Fold)> = s
            .blocks
            .iter()
            .filter_map(|b| match b {
                Block::Section { title, fold, .. } => Some((title.as_str(), *fold)),
                _ => None,
            })
            .collect();
        assert_eq!(bands, [("Ouverts sur le PC", Fold::Open), ("dev", Fold::Open), ("Récents", Fold::Open)]);
        let rows: Vec<&Row> = s
            .blocks
            .iter()
            .filter_map(|b| match b {
                Block::Row(r) => Some(r),
                _ => None,
            })
            .collect();
        let key = repo_key(Path::new(r"C:\dev\ozmo"));
        assert_eq!((rows[0].subtitle.as_str(), rows[0].badge.as_str()), ("main · ↑2 ↓1 · 3 modifications", "affiché"));
        assert_eq!((rows[0].tone, rows[0].key.as_str()), (Some(Tone::Warn), key.as_str()));
        assert_eq!(
            (rows[1].subtitle.as_str(), rows[1].badge.as_str(), rows[1].progress),
            ("lecture…", "en cours", Some(40))
        );
        assert_eq!((rows[2].subtitle.as_str(), rows[2].tone), ("dossier introuvable", Some(Tone::Danger)));
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
    fn a_big_root_is_folded_at_first() {
        let root = Group::Root(r"C:\dev".into());
        let entries: Vec<Entry> =
            (0..=FOLD_ABOVE).map(|i| entry(&format!(r"C:\dev\d{i}"), root.clone(), None)).collect();
        let s = home(&entries, false, None);
        assert!(matches!(&s.blocks[0], Block::Section { fold: Fold::Closed, aside, .. } if aside == "16"));
    }
}
