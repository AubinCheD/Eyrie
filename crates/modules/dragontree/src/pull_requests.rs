//! Les pull requests sur le téléphone : celles que DragonTree lit sur les comptes de la suite, comme sa fenêtre « Pull
//! requests », qui reste fermée (`refresh_pull_requests`, en fond, quand le téléphone les regarde et qu'elles ont plus
//! de 5 min). Écran `pr` : un bandeau par compte, avec ce qui a manqué à sa place (jeton refusé, organisation Azure
//! DevOps qui n'a pas répondu), les plus récentes d'abord ; « Créées par moi » (`pr/miennes`) et « À relire »
//! (`pr/a-relire`) en onglets. Celles d'un dépôt sont aussi dans son onglet Plus ([`repo_band`]).
//!
//! Une pull request s'ouvre dans le navigateur du téléphone (une adresse web seulement) ; son menu mène à la page de
//! son dépôt, et au dépôt sur le téléphone quand une copie locale est dans la liste. Rien n'est envoyé au PC par ces
//! gestes : seule « Actualiser » lui demande une lecture.

use std::path::Path;

use dragontree_app::pull_requests::{ListedPullRequest, PullRequestsSnapshot};
use dragontree_app::settings::path_key;
use suite_sdk::mobile::ui::{Act, Band, Chip, Row, Screen, Tab, Tone};

use crate::clonage::forge_glyph;
use crate::depot::ago;
use crate::mobile::short_error;

/// Les écrans : toutes, celles de l'utilisateur, celles qu'on lui demande de relire.
pub const ALL: &str = "pr";
pub const MINE: &str = "pr/miennes";
pub const TO_REVIEW: &str = "pr/a-relire";

/// Un écran des pull requests.
pub fn is_screen(path: &str) -> bool {
    matches!(path, ALL | MINE | TO_REVIEW)
}

/// « Pull requests » (menu de la liste) : un écran du téléphone, rien n'est envoyé au PC.
pub fn list_act() -> Act {
    Act::screen("Pull requests", ALL).glyph("🔀")
}

/// Une adresse que le téléphone ouvre dans son navigateur.
fn web(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    lower.starts_with("https://") || lower.starts_with("http://")
}

/// La pull request `p` est-elle sur l'écran `path` ?
fn kept(path: &str, p: &ListedPullRequest) -> bool {
    match path {
        MINE => p.pr.created_by_me,
        TO_REVIEW => p.pr.review_requested,
        _ => true,
    }
}

/// Une pull request : son titre ; son dépôt, son numéro, son auteur et ses branches ; sa date ; brouillon et relecture
/// demandée en étiquettes. Le toucher l'ouvre dans le navigateur ; son menu mène à la page du dépôt et, `open`, au dépôt
/// sur le téléphone.
pub fn row(p: &ListedPullRequest, open: Option<String>, now: i64) -> Row {
    let pr = &p.pr;
    let mut parts = vec![pr.repo.display_name(), pr.reference()];
    if !pr.author.is_empty() {
        parts.push(format!("par {}", pr.author));
    }
    match (&pr.source_branch, &pr.target_branch) {
        (Some(source), Some(target)) => parts.push(format!("{source} → {target}")),
        (Some(branch), None) | (None, Some(branch)) => parts.push(branch.clone()),
        (None, None) => {}
    }
    let mut row = Row::new(&pr.title).subtitle(parts.join(" · ")).glyph(forge_glyph(pr.kind())).key(&pr.url);
    if let Some(ts) = pr.updated_timestamp() {
        row = row.detail(ago(ts, now));
    }
    if pr.review_requested {
        row = row.chip(Chip::new("À relire").glyph("👀").tone(Tone::Warn));
    }
    if pr.draft {
        row = row.chip(Chip::new("Brouillon").glyph("📝"));
    }
    if web(&pr.url) {
        row = row.tap(Act::link("Ouvrir dans le navigateur", &pr.url));
    }
    let page = pr.repo.web_url();
    if web(&page) {
        row = row.act(Act::link("Page du dépôt", page));
    }
    if let Some(open) = open {
        row = row.act(Act::screen("Ouvrir le dépôt", open).glyph("📂"));
    }
    row
}

