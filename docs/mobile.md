# Eyrie sur le téléphone

Demande du 27/10/2026 : « l'application mobile pour Eyrie, en Rust, très performante, une UX irréprochable ; un
maximum des fonctionnalités du PC sans en être une copie (un module à la fois) ; toutes les commandes du téléphone
retranscrites sur le PC en temps réel (si Tailscale est connecté) ; appairage des appareils par QR code ». Questions
sautées à la demande de l'utilisateur : les choix ci-dessous sont les miens, marqués **(choix)** quand ils engagent.

Demande du 28/10 : une passe d'ergonomie (bandeaux pliants), DragonTree approfondi, les dossiers du PC vus du
téléphone, démarrer Eyrie fermée depuis le téléphone. Conception et choix :
`docs/superpowers/specs/2026-10-28-mobile-ergonomie-design.md`.

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
  des onglets d'écran, des sous-écrans empilés (retour par le geste ou le bouton du système).
- **Sécurité d'abord** (prérequis de l'audit du 27/10) : voir §4.

## 2. Ce qui est construit

| Pièce | Où | Rôle |
|---|---|---|
| `suite-mobile` | `crates/suite-mobile` | Protocole partagé PC ↔ téléphone : messages, écrans génériques, terminal, appairage, canal chiffré Noise, défi du veilleur (`veille.rs`), dossiers protégés (`paths.rs`). Ni egui ni Slint. |
| Pont mobile | `crates/eyrie/src/mobile/` | Dans Eyrie : écoute, appareils, appairage, journal, interrupteur, synchro, aiguillage des actions vers les modules, dossiers du PC pour le sélecteur du téléphone (`browse.rs`). |
| Veilleur, côté Eyrie | `crates/eyrie/src/veille.rs`, `tailnet.rs` | La case « Démarrer Eyrie depuis le téléphone », la valeur Run, le rattachement au veilleur, le secret de démarrage de chaque téléphone (§3). |
| Interface mobile des modules | `Module::mobile_*` (`suite-sdk`) et chaque adaptateur `crates/modules/*` | Chaque module décrit ses écrans et ses actions pour le téléphone. |
| Compagnon Tailscale d'Eyrie | `compagnon/` (Go, tsnet) → `eyrie-tailnet.exe` | Le nœud de la suite sur le tailnet, et le relais des connexions du téléphone vers Eyrie ; lancé avec `--veille`, le veilleur, qui garde le nœud quand Eyrie est fermée et la démarre à la demande d'un téléphone (§3). |
| Nœud Tailscale du téléphone | `compagnon/telephone` (Go, tsnet) → `libeyrie_tailnet.so` dans l'APK | Le téléphone sur le tailnet par l'application elle-même ; ses connexions vers le PC passent par lui (§3). |
| Application mobile | `crates/eyrie-mobile` | Slint, Rust. Tourne sur Android et, pour le développement, sur le bureau dans une fenêtre de téléphone (démonstration sans PC : `EYRIE_MOBILE_DEMO=<scène>`, scènes en tête de `demo.rs`). |

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
- **Un seul compagnon par nœud** : il tient le verrou `Local\EyrieTailnet:<dossier du nœud>` toute sa vie ; un second
  s'arrête aussitôt. Eyrie le teste avant d'en lancer un : tenu, elle se rattache au veilleur (ci-dessous), et ne
  lance jamais celui de Fawkes à côté.
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
    dedans, inchangé. Après une connexion ratée, le nœud dit si le PC est en ligne sur le tailnet (`EyrieTailnetPeer`,
    §7).
  - **Android** : la liste des interfaces réseau est refusée aux applications. Comme la fonctionnalité « androidbin »
    de Tailscale, le nœud déclare une interface unique avec les adresses de sortie choisies par le système (connexion
    UDP sans envoi). Le DNS passe par le résolveur du système (cgo), les changements de réseau par le sondage de
    Tailscale.
  - **Journaux** : dans le dossier du nœud (`journal.txt`, 4 Mo au plus), rien n'est envoyé à Tailscale
    (`logtail.Disable`).
  - **État du nœud** : sa clé reste dans le stockage privé de l'application, ni sauvegardée ni transférée (§4). Elle
    n'est pas chiffrée en plus par le Keystore d'Android, contrairement à celle du PC chiffrée par DPAPI : piste notée
    au §8.
  - **Bureau** (développement) : pas de nœud, connexion directe par le réseau du poste.
- Le compagnon de Fawkes reste utilisable en secours pour le nœud seul (sans relais : le téléphone ne peut pas se
  connecter, la page Téléphones le dit).
