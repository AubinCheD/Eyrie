//! Le fichier des comptes et le coffre des jetons.
//!
//! Plusieurs applications (Eyrie, DragonTree et Owlery seuls) peuvent écrire en même temps : chaque
//! écriture prend le verrou de Windows sur `accounts.lock` (rendu par Windows si l'application meurt),
//! relit le fichier, et le remplace d'un coup (fichier temporaire puis renommage). La lecture seule
//! n'attend personne.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use serde_json::{Map, Value};

use crate::Account;

const FILE: &str = "accounts.json";
const VERSION: u64 = 1;
const LOCK: &str = "accounts.lock";
/// Attente du verrou d'une autre application, au plus.
const LOCK_WAIT: Duration = Duration::from_secs(5);

/// Le verrou des écritures : celui de Windows sur le fichier ouvert, rendu à la fin de la portée (ou par Windows, si
/// l'application meurt pendant l'écriture). Le fichier reste : l'effacer laisserait deux applications verrouiller
/// chacune le sien.
struct Lock(#[allow(dead_code)] std::fs::File);

/// Un coffre de jetons, indexé par identifiant de compte : celui de Windows, ou en mémoire pour les tests.
pub trait Vault: Send + Sync {
    fn get(&self, id: &str) -> Result<Option<String>, String>;
    fn set(&self, id: &str, token: &str) -> Result<(), String>;
    fn delete(&self, id: &str) -> Result<(), String>;
}

/// Le Gestionnaire d'identifiants de Windows : service donné, utilisateur `account:<id>`.
pub struct OsVault {
    service: String,
}

impl OsVault {
    pub fn new(service: &str) -> OsVault {
        OsVault { service: service.to_string() }
    }

    /// Le coffre commun de la suite (service « EyrieSuite »).
    pub fn suite() -> OsVault {
        OsVault::new(crate::SUITE)
    }

    fn entry(&self, id: &str) -> Result<keyring::Entry, String> {
        let id = id.trim();
        if id.is_empty() {
            return Err("identifiant de compte vide".into());
        }
        keyring::Entry::new(&self.service, &format!("account:{id}")).map_err(|e| e.to_string())
    }
}

impl Vault for OsVault {
    fn get(&self, id: &str) -> Result<Option<String>, String> {
        match self.entry(id)?.get_password() {
            Ok(token) => Ok(Some(token)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }

    fn set(&self, id: &str, token: &str) -> Result<(), String> {
        self.entry(id)?.set_password(token).map_err(|e| e.to_string())
    }

    fn delete(&self, id: &str) -> Result<(), String> {
        match self.entry(id)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }
}

/// Coffre en mémoire, pour les tests et les démonstrations (jamais les vrais jetons).
#[derive(Default)]
pub struct MemoryVault(Mutex<BTreeMap<String, String>>);

impl Vault for MemoryVault {
    fn get(&self, id: &str) -> Result<Option<String>, String> {
        Ok(self.0.lock().unwrap_or_else(|e| e.into_inner()).get(id).cloned())
    }

    fn set(&self, id: &str, token: &str) -> Result<(), String> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).insert(id.to_string(), token.to_string());
        Ok(())
    }

    fn delete(&self, id: &str) -> Result<(), String> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).remove(id);
        Ok(())
    }
}

/// Les comptes de la suite : `<dossier>\accounts.json` et le coffre.
#[derive(Clone)]
pub struct Store {
    dir: PathBuf,
    vault: Arc<dyn Vault>,
}

/// Le fichier lu : les comptes lisibles, ceux d'une autre version (réécrits tels quels), le reste.
#[derive(Default)]
struct Parsed {
    accounts: Vec<Account>,
    unreadable: Vec<Value>,
    rest: Map<String, Value>,
}

impl Store {
    /// Les comptes communs : dossier de [`crate::accounts_dir`] (sans profil isolé), coffre « EyrieSuite ».
    pub fn open() -> Store {
        Store::new(&crate::accounts_dir(None), Arc::new(OsVault::suite()))
    }