/// L'écran `path` (`pr`, `pr/miennes`, `pr/a-relire`) ; `open` : l'écran du dépôt d'une pull request sur le
/// téléphone, si une copie locale est dans la liste.
pub fn screen(
    path: &str,
    snap: &PullRequestsSnapshot,
    open: impl Fn(&ListedPullRequest) -> Option<String>,
    now: i64,
) -> Screen {
    let mut s = Screen::new("Pull requests").search("Chercher une pull request");
    let count = |path: &str| snap.prs.iter().filter(|p| kept(path, p)).count();
    let tab = |label: &str, path: &str| {
        let n = count(path);
        let badge = if path != ALL && n > 0 { n.to_string() } else { String::new() };
        Tab { label: label.into(), path: path.into(), badge }
    };
    s.tabs = vec![tab("Toutes", ALL), tab("Créées par moi", MINE), tab("À relire", TO_REVIEW)];
    s.acts.push(Act::new("actualiser", "Actualiser").glyph("🔄").busy(snap.loading));
    s.subtitle = match snap.refreshed_at {
        _ if snap.loading => "Lecture des pull requests…".into(),
        Some(at) => format!("Mise à jour : {}", ago(at, now)),
        None => String::new(),
    };
    if snap.accounts.is_empty() {
        s.empty("Aucun compte : ajoutez un compte GitHub, GitLab ou Azure DevOps dans Eyrie sur le PC (Réglages › Comptes).");
        return s;
    }
    for a in &snap.accounts {
        let prs: Vec<&ListedPullRequest> =
            snap.prs.iter().filter(|p| p.account == a.account.id && kept(path, p)).collect();
        let mut band = Band::new(format!("compte:{}", a.account.id), a.account.title())
            .glyph(forge_glyph(a.account.kind))
            .aside(prs.len().to_string());
        if a.error.is_some() {
            band = band.tone(Tone::Danger);
        } else if !a.failed_orgs.is_empty() {
            band = band.tone(Tone::Warn);
        }
        s.band(band);
        for (org, error) in &a.failed_orgs {
            let text =
                format!("L'organisation {org} n'a pas répondu, ses pull requests manquent : {}", short_error(error));
            s.note(text, Tone::Warn);
        }
        if let Some(error) = &a.error {
            s.note(short_error(error), Tone::Danger);
        } else if prs.is_empty() && (a.loading || !a.read) {
            s.empty(format!("Lecture des pull requests de {}…", a.account.title()));
        } else if prs.is_empty() {
            s.empty(if path == ALL { "Aucune pull request ouverte." } else { "Aucune pull request ici." });
        }
        for p in prs {
            s.row(row(p, open(p), now));
        }
    }
    s
}

/// Les pull requests d'un dépôt, pour son onglet Plus.
#[derive(Clone, Debug, Default)]
pub struct RepoPullRequests {
    /// Celles dont une copie locale est le dépôt.
    pub prs: Vec<ListedPullRequest>,
    /// DragonTree les lit (ou ne les a jamais lues).
    pub reading: bool,
}

impl RepoPullRequests {
    /// Celles du dépôt `repo` parmi ce que DragonTree a lu ; `None` sans compte (rien à montrer).
    pub fn of(snap: &PullRequestsSnapshot, repo: &Path) -> Option<RepoPullRequests> {
        if snap.accounts.is_empty() {
            return None;
        }
        let key = path_key(repo);
        let prs = snap.prs.iter().filter(|p| p.local.iter().any(|l| path_key(l) == key)).cloned().collect();
        Some(RepoPullRequests { prs, reading: snap.loading || snap.refreshed.is_none() })
    }
}

