# Audit de Patronus et Menagerie en vue de la suite (22/10/2026)

Audit en lecture seule des dépôts voisins `../Patronus` et `../Menagerie`. Chemins relatifs à chaque dépôt.

## Patronus

**1. Structure**
- Le dépôt est un workspace : `patronus-core` (aucune UI), plus `patronus-app` qui donne une lib `patronus_app` et un exécutable `patronus`.
- Point d'entrée : `src/main.rs:172`. La ligne de commande accepte la fenêtre, `lancer <tâche>`, `planifie <tâche>`, `--donnees` et `--reduit`. Elle mène à `gui()` (`main.rs:204`), puis à `lib.rs:20`.
- `eframe::run_native` est à `lib.rs:29`. Les `NativeOptions` (`lib.rs:22-28`) donnent le titre, `app_id` "patronus", la taille lue dans l'état, un minimum de 760×480 et l'icône. Décorations natives, pas de barre de titre perso.
- Le type est `app::App` (`app/mod.rs:104`). Constructeurs : `App::new(cc,…)` et `App::with(cc, Options)` (`:167`, `:171`). De `cc`, seul `egui_ctx` sert ; `cc.window_handle()` sert aussi, dans `lib.rs:33`, pour la zone de notification.
- `impl eframe::App` (`:905`) : `fn ui(&mut self, ui: &mut egui::Ui, _frame)` et `on_exit` (`:971`).
- Les panneaux (`Panel::top/bottom/left(..).show(ui)`, `CentralPanel::show(ui)`, `:923-937`) sont déjà dessinés dans un `Ui`, donc dockables.
- Restent au niveau `Context` :
  - les fenêtres Réglages et Problèmes (`settings.rs:15,174`) ;
  - la palette (`palette.rs:70`) ;
  - trois `Modal` (`mod.rs:708,811,840`) ;
  - les toasts, une `Area` ancrée en bas à droite (`:869`).
- Ce qu'il faudrait changer : extraire un `show(ui)` et sortir tout ce qui touche à la fenêtre (titre, fermeture, taille, zoom, thème).
- Embarquement : seulement l'intention (`lib.rs:1-2`). Ni trait ni API « module ».

**2. État global au processus**
- `static TRAY: Mutex` (`notify.rs:18`) : icône de la zone de notification avec `uID=1`, identique à Fawkes (`Fawkes/src/notify.rs:22`). Collision dans un même processus.
- Thème : `ctx.set_visuals_of(Light/Dark)` et `ctx.set_theme` (`theme.rs:105-112`, appelés à `mod.rs:178`).
- Zoom : `ctx.set_zoom_factor` (`mod.rs:75,183`) et Ctrl+molette (`:766`). Le zoom est global et enregistré dans l'état de Patronus.
- `ViewportCommand` : `Title` avec le compteur de non lus (`:919`), `CancelClose` et `Close` (`:802,827`), `Minimized` (`lib.rs:40`).
- La taille de fenêtre est recopiée à chaque image (`:962-967`).
- Pas de sous-classement Win32, pas de raccourci global.
- Instance unique : mutex `Local\Patronus-single-instance:<dossier>` (`main.rs:16-29`), pas de tube. `planifie` s'en sert pour savoir si la fenêtre est ouverte (`main.rs:104,146`).
- Planificateur Windows et clé `Run` utilisent `current_exe()` (`winsched.rs:268`, `autostart.rs:58`). `AttachConsole` est à `main.rs:178`.

