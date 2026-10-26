# Recherche globale : étude de faisabilité

État au 26/10/2026. Fonction notée pour plus tard, rien n'est construit.

## La demande

Une barre de recherche au milieu du bandeau d'Eyrie (même ligne que les menus de gauche et de droite), où l'on
tape n'importe quoi sur n'importe quel module pour atterrir au bon endroit :

- « Recherche Eyrie » : le terminal de Fawkes où tourne la conversation Claude sur ce sujet ;
- « réglage » : au choix, les réglages d'Eyrie, ceux de Grimoire pour Claude Code, etc.

Variante évoquée : seulement une barre de recherche dans les réglages d'Eyrie, sur les pages de chaque module.

## Ce qui existe

**La palette (F1)** (`crates/eyrie/src/palette.rs`) cherche dans ce qu'Eyrie connaît : aller à un module ou à une
de ses vues, ouvrir une vue dans une nouvelle fenêtre, lancer une commande d'un module, dispositions, thème,
langue, six pages de réglages d'Eyrie, relancer. Correspondance par lettres dans l'ordre, sans accents.

Ce qu'elle ne trouve pas :

- les pages de réglages des modules (`Page::Module`), absentes de la liste ;
- tout ce qui est *dans* un module : conversations, terminaux, dépôts, entrées d'Owlery, réglages de Claude Code…

**L'interface de module** (`suite_sdk::Module`) donne au plus des vues, des commandes et des demandes sur le bus
(`on_request`). Rien pour chercher.

**Les modules savent déjà chercher chez eux**, sans l'exposer :

| Module | Ce qu'il connaît et cherche déjà |
|---|---|
| Fawkes | Recherche dans tous les terminaux (`ui/search_all.rs`) ; la session Claude de chaque terminal (`TermSpec.claude.session_id`, lue dans `~/.claude/sessions/<pid>.json`) ; sait relancer `claude --resume` |
| Grimoire | Chaque conversation : titre, dossier, branche, dates, texte des demandes et réponses (`transcripts::Session`) ; les réglages de Claude Code clé par clé, skills, hooks, serveurs MCP |
| DragonTree | Dépôts connus, branches, palette de commandes (`commands::MENUS`) |
| Owlery | Entrées (titre, dépôt, numéro, auteur), palette (Ctrl+P), filtres |
| Menagerie, Patronus | Services et ports ; tâches |
| Carnet | Notes par dépôt (module natif d'Eyrie) |

## Verdict

**Faisable**, en trois paliers. Les deux premiers ne touchent qu'Eyrie. Le troisième demande une méthode de plus
dans l'interface de module et un peu de code dans chaque module, à accorder module par module.

Les estimations ci-dessous sont des ordres de grandeur, essais compris.

## Palier 1 : la barre, sur ce qu'Eyrie sait déjà (Eyrie seul, environ 1 jour)

- La barre au milieu du bandeau. Elle dépend donc du chantier « menus dans le bandeau » : la zone qui sert à
  déplacer la fenêtre doit laisser un trou pour le champ (Fawkes le fait déjà pour sa recherche, voir
  `winhook.rs`).
- F1 (et Ctrl+K) y mettent le curseur. Les résultats se déroulent dessous, Échap ferme. La palette actuelle devient
  ce menu déroulant : une seule barre pour les actions et, plus tard, le contenu.
- Sources en plus : la page de réglages de chaque module (« Réglages : Grimoire »), chaque section des réglages
  d'Eyrie.
- Recherche par mots (chaque mot tapé doit apparaître), en plus des lettres dans l'ordre. Synonymes : paramètres,
  préférences et options pour réglages ; repo pour dépôt ; session pour conversation.
- Résultats groupés : Aller à, Réglages, Commandes, Dispositions.

Exemple 2 après ce palier : « réglage » propose « Réglages d'Eyrie » et « Réglages : Grimoire » (la page d'Eyrie).
Pas encore l'onglet Réglages de Grimoire (les réglages de Claude Code) : seul Grimoire le connaît (palier 3).

## Palier 2 : chercher dans les réglages (Eyrie seul, environ 1 à 2 jours)

C'est la variante « une barre de recherche dans les réglages d'Eyrie ». Eyrie dessine déjà les réglages de chaque
module dans sa propre fenêtre (`settings_ui`) : il peut relever leurs libellés sans rien demander aux modules.

- **Relevé** : à l'ouverture de la recherche, chaque page de réglages est dessinée une fois hors de l'écran, avec
  l'accessibilité active. Les libellés et leur position (case à cocher, champ, titre de section) forment l'index.
  Aucune frappe n'est envoyée : rien ne change dans les modules.
