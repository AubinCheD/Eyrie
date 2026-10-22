# Audit de DragonTree en vue de la suite (22/10/2026)

Audit en lecture seule du dépôt voisin `../DragonTree`. Chemins relatifs à `crates/` sauf mention contraire.

## 1. Structure
- **Crates.** Le workspace en compte quatre (`Cargo.toml:4`) : `-i18n`, `-git` et `-forges` sont des bibliothèques. `-app` fournit la bibliothèque `dragontree_app` et le binaire `dragontree` (`dragontree-app/Cargo.toml:11-17`). Son `build.rs` intègre l'icône (objet COFF écrit à la main, sans `rc`).
- **Lancement** (`dragontree-app/src/main.rs:107-134`) :
  - `run_native("DragonTree")`, rendu Glow, décorations de l'OS, pas de barre de titre personnalisée ;
  - icône `app_icon::window_icon()` ;
  - `persist_window` actif ;
  - `persistence_path` vaut `<profil>/window.ron`, sinon le défaut d'eframe `%APPDATA%\DragonTree\data\app.ron` (fenêtre et mémoire egui).
- **Type de l'App.** `DragonTreeApp` (`app.rs:335`). `new(cc, StartOptions)` (`app.rs:374`) n'utilise que `cc.egui_ctx` : il suffit de le remplacer par un `&egui::Context`. Les tests le construisent avec `build_eframe` (`tests/common/mod.rs:98`).
- **Méthodes de `eframe::App` implémentées** (`app.rs:1258`) :
  - `logic(ctx, frame)` : événements, interception de la fermeture, IPC ;
  - `ui(&mut Ui, frame)` ;
  - `save` : ignore le `Storage` et écrit `settings.json` ;
  - `on_exit`.
- **Dessin dans un `Ui` : c'est déjà presque le cas.**
  - Avec egui 0.37, `Panel::…show(ui)` est rattaché au `Ui` parent. Les panneaux `"menu-bar"`, `"tab-bar"`, `"repo-list"` et le panneau central (`app.rs:1299/1320/1337/1350`) se dessinent donc dans le `Ui` reçu.
  - `RepoTab::ui(ui, &mut Shared)` (`repo/mod.rs:1358`) aussi, avec des panneaux salés par l'id de l'onglet.
  - En revanche, 27 endroits dessinent au niveau `Context` : `Window`, `Modal`, `Area` (`app.rs:1378-1428`, `repo/dialogs.rs`, `toasts.rs:113`, `quick_open.rs:91`, et `widgets.rs:19` centré sur `content_rect`).
- **Pour exposer « dessine-toi dans ce Ui »** : extraire un module avec `new(ctx, services_hôte, opts)`, `logic`, `ui(ui)`, `save`, `on_exit`, `busy_ops()` et `title()`. Il faut aussi :
  - sortir du module l'application du thème, des polices et de la langue, les `ViewportCommand`, l'interception de la fermeture et l'instance unique ;
  - les garder dans un wrapper autonome `main.rs`.
- **Onglets multi-dépôts.**
  - Barre maison (`tabbar.rs`) : `Vec<Tab>` plus un index actif (`app.rs:325-340`), avec `TabKind::Welcome | Repo(Box<RepoTab>)`.
  - Seul l'onglet actif est dessiné, mais tous reçoivent `tick` (fetch automatique) (`app.rs:1354-1362`).
  - Les événements sont routés par `TabId` sur un seul canal mpsc (`app.rs:720`).
  - S'y ajoutent les espaces de travail et une pile d'onglets fermés.
  - Chaque `RepoTab` pourrait devenir un onglet de docking, mais la gestion du clavier suppose un seul onglet visible.

## 2. État global au processus
- **i18n** : `CURRENT: AtomicU8` (`dragontree-i18n/src/lib.rs:84`). DragonTree réimpose sa propre langue à chaque image (`app.rs:815-819`) et écraserait donc celle de la suite.
- **Polices** : `ctx.set_fonts` remplace toute la `FontDefinitions` (`theme.rs:22,37`), y compris depuis un thread de fond quand la police du diff change (`app.rs:806-810`). Les polices des autres modules seraient effacées.
- **Style** : `theme::apply` (`theme.rs:253-279`) agit sur toute la fenêtre :
  - `set_visuals_of` clair et sombre, `set_theme` ;
  - `all_styles_mut` : tailles de texte, espacements, `selectable_labels=false` ;
  - `zoom_with_keyboard=false`, `set_zoom_factor`.
- **`ViewportCommand`** — la fenêtre de la suite serait renommée, fermée ou bloquée :
  - `Title` (`app.rs:1294`) ;
  - `Close` (`app.rs:895`, `closing.rs:109`) ;
  - `Focus` / `Minimized` (`app.rs:524-526`) ;
  - `CancelClose` (`closing.rs:139,161`).
