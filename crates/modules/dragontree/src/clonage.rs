//! Cloner un dépôt depuis le téléphone, par DragonTree (`clone_remote` : sans fenêtre, comme « Cloner » depuis une
//! URL).
//!
//! « Cloner un dépôt… » (la liste) ouvre un brouillon à lui, `cloner/<n>` : deux téléphones (ou deux essais) ne se
//! mêlent pas. Il se remplit pas à pas sur le PC : l'URL, le dossier parent choisi dans le sélecteur de dossiers du
//! téléphone (`FOLDER_PARENT` ; d'abord celui que propose DragonTree), le nom du dossier (déduit de l'URL tant qu'il
//! n'a pas été saisi), la branche, le compte (celui que DragonTree présélectionne d'après l'URL, ou un autre qui lui
//! correspond, ou aucun), les sous-modules ; puis « Cloner ». Chaque saisie est contrôlée ici (le pont a déjà revérifié
//! le dossier choisi), et DragonTree contrôle la demande à son tour.
//!
//! DragonTree mène le clonage : le compte authentifie git et reste celui du dépôt (sa règle : chaque dépôt garde le
//! compte de son remote), la progression et « Annuler » viennent de lui, ses notifications s'affichent sur le PC. Fini,
//! le dépôt est ouvert dans un onglet de fond, en tête des récents ; le brouillon mène à lui.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use dragontree_app::clone::{CloneRequest, HostJob, folder_from_url};
use dragontree_forges::{Account, ForgeKind};
use dragontree_git::Progress;
use suite_sdk::mobile::ui::{Act, Block, FolderInput, PROGRESS_UNKNOWN, Row, Screen, Tone};

use crate::mobile::{rounded, short_error};

/// Un brouillon que le téléphone ne regarde plus est oublié au bout d'une heure, sauf pendant son clonage ; 16 au plus
/// (les plus anciens d'abord).
pub const DRAFT_KEPT: Duration = Duration::from_secs(3600);
pub const DRAFTS_MAX: usize = 16;

/// « Cloner un dépôt… » : un brouillon neuf (`@ouvrir cloner/<n>`).
pub fn clone_act() -> Act {
    Act::new("cloner", "Cloner un dépôt…").glyph("📥")
}

/// Le glyphe d'une forge (un compte, une pull request).
pub fn forge_glyph(kind: ForgeKind) -> &'static str {
    match kind {
        ForgeKind::GitHub => "🐙",
        ForgeKind::GitLab => "🦊",
        ForgeKind::AzureDevOps => "🔷",
    }
}

/// Une URL de dépôt saisie sur le téléphone : `https://`, `http://`, `ssh://`, `git://`, ou `utilisateur@hôte:chemin`.
/// Jamais un chemin du PC, `file://` ni un transport de git (`ext::…`) : cloner un dépôt du PC le copierait hors des
/// dossiers protégés ; jamais un hôte qui passerait pour une option de ssh (`-o…`).
pub fn url_ok(url: &str) -> Result<(), String> {
    let refused = || {
        Err(format!(
            "URL refusée : « {url} ». Une adresse https://…, ssh://… ou git@hôte:chemin est attendue (pas de chemin du \
             PC)."
        ))
    };
    if url.is_empty()
        || url.len() > 2048
        || url.starts_with('-')
        || url.chars().any(|c| c.is_whitespace() || c.is_control())
    {
        return refused();
    }
    let lower = url.to_ascii_lowercase();
    if let Some((scheme, rest)) = lower.split_once("://") {
        let host = rest.split(['/', '?', '#']).next().unwrap_or_default();
        let host = host.rsplit('@').next().unwrap_or_default();
        let host_ok = !host.is_empty() && !host.starts_with('-');
        return if matches!(scheme, "https" | "http" | "ssh" | "git") && host_ok { Ok(()) } else { refused() };
    }
    // `utilisateur@hôte:chemin` : un hôte d'au moins deux lettres (`C:\…` est un chemin du PC), sans `::` (transport).
    match url.split_once(':') {
        Some((host, path))
            if !url.contains("::")
                && !path.is_empty()
                && !host.contains(['/', '\\'])
                && host.rsplit('@').next().is_some_and(|h| h.len() >= 2 && !h.starts_with('-')) =>
        {
            Ok(())
        }
        _ => refused(),
    }
}