- **Atterrissage** : choisir un résultat ouvre la page, fait défiler jusqu'à la ligne et la surligne un instant.
- **Limites** : une section repliée n'est pas relevée tant qu'elle est fermée. Un texte dessiné à la main sans nom
  d'accessibilité est invisible (c'est aussi un défaut d'accessibilité, à corriger au cas par cas).
- **Si le relevé déçoit** : chaque module déclarerait ses sections (titre, mots-clés, ancre). C'est plus sûr, mais
  il faut toucher chaque module.

Recommandation : essayer le relevé d'abord, mesuré sur les sept pages réelles.

## Palier 3 : chercher dans le contenu des modules (interface et modules)

**Interface** : deux méthodes facultatives dans `suite_sdk::Module`.

```rust
/// Ce que le module trouve pour `query`, en mémoire seulement (appelé à chaque frappe, sur le fil de
/// l'interface : environ 1 ms par module au plus).
fn search(&self, query: &str, limit: usize) -> Vec<Hit> { Vec::new() }
/// Montre le résultat choisi ; Eyrie a déjà amené la case du module au premier plan.
fn open_hit(&mut self, id: &str, cx: &mut Cx) {}

pub struct Hit { pub id: String, pub title: String, pub detail: String, pub kind: HitKind, pub score: i32 }
```

Côté Eyrie, environ 1 jour : appel des modules qui tournent, résultats groupés par module (cinq au plus par
groupe, puis « voir plus »), ouverture de la case, puis `open_hit`.

**Par module** (module par module, avec accord) :

| Module | Ce qu'il trouverait | Où l'on atterrit | Coût |
|---|---|---|---|
| Fawkes | Terminaux (titre, dossier, groupe), sessions Claude en cours | Le terminal | 0,5 à 1 j |
| Grimoire | Conversations (titre, dossier), onglets, clés des réglages de Claude Code, skills, hooks, MCP | L'onglet ou l'élément | 1 à 2 j |
| DragonTree | Dépôts connus, branches du dépôt ouvert, commandes de menu | L'onglet du dépôt, la branche | 1 j |
| Owlery | Entrées (titre, dépôt, numéro, auteur) | L'entrée sélectionnée | 0,5 j |
| Menagerie | Services, ports, conteneurs | La ligne | 0,5 j |
| Patronus | Tâches | La tâche | 0,5 j |
| Carnet | Texte des notes | La note | moins d'une heure |

**Exemple 1, « Recherche Eyrie »** : il demande deux modules.

1. Grimoire connaît le titre et le texte de chaque conversation ; il trouve la bonne.
2. Fawkes sait quel terminal fait tourner quelle session (`session_id`).

Le résultat de Grimoire, une fois choisi, envoie une demande à Fawkes sur le bus (à créer : `montrer_session`,
avec l'identifiant de session). Selon le cas :
- la conversation tourne dans un terminal : Fawkes l'amène au premier plan ;
- elle ne tourne nulle part : on propose « Reprendre dans Fawkes » (`claude --resume`, que Fawkes sait déjà
  lancer).

Coût : 0,5 à 1 jour de plus, des deux côtés du bus.

## Points durs

- **Classement entre modules** : leurs scores ne se comparent pas. On classe dans chaque groupe et on ordonne les
  groupes (actions d'Eyrie d'abord, puis le module de la case active, puis les autres).
- **Vitesse** : à chaque frappe, en mémoire seulement. Le texte complet des conversations (Grimoire) peut peser
  des centaines de Mo : titres et dossiers dans la barre ; le plein texte sur Entrée, ou dans Grimoire. Un index
  construit en arrière-plan reste possible plus tard.
- **Exe autonomes** : la barre globale n'existe que dans Eyrie. Lancés seuls, les modules gardent leur palette.
- **Modules arrêtés** : une case fermée tourne encore, elle répond. Un module désactivé ne répond pas, on le dit
  (« Grimoire est désactivé »).

## Ordre proposé

1. Menus dans le bandeau (prérequis de l'emplacement).
2. Palier 1.
3. Palier 2.
4. Palier 3 : Grimoire et Fawkes d'abord (exemple 1), puis les autres modules.

## Questions pour plus tard

- Une seule barre pour les actions et le contenu (recommandé), ou F1 séparé ?
- Raccourcis : F1 seul, ou Ctrl+K en plus ?
- Plein texte des conversations : dans la barre globale, ou seulement dans Grimoire ?
