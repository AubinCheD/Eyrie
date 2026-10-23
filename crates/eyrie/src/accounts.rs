//! Les comptes de forge dans Eyrie : ceux de toute la suite (crate `suite-accounts`, voir
//! `docs/comptes.md`), vérifiés auprès de la forge par `dragontree-forges` avant d'être enregistrés.
//! DragonTree et Owlery, seuls ou dans Eyrie, lisent et écrivent les mêmes.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant, SystemTime};

use dragontree_forges::{ForgeError, ForgeKind, VerifiedUser};
use suite_accounts::{Account, Store, Vault, forge};

use crate::i18n::tr;

/// Relecture du fichier commun (une autre application a pu le changer) : au plus toutes les 2 s.
const CHECK_EVERY: Duration = Duration::from_secs(2);

/// « GitHub · ada », « Azure DevOps · Camille Martin » ; l'hôte entre parenthèses s'il n'est pas celui de
/// la forge.
pub fn title(a: &Account) -> String {
    let label = ForgeKind::from_suite_key(&a.forge).map_or(a.forge.as_str(), |k| k.label());
    // Azure : l'identifiant est une adresse e-mail, le nom affiché est plus lisible.
    let (first, second) =
        if a.forge == forge::AZURE { (&a.display_name, &a.username) } else { (&a.username, &a.display_name) };
    let who = if first.trim().is_empty() { second } else { first };
    match forge::default_host(&a.forge) {
        Some(h) if h == a.host => format!("{label} · {who}"),
        _ => format!("{label} · {who} ({})", a.host),
    }
}

/// Hôte (celui de la forge s'il est vide) et organisations à vérifier : sans organisation saisie, celle
/// d'un ancien hôte Azure `<org>.visualstudio.com` est déduite (un jeton limité à une organisation n'a
/// pas accès au profil global).
pub fn prepare(kind: ForgeKind, host: &str, orgs: Vec<String>) -> (String, Vec<String>) {
    let host = match host.trim() {
        "" => kind.default_host().to_string(),
        h => h.to_string(),
    };
    let mut orgs: Vec<String> = orgs.into_iter().map(|o| o.trim().to_string()).filter(|o| !o.is_empty()).collect();
    if kind == ForgeKind::AzureDevOps
        && orgs.is_empty()
        && let Some(org) = suite_accounts::azure_org_from_host(&host)
    {
        orgs.push(org);
    }
    (host, orgs)
}

/// Une erreur de vérification, dite pour l'utilisateur.
pub fn explain(e: &ForgeError, kind: ForgeKind) -> String {
    match e {
        ForgeError::Unauthorized if kind == ForgeKind::AzureDevOps => tr(
            "Jeton refusé. S'il est limité à une organisation, indiquez-la ; sinon vérifiez sa validité et ses droits.",
        )
        .to_string(),
        ForgeError::Unauthorized => {
            tr("Jeton refusé : vérifiez qu'il est valide et qu'il a les droits recommandés.").to_string()
        }
        other => other.to_string(),
    }
}

/// Réponse d'une vérification faite hors du fil de l'interface.
enum Job {
    Added { kind: ForgeKind, host: String, token: String, result: Result<VerifiedUser, ForgeError> },
    Rechecked { id: String, kind: ForgeKind, result: Result<VerifiedUser, ForgeError> },
}

pub struct Accounts {
    store: Store,
    list: Vec<Account>,
    stamp: Option<SystemTime>,
    next_check: Option<Instant>,
    tx: Sender<Job>,
    rx: Receiver<Job>,
    adding: usize,
    checking: BTreeSet<String>,
    /// Anciens coffres (« SuperApp », « DragonTree », « Owlery ») : un compte retiré y est effacé aussi.
    legacy: Vec<Arc<dyn Vault>>,
    /// Résultat de la dernière vérification de chaque compte (`Err` : l'explication).
    pub status: BTreeMap<String, Result<(), String>>,
    pub message: Option<(bool, String)>,
}

impl Accounts {
    pub fn new(store: Store) -> Accounts {
        let (tx, rx) = channel();
        let mut a = Accounts {
            store,
            list: Vec::new(),
            stamp: None,
            next_check: None,
            tx,
            rx,
            adding: 0,
            checking: BTreeSet::new(),
            legacy: Vec::new(),
            status: BTreeMap::new(),
            message: None,
        };
        a.reload();
        a
    }

    /// Les anciens coffres où un compte retiré doit aussi disparaître.
    pub fn with_legacy_vaults(mut self, vaults: Vec<Arc<dyn Vault>>) -> Accounts {
        self.legacy = vaults;
        self
    }

    /// Relit les comptes. Un fichier illisible n'est pas « aucun compte » : la dernière liste reste, la date n'avance
    /// pas (il sera relu), et l'erreur est dite.
    fn reload(&mut self) {
        self.next_check = Some(Instant::now() + CHECK_EVERY);
        // La date avant la liste : une écriture qui tombe entre les deux sera relue la fois suivante.
        let stamp = self.store.stamp();
        match self.store.try_list() {
            Ok(list) => {
                self.list = list;
                self.stamp = stamp;
            }
            Err(e) => self.message = Some((false, e)),
        }
    }

