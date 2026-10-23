# Eyrie : nom, comptes communs et réglages — plan d'exécution

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** SuperApp devient Eyrie ; les comptes de forge sont communs à toute la suite (EyrieSuite), qu'un module tourne seul ou dans Eyrie ; les réglages d'Eyrie sont refaits, courts et sans doublons.

**Architecture:** une petite crate `suite-accounts` (dans ce dépôt) porte le format, le dossier et le coffre communs ; Eyrie, DragonTree et Owlery la lisent et l'écrivent directement, seuls comme intégrés, et relisent le fichier quand il change. Un module intégré le sait (`set_hosted` / `set_accounts_link`) : ses fenêtres Comptes renvoient aux réglages d'Eyrie, ses réglages de thème et de langue (imposés par Eyrie) sont masqués.

**Tech Stack:** Rust 2024, egui/eframe 0.37 (toolchain GNU, `.cargo/config.toml` local), `keyring` 4, `serde_json`.

**Spec :** pas de document de conception (questions sautées le 23/10/2026) ; les exigences retenues sont ci-dessous.

## Exigences (conversation du 23/10/2026)

1. Nom : l'application s'appelle **Eyrie** ; ce qui est commun à la suite s'appelle **EyrieSuite** (`%APPDATA%\EyrieSuite`, service « EyrieSuite » du Gestionnaire d'identifiants, à terme le dossier d'installation).
2. Les adresses des comptes sont normalisées.
3. DragonTree hérite des comptes d'Eyrie ; plus généralement, l'utilisateur se connecte **une fois**, dans un module ou dans Eyrie, et il est connecté partout, y compris dans une application installée plus tard.
4. Un module sait s'il est autonome ou intégré ; passer d'un mode à l'autre est transparent.
5. Dans Eyrie, les comptes se règlent à un seul endroit (plus de doublon dans les réglages des modules).
6. Patronus ne reçoit plus de comptes (il n'en utilise pas).
7. Passe complète sur les réglages d'Eyrie : plus courts, orientés utilisateur.

## Global Constraints

- Code dans `..\DragonTree` et `..\Owlery` : branche `suite/module` (accord du 23/10). Fawkes, Patronus, Menagerie, Grimoire : **aucune** retouche.
- **Aucun commit** sans demande de l'utilisateur : les étapes « Commit » sont remplacées par « tests verts, diff relu ».
- Textes d'interface en français ; tout texte `tr(...)` d'Eyrie a son entrée dans `crates/eyrie/locales/en.json` (le test `catalog_matches_sources` refuse les manquantes et les orphelines) ; DragonTree passe par `tr!` et son propre catalogue.
- Jamais de jeton affiché, journalisé ni écrit hors du coffre.
- Les tests ne touchent jamais au vrai coffre ni à `%APPDATA%` : coffre en mémoire et dossiers temporaires.
- Style : celui du code voisin (commentaires `//!` / `///` en français, identifiants en anglais).

## Review Focus

- Compte déjà présent sous la même connexion (même forge, hôte normalisé, identifiant sans casse) ajouté une seconde fois, ailleurs : un seul compte, l'identifiant existant gardé, le nouveau jeton retenu.
- Ancien `accounts.json` de SuperApp (champs `shared_with`, `source`, hôte `https://github.com/`) : relu, hôte réparé, pas de doublon, rien de perdu.
- Hôte Azure `https://dev-xelya.visualstudio.com/` sans organisation : hôte `dev.azure.com`, organisation `dev-xelya`.
- Deux applications ouvertes en même temps (DragonTree seul et Eyrie) : un compte ajouté dans l'une apparaît dans l'autre sans relance.
- DragonTree : associations dépôt → compte gardées après la migration, même quand le compte existait déjà dans EyrieSuite sous un autre identifiant.

---

### Task 1 : crate `suite-accounts`

**Files:**
- Create: `crates/suite-accounts/Cargo.toml`, `crates/suite-accounts/src/lib.rs` (modèle, normalisation, dossier), `crates/suite-accounts/src/store.rs` (fichier + coffre)
- Modify: `Cargo.toml` (membre et `[workspace.dependencies] suite-accounts = { path = "crates/suite-accounts" }`)

**Interfaces (Produces):**
- `pub const SUITE: &str = "EyrieSuite";` `pub const DIR_ENV: &str = "EYRIESUITE_DIR";`
- `pub fn suite_dir() -> PathBuf` : `EYRIESUITE_DIR` s'il est posé et non vide, sinon `dirs::config_dir()/EyrieSuite`.
- `pub fn accounts_dir(isolated: Option<&Path>) -> PathBuf` : `EYRIESUITE_DIR`, sinon `isolated` (profil d'essai ou de test de l'application), sinon `suite_dir()`.
- `pub mod forge { pub const GITHUB: &str = "github"; pub const GITLAB: &str = "gitlab"; pub const AZURE: &str = "azure"; }`
- `pub struct Account { pub id, pub forge: String, pub host, pub username, pub display_name, pub orgs: Vec<String>, pub avatar_url: Option<String>, #[serde(flatten)] pub extra: BTreeMap<String, Value> }` (Serialize, Deserialize, Clone, Debug, PartialEq ; champs absents → défaut).
- `pub fn normalize_host(forge: &str, host: &str) -> String` ; `pub fn azure_org_from_host(host: &str) -> Option<String>` ; `impl Account { pub fn normalized(self) -> Account; pub fn same_login(&self, other: &Account) -> bool }`.
- `pub trait Vault: Send + Sync { fn get(&self, id: &str) -> Result<Option<String>, String>; fn set(&self, id: &str, token: &str) -> Result<(), String>; fn delete(&self, id: &str) -> Result<(), String>; }` ; `pub struct OsVault` (`OsVault::new(service)`, `OsVault::suite()`, entrée `account:<id>`) ; `pub struct MemoryVault` (Default).
- `pub struct Store` : `Store::open() -> Store` (= `Store::new(&accounts_dir(None), Arc::new(OsVault::suite()))`), `Store::new(dir: &Path, vault: Arc<dyn Vault>) -> Store`, `list(&self) -> Vec<Account>`, `stamp(&self) -> Option<SystemTime>`, `add(&self, account: Account, token: &str) -> Result<Account, String>`, `update(&self, account: &Account) -> Result<(), String>`, `remove(&self, id: &str) -> Result<(), String>`, `token(&self, id: &str) -> Result<Option<String>, String>`, `adopt(&self, account: Account, token: Option<String>) -> Result<String, String>`, `dir(&self) -> &Path`.

Règles : `normalize_host` retire schéma, chemin, `user@`, met en minuscules ; vide → hôte par défaut de la forge (`github.com`, `gitlab.com`, `dev.azure.com`) ; Azure `*.visualstudio.com` → `dev.azure.com`. `normalized()` applique `normalize_host`, ajoute l'organisation tirée de `<org>.visualstudio.com` si `orgs` est vide, et retire les champs hérités `shared_with` et `source`. Fichier `<dir>\accounts.json` = `{"version":1,"accounts":[…]}`, clés inconnues gardées, écrit de façon atomique (fichier temporaire puis renommage) après relecture (lecture-modification-écriture). `add` : même connexion déjà là → ses infos et son jeton sont remplacés, son `id` gardé. `adopt` (migration) : même `id` ou même connexion déjà là → renvoie l'`id` existant (jeton copié seulement s'il manque) ; sinon ajouté sous son `id`.

- [ ] **Step 1 : tests en échec** (`lib.rs` et `store.rs`, `#[cfg(test)]`, `tempfile` en dev-dependency, `MemoryVault`) :
  - `hosts_are_normalized` : `normalize_host("github", " HTTPS://GitHub.com/ ") == "github.com"` ; `normalize_host("gitlab", "https://moi@git.contoso.test:8443/x") == "git.contoso.test:8443"` ; `normalize_host("azure", "https://dev-xelya.visualstudio.com/") == "dev.azure.com"` ; `normalize_host("azure", "") == "dev.azure.com"`.
  - `azure_org_comes_from_the_old_host` : compte `azure`, hôte `https://dev-xelya.visualstudio.com/`, `orgs` vide → `normalized()` donne hôte `dev.azure.com`, `orgs == ["dev-xelya"]` ; avec `orgs == ["autre"]` : inchangées.
  - `legacy_superapp_file_is_read_and_repaired` : `accounts.json` écrit à la main avec `shared_with`, `source`, hôte `https://github.com/` → `list()` : un compte, hôte `github.com`, `extra` sans `shared_with` ni `source`.
  - `adding_the_same_login_twice_keeps_one_account` : `add(a, "t1")` puis `add(a' (autre id, hôte « https://GitHub.com », username « ADA »), "t2")` → `list().len() == 1`, id du premier, `token(id) == Some("t2")`.
  - `remove_forgets_list_and_token`.
  - `adopt_keeps_ids_and_maps_duplicates` : `adopt(x, Some("t"))` → `x.id` ; `adopt(y même connexion, autre id, Some("u"))` → renvoie `x.id`, jeton toujours `"t"`.
  - `unknown_fields_survive_a_write` : clé inconnue au premier niveau et dans un compte, gardées après `add`.
  - `stamp_changes_after_a_write`.
- [ ] **Step 2 :** `cargo test -p suite-accounts` → échec (rien n'est défini).
- [ ] **Step 3 :** implémenter.
- [ ] **Step 4 :** `cargo test -p suite-accounts` → tous verts.

### Task 2 : SuperApp devient Eyrie

**Files:**
- Move: `crates/superapp` → `crates/eyrie` (`git mv`), paquet `eyrie`, exécutable `eyrie.exe`.
- Modify: `Cargo.toml` (membres, `authors = ["Eyrie"]`), `crates/eyrie/src/{main.rs,app.rs,persist.rs,capture.rs,sandbox.rs,tailnet.rs,tests.rs,tests_deep.rs}`, `crates/suite-sdk/src/{host.rs,embed.rs}`, commentaires de `crates/modules/{patronus,menagerie}`, `README.md`, `docs/*.md` (nom du produit ; l'historique daté reste tel quel).
- Create: `crates/eyrie/src/migrate.rs`.

**Interfaces:**
- `app::TITLE = "Eyrie"` ; `with_app_id("eyrie")` ; verrou `Local\Eyrie-instance:<clé>`.
- Variables : `EYRIE_DATA_DIR`, `EYRIE_CAPTURE`, `EYRIE_CAPTURE_AFTER_MS`, `EYRIE_CAPTURE_SCENE` (remplacent `SUPERAPP_*`).
- `persist::data_dir()` : `EYRIE_DATA_DIR`, sinon `suite_accounts::suite_dir().join("Eyrie")`. `persist::is_default_dir(&Path) -> bool`.
- `migrate::from_superapp(old: &Path, new: &Path, old_vault: &dyn Vault, store: &Store) -> Result<bool, String>` : si `new` n'existe pas et `old` existe, renomme `old` en `new` ; puis si `new\accounts.json` existe (format SuperApp), `adopt` chaque compte avec le jeton de `old_vault`, et renomme le fichier en `accounts.json.migre`. Vrai si quelque chose a été repris.
- `main` : avant le moindre fil, si `EYRIESUITE_DIR` n'est pas posé, le pose (`suite_dir()` si le dossier d'Eyrie est celui par défaut, sinon le dossier d'Eyrie lui-même : un Eyrie isolé garde des comptes isolés) ; appelle `migrate::from_superapp(%APPDATA%\SuperApp, data_dir(), OsVault::new("SuperApp"), Store::open())` seulement si le dossier est celui par défaut.

- [ ] **Step 1 : tests en échec** (`migrate.rs`) : `superapp_folder_and_accounts_move_to_eyrie` (dossiers temporaires, `MemoryVault` ancien et nouveau : `state.json` déplacé, compte repris avec son id et son jeton, hôte normalisé, `accounts.json.migre` présent) ; `nothing_happens_when_eyrie_already_exists`.
- [ ] **Step 2 :** `cargo test -p eyrie migrate` → échec.
- [ ] **Step 3 :** renommage + migration.
- [ ] **Step 4 :** `cargo test --workspace` → vert ; `grep -ri "superapp" crates Cargo.toml` → seulement `migrate.rs` (ancien dossier et ancien coffre) et le test qui l'exerce.

### Task 3 : les comptes d'Eyrie sur le stockage commun

**Files:**
- Modify: `crates/eyrie/src/accounts.rs` (réécrit), `crates/eyrie/src/app.rs:166,300-321,1079`, `crates/suite-sdk/src/host.rs` (comptes retirés, `open_settings` ajouté), `crates/eyrie/src/app.rs` (traitement de `Outbox::settings`).

**Interfaces:**
- `accounts::Accounts::new(store: suite_accounts::Store) -> Accounts` ; `list(&mut self) -> &[Account]` (relit si `stamp` a changé) ; `add(kind: ForgeKind, host: &str, token: &str, orgs: Vec<String>)` (vérification hors du fil de l'interface, comme aujourd'hui) ; `poll(&mut self)` ; `remove(&mut self, id: &str)` ; `recheck(&mut self, id: &str)` ; `pub message: Option<(bool, String)>`.
- `fn explain(e: &ForgeError, kind: ForgeKind) -> String` : Azure + `Unauthorized` → « Jeton refusé. S'il est limité à une organisation, indiquez-la ; sinon vérifiez sa validité et ses droits. »
- Avant vérification : hôte Azure `<org>.visualstudio.com` sans organisation saisie → organisation déduite (`suite_accounts::azure_org_from_host`).
- `suite_sdk::Host` : retirer `AccountInfo`, `set_accounts`, `accounts_for`, `token` ; ajouter `pub fn open_settings(&self, page: &str)` → `Outbox { settings: Option<String> }` ; Eyrie traduit `"comptes"` en `Action::OpenSettings(Page::Accounts)`.
- `FORGE_MODULES`, partage par module, `import_dragontree`, `import_owlery` : supprimés.

- [ ] **Step 1 : tests en échec** (`accounts.rs`) : `azure_org_is_deduced_before_verification` (fonction pure `prepare(kind, host, orgs) -> (String, Vec<String>)`) ; `a_verified_account_lands_in_the_common_store` (`Store` en mémoire, résultat de vérification injecté) ; `explain_azure_unauthorized_mentions_the_organisation`. SDK : `open_settings_reaches_the_outbox`.
- [ ] **Step 2 :** `cargo test -p eyrie accounts` et `-p suite-sdk` → échec.
- [ ] **Step 3 :** implémenter ; l'adaptateur Owlery garde provisoirement son coffre propre (`SuiteSecrets` retiré à la Task 5).
- [ ] **Step 4 :** `cargo test --workspace` → vert.

### Task 4 : DragonTree sur les comptes communs, et son mode intégré

**Files (dépôt `..\DragonTree`, branche `suite/module`):**
- Modify: `Cargo.toml` (`suite-accounts = { path = "../SuperApp/crates/suite-accounts" }`), `crates/dragontree-forges/Cargo.toml`, `crates/dragontree-forges/src/secrets.rs`, `crates/dragontree-forges/src/model.rs` (conversions), `crates/dragontree-app/src/{app.rs,accounts.rs,settings.rs,settings_ui.rs,clone.rs,pull_requests.rs,repo/*.rs}`.
- Test: dans les mêmes fichiers (`#[cfg(test)]`), comme le reste de DragonTree.

**Interfaces:**
- `dragontree_forges::secrets` : `store_token`/`load_token`/`delete_token` gardent leurs signatures ; coffre « EyrieSuite » ; `load_token` relit l'ancien coffre « DragonTree » s'il ne trouve rien.
- `impl From<&Account> for suite_accounts::Account` et `pub fn from_suite(a: &suite_accounts::Account) -> Option<Account>` (forge inconnue → `None`).
- `Shared` : `pub accounts: Vec<Account>`, `accounts_store: Option<suite_accounts::Store>` (`None` dans les tests), `accounts_stamp` ; `pub fn refresh_accounts(&mut self)` (relit si l'horodatage change ; appelée dans `logic` au plus toutes les 2 s). Tous les `shared.settings.accounts` deviennent `shared.accounts`.
- `Settings::accounts` : lu pour la migration seulement, `skip_serializing_if = "Vec::is_empty"`.
- Migration dans `DragonTreeApp::new` (pas en capture d'écran) : chaque ancien compte → `store.adopt(compte, ancien jeton)` ; `repo_accounts` réécrit vers l'id renvoyé ; `settings.accounts` vidé ; réglages enregistrés.
- Magasin : `Store::new(&accounts_dir(start.profile_dir.as_deref()), Arc::new(OsVault::suite()))`.
- `DragonTreeApp::set_accounts_link(&mut self, open: Box<dyn Fn()>)` : `AppAction::ShowAccounts` appelle le lien au lieu d'ouvrir la fenêtre ; `Shared::hosted() -> bool`. Fenêtre Réglages : lignes Thème et Langue masquées quand `hosted()` (Eyrie les impose).
- `AccountsWindow::on_verified` : l'injection `store: FnOnce(&str,&str)` devient `add: FnOnce(Account, &str) -> Result<Account, ForgeError>`.

- [ ] **Step 1 : tests en échec** : `old_accounts_move_to_the_suite_and_keep_their_repos` (réglages avec deux comptes dont un déjà présent dans le magasin sous un autre id, `repo_accounts` pointant vers lui → après migration : magasin à 2 comptes, `repo_accounts` vers l'id existant, `settings.accounts` vide) ; `accounts_reload_when_the_file_changes` (écriture par un second `Store` sur le même dossier → `refresh_accounts` les voit) ; `hosted_accounts_action_calls_the_link` ; `hosted_settings_hide_theme_and_language` (test d'interface existant de la fenêtre Réglages, étendu) ; tests existants de `accounts.rs` adaptés à `add`.
- [ ] **Step 2 :** `cargo test` dans `..\DragonTree` → échec.
- [ ] **Step 3 :** implémenter.
- [ ] **Step 4 :** `cargo test` dans `..\DragonTree` → vert ; `cargo clippy --all-targets` sans nouvel avertissement.

### Task 5 : Owlery sur les comptes communs, et son mode intégré

**Files (dépôt `..\Owlery`, branche `suite/module`):**
- Modify: `Cargo.toml` (`suite-accounts` par chemin), `src/secrets.rs`, `src/forges/mod.rs`, `src/app.rs`, `src/main.rs`, `src/ui/{accounts.rs,list.rs,mod.rs,settings.rs}`, `src/ui/tests.rs`.

**Interfaces:**
- `StartOptions::accounts: Option<suite_accounts::Store>` (`None` : démonstration et tests).
- `secrets::store_for(demo)` : hors démonstration, les clés `account:<id>` vont au coffre « EyrieSuite » (lecture de repli dans « Owlery ») ; les autres clés restent dans « Owlery ».
- `forges::from_suite(a: &suite_accounts::Account) -> Option<Account>` (auth `Token`).
- `App::accounts()` : comptes `gh` des réglages + comptes du magasin (+ `gh` implicite comme avant). Relecture du magasin dans `logic` quand l'horodatage change (au plus toutes les 2 s).
- Ajout d'un compte à jeton → `store.add` ; suppression → `store.remove` ; compte `gh` : réglages comme avant.
- Migration dans `App::new` (hors démonstration, état enregistrable) : comptes `Token` des réglages → `store.adopt(…, jeton de « Owlery »)`, retirés des réglages.
- Magasin : `Store::new(&accounts_dir(isolé), Arc::new(OsVault::suite()))`, isolé = `OWLERY_DATA_DIR` s'il diffère du dossier par défaut.
- `App::set_accounts_link(&mut self, open: Box<dyn Fn() + Send + Sync>)` : `Command::Accounts` et les boutons « Ajouter un compte » de l'accueil appellent le lien quand il est posé ; Réglages : ligne Thème masquée quand `hosted`.
- Eyrie (`crates/modules/owlery/src/lib.rs`) : `SuiteSecrets` supprimé, `store_for(demo)` et `accounts: (!demo).then(Store::open)` passés à Owlery ; `app.set_accounts_link(… host.open_settings("comptes"))`. Même chose pour `crates/modules/dragontree/src/lib.rs`.

- [ ] **Step 1 : tests en échec** (`src/ui/tests.rs`, `src/forges/mod.rs`) : `token_accounts_move_to_the_suite` ; `gh_accounts_stay_in_owlery` ; `an_account_added_elsewhere_appears` ; `hosted_accounts_command_calls_the_link`.
- [ ] **Step 2 :** `cargo test` dans `..\Owlery` → échec.
- [ ] **Step 3 :** implémenter, puis brancher les deux adaptateurs d'Eyrie.
- [ ] **Step 4 :** `cargo test` dans `..\Owlery` et `cargo test --workspace` dans Eyrie → verts.

### Task 6 : passe sur les réglages d'Eyrie

**Files:**
- Modify: `crates/eyrie/src/settings.rs` (réécrit page par page), `crates/eyrie/locales/en.json`, `crates/eyrie/src/tests.rs` (scènes des réglages).

**Décisions de contenu :**
- Une phrase d'aide au plus par section, et seulement quand le libellé ne suffit pas ; le détail passe en infobulle. Aucun terme d'implémentation (« embarqué sans changer son code », « style rétabli », « bac à sable », « verrou »).
- **Général** : Apparence (vignettes de thème, zoom), Langue, Données des modules (deux choix d'une ligne chacun), « Suivre le dépôt courant ».
- **Modules** : une ligne par module (interrupteur, icône, nom, état, résumé) ; « Fermer <exe> » et « Relancer » seulement quand c'est utile.
- **Comptes** : « Valables dans toute la suite, seuls ou dans Eyrie. » ; liste (titre, organisations, Vérifier, Retirer) ; formulaire (forge, jeton, hôte et organisations repliés sous « Serveur et organisations » sauf pour Azure où l'organisation est visible), lien « Créer un jeton » (`dragontree_forges::token_creation_url`) et droits recommandés (`recommended_scopes`).
- **Tailscale**, **Raccourcis**, **Dispositions** : libellés raccourcis, aides réduites à une ligne.
- **Page d'un module** : résumé, état, interrupteur, bouton « Réglages de <module> » (commande `reglages` du module, s'il l'a) ; les statistiques techniques dans un repli « Diagnostic ».

- [ ] **Step 1 : test en échec** : `settings_pages_have_no_implementation_jargon` (rend chaque page avec `egui_kittest` et vérifie l'absence de « sans changer son code », « Style rétabli », « verrou ») ; `module_page_offers_its_settings_command`.
- [ ] **Step 2 :** `cargo test -p eyrie settings` → échec.
- [ ] **Step 3 :** réécrire les pages ; mettre `en.json` à jour.
- [ ] **Step 4 :** `cargo test --workspace` → vert (dont `catalog_matches_sources`) ; capture `EYRIE_CAPTURE` de chaque page relue à l'œil.

### Task 7 : documentation et vérification d'ensemble

**Files:**
- Create: `docs/comptes.md` (contrat : dossier, fichier, coffre, variables, règles de normalisation et de migration, ce qu'un module doit faire seul et intégré).
- Modify: `README.md`, `docs/premier-jet.md` (§5 n° 7, tableau du §6), `..\Owlery\docs\contrat-local.md` §5 (renvoi au contrat), `..\DragonTree\README.md` (comptes communs).

- [ ] **Step 1 :** écrire et mettre à jour.
- [ ] **Step 2 :** `cargo test` dans les trois dépôts → verts ; `cargo build -p eyrie` ; lancer Eyrie sur un dossier d'essai (`EYRIE_DATA_DIR`, `EYRIESUITE_DIR` temporaires) : ajout d'un compte, visible dans DragonTree et Owlery intégrés ; DragonTree lancé seul sur le même `EYRIESUITE_DIR` le voit aussi.
- [ ] **Step 3 :** relire les trois diffs ; résumé à l'utilisateur, avec la question du commit.