    pub fn new(dir: &Path, vault: Arc<dyn Vault>) -> Store {
        Store { dir: dir.to_path_buf(), vault }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn path(&self) -> PathBuf {
        self.dir.join(FILE)
    }

    /// Les comptes, normalisés, sans doublon (le premier d'une même connexion est gardé). Fichier absent
    /// ou illisible : aucun (voir [`Store::try_list`] pour distinguer les deux).
    pub fn list(&self) -> Vec<Account> {
        self.try_list().unwrap_or_default()
    }

    /// Les comptes ; un fichier illisible est une erreur, et non « aucun compte » : une application garde
    /// alors ceux qu'elle avait lus et le dit.
    pub fn try_list(&self) -> Result<Vec<Account>, String> {
        self.read().map(|p| p.accounts)
    }

    /// Date de dernière écriture du fichier : une application la compare pour relire les comptes
    /// quand une autre les a changés.
    pub fn stamp(&self) -> Option<SystemTime> {
        std::fs::metadata(self.path()).and_then(|m| m.modified()).ok()
    }

    /// Le jeton d'un compte.
    pub fn token(&self, id: &str) -> Result<Option<String>, String> {
        self.vault.get(id)
    }

    /// Ajoute un compte vérifié et son jeton. La même connexion déjà là garde son identifiant et prend les
    /// nouvelles informations et le nouveau jeton. Renvoie le compte enregistré.
    pub fn add(&self, account: Account, token: &str) -> Result<Account, String> {
        let token = token.trim();
        if token.is_empty() {
            return Err("le jeton est vide".into());
        }
        let _lock = self.lock()?;
        let mut file = self.read()?;
        let mut account = account.normalized();
        match file.accounts.iter_mut().find(|a| a.same_login(&account)) {
            Some(existing) => {
                account.id = existing.id.clone();
                let mut extra = std::mem::take(&mut existing.extra);
                extra.append(&mut account.extra);
                account.extra = extra;
                *existing = account.clone();
            }
            None => file.accounts.push(account.clone()),
        }
        // Le jeton d'abord : la liste ne nomme jamais un compte sans jeton.
        self.vault.set(&account.id, token)?;
        self.write(&file)?;
        Ok(account)
    }

    /// Remplace le jeton d'un compte connu (la reprise des comptes de DragonTree garde le sien).
    pub fn set_token(&self, id: &str, token: &str) -> Result<(), String> {
        let token = token.trim();
        if token.is_empty() {
            return Err("le jeton est vide".into());
        }
        self.vault.set(id, token)
    }

    /// Met à jour les informations d'un compte (nom affiché, organisations…), sans toucher au jeton. La forge, l'hôte
    /// et l'utilisateur ne changent pas : le jeton partirait vers un autre serveur à la prochaine requête.
    pub fn update(&self, account: &Account) -> Result<(), String> {
        let _lock = self.lock()?;
        let mut file = self.read()?;
        let Some(existing) = file.accounts.iter_mut().find(|a| a.id == account.id) else {
            return Err(format!("compte inconnu : {}", account.id));
        };
        if !existing.same_login(account) {
            return Err(format!(
                "la forge, l'hôte et l'utilisateur d'un compte ne changent pas (son jeton irait à un autre serveur) : \
                 retirez le compte « {} » et ajoutez-le de nouveau, avec un jeton",
                existing.username
            ));
        }
        // Les champs écrits par une autre version restent.
        let mut updated = account.clone().normalized();
        let mut extra = std::mem::take(&mut existing.extra);
        extra.append(&mut updated.extra);
        updated.extra = extra;
        *existing = updated;
        self.write(&file)
    }

    /// Retire un compte et son jeton.
    pub fn remove(&self, id: &str) -> Result<(), String> {
        let _lock = self.lock()?;
        let mut file = self.read()?;
        file.accounts.retain(|a| a.id != id);
        self.write(&file)?;
        self.vault.delete(id)
    }

    /// Reprend un compte d'une application (migration) : déjà là sous le même identifiant ou la même
    /// connexion, c'est l'identifiant existant qui est renvoyé (le jeton n'est copié que s'il manque) ;
    /// sinon le compte est ajouté sous son identifiant. Renvoie l'identifiant à utiliser désormais.
    pub fn adopt(&self, account: Account, token: Option<String>) -> Result<String, String> {
        let account = account.normalized();
        let _lock = self.lock()?;
        let mut file = self.read()?;
        let existing = file
            .accounts
            .iter()
            .find(|a| a.id == account.id)
            .or_else(|| file.accounts.iter().find(|a| a.same_login(&account)))
            .map(|a| a.id.clone());
        let id = existing.clone().unwrap_or_else(|| account.id.clone());
        if let Some(token) = token.as_deref().map(str::trim).filter(|t| !t.is_empty())
            && self.vault.get(&id)?.is_none()
        {
            self.vault.set(&id, token)?;
        }
        if existing.is_none() {
            file.accounts.push(account);
            self.write(&file)?;
        }
        Ok(id)
    }

    /// Le verrou des écritures ; une autre application le tient : on attend un peu, puis on renonce.
    fn lock(&self) -> Result<Lock, String> {
        std::fs::create_dir_all(&self.dir).map_err(|e| format!("{} : {e}", self.dir.display()))?;
        let path = self.dir.join(LOCK);
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
            .map_err(|e| format!("{} : {e}", path.display()))?;
        let started = Instant::now();
        loop {
            match file.try_lock() {
                Ok(()) => return Ok(Lock(file)),
                Err(std::fs::TryLockError::WouldBlock) if started.elapsed() <= LOCK_WAIT => {
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(std::fs::TryLockError::WouldBlock) => {
                    return Err(format!("{} : comptes occupés par une autre application", self.path().display()));
                }
                Err(std::fs::TryLockError::Error(e)) => return Err(format!("{} : {e}", path.display())),
            }
        }
    }

    fn read(&self) -> Result<Parsed, String> {
        let text = match std::fs::read_to_string(self.path()) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Parsed::default()),
            Err(e) => return Err(format!("{} : {e}", self.path().display())),
        };
        let Value::Object(mut rest) = serde_json::from_str::<Value>(text.trim_start_matches('\u{feff}'))
            .map_err(|e| format!("{} illisible : {e}", self.path().display()))?
        else {
            return Err(format!("{} illisible : pas un objet JSON", self.path().display()));
        };
        let mut file = Parsed::default();
        let raw = match rest.remove("accounts") {
            Some(Value::Array(list)) => list,
            _ => Vec::new(),
        };
        for value in raw {
            match serde_json::from_value::<Account>(value.clone()) {
                Ok(a) => {
                    let a = a.normalized();
                    if !file.accounts.iter().any(|k| k.id == a.id || k.same_login(&a)) {
                        file.accounts.push(a);
                    }
                }
                Err(_) => file.unreadable.push(value),
            }
        }
        file.rest = rest;
        Ok(file)
    }

