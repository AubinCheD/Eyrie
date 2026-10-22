# Audit d'Owlery et Grimoire en vue de la suite (22/10/2026)

Audit en lecture seule des dépôts voisins `../Owlery` et `../Grimoire`. Chemins relatifs à chaque dépôt.

## Owlery 0.2.0

**1. Structure.**
- Une lib et un bin. `main.rs:12-120` câble toutes les dépendances ; `run_native` est en `main.rs:111`.
- `NativeOptions` ne porte que le viewport et `centered` (`main.rs:93-97`, `app.rs:992-1005`) : décorations Windows, pas de barre de titre perso.
- Constructeur : `App::new(cc: &CreationContext, StartOptions, Loaded)` (`app.rs:228`). Il n'utilise que `cc.egui_ctx`.
- `impl eframe::App` (`app.rs:1007-1036`) sépare déjà `logic(ctx, frame)` (sondages, sauvegarde) et `ui(ui, frame)`.
- `ui::draw(app, ui)` (`ui/mod.rs:18-36`) dessine `Panel::top/right` et `CentralPanel` dans le `Ui` reçu. Il est donc embarquable : en egui 0.37, l'id d'un panneau est relatif au `Ui` parent.
- En revanche, plusieurs éléments sont posés sur le `Context` et flottent sur toute la fenêtre hôte :
  - les fenêtres « Comptes » et « Réglages » (`ui/accounts.rs:43`, `ui/settings.rs:20`) ;
  - les `Modal` palette et Fawkes (`ui/palette.rs:68`, `ui/detail.rs:217`), qui bloqueraient toute la fenêtre ;
  - les toasts en `Area` (`ui/mod.rs:205`).
- Le code est pensé pour l'injection (traits `FawkesLink`, `Notifier`, `SecretStore`, `Fetcher` ; `StartOptions` en `app.rs:43-122`), mais il n'y a pas de trait « module ».
- À changer : constructeur `App::new(&Context)`, plus de `frame`, `logic`/`ui` exposés hors `eframe::App`, interface hôte.

**2. État global au processus.**
- Statiques : `MAIN_WINDOW` (HWND, `win/mod.rs:31`) ; `WINDOW`, `QUEUE`, `TASKBAR_CREATED`, `SHOWN`, `ICONS` (`win/tray.rs:33-36,133,174`), avec un fil et une fenêtre message propres.
- Instance unique : mutex `Local\Owlery-single-instance` (`main.rs:29`, `contract.rs:156`), verrou `state.lock` (`state.rs:410`), serveur de tube (`main.rs:56`).
- Thème : `ctx.set_theme` (`app.rs:983`, appelé en `app.rs:309` et `ui/settings.rs:33`).
- Commandes de viewport :
  - `ViewportCommand::Title` avec le compteur (`app.rs:901`), qui renommerait la fenêtre hôte ;
  - `Visible/Minimized/Focus` puis `SetForegroundWindow` (`app.rs:851-855`) ;
  - suivi de la taille et de la position (`app.rs:906-935`) ;
  - HWND lu via `frame.window_handle()` (`app.rs:967-980`).
- Réveil à 1 Hz permanent (`app.rs:1022`) ; `request_screenshot` pour la capture (`capture.rs:18`).
- Pas de sous-classement Win32 ni de raccourci global.

**3. Thème.** Pas copié de Fawkes. Seulement Système/Sombre/Clair d'egui (`state.rs:26-31`), couleurs prises dans `ui.visuals()` ; quelques couleurs en dur (`ui/mod.rs:97,218-223`). Il suivra les visuels de l'hôte si `apply_theme` est neutralisé. Les glyphes ⚙👤⟳ viennent de `epaint/monochrome_emoji_fonts` : la police de l'hôte doit les couvrir.

**4. Langue.** Français en dur partout (libellés, erreurs, `ago()` en `ui/mod.rs:248`, dates `%d/%m`). Aucune i18n, alors que DragonTree a `dragontree_i18n`.