/// Le nom du dossier du dépôt : un seul nom, que Windows accepte (comme le pont pour le dossier qu'un clonage crée).
pub fn folder_name_ok(name: &str) -> Result<(), String> {
    let refused = || Err(format!("Nom de dossier refusé : « {name} »."));
    let single =
        !name.is_empty() && name.len() <= 255 && !name.contains(['\\', '/', ':']) && name != "." && name != "..";
    let clean = !name.chars().any(|c| c.is_control() || matches!(c, '*' | '?' | '"' | '<' | '>' | '|'));
    if !single || !clean || name.ends_with(['.', ' ']) {
        return refused();
    }
    let stem = name.split('.').next().unwrap_or_default().trim_end().to_ascii_uppercase();
    let numbered = |p: &str| {
        stem.strip_prefix(p).is_some_and(|n| matches!(n.as_bytes(), [b'1'..=b'9']) || matches!(n, "¹" | "²" | "³"))
    };
    if matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$")
        || numbered("COM")
        || numbered("LPT")
    {
        return refused();
    }
    Ok(())
}

/// Une branche à extraire : un nom que git accepte (ni espace, ni `~^:?*[\`, ni `..`), qui ne peut pas passer pour une
/// option.
pub fn branch_ok(name: &str) -> Result<(), String> {
    let bad = name.starts_with(['-', '/'])
        || name.ends_with(['/', '.'])
        || name.ends_with(".lock")
        || name.contains("..")
        || name.contains("@{")
        || name.len() > 255
        || name.chars().any(|c| c.is_whitespace() || c.is_control() || "~^:?*[\\".contains(c));
    if bad { Err(format!("Nom de branche refusé : « {name} ».")) } else { Ok(()) }
}

/// Un brouillon de clonage, rempli depuis le téléphone.
#[derive(Clone, Debug)]
pub struct Draft {
    pub url: String,
    /// Le dossier parent : celui que propose DragonTree, puis celui que le téléphone a choisi.
    pub parent: PathBuf,
    pub folder: String,
    /// Le nom du dossier a été saisi : une autre URL ne le change plus.
    folder_touched: bool,
    /// Vide : la branche par défaut du dépôt.
    pub branch: String,
    pub account: Option<String>,
    /// L'URL pour laquelle le compte a été présélectionné : un compte choisi à la main tient tant qu'elle ne change pas.
    account_for: String,
    pub recurse: bool,
    /// La destination contrôlée : le dossier parent résolu, ou ce qui ne va pas ; `None` : le nom est à saisir.
    pub target: Option<Result<PathBuf, String>>,
    /// Le clonage lancé (son numéro dans DragonTree).
    pub job: Option<u64>,
    /// Quand le téléphone l'a regardé pour la dernière fois.
    pub seen: Instant,
}

impl Draft {
    /// Un brouillon vide, dans le dossier parent `parent` (celui que propose DragonTree).
    pub fn new(parent: PathBuf) -> Draft {
        Draft {
            url: String::new(),
            parent,
            folder: String::new(),
            folder_touched: false,
            branch: String::new(),
            account: None,
            account_for: String::new(),
            recurse: false,
            target: None,
            job: None,
            seen: Instant::now(),
        }
    }

