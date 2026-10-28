# Téléphone : passe d'ergonomie, DragonTree approfondi, dossiers du PC, démarrage à distance

Demande du 28/10/2026 : « une passe complète d'amélioration de l'appli mobile au niveau ergonomie (ex. des bandeaux
pliants par groupe de terminaux dans Fawkes). Dans DragonTree, rien ne fonctionne sur l'appli mobile : je veux voir
mes dépôts, l'historique des branches, etc. (passe approfondie). Est-il possible de voir l'arborescence de dossiers du
PC depuis le téléphone pour choisir où cloner un dépôt, ou pour ajouter un dépôt existant à DragonTree ? Est-il
possible de démarrer Eyrie sur le PC depuis le téléphone si l'application PC n'est pas démarrée ? »

L'utilisateur a demandé de sauter les questions pour le téléphone (27/10) : les choix ci-dessous sont les miens,
marqués **(choix)**. Le protocole commun est fait (commit f4b1c15, `crates/suite-mobile`).

## 1. Pourquoi DragonTree « ne marche pas »

Constaté sur le code et sur les vrais dépôts de l'utilisateur (Ozmo : 1 605 branches distantes, 2 073 étiquettes,
87 000 commits) :

1. **Bloquant** : les sous-écrans d'un dépôt ouvrent des chemins relatifs (`modifs`, `historique`, `branches`,
   `commit/<id>`…) ; le PC ne les trouve pas (`Phone::screen` attend `depot/<clé>/…`) et répond « Cet écran n'existe
   plus sur le PC ». Seules la liste et l'accueil d'un dépôt s'affichaient. Les essais appelaient `Phone::screen` avec
   des chemins complets.
2. « Historique » montrait le haut du graphe de DragonTree sur toutes les références (des commits de collègues), sans
   graphe ni étiquettes, et se disait celui de la branche courante. Toucher une branche la basculait.
3. La liste ne montrait que 13 des 23 dépôts (onglets et récents), et cinq lignes s'appelaient « Ozmo ».
4. Branches et étiquettes faisaient 1 600 et 2 000 lignes (≈ 1 Mo), recalculées toutes les 300 ms et renvoyées à
   chaque pour cent d'une progression ; le téléphone recréait toute la liste à chaque fois (touchers perdus).
5. Une lecture ratée restait « Lecture… » sans dire pourquoi ; les onglets de fond n'étaient jamais relus quand
   DragonTree n'est pas affiché sur le PC (`tick` et l'enregistrement des réglages ne tournent que dans `ui`).

## 2. Protocole (fait, `crates/suite-mobile`)

Tout champ ajouté est `#[serde(default)]` ; aucune variante de `Block` n'est ajoutée (une application plus ancienne
perdrait la connexion). Les deux messages nouveaux ne partent que si l'autre côté les a annoncés.

- **Chemins d'écran absolus dans le module** (règle écrite en tête de `ui.rs`).
- `Block::Section` = **bandeau** : `key`, `fold` (`Fold::No/Open/Closed`, état de départ), `color`, `glyph`, `badge`
  (visible plié), `tone`, `acts` (menu ⋯). Construction : `Screen::band(Band::new(clé, titre)…)`.
- `Row` : `key` (identité stable : mise à jour sur place), `trailing` (bouton visible), `chips` (étiquettes),
  `graph: GraphCell` (ligne de commit), `progress` (pour cent, `PROGRESS_UNKNOWN` = indéterminée).
