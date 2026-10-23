# Premier jet de la suite (23/10/2026)

> Suite : `etape-2.md` (premières retouches dans les modules, sur des branches à part). Ce
> document décrit le premier jet tel qu'il est sur la branche `premier-jet`.

Ce document décrit ce qui est construit, les choix faits pour avancer sur les questions ouvertes
(`reflexions.md` §11) et ce qui reste. Le constat de départ est dans `etat-des-lieux.md`.

**Contrainte du premier jet** : ne pas toucher au code des modules. Les six dépôts voisins sont lus,
compilés, jamais modifiés. Tout ce qui déborde de leur case est contenu par l'hôte.

## 1. Ce qui tourne

Les six modules réels, plus un module natif (Carnet), dans une seule fenêtre, en un seul processus :

- Fawkes (ses terminaux, sa restauration, sa barre de titre dans sa case) ;
- DragonTree (son interface complète, ses onglets de dépôts) ;
- Owlery (sa boîte de réception ; en démonstration avec `OWLERY_DEMO=1`) ;
- Patronus (tâches, boîte de résultats, planification) ;
- Menagerie (services, ports relevés, Docker, mails, environnements) ;
- Grimoire (configuration de Claude Code, conversations, consommation) ;
- Carnet (notes par dépôt, journal du bus) : le gabarit de module.

Vérifié en lançant la suite en mode essai (dossiers d'état d'essai pour chaque module) avec des
captures automatiques : grille des sept modules, vue d'ensemble, palette, réglages, thèmes Graphite,
Fluent et Nuit, interface en anglais, case détachée, et une demande `ouvrir` qui ouvre un vrai
terminal pwsh dans Fawkes.

## 2. Architecture

```
eyrie (exe)               l'hôte : cases, fenêtres, bac à sable, palette, réglages, comptes…
  └─ suite-sdk            interface Module, services Host, adaptateur Embedded, thème, polices,
  │                       langue, raccourcis, état versionné, instance unique
  ├─ suite-fawkes         sources de ../Fawkes compilées en bibliothèque + adaptateur
  ├─ suite-dragontree     dragontree-app (bibliothèque) + adaptateur
  ├─ suite-owlery         owlery (bibliothèque) + adaptateur, coffre/notifications/Fawkes de la suite
  ├─ suite-patronus       patronus-app + adaptateur, `lancer` et `planifie` pour eyrie.exe
  ├─ suite-menagerie      menagerie (bibliothèque) + adaptateur
  ├─ suite-grimoire       grimoire (bibliothèque) + adaptateur
  └─ suite-carnet         module natif
```

**Interface d'un module** (`suite-sdk/src/module.rs`) : identité (`ModuleInfo`), vues, `logic` (à
chaque image, même masqué), `ui(ui, vue)`, besoin d'un `ui` même masqué, état (titre, pastille, ce
qui attend), commandes, page de réglages, événements du bus, demandes des autres modules, commandes
de fenêtre relevées, « peut-on fermer ? », `on_exit`.

**Services de l'hôte** (`suite-sdk/src/host.rs`) : palette et thème, langue, dossiers, notifications,
bus d'événements, demandes entre modules, « montre-moi », attention, contexte partagé, comptes et
jetons (partage compte par compte), modules présents. `Host` se clone et passe d'un fil à l'autre ;
tout est mis en file et traité par l'hôte à l'image suivante, jamais pendant l'appel d'un autre module.

**À chaque image** (`eyrie/src/app.rs`) :
1. `logic` : polices rétablies si un module les a remplacées ; thème suivi si « Système » ; fermeture
   concertée ; file de l'hôte (notifications, événements, demandes, « montre-moi ») ; logique de tous
   les modules actifs, affichés ou non, chacun dans le bac à sable ;
2. `ui` : raccourcis de la suite, barre de menus, barre d'activité, barre d'état, cases de la fenêtre
   principale, fenêtres détachées, modules masqués qui ont besoin de leur `ui`, puis palette,
   réglages, bulles et dialogues.

**Modules compilés dans la suite, activés à l'exécution**. Activer, arrêter ou relancer un module se
fait à chaud : la reconstruction utilise un `CreationContext` sans fenêtre
(`CreationContext::_new_kittest`, une API cachée d'eframe 0.37 ; à surveiller aux montées de version).
Seule conséquence : Patronus reconstruit à chaud n'a pas d'icône dans la zone de notification.

## 3. Embarquer un module sans changer son code

**Fawkes** n'a pas de `lib.rs`. `crates/modules/fawkes/build.rs` relit les `mod …;` de
`Fawkes/src/main.rs` et écrit les mêmes déclarations avec un `#[path]` vers le dépôt voisin : les
sources de Fawkes sont compilées telles quelles à la racine de `suite-fawkes` (leurs chemins
`crate::…` restent justes). Un module ajouté à Fawkes est pris automatiquement ; seules ses
dépendances (`Cargo.toml`) sont à recopier, et la version (Fawkes lit `CARGO_PKG_VERSION`) est
vérifiée par le script.

**Les cinq autres** ont déjà une bibliothèque. Chaque adaptateur refait ce que fait le `main.rs` du
module (verrou d'instance, canal nommé, chargement de l'état), sans `run_native`, puis passe
l'`eframe::App` à `suite_sdk::embed::Embedded`, qui :
- appelle `logic` et `ui` d'eframe ;
- lit la pastille dans le titre que l'application donne à sa fenêtre (« Owlery — 6 en attente »,
  « (3) Patronus », « Menagerie — 2 à voir ») ;