    /// Une URL saisie : le nom du dossier en est déduit tant qu'il n'a pas été saisi, et le compte est celui que
    /// DragonTree présélectionne pour elle (`preselect`) tant qu'un autre n'a pas été choisi pour cette URL.
    pub fn set_url(&mut self, url: &str, preselect: impl FnOnce(&str) -> Option<String>) -> Result<(), String> {
        let url = url.trim();
        url_ok(url)?;
        self.url = url.to_string();
        if !self.folder_touched {
            let name = folder_from_url(url);
            self.folder = if folder_name_ok(&name).is_ok() { name } else { String::new() };
        }
        if self.account_for != self.url {
            self.account_for = self.url.clone();
            self.account = preselect(url);
        }
        Ok(())
    }

    /// Le nom du dossier saisi.
    pub fn set_folder(&mut self, name: &str) -> Result<(), String> {
        let name = name.trim();
        folder_name_ok(name)?;
        self.folder = name.to_string();
        self.folder_touched = true;
        Ok(())
    }

    /// La branche saisie ; vide : celle par défaut.
    pub fn set_branch(&mut self, name: &str) -> Result<(), String> {
        let name = name.trim();
        if !name.is_empty() {
            branch_ok(name)?;
        }
        self.branch = name.to_string();
        Ok(())
    }

    /// Le compte choisi (`None` : les identifiants de git) : un compte de DragonTree qui correspond à l'URL, comme dans
    /// sa fenêtre.
    pub fn set_account(&mut self, id: Option<&str>, accounts: &[Account]) -> Result<(), String> {
        if let Some(id) = id
            && !matching(accounts, &self.url).iter().any(|a| a.id == id)
        {
            return Err("Ce compte ne correspond pas à l'URL du dépôt.".into());
        }
        self.account = id.map(str::to_string);
        self.account_for = self.url.clone();
        Ok(())
    }

    /// Ce qui manque encore pour cloner (vide : prêt).
    pub fn missing(&self) -> Vec<&'static str> {
        let mut missing = Vec::new();
        if self.url.is_empty() {
            missing.push("l'URL du dépôt");
        }
        if self.folder.is_empty() {
            missing.push("le nom du dossier");
        }
        missing
    }

    /// La demande confiée à DragonTree, une fois la destination contrôlée (le dossier parent résolu).
    pub fn request(&self) -> Result<CloneRequest, String> {
        if let Some(what) = self.missing().first() {
            return Err(format!("Il manque {what}."));
        }
        let parent = match &self.target {
            Some(Ok(parent)) => parent.clone(),
            Some(Err(e)) => return Err(e.clone()),
            None => return Err("Il manque le nom du dossier.".into()),
        };
        Ok(CloneRequest {
            url: self.url.clone(),
            parent,
            folder: self.folder.clone(),
            branch: (!self.branch.is_empty()).then(|| self.branch.clone()),
            account: self.account.clone(),
            recurse: self.recurse,
        })
    }
}

/// Les comptes qui correspondent à `url` (hôte, organisation Azure DevOps), dans l'ordre des réglages.
pub fn matching<'a>(accounts: &'a [Account], url: &str) -> Vec<&'a Account> {
    accounts.iter().filter(|a| !url.is_empty() && dragontree_forges::account_matches_url(a, url)).collect()
}

/// Un clonage de DragonTree (`host_jobs`), tel que le téléphone le montre.
#[derive(Clone, Debug, PartialEq)]
pub struct JobView {
    pub id: u64,
    /// « Clonage de <dossier> ».
    pub title: String,
    pub dest: PathBuf,
    pub progress: Option<Progress>,
    /// `None` : en cours.
    pub result: Option<Result<(), String>>,
}

impl From<&HostJob> for JobView {
    fn from(j: &HostJob) -> JobView {
        JobView {
            id: j.id,
            title: j.title.clone(),
            dest: j.dest.clone(),
            progress: j.progress.clone(),
            result: j.result.clone(),
        }
    }
}

/// « Annuler » d'un clonage en cours.
fn cancel_act(j: &JobView) -> Act {
    let name = j.dest.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    Act::new(format!("annuler-clone:{}", j.id), "Annuler le clonage")
        .glyph("⏹️")
        .danger()
        .confirm(format!("Annuler le clonage de {name} ? Ce qui a été copié est supprimé."))
}

