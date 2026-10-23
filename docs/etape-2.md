# Étape 2 : premières retouches dans les modules, essais approfondis (23/10/2026)

Suite du premier jet (`premier-jet.md`). Tout est sur des branches à part, rien sur `main` :

| Dépôt | Branche | Contenu |
|---|---|---|
| SuperApp | `premier-jet` | le premier jet, sans toucher aux modules |
| SuperApp | `suite/etape-2` | cette étape (part de `premier-jet`) |
| Fawkes, DragonTree, Owlery, Patronus, Menagerie, Grimoire | `suite/module` | les retouches ci-dessous (part de leur `main`) |

`suite/etape-2` se compile avec les six modules sur `suite/module` (les dépôts voisins sont lus
par chemin, tels qu'ils sont extraits). Pour revenir au premier jet : `premier-jet` dans SuperApp
et `main` dans les modules.

## Retouches dans les modules (`premier-jet.md` §6)

Chacune est petite, sans effet sur l'exécutable autonome (tout est désactivé par défaut), et passe
les tests du module.

| Module | Retouche | Ce que ça lève |
|---|---|---|
| Fawkes | `src/lib.rs` : les modules de `main.rs` deviennent une bibliothèque, `main.rs` l'enveloppe | le `build.rs` de la suite qui recompilait ses sources par `#[path]` disparaît : `suite-fawkes` dépend de la bibliothèque `fawkes` |
| DragonTree | `set_host_overrides(thème, langue)` : les remplacements de `--theme` et `--lang`, posés par l'hôte | DragonTree suit **la langue** et le clair/sombre de la suite, sans écrire dans ses réglages |
| Owlery, Patronus, Menagerie, Grimoire | `set_hosted(true)` : la géométrie de la fenêtre n'est pas enregistrée | lancés seuls après la suite, ils rouvrent à leur taille, pas à celle de la suite |
| Menagerie, Grimoire | `set_theme_override(thème)` et `theme()` : la palette affichée suit l'hôte | leurs couleurs suivent le thème clair ou sombre de la suite (Patronus le faisait déjà : sa palette suit le thème d'egui) |
| DragonTree | `active_repo_dir()` : le dépôt de l'onglet actif (lecture seule) | **contexte partagé** : le dépôt actif de DragonTree devient le contexte de la suite ; le Carnet passe à ses notes, Fawkes au groupe lié à ce dépôt (`group_for_dir`, déjà public) |

Dans la suite, les adaptateurs appellent ces points d'entrée (`before_logic`, nouveau crochet de
`Embedded`, à chaque image).

## Ce que la suite fait de plus

- Fawkes : les sessions Claude qui se mettent à attendre vont au centre de notifications de la
  suite (dans la suite, Fawkes ne trouve pas sa fenêtre pour poser les siennes).
- Contexte partagé : `Host::follows_context` (réglage de la suite) et crochet `on_event`
  d'`Embedded` ; le Carnet suit le contexte par défaut.
- Concentration : fermer la case agrandie (croix, action, module arrêté) arrête la concentration
  (trouvé par l'essai au hasard : la case disparue restait dessinée).

## Essais approfondis

- `random_actions_keep_the_layout_sound` : 4 graines × 400 actions au hasard (ouvrir, détacher,
  rattacher, agrandir, dispositions, thèmes, arrêt et relance, clics quelconques, événements du
  bus), invariants vérifiés après chacune.
- `real_modules_*` (lents, `cargo test -p eyrie -- --ignored --test-threads=1`) : les sept modules
  réels en mode essai — toutes les dispositions, chaque module agrandi, détaché, rattaché, les trois
  thèmes, arrêt et relance de chaque module, fermeture en séquence jusqu'au bout.
- `real_dragontree_shares_its_active_repository` : le vrai DragonTree reçoit un dépôt par le canal de
  son instance unique (comme `dragontree.exe <dépôt>` lancé à côté) ; il devient le contexte de la
  suite, et le Carnet le suit.
- Les suites de tests des modules, sur leur branche : Fawkes (156 + 3 de l'exe), Owlery (137),
  Patronus (plus de 160) au vert. Celles de Menagerie, Grimoire et DragonTree n'ont pas tourné : le
  poste manquait de mémoire et Claude Code a arrêté la commande (leurs retouches sont compilées et
  exercées par les essais avec les modules réels). À relancer :
  `cargo test --manifest-path ../<Module>/Cargo.toml --workspace`.
- Capture en anglais et en thème clair : DragonTree en anglais, Menagerie et Grimoire en clair.

## Reste (par intérêt)

- Modales et fenêtres des modules bornées à leur case.
- Raccourcis déclarés au registre de la suite, plus de lecture du clavier global.
- Comptes de la suite lus par DragonTree et Patronus ; contexte partagé suivi par Owlery (filtre
  par dépôt), Menagerie et Patronus.
- Une seule icône de notification ; crochet d'arrêt de Windows pour Fawkes.
- Traduction des cinq modules non traduits.