- fait de ses raccourcis connus des commandes de la palette : les exécuter, c'est lui envoyer la
  frappe ;
- garde ce qui doit vivre autant que le module (verrous, serveur de tube, icône de notification).

Ce que chaque adaptateur fait en plus :

| Module | En plus |
|---|---|
| Fawkes | thème de la suite pendant son affichage, puis son propre réglage rétabli (son état garde le thème choisi dans Fawkes seul) ; géométrie de la fenêtre rendue après chaque affichage (sinon `fawkes.exe` rouvrirait à la taille de la suite) ; sessions Claude en attente → pastille et vue d'ensemble ; reçoit `ouvrir{dir, titre, groupe, claude, resume}` en appel Rust |
| Owlery | coffre : les comptes de la suite partagés avec Owlery, puis son propre coffre ; notifications → centre de la suite ; `FawkesLink` → bus de la suite (ses boutons « ouvrir dans Fawkes » ouvrent le Fawkes de la suite) |
| Patronus | même verrou que `patronus.exe` ; icône de notification sur la fenêtre de la suite ; `eyrie.exe lancer|planifie <tâche>` (repris de son `main.rs`) |
| Menagerie | `eyrie.exe --relais …` (son relais chiffré, lancé par `current_exe()`) |
| DragonTree | ses polices (Phosphor pour ses icônes) entrent dans le plan de polices de la suite ; instance unique du profil (`dragontree.exe <dépôt>` ouvre le dépôt dans la suite) |
| Grimoire | même verrou et même canal nommé que `grimoire.exe` |

## 4. Le bac à sable

Autour de **chaque** appel de module (`eyrie/src/sandbox.rs`), en reprenant la liste des
problèmes de l'état des lieux (§3) :

