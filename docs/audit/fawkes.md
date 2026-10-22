# Audit de Fawkes en vue de la suite (22/10/2026)

Audit en lecture seule du dépôt voisin `../Fawkes` (références relatives à sa racine).

## 1. Structure
- **Binaire seul.** Il n'y a ni `[lib]` ni `src/lib.rs` ; tous les modules sont déclarés dans `src/main.rs:4-23`. `build.rs:63` lie une ressource `.res` (version, icône) aux seuls exécutables du paquet.
- **`main()`** (`src/main.rs:83-104`) enchaîne l'instance unique, la migration Tessera, `persist::load()` puis `run_native("Fawkes", …)`.
- **`ViewportBuilder`** :
  - titre « Fawkes », `app_id("fawkes")`, minimum 640×400 ;
  - taille, position et état agrandi repris de `st.window` ;
  - `with_decorations(settings.native_title_bar)`, à `false` par défaut : la barre de titre est dessinée par Fawkes ;
  - icône phénix générée (`:66-69`) ;
  - eframe est compilé sans la feature `persistence`.
- **Type de l'App.** `App` (`src/app.rs:108-140`). `App::new(cc, st, problems)` (`:159`) n'utilise de `cc` que `cc.egui_ctx` : passer un `&egui::Context` suffit.
- **`impl eframe::App`** (`:1203-1312`) :
  - `raw_input_hook`, qui sert à la capture d'essai ;
  - `logic(ctx, frame)` (`:1211`) : tout ce qui n'est pas de l'affichage, appelé même fenêtre réduite ;
  - `ui(ui, frame)` (`:1253`) ;
  - `on_exit` (`:1288`) ;
  - pas de `save()`.
- **Panneaux : déjà dessinables dans un `Ui`.** En egui 0.37, `Panel::show(ui)` est l'ancien `show_inside`. Le menu et le titre (`ui/menu.rs:19,64`), les groupes (`sidebar.rs:14`), les onglets (`tabs.rs:16`) et la grille (`panes.rs:112`, `CentralPanel::show(ui)`) se dessinent dans un `Ui` arbitraire.
- **Ce qui reste au niveau `Context` :**
  - 8 `Modal` (`dialogs.rs:252,531,620,707,862,1041,1069`, `palette.rs:95`), qui grisent et bloquent toute la fenêtre ;
  - des `Area` ancrées à l'écran : le toast (`ui/mod.rs:171`) et la liste de recherche (`search_all.rs:194`) ;
  - `ctx.content_rect()` (`dialogs.rs:867`) ;
  - la barre de titre (`menu.rs:52-110`), qui suppose occuper le haut de la fenêtre ;
  - `track_window` (`app.rs:1175`), qui enregistre la géométrie de la fenêtre.
- **Pour exposer « dessine-toi dans ce Ui » :**
  - créer `lib.rs` et une interface de module `new(ctx, data_dir)`, `logic(ctx)`, `ui(ui)`, `set_hidden()`, `on_exit()` ;
  - réduire `main.rs` à un hôte mince ;
  - réserver au mode autonome la barre de titre, les décorations, la géométrie et l'instance unique ;
  - afficher les menus Actions et Paramètres dans le panneau ;
  - borner les `Modal` et `Area` au rectangle du module.

## 2. État global au processus
- **`app::SHUTTING_DOWN`** (`app.rs:24`) : posé par la procédure de fenêtre. Dans la suite, c'est l'hôte qui doit diffuser l'arrêt de Windows à tous les modules.
- **`winhook`** (`winhook.rs:24-29` : `PREV_PROC`, `WINDOW`, `CTX` et `ON_END` en `OnceLock`, `CAPTION`).
  - Il remplace la procédure de fenêtre par `SetWindowLongPtrW(GWLP_WNDPROC)` (`:134`), sur une fenêtre trouvée **par son titre exact « Fawkes »** (`:107-117`).
  - Il gère `WM_NCHITTEST` (barre de titre dessinée par Fawkes), le clic sur l'icône de notification et `WM_QUERYENDSESSION`/`WM_ENDSESSION`.
  - Dans la suite, le titre change : `install` échoue à chaque image (`app.rs:1212`). La sauvegarde à l'arrêt de Windows et le retour par clic sur la notification sont alors perdus.
  - `ON_END` n'accepte qu'un seul rappel et `CAPTION` un seul propriétaire. Un seul HWND est connu, donc une fenêtre détachée est ignorée.