/// Un clonage de DragonTree : en cours, avec sa progression (arrondie à 5 %) et « Annuler » ; ou ce qu'il a donné,
/// avec `open` (l'écran du dépôt cloné, une fois dans la liste) pour y aller.
pub fn job_block(j: &JobView, open: Option<&str>) -> Block {
    match &j.result {
        None => {
            let (text, percent) = match &j.progress {
                Some(p) => (format!("{} : {}…", j.title, p.phase), rounded(p.percent)),
                None => (format!("{} : connexion…", j.title), None),
            };
            let progress = Some(percent.unwrap_or(PROGRESS_UNKNOWN));
            Block::Note { text, tone: Tone::Info, progress, acts: vec![cancel_act(j)] }
        }
        Some(Ok(())) => {
            let acts = open.map(|o| Act::screen("Ouvrir le dépôt", o).glyph("📂")).into_iter().collect();
            let text = format!("Dépôt cloné dans {}", j.dest.display());
            Block::Note { text, tone: Tone::Ok, progress: None, acts }
        }
        Some(Err(e)) => {
            let text = format!("{} : {}", j.title, short_error(e));
            let tone = if e.starts_with("Clonage annulé") { Tone::Warn } else { Tone::Danger };
            Block::Note { text, tone, progress: None, acts: Vec::new() }
        }
    }
}

/// Ce que DragonTree sait d'un brouillon, pour son écran.
pub struct DraftView<'a> {
    /// Les comptes de DragonTree.
    pub accounts: &'a [Account],
    /// Le compte que DragonTree présélectionne pour l'URL du brouillon.
    pub preselected: Option<String>,
    /// Le clonage du brouillon, tant que DragonTree le suit.
    pub job: Option<&'a JobView>,
    /// L'écran du dépôt cloné, une fois dans la liste.
    pub open: Option<String>,
    /// Le sélecteur du dossier parent (raccourcis, dépôts connus).
    pub folders: FolderInput,
}