| # | Problème | Ce que fait la suite | Ce qui reste |
|---|---|---|---|
| 1 | Style global (`set_visuals`, `set_theme`, `global_style_mut`, zoom, `override_text_color`) | relevé avant l'appel, rétabli après ; les panneaux d'un module héritent du style de sa case | Patronus, Menagerie et Grimoire gardent leurs couleurs propres dans leurs vues |
| 1 | Polices (`set_fonts` remplace tout ; Fawkes panique sans `term*`) | plan de polices de la suite (Segoe UI, Semibold, Cascadia, `term`, `term-primary`, `bold`, `diff`, Phosphor), rétabli au début de l'image si un module l'a remplacé, avant toute mise en page | la police du diff choisie dans DragonTree n'est pas suivie dans la suite |
| 2 | Raccourcis lus sur toute la fenêtre | un module sans le clavier ne voit ni touches, ni texte, ni copier-coller ; le clavier va au module de la case cliquée en dernier ; les raccourcis de la suite passent avant | les conflits sont listés (Réglages > Raccourcis) mais les raccourcis des modules ne se changent que dans les modules |
| 2 | Focus repris (Fawkes) | un module sans le clavier ne peut pas prendre le focus | — |
| 3 | Commandes de fenêtre | relevées, contrées (`Title`, `Minimized`, `Maximized`, `Decorations`, `Fullscreen`, `Visible(false)`) ; `Title` devient titre et pastille, `Focus` devient « montre-moi », `RequestUserAttention` passe ; `Close` d'un module est ignoré, sauf pendant une fermeture qu'il a retenue pour poser sa question | `StartDrag` : glisser la barre de titre de Fawkes déplace la suite |
| 3 | Fermeture | la demande de fermeture n'est montrée aux modules que quand la suite se ferme vraiment, et **un module à la fois** (une image chacun ; DragonTree, qui s'engage dès qu'il la voit, en dernier) : si l'un l'annule pour poser sa question, les suivants n'ont rien vu ; sa réponse (`Close`) fait reprendre la séquence après lui ; une question laissée sans réponse abandonne la fermeture au bout de 5 min. Un module dans une fenêtre détachée voit la demande comme les autres, et son annulation vaut pour la fenêtre principale | les modules ne disent pas d'avance s'ils ont du travail en cours (`can_close`) : leur question vient l'une après l'autre |
| 4 | `Modal`, `Window`, `Area` au niveau du contexte | — | une modale d'un module grise toute la suite ; ses fenêtres flottent sur toute la suite (hors de sa case) |
| 5 | Identifiants egui génériques | chaque case a son sel (`id_salt`) : panneaux, grilles, listes séparés | deux modules qui ouvrent en même temps une `Window` au même titre partagent son état |
| 6 | Instance unique, dossier d'état | même verrou (et même tube) que chaque exe ; exe déjà ouvert : module indisponible, « Fermer l'exe » puis « Réessayer » | — |
| 7 | Zone de notification | Owlery passe par le centre de la suite ; seule l'icône de Patronus est posée (Fawkes cherche sa fenêtre par son titre) | une seule icône pour la suite, à faire |
| 8 | Procédure de fenêtre de Fawkes (titre « Fawkes ») | — | dans la suite, Fawkes ne capte pas l'arrêt de Windows (sa sauvegarde finale passe par `on_exit`, pas par `WM_ENDSESSION`) ni le clic sur sa notification |
| 9 | Travail de fond dans `ui()` | Menagerie, Patronus et Grimoire sont dessinés hors écran, dans un `Ui` invisible et sans clavier, quand leur case est masquée | leurs fenêtres et modales éventuelles restent visibles |
| 10, 11 | Réveils, `Send`/`Sync` | fenêtres détachées en viewports **immédiats**, redessinés avec la fenêtre principale | — |
| 2 | Clic sur une fenêtre posée par-dessus une case | un clic sur une fenêtre flottante (d'un module, ou de la suite) ne donne pas le clavier au module de la case en dessous | — |
| — | Visibilité | un module non affiché voit sa fenêtre « masquée » (Fawkes arrête de redessiner ses terminaux), un module sans le clavier la voit « sans focus » (Owlery envoie alors ses notifications) | — |
| — | Zoom | `set_zoom_factor` d'un module ne s'applique qu'à l'image suivante, hors du bac à sable : le zoom de la suite est redemandé dès qu'il s'en écarte | une image au mauvais zoom ; Ctrl+molette n'importe où dans la suite est vu par Patronus (egui calcule ce zoom avant les appels, la suite ne peut pas le retirer) : il l'enregistre dans son état |
| — | Plantage | une panique est rattrapée : le module s'arrête (case « s'est arrêté », Relancer le module), les autres continuent | un module qui tient un verrou partagé en paniquant le laisse empoisonné pour lui seul |
| 12 | Langue | la suite est traduite (fr/en, catalogue par domaine) | DragonTree garde la langue de ses réglages ; les cinq autres sont en français ; les noms de commandes et les pastilles venant des modules restent en français |
| 13 | Champs inconnus perdus | la suite n'écrit jamais dans les fichiers des modules | — |
| — | Géométrie relevée par les modules | rendue à Fawkes | Owlery, Patronus, Menagerie et Grimoire relèvent la taille de la fenêtre de la suite dans leur état : lancés seuls ensuite, ils s'ouvrent à cette taille |
| — | Arrêter ou relancer un module | ses verrous et fichiers tenus par l'adaptateur sont rendus | les fils de canal nommé d'Owlery et de Grimoire survivent au module (un module relancé peut rester sans canal) ; Owlery et Menagerie gardent leur verrou d'instance jusqu'à la fermeture de la suite (leur exe ne démarre pas entre-temps) ; un module construit à chaud n'a pas de fenêtre dans son `CreationContext` (Patronus : pas d'icône de notification) |
| — | Un seul processus | — | les fonctionnalités des dépendances s'unifient (`serde_json/preserve_order` de Grimoire, `eframe/persistence` de DragonTree) ; le mode essai passe par des variables d'environnement héritées par les processus lancés depuis la suite |

Chaque débordement contenu est noté une fois par module dans le journal de la suite, et compté dans
la vue Ressources.

## 5. Choix provisoires (questions ouvertes de `reflexions.md` §11)

Pris pour pouvoir avancer ; tous se changent sans reprendre l'architecture.

1. **Unité de disposition : la vue de module.** Un module embarqué n'en a qu'une (son interface
   entière) ; un module natif en propose plusieurs (Carnet : Notes et Bus). Une vue n'apparaît qu'une
   fois dans toute la suite (ouvrir une vue déjà ouverte la montre).
2. **Docking libre et dispositions en un clic, les deux.** `egui_tiles` pour l'accroche libre ; les
   dispositions en un clic produisent un arbre (tout en onglets, côte à côte, empilés, grille, un
   grand + pile). Dispositions nommées proposées d'office : Dev (DragonTree, Fawkes), Revue (Owlery,
   DragonTree), Exploitation (Menagerie, Fawkes, Patronus).
