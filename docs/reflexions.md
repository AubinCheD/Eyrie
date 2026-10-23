# Réflexions de conception de la suite

Ce document garde le raisonnement : la demande, ce qui est décidé, ce qui est proposé et ce qui reste
ouvert. Le constat technique est dans `etat-des-lieux.md` (détail par dépôt dans `audit/`), le suivi
dans `feuille-de-route.md`.

Légende : **décidé** (choix de l'utilisateur), **proposé** (à valider), **ouvert** (question posée).

## Sommaire

| # | Sujet | État |
|---|---|---|
| 1 | La demande du 22/10 | — |
| 2 | Décisions prises | décidé |
| 3 | Ce qui change par rapport à Fawkes §3.5 | décidé |
| 4 | Architecture : hôte, modules, services | proposé |
| 5 | Disposition des fenêtres et fenêtres détachées | proposé, questions ouvertes |
| 6 | Réglages communs : thème, langue, comptes, Tailscale | proposé |
| 7 | Clavier, focus et raccourcis | proposé |
| 8 | Identité de processus : exe autonome et suite | proposé |
| 9 | Organisation des dépôts | décidé pour l'instant, avis donné |
| 10 | Idées en plus | proposé |
| 11 | Questions ouvertes | ouvert ; choix provisoires du premier jet dans `premier-jet.md` §5 |

## 1. La demande du 22/10

- Une « super app » qui réunit les six modules : Fawkes, DragonTree, Patronus, Owlery, Menagerie,
  Grimoire.
- **D'abord l'intégration** : les modules dans une seule application, « un peu à la manière de Visual
  Studio et ses différentes fenêtres ».
- **Réorganiser les fenêtres des modules** librement, « à la manière des terminaux dans Fawkes ».
- **Sortir un module dans sa propre fenêtre**, pour le multi-écrans.
- **Garder l'exe de chaque module** : lancé seul, il utilise ses propres paramètres.
- **Homogénéiser certains paramètres** quand les modules tournent dans la suite : thème, langue, comptes
  connectés (ADO, GitHub…), Tailscale interne pour la synchro mobile (Fawkes seul pour l'instant).
- **Tout ou partie des modules**, tout réglable ; d'autres modules viendront.
- **Un dépôt par module** pour l'instant, pour que chacun puisse installer une application seule
  (« je ne sais pas si c'est une bonne idée ») : voir §9.

## 2. Décisions prises

- Les six modules restent des applications autonomes, avec leur exe.
- La suite peut lancer n'importe quel sous-ensemble des modules, choisi dans ses réglages.
- La suite est extensible : ajouter un module ne doit pas demander de retoucher les autres.
- Dans la suite, thème, langue, comptes et Tailscale sont communs.
- Un dépôt par module (provisoire).

## 3. Ce qui change par rapport à Fawkes §3.5

Fawkes `reflexions.md` §3.5 prévoyait que **chaque application lise aussi les réglages communs de la
suite quand elle tourne seule** (défauts < commun < propres au module).

La demande du 22/10 dit l'inverse : **lancé seul, un module utilise ses propres paramètres**. Ce qui
reste de §3.5 :
- chaque module garde son fichier d'état, dans son dossier, lu et écrit de la même façon seul ou dans
  la suite : ses groupes, terminaux, tâches, filtres… sont les mêmes des deux côtés ;
- seuls les réglages **homogénéisés** (thème, langue, comptes, Tailscale) viennent de la suite quand le
  module y tourne ; ses propres valeurs ne sont ni lues ni écrasées, elles reviennent quand on relance
  l'exe seul.

**Proposé** : une case dans chaque exe autonome, *Suivre les réglages de la suite* (désactivée par
défaut), pour qui veut le même thème partout.

## 4. Architecture : hôte, modules, services (proposé)

Reprend Fawkes §3.3 (« Niveau 3 — la suite ») en le précisant avec les audits.

**Un module = une bibliothèque** qui implémente une interface commune. L'exe autonome devient une
enveloppe fine qui fournit à la bibliothèque une implémentation « autonome » des services ; la suite en
fournit une autre.

Ce que le module expose (esquisse, noms provisoires) :
- identité : identifiant stable (`fawkes`, `dragontree`…), nom affiché, icône ;
- `new(ctx, hôte)` : construit le module avec les services de l'hôte ;
- `logic(ctx)` : tout ce qui n'est pas de l'affichage, appelé **à chaque image même si le module n'est pas
  visible** (Menagerie, Patronus et Grimoire doivent sortir leur travail de fond de `ui()`) ;
- `ui(ui, vue)` : dessine une vue dans le `Ui` donné ;
- les vues qu'il propose (voir §5) ;
- badge (compteur, couleur), titre ;
- commandes (palette, raccourcis par défaut) ;
- page de réglages propres ;
- `demande(…)` : les demandes des autres modules (le contrat, en appel Rust) ;
- `peut_fermer()` : travail en cours, modifications non enregistrées ;
- `on_exit()`.

Ce que l'hôte fournit (services) :
- palette de couleurs et polices (le module ne touche plus au style global) ;
- langue ;
- comptes et coffre de jetons ;
- notifications (une seule icône dans la zone de notification) ;
- registre des raccourcis et palette commune ;
- « montre-moi », « attire l'attention », badge, titre (à la place des `ViewportCommand`) ;
- réveil ciblé de la fenêtre où le module est affiché (`request_repaint_of`) ;
- dossier de données du module ;
- envoi de demandes aux autres modules ;
- arrêt de Windows, diffusé à tous les modules (aujourd'hui dans la procédure de fenêtre de Fawkes).

**Modules compilés dans la suite, activés à l'exécution** (Fawkes §3.3). Pas de modules chargés en DLL :
Rust n'a pas d'ABI stable, et tous les modules partagent déjà la même version d'egui. Un module désactivé
n'est pas construit et ne coûte rien.

**Isolation des plantages** (proposé) : dans un seul processus, la panique d'un module tue tout. L'hôte
appelle chaque module sous `catch_unwind` ; un module qui panique est remplacé par une case « Le module X
s'est arrêté » avec *Relancer* et le journal. Les autres continuent.

## 5. Disposition des fenêtres et fenêtres détachées

**Proposé : `egui_tiles`** (0.18, compatible egui 0.37). Il fait des arbres de cases : onglets, lignes,
colonnes et **grilles**, avec glisser-déposer et séparateurs. C'est à la fois le docking de Visual Studio
et les grilles de Fawkes. `egui_dock` est plus proche de Visual Studio (zones d'accroche), mais il est
encore sur egui 0.36 : il faudrait attendre ou le porter.

**Fenêtres détachées** (proposé) :
- une fenêtre OS = un viewport egui **immédiat** (les applications ne sont ni `Send` ni `Sync`) avec
  son propre arbre de cases ;
- *Détacher* sur l'onglet d'une case, ou glisser l'onglet hors de la fenêtre ;
- *Rattacher* ou fermer la fenêtre renvoie ses cases dans la fenêtre principale ;
- position, taille et écran de chaque fenêtre enregistrés ; si l'écran a disparu, la fenêtre revient sur
  l'écran principal (`on_some_monitor` existe déjà dans plusieurs modules).

**Dispositions enregistrées** (proposé, comme les « dispositions de fenêtre » de Visual Studio) :
plusieurs dispositions nommées (« Revue », « Dev », « Exploitation »), une par défaut, et une
**disposition par configuration d'écrans** (portable seul, station d'accueil à deux écrans) choisie
automatiquement au branchement.

**Questions ouvertes**
- **L'unité de disposition** : le module entier, ou des **vues** de module ? Par exemple, DragonTree
  en plusieurs cases (historique, changements, ou un dépôt par case), Menagerie en trois (services,
  mails, environnements), un groupe Fawkes par case. Un même module peut-il apparaître deux fois ?
- **Docking libre ou grilles prédéfinies** ? Fawkes propose des dispositions en un clic (1, 2, 3
  colonnes, 2×2, 1 grand + 2…) avec des séparateurs ; Visual Studio laisse tout accrocher n'importe où.
  Les deux sont possibles avec `egui_tiles` (les dispositions en un clic produisent un arbre).
- **Menus des modules** : dans la barre de titre de la suite pour le module actif (Fawkes §3.3), ou dans
  une barre propre à chaque case ?

## 6. Réglages communs (proposé)

**Thème**
- Les trois thèmes de Fawkes (Graphite, Fluent, Nuit colorée), plus *Système*, deviennent ceux de la
  suite : ce sont les plus aboutis (contrastes WCAG testés).
- Une **palette sémantique** commune (fond, surface, texte, accent, succès, alerte, danger, bordure…),
  que chaque module complète par ses couleurs métier : terminal et 16 couleurs ANSI (Fawkes), branches du
  graphe et coloration syntaxique (DragonTree).
- **Polices** : l'hôte installe une seule fois toutes les familles nécessaires (`term`, `term-primary`,
  `bold`, `diff`, Phosphor, emoji monochromes). Les modules déclarent ce dont ils ont besoin et
  n'appellent plus `set_fonts`.
- Zoom et taille de texte communs.

**Langue**
- `dragontree-i18n` devient la bibliothèque commune, avec un **catalogue par module** (aujourd'hui un seul
  `en.json` pour tout DragonTree).
- La langue de la suite s'applique d'abord à DragonTree seul : les cinq autres sont en français en dur. Les
  traduire est un chantier à part (3 à 5 j chacun).

**Comptes**
- Reprend la proposition du contrat d'Owlery (§5) : un **coffre de la suite** (keyring, service propre à
  la suite, clé `account:<id>`), une liste sans jeton dans le dossier de la suite, et un **partage compte
  par compte** (tel compte GitHub pour DragonTree et Owlery, pas pour Patronus).
- `dragontree-forges` et `Owlery/src/forges` fusionnent dans une crate commune (même modèle, mêmes
  forges : GitHub, GitLab, Azure DevOps, avec leurs versions hébergées).
- À la première ouverture de la suite : **proposer d'importer** les comptes de DragonTree et d'Owlery
  (copie ; les exe autonomes gardent les leurs).
- Les profils Claude de Grimoire (`CLAUDE_CONFIG_DIR`) sont aussi des comptes : Fawkes pourrait ouvrir une
  session Claude avec le bon profil (`--config-dir`).

**Tailscale et synchro mobile**
- Un seul nœud pour la suite (et non un par module), au nom générique, état dans le dossier de la suite
  (chiffré par DPAPI, déjà prévu dans Fawkes).
- Le compagnon de Fawkes (`fawkes-tailnet.exe`, Go/tsnet) devient celui de la suite ; un routeur par module
  derrière (`/fawkes`, `/owlery`…), pour couvrir tous les modules (Fawkes §5, « Super app »).
- Fawkes lancé seul garde son propre nœud.

## 7. Clavier, focus et raccourcis (proposé)

Le problème le plus partagé : chaque module lit ou consomme les raccourcis sur toute la fenêtre.

- **Module actif** : celui de la case qui a reçu le dernier clic ou le focus. L'hôte le connaît.
- **Filtrage par l'hôte** : avant d'appeler `ui()` d'un module qui n'est pas actif, l'hôte retire les
  événements clavier de l'entrée d'egui, puis les rend au module actif. Les modules n'ont presque rien à
  changer ; à vérifier par un essai (le texte saisi dans un champ dépend aussi de la mémoire de focus
  d'egui).
- **Registre commun** : chaque module déclare ses commandes et raccourcis ; l'hôte détecte les conflits,
  et les raccourcis globaux (palette, réglages, changer de case, détacher) lui appartiennent.
  Ctrl+P reste au shell dans Fawkes (Fawkes §3.3).
- **Fawkes ne reprend plus le focus tout seul** (`panes.rs:454`) quand il est dans la suite.

## 8. Identité de processus : exe autonome et suite (proposé)

Règle de Fawkes §3.1 et §3.3 : **un module ne tourne jamais deux fois sur le même dossier d'état**.

- La suite prend, pour chaque module activé, **le même verrou** que son exe (mutex, `state.lock`) et
  **sert le même tube** (`owlery-<SID>`…). Lancer `owlery.exe` pendant que la suite tourne envoie donc
  `afficher` à la suite, qui montre la case d'Owlery : les exe n'ont rien à changer.
- Si l'exe autonome tourne déjà au démarrage de la suite : la suite démarre sans ce module et propose
  *Fermer Fawkes et le reprendre ici* (ouvert : faut-il le faire sans demander ?).
- Ce qui dépend de `current_exe()` (Planificateur et clé `Run` de Patronus, relais `--relais` de
  Menagerie) doit continuer à viser l'exe du module, ou la suite doit accepter les mêmes arguments.
- `grimoire-statusline.exe` est lancé par Claude Code, pas par Grimoire : il reste un exe livré avec la
  suite.

## 9. Organisation des dépôts

**Décidé pour l'instant** : un dépôt par module, plus ce dépôt pour la suite.

**Avis** : pouvoir installer une application seule est une question de **distribution**, pas de dépôts.
Un dépôt unique peut produire six exe et six installeurs séparés, plus la suite. Ce que coûtent des dépôts
séparés :
- une seule version d'egui pour tous : chaque montée de version touche sept dépôts en même temps ;
- la crate commune (thème, comptes, i18n, raccourcis…) doit être publiée ou référencée par chemin ou par
  git, avec un numéro de révision à suivre dans chaque dépôt ;
- un changement transversal (renommer un service de l'hôte) demande sept commits coordonnés.

Ce qu'ils apportent : historiques, README et suivi séparés, et l'habitude actuelle.

**Pour que ça marche quand même** (proposé) :
- dépôts voisins dans le même dossier (c'est le cas : `DEV/Fawkes`, `DEV/DragonTree`…), dépendances par
  chemin relatif (`../Fawkes`) pour le développement ;
- un script dans ce dépôt qui clone ou met à jour les dépôts voisins ;
- versions d'egui et d'eframe vérifiées identiques par un test ou un script ;
- **ouvert** : où vit la crate commune (dans ce dépôt, ou un dépôt à part dont tous dépendent) ?

On pourra regrouper plus tard (`git subtree` garde l'historique de chaque dépôt) si la coordination
pèse trop.

## 10. Idées en plus (proposé)

- **Contexte partagé** : choisir un dépôt dans DragonTree peut, au choix, filtrer les autres cases : le
  groupe Fawkes du dépôt, ses PR dans Owlery, ses services dans Menagerie, son `CLAUDE.md` dans
  Grimoire, ses tâches dans Patronus. Une option *Suivre le contexte* par case, comme la synchronisation
  avec le document actif de Visual Studio.
- **Bus d'événements interne** : les événements prévus dans les contrats (`service_arrete`, `ci_echec`,
  `pr_ouverte`, motif dans un terminal…) passent par l'hôte. Patronus peut alors déclencher une tâche sur
  n'importe quel événement de n'importe quel module, sans tube.
- **Vue d'ensemble** (Fawkes §2f, élargie) : ce qui attend l'utilisateur dans tous les modules (sessions
  Claude en attente, CI en échec, services arrêtés, tâches terminées).
- **Barre d'activité** à gauche avec badges, **palette commune**, **centre de notifications** commun,
  **une seule icône** dans la zone de notification (Fawkes §3.3).
- **Agrandir une case** (comme Ctrl+Maj+M dans Fawkes) et un **mode concentration** qui masque le reste.
- **Démarrage progressif** : la logique des modules activés démarre tout de suite, leur interface à la
  première ouverture de leur case.
- **Ressources par module** : mémoire, fils et temps d'image de chaque module, utile dans un seul
  processus.
- **Gabarit de module** : un module d'exemple minimal et une page « écrire un module », pour les
  modules à venir.
- **Une seule mise à jour** pour la suite (Fawkes §6), et un installeur à cases (Fawkes §3.3).
- **Nom de la suite** (décidé le 23/10/2026 : **Eyrie**, et **EyrieSuite** pour ce qui est commun) : *SuperApp* était un nom de travail. Dans l'esprit des modules : *Poudlard*
  (Hogwarts, le château qui les réunit), *Diagon* (le Chemin de Traverse, une rue de boutiques),
  *Salle sur Demande* (une pièce qui prend la forme qu'on lui demande, comme la disposition). Pour la
  crate des échanges entre modules : *Floo* (le réseau des cheminées).

## 11. Questions ouvertes

Le premier jet (23/10) a pris un choix provisoire pour chacune, pour avancer sans toucher aux
modules : voir `premier-jet.md` §5. Ils restent à valider.

Par ordre d'importance pour la conception :
1. Unité de disposition : module entier ou vues de module ; un module deux fois (§5).
2. Docking libre, grilles prédéfinies, ou les deux (§5).
3. Où vont les menus des modules (§5).
4. Exe autonome déjà ouvert au lancement de la suite : le reprendre, demander, ou démarrer sans (§8).
5. Réglages homogénéisés : imposés dans la suite, ou *Suivre la suite* module par module ?
6. Langue : faut-il traduire les cinq autres modules, et dans quelles langues ?
7. Comptes : importer ceux de DragonTree et d'Owlery ; les profils Claude comptent-ils comme des comptes ?
8. Emplacement de la crate commune (§9).
9. Nom de la suite.
