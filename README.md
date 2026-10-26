# Eyrie

Les six modules — Fawkes, DragonTree, Patronus, Owlery, Menagerie, Grimoire — réunis dans une seule
application, « un peu à la manière de Visual Studio et ses différentes fenêtres ». **Eyrie** est
l'application ; **EyrieSuite** désigne ce qui est commun à toute la suite (`%APPDATA%\EyrieSuite`, le
coffre « EyrieSuite » de Windows et, à terme, le dossier d'installation de tous les modules).

Chaque module garde son dépôt et son exécutable ; Eyrie compile leurs bibliothèques et les contient
(voir `docs/premier-jet.md`). Un module sait s'il est lancé seul ou dans Eyrie : seul, il a ses propres
réglages ; dans Eyrie, ses réglages sont dans ceux d'Eyrie et ce qu'Eyrie règle pour tous (thème,
langue, comptes, Tailscale) disparaît des siens.

## Ce que fait Eyrie

- **Cases** (`egui_tiles`) : onglets, colonnes, lignes, grilles, glisser-déposer, séparateurs ;
  dispositions en un clic (tout en onglets, côte à côte, empilés, grille, un grand + pile) ;
  dispositions enregistrées sous un nom, et une par configuration d'écrans.
- **Fenêtres détachées** : une case dans sa propre fenêtre (deuxième écran), rattachée d'un clic ou à
  la fermeture ; position et taille gardées, retour sur l'écran principal si l'écran a disparu.
- **Mode concentration** : une case en grand (double-clic sur l'onglet, Alt+Maj+Entrée).
- **Barre d'activité** avec pastilles, **vue d'ensemble** de ce qui attend dans tous les modules,
  **centre de notifications** commun, **palette de commandes** (F1) qui exécute aussi les commandes
  des modules, **journal** d'Eyrie, **ressources** (temps passé par module).
- **Réglages en un seul endroit** : ceux d'Eyrie (thème, langue, zoom, Tailscale, raccourcis et leurs
  conflits, dispositions) et ceux de chaque module, sur sa page ; le « Réglages » d'un module y mène.
  Dans une fenêtre détachée, ⚙ Réglages ne montre qu'Eyrie et les modules de cette fenêtre.
- **Comptes communs à toute la suite** (GitHub, GitLab, Azure DevOps) : on se connecte une fois, dans
  Eyrie, DragonTree ou Owlery, seuls ou intégrés ; liste dans `%APPDATA%\EyrieSuite`, jetons dans le
  coffre « EyrieSuite » de Windows. Intégrés, les modules renvoient à la page Comptes d'Eyrie
  (`docs/comptes.md`).
- **Bac à sable** autour de chaque appel de module : style global rétabli, clavier au seul module de
  la case cliquée, commandes de fenêtre contenues, plantage isolé.
- **Contrat local en appel Rust** : un module demande à un autre (`ouvrir` un terminal dans Fawkes
  depuis Owlery…) par le bus d'Eyrie, sans tube.
- **Tout ou partie des modules**, activables et arrêtables à chaud ; un module natif d'exemple,
  **Carnet** (notes par dépôt, journal du bus), sert de gabarit (`docs/ecrire-un-module.md`).

## Branches

Tout est sur `main` (23/10/2026), ici comme dans les six modules : Eyrie se compile avec leur `main`.
Les branches de travail sont gardées pour l'historique : `premier-jet` (l'hôte, sans toucher aux
modules), `suite/etape-2` dans Eyrie et `suite/module` dans les modules (voir `docs/etape-2.md`).

## Compiler

Les dépôts des modules doivent être **voisins** de celui-ci (`../Fawkes`, `../DragonTree`,
`../Patronus`, `../Owlery`, `../Menagerie`, `../Grimoire`) : Eyrie en dépend par chemin, en lecture
seule.

Toolchain `x86_64-pc-windows-gnu` (comme les modules) : `tools\setup.ps1` écrit `.cargo\config.toml`
(rust-lld, dlltool), propre au poste et non versionné. Avec la toolchain MSVC, rien à faire.

```powershell
tools\setup.ps1
cargo build --release        # target\release\eyrie.exe
cargo test --workspace
cargo test -p eyrie -- --ignored   # essais avec les huit modules réels (mode essai), un par un
```

