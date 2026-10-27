//! Les comptes de forge communs à toute la suite EyrieSuite (voir `docs/comptes.md`).
//!
//! Un seul endroit pour toute la suite, que l'application tourne seule ou dans Eyrie :
//! - la liste, sans jeton, dans `%APPDATA%\EyrieSuite\accounts.json` ;
//! - les jetons dans le Gestionnaire d'identifiants de Windows, service « EyrieSuite », utilisateur
//!   `account:<id>`.
//!
//! On se connecte une fois, dans Eyrie, DragonTree ou Owlery : le compte vaut partout, y compris pour
//! une application installée plus tard.

mod store;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub use store::{MemoryVault, OsVault, Store, Vault};

/// Nom commun de la suite : dossier de `%APPDATA%` (et, à terme, d'installation), service du coffre.
pub const SUITE: &str = "EyrieSuite";
/// Dossier commun imposé (essais, tests) ; posé par Eyrie pour les modules qu'il embarque.
pub const DIR_ENV: &str = "EYRIESUITE_DIR";

/// Les forges connues (`Account::forge`). Une forge inconnue est gardée telle quelle et ignorée par
/// les applications qui ne la connaissent pas.
pub mod forge {
    pub const GITHUB: &str = "github";
    pub const GITLAB: &str = "gitlab";
    pub const AZURE: &str = "azure";

    /// Hôte utilisé quand l'utilisateur n'en saisit pas.
    pub fn default_host(kind: &str) -> Option<&'static str> {
        match kind {
            GITHUB => Some("github.com"),
            GITLAB => Some("gitlab.com"),
            AZURE => Some("dev.azure.com"),
            _ => None,
        }
    }
}

/// Dossier commun de la suite : `EYRIESUITE_DIR`, sinon `%APPDATA%\EyrieSuite`.
pub fn suite_dir() -> PathBuf {
    pick_dir(env_dir(), None, default_suite_dir())
}

/// Dossier des comptes : `EYRIESUITE_DIR`, sinon `isolated` (profil d'essai ou de test de
/// l'application, qui garde alors des comptes à part), sinon le dossier commun.
pub fn accounts_dir(isolated: Option<&Path>) -> PathBuf {
    pick_dir(env_dir(), isolated, default_suite_dir())
}

fn env_dir() -> Option<PathBuf> {
    std::env::var_os(DIR_ENV).filter(|d| !d.is_empty()).map(PathBuf::from)
}

fn default_suite_dir() -> PathBuf {
    dirs::config_dir().unwrap_or_else(std::env::temp_dir).join(SUITE)
}

fn pick_dir(forced: Option<PathBuf>, isolated: Option<&Path>, common: PathBuf) -> PathBuf {
    forced.or_else(|| isolated.map(Path::to_path_buf)).unwrap_or(common)
}

/// Service du coffre pour les comptes d'un profil isolé (`dir`) : « EyrieSuite:<empreinte du dossier> ». Ses jetons
/// ne côtoient pas ceux de la suite : retirer un compte d'une copie de `accounts.json` (mêmes identifiants) n'efface
/// pas le vrai jeton, et un profil d'essai ne lit jamais les vrais.
pub fn isolated_vault_service(dir: &Path) -> String {
    // FNV-1a 64 bits du chemin en minuscules, séparateurs unifiés : stable d'un lancement à l'autre.
    let key = dir.to_string_lossy().replace('/', "\\").trim_end_matches('\\').to_lowercase();
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in key.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{SUITE}:{h:016x}")
}

/// Un compte de forge, sans son jeton.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Account {
    /// Identifiant tiré au hasard à l'ajout ; il ne change plus (clé du jeton, dépôts associés).
    pub id: String,
    /// Voir [`forge`].
    pub forge: String,
    /// Hôte normalisé (voir [`normalize_host`]).
    #[serde(default)]
    pub host: String,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub display_name: String,
    /// Organisations (Azure DevOps Services) ou collections (Azure DevOps Server).
    #[serde(default)]
    pub orgs: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avatar_url: Option<String>,
    /// Champs écrits par une autre version : gardés tels quels.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

/// Champs des comptes de SuperApp, qui n'ont plus de sens : tous les comptes valent pour toute la suite.
const LEGACY_FIELDS: [&str; 2] = ["shared_with", "source"];

impl Account {
    /// Hôte normalisé, organisation Azure tirée d'un ancien hôte `<org>.visualstudio.com`, champs
    /// hérités retirés.
    pub fn normalized(mut self) -> Account {
        if self.forge == forge::AZURE
            && self.orgs.iter().all(|o| o.trim().is_empty())
            && let Some(org) = azure_org_from_host(&self.host)
        {
            self.orgs = vec![org];
        }
        self.host = normalize_host(&self.forge, &self.host);
        self.orgs.retain(|o| !o.trim().is_empty());
        for key in LEGACY_FIELDS {
            self.extra.remove(key);
        }
        self
    }

