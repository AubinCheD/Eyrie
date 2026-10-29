# Eyrie sur le téléphone

Demande du 27/10/2026 : « l'application mobile pour Eyrie, en Rust, très performante, une UX irréprochable ; un
maximum des fonctionnalités du PC sans en être une copie (un module à la fois) ; toutes les commandes du téléphone
retranscrites sur le PC en temps réel (si Tailscale est connecté) ; appairage des appareils par QR code ». Questions
sautées à la demande de l'utilisateur : les choix ci-dessous sont les miens, marqués **(choix)** quand ils engagent.

## 1. Principes

- **Le PC est la seule source de vérité.** Le téléphone n'exécute rien : il montre ce que le PC lui envoie et lui
  demande des actions. Une action faite au téléphone est faite *sur le PC*, par le module concerné : l'écran du PC
  change de lui-même, comme si l'utilisateur avait cliqué. Eyrie le signale d'une bulle discrète (« 📱 Pixel 8 :
  Patronus › Lancer « build » ») et l'écrit au journal du pont.
- **Par le module lui-même, jamais à côté** (demande du 27/10 au soir : « TOUTES les commandes du téléphone doivent
  s'exécuter sur le PC, sur DragonTree aussi ») : une commande du téléphone prend le chemin du bouton du module —
  sa file d'opérations, ses comptes et identifiants, ses dialogues et notifications, son état « occupé ». Le module
  montre l'opération pendant qu'elle se fait et le téléphone lit ce que le module en dit. Une fusion en conflit
  est celle de DragonTree, un conteneur arrêté l'est par Menagerie, un diff appliqué l'est par Patronus. Seules
  les *lectures* que le module ne garde pas (le diff d'un fichier, le journal d'un conteneur) sont faites à part,
  sur le PC, dans un fil.
- **Temps réel** : le PC pousse ce qui change (pastilles, vue d'ensemble, notifications, écran ouvert, terminal
  suivi) ; rien n'est demandé en boucle par le téléphone.
- **Un module à la fois** : une barre de navigation en bas (Ensemble, trois modules, Plus), un écran par module,
  des sous-écrans empilés (retour par le geste ou le bouton du système).
- **Sécurité d'abord** (prérequis de l'audit du 27/10) : voir §4.

## 2. Ce qui est construit

| Pièce | Où | Rôle |
|---|---|---|
| `suite-mobile` | `crates/suite-mobile` | Protocole partagé PC ↔ téléphone : messages, écrans génériques, terminal, appairage, canal chiffré Noise. Ni egui ni Slint. |
| Pont mobile | `crates/eyrie/src/mobile/` | Dans Eyrie : écoute, appareils, appairage, journal, interrupteur, synchro, aiguillage des actions vers les modules. |
| Interface mobile des modules | `Module::mobile_*` (`suite-sdk`) et chaque adaptateur `crates/modules/*` | Chaque module décrit ses écrans et ses actions pour le téléphone. |
| Compagnon Tailscale d'Eyrie | `compagnon/` (Go, tsnet) → `eyrie-tailnet.exe` | Le nœud de la suite sur le tailnet, et le relais des connexions du téléphone vers Eyrie. |
| Nœud Tailscale du téléphone | `compagnon/telephone` (Go, tsnet) → `libeyrie_tailnet.so` dans l'APK | Le téléphone sur le tailnet par l'application elle-même ; ses connexions vers le PC passent par lui (§3). |
| Application mobile | `crates/eyrie-mobile` | Slint, Rust. Tourne sur Android et, pour le développement, sur le bureau dans une fenêtre de téléphone. |

**Android d'abord (choix)** : iOS ne se compile et ne se signe que sur un Mac. Le code de l'application ne dépend
d'Android que dans `android.rs` (caméra, liens, vibration) et `tailnet.rs` (le nœud Tailscale) : iOS viendra quand
Slint l'aura stabilisé et qu'un Mac sera disponible.