    fn write(&self, file: &Parsed) -> Result<(), String> {
        let mut top = file.rest.clone();
        top.insert("version".into(), VERSION.into());
        let mut list: Vec<Value> = file
            .accounts
            .iter()
            .map(|a| serde_json::to_value(a).map_err(|e| e.to_string()))
            .collect::<Result<_, _>>()?;
        list.extend(file.unreadable.iter().cloned());
        top.insert("accounts".into(), Value::Array(list));
        let text = serde_json::to_string_pretty(&Value::Object(top)).map_err(|e| e.to_string())?;
        std::fs::create_dir_all(&self.dir).map_err(|e| format!("{} : {e}", self.dir.display()))?;
        let tmp = self.dir.join(format!("{FILE}.{}.tmp", std::process::id()));
        // Sur le disque avant le renommage : une coupure ne laisse jamais un fichier vide.
        let written = std::fs::File::create(&tmp).and_then(|mut f| {
            std::io::Write::write_all(&mut f, text.as_bytes())?;
            f.sync_all()
        });
        written.map_err(|e| format!("{} : {e}", tmp.display()))?;
        std::fs::rename(&tmp, self.path()).map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            format!("{} : {e}", self.path().display())
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::{Account, forge};

    fn store(dir: &std::path::Path) -> (Store, Arc<MemoryVault>) {
        let vault = Arc::new(MemoryVault::default());
        (Store::new(dir, vault.clone()), vault)
    }

    fn github(id: &str, host: &str, username: &str) -> Account {
        Account {
            id: id.into(),
            forge: forge::GITHUB.into(),
            host: host.into(),
            username: username.into(),
            ..Account::default()
        }
    }

    #[test]
    fn an_unreadable_file_is_reported_not_taken_for_an_empty_one() {
        let dir = tempfile::tempdir().unwrap();
        let (s, _) = store(dir.path());
        assert_eq!(s.try_list(), Ok(Vec::new()), "pas de fichier : aucun compte, sans erreur");
        std::fs::write(dir.path().join("accounts.json"), "{ pas du json").unwrap();
        let err = s.try_list().unwrap_err();
        assert!(err.contains("illisible"), "{err}");
    }

    #[test]
    fn set_token_replaces_the_token_of_a_known_account() {
        let dir = tempfile::tempdir().unwrap();
        let (s, _) = store(dir.path());
        let a = s.add(github("aaaaaaaaaaaaaaaa", "github.com", "ada"), "lecture").unwrap();
        s.set_token(&a.id, " ecriture ").unwrap();
        assert_eq!(s.token(&a.id).unwrap().as_deref(), Some("ecriture"));
        assert!(s.set_token(&a.id, "  ").is_err(), "un jeton vide est refusé");
    }

    #[test]
    fn concurrent_writers_lose_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let vault: Arc<MemoryVault> = Arc::new(MemoryVault::default());
        let writers: Vec<_> = (0..2)
            .map(|w| {
                let s = Store::new(dir.path(), vault.clone());
                std::thread::spawn(move || {
                    for i in 0..25 {
                        let id = format!("{w:x}{i:015x}");
                        s.add(github(&id, "github.com", &format!("u{w}-{i}")), "t").unwrap();
                    }
                })
            })
            .collect();
        for w in writers {
            w.join().unwrap();
        }
        assert_eq!(Store::new(dir.path(), vault).list().len(), 50, "aucune écriture perdue");
        let lock = std::fs::File::options().write(true).open(dir.path().join("accounts.lock")).unwrap();
        assert!(lock.try_lock().is_ok(), "le verrou est rendu");
    }

