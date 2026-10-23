# Finitions de la suite : points en attente, DragonTree (fetch, ajout de dépôts)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Traiter les points laissés pour plus tard par la relecture, et deux manques de DragonTree signalés par
l'utilisateur, plus les points d'UX du même ordre repérés en route.

**Architecture:** Corrections locales dans chaque dépôt, sur les branches actuelles (Eyrie `suite/etape-2`, modules
`suite/module`) ; chaque tâche a son test qui échoue d'abord.

**Tech Stack:** Rust 2024, egui/eframe 0.37, egui_kittest ; toolchain GNU, modules compilés avec
`--config C:/Users/clbr-achedotal/DEV/TestAubin/SuperApp/.cargo/config.toml`.

**Spec:** message de l'utilisateur du 23/10/2026 : « Continue avec ta liste de "points laissés pour plus tard". Aussi,
deux points sur DragonTree : origin ne liste pas l'entièreté des branches distantes du repo en question alors qu'il
devrait. Le bouton plus sur Dossier devrait permettre de sélectionner soit un dossier soit une repo distant à cloner
(avec choix du compte en question). Si tu vois d'autres points d'UX ou de fonctionnel de ce type n'hésite pas. Reste
sur les branches actuelles pour l'instant. » Précision (question) : Barre latérale › Remotes, dans un clone d'Ozmo.

## Global Constraints

- Branches actuelles seulement ; commits locaux, aucun push.
- Les tests ne touchent jamais au vrai coffre Windows ni à `%APPDATA%`.
- Un dépôt de tests à la fois, `cargo test -j 4` (mémoire).
- Pas de `cargo fmt` dans Fawkes.

## Constat (origin)

Les données sont complètes (Survey2/Ozmo : 1572 branches sur le serveur et en local) et la barre latérale les montre
toutes (essai avec les 1572 noms réels). Ozmo5, fetché pour la dernière fois à 14 h 54, en avait 11 de moins et 6 de
trop : à l'ouverture d'un onglet, `last_fetch = Instant::now()` repousse le premier fetch automatique de 10 minutes,
quel que soit l'âge du dernier fetch du dépôt.

## Review Focus

- Dépôt jamais fetché (pas de `FETCH_HEAD`) : fetch dès l'ouverture.
- Plusieurs onglets ouverts au lancement : un fetch par dépôt, pas plus (file d'arrière-plan du groupe).
- URL d'un hôte sans compte : clonage avec les identifiants de git, comme avant.
- Verrou de comptes tenu par une application qui plante : libéré par Windows, l'écriture suivante passe.
- Patronus intégré : ses tâches Windows lancent `eyrie.exe planifie <tâche>`, que la suite sait exécuter.

---

### Task 1 : DragonTree — premier fetch selon l'âge du dernier
- `RepoTab::last_fetch: Option<Instant>` ; `None` : dû. À l'ouverture, l'âge de `FETCH_HEAD` (dossier commun) donne
  l'instant du dernier fetch ; absent ou plus vieux que l'uptime : dû.
- Test `ui_network` (ou nouveau `ui_fetch_on_open`) : `a_stale_repository_is_fetched_when_opened` — serveur local,
  branche créée après le clonage, `FETCH_HEAD` daté d'il y a une heure, fetch automatique par défaut (10 min) : la
  branche apparaît dans Remotes sans attendre. Fonction pure testée : `fetch_due_at_open(age, every)`.

### Task 2 : DragonTree — « + » de Dépôts : dossier ou dépôt distant, avec compte
- Le « + » de l'en-tête Dépôts ouvre un menu : « Ouvrir un dépôt… », « Cloner un dépôt distant… », « Ajouter un
  dossier à scanner… ».
- Fenêtre Cloner, mode URL : liste « Compte » (comptes dont `account_matches_url` est vrai d'abord, puis « Aucun :
  identifiants de git ») ; présélection du premier compte qui correspond ; le compte choisi authentifie le clonage et
  est associé au dépôt cloné.
- Tests : `the_plus_of_repositories_offers_folder_and_clone` (menu, ouvre Cloner) ;
  `cloning_by_url_preselects_the_matching_account` ; fonction pure `default_account_for(url, accounts)`.

### Task 3 : suite-accounts — verrou du système
- `accounts.lock` tenu par `File::try_lock` (verrou de Windows, rendu à la mort du processus) au lieu de
  `create_new` + abandon à 10 s ; attente 5 s puis « comptes occupés ».
- Tests : `a_released_lock_is_taken_at_once`, `a_held_lock_makes_a_writer_wait_then_fail`, les tests de concurrence
  existants restent verts.

### Task 4 : Eyrie — réglages
- `confirm_remove` oublié quand la page Comptes est quittée ou la fenêtre fermée
  (`a_pending_removal_is_forgotten_when_the_page_is_left`).
- Fenêtre de réglages bornée à la fenêtre qui la montre (`the_settings_window_fits_a_small_window`).
- Les panneaux dessinés dans une fenêtre détachée reçoivent le `WindowWish` de cette fenêtre (`SettingsCx::wish`).
- Tests manquants : `settings_of_a_detached_window_with_two_modules_list_both_only`,
  `a_successful_add_clears_the_form`.
- Eyrie en test : coffre en mémoire, jamais le vrai (`cfg!(test)`).

### Task 5 : Menagerie — « Fermer » intégré
- Intégré, le bouton du bas de ses réglages devient « Annuler » (il remet le brouillon). Essai approfondi : pas de
  « Fermer » dans sa page.

### Task 6 : Patronus — tâches Windows intégré

> **Abandonnée en route** : `eyrie.exe` sait déjà lancer les tâches de Patronus (`eyrie.exe lancer|planifie <tâche>`,
> `suite_patronus::cli`), et `winsched::sync` réinscrit chaque tâche avec l'exécutable courant : rien ne pointe vers
> l'ancien `superapp.exe` (vérifié). Seul l'essai de `settings_left` de Patronus est gardé.

- `set_scheduler_exe(Option<PathBuf>)` : intégré, les tâches Windows lancent ce `patronus.exe` ; `None` : pas de
  synchronisation (message dans ses réglages). L'adaptateur cherche `patronus.exe` à côté d'`eyrie.exe`, puis dans
  `../Patronus/target/release`.
- Tests : `scheduler_exe` (seul : l'exe courant ; intégré : celui donné ou rien) ; adaptateur : recherche de l'exe.

### Task 7 : DragonTree — UX des remotes
- Infobulle d'un remote : « Dernier fetch : il y a … » (âge de `FETCH_HEAD`).
- Menu d'un remote et d'un dossier : « Tout déplier » / « Tout replier ».
- Tests : `unfolding_all_shows_every_branch_of_the_remote`, `remote_tooltip_tells_the_last_fetch`.