**Slint (choix)** plutôt qu'egui pour le téléphone : clavier virtuel et méthodes de saisie gérés par le système
(egui/winit ne les gèrent pas sur Android), défilement inertiel, animations, rendu GPU, interface déclarative. Licence
« royalty-free » de Slint : mention dans *À propos*.

## 3. Transport

- Le **compagnon d'Eyrie** (copie de celui de Fawkes, même clé chiffrée par DPAPI dans `%LOCALAPPDATA%`) écoute sur le
  tailnet, **port 4747**. Pour chaque connexion, il demande à Tailscale qui se connecte (`WhoIs` : compte et appareil),
  ouvre une connexion vers le port local d'Eyrie (127.0.0.1, port aléatoire), y écrit une ligne d'en-tête
  `{"jeton":…,"login":…,"noeud":…,"adresse":…}` puis relaie les octets dans les deux sens.
- Un compagnon qui s'arrête de lui-même est relancé par Eyrie (après 5 s, puis le double à chaque fois, 2 min au
  plus ; remis à zéro une fois le nœud connecté). Tailscale se reconnecte seul après une coupure du réseau.
- Eyrie lui donne l'adresse locale et un **jeton aléatoire** par son entrée standard (jamais en argument : la ligne de
  commande se lit depuis les autres processus). Une connexion locale sans le bon jeton est fermée.