- **Le veilleur : démarrer Eyrie fermée depuis le téléphone.** Case « Démarrer Eyrie depuis le téléphone » (Réglages ›
  Téléphones), **coupée par défaut (choix)** ; une ligne dessous dit que seuls les téléphones qui ont *agir* le peuvent.
  - Cochée, Eyrie inscrit `eyrie-tailnet.exe --veille` à l'ouverture de session Windows (valeur `EyrieVeille` de
    `HKCU\…\Run`) et le lance détaché. Le veilleur tient alors le nœud d'Eyrie, ouverte ou fermée. Ouverte, elle s'y
    rattache par un tube nommé (nom tiré au hasard à chaque démarrage du veilleur et écrit dans `tube.txt`, dans le
    dossier du nœud ; réservé à l'utilisateur ; client et serveur vérifiés : même compte, exécutable attendu) et lui
    confie le relais du port 4747. Fermée, il garde le nœud sur le tailnet et répond au **port 4748**. Décochée : valeur
    Run retirée, veilleur arrêté, et le compagnon d'Eyrie reprend le nœud.
  - Il y faut l'Eyrie de tous les jours (une Eyrie d'essai ne touche pas à la valeur Run) et `eyrie-tailnet.exe` à
    côté d'`eyrie.exe`, à l'abri des autres comptes du poste : propriétaire l'utilisateur, SYSTEM, les administrateurs
    ou TrustedInstaller ; ni l'exécutable ni son dossier modifiables par un autre compte, ni les dossiers au-dessus
    renommables. Eyrie le contrôle avec les règles du veilleur (`veille/exeplace.rs`, repris d'`exeplace.go`) avant de
    l'inscrire ou de garder la commande déjà inscrite ; le veilleur le refait avant chaque lancement.
  - **Jamais élevé** : une Eyrie lancée en administrateur ne lance pas le veilleur (elle prend son compagnon, la page
    dit pourquoi ; la valeur Run reste, il démarrera sans élévation à la session suivante). Un veilleur élevé refuse de
    tourner, quel que soit qui l'a lancé : il démarrerait Eyrie en administrateur sans invite.
  - **Port 4748** : défi-réponse (`suite_mobile::veille`, vecteurs partagés avec le Go) avec le secret de l'appareil,
    que le PC ne donne dans l'accueil qu'à un appareil qui a *agir*. À chaque demande, le veilleur relit
    `mobile\veille.dpapi` (DPAPI : actif ou non, sa clé, le verrou d'Eyrie) et `mobile\appareils.json` : le compte
    Tailscale doit être celui d'un appareil appairé, et cet appareil avoir *agir*. Limites : cinq preuves fausses,
    15 min de blocage ; un démarrage par 30 s, six par heure ; une connexion par adresse du tailnet, huit adresses à la
    fois. « Accès depuis le téléphone » coupé : il répond « accès coupé ».
  - Il lance `eyrie.exe --reduite`, rangé à côté de lui, avec l'environnement à jour de l'utilisateur : Eyrie s'ouvre
    réduite, et le dit au journal du pont et en bulle (« Eyrie démarrée depuis <appareil> »). Fermée normalement,
    Eyrie le dit aux téléphones connectés, qui affichent « Eyrie a été fermée » plutôt que « hors ligne ».
  - **Page Téléphones** : ce que fait le veilleur (il tient le nœud ; ou il ne tourne pas, pourquoi, avec
    « Réessayer » et son dernier échec), puis « Lancé à chaque ouverture de session Windows. », ou qu'il est désactivé
    dans le Gestionnaire des tâches (onglet Démarrage) : Eyrie le dit et ne passe pas outre.
  - **Préalables** : PC allumé et éveillé, session Windows ouverte (verrouillée, ça va). PC éteint, en veille ou en
    veille prolongée, ou session fermée : impossible (le Wake-on-LAN de Tailscale demande un autre appareil éveillé
    sur le même réseau local).

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
6. **Journal persistant** : `mobile\journal.jsonl` (connexions, refus, appairages, actions, notifications du PC
   marquées lues depuis le téléphone, ouverture du sélecteur de dossiers et dossier choisi, en clair ; démarrages
   d'Eyrie par le veilleur et leurs refus ; jamais le texte saisi dans un terminal, seulement sa longueur), lisible
   dans la page Téléphones. Les refus (action non proposée, droit retiré, commande inconnue, dossier protégé) y sont au
   plus dix par minute et par téléphone : un téléphone qui sonde laisse une trace sans remplir le journal.
7. **Interrupteur** : « Accès depuis le téléphone » (page Téléphones, barre d'état, palette) coupe l'écoute, ferme
   toutes les connexions et retire le relais au compagnon ; le veilleur refuse alors de démarrer Eyrie. « Couper tout
   accès à distance » arrête aussi le nœud et le veilleur, et décoche « Démarrer Eyrie depuis le téléphone ».

Autres règles :
- **Ticket d'appairage** : secret de 32 octets, valable 5 minutes, à usage unique, 5 essais. QR :
  `eyrie://appairer?v=1&n=<nom du PC>&h=<nom MagicDNS>&a=<IP tailnet>&p=4747&k=<clé du PC>&s=<secret>`. Le téléphone et
  le PC se prouvent mutuellement le secret (MAC BLAKE2s sur le hachage de la poignée de main), et le téléphone vérifie
  la clé du PC lue dans le QR. **Code à saisir** en secours : dix caractères (alphabet sans ambiguïté), plus le nom du
  PC sur le tailnet. Un appairage abandonné (retour, autre QR) ferme sa connexion et ne parle plus à la place du
  suivant. Le PC regarde cette connexion pendant qu'il attend le clic : fermée, la demande est close, sa confirmation
  disparaît (« abandonné par le téléphone » au journal, et une bulle), et un « Appairer » cliqué trop tard n'appaire
  rien. Le ticket qu'il avait consommé, s'il est encore montré, est remplacé par un nouveau valable jusqu'à la même
  heure (la fenêtre ne s'allonge jamais) : la page dit « Nouveau code : le précédent a servi à un appairage
  abandonné. », et scanner de nouveau marche. À moins de 20 s de sa fin, il est retiré : « Code expiré : demandez-en
  un nouveau. ». Un appairage refusé sur le PC, ou resté sans réponse, ne donne pas de nouveau code.
- **Droits par appareil** : *voir* ; *agir* (actions des modules et commandes, et démarrer Eyrie fermée : seul un
  appareil qui peut agir en reçoit le secret, et le veilleur refuse les autres) ; *terminaux* (saisir dans un terminal,
  en ouvrir un : shell, session Claude, « Nouveau terminal » ; Remote Control) ; *dossiers du PC* (parcourir les
  dossiers pour une action qui en demande un ; avec *agir* : sa case est grisée sans lui). Tous accordés par défaut,
  *dossiers* compris pour un appareil appairé avant ce droit ; réglables appareil par appareil ; révocation immédiate.
  Un changement de droits renvoie aussitôt l'accueil aux connexions de l'appareil, avec ou sans le secret de
  démarrage.
- **Clé du PC** : `%LOCALAPPDATA%\EyrieSuite\Eyrie\mobile\cle`, chiffrée par DPAPI ; appareils dans
  `mobile\appareils.json` (clés publiques seulement). Un Eyrie sur un autre dossier (essais) garde tout dans le sien.
- **Clé du téléphone** : stockage privé de l'application (inaccessible aux autres applications), avec les PC appairés
  et leurs secrets de démarrage (`pcs.json`). Ni sauvegarde ni transfert vers un autre téléphone : `allowBackup` et
  `fullBackupContent` à `false`, et, à partir d'Android 12, des règles d'extraction (`dataExtractionRules`) qui
  excluent tout, du nuage comme du transfert d'appareil à appareil (certains fabricants passent outre `allowBackup`).
- **Dossiers du PC** : §5.

**Revue de sécurité du 29/10**, par axe (pont, veilleur, téléphone, DragonTree) : chaque droit sur tous ses chemins,
ce que le veilleur accepte et lance, le contrôle des dossiers jusqu'à la destination d'un clone, les saisies passées
à git, ce que le téléphone envoie ou laisse voir, le journal. Corrigé : le secret de démarrage n'est donné qu'aux
téléphones qui ont *agir*, et le veilleur le revérifie ; sans *terminaux*, ni shell, ni session Claude, ni « Nouveau
terminal » de la palette, ni Remote Control ; le veilleur refuse de tourner élevé et de lancer Eyrie d'un dossier
qu'un autre compte possède ou peut renommer, tire le nom de son tube au hasard (un autre compte ne le prend plus avant
lui), garde une connexion par adresse et n'écrit plus au journal un identifiant d'appareil mal formé ; une liste de
dossiers en attente est refusée après le retrait du droit, un dossier protégé n'est plus décrit, un clone ne va plus
dans un dossier de secrets ou de configuration, un nom de branche ou d'étiquette ne passe plus pour une option de
git, les commits lus pour le téléphone sont gardés 64 au plus par dépôt ; un collage qui cache Échap est demandé, un
mot de passe se tape masqué, l'écran verrouillé ne montre plus le détail d'une notification, une confirmation ne vaut
plus pour un autre écran ; redémarrer un terminal, arrêter un conteneur ou relancer un service se confirme ; les
refus et « Tout marquer lu » sont notés au journal.

## 5. Protocole

Trames : longueur (u16, gros-boutiste) + message Noise ; un message applicatif de plus de 60 000 octets est découpé.
Contenu : MessagePack (`rmp-serde`, champs nommés : un champ ajouté plus tard est ignoré par une version plus
ancienne). Version de protocole `1` échangée au `Bonjour` ; écart : « mettez à jour l'application ».

- Téléphone → PC : `Bonjour` (avec ce que l'application sait faire), `Appairer`, `Suivre { ecran, terminal }`,
  `Action { req, module, chemin, action, saisie }`, `Terminal { id, saisie }`, `Commande { module, id }`,
  `NotificationsLues`, `Ping`, `Parcourir { req, module, chemin, action, dossier, filtre, debut, caches }`.
- PC → téléphone : `Accueil` (nom du PC, droits, secret de démarrage pour un appareil qui a *agir*), `Appaire`,
  `Modules` (cartes : nom, couleur, logo une fois, pastille, ligne d'état), `Ensemble` (ce qui attend),
  `Notifications` (avec l'écran où chacune mène), `Commandes` (celles qui ouvrent un terminal sont marquées),
  `Ecran { module, chemin, ecran }`, `TerminalVue` (différences), `Resultat { req, ok, message }`, `Bulle`, `Pong`,
  `Dossier { req, liste }`, `AuRevoir { raison, fermeture }` (accès coupé, appareil retiré, ou Eyrie qui se ferme).

**Écran générique** (`suite_mobile::ui::Screen`) : titre, sous-titre, actions de barre, onglets, champ de recherche,
blocs (`Section`, `Row`, `Note`, `Meter`, `Text`, `Empty`, `Buttons`, `Diff`). Les chemins d'écran sont absolus dans
le module (`depot/<clé>/historique`, jamais `historique`). Ce qui s'est ajouté depuis la première version est un champ
`#[serde(default)]`, jamais une variante de `Block` (une application plus ancienne perdrait la connexion). Le
téléphone dessine tous les modules avec les mêmes composants : cohérence, et un module ajouté au PC arrive au
téléphone sans mise à jour de l'application.
- **Bandeau** : une `Section` avec une clé et un pliage ; couleur, glyphe, pastille visible même plié, ton, menu ⋯.
  Plié, il cache les blocs qui le suivent jusqu'à la section suivante. Le PC donne l'état de départ ; le choix de
  l'utilisateur est gardé sur le téléphone, par PC, module, écran et clé (rien n'est envoyé au PC). « L'alerte
  gagne » : un bandeau plié qui passe à Attention ou Danger se rouvre, une fois par alerte. Après une action de son
  menu, il garde son pliage six secondes, prolongées à la réponse. « Tout replier » et « Tout déplier » au menu de
  l'écran.
- **Onglets** : des écrans voisins du module (État, Historique, Branches, Plus d'un dépôt) entre lesquels on passe sans
  empiler ; chacun garde sa position dans la liste.
- **Recherche** : filtre fait sur le téléphone, sans casse ni accents, sur le titre, le sous-titre, le texte à droite,
  la pastille et les étiquettes des lignes ; les sections qui ont des résultats restent dépliées le temps de la
  recherche.
- **Ligne** (`Row`) : une clé stable, qui la met à jour sur place (un toucher ou un appui long en cours ne se perd
  pas) ; un sous-écran ouvert au toucher (chevron), ou une action au toucher (son libellé en pastille) ; un menu
  (appui long ou ⋯) ; un bouton visible à droite ; des étiquettes (trois au plus, deux sur une ligne de commit, les
  autres comptées « +2 ») ; une barre de progression (en pour cent, ou indéterminée) ; une cellule de graphe, qui en
  fait une ligne de commit dense : traits raccordés d'une ligne à l'autre comme dans DragonTree, disque, anneau pour
  HEAD, carré pour une remise, huit voies au plus, palette de DragonTree.
- **Note** avec une progression et des actions : une opération en cours, sa barre et son « Annuler ». **Diff** : chasse
  fixe, numéros de ligne en gouttière, lignes coupées ou retour à la ligne au choix.
- **Action** (`Act`) : confirmation (le bouton porte le libellé de l'action), saisie (un texte, ou un dossier du PC),
  lien (`url`), écran local (`open` : rien n'est envoyé au PC), `busy` (le module la fait déjà : grisée, avec un
  indicateur), `terminal` (elle ouvre un terminal : droit *terminaux*).

**Liens entre modules** : « @module <id> [<chemin>] » dans `Act.open`, `Row.open` ou le chemin d'un onglet ouvre
l'écran d'un autre module (son accueil sans chemin), sur le téléphone seulement : ni journal, ni bulle, ni droit *agir*
(« Voir dans Gringotts » de Grimoire). Une réponse d'action peut faire de même (`@module fawkes`, ou `@ouvrir <chemin>`
pour un écran de son module), et une notification (`Note.path`).

**Dossiers du PC** (`crates/eyrie/src/mobile/browse.rs`) : une action dont la saisie est un dossier (`Input.folder` :
n'importe lequel, un dépôt git, ou le parent d'un clone) se remplit dans le sélecteur du téléphone (§7), que le PC
alimente (`Parcourir` → `Dossier`).
- Le pont garde la saisie avec l'action (liste blanche). Il ne liste que pour une action de l'écran suivi qui demande
  un dossier, et pour un appareil qui a *agir* et *dossiers du PC*, droits revérifiés au moment de lire (une demande qui
  attendait son tour est refusée après leur retrait). Le chemin choisi est revérifié avant d'appeler le module, qui
  reçoit le chemin résolu ; un refus n'atteint jamais le module.
- Un seul contrôle, pour lister comme pour agir. Refusés : les formes qui contournent la lecture d'un chemin (UNC,
  `\\?\`, `\\.\`, flux `:`, `..`, noms réservés) ; tout ce qui n'est pas un disque fixe du PC (un partage ou un
  lecteur réseau absent figerait Eyrie) ; un lien ou une jonction qui y mène, lu sans être suivi, et les boucles de
  liens. Le chemin est ensuite résolu (`canonicalize` : noms courts 8.3, jonctions), pour qu'un nom court ne contourne
  pas les règles, puis revérifié hors des zones interdites : Windows, les programmes, ProgramData, AppData, les données
  d'Eyrie ; les profils des autres comptes, même écrits sous leur nom court, sans rien en dire, pas même s'ils
  existent (celui de l'utilisateur est admis sous ses deux noms) ; les dossiers de secrets où qu'ils soient (`.ssh`,
  `.gnupg`, `.aws`, `.azure`, `.kube`, `.docker`, `.password-store`, corbeille, `System Volume Information`) ; les
  dossiers à point du profil (`.config`, `.claude`…) et les profils de PowerShell des Documents (même absents, aussi
  là où mènent des Documents déplacés par une jonction), que git, Claude Code ou PowerShell lisent d'eux-mêmes,
  parfois pour lancer des commandes. Un dossier protégé se montre « protégé », sans que le PC regarde dedans. Vient
  enfin la règle de la saisie (un `.git` ; `parent\nom` absent ou vide).
- Des dossiers seulement, jamais de fichier, ni de création. Une lecture à la fois par téléphone, dans un fil (jamais
  celui de l'interface) : pages de 200, 20 000 entrées ou 3 s au plus (au-delà, filtrer), huit lectures par 2 s. La
  racine : « Récents » (les huit derniers dossiers choisis pour ce module, `mobile\dossiers.json`), les raccourcis du
  module, « Accès rapide » (dossier personnel, Bureau, Documents, Téléchargements, `source\repos`, `DEV`, les dossiers
  parents des dépôts connus), « Lecteurs » (disques fixes).
- Les noms protégés sont une seule liste (`suite_mobile::paths`), pour le pont et pour DragonTree, qui revérifie la
  destination d'un clone et refuse un nom de dossier qui commence par un point.

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
| Fawkes | un bandeau par groupe, dans l'ordre du PC (couleur, icône, « ✳️2 · 5 » pour le « ✻2 · 5 » du PC, pastille « 2 attendent », « 1 en échec » ou 🔔 ; ouvert s'il attend, échoue ou sonne, plié s'il est vide ou tout en veille) ; « Claude attend » en tête, qui mène à chaque session ; états (Claude travaille, prêt, en veille, en échec) ; terminal en direct | saisie, touches, réponses rapides à Claude ; « Nouveau terminal » en barre du haut (dossiers récents, shell ou Claude, « Autre dossier… » par le sélecteur) ; d'un terminal : redémarrer (confirmé s'il tourne ou dort), veille, réveil, fermer, Remote Control (aux seules sessions qui peuvent le prendre), montrer sur le PC ; du menu d'un groupe : shell ou Claude dans son dépôt, nouveau terminal dans ce groupe, Remote Control, tout réveiller, tout mettre en veille, **Tout redémarrer** (`restart_group`, confirmé) | ouvrir un terminal ou Remote Control demande *terminaux*. « Tout redémarrer » relance un terminal en veille sans son ancien écran : la question le dit et renvoie à « Tout réveiller », qui le garde. Modifier ou supprimer un groupe : sur le PC |
| Patronus | « En cours » (depuis quand, barre), « Boîte de réception » (non lus), « Tâches » (ouvertes si l'une ne peut pas partir), détail d'une exécution (journal) | lancer (avec ou sans consigne), arrêter (confirmé), tout arrêter, pause, tout marquer lu ; d'un résultat : lu, non lu, archiver ; appliquer le diff, supprimer le worktree (ces deux-là par les boutons de Patronus : `apply_diff`, `drop_worktree`, suivis par `run_job`) | — |
| Owlery | boîte de réception en direct, groupée comme sur le PC (par dépôt par défaut) : un bandeau par groupe (ouvert s'il a des non lues, en alerte si une CI y échoue), « N autres… » vers le groupe entier, avec recherche ; détail | marquer lue ou non lue, traitée, reporter (3 h, demain matin, lundi matin), ignorer le dépôt, corriger avec Claude (puis Fawkes), ouvrir sur le téléphone ou le PC, actualiser ; **Tout marquer lu** d'un groupe (`mark_read_many` : un seul geste) ; « Annuler le dernier geste », montré seulement s'il y en a un (`can_undo`), qui défait tout un « Tout marquer lu » | « Corriger avec Claude » demande *terminaux* |
| Menagerie | services gérés, ports ouverts (pliés au-delà de six ; ouverts et en alerte si un port est visible du réseau), conteneurs : un bandeau par projet compose (« 2 / 3 »), puis les autres ; recherche ; une action Docker en cours en barre de progression | démarrer, arrêter et relancer (confirmés) un service géré ; arrêter un processus (`kill_program`) ; conteneurs : démarrer, arrêter et redémarrer (confirmés) par `docker_action`, suivis par `docker_job_of`, journaux ; d'un projet : **Démarrer tout**, **Arrêter tout** (confirmé) par `docker_project_action` | « Démarrer tout » et « Arrêter tout » sont grisés tant qu'une action tourne sur un conteneur du projet ; sur le PC, ses boutons restent actifs |
| DragonTree | tous ses dépôts, lus dans DragonTree en mémoire et rangés comme sa barre latérale (ouverts sur le PC, épinglés, un bandeau par dossier racine analysé, récents), homonymes distingués par leur dossier parent, état rapide, recherche ; un dépôt en quatre onglets : État (branche, suivi, modifications et diff), Historique (la branche courante avec son graphe et ses étiquettes, ou toutes les branches ; 100 commits par page, 2 000 au plus), Branches (la courante, les locales par date et par dossier, les distantes par remote, pliées ; « N autres… »), Plus (remises, étiquettes, worktrees, remotes, pull requests du dépôt) ; détail d'une branche (suivi, à pousser et à tirer, historique) ; commits (fichiers, diff) ; pull requests de tous les comptes (« Créées par moi », « À relire ») ; en tête de chaque écran, l'opération de DragonTree (progression, « Annuler ») ou sa lecture ratée | fetch, pull (fusion ou rebase), push (forcé avec bail : au menu, avec ce qui sera perdu), publier ; indexer, désindexer, annuler (un fichier ou tout), valider, corriger le dernier commit ; conflits : garder une version, marquer résolu ; depuis le détail d'une branche (un toucher n'en bascule plus) : basculer, fusionner, rebaser, pousser, renommer, supprimer, suivre une distante, **supprimer une distante sur le serveur** (confirmé) ; cherry-pick, revert, reset soft/mixed/hard, sortir un commit ; remises, étiquettes, worktrees ; continuer, passer, abandonner ; **ajouter un dépôt existant** (sélecteur : un dépôt) ; **cloner** (brouillon rempli pas à pas : URL, dossier parent par le sélecteur, nom, branche, compte, sous-modules ; progression et « Annuler ») ; ouvrir une pull request dans le navigateur ; afficher dans DragonTree | tout par DragonTree (`queue_operation`, `add_repo`, `clone_remote`) : le dépôt regardé est ouvert dans un onglet de fond, l'onglet du PC ne change pas ; l'opération passe par sa file, avec le compte du dépôt, sa progression, ses notifications et son bandeau de conflits sur le PC. Masqué dans Eyrie, ou Eyrie réduite, DragonTree vit (`background_tick`) : le dépôt regardé est relu dès qu'il change sur le disque. Les onglets que le téléphone a ouverts se referment après 15 min sans regard ni opération, même après un redémarrage ; jamais celui de l'utilisateur. Un conflit se règle fichier par fichier, ou sur le PC ; une branche dont la suivie porte un autre nom se pousse depuis le PC ; un clone qui se connecte encore après 15 s dit qu'une fenêtre attend peut-être sur le PC |
| Grimoire | profils ; limites : un lien « Voir dans Gringotts » quand il tourne, sinon un bandeau par compte (5 h, semaine) ; serveurs MCP (pliés) | « Voir dans Gringotts » (ouvert sur le téléphone : rien n'est envoyé au PC) ; « Ouvrir l'onglet MCP sur le PC » au menu du bandeau | « Rendre actif » un profil : §8 |
| Gringotts | un bandeau par compte (en alerte au-delà de 80 % sur 5 h ou la semaine, pourcentage visible plié), « Aujourd'hui », « Les plus coûteuses, 7 jours », conversations | relire | « Reprendre dans Fawkes » : §8 |
| Carnet | notes du dépôt courant ; les autres dépôts en bandeau pliable | « Ajouter une ligne » en barre du haut | — |
| tous | vue d'ensemble (ce qui attend mène à son écran), notifications (qui mènent à leur écran), commandes de la palette (« sur le PC ») ; écran d'un module arrêté ou indisponible | « Relancer le module » ou « Réessayer », par le chemin de l'hôte, Eyrie réduite comprise ; une commande lancée dit qu'elle est partie | une commande qui ouvre un terminal demande *terminaux* ; un module désactivé ne s'active pas d'ici |

Les modules voisins ont reçu, avec l'accord du 27/10, de petites fonctions Rust `pub` pour cela (branches
`suite/mobile` de Fawkes, Owlery, Menagerie, Patronus et DragonTree, fusionnées depuis dans leur `main`) :
`close_terminal`, `remote_control` ; `state`, `mark`, `mute`, `undo`, `work_with_claude`, `knows_local_dir` ;
`managed`, `start_service`, `stop_service`, `restart_service`, `docker_action`, `docker_job_of`, `kill_program` ;
`apply_diff`, `drop_worktree`, `run_job` ; `repo_tab`, `open_repo_tab`, `show_repo`, `refresh_repo`,
`queue_operation` (`RemoteOp`), `cancel_operation`, `RepoTab::outcome`.

La passe d'ergonomie, avec l'accord du 28/10, en ajoute d'autres, un commit par fonction accordée, sur la branche
`suite/mobile-ergonomie` de chaque dépôt (locale, rien de poussé) :
- DragonTree (8 commits) : `repo_directory` et `repo_quick_status` ; `set_remote_focus` et `background_tick` ;
  `RepoTab::status_error` et `refs_error` ; `add_repo` ; `close_repo_tab` ; `RemoteOp::DeleteRemoteBranch` ; le
  clonage sans fenêtre (`clone_remote`, `clone_defaults`, `account_for_url`, `host_jobs`, `cancel_host_job`) ; les
  pull requests lues en fond (`refresh_pull_requests`, `pull_requests`).
- Fawkes : `restart_group`. Menagerie : `docker_project_action`. Owlery : `mark_read_many`, et `can_undo`, que
  l'accord (« tout marquer lu annulable ») ne nommait pas, confirmée le 29/10.

## 7. L'application

- **Premier lancement** : connexion du téléphone à Tailscale (« Se connecter à Tailscale », une fois, dans le
  navigateur), puis « Scanner le QR code » (caméra) ou « Saisir le code » ; enfin le code à six chiffres à comparer
  avec celui du PC, qui confirme.
- **Navigation** : barre du bas (Ensemble, les trois premiers modules du PC, Plus), pastilles ; *Plus* ouvre la liste
  de tous les modules, les notifications, la palette (recherche), les réglages. Chaque écran garde sa position dans la
  liste, rendue au retour ; toucher l'onglet du module affiché remonte en haut, puis ramène à son accueil. Un lien vers
  un autre module ouvre son écran ; le retour ramène au module d'où l'on vient, avec sa pile (un onglet du bas touché
  entre-temps, ou un écran ouvert par le PC, oublie le lien).
- **Actions** : une action envoyée reste en cours jusqu'à la réponse du PC : son bouton, sa ligne, son entrée de menu
  ou la barre du haut sont grisés avec un indicateur, et un second toucher n'envoie rien. Succès : vibration brève ;
  échec : bulle rouge, qui se déplie pour être lue en entier ; sans réponse après 15 s : « Le PC n'a pas répondu ». Un
  menu, une confirmation ou une saisie ouverts se ferment si le PC ouvre un autre écran (« L'écran a changé : rien
  n'est parti. »). Dans un menu, les actions dangereuses sont en bas ; la barre du haut montre une action en clair, ou
  deux en icônes avant ⋯. Cibles de 44 à 48 px, texte pâle à 4,5:1 au moins, libellés pour le lecteur d'écran.
- **Sélecteur de dossiers du PC** : une action qui demande un dossier (cloner, ajouter un dépôt, « Autre dossier… »
  de Fawkes) ouvre un sélecteur plein écran ; le PC liste, le téléphone montre (§5). À la racine : « Récents », les
  dossiers du module (dossier de clonage, dossiers analysés de DragonTree, dossiers connus de Fawkes), « Accès
  rapide », « Lecteurs ». Fil d'Ariane, marques (dépôt, déjà connu, caché, lien, en ligne seulement, lecteur ;
  protégé : grisé), pages (« N de plus »), dossiers cachés au menu, filtre fait par le PC, chemin saisi à la main, et
  en bas « Choisir ce dossier », ou pourquoi pas, ou ce que donnera le clone ; quand l'action attend un dépôt, il se
  choisit d'un toucher. Rien ne se choisit pendant qu'un autre dossier se charge. Un refus du PC s'affiche à sa place,
  avec « Réessayer » et « Saisir le chemin » (un champ de texte, contrôlé de la même façon) ; délai de 15 s ou
  coupure : « Réessayer », et la demande perdue repart au retour du PC. Sans *dossiers du PC*, ou avec un PC qui ne
  liste pas, un champ de texte, avec la raison.
- **Terminal** : plein écran, police à chasse fixe ; zoom au pincement ou par A− / A+, gardé ; défilement de
  l'historique ; le bas reste en vue quand le clavier s'ouvre, tant qu'on n'a pas remonté. Barre de touches : Ctrl et
  Alt collants pour la touche suivante (Ctrl + lettre ; Alt et le caractère partent d'un seul envoi, pour ne pas
  interrompre Claude par un Échap seul), relâchés par la touche suivante, Retour, un toucher du champ, et par
  « Coller », A− et A+ ; Échap, Tab, flèches, Entrée. « Coller » envoie le presse-papiers en collage (demandé d'abord
  s'il fait plusieurs lignes ou cache des touches, §5). « Copier », ou un appui long sur une ligne, ouvre le texte
  montré et l'historique gardé dans un champ figé, sans retour à la ligne (les colonnes sont gardées, il défile de
  côté) : sélection d'Android, « Copier », « Tout copier ». Le champ de saisie va à la ligne (cinq lignes environ) ;
  sur plusieurs lignes, il part en collage puis Entrée ; ni majuscule ni correction automatiques. Réponses rapides
  quand Claude attend.
- **Hors ligne** : bandeau « Reconnexion… », dernières données gardées, reconnexion automatique (délai croissant
  jusqu'à 30 s, immédiate au retour au premier plan et quand le téléphone se reconnecte à Tailscale). Une coupure
  sans fermeture (changement de réseau, PC en veille) se voit au plus tard après 45 s sans rien du PC, qui répond
  `Pong` à chaque `Ping` (toutes les 15 s). Tant que rien n'est arrivé du PC, la vue d'ensemble dit « En attente du
  PC… » ; hors ligne, de quand datent ses données.
- **PC hors d'atteinte** : après une connexion ratée, la liaison regarde (au plus toutes les 3 s, hors du fil de
  l'interface) le nœud du téléphone, puis le veilleur du PC. Une carte de la vue d'ensemble et le bandeau du haut
  disent : « PC injoignable » (éteint, en veille ou hors de Tailscale ; « Vu sur Tailscale il y a 2 h. »), « Ce
  téléphone est hors ligne », « Eyrie est fermée » (« a été fermée » quand elle l'a dit en se fermant), « Eyrie
  démarre… », « Accès coupé sur le PC », « Accès refusé par le PC ». Eyrie fermée et le veilleur qui répond :
  « Démarrer Eyrie », confirmé, sur la carte et dans le bandeau de chaque onglet ; sans secret, le téléphone dit ce
  qu'il faut (le droit *agir* et une connexion à Eyrie pour le recevoir ; ou, le PC parti du tailnet avec elle, cocher
  « Démarrer Eyrie depuis le téléphone »). Après l'accord du veilleur, un essai toutes les 2 s pendant une minute. Un
  refus est montré tel quel ; après « preuve refusée » ou un refus faute d'*agir*, le secret est oublié jusqu'au
  prochain accueil. Rien n'est jamais redemandé de soi-même.
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
  détail (titre du terminal, message) est dans l'application. Toucher une notification ouvre l'écran où elle mène (la
  session Claude qui attend, ou l'accueil de Fawkes quand plusieurs attendent ; l'écran d'un module arrêté, avec
  « Relancer le module ») ; sans écran, elle se déplie.
- **Thème** : celui du système (clair ou sombre), couleurs de la suite.
- **Réglages** : PC appairés (changer, oublier), nom de l'appareil, Tailscale (état, connexion, déconnexion), taille
  du terminal, à propos.

## 8. Plus tard

- Chiffrer l'état du nœud Tailscale du téléphone par le Keystore d'Android, comme celui du PC par DPAPI.
- ~~Service au premier plan (rester connecté en arrière-plan)~~ : pas nécessaire selon l'utilisateur (28/10), tant
  que le téléphone rattrape tout à la reconnexion (§7).
- iOS.
- Ce qui attend une fonction Rust `pub` des voisins, pas encore accordée : « Reprendre dans Fawkes » une conversation
  de Gringotts (le profil de Claude dans la demande « ouvrir » de Fawkes) ; « Rendre actif » un profil de Grimoire ;
  pousser d'ici une branche de DragonTree dont la suivie porte un autre nom (`RemoteOp::PushTo`) ; « dernier fetch il y
  a 5 min » (`last_fetch`).
- Une application d'avant la passe d'ergonomie ne sait ni les écrans locaux ni les liens « @module » (« Cet écran
  n'existe plus ») : le PC pourrait s'y adapter par ce que dit son `Bonjour`, qu'il ne lit pas encore.

## 9. État au 29/10/2026

Fait. Le premier jet (27-28/10) est sur `main`, poussé, avec les fonctions des voisins (leurs branches `suite/mobile`,
fusionnées dans leur `main`). La passe d'ergonomie (28-29/10) est sur la branche `mobile-ergonomie` d'Eyrie et sur
`suite/mobile-ergonomie` de DragonTree, Fawkes, Owlery et Menagerie : locales, rien de poussé.

| Pièce | Vérifié par |
|---|---|
| Protocole `suite-mobile` | ticket et code, preuves, code de vérification, canal Noise (trame altérée, requête HTTP refusées), messages, différences de terminal, réponses rapides ; défi du veilleur (vecteurs partagés avec le Go) ; dossiers protégés |
| Pont (`eyrie`) | appairage confirmé ou refusé, abandonné par le téléphone pendant que le PC attend (demande retirée, accord trop tard sans effet, nouveau code jusqu'à la même heure, ou dit expiré près de sa fin), code faux ou expiré, cinq essais puis ticket brûlé, appareil inconnu, autre compte Tailscale, jeton du compagnon faux, clé du PC du QR ; bout en bout avec un module factice (écran, action proposée ou non, terminal et saisie, journal, bulle) ; interrupteur ; chaque droit sur ses chemins (*terminaux*, *dossiers du PC*, secret de démarrage selon *agir*), refus au journal ; dossiers (formes, lecteurs, liens, noms courts, zones, racine et récents) ; notifications qui mènent à leur écran ; veilleur sur un faux système (valeur Run, DPAPI, élévation, Gestionnaire des tâches, rattachement, compagnon lent), le vrai tube sur des noms propres à l'essai ; emplacement d'Eyrie avec les règles du veilleur (droits relevés sur le poste de l'utilisateur, vrais dossiers temporaires ; la vraie installation à la main, `EYRIE_PLACE_REELLE`) |
| Écrans des modules | un crate d'essais par module (`suite-fawkes`, `suite-patronus`, `suite-owlery`, `suite-menagerie`, `suite-dragontree`, `suite-grimoire`, `suite-gringotts`, `suite-carnet`) : chemins, liste blanche, bandeaux, menus ; DragonTree sur de vrais dépôts temporaires (chaque écran atteignable depuis la liste existe sur le PC, clonage, onglets refermés, branche distante supprimée, pull requests) |
| Application (`eyrie-mobile`) | mise à jour sur place, pliage, navigation et liens, actions en cours, sélecteur (réponses du pont), veilleur (échange, secret oublié, liaison abandonnée), touches du terminal, texte à copier ; captures de toutes les scènes de démonstration, dans les deux thèmes |
| Compagnon (`go test`) | relais et en-tête, un compagnon par nœud, tube (nom tiré, client vérifié, un autre compte), port 4748 (preuves, droits, limites, journal), emplacement d'Eyrie, élévation, arrêt |
| Voisins | DragonTree, Fawkes, Owlery, Menagerie : les essais de chaque fonction ajoutée |
| Ensemble (27/10) | essai réel sur le poste : Eyrie (dossier d'essai, `EYRIE_MOBILE_LOCAL`) et l'application de bureau, appairage par le lien, clic « Appairer » par l'automatisation d'interface de Windows, vraies données (Fawkes et un vrai PowerShell en direct, Gringotts, Owlery), journal |

Nombre d'essais par crate : voir le rapport de la vague 3.

Reste :
- **APK** : SDK Android dans `%LOCALAPPDATA%\Android` (JDK 17, licences acceptées par l'utilisateur,
  `platform-tools`, `platforms;android-36`, `build-tools;36.1.0`, `ndk;28.2.13676358`), puis `tools\android.ps1
  -Release` → `target\android\eyrie-release.apk`, signé par la clé de débogage d'Android
  (`%USERPROFILE%\.android\debug.keystore`, à garder : une version signée d'une autre clé ne s'installe pas
  par-dessus). `-Install` l'installe par le débogage (adb sans fil sur le téléphone de l'utilisateur). L'APK de `main`
  démarre sur ce téléphone depuis le 28/10 (plantage de tsnet corrigé) ; la connexion à Tailscale et l'appairage
  restent à essayer.
- **La passe sur le téléphone** : reconstruire ensemble Eyrie et le compagnon (une Eyrie nouvelle ne trouve pas le
  tube d'un veilleur d'avant ; `tools\compagnon.ps1 -Release` arrête le veilleur avant de le remplacer), puis l'APK.
  L'APK de la passe n'a encore été ni construit ni installé.
- **Pas encore essayé en vrai** : sur le téléphone, le pincement, le clavier avec Ctrl ou Alt collé, la sélection
  d'Android dans le texte à copier, Retour dans le sélecteur de dossiers ; sur le PC, le veilleur avec un vrai nœud et
  une vraie valeur Run, une Eyrie élevée, la fenêtre réduite au démarrage.
- **Essai par Tailscale** : `eyrie.exe` (release) et `tools\compagnon.ps1 -Release`, activer Réglages › Tailscale
  puis Téléphones ; sur le téléphone, connecter Eyrie à Tailscale (même compte), puis scanner le QR code.
- **Voisins** : Eyrie compile DragonTree, Fawkes, Owlery et Menagerie par chemin ; leurs branches
  `suite/mobile-ergonomie` doivent rester extraites tant que `mobile-ergonomie` n'est pas fusionnée, et se fusionner
  avec elle.
- Plus tard : §8.