/// Le bandeau des pull requests d'un dépôt (onglet Plus), et le chemin de toutes.
pub fn repo_band(s: &mut Screen, prs: &RepoPullRequests, now: i64) {
    s.band(Band::new("pull-requests", "Pull requests").glyph("🔀").aside(prs.prs.len().to_string()));
    if prs.prs.is_empty() {
        s.empty(if prs.reading {
            "Lecture des pull requests…"
        } else {
            "Aucune pull request ouverte sur ce dépôt."
        });
    }
    for p in &prs.prs {
        s.row(row(p, None, now));
    }
    s.row(Row::new("Toutes les pull requests").glyph("🔀").key("toutes-pr").open(ALL));
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use dragontree_app::pull_requests::AccountPullRequests;
    use dragontree_forges::{Account, ForgeKind, PullRequest, RepoLocation};
    use suite_sdk::mobile::ui::Block;

    use super::*;

    fn account(id: &str) -> Account {
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

    fn listed(account: &str, title: &str, url: &str, local: &[&str]) -> ListedPullRequest {
        ListedPullRequest {
            account: account.into(),
            pr: PullRequest {
                account_id: account.into(),
                url: url.into(),
                repo: RepoLocation::GitHub {
                    host: "github.com".into(),
                    owner: "contoso".into(),
                    repo: "northwind".into(),
                },
                number: 12,
                title: title.into(),
                author: "asmith".into(),
                draft: false,
                reviewers: vec![],
                created_by_me: false,
                review_requested: false,
                updated_at: Some("2026-10-20T08:00:00Z".into()),
                source_branch: Some("feature/x".into()),
                target_branch: Some("main".into()),
            },
            local: local.iter().map(PathBuf::from).collect(),
        }
    }

    fn snapshot() -> PullRequestsSnapshot {
        let state = |id: &str, error: Option<&str>| AccountPullRequests {
            account: account(id),
            read: true,
            loading: false,
            error: error.map(str::to_string),
            failed_orgs: vec![],
        };
        let mut mine =
            listed("perso", "Ajoute l'accueil", "https://github.com/contoso/northwind/pull/12", &[r"C:\dev\northwind"]);
        mine.pr.created_by_me = true;
        mine.pr.draft = true;
        let mut review = listed("perso", "Corrige les remises", "javascript:alert(1)", &[]);
        review.pr.review_requested = true;
        PullRequestsSnapshot {
            accounts: vec![state("perso", None), state("pro", Some("Accès refusé"))],
            prs: vec![mine, review],
            loading: false,
            refreshed: Some(std::time::Instant::now()),
            refreshed_at: Some(1_761_000_000),
        }
    }

    /// Un bandeau par compte, l'erreur d'un compte à sa place ; les onglets comptent celles de l'utilisateur et celles à
    /// relire ; une pull request s'ouvre dans le navigateur (une adresse web seulement), et son dépôt sur le téléphone
    /// quand une copie locale est dans la liste.
    #[test]
    fn the_pull_requests_are_shown_by_account_with_their_links() {
        let snap = snapshot();
        let open = |p: &ListedPullRequest| (!p.local.is_empty()).then(|| "depot/abc".to_string());
        let s = screen(ALL, &snap, open, 1_761_000_000 + 120);
        let badges: Vec<(&str, &str)> = s.tabs.iter().map(|t| (t.path.as_str(), t.badge.as_str())).collect();
        assert_eq!(badges, [(ALL, ""), (MINE, "1"), (TO_REVIEW, "1")]);
        assert_eq!(s.subtitle, "Mise à jour : il y a 2 min");
        let bands: Vec<&str> = s
            .blocks
            .iter()
            .filter_map(|b| match b {
                Block::Section { title, .. } => Some(title.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(bands, ["GitHub · perso", "GitHub · pro"]);
        assert!(
            s.blocks
                .iter()
                .any(|b| matches!(b, Block::Note { tone: Tone::Danger, text, .. } if text == "Accès refusé"))
        );
        let rows: Vec<&Row> = s
            .blocks
            .iter()
            .filter_map(|b| match b {
                Block::Row(r) => Some(r),
                _ => None,
            })
            .collect();
        assert_eq!(
            rows[0].tap.as_ref().and_then(|a| a.url.as_deref()),
            Some("https://github.com/contoso/northwind/pull/12")
        );
        assert!(rows[0].chips.iter().any(|c| c.text == "Brouillon"));
        assert!(rows[0].acts.iter().any(|a| a.open.as_deref() == Some("depot/abc")), "le dépôt, sur le téléphone");
        assert!(rows[1].tap.is_none(), "pas une adresse web : rien à ouvrir");
        assert!(s.action_ids().into_iter().eq(["actualiser".to_string()]), "les liens ne vont pas au PC");
        let s = screen(TO_REVIEW, &snap, open, 0);
        assert!(format!("{s:?}").contains("Corrige les remises") && !format!("{s:?}").contains("Ajoute l'accueil"));
        let none = PullRequestsSnapshot::default();
        assert!(format!("{:?}", screen(ALL, &none, open, 0)).contains("Aucun compte"));
    }

    /// L'onglet Plus d'un dépôt montre celles dont une copie locale est ce dépôt (sans casse ni séparateurs), et mène à
    /// toutes ; sans compte, rien.
    #[test]
    fn a_repo_shows_its_own_pull_requests() {
        let snap = snapshot();
        let mine = RepoPullRequests::of(&snap, Path::new("c:/DEV/Northwind")).unwrap();
        assert_eq!(mine.prs.len(), 1);
        let mut s = Screen::new("Plus");
        repo_band(&mut s, &mine, 0);
        assert!(s.opens().contains(ALL) && format!("{s:?}").contains("Ajoute l'accueil"), "{s:?}");
        assert!(RepoPullRequests::of(&snap, Path::new(r"C:\dev\autre")).unwrap().prs.is_empty());
        assert!(RepoPullRequests::of(&PullRequestsSnapshot::default(), Path::new(r"C:\dev\northwind")).is_none());
    }
}