- **`theme::CURRENT`** (`theme.rs:265`) est lu par `theme::p()` à 140 endroits, dans 15 fichiers.
- **Icône de notification** : `notify::CLICKED` (`notify.rs:25`), `uID=1` et le message `WM_APP+0x46` (`:21-22`) sur le HWND principal. Collision si un autre module fait de même.
- **Style et polices de egui** :
  - `ctx.set_fonts` (`theme.rs:337`) remplace **toutes** les polices ;
  - `set_theme`, `set_visuals_of` et `global_style_mut` (`:346-378`) imposent le style à toute la fenêtre, et sont réappliqués à chaque changement de thème (`app.rs:1256-1259`) ;
  - pas de `set_zoom_factor`.
- **`ViewportCommand`** :
  - `Decorations`, envoyé quand le réglage change (`app.rs:1263`) ;
  - `Minimized`, `Maximized` et `Close` (`menu.rs:75-82`) ;
  - `RequestUserAttention` (`alert.rs:58,84`, `commands.rs:198`), qui fera clignoter toute la suite.
- **Instance unique** : mutex `Local\Fawkes-single-instance[:dossier]` puis `FindWindowW("Fawkes")` (`main.rs:36-63`).
- **Rien de plus global** : pas de `RegisterHotKey` ni de hook clavier. Le lancement au démarrage passe par la valeur `Fawkes` de la clé `Run` (`autostart.rs:15-17`).
- **Focus** : un terminal reprend le focus dès que rien d'autre ne l'a (`panes.rs:454`). Il le prendra donc aux autres modules.

## 3. Thème
- **Palettes** : trois palettes statiques `GRAPHITE`, `FLUENT` et `NUIT` (`theme.rs:83,144,205`). `Palette` (`:25-77`) mêle couleurs d'interface, de widgets et de terminal (curseur, 16 couleurs ANSI).
- **Stockage** : `ThemeId` (`model.rs:47-67`), rangé dans `settings.theme`.
- **Contrôle** : un test vérifie les contrastes WCAG (`theme.rs:411`).
- **Polices** : Cascadia Mono ou Consolas, Segoe UI et Segoe UI Symbol, lues dans le dossier Fonts de Windows (`:291-338`). Les familles nommées `term` et `term-primary` (`term/render.rs:24-25`) servent à `Metrics` (`render.rs:91`). Si l'hôte réinstalle ses polices sans elles, egui panique.

## 4. Langue
Le français est codé en dur partout, dans plusieurs centaines de chaînes :
- menus, dialogues et toasts ;
- messages écrits dans les shells (`restore.rs`) ;
- notifications (`notify.rs:104`) et noms de touches (`shortcuts.rs:176`) ;
- page web du compagnon Go.

Aucune bibliothèque de traduction n'est utilisée. La ressource de version est en fr-FR (`build.rs:21`).

## 5. Comptes et identifiants
- **Aucun compte** : ni GitHub, ni ADO, ni keyring, ni Gestionnaire d'identification.
- **Seule exception, Tailscale** : la connexion se fait par un lien https (`dialogs.rs:812`). La clé du nœud est en clair dans `%APPDATA%\Fawkes\tailnet` ; un chiffrement DPAPI est prévu (`feuille-de-route.md:137`).
- `profiles.rs:32` retire `CLAUDE_CODE_MESSAGING_TOKEN` de l'environnement des shells.

## 6. Réglages et état
- **Un seul fichier** : `%APPDATA%\Fawkes\state.json` (`persist.rs:11-20,58`), déplaçable par `FAWKES_DATA_DIR`. Il contient réglages **et** état, dont `settings` et `window` (`model.rs:341-375`).
- **Fichiers voisins** : `scrollback\<id>.ans`, `tailnet\`, `fawkes.bashrc`, `.bak`, `state.avant-<version>.json`, `*.illisible-*`.
- **Écriture** :
  - un seul thread s'en charge (`persist.rs:284-321`) ;
  - elle est atomique (fichier temporaire, `sync_all`, renommage, `:344-355`), avec copie `.bak` (`:256-262`) ;
  - elle a lieu toutes les 2 s si l'état a changé, et toutes les 60 s pour les tampons (`app.rs:151-152,1243-1248`).
- **Champs inconnus conservés** grâce à `#[serde(flatten)] extra` (`model.rs:139,218,298,363`), avec un test (`persist.rs:448`).
- **Versionnage** :
  - `STATE_VERSION = 1` et `saved_by` ;
  - copie de l'état avant tout changement de version (`persist.rs:232-254`) ;
  - lecture tolérante, partie par partie (`:131-178`).