- Le téléphone rejoint le tailnet **par Eyrie elle-même** (demande du 28/10 : « je dois être connecté à Tailscale via
  Eyrie, pas via l'appli Tailscale ») : l'application porte son propre nœud, tsnet en espace utilisateur, sans VPN
  Android ni application Tailscale. C'est `compagnon/telephone` (Go), compilé en bibliothèque partagée
  `libeyrie_tailnet.so` par `tools\android.ps1` et appelé par `crates/eyrie-mobile/src/tailnet.rs`. Il partage avec le
  compagnon du PC le paquet `compagnon/noeud` : le suivi de l'état et le relais des deux sens. `tailscale-rs`, lui, ne
  gère toujours pas Android (préversion d'avril 2026, relais DERP seulement).
  - **Connexion au compte** : au premier lancement, le nœud demande un lien de connexion à Tailscale ; « Se connecter à
    Tailscale » l'ouvre dans le navigateur, avec le même compte que le PC. L'appareil apparaît dans le tailnet sous
    `eyrie-<nom du téléphone>`. Déconnexion : Réglages › Tailscale.
  - **Connexions vers le PC** : l'application crée une paire de sockets Unix et en donne un bout au nœud, qui le relie
    au PC par le tailnet (`EyrieTailnetDial`) ; rien n'écoute sur la boucle locale du téléphone. Le canal Noise passe
    dedans, inchangé.
  - **Android** : la liste des interfaces réseau est refusée aux applications. Comme la fonctionnalité « androidbin »
    de Tailscale, le nœud déclare une interface unique avec les adresses de sortie choisies par le système (connexion
    UDP sans envoi). Le DNS passe par le résolveur du système (cgo), les changements de réseau par le sondage de
    Tailscale.
  - **Journaux** : dans le dossier du nœud (`journal.txt`, 4 Mo au plus), rien n'est envoyé à Tailscale
    (`logtail.Disable`).
  - **État du nœud** : sa clé reste dans le stockage privé de l'application, non sauvegardé (`allowBackup=false`). Elle
    n'est pas chiffrée en plus par le Keystore d'Android, contrairement à celle du PC chiffrée par DPAPI : piste notée
    au §8.
  - **Bureau** (développement) : pas de nœud, connexion directe par le réseau du poste.
- Le compagnon de Fawkes reste utilisable en secours pour le nœud seul (sans relais : le téléphone ne peut pas se
  connecter, la page Téléphones le dit).

## 4. Sécurité

Les sept prérequis de l'audit, et comment ils sont tenus :

1. **Point d'entrée unique dans l'hôte** : `mobile::Bridge` reçoit tout, vérifie tout, et seul l'appelle les modules
   (`Module::mobile_action`, `mobile_term_input`), toujours dans le bac à sable de l'hôte.
2. **Actions typées en liste blanche** : le protocole est un `enum` fermé. Une action de module n'est acceptée que si
   le PC l'a lui-même proposée dans le dernier écran envoyé à cet appareil (même chemin, même identifiant) ; une saisie
   de terminal, seulement pour le terminal que le téléphone suit. Une action qui ouvre un terminal (`Act::terminal` :
   un shell ou Claude dans Fawkes, « Corriger avec Claude » d'Owlery) ou qui en confie un à Remote Control demande
   aussi le droit *terminaux*, comme une commande de la palette qui en ouvre un (`CommandInfo::terminal` : « Nouveau
   terminal », « Rouvrir le terminal fermé » de Fawkes), envoyée seule ou touchée sur un écran générique.
3. **Appairage par appareil en plus de WhoIs** : chaque téléphone a sa clé (X25519) ; le canal est Noise
   `XX_25519_ChaChaPoly_BLAKE2s`, prologue `eyrie-mobile/1`. Le PC ne parle qu'aux clés appairées ; le téléphone ne
   parle qu'à la clé du PC appairée. Le compte Tailscale noté à l'appairage doit être le même à chaque connexion.
4. **Contrôle Host/Origin** : rien d'HTTP ni de WebSocket ; un navigateur ne peut rien envoyer d'utile (la première
   trame doit être un message Noise). Le port local n'écoute que 127.0.0.1 et exige le jeton du compagnon.
5. **Confirmation sur le PC** : un appairage n'aboutit qu'après un clic sur le PC, qui affiche le nom de l'appareil, le
   compte Tailscale et un code à six chiffres que le téléphone affiche aussi (tiré du hachage de la poignée de main).
6. **Journal persistant** : `mobile\journal.jsonl` (connexions, refus, appairages, actions ; jamais le texte saisi dans
   un terminal, seulement sa longueur), lisible dans la page Téléphones.
7. **Interrupteur** : « Accès depuis le téléphone » (page Téléphones, barre d'état, palette) coupe l'écoute, ferme
   toutes les connexions et retire le relais au compagnon ; « Couper tout accès à distance » arrête aussi le nœud.

Autres règles :
- **Ticket d'appairage** : secret de 32 octets, valable 5 minutes, à usage unique, 5 essais. QR :
  `eyrie://appairer?v=1&n=<nom du PC>&h=<nom MagicDNS>&a=<IP tailnet>&p=4747&k=<clé du PC>&s=<secret>`. Le téléphone et
  le PC se prouvent mutuellement le secret (MAC BLAKE2s sur le hachage de la poignée de main), et le téléphone vérifie
  la clé du PC lue dans le QR. **Code à saisir** en secours : dix caractères (alphabet sans ambiguïté), plus le nom du
  PC sur le tailnet.
- **Droits par appareil** : *voir*, *agir* (actions des modules, et démarrer Eyrie fermée : seul un appareil qui peut
  agir en reçoit le secret, et le veilleur refuse les autres), *terminaux* (saisie et création) ; tous accordés par
  défaut, réglables appareil par appareil ; révocation immédiate.
- **Clé du PC** : `%LOCALAPPDATA%\EyrieSuite\Eyrie\mobile\cle`, chiffrée par DPAPI ; appareils dans
  `mobile\appareils.json` (clés publiques seulement). Un Eyrie sur un autre dossier (essais) garde tout dans le sien.
- **Clé du téléphone** : stockage privé de l'application (inaccessible aux autres applications).

## 5. Protocole

Trames : longueur (u16, gros-boutiste) + message Noise ; un message applicatif de plus de 60 000 octets est découpé.
Contenu : MessagePack (`rmp-serde`, champs nommés : un champ ajouté plus tard est ignoré par une version plus
ancienne). Version de protocole `1` échangée au `Bonjour` ; écart : « mettez à jour l'application ».

- Téléphone → PC : `Bonjour`, `Appairer`, `Suivre { ecran, terminal }`, `Action { req, module, chemin, action,
  saisie }`, `Terminal { id, saisie }`, `Commande { module, id }`, `NotificationsLues`, `Ping`.
- PC → téléphone : `Accueil` (nom du PC, modules), `Appaire`, `Modules` (cartes : nom, couleur, logo une fois,
  pastille, ligne d'état), `Ensemble` (ce qui attend), `Notifications`, `Ecran { module, chemin, ecran }`,
  `TerminalVue` (différences), `Resultat { req, ok, message }`, `Bulle`, `Pong`.

**Écran générique** (`suite_mobile::ui::Screen`) : titre, sous-titre, actions de barre, blocs (`Section`, `Row`,
`Note`, `Meter`, `Text`, `Empty`, `Buttons`, `Field`). Une `Row` ouvre un sous-écran (`chemin`) ou porte des actions
(appui long). Une action peut demander confirmation ou une saisie. Le téléphone dessine tous les modules avec les
mêmes composants : cohérence, et un module ajouté au PC arrive au téléphone sans mise à jour de l'application.

**Terminal** : le PC envoie une fenêtre (écran + jusqu'à 500 lignes d'historique) en différences : décalage (lignes
sorties par le haut) + lignes changées, couleurs symboliques (par défaut, 256 couleurs, RVB) que le téléphone
résout dans son thème. Jusqu'à 30 images/s tant qu'un téléphone le suit. Saisie : texte, touches nommées (Entrée,
Échap, Tab, flèches, Ctrl+C…), collage ; encodées par Fawkes lui-même (`replay_keyboard`), donc justes en mode
win32-input de ConPTY. Une session Claude qui attend montre ses choix (« 1. Yes ») en boutons. Un collage sur
plusieurs lignes, ou qui cache des caractères de contrôle (Échap…, qu'un shell lirait comme des touches, Entrée
comprise), est demandé d'abord ; le PC retire ces caractères d'un collage et d'un texte tapé (sauf Alt+touche).
Quand le programme demande un mot de passe, la saisie se masque : ni affichée, ni apprise par le clavier du
téléphone (« Saisie masquée » la bascule à la main).

## 6. Ce que chaque module offre au téléphone

| Module | Écrans | Actions | Limites |
|---|---|---|---|
| Fawkes | groupes et terminaux (état, Claude qui attend), terminal en direct | saisie, touches, réponses rapides à Claude, nouveau terminal (dossiers récents, shell ou Claude), redémarrer, veille/réveil, fermer, Remote Control, montrer sur le PC | — |
| Patronus | tâches, boîte de réception, détail d'une exécution (journal) | lancer (avec ou sans consigne), arrêter, tout arrêter, pause, marquer lu, archiver, appliquer le diff, supprimer le worktree (ces deux-là par les boutons de Patronus : `apply_diff`, `drop_worktree`, suivis par `run_job`) | — |
| Owlery | boîte de réception en direct, par sorte ; détail | marquer lue ou non lue, traitée, reporter à demain, ignorer le dépôt, corriger avec Claude (puis Fawkes), ouvrir sur le téléphone ou le PC, actualiser | — |
| Menagerie | services gérés (état, journal), ports ouverts, Docker | démarrer, arrêter, relancer un service géré ; arrêter un processus (`kill_program`) ; conteneurs (démarrer, arrêter, relancer par `docker_action`, suivis par `docker_job_of` ; journaux) | — |
| DragonTree | dépôts ; dans un dépôt : modifications et diff en couleurs, historique et commits (fichiers, diff), branches locales et distantes, remises, étiquettes, worktrees, opération en cours ; ce que fait DragonTree (progression) et ce qu'il a donné, d'où que vienne l'opération | fetch, pull (fusion ou rebase), push (ou forcé avec bail), publier ; indexer, désindexer, annuler (un fichier ou tout), valider, corriger le dernier commit ; conflits : garder une version, marquer résolu ; basculer, nouvelle branche, renommer, supprimer, suivre une distante, fusionner, rebaser ; cherry-pick, revert, reset soft/mixed/hard, sortir un commit ; mettre de côté, appliquer, retirer, jeter ; étiquette (créer, pousser, supprimer) ; supprimer et élaguer les worktrees ; continuer, passer, abandonner ; afficher dans DragonTree | tout par DragonTree (`queue_operation`) : le dépôt regardé est ouvert dans un onglet de fond, l'opération passe par sa file, avec le compte associé au dépôt, sa progression, ses notifications et son bandeau de conflits sur le PC. Un conflit se règle fichier par fichier en gardant une version, ou sur le PC (éditeur de conflits de DragonTree) |
| Grimoire | profils, limites, serveurs MCP | ouvrir sur le PC | — |
| Gringotts | limites en cours, coût du jour, conversations | relire | — |
| Carnet | notes du dépôt courant et des autres | ajouter une ligne | — |
| tous | vue d'ensemble (ce qui attend mène à son écran), notifications, commandes de la palette (« sur le PC ») | | |

Les modules voisins ont reçu, avec l'accord du 27/10, de petites API publiques pour cela (branche `suite/mobile` de
Fawkes, Owlery, Menagerie, Patronus et DragonTree) : `close_terminal`, `remote_control` ; `state`, `mark`, `mute`,
`undo`, `work_with_claude`, `knows_local_dir` ; `managed`, `start_service`, `stop_service`, `restart_service`,
`docker_action`, `docker_job_of`, `kill_program` ; `apply_diff`, `drop_worktree`, `run_job` ; `repo_tab`,
`open_repo_tab`, `show_repo`, `refresh_repo`, `queue_operation` (`RemoteOp`), `cancel_operation`,
`RepoTab::outcome`.

## 7. L'application

- **Premier lancement** : connexion du téléphone à Tailscale (« Se connecter à Tailscale », une fois, dans le
  navigateur), puis « Scanner le QR code » (caméra) ou « Saisir le code » ; enfin le code à six chiffres à comparer
  avec celui du PC, qui confirme.
- **Navigation** : barre du bas (Ensemble, les trois premiers modules du PC, Plus), pastilles ; *Plus* ouvre la liste
  de tous les modules, les notifications, la palette (recherche), les réglages.
- **Terminal** : plein écran, police à chasse fixe, zoom (A− / A+), défilement de l'historique, barre de touches
  (Échap, Tab, Ctrl, flèches, Entrée), champ de saisie au-dessus du clavier, réponses rapides quand Claude attend.
- **Hors ligne** : bandeau « Reconnexion… », dernières données gardées, reconnexion automatique (délai croissant
  jusqu'à 30 s, immédiate au retour au premier plan et quand le téléphone se reconnecte à Tailscale). Une coupure
  sans fermeture (changement de réseau, PC en veille) se voit au plus tard après 45 s sans rien du PC, qui répond
  `Pong` à chaque `Ping` (toutes les 15 s).
- **Rattrapage** (« ce n'est pas grave tant qu'on remet à jour les infos avec tout ce qui s'est passé sur le PC entre
  temps », 28/10) : à chaque connexion, le PC renvoie tout son état. Ça couvre les modules et leurs pastilles, ce qui
  attend, les 60 dernières notifications (lues ou non), les commandes, l'écran ouvert et le terminal suivi (500 lignes
  d'historique). Le téléphone garde, pour chaque PC, jusqu'où il a vu les notifications, même après un arrêt de
  l'application par Android. Celles arrivées entre-temps et non lues sur le PC sont signalées d'une bulle : la
  notification elle-même, ou leur nombre et la plus récente.
- **Notifications** : une bulle quand une notification arrive du PC ; sur Android, en arrière-plan, une
  notification du système pour ce qui se met à attendre (session Claude…) et pour les alertes, tant que
  l'application tourne (pas de service au premier plan : Android peut couper le réseau au bout d'un moment).
  Téléphone verrouillé, elle ne dit que le module et ce qui attend (« Quelque chose vous attend sur le PC. ») ; le
  détail (titre du terminal, message) est dans l'application.
- **Thème** : celui du système (clair ou sombre), couleurs de la suite.
- **Réglages** : PC appairés (changer, oublier), nom de l'appareil, Tailscale (état, connexion, déconnexion), taille
  du terminal, à propos.

## 8. Plus tard

- Chiffrer l'état du nœud Tailscale du téléphone par le Keystore d'Android, comme celui du PC par DPAPI.
- ~~Service au premier plan (rester connecté en arrière-plan)~~ : pas nécessaire selon l'utilisateur (28/10), tant
  que le téléphone rattrape tout à la reconnexion (§7).
- iOS.
- Les actions marquées « changement du module nécessaire » ci-dessus, module par module et avec accord.

## 9. État au 27/10/2026 (soir)

Fait, sur la branche `mobile` (rien de poussé) :

| Pièce | Vérifié par |
|---|---|
| Protocole `suite-mobile` | 18 essais : ticket et code, preuves, code de vérification, canal Noise (trame altérée, requête HTTP refusées), messages, différences de terminal, réponses rapides |
| Pont dans Eyrie | 13 essais : appairage confirmé ou refusé, code faux ou expiré, cinq essais puis ticket brûlé, appareil inconnu, autre compte Tailscale, jeton du compagnon faux, clé du PC du QR, session calme au-delà de 10 s ; bout en bout avec un module factice (écran, action proposée ou non, terminal et saisie, journal, bulle) ; interrupteur |
| Écrans des modules | essais par module (Fawkes 6, Patronus 7, Owlery 4, Menagerie 5, DragonTree 10, Grimoire 3, Gringotts 27, Carnet 4) |
| Application | 6 essais (tampon de terminal, couleurs, lignes, magasin) ; captures de toutes les scènes en démonstration |
| Compagnon `eyrie-tailnet.exe` | `go test` : consigne de relais (boucle locale seulement), en-tête et relais dans les deux sens, relais coupé |
| Ensemble | essai réel sur le poste : Eyrie (dossier d'essai, `EYRIE_MOBILE_LOCAL`) et l'application de bureau, appairage par le lien, clic « Appairer » par l'automatisation d'interface de Windows, vraies données (Fawkes et un vrai PowerShell en direct, Gringotts, Owlery), journal |

Reste :
- **L'APK se construit** (27/10 au soir) : SDK Android dans `%LOCALAPPDATA%\Android` (JDK 17, licences
  acceptées par l'utilisateur, `platform-tools`, `platforms;android-36`, `build-tools;36.1.0`,
  `ndk;28.2.13676358`), puis `tools\android.ps1 -Release` → `target\android\eyrie-release.apk` (8 Mo), signé
  par la clé de débogage d'Android (`%USERPROFILE%\.android\debug.keystore`, à garder : une version signée
  d'une autre clé ne s'installe pas par-dessus). `-Install` l'installe par le débogage USB. Le code Android
  (`android.rs` : caméra du NDK, permission, liens, vibration) compile, mais n'a pas encore été essayé sur un
  téléphone.
- **Essai par Tailscale** : `eyrie.exe` (release) et `tools\compagnon.ps1 -Release`, activer Réglages › Tailscale
  puis Téléphones ; sur le téléphone, connecter Eyrie à Tailscale (même compte), puis scanner le QR code. Le nœud
  intégré (28/10) compile et se lie, mais n'a pas encore tourné sur un téléphone.
- Les branches `suite/mobile` de Fawkes, Owlery, Menagerie, Patronus et DragonTree (API pour le téléphone) sont à
  fusionner avec celle d'Eyrie : Eyrie les compile par chemin, il lui faut donc ces branches extraites.
- Plus tard : §8.
