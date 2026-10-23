//! Les comptes de la suite (proposition du contrat d'Owlery, §5) : la liste, sans jeton, dans
//! `<suite>\accounts.json` ; les jetons dans le Gestionnaire d'identifiants de Windows (service
//! « SuperApp », clé `account:<id>`) ; un partage **compte par compte** (tel compte GitHub pour
//! DragonTree et Owlery, pas pour Patronus).
//!
//! La vérification passe par `dragontree-forges` (GitHub, GitLab, Azure DevOps et leurs versions
//! hébergées) : premier pas vers la crate de forges commune. À la première ouverture, la suite
//! propose de **copier** les comptes de DragonTree et d'Owlery ; les exécutables gardent les leurs.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};

use dragontree_forges::ForgeKind;
use serde::{Deserialize, Serialize};
use suite_sdk::Host;
use suite_sdk::host::AccountInfo;

pub const SERVICE: &str = "SuperApp";

/// Les modules qui savent utiliser un compte de forge.
pub const FORGE_MODULES: [&str; 3] = ["dragontree", "owlery", "patronus"];

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StoredAccount {
    #[serde(flatten)]
    pub info: AccountInfo,
    #[serde(default)]
    pub display_name: String,
    #[serde(default)]
    pub orgs: Vec<String>,
    /// D'où vient le compte : `suite`, `dragontree`, `owlery`.
    #[serde(default)]
    pub source: String,
}

impl StoredAccount {
    pub fn title(&self) -> String {
        let forge = match self.info.forge.as_str() {
            "github" => "GitHub",
            "gitlab" => "GitLab",
            "azure" => "Azure DevOps",
            "claude" => "Claude",
            other => other,
        };
        let who = if self.display_name.trim().is_empty() { &self.info.username } else { &self.display_name };
        format!("{forge} · {who} ({})", self.info.host)
    }
}

#[derive(Default, Serialize, Deserialize)]
struct File {
    #[serde(default)]
    accounts: Vec<StoredAccount>,
    #[serde(flatten)]
    extra: std::collections::BTreeMap<String, serde_json::Value>,
}

pub struct Accounts {
    pub list: Vec<StoredAccount>,
    path: PathBuf,
    extra: std::collections::BTreeMap<String, serde_json::Value>,
    /// Vérification d'un jeton en cours (appel réseau, hors du fil de l'interface).
    pending: Option<Receiver<Result<(StoredAccount, String), String>>>,
    pub message: Option<(bool, String)>,
}

pub fn forge_key(kind: ForgeKind) -> &'static str {
    match kind {
        ForgeKind::GitHub => "github",
        ForgeKind::GitLab => "gitlab",
        ForgeKind::AzureDevOps => "azure",
    }
}

fn entry(service: &str, id: &str) -> Result<keyring::Entry, String> {
    keyring::Entry::new(service, &format!("account:{id}")).map_err(|e| e.to_string())
}

fn read_token(service: &str, id: &str) -> Option<String> {
    entry(service, id).ok()?.get_password().ok()
}

