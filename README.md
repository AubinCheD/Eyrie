# SuperApp

Les six modules — Fawkes, DragonTree, Patronus, Owlery, Menagerie, Grimoire — réunis dans une seule
application, « un peu à la manière de Visual Studio et ses différentes fenêtres ». *SuperApp* est un
nom de travail.

**Premier jet** : l'hôte et les modules réunis **sans changer le code des modules**. Chaque module
garde son dépôt et son exécutable ; la suite compile leurs bibliothèques et les contient (voir
`docs/premier-jet.md`).

## Ce que fait la suite

- **Cases** (`egui_tiles`) : onglets, colonnes, lignes, grilles, glisser-déposer, séparateurs ;
  dispositions en un clic (tout en onglets, côte à côte, empilés, grille, un grand + pile) ;
  dispositions enregistrées sous un nom, et une par configuration d'écrans.
- **Fenêtres détachées** : une case dans sa propre fenêtre (deuxième écran), rattachée d'un clic ou à
  la fermeture ; position et taille gardées, retour sur l'écran principal si l'écran a disparu.
- **Mode concentration** : une case en grand (double-clic sur l'onglet, Alt+Maj+Entrée).
- **Barre d'activité** avec pastilles, **vue d'ensemble** de ce qui attend dans tous les modules,
  **centre de notifications** commun, **palette de commandes** (F1) qui exécute aussi les commandes
  des modules, **journal** de la suite, **ressources** (temps passé par module).
- **Réglages communs** : thème (les trois thèmes de Fawkes + Système), langue (fr/en), zoom,
  comptes de forges (coffre de Windows, partage compte par compte, import depuis DragonTree et
  Owlery), Tailscale (un seul nœud pour la suite), raccourcis et leurs conflits.
- **Bac à sable** autour de chaque appel de module : style global rétabli, clavier au seul module de
  la case cliquée, commandes de fenêtre contenues, plantage isolé.
- **Contrat local en appel Rust** : un module demande à un autre (`ouvrir` un terminal dans Fawkes
  depuis Owlery…) par le bus de la suite, sans tube.
- **Tout ou partie des modules**, activables et arrêtables à chaud ; un module natif d'exemple,
  **Carnet** (notes par dépôt, journal du bus), sert de gabarit (`docs/ecrire-un-module.md`).

## Compiler

Les dépôts des modules doivent être **voisins** de celui-ci (`../Fawkes`, `../DragonTree`,
`../Patronus`, `../Owlery`, `../Menagerie`, `../Grimoire`) : la suite en dépend par chemin, en lecture
seule.

Toolchain `x86_64-pc-windows-gnu` (comme les modules) : `tools\setup.ps1` écrit `.cargo\config.toml`
(rust-lld, dlltool), propre au poste et non versionné. Avec la toolchain MSVC, rien à faire.

```powershell
tools\setup.ps1
cargo build --release        # target\release\superapp.exe
cargo test --workspace --exclude suite-fawkes
cargo test -p suite-fawkes suite::   # l'adaptateur seul (les tests de Fawkes restent dans son dépôt)
```

La première compilation prend quelques minutes : toutes les dépendances des six modules (un seul
egui 0.37 pour tous).

## Lancer

```powershell
target\release\superapp.exe
```

Au premier lancement, la suite demande où les modules rangent leur état (aucun module ne démarre
avant ce choix ; elle se relance ensuite) :

- **les données des exécutables** (par défaut) : mêmes groupes, terminaux, tâches et filtres que
  `fawkes.exe`, `patronus.exe`… La suite prend le même verrou que chaque exe : un module dont l'exe
  tourne déjà n'est pas démarré (sa case propose de fermer l'exe puis de réessayer) ;
- **un dossier d'essai** (`%APPDATA%\SuperApp\essai\<module>`) : pour essayer sans rien toucher.

Variables utiles :

| Variable | Effet |
|---|---|
| `SUPERAPP_DATA_DIR` | dossier de la suite (défaut `%APPDATA%\SuperApp`) |
| `OWLERY_DEMO=1` | Owlery en démonstration (aucun réseau, jamais les vrais jetons) |
| `SUPERAPP_CAPTURE=<png>` | capture de la fenêtre principale puis fermeture (essais) |
| `SUPERAPP_CAPTURE_AFTER_MS` | délai avant la capture (4000 par défaut) |
| `SUPERAPP_CAPTURE_SCENE` | scène : `palette`, `reglages`, `modules`, `raccourcis`, `ensemble`, `journal`, `ressources`, `grille`, `colonnes`, `detacher`, `concentration`, `terminal`, `module:<id>`, `theme:<graphite\|fluent\|nuit>` (séparées par des virgules) |
| `FAWKES_TAILNET_EXE` | compagnon Tailscale (sinon cherché à côté de l'exe ou dans `../Fawkes/target`) |
| `SUITE_FAWKES_DIR` | autre emplacement des sources de Fawkes |

`superapp.exe` comprend aussi ce que les modules demandent à leur propre exécutable quand ils
tournent dans la suite (`current_exe()`) : `--relais …` (Menagerie), `lancer <tâche>` et
`planifie <tâche>` (Patronus, Planificateur de Windows).

## Raccourcis de la suite

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
  superapp/         l'hôte (exe superapp) : cases, fenêtres, bac à sable, palette, réglages…
  modules/
    fawkes/         Fawkes : ses sources compilées en bibliothèque (build.rs), plus l'adaptateur
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
  ecrire-un-module.md         écrire un module, ou embarquer une application existante
  feuille-de-route.md         le suivi
```