Formater : `cargo fmt -p eyrie -p suite-sdk -p suite-carnet …`, **pas `cargo fmt --all`**, qui
formaterait aussi les dépôts des modules (dépendances par chemin).

Un module seul, depuis son dépôt, avec la même configuration :
`cargo test --config ..\Eyrie\.cargo\config.toml`.

La première compilation prend quelques minutes : toutes les dépendances des six modules (un seul
egui 0.37 pour tous).

## Lancer

```powershell
target\release\eyrie.exe
```

Au premier lancement, Eyrie demande où les modules rangent leur état (aucun module ne démarre
avant ce choix ; elle se relance ensuite) :

- **les données des exécutables** (par défaut) : mêmes groupes, terminaux, tâches et filtres que
  `fawkes.exe`, `patronus.exe`… La suite prend le même verrou que chaque exe : un module dont l'exe
  tourne déjà n'est pas démarré (sa case propose de fermer l'exe puis de réessayer) ;
- **un dossier d'essai** (`%APPDATA%\EyrieSuite\Eyrie\essai\<module>`) : pour essayer sans rien toucher.

Variables utiles :

| Variable | Effet |
|---|---|
| `EYRIE_DATA_DIR` | dossier d'Eyrie (défaut `%APPDATA%\EyrieSuite\Eyrie`) ; les comptes y sont alors isolés |
| `EYRIESUITE_DIR` | dossier commun de la suite (défaut `%APPDATA%\EyrieSuite`) : comptes partagés |
| `OWLERY_DEMO=1` | Owlery en démonstration (aucun réseau, jamais les vrais jetons) |
| `EYRIE_CAPTURE=<png>` | capture de la fenêtre principale puis fermeture (essais) |
| `EYRIE_CAPTURE_AFTER_MS` | délai avant la capture (4000 par défaut) |
| `EYRIE_CAPTURE_SCENE` | scène : `palette`, `reglages`, `modules`, `raccourcis`, `ensemble`, `journal`, `ressources`, `grille`, `colonnes`, `detacher`, `concentration`, `terminal`, `module:<id>`, `theme:<graphite\|fluent\|nuit>` (séparées par des virgules) |
| `FAWKES_TAILNET_EXE` | compagnon Tailscale (sinon cherché à côté de l'exe ou dans `../Fawkes/target`) |
| `SUITE_FAWKES_DIR` | autre emplacement des sources de Fawkes |

`eyrie.exe` comprend aussi ce que les modules demandent à leur propre exécutable quand ils
tournent dans Eyrie (`current_exe()`) : `--relais …` (Menagerie), `lancer <tâche>` et
`planifie <tâche>` (Patronus, Planificateur de Windows).

## Raccourcis d'Eyrie

| Raccourci | Action |
|---|---|
| F1 | palette de commandes |
| Ctrl+F6 / Ctrl+Maj+F6 | case suivante / précédente |
| Alt+1…9 | aller au module n |
| Alt+Maj+Entrée | agrandir ou restaurer la case (concentration) |
| Alt+Maj+D | détacher la case dans une fenêtre |
| Alt+Maj+O | vue d'ensemble |

Ils passent avant ceux des modules (modifiables dans Réglages > Raccourcis). Ctrl+Alt est évité :
c'est AltGr sur un clavier français.

## Organisation

```
crates/
  suite-sdk/        interface de module, services de l'hôte, adaptateur eframe, thèmes, polices,
                    langue, raccourcis, état versionné, instance unique
  suite-accounts/   comptes de forge communs à toute la suite (liste, coffre, normalisation)
  eyrie/            l'hôte (exe eyrie) : cases, fenêtres, bac à sable, palette, réglages…
  modules/
    fawkes/         Fawkes : sa bibliothèque, plus l'adaptateur
    dragontree/     adaptateurs des cinq autres modules (leur bibliothèque, telle quelle)
    owlery/
    patronus/
    menagerie/
    grimoire/
    carnet/         module natif, gabarit
docs/
  etat-des-lieux.md, audit/   le constat (avant ce premier jet)
  reflexions.md               le raisonnement, les questions ouvertes
  premier-jet.md              ce qui est fait, les choix provisoires, les limites
  etape-2.md                  premières retouches dans les modules, essais approfondis
  ecrire-un-module.md         écrire un module, ou embarquer une application existante
  feuille-de-route.md         le suivi
```
