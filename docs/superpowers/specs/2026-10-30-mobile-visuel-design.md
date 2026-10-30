# Téléphone : icônes Material, écrans allégés, Owlery en onglets, saisie du terminal et fichiers

Demande du 29/10/2026 au soir : « améliore grandement les icônes de l'ensemble de l'application mobile en utilisant
les icônes Google Material. Supprime l'icône feuille DragonTree sur la vue principale (à gauche sur chaque dépôt) et
sur la vue détail. Les vues au sein d'un dépôt sélectionné peuvent être grandement améliorées, ça fait brouillon.
Pareil sur Owlery avec les « non lu ». C'est trop chargé. La vue principale d'Owlery doit sans doute être divisée en
plusieurs pages ou onglets que l'on fait glisser plutôt qu'une seule avec des bandeaux repliables. Sur Fawkes,
améliore l'ergonomie de la zone d'écriture (le texte se sélectionne tout seul, le curseur ne va pas exactement où l'on
veut), et je dois pouvoir insérer facilement des fichiers. »

L'utilisateur n'est pas là pendant le travail (pause demandée à 99 % de la limite hebdomadaire) : les choix sont les
miens, marqués **(choix)**.

## 1. Constat (captures de la démonstration et des écrans enregistrés d'Owlery)

- Chaque ligne a une tuile de 34 px avec un émoji (🌿 sur tous les dépôts et toutes les branches, « • » sur les
  lignes de détail d'Owlery), puis ⋯ **et** un chevron : la moitié de la largeur sert à des signes répétés.
- Owlery dit trois fois la même chose : liseré de couleur, pastille « non lu » sur la ligne, pastille « 1 non lue »
  sur le bandeau.
- Dans un dépôt : cinq gros boutons pleins sur trois rangées (Fetch, Pull fusion, Pull rebase, Push, Push forcé),
  des fichiers « M crates/…/view.rs — modifié » où le chemin entier écrase le nom.
- Les bandeaux ont leur chevron à gauche, puis un émoji, puis le titre : trois signes avant le texte.

## 2. Icônes

- **Material Symbols Rounded** (Google, Apache 2.0), le jeu actuel d'Android, en contour (poids 400), plein pour
  l'onglet actif du bas **(choix)** : tracés officiels (dépôt `google/material-design-icons`, `symbols/web`), rangés
  dans `ui/icons.slint`, produit par `tools/icones.py` (la liste des noms y est ; relancer le script pour en ajouter).
- Le PC continue d'envoyer des émojis (protocole inchangé, PC et téléphone d'âges différents compatibles) : le
  téléphone les ramène à une icône (`view::icon_for`) partout — lignes, bandeaux, étiquettes, boutons, menus, barre
  du haut. Un émoji sans icône reste dessiné tel quel ; une ou deux lettres (« M », « ? », état d'un fichier) restent
  du texte, dans une petite pastille du ton de la ligne.
- Modules : une icône par module (Fawkes `terminal`, DragonTree `account_tree`, Owlery `inbox`, Patronus
  `smart_toy`, Menagerie `dns`, Grimoire `menu_book`, Gringotts `savings`, Carnet `edit_note`) dans la barre du bas,
  sur les cartes et dans « Plus », à la place des logos PNG et des initiales **(choix ; le logo reste pour un module
  inconnu)**.

## 3. Lignes, bandeaux, boutons, onglets (tous les modules)

- Ligne : l'icône seule à gauche (24 px, sans tuile ; dans un rond teinté quand la ligne a une couleur, un groupe de
  Fawkes) ; **rien** quand la ligne n'a pas de glyphe (« • » supprimé). Plus de chevron (usage Android : toute la
  ligne se touche) ; ⋮ plus discret. Pastilles en ton doux (fond pâle, texte du ton) au lieu de pleines.
- `Row::unread` (nouveau, `#[serde(default)]`) : titre en gras et point de couleur — la façon des messageries —
  au lieu de la pastille « non lu ».