3. **Menus : chaque module garde les siens dans sa case.** La suite a sa barre (Suite, Affichage,
   Modules, Aide) ; chaque barre d'onglets a un menu ⋯ (détacher, agrandir, vues, commandes du module,
   fermer la case).
4. **Exe autonome déjà ouvert : la suite démarre sans ce module** et propose de le fermer (`WM_CLOSE`
   poli : l'exe garde la main pour enregistrer), puis Réessayer. Jamais sans demander.
5. **Réglages homogénéisés : imposés dans la suite, là où le module sait les suivre.** La case
   « Suivre les réglages de la suite » dans chaque exe autonome demanderait de toucher aux modules :
   pas dans ce premier jet.
6. **Langue : la suite en français et en anglais.** La traduction des cinq modules non traduits reste
   un chantier à part.
7. **Comptes : communs à toute la suite** (décidé le 23/10/2026, `docs/comptes.md`). Eyrie, DragonTree
   et Owlery lisent et écrivent les mêmes, seuls ou intégrés ; chacun y reprend ses anciens comptes au
   premier lancement. Les profils Claude de Grimoire sont un type de compte prévu (`forge: "claude"`),
   pas encore géré.
8. **Crate commune : dans ce dépôt** (`crates/suite-sdk`). Un dépôt à part se justifiera quand les
   modules en dépendront eux-mêmes.
9. **Nom : Eyrie** (décidé le 23/10/2026 ; « SuperApp » était le nom de travail) ; **EyrieSuite** pour
   ce qui est commun à toute la suite.

Données des modules : par défaut celles des exécutables (décision du 22/10) ; un **mode essai** range
l'état de chaque module dans `<suite>\essai\<module>` (Réglages > Général). Au premier lancement,
**aucun module ne démarre avant ce choix** ; la suite se relance ensuite (les variables
d'environnement du mode essai doivent être posées avant le moindre fil).

## 6. Ce qui demanderait de toucher aux modules

Le premier jet s'en passe ; voici ce qui lèverait les limites restantes, du plus rentable au plus
coûteux. À faire module par module, avec accord.

| Retouche | Modules | Ce que ça lève |
|---|---|---|
| Exposer un vrai `impl Module` (ou un `show(ui)` + `logic()` sans `eframe::Frame`) | tous | l'adaptateur générique, la reconstruction sans API cachée d'eframe |
| Modales et fenêtres bornées au rectangle du module (ou routées vers l'hôte) | tous | §4 n° 4 |
| Palette fournie par l'hôte au lieu de leur thème propre | Patronus, Menagerie, Grimoire | thème commun complet |
| Langue : `set_lang` de la suite respecté | DragonTree | langue commune |
| Commandes et raccourcis déclarés (registre), plus de lecture du clavier global | tous | raccourcis remappables, palette exacte |
| Ne plus relever la géométrie de la fenêtre en mode module | Owlery, Patronus, Menagerie, Grimoire | exe autonome rouvert à sa taille |
| Crochet d'arrêt de Windows fourni par l'hôte | Fawkes | sauvegarde finale à l'arrêt de Windows |
| ~~Comptes lus via la suite~~ : fait le 23/10/2026 par les comptes communs (`suite-accounts`) | DragonTree, Owlery | comptes communs partout |
| Contexte partagé publié et suivi | DragonTree (dépôt actif), Fawkes (groupe), Owlery (filtre) | « Suivre le contexte » pour tous |
| Notifications par le centre de la suite | Patronus, Fawkes | une seule icône |

## 7. Vérifications

- `cargo test` : 20 tests du SDK (thèmes et contrastes WCAG, polices, bus, contexte, comptes, état
  versionné, instance unique, raccourcis, langue), 31 de l'hôte (bac à sable : clavier, style,
  commandes de fenêtre, panique, fermeture ; dispositions ; palette ; catalogue de traduction ; et de
  bout en bout avec egui_kittest : construction, détacher/rattacher, une vue jamais deux fois,
  dispositions nommées, bus, arrêt et relance d'un module, **un module qui panique est arrêté, le
  style de la suite rétabli, les autres continuent**, **fermeture en séquence : une question retient
  les modules suivants, sa réponse fait reprendre**, `Close` d'un module sans effet, zoom de la suite
  rétabli), plus ceux des adaptateurs et de Carnet.
- Relecture indépendante du code (13 points) : fermeture, premier lancement, zoom, double affichage,
  focus, fenêtres détachées (polices, commandes de la palette, fermeture), Tailscale, raccourcis —
  corrigés ; ce qui ne peut pas l'être sans toucher aux modules est dans le tableau §4.
- Lancements réels en mode essai, avec captures (`EYRIE_CAPTURE_SCENE`).
- Les six dépôts des modules sont inchangés (`git status` vide dans chacun).
