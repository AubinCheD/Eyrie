//! Reprise des données de SuperApp, l'ancien nom d'Eyrie : son dossier `%APPDATA%\SuperApp` devient
//! `%APPDATA%\EyrieSuite\Eyrie`, et ses comptes (coffre « SuperApp ») passent dans les comptes communs
//! de la suite, sous le même identifiant.

use std::path::Path;

use suite_accounts::{Account, Store, Vault};

/// Fait la reprise si elle n'a pas encore eu lieu. L'état : le dossier est renommé si celui d'Eyrie n'existe pas ;
/// sinon (renommage refusé, SuperApp encore ouvert, ou Eyrie démarré sur un dossier vide), `state.json` et `egui.ron`
/// sont copiés tant qu'Eyrie n'a pas d'état à lui. Les comptes : l'ancien `accounts.json`, dans le dossier d'Eyrie
/// après le renommage ou resté dans celui de SuperApp, est repris puis renommé `accounts.json.migre`. Vrai si quelque
/// chose a été repris. Les anciens jetons restent dans le coffre « SuperApp ».
pub fn from_superapp(old: &Path, new: &Path, old_vault: &dyn Vault, store: &Store) -> Result<bool, String> {
    let mut done = false;
    if old.is_dir() && !new.join("state.json").exists() {
        let moved = !new.exists()
            && new.parent().is_none_or(|p| std::fs::create_dir_all(p).is_ok())
            && std::fs::rename(old, new).is_ok();
        if !moved {
            std::fs::create_dir_all(new).map_err(|e| format!("{} : {e}", new.display()))?;
            for name in ["state.json", "egui.ron"] {
                let from = old.join(name);
                if from.is_file() {
                    std::fs::copy(&from, new.join(name)).map_err(|e| format!("{} : {e}", from.display()))?;
                }
            }
        }
        done = true;
    }
    for legacy in [new.join("accounts.json"), old.join("accounts.json")] {
        // Un Eyrie isolé range les comptes communs dans son propre dossier : ce fichier-là n'est pas l'ancien.
        if legacy.parent() == Some(store.dir()) || !legacy.is_file() {
            continue;
        }
        let text = std::fs::read_to_string(&legacy).map_err(|e| format!("{} : {e}", legacy.display()))?;
        let value: serde_json::Value =
            serde_json::from_str(&text).map_err(|e| format!("{} illisible : {e}", legacy.display()))?;
        for raw in value.get("accounts").and_then(|a| a.as_array()).cloned().unwrap_or_default() {
            let Ok(account) = serde_json::from_value::<Account>(raw) else { continue };
            let token = old_vault.get(&account.id)?;
            store.adopt(account, token)?;
        }
        let kept = legacy.with_file_name("accounts.json.migre");
        std::fs::rename(&legacy, &kept).map_err(|e| format!("{} : {e}", kept.display()))?;
        done = true;
    }
    Ok(done)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use suite_accounts::{MemoryVault, Store, Vault};

    use super::*;

    const LEGACY: &str = r#"{"accounts":[{"id":"e7d3af9721b2eb45","forge":"azure","host":"https://dev-xelya.visualstudio.com/",
        "username":"camille@contoso.test","shared_with":["dragontree","owlery","patronus"],"display_name":"Camille",
        "orgs":[],"source":"suite"}]}"#;

    #[test]
    fn superapp_folder_and_accounts_move_to_eyrie() {
        let root = tempfile::tempdir().unwrap();
        let old = root.path().join("SuperApp");
        std::fs::create_dir_all(&old).unwrap();
        std::fs::write(old.join("state.json"), "{}").unwrap();
        std::fs::write(old.join("accounts.json"), LEGACY).unwrap();
        let old_vault = MemoryVault::default();
        old_vault.set("e7d3af9721b2eb45", "jeton").unwrap();
        let suite = root.path().join("EyrieSuite");
        let new = suite.join("Eyrie");
        let store = Store::new(&suite, Arc::new(MemoryVault::default()));

        assert_eq!(from_superapp(&old, &new, &old_vault, &store), Ok(true));

        assert!(!old.exists());
        assert!(new.join("state.json").exists());
        assert!(!new.join("accounts.json").exists());
        assert!(new.join("accounts.json.migre").exists());
        let list = store.list();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, "e7d3af9721b2eb45");
        assert_eq!(list[0].host, "dev.azure.com");
        assert_eq!(list[0].orgs, ["dev-xelya"]);
        assert_eq!(store.token("e7d3af9721b2eb45").unwrap().as_deref(), Some("jeton"));
    }

    #[test]
    fn an_eyrie_in_use_keeps_its_state_but_takes_the_accounts() {
        let root = tempfile::tempdir().unwrap();
        let old = root.path().join("SuperApp");
        std::fs::create_dir_all(&old).unwrap();
        std::fs::write(old.join("state.json"), "superapp").unwrap();
        std::fs::write(old.join("accounts.json"), LEGACY).unwrap();
        let suite = root.path().join("EyrieSuite");
        let new = suite.join("Eyrie");
        std::fs::create_dir_all(&new).unwrap();
        std::fs::write(new.join("state.json"), "eyrie").unwrap();
        let store = Store::new(&suite, Arc::new(MemoryVault::default()));

        assert_eq!(from_superapp(&old, &new, &MemoryVault::default(), &store), Ok(true));

        assert_eq!(std::fs::read_to_string(new.join("state.json")).unwrap(), "eyrie", "l'état d'Eyrie est gardé");
        assert_eq!(store.list().len(), 1, "les comptes laissés par SuperApp sont repris");
        assert!(old.join("accounts.json.migre").exists());
        assert_eq!(from_superapp(&old, &new, &MemoryVault::default(), &store), Ok(false), "une seule fois");
    }

    /// Le déplacement du dossier a échoué (SuperApp encore ouvert…) et Eyrie a démarré sur un dossier vide : la
    /// reprise se fait au lancement suivant, par copie.
    #[test]
    fn a_superapp_folder_left_behind_is_still_taken_over() {
        let root = tempfile::tempdir().unwrap();
        let old = root.path().join("SuperApp");
        std::fs::create_dir_all(&old).unwrap();
        std::fs::write(old.join("state.json"), "superapp").unwrap();
        std::fs::write(old.join("egui.ron"), "()").unwrap();
        std::fs::write(old.join("accounts.json"), LEGACY).unwrap();
        let old_vault = MemoryVault::default();
        old_vault.set("e7d3af9721b2eb45", "jeton").unwrap();
        let suite = root.path().join("EyrieSuite");
        let new = suite.join("Eyrie");
        std::fs::create_dir_all(&new).unwrap();
        let store = Store::new(&suite, Arc::new(MemoryVault::default()));

        assert_eq!(from_superapp(&old, &new, &old_vault, &store), Ok(true));

        assert_eq!(std::fs::read_to_string(new.join("state.json")).unwrap(), "superapp");
        assert!(new.join("egui.ron").exists());
        assert_eq!(store.token("e7d3af9721b2eb45").unwrap().as_deref(), Some("jeton"));
        assert!(old.join("accounts.json.migre").exists());
    }

    #[test]
    fn nothing_happens_without_superapp() {
        let root = tempfile::tempdir().unwrap();
        let suite = root.path().join("EyrieSuite");
        let store = Store::new(&suite, Arc::new(MemoryVault::default()));
        let r = from_superapp(&root.path().join("SuperApp"), &suite.join("Eyrie"), &MemoryVault::default(), &store);
        assert_eq!(r, Ok(false));
        assert!(!suite.join("Eyrie").exists());
    }
}