/// L'écran d'un brouillon : ce qui est saisi (chaque ligne se touche pour le changer), puis « Cloner » ; pendant le
/// clonage, sa progression et « Annuler » ; fini, le dépôt cloné ; échoué ou annulé, le brouillon de nouveau, pour
/// réessayer.
pub fn screen(d: &Draft, v: &DraftView) -> Screen {
    let mut s = Screen::new("Cloner un dépôt").subtitle("Par DragonTree, sur le PC");
    let job = v.job;
    if let Some(j) = job {
        s.blocks.push(job_block(j, v.open.as_deref()));
    }
    let done = job.is_some_and(|j| j.result == Some(Ok(())));
    if done {
        let mut acts: Vec<Act> =
            v.open.iter().map(|o| Act::screen("Ouvrir le dépôt", o).glyph("📂").primary()).collect();
        acts.push(Act::new("cloner", "Cloner un autre dépôt").glyph("📥"));
        s.buttons(acts);
    }
    // Pendant le clonage et après sa réussite, le brouillon se lit sans se changer.
    let edit = job.is_none_or(|j| matches!(j.result, Some(Err(_))));
    let tap = |row: Row, act: Act| if edit { row.tap(act) } else { row };

    s.section("Dépôt", "");
    let url = if d.url.is_empty() { "à saisir (HTTPS ou SSH)".to_string() } else { crate::depot::safe_url(&d.url) };
    s.row(tap(
        Row::new("URL du dépôt").subtitle(url).glyph("🔗").key("url"),
        Act::new("url", "URL du dépôt…")
            .input("URL du dépôt (HTTPS ou SSH)", "https://github.com/organisation/depot.git")
            .input_value(&d.url),
    ));
    let branch = if d.branch.is_empty() { "celle par défaut du dépôt".to_string() } else { d.branch.clone() };
    s.row(tap(
        Row::new("Branche").subtitle(branch).glyph("🌿").key("branche"),
        Act::new("branche", "Branche…")
            .input("Branche à extraire (vide : celle par défaut)", "main")
            .input_value(&d.branch),
    ));

    s.section("Destination", "");
    let mut folders = v.folders.clone();
    folders.start = d.parent.display().to_string();
    folders.child = d.folder.clone();
    s.row(tap(
        Row::new("Dossier parent").subtitle(d.parent.display().to_string()).glyph("📁").key("dossier"),
        Act::new("dossier", "Choisir le dossier parent…")
            .input("Dossier où cloner", r"C:\…\dossier")
            .input_value(d.parent.display().to_string())
            .folder("Dossier où cloner", folders),
    ));
    let folder = if d.folder.is_empty() { "à saisir".to_string() } else { d.folder.clone() };
    s.row(tap(
        Row::new("Nom du dossier").subtitle(folder).glyph("✏️").key("nom"),
        Act::new("nom", "Nom du dossier…").input("Nom du dossier du dépôt", "mon-depot").input_value(&d.folder),
    ));
    if !d.folder.is_empty() {
        let dest = d.parent.join(&d.folder).display().to_string();
        let row = Row::new("Destination").subtitle(dest).glyph("📂").key("destination");
        s.row(match &d.target {
            Some(Err(e)) => row.detail("refusée").tone(Tone::Danger).subtitle(e.clone()),
            _ => row,
        });
    }

    s.section("Compte", "");
    if d.url.is_empty() {
        s.empty("Saisissez d'abord l'URL : DragonTree propose le compte qui lui correspond.");
    } else {
        let matching = matching(v.accounts, &d.url);
        for a in &matching {
            let chosen = d.account.as_deref() == Some(a.id.as_str());
            let mut row = Row::new(a.title()).glyph(forge_glyph(a.kind)).key(format!("compte:{}", a.id));
            if v.preselected.as_deref() == Some(a.id.as_str()) {
                row = row.subtitle("proposé par DragonTree pour cette URL");
            }
            s.row(if chosen {
                row.detail("✓ choisi").tone(Tone::Ok)
            } else {
                tap(row, Act::new(format!("compte:{}", a.id), "Utiliser ce compte"))
            });
        }
        let none = Row::new("Aucun : identifiants de git").glyph("🔑").key("compte:");
        let none = if matching.is_empty() {
            none.subtitle("Aucun compte de DragonTree ne correspond à cette URL.")
        } else {
            none.subtitle("Git demande ses identifiants habituels (sur le PC).")
        };
        s.row(if d.account.is_none() {
            none.detail("✓ choisi").tone(Tone::Ok)
        } else {
            tap(none, Act::new("compte:", "N'utiliser aucun compte"))
        });
    }

    s.section("Options", "");
    let (recurse, toggle) =
        if d.recurse { ("oui", "Ne pas cloner les sous-modules") } else { ("non", "Cloner aussi les sous-modules") };
    s.row(tap(
        Row::new("Sous-modules")
            .subtitle("Leurs URL sont choisies par le dépôt cloné : à cocher en connaissance de cause.")
            .detail(recurse)
            .glyph("🧩")
            .key("sous-modules"),
        Act::new("sous-modules", toggle),
    ));

    if edit {
        let missing = d.missing();
        if !missing.is_empty() {
            s.note(format!("À compléter : {}.", missing.join(", ")), Tone::Info);
        } else if matches!(d.target, Some(Ok(_))) {
            s.buttons(vec![Act::new("lancer", "Cloner").glyph("📥").primary()]);
        }
    }
    s
}

/// L'écran d'un brouillon que le PC n'a plus (Eyrie a redémarré, ou il est resté une heure sans être regardé).
pub fn lost() -> Screen {
    let mut s = Screen::new("Cloner un dépôt");
    s.note(
        "Ce brouillon n'est plus sur le PC (Eyrie a redémarré, ou il est resté une heure sans être regardé).",
        Tone::Warn,
    );
    s.buttons(vec![Act::new("cloner", "Recommencer").glyph("📥").primary()]);
    s
}