- **Pas de surveillance de fichier** : un réglage commun modifié ailleurs ne serait pas relu.

## 7. IPC
- **Rien n'est implémenté** : ni tube nommé, ni socket.
- **Prévu** : une API locale par canal nommé (`feuille-de-route.md:135`, `journal.md:49-52`).
- **Interface interne réutilisable** dans un même processus :
  - l'énumération `Action` (`app.rs:67-106`), dont `NewTerminal(NewTerm{profile, cwd, group, claude: NewClaude{resume, remote_control…}, command…})` (`:42-64`) ;
  - `create_terminal` (`:479`) et `run_command` (`:1123`).

## 8. Travail en arrière-plan
- **Threads** :
  - `persist` ;
  - `proc-monitor`, qui tourne toutes les 2 s (`procs.rs:115-131`) ;
  - pour chaque terminal, `pty-read` et `pty-write` (`term/mod.rs:161-195`), plus des threads courts (`:221,282`) ;
  - sondage de l'environnement (`app.rs:173`), sélecteur de dossier (`ui/mod.rs:110`), liste des sessions (`dialogs.rs:151`) et lecteur du compagnon (`tailnet.rs:114`).
- **Minuteries** : `logic` toutes les 2 s (`app.rs:1250`), démarrages échelonnés (`:295`), clignotement des alertes (`alert.rs:35`).
- **Ce qui doit tourner vue masquée** : tout `logic()` (suivi des processus, démarrages, alertes, notifications, tailnet, sauvegardes, arrêt de Windows).
- **Risque, vue masquée** : la visibilité des terminaux est calculée dans `ui()` (`app.rs:1277-1285`). Si l'hôte n'appelle plus `ui()`, les terminaux restent « visibles » et chaque sortie de terminal redessine la suite.
- **Risque, fenêtre détachée** : les `request_repaint()` lancés depuis les threads visent le viewport racine. Pour un viewport différé, il faut `request_repaint_of(id)`.

## 9. Raccourcis clavier
- Les raccourcis par défaut sont définis dans `shortcuts.rs:116-140`.
- **Problème** : `handle_shortcuts` (`app.rs:1092-1121`) **retire les événements de toute la fenêtre**, quel que soit le module qui a le focus. Seule exception : un dialogue de Fawkes ouvert.
- **Conflits probables avec les autres modules** :
  - Ctrl+Tab, Ctrl+Maj+Tab et Ctrl+Page↑/↓, qui servent d'ordinaire à changer d'onglet dans un ancrage ;
  - Ctrl+, (réglages) et Ctrl+Maj+P (palette) ;
  - Ctrl+Maj+F/G/C/V/T/W/N/R/M/A ;
  - les boutons de souris, s'ils sont associés à une commande.
- Le clavier du terminal n'est lu qu'avec le focus (`panes.rs:590`) ; il verrouille alors Tab, les flèches et Échap (`:466`).

## 10. Identifiants egui génériques
En 0.37, `Id::unique` est global, pas relatif au parent.
- **Noms génériques** :
  - `"settings"` (`dialogs.rs:862`), `"palette"` (`palette.rs:93`), `"toast"` (`ui/mod.rs:171`) ;
  - `"search-all"` et `"search-all-list"` (`search_all.rs:103,194`) ;
  - `"open-terminal"`, `"group-dialog"`, `"rename-dialog"`, `"confirm-close"`, `"confirm-delete-group"`, `"confirm-sensitive"` ;
  - `"drag-ghost"` (`panes.rs:430`).
- **Tuples sur des numéros séquentiels** (u64 à partir de 1) : `("term", id)` (`panes.rs:441`), `("find", id)`, `("divider", gid, i)`, `("tab-close", id)`, `("window-button", tip)`.
- Les `Panel` sont salés par leur parent : sans risque.

## 11. Viewports multiples
- Aucun `show_viewport_*` : un seul viewport.
- `App` n'est pas `Send + Sync`. Un viewport différé imposerait `Arc<Mutex<…>>` ; un viewport immédiat s'en passe.

## 12. Tailscale et compagnon mobile
- **Lancement** : `tailnet.rs` lance `fawkes-tailnet.exe` (`:20,72-78`), pris à côté de l'exe ou désigné par `FAWKES_TAILNET_EXE`. Arguments : `--dir %APPDATA%\Fawkes\tailnet --hostname --version`.
- **Dialogue avec le compagnon** :
  - l'entrée standard sert de cordon de vie ;
  - la sortie standard envoie des lignes JSON `starting|needs_login|running|error` (`:33-43`) ;
  - l'arrêt ferme l'entrée, puis tue le processus après 3 s (`:142-155`).