    /// Les comptes de la suite, relus si une autre application les a changés.
    pub fn list(&mut self) -> &[Account] {
        if self.next_check.is_none_or(|t| Instant::now() >= t) {
            self.next_check = Some(Instant::now() + CHECK_EVERY);
            if self.store.stamp() != self.stamp {
                self.reload();
            }
        }
        &self.list
    }

    /// Un ajout est en cours de vérification.
    pub fn is_busy(&self) -> bool {
        self.adding > 0
    }

    pub fn is_checking(&self, id: &str) -> bool {
        self.checking.contains(id)
    }

    /// Vérifie le jeton auprès de la forge (hors du fil de l'interface), puis range le compte.
    pub fn add(&mut self, kind: ForgeKind, host: &str, token: &str, orgs: Vec<String>) {
        let (host, orgs) = prepare(kind, host, orgs);
        let token = token.trim().to_string();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let result = dragontree_forges::verify_new(kind, &host, &token, &orgs);
            let _ = tx.send(Job::Added { kind, host, token, result });
        });
        self.adding += 1;
        self.message = None;
    }

    /// Revérifie le jeton d'un compte : son nom et ses organisations sont mis à jour.
    pub fn recheck(&mut self, id: &str) {
        let Some(a) = self.list.iter().find(|a| a.id == id).cloned() else { return };
        let Some(kind) = ForgeKind::from_suite_key(&a.forge) else { return };
        let token = self.store.token(id);
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let result = match token {
                Ok(Some(t)) => dragontree_forges::verify(kind, &a.host, &t, &a.orgs),
                Ok(None) => Err(ForgeError::Secret(tr("jeton absent du Gestionnaire d'identifiants").into())),
                Err(e) => Err(ForgeError::Secret(e)),
            };
            let _ = tx.send(Job::Rechecked { id: a.id, kind, result });
        });
        self.checking.insert(id.to_string());
    }

    /// À appeler à chaque image : range les vérifications terminées.
    pub fn poll(&mut self) {
        while let Ok(job) = self.rx.try_recv() {
            match job {
                Job::Added { kind, host, token, result } => {
                    self.adding = self.adding.saturating_sub(1);
                    self.finish(kind, &host, result, &token);
                }
                Job::Rechecked { id, kind, result } => self.finish_recheck(&id, kind, result),
            }
        }
    }

    fn finish(&mut self, kind: ForgeKind, host: &str, result: Result<VerifiedUser, ForgeError>, token: &str) {
        let user = match result {
            Ok(user) => user,
            Err(e) => {
                self.message = Some((false, explain(&e, kind)));
                return;
            }
        };
        let account = Account {
            id: dragontree_forges::new_account_id(),
            forge: kind.suite_key().into(),
            host: host.into(),
            username: user.username,
            display_name: user.display_name,
            orgs: user.orgs,
            avatar_url: user.avatar_url,
            ..Account::default()
        };
        // Une connexion déjà connue garde son identifiant : le jeton est alors remplacé pour toute la suite.
        let new_id = account.id.clone();
        self.message = Some(match self.store.add(account, token) {
            Ok(a) if a.id != new_id => (
                true,
                format!("{} {} — {}", tr("Compte mis à jour :"), title(&a), tr("jeton remplacé pour toute la suite")),
            ),
            Ok(a) => (true, format!("{} {}", tr("Compte ajouté :"), title(&a))),
            Err(e) => (false, format!("{} {e}", tr("Enregistrement impossible :"))),
        });
        self.reload();
    }

    fn finish_recheck(&mut self, id: &str, kind: ForgeKind, result: Result<VerifiedUser, ForgeError>) {
        self.checking.remove(id);
        let status = match result {
            Ok(user) => {
                if let Some(mut a) = self.list.iter().find(|a| a.id == id).cloned() {
                    a.display_name = user.display_name;
                    if !user.orgs.is_empty() {
                        a.orgs = user.orgs;
                    }
                    a.avatar_url = user.avatar_url;
                    if let Err(e) = self.store.update(&a) {
                        self.message = Some((false, format!("{} {e}", tr("Enregistrement impossible :"))));
                    }
                    self.reload();
                }
                Ok(())
            }
            Err(e) => Err(explain(&e, kind)),
        };
        self.status.insert(id.to_string(), status);
    }

    /// Retire le compte de toute la suite (liste et jeton, anciens coffres compris).
    pub fn remove(&mut self, id: &str) {
        if let Err(e) = self.store.remove(id) {
            self.message = Some((false, format!("{} {e}", tr("Suppression impossible :"))));
            return;
        }
        for vault in &self.legacy {
            let _ = vault.delete(id);
        }
        self.status.remove(id);
        self.reload();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use suite_accounts::{MemoryVault, Store};

    use super::*;

    fn accounts() -> (Accounts, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        (Accounts::new(Store::new(dir.path(), Arc::new(MemoryVault::default()))), dir)
    }

    fn ada() -> VerifiedUser {
        VerifiedUser { username: "ada".into(), display_name: String::new(), avatar_url: None, orgs: Vec::new() }
    }

    #[test]
    fn re_adding_a_known_login_says_the_token_is_replaced() {
        let (mut a, _dir) = accounts();
        a.finish(ForgeKind::GitHub, "github.com", Ok(ada()), "t1");
        let replaced = tr("jeton remplacé pour toute la suite");
        assert!(!a.message.clone().unwrap().1.contains(replaced));
        a.finish(ForgeKind::GitHub, "https://GitHub.com/", Ok(ada()), "t2");
        let (ok, msg) = a.message.clone().unwrap();
        assert!(ok);
        assert!(msg.contains(replaced), "{msg}");
        assert_eq!(a.list().len(), 1);
    }

    #[test]
    fn an_unreadable_file_keeps_the_last_accounts_and_says_so() {
        let (mut a, dir) = accounts();
        a.finish(ForgeKind::GitHub, "github.com", Ok(ada()), "t");
        std::fs::write(dir.path().join("accounts.json"), "{ pas du json").unwrap();
        a.next_check = None;
        assert_eq!(a.list().len(), 1, "la dernière liste lue reste");
        let (ok, msg) = a.message.clone().unwrap();
        assert!(!ok);
        assert!(msg.contains("illisible"), "{msg}");
    }

    #[test]
    fn removing_forgets_the_old_tokens_too() {
        use suite_accounts::Vault;
        let dir = tempfile::tempdir().unwrap();
        let old = Arc::new(MemoryVault::default());
        let mut a = Accounts::new(Store::new(dir.path(), Arc::new(MemoryVault::default())))
            .with_legacy_vaults(vec![old.clone()]);
        a.finish(ForgeKind::GitHub, "github.com", Ok(ada()), "t");
        let id = a.list()[0].id.clone();
        old.set(&id, "ancien").unwrap();
        a.remove(&id);
        assert!(a.list().is_empty());
        assert_eq!(old.get(&id).unwrap(), None, "plus aucune copie du jeton sur le poste");
    }

    #[test]
    fn azure_org_is_deduced_before_verification() {
        let (host, orgs) = prepare(ForgeKind::AzureDevOps, " https://dev-xelya.visualstudio.com/ ", vec![]);
        assert_eq!(host, "https://dev-xelya.visualstudio.com/");
        assert_eq!(orgs, ["dev-xelya"]);
        let (_, orgs) = prepare(ForgeKind::AzureDevOps, "https://dev-xelya.visualstudio.com/", vec!["autre".into()]);
        assert_eq!(orgs, ["autre"], "l'organisation saisie l'emporte");
        let (host, orgs) = prepare(ForgeKind::GitHub, "  ", vec![]);
        assert_eq!((host.as_str(), orgs.len()), ("github.com", 0));
    }

    #[test]
    fn a_verified_account_lands_in_the_common_store() {
        let (mut a, _dir) = accounts();
        let user = VerifiedUser {
            username: "camille@contoso.test".into(),
            display_name: "Camille Martin".into(),
            avatar_url: None,
            orgs: vec!["dev-xelya".into()],
        };
        a.finish(ForgeKind::AzureDevOps, "https://dev-xelya.visualstudio.com/", Ok(user), "jeton");
        let list = a.list().to_vec();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].forge, "azure");
        assert_eq!(list[0].host, "dev.azure.com");
        assert_eq!(title(&list[0]), "Azure DevOps · Camille Martin");
        assert_eq!(a.message.as_ref().map(|(ok, _)| *ok), Some(true));
        assert_eq!(a.store.token(&list[0].id).unwrap().as_deref(), Some("jeton"));
    }

    #[test]
    fn a_refused_token_stores_nothing_and_explains() {
        let (mut a, _dir) = accounts();
        a.finish(ForgeKind::AzureDevOps, "dev.azure.com", Err(ForgeError::Unauthorized), "jeton");
        assert!(a.list().is_empty());
        let (ok, msg) = a.message.clone().unwrap();
        assert!(!ok);
        assert!(msg.contains("organi"), "{msg}");
    }

    #[test]
    fn an_account_added_elsewhere_appears() {
        let (mut a, dir) = accounts();
        assert!(a.list().is_empty());
        let other = Store::new(dir.path(), Arc::new(MemoryVault::default()));
        let gh = suite_accounts::Account {
            id: "aaaaaaaaaaaaaaaa".into(),
            forge: "github".into(),
            host: "github.com".into(),
            username: "ada".into(),
            ..Default::default()
        };
        other.add(gh, "t").unwrap();
        a.next_check = None;
        assert_eq!(a.list().len(), 1);
        assert_eq!(title(&a.list()[0]), "GitHub · ada");
    }

    #[test]
    fn titles_name_the_host_only_when_unusual() {
        let mut acc = suite_accounts::Account {
            id: "x".into(),
            forge: "gitlab".into(),
            host: "gitlab.contoso.test".into(),
            username: "ada".into(),
            ..Default::default()
        };
        assert_eq!(title(&acc), "GitLab · ada (gitlab.contoso.test)");
        acc.host = "gitlab.com".into();
        assert_eq!(title(&acc), "GitLab · ada");
    }
}