/// Le numéro du brouillon de l'écran `path` (`cloner/<n>`).
pub fn draft_of(path: &str) -> Option<u64> {
    path.strip_prefix("cloner/")?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn github(id: &str) -> Account {
        Account {
            id: id.into(),
            kind: ForgeKind::GitHub,
            host: "github.com".into(),
            username: id.into(),
            display_name: String::new(),
            orgs: Vec::new(),
            avatar_url: None,
        }
    }

    /// Ce que le téléphone saisit est contrôlé sur le PC : une adresse de dépôt (jamais un chemin du PC, `file://` ou
    /// un transport de git), un simple nom de dossier que Windows accepte, une branche que git accepte.
    #[test]
    fn what_the_phone_types_is_checked() {
        for good in [
            "https://github.com/ada/depot.git",
            "HTTP://serveur.local/x",
            "ssh://git@github.com/ada/depot.git",
            "git://exemple.org/x.git",
            "git@github.com:ada/depot.git",
            "github.com:ada/depot",
        ] {
            assert!(url_ok(good).is_ok(), "{good}");
        }
        for bad in [
            "",
            r"C:\dev\depot",
            "C:/dev/depot",
            r"\\serveur\partage\depot",
            "file:///C:/dev/depot",
            "ext::sh -c touch% x",
            "fd::3",
            "-u",
            "ssh://-oProxyCommand=x/depot",
            "git@-oProxyCommand=x:depot",
            "https://",
            "https://exemple.org/a b",
            "depot",
            "/srv/git/depot",
        ] {
            assert!(url_ok(bad).is_err(), "{bad}");
        }
        for good in ["depot", "mon depot", "Ozmo2", "v1.2"] {
            assert!(folder_name_ok(good).is_ok(), "{good}");
        }
        for bad in ["", ".", "..", r"a\b", "a/b", "C:", "con", "COM1", "lpt²", "nul.txt", "fin.", "fin ", "a*b", "a|b"]
        {
            assert!(folder_name_ok(bad).is_err(), "{bad}");
        }
        for good in ["main", "feature/x", "v1.2"] {
            assert!(branch_ok(good).is_ok(), "{good}");
        }
        for bad in ["-x", "a b", "a..b", "a~1", "a:b", "x.lock", "x/", "/x", "a@{1}"] {
            assert!(branch_ok(bad).is_err(), "{bad}");
        }
    }

    /// L'URL donne le nom du dossier tant qu'il n'a pas été saisi, et le compte que DragonTree présélectionne tant qu'un
    /// autre n'a pas été choisi pour elle ; un compte qui ne correspond pas à l'URL est refusé, comme dans sa fenêtre.
    #[test]
    fn the_url_suggests_the_folder_and_dragontree_the_account() {
        let accounts = [github("perso"), github("pro")];
        let preselect = |_: &str| Some("perso".to_string());
        let mut d = Draft::new(PathBuf::from(r"C:\dev"));
        assert_eq!(d.missing(), ["l'URL du dépôt", "le nom du dossier"]);
        d.set_url(" https://github.com/ada/outil.git ", preselect).unwrap();
        assert_eq!(
            (d.url.as_str(), d.folder.as_str(), d.account.as_deref()),
            ("https://github.com/ada/outil.git", "outil", Some("perso"))
        );
        d.set_account(Some("pro"), &accounts).unwrap();
        d.set_url("https://github.com/ada/outil.git", |_| panic!("même URL : pas de nouvelle présélection")).unwrap();
        assert_eq!(d.account.as_deref(), Some("pro"), "choisi à la main pour cette URL");
        d.set_folder("copie").unwrap();
        d.set_url("https://github.com/ada/autre.git", preselect).unwrap();
        assert_eq!(
            (d.folder.as_str(), d.account.as_deref()),
            ("copie", Some("perso")),
            "nom saisi gardé, compte proposé"
        );
        d.set_url("https://gitlab.com/ada/x.git", |_| None).unwrap();
        assert!(d.set_account(Some("perso"), &accounts).is_err(), "ne correspond pas à l'URL");
        assert!(d.set_account(None, &accounts).is_ok());
        assert!(d.set_url("C:/dev/x", preselect).is_err() && d.url == "https://gitlab.com/ada/x.git", "inchangée");
        assert!(d.request().is_err(), "destination pas encore contrôlée");
        d.target = Some(Ok(PathBuf::from(r"C:\dev")));
        d.set_branch("main").unwrap();
        let r = d.request().unwrap();
        assert_eq!((r.folder.as_str(), r.branch.as_deref(), r.account), ("copie", Some("main"), None));
    }

    /// L'écran d'un brouillon : chaque ligne se touche pour la changer ; « Cloner » une fois tout saisi et la
    /// destination contrôlée ; pendant le clonage, sa progression et « Annuler », et plus rien à changer.
    #[test]
    fn a_draft_screen_offers_each_step_then_cloner() {
        let accounts = [github("perso")];
        let folders = FolderInput { want: suite_sdk::mobile::ui::FOLDER_PARENT, ..Default::default() };
        let view =
            |job| DraftView { accounts: &accounts, preselected: None, job, open: None, folders: folders.clone() };
        let mut d = Draft::new(PathBuf::from(r"C:\dev"));
        let ids = |s: &Screen| s.action_ids().into_iter().collect::<Vec<_>>();
        let s = screen(&d, &view(None));
        assert_eq!(ids(&s), ["branche", "dossier", "nom", "sous-modules", "url"], "{s:?}");
        let spec = s.all_acts().into_iter().find(|a| a.id == "dossier").and_then(|a| a.input.clone()?.folder).unwrap();
        assert_eq!(
            (spec.want, spec.start.as_str(), spec.child.as_str()),
            (suite_sdk::mobile::ui::FOLDER_PARENT, r"C:\dev", "")
        );
        d.set_url("https://github.com/ada/outil.git", |_| Some("perso".into())).unwrap();
        d.target = Some(Ok(PathBuf::from(r"C:\dev")));
        let s = screen(&d, &view(None));
        assert_eq!(ids(&s), ["branche", "compte:", "dossier", "lancer", "nom", "sous-modules", "url"]);
        let spec = s.all_acts().into_iter().find(|a| a.id == "dossier").and_then(|a| a.input.clone()?.folder).unwrap();
        assert_eq!(spec.child, "outil", "le sélecteur contrôle le dossier qui sera créé");
        d.target = Some(Err("C:\\dev\\outil existe déjà et n'est pas vide.".into()));
        assert!(!ids(&screen(&d, &view(None))).contains(&"lancer".to_string()), "destination refusée");

        let job = JobView {
            id: 7,
            title: "Clonage de outil".into(),
            dest: PathBuf::from(r"C:\dev\outil"),
            progress: Some(Progress { phase: "Receiving objects".into(), percent: Some(47), current: 0, total: None }),
            result: None,
        };
        let s = screen(&d, &view(Some(&job)));
        assert_eq!(ids(&s), ["annuler-clone:7"], "pendant le clonage, rien d'autre : {s:?}");
        assert!(matches!(&s.blocks[0], Block::Note { progress: Some(45), .. }), "{:?}", s.blocks[0]);
        let ok = JobView { result: Some(Ok(())), progress: None, ..job.clone() };
        let s = screen(&d, &DraftView { open: Some("depot/abc".into()), ..view(Some(&ok)) });
        assert!(s.opens().contains("depot/abc") && ids(&s) == ["cloner"], "{s:?}");
        let failed = JobView { result: Some(Err("Clonage annulé.".into())), ..job.clone() };
        assert!(ids(&screen(&d, &view(Some(&failed)))).contains(&"url".to_string()), "de nouveau à changer");
    }
}