    /// Même connexion : même forge, même hôte normalisé, même identifiant (casse ignorée), quel que soit `id`.
    pub fn same_login(&self, other: &Account) -> bool {
        self.forge.eq_ignore_ascii_case(&other.forge)
            && normalize_host(&self.forge, &self.host) == normalize_host(&other.forge, &other.host)
            && self.username.trim().eq_ignore_ascii_case(other.username.trim())
    }
}

/// Hôte sans schéma, chemin, utilisateur ni majuscules ; vide : celui de la forge. Azure DevOps Services
/// (`dev.azure.com` ou l'ancien `<org>.visualstudio.com`) devient `dev.azure.com`.
pub fn normalize_host(kind: &str, host: &str) -> String {
    let h = bare_host(host);
    if h.is_empty() {
        return forge::default_host(kind).unwrap_or_default().to_string();
    }
    if kind == forge::AZURE && h.ends_with(".visualstudio.com") {
        return "dev.azure.com".into();
    }
    if kind == forge::GITHUB && h == "api.github.com" {
        return "github.com".into();
    }
    h
}

/// Organisation d'un ancien hôte Azure DevOps `<org>.visualstudio.com`.
pub fn azure_org_from_host(host: &str) -> Option<String> {
    let h = bare_host(host);
    h.strip_suffix(".visualstudio.com").filter(|o| !o.is_empty() && !o.contains('.')).map(str::to_string)
}

fn bare_host(host: &str) -> String {
    let mut h = host.trim();
    if let Some(i) = h.find("://") {
        h = &h[i + 3..];
    }
    let h = h.split(['/', '?', '#']).next().unwrap_or_default();
    h.rsplit('@').next().unwrap_or_default().trim().to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account(forge: &str, host: &str, username: &str) -> Account {
        Account {
            id: "0123456789abcdef".into(),
            forge: forge.into(),
            host: host.into(),
            username: username.into(),
            ..Account::default()
        }
    }

    #[test]
    fn hosts_are_normalized() {
        assert_eq!(normalize_host(forge::GITHUB, " HTTPS://GitHub.com/ "), "github.com");
        assert_eq!(normalize_host(forge::GITLAB, "https://moi@git.contoso.test:8443/x"), "git.contoso.test:8443");
        assert_eq!(normalize_host(forge::AZURE, "https://dev-xelya.visualstudio.com/"), "dev.azure.com");
        assert_eq!(normalize_host(forge::AZURE, ""), "dev.azure.com");
        assert_eq!(normalize_host(forge::GITHUB, "  "), "github.com");
        assert_eq!(normalize_host(forge::GITLAB, ""), "gitlab.com");
        assert_eq!(normalize_host(forge::AZURE, "tfs.contoso.test/DefaultCollection"), "tfs.contoso.test");
    }

    #[test]
    fn azure_org_comes_from_the_old_host() {
        assert_eq!(azure_org_from_host("https://dev-xelya.visualstudio.com/").as_deref(), Some("dev-xelya"));
        assert_eq!(azure_org_from_host("dev.azure.com"), None);
        let a = account(forge::AZURE, "https://dev-xelya.visualstudio.com/", "a@b.c").normalized();
        assert_eq!(a.host, "dev.azure.com");
        assert_eq!(a.orgs, ["dev-xelya"]);
        let mut b = account(forge::AZURE, "https://dev-xelya.visualstudio.com/", "a@b.c");
        b.orgs = vec!["autre".into()];
        assert_eq!(b.normalized().orgs, ["autre"]);
    }

    #[test]
    fn same_login_ignores_case_scheme_and_id() {
        let a = account(forge::GITHUB, "github.com", "Ada");
        let mut b = account(forge::GITHUB, "https://GitHub.com/", " ada ");
        b.id = "fedcba9876543210".into();
        assert!(a.same_login(&b));
        assert!(!a.same_login(&account(forge::GITLAB, "github.com", "ada")));
        assert!(!a.same_login(&account(forge::GITHUB, "github.com", "grace")));
    }

    #[test]
    fn accounts_dir_prefers_the_variable_then_the_isolated_profile() {
        use std::path::{Path, PathBuf};
        let common = PathBuf::from(r"C:\Users\ada\AppData\Roaming\EyrieSuite");
        let isolated = Path::new(r"C:\Temp\profil-essai");
        let forced = PathBuf::from(r"D:\essais\suite");
        assert_eq!(pick_dir(None, None, common.clone()), common);
        assert_eq!(pick_dir(None, Some(isolated), common.clone()), isolated);
        assert_eq!(pick_dir(Some(forced.clone()), Some(isolated), common), forced);
    }

    #[test]
    fn an_isolated_profile_has_a_vault_service_of_its_own() {
        use std::path::Path;
        let a = isolated_vault_service(Path::new(r"C:\Temp\profil-essai"));
        assert!(a.starts_with("EyrieSuite:") && a != SUITE, "{a}");
        assert_eq!(a, isolated_vault_service(Path::new(r"c:/temp/PROFIL-ESSAI/")), "même dossier, même service");
        assert_ne!(a, isolated_vault_service(Path::new(r"C:\Temp\autre")));
    }
}