impl Accounts {
    pub fn load(dir: &Path) -> Self {
        let path = dir.join("accounts.json");
        let file: File = std::fs::read_to_string(&path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        Accounts { list: file.accounts, path, extra: file.extra, pending: None, message: None }
    }

    fn save(&mut self) {
        let file = File { accounts: self.list.clone(), extra: self.extra.clone() };
        let text = serde_json::to_string_pretty(&file).unwrap_or_default();
        if let Err(e) = suite_sdk::state::write_atomic(&self.path, text.as_bytes()) {
            self.message = Some((false, format!("accounts.json : {e}")));
        }
    }

    /// Donne aux modules la liste (sans jeton) et la lecture du coffre.
    pub fn publish(&self, host: &Host) {
        host.set_accounts(self.list.iter().map(|a| a.info.clone()).collect(), Arc::new(|id: &str| read_token(SERVICE, id)));
    }

    pub fn is_busy(&self) -> bool {
        self.pending.is_some()
    }

    /// Vérifie le jeton auprès de la forge, puis range le compte (en arrière-plan).
    pub fn add(&mut self, kind: ForgeKind, host: &str, token: &str, orgs: Vec<String>) {
        let (tx, rx) = channel();
        let (host, token) = (if host.trim().is_empty() { kind.default_host().to_string() } else { host.trim().to_string() }, token.trim().to_string());
        std::thread::spawn(move || {
            let r = dragontree_forges::verify_new(kind, &host, &token, &orgs).map_err(|e| e.to_string()).map(|u| {
                let acc = StoredAccount {
                    info: AccountInfo {
                        id: dragontree_forges::new_account_id(),
                        forge: forge_key(kind).into(),
                        host: host.clone(),
                        username: u.username,
                        shared_with: FORGE_MODULES.iter().map(|m| m.to_string()).collect(),
                    },
                    display_name: u.display_name,
                    orgs: u.orgs,
                    source: "suite".into(),
                };
                (acc, token)
            });
            let _ = tx.send(r);
        });
        self.pending = Some(rx);
        self.message = None;
    }

    /// À appeler à chaque image tant qu'une vérification est en cours.
    pub fn poll(&mut self, host: &Host) {
        let Some(rx) = &self.pending else { return };
        let Ok(r) = rx.try_recv() else { return };
        self.pending = None;
        match r {
            Ok((acc, token)) => match entry(SERVICE, &acc.info.id).and_then(|e| e.set_password(&token).map_err(|e| e.to_string())) {
                Ok(()) => {
                    self.message = Some((true, format!("{} : compte ajouté.", acc.title())));
                    self.list.push(acc);
                    self.save();
                    self.publish(host);
                }
                Err(e) => self.message = Some((false, format!("Coffre de Windows : {e}"))),
            },
            Err(e) => self.message = Some((false, e)),
        }
    }

    pub fn remove(&mut self, id: &str, host: &Host) {
        if let Ok(e) = entry(SERVICE, id) {
            let _ = e.delete_credential();
        }
        self.list.retain(|a| a.info.id != id);
        self.save();
        self.publish(host);
    }

    pub fn set_share(&mut self, id: &str, module: &str, shared: bool, host: &Host) {
        if let Some(a) = self.list.iter_mut().find(|a| a.info.id == id) {
            a.info.shared_with.retain(|m| m != module);
            if shared {
                a.info.shared_with.push(module.to_string());
            }
        }
        self.save();
        self.publish(host);
    }

    /// Copie les comptes d'un module (liste et jetons) dans la suite. Renvoie (copiés, déjà là).
    fn import(&mut self, source: &str, service: &str, accounts: Vec<serde_json::Value>, host: &Host) -> (usize, usize) {
        let (mut copied, mut known) = (0, 0);
        for a in accounts {
            let s = |k: &str| a.get(k).and_then(|v| v.as_str()).unwrap_or_default().to_string();
            let id = s("id");
            if id.is_empty() {
                continue;
            }
            let forge_raw = if a.get("kind").is_some() { s("kind") } else { s("forge") }.to_lowercase();
            let forge = if forge_raw.contains("gitlab") {
                "gitlab"
            } else if forge_raw.contains("azure") {
                "azure"
            } else {
                "github"
            };
            if self.list.iter().any(|x| x.info.id == id || (x.info.forge == forge && x.info.host == s("host") && x.info.username == s("username"))) {
                known += 1;
                continue;
            }
            // Owlery : un compte « gh » passe par la CLI, sans jeton à copier.
            let Some(token) = read_token(service, &id) else { continue };
            if entry(SERVICE, &id).and_then(|e| e.set_password(&token).map_err(|e| e.to_string())).is_err() {
                continue;
            }
            let orgs = a.get("orgs").and_then(|v| serde_json::from_value(v.clone()).ok()).unwrap_or_default();
            self.list.push(StoredAccount {
                info: AccountInfo { id, forge: forge.into(), host: s("host"), username: s("username"), shared_with: FORGE_MODULES.iter().map(|m| m.to_string()).collect() },
                display_name: s("display_name"),
                orgs,
                source: source.into(),
            });
            copied += 1;
        }
        self.save();
        self.publish(host);
        (copied, known)
    }

    pub fn import_dragontree(&mut self, host: &Host) {
        let path = dirs::config_dir().unwrap_or_default().join("DragonTree").join("settings.json");
        self.import_from(&path, "dragontree", "DragonTree", host);
    }

    pub fn import_owlery(&mut self, host: &Host) {
        let dir = std::env::var_os("OWLERY_DATA_DIR").map(PathBuf::from).unwrap_or_else(|| dirs::config_dir().unwrap_or_default().join("Owlery"));
        self.import_from(&dir.join("state.json"), "owlery", "Owlery", host);
    }

    fn import_from(&mut self, path: &Path, source: &str, service: &str, host: &Host) {
        let accounts = std::fs::read_to_string(path)
            .ok()
            .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
            .and_then(|v| v.get("accounts").and_then(|a| a.as_array().cloned()));
        self.message = Some(match accounts {
            None => (false, format!("{} : aucun compte trouvé ({}).", service, path.display())),
            Some(list) => {
                let (copied, known) = self.import(source, service, list, host);
                (true, format!("{service} : {copied} compte(s) copié(s), {known} déjà dans la suite."))
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles() {
        let a = StoredAccount {
            info: AccountInfo { id: "x".into(), forge: "azure".into(), host: "dev.azure.com".into(), username: "a@b.c".into(), shared_with: vec![] },
            display_name: "Camille Martin".into(),
            orgs: vec![],
            source: "suite".into(),
        };
        assert_eq!(a.title(), "Azure DevOps · Camille Martin (dev.azure.com)");
    }

    #[test]
    fn file_keeps_unknown_fields() {
        let f: File = serde_json::from_str(r#"{"accounts":[],"futur":1}"#).unwrap();
        assert_eq!(serde_json::to_value(&f).unwrap()["futur"], 1);
    }
}