    /// Une application qui tient le verrou fait attendre les autres ; dès qu'elle le rend (ou que Windows le rend
    /// pour elle, si elle meurt), l'écriture suivante passe.
    #[test]
    fn a_released_lock_is_taken_at_once() {
        let dir = tempfile::tempdir().unwrap();
        let (s, _) = store(dir.path());
        let held = std::fs::File::options()
            .create(true)
            .truncate(false)
            .write(true)
            .open(dir.path().join("accounts.lock"))
            .unwrap();
        held.try_lock().unwrap();
        let release = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(200));
            drop(held);
        });
        let started = std::time::Instant::now();
        s.add(github("aaaaaaaaaaaaaaaa", "github.com", "ada"), "t").unwrap();
        assert!(started.elapsed() < std::time::Duration::from_secs(2), "pris dès qu'il est rendu");
        release.join().unwrap();
        assert_eq!(s.list().len(), 1);
    }

    #[test]
    fn a_held_lock_makes_a_writer_wait_then_fail() {
        let dir = tempfile::tempdir().unwrap();
        let (s, _) = store(dir.path());
        let held = std::fs::File::options()
            .create(true)
            .truncate(false)
            .write(true)
            .open(dir.path().join("accounts.lock"))
            .unwrap();
        held.try_lock().unwrap();
        let e = s.add(github("aaaaaaaaaaaaaaaa", "github.com", "ada"), "t").unwrap_err();
        assert!(e.contains("occupés"), "{e}");
        assert!(s.list().is_empty());
    }

    /// Un fichier de verrou resté sur le disque (application tuée) ne bloque rien : seul le verrou de Windows compte.
    #[test]
    fn a_lock_file_left_behind_does_not_block() {
        let dir = tempfile::tempdir().unwrap();
        let (s, _) = store(dir.path());
        let lock = dir.path().join("accounts.lock");
        std::fs::write(&lock, "").unwrap();
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(60);
        std::fs::File::options().write(true).open(&lock).unwrap().set_modified(old).unwrap();
        s.add(github("aaaaaaaaaaaaaaaa", "github.com", "ada"), "t").unwrap();
        assert_eq!(s.list().len(), 1);
    }

    #[test]
    fn update_keeps_unknown_fields() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("accounts.json"),
            r#"{"accounts":[{"id":"aaaaaaaaaaaaaaaa","forge":"github","host":"github.com","username":"ada","scopes":["repo"]}]}"#,
        )
        .unwrap();
        let (s, _) = store(dir.path());
        s.update(&Account { display_name: "Ada".into(), ..github("aaaaaaaaaaaaaaaa", "github.com", "ada") }).unwrap();
        let a = &s.list()[0];
        assert_eq!(a.display_name, "Ada");
        assert_eq!(a.extra["scopes"][0], "repo", "écrit par une autre version : gardé");
    }

    #[test]
    fn legacy_superapp_file_is_read_and_repaired() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("accounts.json"),
            r#"{"accounts":[{"id":"667e99e1d28de395","forge":"github","host":"https://github.com/","username":"AubinCheD",
                "shared_with":["dragontree","owlery","patronus"],"display_name":"","orgs":[],"source":"suite"}]}"#,
        )
        .unwrap();
        let (s, _) = store(dir.path());
        let list = s.list();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].host, "github.com");
        assert_eq!(list[0].username, "AubinCheD");
        assert!(!list[0].extra.contains_key("shared_with"), "{:?}", list[0].extra);
        assert!(!list[0].extra.contains_key("source"), "{:?}", list[0].extra);
    }

    #[test]
    fn adding_the_same_login_twice_keeps_one_account() {
        let dir = tempfile::tempdir().unwrap();
        let (s, _) = store(dir.path());
        let first = s.add(github("aaaaaaaaaaaaaaaa", "github.com", "ada"), "t1").unwrap();
        let mut again = github("bbbbbbbbbbbbbbbb", "https://GitHub.com", "ADA");
        again.display_name = "Ada Lovelace".into();
        let second = s.add(again, "t2").unwrap();
        assert_eq!(second.id, first.id);
        let list = s.list();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].display_name, "Ada Lovelace");
        assert_eq!(s.token(&first.id).unwrap().as_deref(), Some("t2"));
    }

    #[test]
    fn an_empty_token_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let (s, _) = store(dir.path());
        assert!(s.add(github("aaaaaaaaaaaaaaaa", "github.com", "ada"), " \n").is_err());
        assert!(s.list().is_empty());
    }

    #[test]
    fn remove_forgets_list_and_token() {
        let dir = tempfile::tempdir().unwrap();
        let (s, _) = store(dir.path());
        let a = s.add(github("aaaaaaaaaaaaaaaa", "github.com", "ada"), "t1").unwrap();
        s.remove(&a.id).unwrap();
        assert!(s.list().is_empty());
        assert_eq!(s.token(&a.id).unwrap(), None);
    }

    #[test]
    fn update_changes_the_information_but_not_the_token() {
        let dir = tempfile::tempdir().unwrap();
        let (s, _) = store(dir.path());
        let mut a = s.add(github("aaaaaaaaaaaaaaaa", "github.com", "ada"), "t1").unwrap();
        a.display_name = "Ada".into();
        a.avatar_url = Some("https://avatars.example/ada".into());
        s.update(&a).unwrap();
        assert_eq!(s.list()[0].display_name, "Ada");
        assert_eq!(s.token(&a.id).unwrap().as_deref(), Some("t1"));
    }

    /// Le jeton d'un compte vaut pour sa forge, son hôte et son utilisateur : les changer par une mise à jour
    /// l'enverrait à un autre serveur. Il faut retirer le compte et l'ajouter avec un jeton.
    #[test]
    fn update_never_moves_a_token_to_another_host() {
        let dir = tempfile::tempdir().unwrap();
        let (s, _) = store(dir.path());
        let a = s.add(github("aaaaaaaaaaaaaaaa", "github.com", "ada"), "t1").unwrap();
        for changed in [
            Account { host: "github.attaquant.example".into(), ..a.clone() },
            Account { forge: forge::GITLAB.into(), ..a.clone() },
            Account { username: "grace".into(), ..a.clone() },
        ] {
            assert!(s.update(&changed).is_err(), "{changed:?}");
        }
        assert_eq!(s.list()[0].host, "github.com");
        // Même connexion écrite autrement : acceptée.
        s.update(&Account { host: "https://GitHub.com/".into(), username: " Ada ".into(), ..a.clone() }).unwrap();
    }

    #[test]
    fn adopt_keeps_ids_and_maps_duplicates() {
        let dir = tempfile::tempdir().unwrap();
        let (s, _) = store(dir.path());
        let x = github("aaaaaaaaaaaaaaaa", "github.com", "ada");
        assert_eq!(s.adopt(x.clone(), Some("t".into())).unwrap(), x.id);
        let y = github("bbbbbbbbbbbbbbbb", "https://github.com/", "Ada");
        assert_eq!(s.adopt(y, Some("u".into())).unwrap(), x.id);
        assert_eq!(s.list().len(), 1);
        assert_eq!(s.token(&x.id).unwrap().as_deref(), Some("t"), "le jeton déjà là est gardé");
        // Même identifiant, jeton absent du coffre commun : celui de la migration le complète.
        let z = github("cccccccccccccccc", "gitlab.contoso.test", "grace");
        s.adopt(z.clone(), None).unwrap();
        assert_eq!(s.adopt(z.clone(), Some("v".into())).unwrap(), z.id);
        assert_eq!(s.token(&z.id).unwrap().as_deref(), Some("v"));
    }

    #[test]
    fn unknown_fields_survive_a_write() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("accounts.json"),
            r#"{"version":1,"futur":{"a":1},"accounts":[{"id":"aaaaaaaaaaaaaaaa","forge":"gitlab","host":"gitlab.com",
                "username":"ada","scopes":["read_api"]}]}"#,
        )
        .unwrap();
        let (s, _) = store(dir.path());
        s.add(github("bbbbbbbbbbbbbbbb", "github.com", "grace"), "t").unwrap();
        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(dir.path().join("accounts.json")).unwrap()).unwrap();
        assert_eq!(v["futur"]["a"], 1);
        assert_eq!(v["version"], 1);
        assert_eq!(v["accounts"][0]["scopes"][0], "read_api");
        assert_eq!(v["accounts"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn stamp_changes_after_a_write() {
        let dir = tempfile::tempdir().unwrap();
        let (s, _) = store(dir.path());
        assert_eq!(s.stamp(), None);
        s.add(github("aaaaaaaaaaaaaaaa", "github.com", "ada"), "t").unwrap();
        let first = s.stamp();
        assert!(first.is_some());
        std::thread::sleep(std::time::Duration::from_millis(20));
        s.add(github("bbbbbbbbbbbbbbbb", "github.com", "grace"), "t").unwrap();
        assert_ne!(s.stamp(), first);
    }

    #[test]
    fn a_corrupt_file_is_never_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("accounts.json"), "{ pas du json").unwrap();
        let (s, _) = store(dir.path());
        assert!(s.list().is_empty());
        assert!(s.add(github("aaaaaaaaaaaaaaaa", "github.com", "ada"), "t").is_err());
        assert_eq!(std::fs::read_to_string(dir.path().join("accounts.json")).unwrap(), "{ pas du json");
    }

    #[test]
    fn two_stores_on_the_same_folder_see_each_other() {
        let dir = tempfile::tempdir().unwrap();
        let vault = Arc::new(MemoryVault::default());
        let eyrie = Store::new(dir.path(), vault.clone());
        let dragontree = Store::new(dir.path(), vault);
        eyrie.add(github("aaaaaaaaaaaaaaaa", "github.com", "ada"), "t").unwrap();
        assert_eq!(dragontree.list().len(), 1);
        assert_eq!(dragontree.token("aaaaaaaaaaaaaaaa").unwrap().as_deref(), Some("t"));
    }
}
