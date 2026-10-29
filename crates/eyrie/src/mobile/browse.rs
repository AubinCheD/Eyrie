//! Les dossiers du PC pour le sélecteur du téléphone (`docs/mobile.md` §5). Le sélecteur n'existe que pour une action
//! qu'un écran a proposée avec une saisie « dossier » ([`FolderInput`]) : le pont garde sa spécification avec la liste
//! blanche ([`super::sync`]).
//!
//! - [`check`] est le seul contrôle d'un chemin venu du téléphone, pour lister comme pour agir : forme `X:\…` (ni UNC,
//!   ni `\\?\` ou `\\.\`, ni flux `:`, ni `..`, ni nom réservé), disque fixe du PC, liens qui ne mènent pas ailleurs
//!   (lus sans être suivis), résolu (`canonicalize` : noms courts, jonctions) puis revérifié, hors des zones
//!   interdites ([`Env`]) ;
//! - [`chosen`] y ajoute la règle de la saisie (`want`), avant que le pont n'appelle le module ;
//! - [`listing`] lit un niveau (des dossiers seulement, triés, filtrés, par pages de [`PAGE`], avec leurs drapeaux) ou
//!   la racine (dossiers choisis récemment, raccourcis du module, accès rapide, lecteurs fixes) ;
//! - [`Browser`] : une lecture à la fois par téléphone, dans un fil (jamais dans celui de l'interface) ; la demande la
//!   plus récente attend son tour, les rafales sont limitées.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use suite_mobile::msg::{
    ENTRY_CLOUD, ENTRY_DENIED, ENTRY_DRIVE, ENTRY_HIDDEN, ENTRY_KNOWN, ENTRY_LINK, ENTRY_REPO, FolderEntry, Listing,
    PlaceGroup, ToPhone,
};
use suite_mobile::paths::protected_name;
use suite_mobile::ui::{FOLDER_ANY, FOLDER_PARENT, FOLDER_REPO, FolderInput, Tone};

/// Dossiers par page.
pub const PAGE: usize = 200;
/// Une lecture s'arrête à tant d'entrées ou au bout de tant de temps : le dossier est dit tronqué (il faut filtrer).
const MAX_ENTRIES: usize = 20_000;
const MAX_TIME: Duration = Duration::from_secs(3);
/// Au plus tant de lectures lancées par fenêtre, pour un téléphone.
const BURST: usize = 8;
const BURST_WINDOW: Duration = Duration::from_secs(2);
const MAX_PATH_LEN: usize = 2048;
const MAX_FILTER_LEN: usize = 200;
/// Dossiers connus du module relus (résolus) pour les reconnaître : au-delà, comparés tels qu'écrits.
const MAX_KNOWN: usize = 1000;
/// Dossiers parents des dossiers connus ajoutés à l'accès rapide.
const MAX_ROOTS: usize = 6;

/// Le libellé de la racine du sélecteur.
const ROOT: &str = "Ce PC";
const NETWORK: &str = "Chemin réseau refusé : seuls les disques du PC sont proposés.";

/// Attributs de fichier de Windows (`FILE_ATTRIBUTE_…`).
const HIDDEN: u32 = 0x2;
const SYSTEM: u32 = 0x4;
const DIRECTORY: u32 = 0x10;
const OFFLINE: u32 = 0x1000;
const RECALL_ON_OPEN: u32 = 0x40000;
const RECALL_ON_DATA_ACCESS: u32 = 0x40_0000;

/// La sorte d'un lecteur (`GetDriveTypeW`) : seuls les disques fixes sont proposés.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Drive {
    Fixed,
    Removable,
    Remote,
    Other,
}

/// Ce que le contrôle et le sélecteur savent du PC : [`Env::system`] pour de vrai, un autre dans les essais (le dossier
/// temporaire est sous `AppData`, interdit).
pub struct Env {
    /// Dossiers interdits, et tout ce qu'ils contiennent : tels qu'écrits et résolus.
    forbidden: Vec<PathBuf>,
    /// Le dossier des profils (`C:\Users`) et celui de l'utilisateur, sous ses noms long et court
    /// (`C:\Users\CLBR-A~1`) : les autres profils sont interdits.
    profiles: Option<(PathBuf, Vec<PathBuf>)>,
    /// L'accès rapide : libellé et dossier, montré s'il existe et passe le contrôle.
    quick: Vec<(String, PathBuf)>,
    drive: fn(char) -> Drive,
    /// Les lecteurs (lettre, nom du volume).
    drives: fn() -> Vec<(char, String)>,
    max_entries: usize,
    max_time: Duration,
}

impl Env {
    /// Le PC : Windows, les programmes, les données des applications (celles de la suite et `data`, celles d'Eyrie,
    /// comprises), les profils des autres utilisateurs.
    pub fn system(data: &[&Path]) -> Env {
        let vars = ["SystemRoot", "windir", "ProgramFiles", "ProgramFiles(x86)", "ProgramW6432", "ProgramData"];
        let vars = vars.into_iter().chain(["APPDATA", "LOCALAPPDATA"]);
        let mut forbidden: Vec<PathBuf> =
            vars.filter_map(|v| std::env::var_os(v).filter(|d| !d.is_empty())).map(PathBuf::from).collect();
        let home = dirs::home_dir();
        forbidden.extend(home.as_ref().map(|h| h.join("AppData")));
        forbidden.push(suite_accounts::suite_dir());
        forbidden.push(suite_accounts::local_suite_dir());
        forbidden.extend(data.iter().map(|d| d.to_path_buf()));
        let profiles = home.as_ref().and_then(|h| Some((resolved_or(h.parent()?), forms(h))));
        let mut quick = Vec::new();
        quick.extend(home.clone().map(|h| ("Dossier personnel".to_string(), h)));
        quick.extend(dirs::desktop_dir().map(|d| ("Bureau".to_string(), d)));
        quick.extend(dirs::document_dir().map(|d| ("Documents".to_string(), d)));
        quick.extend(dirs::download_dir().map(|d| ("Téléchargements".to_string(), d)));
        if let Some(h) = &home {
            quick.push((r"source\repos".to_string(), h.join("source").join("repos")));
            quick.push(("DEV".to_string(), h.join("DEV")));
        }
        if let Some(drive) = std::env::var("SystemDrive").ok().filter(|d| d.len() == 2) {
            quick.push((format!("DEV ({drive})"), PathBuf::from(format!(r"{drive}\DEV"))));
        }
        Env {
            forbidden: with_resolved(forbidden),
            profiles,
            quick,
            drive: drive_kind,
            drives: fixed_drives,
            max_entries: MAX_ENTRIES,
            max_time: MAX_TIME,
        }
    }

    /// Essais : seulement `forbidden` et les noms interdits ; les vrais lecteurs.
    #[cfg(test)]
    pub fn for_tests(forbidden: Vec<PathBuf>) -> Env {
        Env {
            forbidden: with_resolved(forbidden),
            profiles: None,
            quick: Vec::new(),
            drive: drive_kind,
            drives: fixed_drives,
            max_entries: MAX_ENTRIES,
            max_time: MAX_TIME,
        }
    }
}

/// Les zones et leur forme résolue (noms courts, jonctions), quand elle diffère.
fn with_resolved(zones: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for z in zones {
        if let Some(full) = resolved(&z).filter(|f| key(f) != key(&z)) {
            out.push(full);
        }
        out.push(z);
    }
    out
}