**5. Comptes.**
- Forges : GitHub (Enterprise compris), GitLab (gitlab.com ou instance), Azure DevOps (Services et Server).
- Modèle : `Account {id, forge, host, orgs, auth: Token|Gh, username, display_name, unknown}` (`forges/mod.rs:29-56`), rangé dans `state.json`.
- Jeton : Gestionnaire d'identifiants, service `Owlery`, clé `account:<id>` (`secrets.rs:5,53`, `forges/mod.rs:81`). L'ancienne clé `github:token` est migrée (`forges/mod.rs:143-164`).
- Auth : PAT saisi et vérifié avant stockage. Bearer pour GitHub et GitLab (`github.rs:54`, `gitlab.rs:20`), Basic `:PAT` pour Azure (`azure.rs:33`). Ou compte implicite « gh » qui appelle `gh api` (`github.rs:66-130`), pas `gh auth token`.
- Ni OAuth device flow, ni `az`. Aide de création de jeton et droits minimaux par forge (`ui/accounts.rs:13-35`).
- **Très proche d'un gestionnaire commun** : même modèle, même `title()` et même clé que `DragonTree/crates/dragontree-forges` (service `DragonTree`, `account:<id>`). `SecretStore` est déjà injectable, l'hôte peut donc fournir un coffre « Suite ». Manquent : le partage opt-in, les droits annoncés et l'OAuth.