**3. Thème** : pas copié de Fawkes.
- Patronus propose Système / Sombre / Clair (`theme.rs:8`) avec une palette propre de 11 couleurs (`:28-76`) et suit le système. Il garde les polices egui par défaut et ne règle pas les espacements.
- Fawkes : Graphite / Fluent / Nuit, `static CURRENT: AtomicU8`, une quarantaine de champs (dont terminal et ANSI), `set_fonts` (Segoe UI, Cascadia), `global_style_mut`.
- Les couleurs diffèrent (accent #9fc5ff contre #5b8def).

**4. Langue** : français en dur partout, y compris les erreurs `String` du cœur, les statuts (`run.rs:35`) et « Maj » dans les raccourcis. Aucune i18n. Les clés JSON sont en français.

**5. Comptes** : aucun stocké. Patronus utilise la CLI `claude` déjà connectée sur le poste et git avec les identifiants système. La spec prévoit des forges GitHub / GitLab / ADO « comptes partagés avec DragonTree » (`spec.md:24`). Les secrets sont détectés dans les tâches (`task.rs:293`).

**6. Réglages et état**
- Dossier : `%APPDATA%\Patronus`, ou `PATRONUS_DATA_DIR` / `--donnees` (`fsutil.rs:8-18`).
- `state.json` (`state.rs:18-40`) contient thème, raccourcis, rétention, notifications, taille de fenêtre et zoom.
- Robustesse : champs `version` et `saved_by`, copie `state.avant-<ver>.json`, fichier illisible mis de côté.
- Champs inconnus gardés (`flatten autres`), aussi dans `planning.json` (budget, pause), les tâches (`task.rs:34`) et `run.json`.
- Écritures atomiques avec `write_atomic` (fichier tmp, sync, renommage, `fsutil.rs:22`).

**7. Contrat (`docs/contrat-local.md`)**
- Canal `\\.\pipe\<app>-<SID>` réservé au compte, une ligne JSON `{"v":1,"demande":…}`, réponse `{"v":1,"ok":…}`.
- Une demande inconnue reçoit une erreur explicite. Pas de schéma d'URL. Repli par ligne de commande si l'appli ne tourne pas.
- Ce que Patronus reçoit :
  - `lancer{tache}` ;
  - `afficher_resultat{execution}` ;
  - `nouvelle_tache{depot}` ;
  - `evenement{source,type,dossier,texte}` : motif Fawkes, `service_arrete` de Menagerie, `pr_ouverte` / `pr_commentee` / `ci_echec` d'Owlery.
- Ce qu'il émet :
  - vers Fawkes, `ouvrir{dir,groupe,titre,claude,resume,consigne}` et `surveiller_motifs[{tache,dossier,motif}]` ;
  - vers DragonTree, `ouvrir{dir,diff}`.
- Implémenté : seulement `patronus lancer` (exécuté dans le processus appelant, `main.rs:66`) et `planifie`. Aucun tube.
- Le vocabulaire diverge de celui de Menagerie (`demande` / `erreur` contre `op` / `error`).

**8. Arrière-plan**
- Le `Ticker` tourne dans un fil toutes les 250 ms (`engine.rs:66-97`). S'y ajoutent le watcher de fichiers et les relevés et maintenance git, chacun dans des fils.
- Il doit tourner quand la vue est masquée.
- Le cœur sait tourner sans UI (`Engine::open_opts` + `Ticker`). Mais le `Ticker` est créé dans `App::with` (`mod.rs:193`) avec un rappel lié à l'UI (toasts, notification, repaint).
- Restent dans `ui()` : la synchro du Planificateur (`:958`), le marquage « lu » et le vidage des toasts.
- Le verrou du moteur est tenu pendant toute l'image (`:910`).

**9. Raccourcis et identifiants egui**
- Raccourcis (`shortcuts.rs:58-70`) : Ctrl+N, Ctrl+B, F5, Ctrl+Maj+X, Ctrl+P, Ctrl+,, Ctrl+J, Ctrl+Maj+P. Ils sont lus dans `ctx.input` sans être consommés (`mod.rs:524`), où que soit le focus.
- Conflits :
  - Ctrl+, avec Fawkes et Menagerie ;
  - Ctrl+N et F5 avec Menagerie ;
  - Ctrl+Maj+P est la palette de Fawkes.
- La palette consomme flèches et Entrée (`palette.rs:53`).
- Identifiants génériques :
  - panneaux "top", "status", "tasks", "runs" ;
  - `ComboBox` "filter" et "mode" ;
  - `Grid` "settings" ;
  - `id_salt` "journal" et "diff" ;
  - fenêtres « Réglages », « Problèmes », « Palette », dont l'identifiant est le titre.

**10. Documentation**
- README:43-48 : « futur module de la suite », « aucune migration », contrat commun.
- spec §9-10 : l'interface « ou le module de la suite » ; Patronus dans la palette de Fawkes, groupe « Agents » ; dans DragonTree, *Lancer une tâche sur ce dépôt* ; PR et CI via Owlery.
- feuille-de-route:339-344 : contrat par tube, masquage des secrets selon les règles de Fawkes, notification cliquable.
- Rien sur la langue, Tailscale ou des réglages communs.

**11. Code dupliqué avec Fawkes**
- `write_atomic`, identique à `Fawkes/src/persist.rs:344`.
- Le versionnage d'état, repris de Fawkes.
- `build.rs` (ressource icône et version).
- `notify.rs` et `autostart.rs`, même procédé que Fawkes.
- Le mutex d'instance.
- `capture.rs`, identique à celui de Menagerie à une ligne près.
- Le parse / format des raccourcis (trois versions).
- Palette, thème, toasts.
- `tools/setup.ps1`, `.cargo/config.toml`, `rustfmt.toml`, `tests/common`.

## Menagerie

**1. Structure**
- Une crate lib `menagerie` et un exécutable.
- `main.rs` : `--relais` est traité en premier (`:10`), puis `instance::acquire` (`:13`). Taille, position et état agrandi sont restaurés. `NativeOptions` à `:29`, `run_native` à `:30`. Décorations natives.
- Le type est `app::App` (`app/mod.rs:98`).
  - `App::new(cc,…)` (`:145`) n'utilise `cc` que pour le repaint du `Monitor`.
  - `App::with(st, problems, monitor, dirs)` (`:154`) se passe de `cc` : il est déjà injectable.
- `ui(&mut self, ui, _frame)` est à `:560`, `on_exit` à `:668`.
- Panneaux "onglets", "problemes" et `CentralPanel` dans le `ui` (`:599-659`). Les vues `services_view`, `mails.ui` et `env.ui` prennent un `Ui` : chaque onglet pourrait devenir un panneau dockable.
- Un seul `Modal`, au niveau `ctx` (`dialogs.rs:96`).
- Pas de trait de module (intention seulement, `lib.rs:3-4`).

**2. État global au processus**
- `static OnceLock` pour AES (`vault.rs:489`), sans conséquence.
- Thème : `ctx.set_visuals` (`theme.rs:49`), appliqué à la première image et à chaque changement (`app/mod.rs:566`). Pas de zoom ni de polices.
- `ViewportCommand` : `Title` (« Menagerie — N à voir », `:595`), `CancelClose` (`:320`), `Close` (`dialogs.rs:530,563`). Le code lit aussi `close_requested`, `focused` et la géométrie de la fenêtre (`:563,584-592`).
- Win32 :
  - `FlashWindowEx` sur la « première fenêtre visible du processus » (`taskbar.rs:12-43`) ;
  - `EnumWindows` pour ramener l'autre instance devant, repérée par son titre et le nom de l'exe (`instance.rs:35-66`).
- Instance unique : mutex `Local\Menagerie-instance-unique[:<dossier>]` (`instance.rs:8-31`), pas de tube.
- Le relais est `current_exe()` (`app/mod.rs:91`).

**3. Thème** : pas copié de Fawkes.
- Sombre / Clair, sombre par défaut, ne suit pas le système. Palette de 7 couleurs (`theme.rs:7-38`), différente de Patronus et de Fawkes.
- La palette est lue depuis `st.theme` et non depuis le contexte : elle devient incohérente si l'hôte impose un autre thème.
- `set_visuals` n'agit que sur le thème egui courant : si Windows change de thème, Menagerie retombe sur les visuels egui par défaut.

**4. Langue** : français en dur. Les clés JSON sont en anglais.

**5. Comptes** : aucun.
- La clé des journaux est un certificat « Menagerie journaux » du magasin utilisateur, protégé par la puce TPM ou à défaut DPAPI (`vault.rs:1-24`). Son sujet change pour un dossier d'essai (`persist.rs:33-43`).
- Le reste est local : smtp4dev, docker ou podman.

**6. Réglages et état**
- Dossiers : `%APPDATA%\Menagerie\state.json`, journaux dans `%LOCALAPPDATA%\Menagerie\logs`, ou `MENAGERIE_DATA_DIR` (`persist.rs:16-30`).
- `State` (`:134`) contient services lancés, noms, santé, thème, raccourcis, `mail_url`, `docker_cmd`, `env_dir`, fenêtre, `on_close`, `visibility`, `encrypt_logs`.
- Écriture atomique (`save_to:255`), à chaque image où l'état a changé.
- **Champs inconnus perdus** : pas de `flatten`.
- **Une version plus récente fait mettre tout le fichier de côté** et repartir de l'état par défaut (`:221`).
- Pas de `saved_by`.

**7. Contrat (`docs/contrat-fawkes.md`)**
- Transport : `\\.\pipe\fawkes-<SID>` réservé au compte, une ligne JSON par message, `"v":1`. Un champ inconnu est ignoré ; une version inconnue reçoit `{"ok":false,"error":"version"}`.
- Demandes de Menagerie :
  - `terminals` → `{term,title,group,group_color,dir,root_pid,command (null si la commande contient un secret),state,exit_code,started,restart_policy,claude}` ;
  - `focus`, `restart`, `stop` `{term}` ; `stop` est confirmé dans Fawkes si le terminal n'est pas épinglé ;
  - `subscribe{events:[terminal_exited,terminal_started,terminals_changed]}`, puis événements poussés sans `id`.
- Les ports restent relevés par Menagerie.
- Plus tard, sur `\\.\pipe\menagerie-<SID>` : `services` (pour un badge) et `port_owner{port}`.
- Implémenté : **rien**, ni tube ni client. Seul le calcul de la descendance des processus existe (`app/mod.rs:385`).
- `relay.rs` n'est pas un relais du contrat. C'est `menagerie.exe --relais …`, qui lance un service et chiffre sa sortie (AES-GCM, clé publique). Il survit à Menagerie et sort avec le code 70 s'il ne peut pas chiffrer.

**8. Arrière-plan**
- Le `Monitor` a trois fils : ports toutes les 2 s, Docker toutes les 5 s (30 s s'il est absent), santé toutes les 10 s. Ils ne dépendent pas de l'UI, mais chaque relevé redessine tout le contexte. L'UI redessine en plus toutes les 500 ms (`:665`).
- **La supervision est dans `ui()`** : `m.tick()` (arrêts et relances, `:576`), `sync_logs`, cibles de santé, `persist()`, alertes.
- Si la vue est masquée, plus de relance ni de sauvegarde. Il faudrait que ça tourne même masqué.
- Sans UI : la lib le permet, mais il faut en extraire une boucle de service.

**9. Raccourcis et identifiants egui**
- Raccourcis (`shortcuts.rs:59-70`) : F5, Ctrl+1 à Ctrl+4, Ctrl+N, Ctrl+,, Ctrl+F.
- Ils sont lus sans être consommés et **sans tenir compte du focus** (`:477-496`) : ils partent même pendant une saisie dans un autre module.
- Conflits : F5, Ctrl+N et Ctrl+, avec Patronus ; Ctrl+, avec Fawkes ; Ctrl+1 à 4 avec les onglets d'un hôte.
- Identifiants génériques : "onglets", "problemes", "liste-mails", le `Modal` "dialogue", "recherche-services", "autres-programmes", "politique".

**10. Documentation**
- spec §3 : « Badge dans la barre d'activité de la suite » (services arrêtés ou sans réponse).
- spec §1 et §6 : partage des rôles avec Fawkes, intégrations avec Patronus et DragonTree (`.env`).
- spec §7 : faut-il un service Windows quand rien n'est ouvert ?
- README:178-184 : « futur module », « aucune migration ».
- feuille-de-route:366-376 : notifications comme Fawkes, masquage des secrets, icône, sauvegarde hors du fil UI, relais copié hors de l'exe.
- Rien sur la langue, les comptes ou Tailscale.

**11. Code dupliqué avec Fawkes**
- `on_some_monitor`, identique à `Fawkes/src/main.rs:74`.
- `capture.rs`, identique à celui de Patronus.
- L'écriture atomique, réécrite en ligne.
- Le mutex d'instance et les raccourcis (troisième variante dans chaque cas).
- Le thème et la notice éphémère.
- `open_url` (`ShellExecuteW`).
- `scripts/setup.ps1` (identique à celui de Patronus à 5 lignes près), `.cargo/config.toml`, `rustfmt.toml`, `tests/common`.
- Menagerie n'a ni icône ni zone de notification.

## Bloquants pour l'intégration

L'effort est estimé en jours de travail.

**Patronus**

| # | Bloquant | Effort |
|---|---|---|
| 1 | Planificateur, clé `Run`, `planifie` et `lancer` dépendent de `current_exe()` et du mutex : l'hôte doit tenir le même mutex et soit garder `patronus.exe` comme cible, soit porter cette ligne de commande. Il faut aussi empêcher l'exe autonome et la suite de tourner sur le même dossier. | 1 à 2 j |
| 2 | Extraire un module (`new(ctx)`, `show(ui)`, `badge`, `close_losses`, `on_exit`) ; sortir le `Ticker` et la synchro du Planificateur de `ui()`. | 1 à 2 j |
| 3 | Thème, zoom, titre, fermeture et taille de fenêtre doivent être délégués à l'hôte, avec repli en mode autonome. | 1 j |
| 4 | Fenêtres, modales et toasts sont au niveau `Context` ; préfixer les identifiants génériques. | 0,5 à 1 j |
| 5 | Icône de notification `uID=1` en collision avec Fawkes : passer par un service de notification commun. | 0,5 j |
| 6 | Raccourcis à n'activer que si le module a le focus, conflits à arbitrer par l'hôte. | 0,5 j |
| 7 | i18n, cœur compris (pas bloquant pour une V1 en français). | 3 à 5 j |

**Menagerie**

| # | Bloquant | Effort |
|---|---|---|
| 1 | Supervision dans `ui()` : à appeler par l'hôte à chaque image, même vue masquée. | 1 à 2 j |
| 2 | Contrat Fawkes à implémenter des deux côtés. Dans la suite, une API Rust interne au processus suffit ; le tube ne sert qu'aux exe autonomes. Pas bloquant pour une V1. | 2 à 3 j |
| 3 | Fermeture (`Close` / `CancelClose`, arrêter ou laisser les services), titre, flash, retour au premier plan par `EnumWindows` : à remplacer par une API hôte (badge, attention, fermeture). | 1 j |
| 4 | Relais `current_exe()` / `--relais` : l'exe de la suite doit le prendre en charge, et les relais encore vivants le verrouillent. | 0,5 j |
| 5 | État : ajouter `flatten` et `saved_by`, et ne plus mettre de côté un état plus récent, avant que l'exe autonome et la suite écrivent le même fichier. | 0,5 j |
| 6 | `set_visuals` global à la première image écrase le thème de l'hôte ; la palette suit `st.theme`. | 0,5 j |
| 7 | Raccourcis sans test de focus, Ctrl+1 à 4. | 0,5 j |
| 8 | Repaints toutes les 500 ms et à chaque relevé. | 0,25 j |

**Commun aux trois applications** (avec Fawkes) : une crate partagée (thème et palette, écriture atomique et état versionné, capture, raccourcis, notifications, instance unique, `on_some_monitor`, `build.rs`) demanderait 2 à 3 j.