/// `p` résolu, sans `\\?\` ; `None` s'il n'existe pas ou ne se lit pas.
fn resolved(p: &Path) -> Option<PathBuf> {
    let full = std::fs::canonicalize(p).ok()?;
    let text = full.to_str()?;
    Some(PathBuf::from(text.strip_prefix(r"\\?\").unwrap_or(text)))
}

fn resolved_or(p: &Path) -> PathBuf {
    resolved(p).unwrap_or_else(|| p.to_path_buf())
}

/// Un dossier sous ses noms long et court : résolu, puis son dernier nom en 8.3 (`C:\Users\CLBR-A~1`), puis tout en
/// 8.3, quand ils diffèrent. Le contrôle d'avant la résolution reconnaît ainsi le profil de l'utilisateur écrit court.
fn forms(p: &Path) -> Vec<PathBuf> {
    let long = resolved_or(p);
    let mut out = vec![long.clone()];
    if let Some(short) = short_name(&long) {
        if let (Some(parent), Some(name)) = (long.parent(), short.file_name()) {
            out.push(parent.join(name));
        }
        out.push(short);
    }
    let mut seen = BTreeSet::new();
    out.retain(|f| seen.insert(key(f)));
    out
}

/// Le nom court (8.3) d'un chemin existant, s'il en a un.
#[cfg(windows)]
fn short_name(p: &Path) -> Option<PathBuf> {
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use windows_sys::Win32::Storage::FileSystem::GetShortPathNameW;
    let wide: Vec<u16> = p.as_os_str().encode_wide().chain([0]).collect();
    let mut buf = vec![0u16; MAX_PATH_LEN + 1];
    // SAFETY : chaîne terminée par zéro ; le tampon a la taille annoncée.
    let n = unsafe { GetShortPathNameW(wide.as_ptr(), buf.as_mut_ptr(), buf.len() as u32) } as usize;
    (n > 0 && n < buf.len()).then(|| PathBuf::from(std::ffi::OsString::from_wide(&buf[..n])))
}

#[cfg(not(windows))]
fn short_name(_: &Path) -> Option<PathBuf> {
    None
}

/// La clé d'un chemin pour comparer : sans casse, `/` pour séparateur, sans `/` final.
fn key(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/").trim_end_matches('/').to_lowercase()
}

/// `p` est `zone` ou en dessous.
fn inside(p: &Path, zone: &Path) -> bool {
    let (p, z) = (key(p), key(zone));
    !z.is_empty() && (p == z || p.starts_with(&format!("{z}/")))
}

/// Un nom de dossier permis : ni nom réservé de Windows (`CON`, `NUL`, `COM1`…, extension comprise), ni fini par un
/// point ou une espace (Windows les retire).
fn name_ok(name: &str) -> Result<(), String> {
    let stem = name.split('.').next().unwrap_or_default().trim_end().to_ascii_uppercase();
    let numbered = |p: &str| {
        stem.strip_prefix(p).is_some_and(|n| matches!(n.as_bytes(), [b'1'..=b'9']) || matches!(n, "¹" | "²" | "³"))
    };
    if matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$")
        || numbered("COM")
        || numbered("LPT")
    {
        return Err(format!("Chemin refusé : « {name} » est un nom réservé de Windows."));
    }
    if name.ends_with('.') || name.ends_with(' ') {
        return Err(format!("Chemin refusé : « {name} » finit par un point ou une espace."));
    }
    Ok(())
}

/// La forme `X:\…` d'un disque : ni réseau (UNC), ni `\\?\` ou `\\.\`, ni flux (`:` après le lecteur), ni `..`, ni
/// caractère interdit, ni nom refusé par [`name_ok`].
fn shape(text: &str) -> Result<(), String> {
    if text.len() > MAX_PATH_LEN {
        return Err("Chemin refusé : trop long.".into());
    }
    let b = text.as_bytes();
    let sep = |c: u8| matches!(c, b'\\' | b'/');
    if b.len() >= 4 && sep(b[0]) && sep(b[1]) && matches!(b[2], b'?' | b'.') && sep(b[3]) {
        return Err(r"Chemin refusé : les préfixes \\?\ et \\.\ ne sont pas permis.".into());
    }
    if b.len() >= 2 && sep(b[0]) && sep(b[1]) {
        return Err(NETWORK.into());
    }
    if !(b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && sep(b[2])) {
        return Err(r"Chemin refusé : un dossier d'un disque du PC est attendu (C:\…).".into());
    }
    if text[2..].contains(':') {
        return Err("Chemin refusé : « : » après le lecteur (flux de données) n'est pas permis.".into());
    }
    if let Some(c) = text.chars().find(|c| c.is_control() || matches!(c, '*' | '?' | '"' | '<' | '>' | '|')) {
        return Err(format!("Chemin refusé : caractère « {} » non permis.", c.escape_default()));
    }
    for c in Path::new(text).components() {
        match c {
            Component::ParentDir => return Err("Chemin refusé : « .. » n'est pas permis.".into()),
            Component::Normal(n) => name_ok(&n.to_string_lossy())?,
            _ => {}
        }
    }
    Ok(())
}

/// Le lecteur de `text` (de forme `X:\…`) est un disque fixe du PC.
fn fixed(env: &Env, text: &str) -> Result<(), String> {
    let letter = text.chars().next().unwrap_or_default().to_ascii_uppercase();
    match (env.drive)(letter) {
        Drive::Fixed => Ok(()),
        Drive::Remote => Err(NETWORK.into()),
        Drive::Removable => {
            Err(format!("{letter}: est un lecteur amovible : seuls les disques fixes du PC sont proposés."))
        }
        Drive::Other => Err(format!("{letter}: n'est pas un disque du PC.")),
    }
}

/// `p` est dans une zone interdite : sous un nom protégé (`.ssh`, `.aws`… : [`protected_name`]), dans une zone de
/// l'environnement, dans le profil d'un autre utilisateur.
fn protected(env: &Env, p: &Path) -> bool {
    let named = p.components().any(|c| matches!(c, Component::Normal(n) if protected_name(&n.to_string_lossy())));
    let zoned = env.forbidden.iter().any(|z| inside(p, z));
    let other_profile = env
        .profiles
        .as_ref()
        .is_some_and(|(all, mine)| inside(p, all) && key(p) != key(all) && !mine.iter().any(|m| inside(p, m)));
    named || zoned || other_profile
}

fn protected_text(shown: &str) -> String {
    format!("{shown} : dossier protégé, refusé depuis le téléphone.")
}

/// Les liens suivis au plus pour contrôler un chemin.
const MAX_LINKS: usize = 8;

/// Où mène un lien (sa cible lue par `read_link`, `at` son emplacement) : le chemin à contrôler ensuite, ou `None` pour
/// un volume local désigné autrement (`\\?\Volume{…}`) ; refusé s'il mène à un partage réseau ou à un lecteur qui
/// n'est pas un disque fixe du PC.
fn link_target(env: &Env, at: &Path, target: &Path) -> Result<Option<PathBuf>, String> {
    let text = target.to_string_lossy();
    let sep = |c: u8| matches!(c, b'\\' | b'/');
    let bare = text.strip_prefix(r"\\?\").unwrap_or(&text);
    let b = bare.as_bytes();
    if bare.get(..4).is_some_and(|p| p.eq_ignore_ascii_case(r"UNC\")) || (b.len() >= 2 && sep(b[0]) && sep(b[1])) {
        return Err(NETWORK.into());
    }
    if b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':' {
        fixed(env, bare)?;
        return Ok(Some(PathBuf::from(bare)));
    }
    if target.is_relative() {
        return Ok(Some(at.parent().unwrap_or(at).join(target)));
    }
    Ok(None)
}

/// Les liens symboliques et jonctions de `path`, lus sans être suivis, et ceux de leurs cibles : aucun ne doit mener
/// hors des disques fixes du PC. `canonicalize` les suit ensuite sans attendre un serveur absent (le contrôle d'une
/// action se fait dans le fil de l'interface) ; ces lectures-ci restent sur les disques du PC.
fn links(env: &Env, path: &Path, hops: &mut usize) -> Result<(), String> {
    let mut at = PathBuf::new();
    for c in path.components() {
        at.push(c);
        if !matches!(c, Component::Normal(_)) {
            continue;
        }
        // Absent ou illisible : `canonicalize` dira pourquoi.
        let Ok(meta) = std::fs::symlink_metadata(&at) else { return Ok(()) };
        if !meta.file_type().is_symlink() {
            continue;
        }
        *hops += 1;
        if *hops > MAX_LINKS {
            return Err("Chemin refusé : trop de liens à suivre.".into());
        }
        let Ok(target) = std::fs::read_link(&at) else { continue };
        if let Some(next) = link_target(env, &at, &target)? {
            links(env, &next, hops)?;
        }
    }
    Ok(())
}

/// Le contrôle d'un chemin venu du téléphone, le même pour lister et pour agir : un dossier existant d'un disque fixe
/// du PC, hors des zones interdites, avant et après résolution (noms courts, jonctions, liens). Rend le chemin résolu,
/// sans `\\?\` : c'est lui qu'on lit ou que reçoit le module, pas l'endroit où mènerait une jonction changée
/// entre-temps.
pub fn check(env: &Env, text: &str) -> Result<PathBuf, String> {
    shape(text)?;
    fixed(env, text)?;
    if protected(env, Path::new(text)) {
        return Err(protected_text(text));
    }
    links(env, Path::new(text), &mut 0)?;
    let full = std::fs::canonicalize(text).map_err(|e| match e.kind() {
        ErrorKind::NotFound => format!("{text} n'existe pas sur le PC."),
        ErrorKind::PermissionDenied => format!("{text} : accès refusé."),
        _ => format!("{text} : {e}"),
    })?;
    let full = full.to_str().ok_or_else(|| format!("{text} : nom illisible."))?;
    // Un lecteur réseau (ou un `subst` vers un partage) se résout en `\\?\UNC\…`.
    if full.get(..8).is_some_and(|p| p.eq_ignore_ascii_case(r"\\?\UNC\")) {
        return Err(NETWORK.into());
    }
    let full = full.strip_prefix(r"\\?\").unwrap_or(full);
    shape(full)?;
    fixed(env, full)?;
    if protected(env, Path::new(full)) {
        return Err(protected_text(text));
    }
    let full = PathBuf::from(full);
    if !full.is_dir() {
        return Err(format!("{text} n'est pas un dossier."));
    }
    Ok(full)
}

/// Le nom du dossier qu'une saisie [`FOLDER_PARENT`] fera créer : un seul nom permis.
fn child_ok(child: &str) -> Result<(), String> {
    let single = !child.is_empty() && child.len() <= 255 && !child.contains(['\\', '/', ':']) && child != "..";
    let clean = !child.chars().any(|c| c.is_control() || matches!(c, '*' | '?' | '"' | '<' | '>' | '|'));
    if !(single && clean) {
        return Err(format!("Nom de dossier refusé : « {child} »."));
    }
    name_ok(child)
}

/// La règle de la saisie sur un dossier contrôlé par [`check`] : la cible (ce dossier, ou `dossier\child` pour
/// [`FOLDER_PARENT`]) et si elle sera créée ; sinon pourquoi ce dossier ne convient pas.
fn rule(env: &Env, dir: &Path, spec: &FolderInput) -> Result<(PathBuf, bool), String> {
    match spec.want {
        FOLDER_ANY => Ok((dir.to_path_buf(), false)),
        FOLDER_REPO if dir.join(".git").exists() => Ok((dir.to_path_buf(), false)),
        FOLDER_REPO => Err(format!("{} n'est pas un dépôt git.", dir.display())),
        FOLDER_PARENT if spec.child.is_empty() => Ok((dir.to_path_buf(), false)),
        FOLDER_PARENT => {
            child_ok(&spec.child)?;
            let target = dir.join(&spec.child);
            if protected(env, &target) {
                return Err(protected_text(&target.display().to_string()));
            }
            match std::fs::symlink_metadata(&target) {
                Err(e) if e.kind() == ErrorKind::NotFound => Ok((target, true)),
                Err(e) => Err(format!("{} : {e}", target.display())),
                Ok(m) if m.is_dir() => {
                    // Un dossier existant peut être écrit sous son nom court (`AWS~1` pour `.aws`) : contrôlé résolu.
                    check(env, &target.to_string_lossy())?;
                    match std::fs::read_dir(&target).map(|mut r| r.next().is_none()) {
                        Ok(true) => Ok((target, false)),
                        Ok(false) => Err(format!("{} existe déjà et n'est pas vide.", target.display())),
                        Err(e) => Err(format!("{} : {e}", target.display())),
                    }
                }
                Ok(_) => Err(format!("{} existe déjà.", target.display())),
            }
        }
        _ => Err("Cette saisie de dossier est inconnue de ce PC : mettez Eyrie à jour.".into()),
    }
}

/// Le dossier choisi pour une action qui en demande un, contrôlé avant d'appeler le module : [`check`], puis la règle
/// de la saisie. Rend le chemin résolu, celui que reçoit le module (pour [`FOLDER_PARENT`], le dossier parent).
pub fn chosen(env: &Env, input: Option<&str>, spec: &FolderInput) -> Result<PathBuf, String> {
    let text = input.map(str::trim).filter(|t| !t.is_empty()).ok_or("Choisissez un dossier du PC.")?;
    let dir = check(env, text)?;
    rule(env, &dir, spec)?;
    Ok(dir)
}

/// Une demande de liste, acceptée par le pont (droits de l'appareil, action proposée par l'écran suivi).
#[derive(Clone, Debug)]
pub struct Ask {
    pub req: u32,
    /// `None` ou vide : la racine.
    pub dir: Option<String>,
    pub filter: String,
    pub offset: u32,
    /// Montrer les dossiers cachés.
    pub hidden: bool,
    pub spec: FolderInput,
    /// Le nom du module (« déjà dans DragonTree »).
    pub module: String,
    /// Les dossiers choisis récemment pour ce module, le plus récent d'abord (gardés par le pont).
    pub recents: Vec<String>,
}

/// Une réponse qui n'est qu'une erreur.
pub fn refused(dir: Option<String>, error: impl Into<String>) -> Listing {
    Listing { dir: dir.unwrap_or_default(), error: error.into(), ..Default::default() }
}

/// Le contenu d'un dossier, ou la racine, pour le sélecteur.
pub fn listing(env: &Env, ask: &Ask) -> Listing {
    let known = Known::new(env, &ask.spec);
    match ask.dir.as_deref().map(str::trim).filter(|d| !d.is_empty()) {
        None => root(env, ask, &known),
        Some(dir) => folder(env, ask, dir, &known),
    }
}

/// Les dossiers que le module connaît (`known` et `places`), tels qu'écrits et résolus.
struct Known(BTreeSet<String>);

impl Known {
    fn new(env: &Env, spec: &FolderInput) -> Known {
        let mut keys = BTreeSet::new();
        let all = spec.known.iter().chain(spec.places.iter().map(|p| &p.path));
        for (i, p) in all.enumerate() {
            keys.insert(key(Path::new(p)));
            // Résolu seulement sur un disque fixe : un chemin réseau ne fait pas attendre la lecture.
            if i < MAX_KNOWN
                && shape(p).is_ok()
                && fixed(env, p).is_ok()
                && let Some(full) = resolved(Path::new(p))
            {
                keys.insert(key(&full));
            }
        }
        Known(keys)
    }

    fn has(&self, p: &Path) -> bool {
        self.0.contains(&key(p))
    }
}

/// Le texte d'une entrée d'après ses drapeaux.
fn detail(flags: u8, module: &str) -> String {
    let mut parts = Vec::new();
    if flags & ENTRY_REPO != 0 {
        parts.push("dépôt git".to_string());
    }
    if flags & ENTRY_KNOWN != 0 {
        parts.push(format!("déjà dans {module}"));
    }
    if flags & ENTRY_LINK != 0 {
        parts.push("lien".into());
    }
    if flags & ENTRY_CLOUD != 0 {
        parts.push("en ligne seulement".into());
    }
    if flags & ENTRY_DENIED != 0 {
        parts.push("protégé".into());
    }
    parts.join(" · ")
}

/// Un raccourci de la racine : un dossier déjà contrôlé.
fn place(name: &str, dir: &Path, known: &Known, module: &str) -> FolderEntry {
    let mut flags = 0;
    if dir.join(".git").exists() {
        flags |= ENTRY_REPO;
    }
    if known.has(dir) {
        flags |= ENTRY_KNOWN;
    }
    let path = dir.to_string_lossy().into_owned();
    let what = detail(flags, module);
    let detail = if what.is_empty() { path.clone() } else { format!("{path} · {what}") };
    FolderEntry { name: name.to_string(), path, flags, detail }
}

/// La racine : les dossiers choisis récemment, les raccourcis du module, l'accès rapide (avec les dossiers où le module
/// a les siens), les disques fixes. Un dossier n'y est qu'une fois, dans le premier groupe qui le montre.
fn root(env: &Env, ask: &Ask, known: &Known) -> Listing {
    let mut seen = BTreeSet::new();
    let mut groups = Vec::new();
    let mut recents = Vec::new();
    for p in &ask.recents {
        if let Ok(dir) = check(env, p)
            && seen.insert(key(&dir))
        {
            recents.push(place(&title_of(&dir), &dir, known, &ask.module));
        }
    }
    groups.push(PlaceGroup { title: "Récents".into(), places: recents });
    let mut places = Vec::new();
    for p in &ask.spec.places {
        if let Ok(dir) = check(env, &p.path)
            && seen.insert(key(&dir))
        {
            let name = if p.label.is_empty() { title_of(&dir) } else { p.label.clone() };
            places.push(place(&name, &dir, known, &ask.module));
        }
    }
    groups.push(PlaceGroup { title: ask.module.clone(), places });

    // Les dossiers où le module a les siens (C:\DEV pour C:\DEV\Ozmo), les plus fournis d'abord.
    let mut parents: BTreeMap<String, (usize, PathBuf)> = BTreeMap::new();
    let all = ask.spec.known.iter().chain(ask.spec.places.iter().map(|p| &p.path));
    for p in all.filter(|p| shape(p).is_ok()) {
        if let Some(parent) = Path::new(p).parent().filter(|d| d.file_name().is_some()) {
            parents.entry(key(parent)).or_insert_with(|| (0, parent.to_path_buf())).0 += 1;
        }
    }
    let mut parents: Vec<(usize, PathBuf)> = parents.into_values().collect();
    parents.sort_by_key(|(count, _)| std::cmp::Reverse(*count));
    let roots = parents.into_iter().take(MAX_ROOTS).map(|(_, d)| (String::new(), d));
    let mut quick = Vec::new();
    for (label, dir) in env.quick.iter().cloned().chain(roots) {
        if let Ok(dir) = check(env, &dir.to_string_lossy())
            && seen.insert(key(&dir))
        {
            let name = if label.is_empty() { title_of(&dir) } else { label };
            quick.push(place(&name, &dir, known, &ask.module));
        }
    }
    groups.push(PlaceGroup { title: "Accès rapide".into(), places: quick });

    let drives = (env.drives)()
        .into_iter()
        .filter(|(letter, _)| (env.drive)(*letter) == Drive::Fixed)
        .map(|(letter, label)| {
            let label = if label.is_empty() { "Disque local".to_string() } else { label };
            FolderEntry {
                name: format!("{label} ({letter}:)"),
                path: format!(r"{letter}:\"),
                flags: ENTRY_DRIVE,
                detail: String::new(),
            }
        })
        .collect();
    groups.push(PlaceGroup { title: "Lecteurs".into(), places: drives });
    groups.retain(|g| !g.places.is_empty());
    Listing {
        title: ROOT.into(),
        crumbs: vec![(ROOT.into(), String::new())],
        groups,
        why_not: "Ouvrez un dossier pour le choisir.".into(),
        ..Default::default()
    }
}

/// Le titre d'un dossier : son nom, ou son lecteur (`C:`).
fn title_of(dir: &Path) -> String {
    match dir.file_name() {
        Some(n) => n.to_string_lossy().into_owned(),
        None => dir.to_string_lossy().trim_end_matches(['\\', '/']).to_string(),
    }
}

/// Le fil d'Ariane d'un dossier résolu (`C:\DEV\Ozmo`) : « Ce PC », « C: », « DEV », « Ozmo ».
fn crumbs(dir: &str) -> Vec<(String, String)> {
    let mut out = vec![(ROOT.to_string(), String::new())];
    let (drive, rest) = dir.split_at(2.min(dir.len()));
    let mut at = format!(r"{drive}\");
    out.push((drive.to_string(), at.clone()));
    for part in rest.split(['\\', '/']).filter(|p| !p.is_empty()) {
        if !at.ends_with('\\') {
            at.push('\\');
        }
        at.push_str(part);
        out.push((part.to_string(), at.clone()));
    }
    out
}

/// Sans casse ni accents, pour filtrer et trier.
fn fold(s: &str) -> String {
    s.chars()
        .flat_map(char::to_lowercase)
        .map(|c| match c {
            'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' => 'a',
            'ç' => 'c',
            'è' | 'é' | 'ê' | 'ë' => 'e',
            'ì' | 'í' | 'î' | 'ï' => 'i',
            'ñ' => 'n',
            'ò' | 'ó' | 'ô' | 'õ' | 'ö' => 'o',
            'ù' | 'ú' | 'û' | 'ü' => 'u',
            'ý' | 'ÿ' => 'y',
            c => c,
        })
        .collect()
}

/// Un dossier lu, avant tri.
struct Raw {
    name: String,
    attrs: u32,
    link: bool,
}

#[derive(Default)]
struct Level {
    dirs: Vec<Raw>,
    files: u32,
    truncated: bool,
}

#[cfg(windows)]
fn attributes(e: &std::fs::DirEntry) -> u32 {
    use std::os::windows::fs::MetadataExt;
    // Sous Windows, les attributs d'une entrée viennent de la lecture du dossier : pas d'accès de plus au disque.
    e.metadata().map(|m| m.file_attributes()).unwrap_or(0)
}

#[cfg(not(windows))]
fn attributes(_: &std::fs::DirEntry) -> u32 {
    0
}

/// Lit un niveau, sans rien suivre : les dossiers (liens et jonctions compris), et combien de fichiers.
fn read_level(env: &Env, dir: &Path) -> Result<Level, String> {
    let started = Instant::now();
    let entries = std::fs::read_dir(dir).map_err(|e| match e.kind() {
        ErrorKind::PermissionDenied => "Accès refusé à ce dossier.".to_string(),
        ErrorKind::NotFound => "Ce dossier n'existe plus.".to_string(),
        _ => format!("Lecture impossible : {e}"),
    })?;
    let mut level = Level::default();
    for (n, e) in entries.enumerate() {
        if n >= env.max_entries || started.elapsed() >= env.max_time {
            level.truncated = true;
            break;
        }
        let Ok(e) = e else { continue };
        let Ok(kind) = e.file_type() else { continue };
        let attrs = attributes(&e);
        // Un lien ou une jonction (sous Windows, `is_symlink` couvre les deux) n'est pas un `is_dir`.
        let link = kind.is_symlink();
        if !(kind.is_dir() || (link && attrs & DIRECTORY != 0)) {
            level.files += 1;
            continue;
        }
        let Some(name) = e.file_name().to_str().map(str::to_string) else { continue };
        level.dirs.push(Raw { name, attrs, link });
    }
    Ok(level)
}

/// Les drapeaux qui se lisent dans les attributs d'une entrée, sans autre accès au disque : caché (attribut, ou nom en
/// `.`), lien ou jonction, en ligne seulement (OneDrive).
fn flags_of(attrs: u32, link: bool, name: &str) -> u8 {
    let mut f = 0;
    if attrs & (HIDDEN | SYSTEM) != 0 || name.starts_with('.') {
        f |= ENTRY_HIDDEN;
    }
    if link {
        f |= ENTRY_LINK;
    } else if attrs & (OFFLINE | RECALL_ON_OPEN | RECALL_ON_DATA_ACCESS) != 0 {
        f |= ENTRY_CLOUD;
    }
    f
}

fn entry(env: &Env, dir: &Path, r: &Raw, known: &Known, module: &str) -> FolderEntry {
    let path = dir.join(&r.name);
    let mut flags = flags_of(r.attrs, r.link, &r.name);
    let denied = protected(env, &path);
    if denied {
        flags |= ENTRY_DENIED;
    }
    // Ni un dossier protégé, ni un lien, ni un dossier en ligne seulement ne sont lus au travers ; un dossier protégé
    // n'est pas non plus dit connu du module.
    if !denied && flags & (ENTRY_LINK | ENTRY_CLOUD) == 0 && path.join(".git").exists() {
        flags |= ENTRY_REPO;
    }
    if !denied && known.has(&path) {
        flags |= ENTRY_KNOWN;
    }
    FolderEntry {
        name: r.name.clone(),
        path: path.to_string_lossy().into_owned(),
        flags,
        detail: detail(flags, module),
    }
}

/// Un dossier : une page de ses sous-dossiers, et ce que donnerait son choix.
fn folder(env: &Env, ask: &Ask, text: &str, known: &Known) -> Listing {
    let mut out = Listing {
        dir: text.to_string(),
        title: title_of(Path::new(text)),
        crumbs: vec![(ROOT.into(), String::new())],
        ..Default::default()
    };
    let dir = match check(env, text) {
        Ok(d) => d,
        Err(e) => {
            out.error = e;
            return out;
        }
    };
    out.dir = dir.to_string_lossy().into_owned();
    out.title = title_of(&dir);
    out.crumbs = crumbs(&out.dir);
    let level = match read_level(env, &dir) {
        Ok(l) => l,
        Err(e) => {
            out.error = e;
            return out;
        }
    };
    let filter: String = fold(ask.filter.trim()).chars().take(MAX_FILTER_LEN).collect();
    let mut dirs: Vec<(String, Raw)> = level
        .dirs
        .into_iter()
        .map(|r| (fold(&r.name), r))
        .filter(|(f, _)| filter.is_empty() || f.contains(&filter))
        .collect();
    if !ask.hidden {
        let before = dirs.len();
        dirs.retain(|(_, r)| flags_of(r.attrs, r.link, &r.name) & ENTRY_HIDDEN == 0);
        out.hidden = (before - dirs.len()) as u32;
    }
    dirs.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.name.cmp(&b.1.name)));
    let offset = ask.offset as usize;
    out.offset = ask.offset;
    out.total = dirs.len() as u32;
    out.entries = dirs.iter().skip(offset).take(PAGE).map(|(_, r)| entry(env, &dir, r, known, &ask.module)).collect();
    out.more = offset.saturating_add(out.entries.len()) < dirs.len();
    out.files = level.files;
    out.truncated = level.truncated;
    match rule(env, &dir, &ask.spec) {
        Ok((target, create)) => {
            let label = if ask.spec.want == FOLDER_REPO { "Choisir ce dépôt" } else { "Choisir ce dossier" };
            out.choose = Some(label.into());
            let shown = target.display();
            (out.target, out.target_tone) = if create {
                (format!("{shown} : sera créé"), Tone::Ok)
            } else if known.has(&target) {
                (format!("{shown} : déjà dans {}", ask.module), Tone::Info)
            } else {
                (shown.to_string(), Tone::Ok)
            };
        }
        Err(why) => out.why_not = why,
    }
    out
}

/// Les lectures d'un téléphone : une à la fois, dans un fil ; la demande la plus récente attend son tour (celle qu'elle
/// remplace reçoit une erreur) ; au plus [`BURST`] lectures lancées par [`BURST_WINDOW`].
#[derive(Default)]
pub struct Browser {
    /// La lecture en cours : vrai une fois finie.
    running: Option<Arc<AtomicBool>>,
    waiting: Option<Ask>,
    starts: VecDeque<Instant>,
}

impl Browser {
    /// Une demande arrive ; rend celle qui attendait et qu'elle remplace.
    pub fn ask(&mut self, ask: Ask) -> Option<Ask> {
        self.waiting.replace(ask)
    }

    pub fn waiting(&self) -> bool {
        self.waiting.is_some()
    }

    /// Le téléphone n'a plus le droit de parcourir : la demande qui attendait ne sera pas lue (rendue, pour lui
    /// répondre).
    pub fn cancel(&mut self) -> Option<Ask> {
        self.waiting.take()
    }

    /// Essais : une lecture en cours, finie quand le drapeau rendu passe à vrai.
    #[cfg(test)]
    pub fn hold(&mut self) -> Arc<AtomicBool> {
        let done = Arc::new(AtomicBool::new(false));
        self.running = Some(done.clone());
        done
    }

    /// La demande à lancer maintenant, s'il y en a une ; `Err(délai)` : la rafale est pleine, revenir après.
    fn next(&mut self, now: Instant) -> Result<Option<Ask>, Duration> {
        let busy = self.running.as_ref().is_some_and(|done| !done.load(Ordering::Acquire));
        if busy || self.waiting.is_none() {
            return Ok(None);
        }
        while self.starts.front().is_some_and(|t| now.duration_since(*t) >= BURST_WINDOW) {
            self.starts.pop_front();
        }
        if let Some(first) = self.starts.front().filter(|_| self.starts.len() >= BURST) {
            return Err(BURST_WINDOW.saturating_sub(now.duration_since(*first)));
        }
        self.starts.push_back(now);
        Ok(self.waiting.take())
    }

    /// Lance la demande qui attend si c'est son tour : le fil répond au téléphone par `to`, puis `wake` réveille
    /// l'interface (pour la demande suivante). Rend le délai avant de réessayer quand la rafale est pleine.
    pub fn pump(
        &mut self,
        env: &Arc<Env>,
        to: &mpsc::Sender<ToPhone>,
        wake: impl FnOnce() + Send + 'static,
    ) -> Option<Duration> {
        let ask = match self.next(Instant::now()) {
            Ok(Some(ask)) => ask,
            Ok(None) => return None,
            Err(wait) => return Some(wait),
        };
        let done = Arc::new(AtomicBool::new(false));
        let (env, to, flag) = (env.clone(), to.clone(), done.clone());
        let spawned = std::thread::Builder::new().name("mobile-dossiers".into()).spawn(move || {
            let listing = listing(&env, &ask);
            let _ = to.send(ToPhone::Folder { req: ask.req, listing });
            flag.store(true, Ordering::Release);
            wake();
        });
        self.running = spawned.is_ok().then_some(done);
        None
    }
}

#[cfg(windows)]
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

#[cfg(windows)]
fn drive_kind(letter: char) -> Drive {
    use windows_sys::Win32::Storage::FileSystem::GetDriveTypeW;
    // `DRIVE_…` de Windows.
    const REMOVABLE: u32 = 2;
    const FIXED: u32 = 3;
    const REMOTE: u32 = 4;
    if !letter.is_ascii_alphabetic() {
        return Drive::Other;
    }
    let root = wide(&format!(r"{letter}:\"));
    // SAFETY : chaîne terminée par zéro, vivante le temps de l'appel.
    match unsafe { GetDriveTypeW(root.as_ptr()) } {
        FIXED => Drive::Fixed,
        REMOVABLE => Drive::Removable,
        REMOTE => Drive::Remote,
        _ => Drive::Other,
    }
}

#[cfg(not(windows))]
fn drive_kind(_: char) -> Drive {
    Drive::Other
}

#[cfg(windows)]
fn fixed_drives() -> Vec<(char, String)> {
    use windows_sys::Win32::Storage::FileSystem::{GetLogicalDrives, GetVolumeInformationW};
    // SAFETY : sans argument.
    let mask = unsafe { GetLogicalDrives() };
    (0..26u8)
        .filter(|i| mask & (1 << i) != 0)
        .map(|i| (b'A' + i) as char)
        .filter(|letter| drive_kind(*letter) == Drive::Fixed)
        .map(|letter| {
            let root = wide(&format!(r"{letter}:\"));
            let mut name = [0u16; 261];
            // SAFETY : tampon de la taille annoncée ; les sorties facultatives sont nulles.
            let ok = unsafe {
                GetVolumeInformationW(
                    root.as_ptr(),
                    name.as_mut_ptr(),
                    name.len() as u32,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    0,
                )
            };
            let len = name.iter().position(|c| *c == 0).unwrap_or(0);
            let label = if ok != 0 { String::from_utf16_lossy(&name[..len]) } else { String::new() };
            (letter, label)
        })
        .collect()
}

#[cfg(not(windows))]
fn fixed_drives() -> Vec<(char, String)> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use suite_mobile::ui::Place;

    /// Z: est un lecteur réseau, E: une clé USB ; les autres, les vrais.
    fn fake_drive(letter: char) -> Drive {
        match letter {
            'Z' => Drive::Remote,
            'E' => Drive::Removable,
            l => drive_kind(l),
        }
    }

    fn fake_drives() -> Vec<(char, String)> {
        vec![('C', "Windows".into()), ('Z', "Partage".into())]
    }

    /// Un arbre d'essai : `zone` y est interdite, `moi` est le profil de l'utilisateur parmi ceux de `profils`. (Le
    /// dossier temporaire peut avoir un nom court : l'arbre est pris sous son nom résolu, comme le fait le PC.)
    fn env(root: &Path) -> Env {
        let root = long(root);
        Env {
            forbidden: with_resolved(vec![root.join("zone")]),
            profiles: Some((root.join("profils"), vec![root.join("profils").join("moi")])),
            quick: Vec::new(),
            drive: fake_drive,
            drives: fake_drives,
            max_entries: MAX_ENTRIES,
            max_time: MAX_TIME,
        }
    }

    fn long(p: &Path) -> PathBuf {
        resolved(p).unwrap()
    }

    fn mkdir(p: &Path) {
        std::fs::create_dir_all(p).unwrap();
    }

    /// Une jonction (sans droits d'administrateur, contrairement à un lien symbolique).
    fn junction(link: &Path, target: &Path) {
        let ok = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .stdout(std::process::Stdio::null())
            .status()
            .unwrap()
            .success();
        assert!(ok, "jonction {} → {}", link.display(), target.display());
    }

    fn ask(dir: &Path, spec: FolderInput) -> Ask {
        Ask {
            req: 1,
            dir: Some(dir.to_string_lossy().into_owned()),
            filter: String::new(),
            offset: 0,
            hidden: false,
            spec,
            module: "DragonTree".into(),
            recents: Vec::new(),
        }
    }

    #[test]
    fn a_path_from_the_phone_must_be_a_local_folder_outside_the_protected_zones() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        for d in ["libre", "zone/dedans", "profils/moi/projets", "profils/autre/secrets", "libre/.ssh"] {
            mkdir(&root.join(d));
        }
        std::fs::write(root.join("libre").join("fichier.txt"), "x").unwrap();
        junction(&root.join("libre").join("vers-zone"), &root.join("zone").join("dedans"));
        junction(&root.join("libre").join("vers-libre"), &root.join("profils").join("moi").join("projets"));
        let env = env(root);
        let at = |p: &str| root.join(p).to_string_lossy().into_owned();

        let refused = [
            (r"\\serveur\partage\depot".to_string(), "réseau"),
            ("//serveur/partage".into(), "réseau"),
            (r"\\?\C:\DEV".into(), "préfixes"),
            (r"\\.\C:".into(), "préfixes"),
            (r"\\?\UNC\serveur\partage".into(), "préfixes"),
            (r"C:\DEV\depot:flux".into(), "flux"),
            (r"C:\DEV\..\Windows".into(), "« .. »"),
            (r"C:\DEV\CON".into(), "réservé"),
            (r"C:\DEV\nul.txt".into(), "réservé"),
            (r"C:\DEV\com1".into(), "réservé"),
            (r"C:\DEV\depot.".into(), "point"),
            (r"C:\DEV\a*b".into(), "caractère"),
            ("C:DEV".into(), "disque du PC"),
            (r"DEV\depot".into(), "disque du PC"),
            (r"Z:\projets".into(), "réseau"),
            (r"E:\clé".into(), "amovible"),
            (at("zone"), "protégé"),
            (at(r"zone\dedans"), "protégé"),
            // La jonction a une forme permise : c'est sa cible, une fois résolue, qui est refusée.
            (at(r"libre\vers-zone"), "protégé"),
            (at(r"libre\.ssh"), "protégé"),
            (at(r"profils\autre\secrets"), "protégé"),
            (at(r"libre\absent"), "n'existe pas"),
            (at(r"libre\fichier.txt"), "pas un dossier"),
        ];
        for (path, why) in refused {
            let e = check(&env, &path).expect_err(&path);
            assert!(e.contains(why), "{path} : {e}");
        }

        assert_eq!(check(&env, &at("libre")).unwrap(), long(&root.join("libre")));
        assert_eq!(
            check(&env, &at("profils")).unwrap(),
            long(&root.join("profils")),
            "le dossier des profils se parcourt"
        );
        // Une jonction permise mène à sa cible : c'est elle que reçoit le module.
        assert_eq!(check(&env, &at(r"libre\vers-libre")).unwrap(), long(&root.join(r"profils\moi\projets")));
        // Les séparateurs `/` passent.
        assert_eq!(check(&env, &at("libre").replace('\\', "/")).unwrap(), long(&root.join("libre")));
    }

    /// Un lien qui mène hors des disques fixes (partage, lecteur réseau ou amovible) est refusé avant d'être suivi : la
    /// résolution attendrait un serveur absent, dans le fil de l'interface pour une action. Une jonction se suit ; une
    /// boucle de liens s'arrête.
    #[test]
    fn a_link_out_of_the_fixed_drives_is_refused_before_it_is_followed() {
        let tmp = tempfile::tempdir().unwrap();
        let root = long(tmp.path());
        let env = env(&root);
        let at = root.join("lien");
        for t in [r"\\?\UNC\serveur\partage", r"\\serveur\partage", "//serveur/partage", r"Z:\projets", r"\\?\Z:\x"] {
            assert!(link_target(&env, &at, Path::new(t)).unwrap_err().contains("réseau"), "{t}");
        }
        assert!(link_target(&env, &at, Path::new(r"E:\clé")).unwrap_err().contains("amovible"));
        assert_eq!(link_target(&env, &at, Path::new(r"\\?\C:\DEV")), Ok(Some(PathBuf::from(r"C:\DEV"))));
        assert_eq!(link_target(&env, &at, Path::new(r"..\voisin")), Ok(Some(root.join(r"..\voisin"))));
        let volume = r"\\?\Volume{0b1c4a3e-0000-0000-0000-100000000000}\";
        assert_eq!(link_target(&env, &at, Path::new(volume)), Ok(None));

        mkdir(&root.join("cible"));
        junction(&root.join("j1"), &root.join("cible"));
        assert_eq!(check(&env, &root.join("j1").to_string_lossy()), Ok(root.join("cible")));
        junction(&root.join("boucle-a"), &root.join("boucle-b"));
        junction(&root.join("boucle-b"), &root.join("boucle-a"));
        assert!(check(&env, &root.join("boucle-a").to_string_lossy()).unwrap_err().contains("trop de liens"));
        // Un vrai lien vers un partage, si ce compte peut en créer (mode développeur) : refusé sans attendre.
        let share = root.join("partage");
        if std::os::windows::fs::symlink_dir(r"\\192.0.2.1\partage", &share).is_ok() {
            let started = Instant::now();
            assert!(check(&env, &share.join("depot").to_string_lossy()).unwrap_err().contains("réseau"));
            assert!(started.elapsed() < Duration::from_secs(2));
        }
    }

    /// Le profil de l'utilisateur écrit sous son nom court (`C:\Users\CLBR-A~1\DEV`) est le sien : permis, rendu sous
    /// son nom long ; un autre profil, même écrit court, reste interdit.
    #[test]
    fn the_short_name_of_the_users_own_profile_is_allowed() {
        let tmp = tempfile::tempdir().unwrap();
        let root = long(tmp.path());
        let (mine, other) = (root.join("profils").join("utilisateur-du-pc"), root.join("profils").join("autre-compte"));
        mkdir(&mine.join("projets"));
        mkdir(&other.join("secrets"));
        let short = |p: &Path| short_name(p).and_then(|s| s.file_name().map(|n| p.with_file_name(n)));
        let (Some(mine_short), Some(other_short)) = (short(&mine), short(&other)) else { return };
        if key(&mine_short) == key(&mine) {
            eprintln!("pas de noms courts sur ce volume : essai sauté");
            return;
        }
        let env = Env { profiles: Some((root.join("profils"), forms(&mine))), ..env(&root) };
        let projects = mine_short.join("projets");
        assert_eq!(check(&env, &projects.to_string_lossy()), Ok(mine.join("projets")));
        let secrets = other_short.join("secrets");
        assert!(check(&env, &secrets.to_string_lossy()).unwrap_err().contains("protégé"));
        assert!(check(&env, &other.join("secrets").to_string_lossy()).unwrap_err().contains("protégé"));
    }

    #[test]
    fn the_real_pc_protects_windows_and_the_application_data() {
        let env = Env::system(&[]);
        assert_eq!((env.drive)('C'), Drive::Fixed);
        assert!(check(&env, r"C:\").is_ok());
        let windows = std::env::var("SystemRoot").unwrap();
        assert!(check(&env, &windows).unwrap_err().contains("protégé"));
        let local = std::env::var("LOCALAPPDATA").unwrap();
        assert!(check(&env, &local).unwrap_err().contains("protégé"));
        // Le dossier temporaire de l'utilisateur, même sous son nom court.
        assert!(check(&env, &std::env::temp_dir().to_string_lossy()).unwrap_err().contains("protégé"));
        // Son profil, sous son nom court s'il en a un (`C:\Users\CLBR-A~1`).
        let home = dirs::home_dir().unwrap();
        for form in forms(&home) {
            assert_eq!(check(&env, &form.to_string_lossy()), Ok(resolved_or(&home)), "{}", form.display());
        }
    }

    #[test]
    fn the_choice_follows_what_the_input_wants() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        for d in ["depot/.git", "simple", "parent/plein/x", "parent/vide"] {
            mkdir(&root.join(d));
        }
        let env = env(root);
        let at = |p: &str| root.join(p).to_string_lossy().into_owned();
        let repo = FolderInput { want: FOLDER_REPO, ..Default::default() };
        assert_eq!(chosen(&env, Some(&at("depot")), &repo).unwrap(), long(&root.join("depot")));
        assert!(chosen(&env, Some(&at("simple")), &repo).unwrap_err().contains("pas un dépôt git"));
        assert!(chosen(&env, None, &repo).unwrap_err().contains("Choisissez"));
        let any = FolderInput::default();
        assert!(chosen(&env, Some(&format!("  {}  ", at("simple"))), &any).is_ok(), "les espaces autour sont retirés");
        assert!(chosen(&env, Some(r"\\serveur\partage"), &any).is_err());

        let parent = |child: &str| FolderInput { want: FOLDER_PARENT, child: child.into(), ..Default::default() };
        assert!(chosen(&env, Some(&at("parent")), &parent("nouveau")).is_ok());
        assert!(chosen(&env, Some(&at("parent")), &parent("vide")).is_ok(), "un dossier vide sert de destination");
        assert!(chosen(&env, Some(&at("parent")), &parent("plein")).unwrap_err().contains("pas vide"));
        for bad in ["..", r"a\b", "a/b", "CON", "x:y", "fin."] {
            assert!(chosen(&env, Some(&at("parent")), &parent(bad)).is_err(), "{bad}");
        }
        assert!(chosen(&env, Some(&at("")), &parent("zone")).unwrap_err().contains("protégé"));
        for name in [".aws", ".KUBE", ".password-store"] {
            assert!(chosen(&env, Some(&at("parent")), &parent(name)).unwrap_err().contains("protégé"), "{name}");
        }
        // Un dossier protégé vide, écrit sous son nom court (`AWS~1`) : la destination est contrôlée résolue.
        mkdir(&root.join(r"parent\.aws"));
        let aws = long(&root.join(r"parent\.aws"));
        if let Some(short) = short_name(&aws).and_then(|s| s.file_name().map(|n| n.to_string_lossy().into_owned()))
            && short != ".aws"
        {
            assert!(chosen(&env, Some(&at("parent")), &parent(&short)).unwrap_err().contains("protégé"), "{short}");
        }
        let unknown = FolderInput { want: 9, ..Default::default() };
        assert!(chosen(&env, Some(&at("simple")), &unknown).is_err());
    }

    #[test]
    fn a_folder_is_listed_sorted_by_pages_with_its_flags() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        for i in 0..450 {
            mkdir(&root.join(format!("Dossier {i:03}")));
        }
        for d in ["depot/.git", "connu", "zone", "Été", ".cache", "cache-h"] {
            mkdir(&root.join(d));
        }
        for f in ["a.txt", "b.txt", "c.txt"] {
            std::fs::write(root.join(f), "x").unwrap();
        }
        let hidden =
            std::process::Command::new("attrib").arg("+h").arg(root.join("cache-h")).status().unwrap().success();
        assert!(hidden);
        junction(&root.join("lien"), &root.join("depot"));
        let env = env(root);
        let spec = FolderInput {
            want: FOLDER_REPO,
            known: vec![root.join("connu").to_string_lossy().into_owned()],
            ..Default::default()
        };

        let first = listing(&env, &ask(root, spec.clone()));
        assert_eq!(first.error, "");
        assert_eq!(first.dir, long(root).to_string_lossy());
        assert_eq!(first.total, 455, "450 + depot, connu, zone, Été, lien ; ni les cachés ni les fichiers");
        assert_eq!((first.entries.len(), first.more, first.hidden, first.files), (PAGE, true, 2, 3));
        let names: Vec<String> = first.entries.iter().map(|e| fold(&e.name)).collect();
        assert!(names.windows(2).all(|w| w[0] <= w[1]), "trié sans casse : {names:?}");
        assert_eq!(first.crumbs.first().map(|c| c.0.as_str()), Some(ROOT));
        assert_eq!(first.crumbs.last().map(|c| c.1.as_str()), Some(first.dir.as_str()));
        // L'arbre n'est pas un dépôt : il ne se choisit pas.
        assert_eq!(first.choose, None);
        assert!(first.why_not.contains("pas un dépôt git"));

        let flags = |l: &Listing, name: &str| l.entries.iter().find(|e| e.name == name).map(|e| e.flags).unwrap();
        assert_eq!(flags(&first, "depot"), ENTRY_REPO);
        assert_eq!(flags(&first, "connu"), ENTRY_KNOWN);

        let last = listing(&env, &Ask { offset: 400, ..ask(root, spec.clone()) });
        assert_eq!((last.entries.len(), last.more, last.offset), (55, false, 400));
        assert_eq!(flags(&last, "zone"), ENTRY_DENIED);
        // Une jonction n'est pas suivie : rien n'est lu au travers (le dépôt où elle mène n'est pas dit).
        assert_eq!(flags(&last, "lien"), ENTRY_LINK);
        assert!(last.entries.iter().any(|e| e.name == "Été"));

        let filtered = listing(&env, &Ask { filter: "ETE".into(), ..ask(root, spec.clone()) });
        assert_eq!(
            filtered.entries.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(),
            ["Été"],
            "sans casse ni accents"
        );
        let all = listing(&env, &Ask { hidden: true, ..ask(root, spec.clone()) });
        assert_eq!((all.total, all.hidden), (457, 0));
        let cached = listing(&env, &Ask { hidden: true, filter: "cache".into(), ..ask(root, spec.clone()) });
        assert!(cached.entries.iter().all(|e| e.flags & ENTRY_HIDDEN != 0) && cached.entries.len() == 2);

        let repo = listing(&env, &ask(&root.join("depot"), spec.clone()));
        assert_eq!(repo.choose.as_deref(), Some("Choisir ce dépôt"));
        assert_eq!(repo.target_tone, Tone::Ok);
        let known = listing(&env, &ask(&root.join("connu"), FolderInput { want: FOLDER_ANY, ..spec.clone() }));
        assert!(known.target.ends_with("déjà dans DragonTree"), "{}", known.target);
        let clone = FolderInput { want: FOLDER_PARENT, child: "Ozmo".into(), ..Default::default() };
        let here = listing(&env, &ask(root, clone));
        assert!(here.target.ends_with(r"\Ozmo : sera créé"), "{}", here.target);

        let small = Env { max_entries: 100, ..env };
        assert!(listing(&small, &ask(root, spec.clone())).truncated);
        let refused = listing(&small, &ask(&root.join("zone"), spec));
        assert!(refused.error.contains("protégé") && refused.entries.is_empty());
    }

    /// Un dossier protégé se montre refusé, sans rien lire dedans : ni s'il est un dépôt (le profil d'un autre compte,
    /// `ProgramData`), ni s'il est connu du module. Son voisin, lui, se lit.
    #[test]
    fn a_protected_entry_is_not_probed() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        for d in ["zone/.git", "depot/.git"] {
            mkdir(&root.join(d));
        }
        let env = Env::for_tests(vec![root.join("zone")]);
        let spec = FolderInput { known: vec![root.join("zone").to_string_lossy().into_owned()], ..Default::default() };
        let l = listing(&env, &ask(root, spec));
        assert_eq!(l.error, "");
        let entry = |name: &str| l.entries.iter().find(|e| e.name == name).unwrap();
        assert_eq!((entry("zone").flags, entry("zone").detail.as_str()), (ENTRY_DENIED, "protégé"));
        assert_eq!(entry("depot").flags, ENTRY_REPO);
    }

    #[test]
    fn a_cloud_folder_and_a_link_are_told_apart_by_their_attributes() {
        assert_eq!(flags_of(DIRECTORY | RECALL_ON_OPEN, false, "Photos"), ENTRY_CLOUD);
        assert_eq!(flags_of(DIRECTORY | OFFLINE, false, "Archives"), ENTRY_CLOUD);
        assert_eq!(flags_of(DIRECTORY | RECALL_ON_OPEN, true, "Lien"), ENTRY_LINK);
        assert_eq!(flags_of(DIRECTORY | SYSTEM, false, "Recovery"), ENTRY_HIDDEN);
        assert_eq!(flags_of(DIRECTORY, false, ".vscode"), ENTRY_HIDDEN);
        assert_eq!(flags_of(DIRECTORY, false, "DEV"), 0);
    }

    #[test]
    fn the_root_has_the_module_places_then_quick_access_then_fixed_drives() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        for d in ["dev/clones", "dev/.ssh", "bureau", "dev/Ozmo/.git", "dev/Survey"] {
            mkdir(&root.join(d));
        }
        let mut env = env(root);
        env.quick = vec![("Bureau".into(), root.join("bureau")), ("Absent".into(), root.join("absent"))];
        let at = |p: &str| root.join(p).to_string_lossy().into_owned();
        let spec = FolderInput {
            places: vec![
                Place { label: "Dossier de clonage".into(), path: at(r"dev\clones"), glyph: String::new() },
                Place { label: "Interdit".into(), path: at(r"dev\.ssh"), glyph: String::new() },
            ],
            known: vec![at(r"dev\Ozmo"), at(r"dev\Survey")],
            ..Default::default()
        };
        let l = listing(&env, &Ask { dir: None, ..ask(root, spec.clone()) });
        assert_eq!((l.title.as_str(), l.dir.as_str(), l.choose.as_deref()), (ROOT, "", None));
        let titles: Vec<&str> = l.groups.iter().map(|g| g.title.as_str()).collect();
        assert_eq!(titles, ["DragonTree", "Accès rapide", "Lecteurs"], "pas de groupe vide");
        let names = |i: usize| l.groups[i].places.iter().map(|p| p.name.clone()).collect::<Vec<_>>();
        assert_eq!(names(0), ["Dossier de clonage"], "un raccourci interdit n'est pas montré");
        assert_eq!(l.groups[0].places[0].flags, ENTRY_KNOWN);
        // L'accès rapide, puis le dossier où le module a ses dépôts ; un dossier absent n'est pas montré.
        assert_eq!(names(1), ["Bureau", "dev"]);
        assert_eq!(l.groups[1].places[1].path, long(&root.join("dev")).to_string_lossy());
        assert_eq!(names(2), ["Windows (C:)"], "Z: est un lecteur réseau");
        assert_eq!((l.groups[2].places[0].path.as_str(), l.groups[2].places[0].flags), (r"C:\", ENTRY_DRIVE));

        // Les dossiers choisis récemment en tête ; un récent devenu interdit ou absent n'est pas montré, et un
        // dossier n'apparaît qu'une fois.
        let recents = vec![at(r"dev\Survey"), at(r"dev\.ssh"), at("disparu"), at("bureau")];
        let l = listing(&env, &Ask { dir: None, recents, ..ask(root, spec) });
        let titles: Vec<&str> = l.groups.iter().map(|g| g.title.as_str()).collect();
        assert_eq!(titles, ["Récents", "DragonTree", "Accès rapide", "Lecteurs"]);
        let names = |i: usize| l.groups[i].places.iter().map(|p| p.name.clone()).collect::<Vec<_>>();
        assert_eq!(names(0), ["Survey", "bureau"]);
        assert_eq!(l.groups[0].places[0].flags, ENTRY_KNOWN);
        assert_eq!(names(2), ["dev"], "le Bureau est déjà dans les récents");
    }

    #[test]
    fn one_read_at_a_time_the_latest_waits_and_bursts_are_limited() {
        let spec = FolderInput::default();
        let asked = |req: u32| Ask { req, ..ask(Path::new(r"C:\"), spec.clone()) };
        let mut b = Browser::default();
        let now = Instant::now();
        // Une lecture en cours : la demande suivante attend, la plus récente remplace celle qui attendait.
        let running = Arc::new(AtomicBool::new(false));
        b.running = Some(running.clone());
        assert!(b.ask(asked(1)).is_none());
        assert_eq!(b.ask(asked(2)).map(|a| a.req), Some(1));
        assert!(matches!(b.next(now), Ok(None)));
        running.store(true, Ordering::Release);
        assert_eq!(b.next(now).unwrap().map(|a| a.req), Some(2));
        b.running = None;
        // La rafale : BURST départs, puis il faut attendre que le premier sorte de la fenêtre.
        for i in 1..BURST as u32 {
            b.ask(asked(10 + i));
            assert!(matches!(b.next(now), Ok(Some(_))));
        }
        b.ask(asked(99));
        assert_eq!(b.next(now + Duration::from_millis(500)).unwrap_err(), BURST_WINDOW - Duration::from_millis(500));
        assert_eq!(b.next(now + BURST_WINDOW).unwrap().map(|a| a.req), Some(99));
        // Le droit de parcourir retiré : la demande qui attend est rendue, pour être refusée ; plus rien n'attend.
        b.ask(asked(100));
        assert_eq!(b.cancel().map(|a| a.req), Some(100));
        assert!(!b.waiting() && b.cancel().is_none());

        // Pour de vrai : la réponse part du fil, avec le bon numéro.
        let env = Arc::new(Env::for_tests(Vec::new()));
        let (to, from) = mpsc::channel();
        let mut b = Browser::default();
        b.ask(Ask { dir: None, ..asked(7) });
        assert_eq!(b.pump(&env, &to, || {}), None);
        match from.recv_timeout(Duration::from_secs(20)).unwrap() {
            ToPhone::Folder { req, listing } => {
                assert_eq!((req, listing.title.as_str()), (7, ROOT));
                assert!(listing.groups.iter().any(|g| g.places.iter().any(|p| p.path == r"C:\")));
            }
            other => panic!("{other:?}"),
        }
    }
}