- **Pilotage** : `sync_tailnet` dans `logic` (`app.rs:392-404`), selon les réglages `mobile_tailnet` et `tailnet_hostname` (`model.rs:294,296`) ; écran de réglage dans `dialogs.rs:934`.
- **Compagnon** : en Go avec tsnet (`compagnon/main.go`), journaux de Tailscale coupés. Il sert sur le port 80 une page d'essai qui indique qui s'y connecte.
- **Compilation** : `tools/compagnon.ps1` (Go local, `GOTOOLCHAIN=local`), vers `target\{debug,release}`.
- **À mutualiser pour les autres modules** :
  - un seul nœud par PC, avec un nom par défaut générique au lieu de `fawkes-<pc>` (`tailnet.rs:47-65`) ;
  - l'état du nœud dans le dossier de la suite ;
  - un routeur HTTP par module ;
  - la structure `Tailnet` et le protocole en lignes JSON dans une crate commune ;
  - les réglages Tailscale dans les réglages communs.

## 13. Liens vers d'autres modules
Aucun dans le code ; ils n'existent que dans la documentation.

## 14. Documentation, hors `reflexions.md` §3
- **Ordre prévu** (`feuille-de-route.md:135`) :
  1. une API par canal nommé, pour ouvrir un shell ou Claude dans Fawkes **déjà ouvert, sans jamais le démarrer** ;
  2. une boîte à outils commune ;
  3. des configurations communes ;
  4. la suite.
- **Mobile** (`feuille-de-route.md:137`, `journal.md:106-109`) : la synchro mobile sera intégrée à la suite pour tous les modules, tailscale-rs remplacera le compagnon quand il sera stable, et la clé du nœud sera chiffrée par DPAPI.
- **Démon séparé** (`feuille-de-route.md:130`) : un démon hôte des terminaux, séparé de l'interface, est noté. Utile si la suite doit redémarrer sans tuer les terminaux.
- **DragonTree** (`journal.md:49-52`) : `dragontree.exe <dossier>` ouvre un onglet, mais sans instance unique.
- **Choix des modules** (`journal.md:141-147`) : Menagerie est une crate bibliothèque et exécutable, Patronus un workspace. Owlery range son jeton GitHub dans le coffre de Windows (ou passe par la CLI `gh`) et ne lit pas celui de DragonTree.
- **Thèmes et police** : le README (`:37-42`) précise que les thèmes colorent aussi le terminal. Une police configurable reste à faire (`feuille-de-route.md:122`).
- **Langue** : rien en dehors de `reflexions.md:289,360`.

## Bloquants pour l'intégration, du plus coûteux au moins coûteux
1. **Traduction**, si la langue commune doit s'appliquer : extraire plusieurs centaines de chaînes, messages écrits dans les shells compris. Environ 3 à 5 jours.
2. **Thème et polices globaux** : palette fournie par l'hôte, en séparant les couleurs de la suite de celles du terminal ; plus de `set_fonts` ni de `set_style` en mode suite ; familles `term*` ajoutées aux polices de l'hôte. Environ 1,5 à 2 jours.
3. **Crochets Windows** (`winhook`, arrêt de session, notification, barre de titre) : à remonter dans l'hôte, avec diffusion aux modules et gestion de plusieurs fenêtres. Environ 1 à 2 jours.
4. **Séparation des réglages** (communs, propres à Fawkes, géométrie de la fenêtre) avec relecture à chaud. Environ 1 à 2 jours.
5. **Fenêtres modales et overlays** bornés au rectangle du module. Environ 1 jour.
6. **Extraction de `lib.rs`** et de l'interface de module (`new`, `logic`, `ui`, `set_hidden`, `on_exit`), avec instance unique et dossier de données fournis par l'hôte. Environ 1 jour.
7. **Raccourcis et focus** : n'agir que si le focus est dans Fawkes, supprimer la reprise de focus automatique, laisser la palette, les réglages et les onglets à l'hôte. Environ 0,5 à 1 jour.
8. **Fenêtre détachée** : `request_repaint_of`, gestion de la visibilité, et `Arc<Mutex>` si le viewport est différé. Environ 0,5 jour.
9. **Espace de noms des `Id`**. Environ 0,5 jour.

Hors intégration dans un même processus, mais à prévoir : le tube nommé (1 à 2 jours) et la mutualisation du compagnon Tailscale (1 à 2 jours).