**6. Réglages et état.**
- Fichier : `%APPDATA%\Owlery\state.json`, ou `OWLERY_DATA_DIR` ; la démo écrit dans `demo\` (`state.rs:131-143`).
- Versionné par `version` et `saved_by`, avec copie `state.<ancienne version>.json` (`state.rs:185-187`).
- Lecture tolérante champ par champ ; **champs inconnus gardés à tous les niveaux** (`flatten`) ; un fichier illisible est mis de côté.
- Écriture : temporaire puis `rename`, sans `fsync` (`state.rs:396-402`).
- L'état mélange réglages, cache, filtres et `window`.

**7. Contrat (`docs/contrat-local.md`).**
- **Transport** : tube `\\.\pipe\owlery-<SID>[-fnv]`, réservé à l'utilisateur (SDDL), clients distants refusés, première instance seulement. Une ligne JSON de 64 Kio au plus, `"v":1`, champ `demande`, réponse `ok` ou `erreur`. Option `--contract-version`.
- **Reçoit** : `afficher` (`type`), `actualiser`, `etat` (`depot_url` → `a_relire`, pour un badge DragonTree). Un second lancement envoie `afficher` puis se termine.
- **Émet vers Fawkes** : `ouvrir` avec `dir`, `titre`, `claude`, `worktree`, `consigne` (collée sans Entrée) et `de`. Si Fawkes n'écoute pas : dialogue Démarrer (sans argument, attente de 20 s) / Copier / Annuler.
- **DragonTree** : `dragontree.exe <dir>` aujourd'hui ; `ouvrir` avec `branche` souhaité.
- **§5, comptes partagés** : service « Suite », clé `account:<id>`, liste sans jeton dans `%APPDATA%\Suite\accounts.json`, partage compte par compte. L'hôte fournit la liste et le dialogue, les modules ne font que lire.
- **§6** : événements pour Patronus.
- **Implémenté côté Owlery** : §1 à 3 (`contract.rs`, `win/pipe.rs`). §4 seulement par ligne de commande. §5 et §6 ne le sont pas.
- **Pourquoi les tubes et `raw-window-handle`** : les tubes portent le contrat (instance unique, badges, Fawkes). Le HWND est capturé à la première image pour faire `SetForegroundWindow` sur `afficher` ou sur un clic de bulle, celui-ci arrivant dans le fil de la zone de notification, hors egui. Le client appelle `AllowSetForegroundWindow` (`win/pipe.rs:125`).

**8. Arrière-plan.**
- Un fil par compte et par lecture (`app.rs:355-398`), lecture toutes les 5 min par défaut, reports échus, notifications, `Snapshot` publié pour le tube.
- **Doit tourner vue masquée.** C'est déjà dans `logic()`.
- Les notifications ne partent que si la fenêtre n'a pas le focus (`app.rs:481`). Dans la suite, le critère devient « module visible et focalisé ».

**9. Raccourcis.**
- 16 commandes à 2 combinaisons (`shortcuts.rs:93-112`), dont des **touches seules** : R, U, E, H, J, K, O, Entrée, Suppr, flèches. Aussi Ctrl+R/Z/F/G/W/P, Ctrl+Maj+P, Ctrl+,.
- `take_commands` **retire** les événements de `ctx.input_mut` selon le focus *global* (`app.rs:830`, `shortcuts.rs:311`). Dans un hôte partagé, il volerait les frappes d'un terminal Fawkes : avec le focus « Widget », seules Entrée et Espace sont épargnées.
- Conflits avec Fawkes : Ctrl+, et Ctrl+Maj+P.
- Ids : les titres de `Window` « Comptes » et « Réglages » sont globaux. Les `Panel`, `Grid` et `ComboBox` sont relatifs au `Ui`, donc sans risque si chaque module a son `push_id`.

**10. Ce que disent les docs sur la suite.**
- Spec §2 : badge dans la barre d'activité, entrées de palette (*Aller à la boîte de réception*, *PR à relire*, *CI en échec*).
- Feuille de route, pistes : « Module de la suite (barre d'activité, badge, palette commune), boîte à outils commune avec Fawkes et DragonTree (thèmes, raccourcis, notifications) ».
- README 41-47 : l'état reste dans son propre dossier, « aucune migration ».
- Journal : un workspace viendra quand une crate partagée apparaîtra.
- Rien sur Tailscale ni sur la langue.

**11. Code dupliqué.**
- Avec Fawkes :
  - `capture.rs`, copie de celui de Fawkes (`FAWKES_` devenu `OWLERY_`) ;
  - modèle de raccourcis « deux par commande » ;
  - versionnage d'état « comme Fawkes » ;
  - `single_instance`, `on_some_monitor`, `attach_parent_console` (`win/mod.rs`) ;
  - `win/tray.rs`, proche de `Fawkes/src/notify.rs` ;
  - `is_safe_url` (`actions.rs:8`).
- Autres : transport du tube (aussi présent dans Grimoire), toasts, et `forges/` (2 420 lignes), proche de `dragontree-forges`.

## Grimoire 0.2.0

**1. Structure.**
- Une lib et 2 bins. `lib.rs:1-4` le dit : l'exe est « une enveloppe fine, pour qu'elle devienne un module de la suite sans changement ».
- `run_native` en `main.rs:71`. `NativeOptions` ne porte que le viewport (`main.rs:65-70`) : décorations Windows, pas de barre perso, position non gardée.
- Constructeur : `App::new(&egui::Context, Loaded, Option<Request>, Receiver<Request>)` (`app.rs:146`), sans `CreationContext`.
- **Tout est dans `ui()`** (`app.rs:729-781`), il n'y a pas de `logic`.
- `Panel::top("top")` et `CentralPanel` sont dans le `Ui` reçu, et les vues `*_view.ui(ui, palette, …)` acceptent déjà un `Ui` quelconque.
- Une dizaine de `Window` sont posées sur le `ctx` : Palette, Préférences, Profils, Comparer, Tester… (`app.rs:499,570`, `ui/*.rs`).
- Pas de trait module.

**2. État global au processus.**
- Presque rien : un `OnceLock<Context>` local à `main` (`main.rs:51`) et `TMP_COUNTER` (`state.rs:201`).
- Mutex `Local\Grimoire-single-instance[:clé]` (`instance.rs:127,197`) et serveur de tube dans un fil (`main.rs:59`).
- Thème : `set_theme` et `set_visuals_of` avec `override_text_color` (`theme.rs:67-84`).
- Commandes de viewport :
  - `Minimized/Focus` (`app/requests.rs:52-53`) ;
  - `CancelClose`/`Close` sur la fermeture de la fenêtre (`app.rs:533-565`) ;
  - `Close` en mode capture (`capture.rs:51`) ;
  - taille lue sur le viewport (`app.rs:767-772`).

**3. Thème.** Le gabarit vient de Fawkes (`Palette`, `const fn rgb`), mais avec des palettes propres Sombre et Clair (`theme.rs:25-51`) au lieu de Graphite, Fluent et Nuit. La palette est passée aux vues, donc facile à remplacer par celle de l'hôte. En revanche, `override_text_color` imposerait sa couleur de texte à tous les modules.

**4. Langue.** Français en dur. La traduction est notée « si la suite le demande » (feuille de route X, design v2 §10).

**5. Comptes.**
- Aucune forge, aucun keyring.
- Les « profils » sont des comptes Claude, un dossier `CLAUDE_CONFIG_DIR` chacun. Le compte est lu dans `.claude.json`, champ `oauthAccount` (`profiles.rs:1-5,57-90`) ; `.credentials.json` n'est jamais lu.
- Pour l'hôte : on peut en faire un type de compte « Claude » commun, que Fawkes utiliserait avec `--config-dir`.

**6. Réglages et état.**
- `%APPDATA%\Grimoire\state.json` contient thème, raccourcis, dossiers récents, taille, profils et chemin de Fawkes.
- `%LOCALAPPDATA%\Grimoire\` contient l'index `conversations[-profil].json`, `copies\` et `limites\`.
- `GRIMOIRE_DATA_DIR` remplace les deux dossiers (`state.rs:95-113`).
- Écriture atomique avec `fsync` et une copie `.bak` (`state.rs:190-230`).
- **Champs inconnus perdus** (`state.rs:30`, message en 158).
- **Pas de verrou d'état** (`main.rs:42-46` le reconnaît).
- Les fichiers de Claude Code passent par `write.rs` : empreinte vérifiée, copies.

**7. Contrat.**
- **Émet** par ligne de commande : `fawkes.exe open --dir --claude --resume <id> --title [--config-dir]`. Le bouton n'est actif que si `--contract-version` répond au moins 1, et une confirmation est demandée. Si la session tourne déjà, Fawkes devrait montrer son terminal.
- **Reçoit** : `grimoire.exe [dir]`, `open --dir --tab --profile`, `search --query --dir`, `--contract-version`.
- **Tube** : `\\.\pipe\grimoire-<USERNAME>[-fnv]`, nommé d'après le **nom d'utilisateur** (Owlery utilise le SID). Champ **`action`** et erreur **`error`**, contre `demande`/`erreur` chez Owlery.
- §3 : une demande venue d'un clic dans une autre application de la suite se passe de confirmation.
- **Implémenté côté Grimoire** : tout le v1, y compris le SID dans le descripteur (feuille de route S1). La ligne 13 du contrat, qui dit le contraire, est périmée. Côté Fawkes rien n'existe, le bouton reste grisé.

**8. Arrière-plan.**
- Chargeur de configuration et indexeur dans des fils ; surveillance des fichiers toutes les 2 s (`watch.rs:11`) ; relevés de limites toutes les 20 s.
- Rien n'a besoin de tourner vue masquée, sauf la réception des demandes, aujourd'hui sondée dans `ui()`.
- `grimoire-statusline` est **un processus lancé par Claude Code**, pas par la GUI. Il écrit `limites\<compte>.json`. Il doit rester un exe livré avec la suite.

**9. Raccourcis.**
- Ctrl+P, Ctrl+Maj+P, Ctrl+O, F5, Ctrl+F, Ctrl+,, Ctrl+1 à Ctrl+8, Ctrl+Maj+O, Ctrl+Maj+N (`shortcuts.rs:87-103`).
- Ils sont lus **sans aucun test de focus** (`app.rs:365-398`).
- Conflits : Ctrl+, et Ctrl+Maj+P (avec Fawkes et Owlery), Ctrl+Maj+N (« Nouveau groupe » de Fawkes).
- Les titres de `Window` sont globaux.

**10. Ce que disent les docs sur la suite.**
- `lib.rs` et journal l.27 : la lib est le module, l'exe l'enveloppe.
- README 44-48 : état propre, aucune migration.
- Contrat §3 et §4 Q2 : mettre le contrat commun dans une crate partagée.
- Design v2 §1 : trois comptes Claude, bascule quand la limite est atteinte. Idée pour l'hôte : un indicateur des limites par compte.
- Rien sur Tailscale, sur un thème commun ni sur des comptes de forge.

**11. Code dupliqué.**
- Avec Fawkes : `theme.rs` (gabarit), `write_atomic` (proche de `Fawkes/src/persist.rs`).
- Avec Owlery : `instance.rs` (879 lignes), seconde implémentation du transport ; recherche de `fawkes.exe` (aussi dans `Owlery/src/win/mod.rs:87`).
- Variantes propres : `capture.rs`, `shortcuts.rs` (un troisième modèle de raccourcis), `attach_parent_console`.

## Bloquants pour l'intégration (du plus coûteux au moins coûteux)

**Owlery**
1. **Comptes communs** : coffre et liste « Suite » derrière `SecretStore`, à réconcilier avec `dragontree-forges` (une crate commune forges + comptes), en gardant le mode autonome. Environ 3 à 5 j.
2. **Raccourcis** : n'agir que si le module a le focus, ne plus consommer les événements globaux, passer par un registre de l'hôte. Environ 1 à 1,5 j.
3. **Couplage fenêtre** : titre, taille et position, HWND, `bring_to_front`, focus des notifications, à remplacer par une interface `Host` (badge, « me montrer », visible/focalisé). Environ 1 j.
4. **Identité de processus** : quand la suite héberge Owlery, elle doit tenir le mutex, `state.lock`, le tube `owlery-<SID>` et l'icône de notification. Sinon l'exe autonome démarre en parallèle sur le même état. Environ 1 j.
5. **Fenêtres et `Modal` sur le `Context`**, plus les titres globaux : à contenir dans la zone du module ou à router vers l'hôte. Environ 0,5 à 1 j.
6. **Thème** : ne pas appeler `apply_theme` sous l'hôte et masquer le réglage. Environ 0,25 j.

**Grimoire**
1. **Contrat** : harmoniser SID ou nom d'utilisateur, `action`/`demande`, `error`/`erreur` dans une crate commune ; sous l'hôte, router en interne au lieu du tube. Environ 1 à 1,5 j, partagé avec les autres modules.
2. **Thème** : passer la palette de l'hôte aux vues et retirer les visuels globaux, dont `override_text_color`. Environ 0,5 à 1 j.
3. **Raccourcis** : test de focus du module, résolution des conflits. Environ 0,5 à 1 j.
4. **Fermeture et fenêtre** : remplacer `CancelClose` sur la fenêtre hôte par une API « modifications non enregistrées ? », plus taille, `Minimized/Focus` et capture. Environ 0,5 j.
5. **État** : ajouter un verrou (la suite et l'exe autonome écrivent le même fichier) et garder les champs inconnus. Environ 0,5 j.
6. **Fenêtres sur le `ctx`** aux titres génériques. Environ 0,5 j.
7. **Séparer `logic()` de `ui()`**. Environ 0,25 à 0,5 j.
8. **Livrer `grimoire-statusline.exe`** avec la suite en gardant le chemin `limites\`. Environ 0,25 j.