- **Observateur des commandes git** : un `OnceLock` posé une fois par processus (`dragontree-git/src/runner.rs:501-507`, installé par `toasts.rs:186`). Une seconde installation échoue en silence.
- **États globaux sans conséquence et partageables** :
  - agents HTTP et cache du proxy (`forges/src/http.rs:120,130`, `proxy.rs:322,358`) ;
  - syntect et son cache LRU (`highlight.rs:50,83,155`) ;
  - le store par défaut de keyring v4, posé paresseusement.
- **Win32** : aucun sous-classement, aucun hook, aucun `RegisterHotKey`. Seuls `AttachConsole` (en cas d'erreur de ligne de commande) et `AllowSetForegroundWindow` sont appelés. Les Job Objects sont créés par commande git.
- **Instance unique** : mutex `Local\DragonTree-<clé>` et tube nommé (`single_instance.rs:89,95`), uniquement depuis `main.rs:74-91`.
- **Fichiers déposés** : tous les dépôts sur la fenêtre sont lus (`app.rs:1073`), avec une notification si ce n'est pas un patch.

## 3. Thème
- **Choix** : `ThemeChoice` Système/Clair/Sombre (`settings.rs:52`) et `ui_scale` de 0,8 à 1,6 (`settings.rs:17-19`), enregistrés dans `settings.json`. `--theme` le remplace pour l'exécution seulement.
- **Palette** : `Palette` codée en dur, en clair et en sombre : 28 couleurs métier et 10 couleurs de branches du graphe (`theme.rs:90-216`).
  - `Palette::current` lit `global_style().visuals.dark_mode` (`theme.rs:218`) : elle suivra le thème de l'hôte.
  - Aucune personnalisation, sauf la couleur des onglets par dépôt.
- **Polices** :
  - Segoe UI, Segoe UI Semibold et Cascadia Mono ou Consolas, lues dans `%WINDIR%\Fonts` ;
  - Phosphor intégrée (`assets/fonts/Phosphor.ttf`) ;
  - familles `"bold"` et `"diff"` ;
  - police du diff au choix (`diff_style.rs:32`).
- **syntect** : `default-fancy`, thèmes fixes `base16-ocean.dark` et `InspiredGitHub` (`highlight.rs:83-87`), calcul en arrière-plan.

## 4. Langue (`dragontree-i18n`)
- **Catalogues.** Le texte français du code sert de clé. Chaque catalogue `locales/<code>.json` est un objet plat « français → traduction ». Seul `en.json` existe (1 397 entrées), intégré par `include_str!` dans une constante `CATALOGS` unique (`lib.rs:36`).
- **Langues** : `Lang` est une énumération fermée {Fr, En} (`lib.rs:29-39`).
- **Sélection** :
  - `set_lang` agit sur tout le processus (`lib.rs:87`) ;
  - `Lang::system()` lit `GetUserDefaultUILanguage`, avec repli sur l'anglais (`lib.rs:64-77`) ;
  - au démarrage la langue vaut Fr (`CURRENT=0`) ;
  - sérialisée en `"fr"` / `"en"`.
- **API** :
  - `tr!` : `&'static str`, ou `String` avec des paramètres `{nom}` ;
  - `trn!` : pluriel (Fr : n>1, En : n≠1) ;
  - `tr_mark!` avec `tr_str`, plus `translate` et `fill`.
- **Changement à chaud** : oui, à l'image suivante. Les textes déjà construits (notifications, titres d'opérations) ne sont pas retraduits (README:219).
- **Test** : `tests/catalog.rs:12-20` parcourt les `src` des crates voisines pour repérer traductions manquantes, orphelines et paramètres incohérents.
- **Dépendances** : serde, serde_json, windows-sys (Globalization). Rien de propre à DragonTree, sauf le nom et le catalogue unique.
- **Réutilisable tel quel : non.** Tous les modules devraient partager un seul `en.json`. Il faut des catalogues par domaine : une macro qui génère un `tr!` local à chaque crate, ou un `register_catalog`, avec une langue globale commune (l'atome actuel convient).

## 5. Comptes (`dragontree-forges`)
- **Forges** : GitHub (et GitHub Enterprise), GitLab (et auto-hébergé), Azure DevOps (et Server) (`model.rs:10-14`). Ni Gitea ni Bitbucket.
- **Modèle** : `Account { id (16 hex aléatoires), kind, host, username, display_name, orgs, avatar_url }` sérialisable (`model.rs:40-49`). Les comptes sont rangés dans le `settings.json` de DragonTree (`accounts`, `repo_accounts`).
- **Jetons** : keyring 4 (Gestionnaire d'identifiants Windows), service `"DragonTree"`, utilisateur `"account:<id>"` (`secrets.rs:9,42`).
- **Authentification** : PAT collé seulement, vérifié par l'API. Aucun OAuth ni device flow.
  - Page de création du jeton avec `description=DragonTree` (`auth.rs:219-238`).
  - git est authentifié par variables d'environnement `http.extraHeader` (`auth.rs:20`).
- **API** :
  - `verify`, `verify_new`, `list_repos` (`lib.rs:33-62`) ;
  - `list_pull_requests` (PR créées par moi ou à relire) (`open_pulls.rs:96`) ;
  - `create_pull_request` (`pullrequest.rs:48`) ;
  - `RepoLocation`, `account_matches_url`.
- **Client HTTP** :
  - ureq bloquant, TLS natif, proxy Windows WinHTTP avec PAC/WPAD (`proxy.rs`) ;
  - User-Agent `"DragonTree/0.1"` (`http.rs:15`) ;
  - fonctions HTTP en `pub(crate)` ;
  - messages d'erreur traduits par `dragontree-i18n` (`model.rs:139-160`).
- **Pour Owlery : bonne base, avec des ajouts.**
  - Réutilisables directement : `Account`, keyring, `verify`, le client HTTP derrière proxy, et `list_pull_requests` (déjà une source de boîte de réception).
  - À ajouter : API de notifications (GitHub `/notifications`, éléments de travail et mentions Azure DevOps), portées correspondantes, client HTTP public, nom de service et User-Agent configurables, ETag et limites de débit, éventuellement OAuth.

## 6. Réglages et état
- **Emplacements** :
  - `%APPDATA%\DragonTree\settings.json` (`settings.rs:257,612`) ;
  - cache `%LOCALAPPDATA%\DragonTree\repos-cache.json` ;
  - `app.ron` d'eframe ;
  - `--profile` redirige le tout.
- **Format** : une seule structure `Settings` en JSON indenté (`settings.rs:82`). Elle mêle préférences (thème, langue, zoom, comptes, terminal, IA) et disposition (onglets ouverts, espaces de travail, largeurs).
- **Écriture** :
  - atomique, fichier temporaire puis renommage (`settings.rs:621-629`) ;
  - à chaque image « sale » (`app.rs:790`).
  - Un fichier illisible n'est jamais écrasé : il est relu puis fusionné à trois versions (`settings.rs:411-470`). Un fichier invalide est mis de côté.
- **Champs inconnus** : tolérés à la lecture (`#[serde(default)]`, test `settings.rs:1040`), mais **perdus à l'écriture**. Un fichier partagé avec d'autres modules perdrait leurs clés.
- **Surveillance** : aucune sur le fichier de réglages.

## 7. IPC
- **Ouverture depuis l'extérieur** : `dragontree.exe <dossier…>` ou `--open` envoie un JSON `{version:1, open:[…]}` (`single_instance.rs:44-73`).
  - Le tube n'est utilisé que s'il appartient au même utilisateur (vérification du SID).
  - L'instance principale ouvre les dépôts (`AppAction::OpenRepo`) puis passe au premier plan.
  - Le seul « verbe » possible est l'ouverture d'un dépôt.
- **Fawkes** : aucun lien. « Ouvrir un terminal » lance Windows Terminal, pwsh, Git Bash ou cmd (`terminal.rs:199`, via `Shared::open_terminal`, `app.rs:285`). C'est le point d'accroche naturel pour renvoyer vers Fawkes.

## 8. Arrière-plan
- **Threads** :
  - `Jobs::spawn` : un thread par tâche (`jobs.rs:44`) ; `EventSender` appelle `ctx.request_repaint()` (`jobs.rs:22-26`) ;
  - files séquentielles par dépôt (`op_queue.rs`) ;
  - scans des dépôts, syntect, chargement des polices, écoute du tube.
- **Surveillance** : notify-debouncer-mini, 250 ms, récursif par onglet de dépôt (`repo/watcher.rs:117`).
- **Minuteries** : `request_repaint_after` pour le fetch automatique (`repo/mod.rs:1315,1338`).
- **Fenêtre détachée** : `request_repaint()` vise le viewport courant (en pratique la racine), et `logic` ne tourne que pour la racine. Il faudra `request_repaint_of(id)`.

## 9. Raccourcis
- **Registre** (`commands.rs:785-811`) :
  - Ctrl+T, Ctrl+W, Ctrl+Maj+T, Ctrl+P, Ctrl+O, Ctrl+B, Ctrl+0, Ctrl+, ;
  - Ctrl+Maj+ C / A / L / . / D / F / U / P / B / H ;
  - Ctrl+= / - / +, F5.
- **Hors registre** : Ctrl+Tab et Ctrl+1…9 (`app.rs:1098-1112`).
- **Consommation** : `ctx.input_mut` sans tenir compte du focus, même quand la commande est grisée (`app.rs:1090`).
- **Touches de vue** :
  - flèches, Page préc./suiv., Début/Fin dans l'historique (`history.rs:254`) ;
  - Espace, Entrée, Suppr dans les changements (`changes.rs:209-221`) ;
  - seul garde-fou : « aucun widget egui n'a le focus ».
- **Conflits forts** avec Fawkes (Ctrl+Maj+C, Ctrl+W/T/Tab/P, F5…) et avec une palette ou des onglets de la suite.

## 10. Identifiants egui génériques
- **Globaux (`Id::unique`)** :
  - `"settings-window"`, `"about-window"`, `"shortcuts-window"`, `"accounts-window"`, `"clone-window"`, `"log-window"`, `"pull-requests-window"`, `"ssh-keys-window"`, `"git-identity-window"` ;
  - `"closing-dialog"`, `"workspace-dialog"`, `"ssh-confirm"`, `"quick-open"` ;
  - `"tab-rects"` (données en mémoire, `tabbar.rs:45/87`) ;
  - `"branch-drag-ghost"`, `"branch-drop-menu"`, `"file-drag-ghost"`.
- Seul `"dragontree-toasts"` est préfixé.
- Les sels des panneaux et des `ScrollArea` sont relatifs à leur parent : pas de risque.

## 11. Viewports
Aucun. Tout tient dans le viewport racine : les fenêtres secondaires sont des `egui::Window` ou `Modal`. `DragonTreeApp` n'est pas `Sync` (il contient un `Receiver`) : préférer `show_viewport_immediate`, sinon il faudrait un `Arc<Mutex>`.

## 12. Documentation
- **Super app** : rien sur la suite, Fawkes, les autres modules, le docking ou Tailscale. « Reste ouvert » ne liste que des limites, rien n'est planifié (`docs/feuille-de-route.md:179-197`).
- **i18n** : `feuille-de-route.md:45-60`, RV5 (:624-646, `--lang` temporaire, textes non retraduits), `README.md:215-230`.
- **Thème** : `README.md:168-170` seulement.
- **Comptes** : `README.md:232-259` (PAT, portées, proxy). Le renommage (`feuille-de-route.md:32-43`) fixe les dossiers `DragonTree` et le service « DragonTree », avec **« pas de code de migration »**, un précédent à trancher pour des comptes partagés.
- **Raccourcis** : arbitrages en :73-84.

## Bloquants pour l'intégration (du plus au moins coûteux)
1. **Séparer réglages communs et réglages du module** (`Settings` monolithique, fusion à trois versions, champs inconnus perdus) et laisser à l'hôte les préférences et les comptes : 3 à 5 j.
2. **Extraire l'API de module et un wrapper autonome** : `ViewportCommand`, fermeture, instance unique, options de ligne de commande : 2 à 4 j.
3. **Router le clavier selon le focus du module** (consommation globale, garde-fou « aucun focus ») : 1 à 2 j.
4. **Thème, polices, zoom et langue gérés par l'hôte** (`set_fonts` qui remplace tout, `set_lang` à chaque image) : 1 à 2 j.
5. **Comptes partagés** (service keyring « DragonTree », comptes rangés dans les réglages de DragonTree, User-Agent, portées), plus la décision de migration : 1 à 2 j.
6. **Catalogues i18n par domaine** : 1 j.
7. **Fenêtre détachée** : réveils ciblés par viewport, viewport immédiat, position des notifications : 1 j.
8. **IPC de la suite** (module et verbe dans le message) : 1 j.
9. **Préfixer les Id globaux** et filtrer les fichiers déposés : 0,5 j.
10. **Observateur git en `OnceLock`** : 0,5 j.

## Briques réutilisables pour toute la suite
- `dragontree-i18n`, une fois les catalogues par domaine en place : `tr!`/`trn!`, langue globale, `Lang::system`, test des catalogues.
- `dragontree-forges` : `Account`, keyring, vérification, HTTP derrière proxy WinHTTP/PAC avec le magasin de certificats Windows, PR « à relire », `RepoLocation`.
- `SettingsFile` (`settings.rs`) : écriture atomique, fichier illisible jamais écrasé, fusion. À rendre générique sur `T`.
- `single_instance.rs` : mutex, tube, vérification du SID, JSON versionné. Base de l'IPC de la suite.
- `jobs.rs` (`EventSender`, `Jobs`, `SerialWorker`) et `op_queue.rs`.
- `theme.rs` (`Palette`, visuels, polices) et `icons.rs` avec Phosphor : base d'un design system commun.
- `toasts.rs`, `widgets.rs`, le principe de `closing.rs`, `console.rs`.
- `build.rs` pour l'icône sans compilateur de ressources, `app_icon.rs`.
- `terminal.rs`, `editors.rs`, `external_tools.rs` : détection des outils installés.
- `highlight.rs` : syntect en arrière-plan avec cache LRU.
- Les tests egui_kittest.