- Liseré de ton : seulement pour une ligne sans pastille (la pastille dit déjà l'état).
- Bandeau : couleur de groupe, icône, titre, compte ; chevron de pliage à droite (`expand_more`).
- Boutons : 40 px, icône Material et libellé, tonal (le principal plein).
- Onglets d'écran : onglets Material (indicateur souligné, largeur égale quand ils tiennent) ; **glisser de côté
  change d'onglet** (l'écran voisin est montré tout de suite s'il est déjà connu, le PC le rafraîchit).

## 4. DragonTree

- Liste : plus de 🌿 ; nom du dépôt, branche et état en sous-titre.
- Dépôt, onglet État : une carte « synchronisation » (branche, en avance / en retard) avec trois boutons — Fetch,
  Pull, Push — ; Pull (rebase) et Push forcé passent dans le menu ⋮ de la carte **(choix)**. Fichiers modifiés : le
  nom en titre, le dossier en sous-titre, l'état en lettre de couleur (M, A, D, ?, R, U).
- Branches : plus de 🌿 ; la branche courante marquée d'une coche.

## 5. Owlery

- L'accueil devient des **onglets** (« PR », « Revues », « Tickets »…, une sorte par onglet, seulement celles qui ont
  des entrées), le compte des non lus en pastille d'onglet, et l'on glisse de l'un à l'autre. « Tout marquer lu »
  porte sur l'onglet affiché.
- Une entrée non lue : `Row::unread` (gras + point), plus de pastille ni de liseré.
- Détail : lignes clé/valeur sans icône.

## 6. Fawkes : la zone d'écriture

- Slint (`TextInput`, 1.18) sélectionne dès que le doigt bouge d'un pixel après l'appui, et le double toucher
  sélectionne un mot : c'est « le texte se sélectionne tout seul » (puis la frappe ou le collage remplace la
  sélection). **Au doigt, glisser déplace le curseur et ne sélectionne plus** : une sélection faite au toucher est
  ramenée au curseur aussitôt (on voit le curseur suivre le doigt). Sélectionner reste possible par « Tout
  sélectionner ».
- **Poignée de curseur** sous le curseur (comme Android), qu'on fait glisser pour l'avancer ou le reculer d'un
  caractère à la fois, sans que le doigt le cache.
- Bouton **+** à gauche du champ : « Coller ici » (au curseur), « Fichier du PC… » (le sélecteur du PC, ouvert dans
  le dossier du terminal, insère le chemin — `@chemin` dans une session Claude), « Photo ou fichier du téléphone… »
  (le sélecteur d'Android ; le fichier part au PC, qui le range dans un dossier à lui et rend son chemin, inséré
  dans la saisie), « Tout sélectionner », « Effacer ».
- Envoi d'un fichier du téléphone : seulement avec le droit « Terminaux », 20 Mo au plus, rangé par le PC dans
  `%LOCALAPPDATA%\EyrieSuite\Eyrie\Televerses\<date>\` sous un nom nettoyé (jamais dans un dossier de projet),
  journalisé ; rien n'est envoyé au terminal sans que l'utilisateur touche « Envoyer ».

## 7. Ajouts en cours de route

- **Fichier du PC…** (dans le menu + aussi, et au menu ⋮ du terminal) : une saisie `FOLDER_FILE` ouverte dans le
  dossier du terminal ; le pont liste alors les fichiers après les dossiers (droits « Agir », « Dossiers du PC » et
  « Terminaux ») ; un toucher choisit ; le chemin, relatif au dossier du terminal, va au curseur. L'action
  (`INSERT_FILE`, « @fichier-pc ») ne part jamais au PC **(choix : chemin relatif, plus court, valable pour le shell
  comme pour Claude ; pas de « @ » ajouté devant)**.
- Glyphes ramenés à une icône en plus : ● (un commit) → `commit`, ◐ (au travail) → `sync`, ▸ → chevron ; « Tout
  replier » et « Tout déplier » ont les leurs. Les textes qui portaient un émoji le disent en mots (« 1 Claude · 3 »
  dans Fawkes, « remise 14:20 » sur la carte de Gringotts).
- Pas fait **(choix)** : poignée de curseur à faire glisser (Slint ne donne pas la position d'un caractère sous le
  doigt ; la loupe en tient lieu) ; logos PNG des modules (remplacés par les icônes, gardés pour un module inconnu).