- `Block::Note` : `progress` et `acts` (une opération en cours et son « Annuler ») ; `Screen::progress(…)`.
- `Block::Diff` : `numbers` (ancien, nouveau). `Act` : `open` (écran local, rien au PC), `busy` (grisé, indicateur).
- `Screen` : `tabs` (onglets : on passe de l'un à l'autre sans empiler), `search` (texte d'invite ; filtre local).
- `Input.folder: FolderInput { want: FOLDER_ANY|FOLDER_REPO|FOLDER_PARENT, start, child, places, known }`.
- `ToPc::Browse` / `ToPhone::Folder { req, listing: Listing }` ; `Rights.files` (« Dossiers du PC », accordé par
  défaut, y compris aux appareils déjà appairés) ; `Note.path` ; `Hello.features` ; `Welcome.wake: WakeKey` ;
  `Bye.closing`.
- `veille.rs` : défi-réponse du démarrage à distance (§6), vecteurs partagés avec le Go.

Graphe : un lien d'un enfant (ligne `e`, voie `a`) vers son parent (ligne `p`, voie `c`) passant par la voie `b`
donne `(GRAPH_BOTTOM, a, b)` à la ligne `e`, `(GRAPH_THROUGH, b, b)` entre les deux, `(GRAPH_TOP, b, c)` à la ligne
`p` — le dessin de DragonTree (`graph_paint.rs`). Couleurs : indices dans une palette de `LANE_COLORS` (10), celle de
DragonTree (`GRAPH_LIGHT`/`GRAPH_DARK`).

## 3. L'application (`crates/eyrie-mobile`)

Corrections de fond :
- **Listes mises à jour sur place** (modèles persistants, correspondance par `Row.key`, sinon titre+glyphe) : plus
  de reconstruction à chaque message ; le toucher et l'appui long ne se perdent plus. Un écran reçu qui n'est pas
  l'écran affiché ne touche pas la liste ; un message `Modules` ne reconstruit pas l'écran.
- **Défilement gardé par écran** (et par onglet), remis en haut pour un écran nouveau, rendu au retour.
- **Boutons qui passent à la ligne** (FlexboxLayout de Slint 1.18) avec leur glyphe, 48 px.
- **Action en cours** : indicateur et bouton grisé jusqu'au `Result` (même `req`) ; pas de double envoi ; vibration
  courte au succès, bulle d'erreur ; « Le PC n'a pas répondu » après 15 s ; `Act.busy` dessiné pareil.

Nouveaux composants : bandeaux pliants (en-tête de 52 px touchable en entier, chevron, pastille de couleur, glyphe,
pastille de ton, nombre, ⋯), **pliage gardé sur le téléphone** par PC, module, chemin et clé **(choix : local au
téléphone, état de départ donné par le PC ; rien n'est envoyé au PC)**, règle « l'alerte gagne » (un bandeau plié qui
passe à Attention ou Danger se rouvre une fois), « Tout replier / Tout déplier » ; champ de recherche (sans casse ni
accents ; les sections qui ont des résultats restent visibles et dépliées) ; onglets sous le titre ; lignes de commit
denses avec graphe (traits `Path`, nœud, anneau pour HEAD) et étiquettes ; barres de progression ; bouton visible à
droite d'une ligne ; chevron dès qu'une ligne ouvre un écran.

Le reste de l'audit : notifications qui mènent à leur écran (`Note.path`), vue d'ensemble honnête hors ligne (« En
attente du PC… », âge des données), menu d'actions qui défile (actions dangereuses en bas), barre du haut avec deux
actions en icônes avant ⋯, bulles enchaînées sans se couper et erreurs lisibles en entier, cibles de 48 px, contraste
du texte pâle, libellés d'accessibilité. Terminal : pincement pour zoomer (gardé), copier (sélection dans un champ en
lecture seule), coller (`TermInput::Paste`), Ctrl collant, suivi du bas quand le clavier s'ouvre, saisie sur
plusieurs lignes envoyée en collage.

## 4. DragonTree (`crates/modules/dragontree`, et `../DragonTree` sur la branche `suite/mobile-ergonomie`)

**(choix)** :
- **Liste** : tous les dépôts de DragonTree — ouverts sur le PC, épinglés (favoris), un bandeau pliable par dossier
  racine analysé, puis les récents hors racines — lus dans DragonTree lui-même (en mémoire), avec son état rapide ;
  noms homonymes distingués par le dossier parent (« Ozmo · Ozmo2 ») ; recherche.
- **Un dépôt a quatre onglets** : État (`depot/<k>`), Historique (`depot/<k>/historique`), Branches
  (`depot/<k>/branches`), Plus (`depot/<k>/plus` : remises, étiquettes, worktrees, remotes, afficher sur le PC). Les
  détails s'empilent (`depot/<k>/fichier/<f>`, `depot/<k>/commit/<id>`, `depot/<k>/branche/<b>`…).
- **Historique** : par défaut celui de la branche courante, avec son graphe ; « Toutes les branches » montre le graphe
  de DragonTree (sa portée, ses fusions repliées), avec les étiquettes ; 100 commits par page, « plus » jusqu'à 2 000.
- **Branches** : la courante, puis les locales par date ; regroupées par dossier (`feature/…`) en bandeaux au-delà de
  12 ; distantes par remote, pliées ; 100 lignes au plus par bandeau avec « N autres » ; toucher **ouvre le détail**
  (suivi, à pousser, à tirer, historique de la branche avec graphe, et les gestes : basculer, fusionner, rebaser,
  pousser, renommer, supprimer) — on ne bascule plus d'un toucher.
- Un dépôt regardé reste ouvert par DragonTree dans un onglet de fond (règle « tout par DragonTree ») ; ceux que le
  téléphone a ouverts lui-même sont refermés après 15 min sans regard et sans opération.
- DragonTree vit même masqué dans Eyrie (`background_tick`) : surveillance du disque, enregistrement des réglages,
  suites d'un clonage ; le dépôt regardé est traité comme actif. Une lecture ratée se dit sur le téléphone.
- **Ajouter un dépôt existant** (sélecteur, `FOLDER_REPO`) et **Cloner** (URL, sélecteur `FOLDER_PARENT`, nom du
  dossier, branche facultative ; compte choisi par DragonTree d'après l'URL) : par DragonTree, en onglet de fond, avec
  sa progression et « Annuler » ; « Afficher sur le PC » ensuite.

API ajoutées à DragonTree (accord du 27/10 : « ce qui sert l'application mobile ») : répertoire des dépôts et leur état
rapide, `background_tick`/foyer distant, erreurs de lecture gardées sur l'onglet, `add_repo`, clonage sans fenêtre
(`clone_remote`, suivi, annulation), `close_repo_tab`, suppression d'une branche distante.

## 5. Dossiers du PC depuis le téléphone (réponse : oui)

**(choix)** Un sélecteur natif du téléphone, alimenté par un service du pont (`crates/eyrie/src/mobile/browse.rs`).
- Il n'existe que pour une action qu'un écran a proposée avec `input.folder` ; le pont retient la spécification de la
  saisie avec l'action (liste blanche) et **revérifie le chemin choisi** avant d'appeler le module : forme `X:\`
  (pas d'UNC, `\\?\`, `\\.\`, flux `:`, `..`, noms réservés), résolution (`canonicalize` : 8.3, jonctions),
  lecteur local fixe, hors zones interdites (Windows, Program Files, ProgramData, AppData, `.ssh`, données d'Eyrie,
  corbeille…), puis la règle de `want` (dépôt git ; destination absente ou vide).
- Des dossiers seulement, jamais de fichier ; cachés et système masqués (« Afficher les dossiers cachés » dans le
  menu) ; un niveau à la fois, dans un fil, pages de 200, 20 000 entrées ou 3 s au plus, filtre côté PC ; rafales
  limitées. Pas de création de dossier (un clone crée sa destination).
- Racine : récents du sélecteur (gardés sur le PC), raccourcis du module, accès rapide (profil, Bureau, Documents,
  Téléchargements, `source\repos`, `DEV`), lecteurs fixes.
- Droit « Dossiers du PC » par appareil (en plus d'« agir ») ; journal à l'ouverture du sélecteur et au choix (le
  chemin choisi est écrit en clair).
- Une application plus ancienne voit un champ de texte, contrôlé de la même façon.

## 6. Démarrer Eyrie depuis le téléphone (réponse : oui, à trois conditions)

Le PC doit être allumé et éveillé, la session Windows ouverte (verrouillée, ça va), et le **veilleur** actif.
Impossible : PC éteint, en veille ou en veille prolongée (le Wake-on-LAN de Tailscale exige un autre appareil éveillé
sur le même réseau local), session fermée.

**(choix)** Désactivé par défaut : case « Garder le PC joignable quand Eyrie est fermée » (Réglages › Téléphones).

- Le compagnon `eyrie-tailnet.exe` gagne un mode `--veille` : lancé à l'ouverture de session (valeur `EyrieVeille` de
  `HKCU\…\Run`, refusée si l'exécutable ou son dossier est modifiable par d'autres comptes), il garde le nœud
  d'Eyrie sur le tailnet quand elle est fermée. Eyrie s'y rattache par un tube nommé au lieu de lancer un compagnon.
- Un seul processus par nœud : mutex `Local\EyrieTailnet:<dir_key(dossier du nœud)>` (`dir_key` de
  `suite-sdk/src/instance.rs` : `\` → `/`, sans `/` final, minuscules).
- Tube : `\\.\pipe\eyrie-tailnet-<16 hex du BLAKE2s du dir_key>`, réservé à l'utilisateur, client vérifié (même
  utilisateur, image `<dossier du compagnon>\eyrie.exe`), un client à la fois. Lignes JSON : Eyrie → veilleur
  `{"relais":"127.0.0.1:port","jeton":"…"}` (comme l'entrée standard aujourd'hui), `{"relais":""}`, `{"arret":true}` ;
  veilleur → Eyrie : les lignes d'état actuelles. Tube fermé : relais coupé, le nœud reste.
- Port **4748** du tailnet : défi-réponse de `suite-mobile/src/veille.rs`. Le veilleur relit à chaque demande
  `mobile\veille.dpapi` (DPAPI, entropie `eyrie/veille` ; JSON `{"v":1,"actif":bool,"cle":"<64 hex>",
  "instance":"Local\\Eyrie-instance:<dir_key>","reduite":true}`) et `mobile\appareils.json` (le compte Tailscale de
  la connexion doit être celui d'un appareil appairé ; `appareil` = son `id` ; secret = `device_secret(cle, clé
  publique)`). Limites : 5 preuves fausses → 15 min de blocage ; un démarrage par 30 s, 6 par heure ; Eyrie déjà
  ouverte ou en démarrage → rien.
- Lancement : seulement `<dossier du compagnon>\eyrie.exe`, après le contrôle d'emplacement ; environnement de
  l'utilisateur à jour ; `--reduite` ; `EYRIE_DEMARRE_PAR=<nom de l'appareil>` ; ligne `veille` au journal du pont.
  **(choix)** Eyrie démarrée à distance s'ouvre réduite.
- « Accès depuis le téléphone » coupé → `actif=false` (le veilleur refuse, répond `acces-coupe`) ; « Couper tout
  accès à distance » → arrêt du veilleur et valeur Run retirée.
- Téléphone : trois états distincts — « PC injoignable (éteint, en veille, hors réseau) », « Eyrie est fermée » avec
  « Démarrer Eyrie » (confirmation) si le veilleur répond, « Accès coupé sur le PC » ; Eyrie envoie
  `Bye { closing: true }` en se fermant. Le nœud du téléphone dit si le PC est en ligne sur le tailnet
  (`EyrieTailnetPeer`).

## 7. Autres modules

- **Fawkes** **(choix : pliage local, ouvert par défaut sauf groupes vides ou tout en veille ; toujours ouvert s'il
  attend, est en échec ou sonne)** : un bandeau par groupe, dans l'ordre du PC, groupes vides compris : couleur et
  icône du groupe, « ✻2 · 5 » (sessions Claude, terminaux), pastille « N attend » / « en échec » / 🔔. Menu : shell
  ou Claude dans le dépôt du groupe, nouveau terminal dans ce groupe, Remote Control des sessions du groupe, tout
  mettre en veille, tout réveiller, tout redémarrer (API `restart_group` de Fawkes), montrer sur le PC. Un bandeau
  « Claude attend » en tête mène aux sessions. « Nouveau terminal » en barre du haut ; « Autre dossier… » par le
  sélecteur. États distincts (travaille, prêt, en veille).
- **Owlery** : bandeaux comme le PC (par dépôt par défaut), « N autres… », reporter (3 h, demain, lundi), annuler,
  tout marquer lu (API `mark_many`), ouvrir.
- **Patronus** : bandeaux En cours / Boîte de réception / Tâches, menu des résultats, tout marquer lu.
- **Menagerie** : un bandeau par projet compose avec Démarrer tout / Arrêter tout (API `docker_project_action`),
  ports pliés au-delà de 6.
- **Grimoire, Gringotts, Carnet** : bandeaux ; Carnet « Ajouter une ligne » en barre du haut.
- **Écran générique d'Eyrie** : relancer un module arrêté, réessayer un module indisponible ; une commande lancée dit
  qu'elle est partie.

## 8. Non-buts

- Pas de création, suppression ou lecture de fichier depuis le téléphone ; pas de « Faire confiance » à un dépôt.
- Pas de service Android au premier plan (décision du 28/10).
- Pas de nouvelle variante de `Block`, pas de changement de `PROTOCOL`.
- Rien n'est poussé : commits locaux, branche `mobile-ergonomie` (Eyrie) et `suite/mobile-ergonomie` (modules).
