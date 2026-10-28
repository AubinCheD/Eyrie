//! L'écran générique : ce qu'un module montre sur le téléphone, décrit par le PC et dessiné par l'application avec
//! les mêmes composants pour tous les modules.
//!
//! Un écran a un chemin dans son module (`""` : l'accueil du module, `t/12` : le terminal 12…). Une ligne ouvre un
//! sous-écran (son chemin) ou porte des actions ; une action peut demander une confirmation ou une saisie. Le PC
//! n'accepte une action que s'il l'a proposée dans le dernier écran envoyé ([`Screen::action_ids`]).
//!
//! **Compatibilité** : tout champ ajouté après la première version est `#[serde(default)]` (une application plus
//! ancienne l'ignore, un PC plus ancien ne l'envoie pas) ; on n'ajoute pas de variante à [`Block`] : une application
//! plus ancienne ne saurait pas la lire et perdrait la connexion.
//!
//! Les chemins d'écran sont **absolus dans le module** : un sous-écran d'un dépôt de DragonTree est
//! `depot/<clé>/historique`, jamais `historique` (le téléphone demande au PC le chemin tel quel).

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

/// Ton d'une ligne, d'une note, d'une jauge, d'une bulle.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Tone {
    #[default]
    Info,
    Ok,
    Warn,
    Danger,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ActStyle {
    #[default]
    Normal,
    Primary,
    Danger,
}

/// Ce qu'une saisie « dossier » attend.
pub const FOLDER_ANY: u8 = 0;
/// … un dépôt git existant.
pub const FOLDER_REPO: u8 = 1;
/// … le dossier où créer `child` (un clone) : `dossier\child` doit être absent ou vide.
pub const FOLDER_PARENT: u8 = 2;

/// Un raccourci du sélecteur de dossiers (« Dossier de clonage », un dossier récent du module).
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Place {
    pub label: String,
    /// Chemin Windows (affiché ; le PC le revérifie avant de le lister).
    pub path: String,
    #[serde(default)]
    pub glyph: String,
}

/// Une saisie choisie dans les dossiers du PC, par le sélecteur du téléphone (le PC liste, le téléphone montre).
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FolderInput {
    /// [`FOLDER_ANY`], [`FOLDER_REPO`] ou [`FOLDER_PARENT`].
    #[serde(default)]
    pub want: u8,
    /// Dossier où s'ouvre le sélecteur ; vide : la racine (raccourcis et lecteurs).
    #[serde(default)]
    pub start: String,
    /// Pour [`FOLDER_PARENT`] : le nom du dossier qui sera créé dedans.
    #[serde(default)]
    pub child: String,
    /// Raccourcis du module, montrés à la racine.
    #[serde(default)]
    pub places: Vec<Place>,
    /// Dossiers que le module connaît déjà (marqués « déjà là »).
    #[serde(default)]
    pub known: Vec<String>,
}

/// Une saisie demandée avant d'envoyer l'action.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Input {
    pub label: String,
    #[serde(default)]
    pub placeholder: String,
    /// Valeur proposée.
    #[serde(default)]
    pub value: String,
    #[serde(default)]
    pub multiline: bool,
    /// La saisie est un dossier du PC, choisi dans le sélecteur ; une application plus ancienne montre un champ de
    /// texte, et le PC contrôle le chemin de la même façon.
    #[serde(default)]
    pub folder: Option<FolderInput>,
}

/// Une action : un bouton, l'appui sur une ligne, une entrée du menu d'une ligne.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Act {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub glyph: String,
    #[serde(default)]
    pub style: ActStyle,
    /// Question posée avant d'agir (« Tout arrêter ? »).
    #[serde(default)]
    pub confirm: Option<String>,
    #[serde(default)]
    pub input: Option<Input>,
    /// Action propre au téléphone : ouvrir ce lien (rien n'est envoyé au PC).
    #[serde(default)]
    pub url: Option<String>,
    /// Action propre au téléphone : ouvrir ce sous-écran du module (rien n'est envoyé au PC).
    #[serde(default)]
    pub open: Option<String>,
    /// Le module fait déjà ce que fait cette action (son opération tourne) : le bouton est grisé, avec un indicateur.
    #[serde(default)]
    pub busy: bool,
}

impl Act {
    pub fn new(id: impl Into<String>, label: impl Into<String>) -> Self {
        Act { id: id.into(), label: label.into(), ..Default::default() }
    }

    /// Ouvre `url` sur le téléphone.
    pub fn link(label: impl Into<String>, url: impl Into<String>) -> Self {
        Act { label: label.into(), url: Some(url.into()), glyph: "↗".into(), ..Default::default() }
    }

    /// Ouvre le sous-écran `path` du module (depuis un menu, un bandeau).
    pub fn screen(label: impl Into<String>, path: impl Into<String>) -> Self {
        Act { label: label.into(), open: Some(path.into()), ..Default::default() }
    }

    /// Ouvre l'écran `path` (vide : l'accueil) d'un autre module, sur le téléphone : rien n'est envoyé au PC.
    pub fn module(label: impl Into<String>, module: &str, path: &str) -> Self {
        let open = if path.is_empty() {
            format!("{}{module}", crate::msg::MODULE_PREFIX)
        } else {
            format!("{}{module} {path}", crate::msg::MODULE_PREFIX)
        };
        Act { label: label.into(), open: Some(open), ..Default::default() }
    }

    pub fn glyph(mut self, glyph: impl Into<String>) -> Self {
        self.glyph = glyph.into();
        self
    }

    pub fn primary(mut self) -> Self {
        self.style = ActStyle::Primary;
        self
    }

    pub fn danger(mut self) -> Self {
        self.style = ActStyle::Danger;
        self
    }

    pub fn confirm(mut self, question: impl Into<String>) -> Self {
        self.confirm = Some(question.into());
        self
    }

    pub fn input(mut self, label: impl Into<String>, placeholder: impl Into<String>) -> Self {
        self.input = Some(Input { label: label.into(), placeholder: placeholder.into(), ..Default::default() });
        self
    }

    pub fn input_value(mut self, value: impl Into<String>) -> Self {
        self.input.get_or_insert_with(Input::default).value = value.into();
        self
    }

    pub fn multiline(mut self) -> Self {
        self.input.get_or_insert_with(Input::default).multiline = true;
        self
    }

    /// La saisie est un dossier du PC (sélecteur) ; `label` titre le sélecteur.
    pub fn folder(mut self, label: impl Into<String>, spec: FolderInput) -> Self {
        let input = self.input.get_or_insert_with(Input::default);
        input.label = label.into();
        input.folder = Some(spec);
        self
    }

    pub fn busy(mut self, yes: bool) -> Self {
        self.busy = yes;
        self
    }

    /// Envoyée au PC (et non lien ou écran local).
    pub fn reaches_pc(&self) -> bool {
        self.url.is_none() && self.open.is_none() && !self.id.is_empty()
    }
}

/// Partie d'un trait du graphe des commits dans une ligne : la moitié haute (du haut de la ligne au milieu), la
/// moitié basse (du milieu au bas), ou toute la hauteur.
pub const GRAPH_TOP: u8 = 0;
pub const GRAPH_BOTTOM: u8 = 1;
pub const GRAPH_THROUGH: u8 = 2;

/// Sortes de nœud.
pub const NODE_COMMIT: u8 = 0;
pub const NODE_MERGE: u8 = 1;
/// Le commit de HEAD (anneau).
pub const NODE_HEAD: u8 = 2;
/// Un commit qui n'est plus atteint par aucune référence (atténué).
pub const NODE_LOST: u8 = 3;
/// Une remise (stash).
pub const NODE_STASH: u8 = 4;
/// Pas de nœud dans cette ligne.
pub const NODE_NONE: u8 = 255;

/// Couleurs des voies : un indice dans une palette de cette taille, résolue par le téléphone dans son thème.
pub const LANE_COLORS: u8 = 10;

/// La part du graphe des commits dessinée dans une ligne. Les traits de lignes voisines se raccordent : un lien d'un
/// enfant (ligne `e`, voie `a`) vers son parent (ligne `p`, voie `c`) en passant par la voie `b` donne
/// `(GRAPH_BOTTOM, a, b)` à la ligne `e`, `(GRAPH_THROUGH, b, b)` aux lignes entre les deux, et `(GRAPH_TOP, b, c)` à
/// la ligne `p`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct GraphCell {
    /// Nombre de voies dessinées (largeur de la colonne).
    pub lanes: u8,
    /// Voie du nœud ([`NODE_NONE`] : pas de nœud).
    pub node: u8,
    /// Couleur du nœud (indice, voir [`LANE_COLORS`]).
    pub color: u8,
    /// [`NODE_COMMIT`], [`NODE_MERGE`], [`NODE_HEAD`]…
    pub kind: u8,
    /// Traits : (partie, voie de départ, voie d'arrivée, couleur).
    #[serde(default)]
    pub lines: Vec<(u8, u8, u8, u8)>,
}

/// Une étiquette dans une ligne : une branche, une étiquette git, HEAD.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Chip {
    pub text: String,
    #[serde(default)]
    pub glyph: String,
    /// 0xRRGGBB.
    #[serde(default)]
    pub color: Option<u32>,
    #[serde(default)]
    pub tone: Option<Tone>,
    /// Mise en avant (HEAD, la branche courante).
    #[serde(default)]
    pub strong: bool,
}

impl Chip {
    pub fn new(text: impl Into<String>) -> Self {
        Chip { text: text.into(), ..Default::default() }
    }

    pub fn glyph(mut self, s: impl Into<String>) -> Self {
        self.glyph = s.into();
        self
    }

    pub fn color(mut self, rgb: u32) -> Self {
        self.color = Some(rgb);
        self
    }

    pub fn tone(mut self, tone: Tone) -> Self {
        self.tone = Some(tone);
        self
    }

    pub fn strong(mut self) -> Self {
        self.strong = true;
        self
    }
}

/// Progression inconnue (barre animée) : valeur de `progress` hors de 0..=100.
pub const PROGRESS_UNKNOWN: u8 = 255;

/// Une ligne de liste.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Row {
    pub title: String,
    #[serde(default)]
    pub subtitle: String,
    /// Texte court à droite (« 3 min », « 42 % »).
    #[serde(default)]
    pub detail: String,
    /// Glyphe à gauche.
    #[serde(default)]
    pub glyph: String,
    /// Pastille de couleur à gauche (0xRRGGBB) : couleur d'un groupe, d'un dépôt.
    #[serde(default)]
    pub color: Option<u32>,
    #[serde(default)]
    pub tone: Option<Tone>,
    /// Petite étiquette (« attend », « 3 »).
    #[serde(default)]
    pub badge: String,
    /// Sous-écran ouvert au toucher.
    #[serde(default)]
    pub open: Option<String>,
    /// Action au toucher, quand la ligne n'ouvre rien.
    #[serde(default)]
    pub tap: Option<Act>,
    /// Actions du menu de la ligne (appui long, ou bouton ⋯).
    #[serde(default)]
    pub acts: Vec<Act>,
    /// Identifiant stable de la ligne dans l'écran (un terminal, un dépôt, un commit) : le téléphone met la ligne à
    /// jour sur place au lieu de la recréer. Vide : le titre et le glyphe en tiennent lieu.
    #[serde(default)]
    pub key: String,
    /// Bouton visible à droite, en plus du menu (« Lancer », « Basculer »).
    #[serde(default)]
    pub trailing: Option<Act>,
    /// Étiquettes sous le titre (branches et étiquettes d'un commit).
    #[serde(default)]
    pub chips: Vec<Chip>,
    /// Le graphe des commits : la ligne devient une ligne de commit, dense, avec sa colonne de graphe.
    #[serde(default)]
    pub graph: Option<GraphCell>,
    /// Barre de progression sous la ligne, en pour cent ([`PROGRESS_UNKNOWN`] : indéterminée).
    #[serde(default)]
    pub progress: Option<u8>,
}

impl Row {
    pub fn new(title: impl Into<String>) -> Self {
        Row { title: title.into(), ..Default::default() }
    }

    pub fn subtitle(mut self, s: impl Into<String>) -> Self {
        self.subtitle = s.into();
        self
    }

    pub fn detail(mut self, s: impl Into<String>) -> Self {
        self.detail = s.into();
        self
    }

    pub fn glyph(mut self, s: impl Into<String>) -> Self {
        self.glyph = s.into();
        self
    }

    pub fn color(mut self, rgb: u32) -> Self {
        self.color = Some(rgb);
        self
    }

    pub fn tone(mut self, tone: Tone) -> Self {
        self.tone = Some(tone);
        self
    }

    pub fn badge(mut self, s: impl Into<String>) -> Self {
        self.badge = s.into();
        self
    }

    pub fn open(mut self, path: impl Into<String>) -> Self {
        self.open = Some(path.into());
        self
    }

    pub fn tap(mut self, act: Act) -> Self {
        self.tap = Some(act);
        self
    }

    pub fn act(mut self, act: Act) -> Self {
        self.acts.push(act);
        self
    }

    pub fn key(mut self, key: impl Into<String>) -> Self {
        self.key = key.into();
        self
    }

    pub fn trailing(mut self, act: Act) -> Self {
        self.trailing = Some(act);
        self
    }

    pub fn chip(mut self, chip: Chip) -> Self {
        self.chips.push(chip);
        self
    }

    pub fn graph(mut self, cell: GraphCell) -> Self {
        self.graph = Some(cell);
        self
    }

    /// Progression en pour cent ; `None` : indéterminée.
    pub fn progress(mut self, percent: Option<u8>) -> Self {
        self.progress = Some(percent.map_or(PROGRESS_UNKNOWN, |p| p.min(100)));
        self
    }
}

/// Pliage d'un bandeau : l'état de départ que propose le PC. Une fois que l'utilisateur l'a plié ou déplié, le
/// téléphone retient son choix (rien n'est envoyé au PC).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Fold {
    /// Simple titre, qui ne se plie pas.
    #[default]
    No,
    Open,
    Closed,
}

/// Un bandeau : un titre de section, pliable s'il a une clé et un pliage.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Band {
    pub key: String,
    pub title: String,
    pub aside: String,
    pub fold: Fold,
    pub color: Option<u32>,
    pub glyph: String,
    pub badge: String,
    pub tone: Option<Tone>,
    pub acts: Vec<Act>,
}

impl Band {
    /// Un bandeau pliable, ouvert au départ ; `key` doit être stable dans l'écran (l'identifiant d'un groupe).
    pub fn new(key: impl Into<String>, title: impl Into<String>) -> Self {
        Band { key: key.into(), title: title.into(), fold: Fold::Open, ..Default::default() }
    }

    pub fn aside(mut self, s: impl Into<String>) -> Self {
        self.aside = s.into();
        self
    }

    /// Ouvert ou fermé au départ.
    pub fn open(mut self, open: bool) -> Self {
        self.fold = if open { Fold::Open } else { Fold::Closed };
        self
    }

    /// Ne se plie pas.
    pub fn fixed(mut self) -> Self {
        self.fold = Fold::No;
        self
    }

    pub fn color(mut self, rgb: u32) -> Self {
        self.color = Some(rgb);
        self
    }

    pub fn glyph(mut self, s: impl Into<String>) -> Self {
        self.glyph = s.into();
        self
    }

    pub fn badge(mut self, s: impl Into<String>) -> Self {
        self.badge = s.into();
        self
    }

    pub fn tone(mut self, tone: Tone) -> Self {
        self.tone = Some(tone);
        self
    }

    pub fn act(mut self, act: Act) -> Self {
        self.acts.push(act);
        self
    }
}

/// Un bloc d'écran.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Block {
    /// Titre de section ; `aside` : texte court à droite (« 3 »). Avec une `key` et un `fold`, c'est un bandeau
    /// pliable : il cache les blocs qui le suivent jusqu'à la section suivante.
    Section {
        title: String,
        aside: String,
        #[serde(default)]
        key: String,
        #[serde(default)]
        fold: Fold,
        /// Pastille de couleur (un groupe de Fawkes).
        #[serde(default)]
        color: Option<u32>,
        #[serde(default)]
        glyph: String,
        /// Visible même plié (« 2 attendent »).
        #[serde(default)]
        badge: String,
        #[serde(default)]
        tone: Option<Tone>,
        /// Menu du bandeau (bouton ⋯).
        #[serde(default)]
        acts: Vec<Act>,
    },
    Row(Row),
    /// Bandeau d'information ou d'alerte ; avec `progress`, une opération en cours (et `acts` : l'annuler).
    Note {
        text: String,
        tone: Tone,
        /// Pour cent ([`PROGRESS_UNKNOWN`] : indéterminée).
        #[serde(default)]
        progress: Option<u8>,
        #[serde(default)]
        acts: Vec<Act>,
    },
    /// Jauge de 0 à 1.
    Meter {
        label: String,
        value: f32,
        text: String,
        tone: Tone,
    },
    /// Texte libre ; `mono` : journal, code.
    Text {
        text: String,
        mono: bool,
    },
    /// Liste vide.
    Empty {
        text: String,
    },
    Buttons {
        acts: Vec<Act>,
    },
    /// Un diff : une ligne par entrée, avec sa sorte (`DIFF_*`).
    Diff {
        lines: Vec<(u8, String)>,
        /// Numéros de ligne (ancien, nouveau ; 0 : sans), un par ligne, ou vide.
        #[serde(default)]
        numbers: Vec<(u32, u32)>,
    },
}

/// Sortes de ligne d'un [`Block::Diff`].
pub const DIFF_CONTEXT: u8 = 0;
pub const DIFF_ADDED: u8 = 1;
pub const DIFF_REMOVED: u8 = 2;
/// En-tête (`@@ … @@`, « Indexé »…).
pub const DIFF_HEADER: u8 = 3;

/// Un onglet d'écran : des écrans voisins du même module (« État », « Historique », « Branches »), entre lesquels on
/// passe sans empiler.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Tab {
    pub label: String,
    /// Chemin de l'écran (absolu dans le module) ; l'onglet actif est celui dont le chemin est celui de l'écran.
    pub path: String,
    #[serde(default)]
    pub badge: String,
}

/// Un écran.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Screen {
    pub title: String,
    #[serde(default)]
    pub subtitle: String,
    /// Actions de la barre du haut.
    #[serde(default)]
    pub acts: Vec<Act>,
    #[serde(default)]
    pub blocks: Vec<Block>,
    /// L'écran est ce terminal, en direct (son identifiant dans le module).
    #[serde(default)]
    pub terminal: Option<String>,
    /// Onglets sous le titre.
    #[serde(default)]
    pub tabs: Vec<Tab>,
    /// Un champ de recherche filtre les lignes sur le téléphone (titre, sous-titre, texte à droite, étiquettes) ;
    /// c'est son texte d'invite, vide : pas de champ. Rien n'est envoyé au PC.
    #[serde(default)]
    pub search: String,
}

impl Screen {
    pub fn new(title: impl Into<String>) -> Self {
        Screen { title: title.into(), ..Default::default() }
    }

    pub fn subtitle(mut self, s: impl Into<String>) -> Self {
        self.subtitle = s.into();
        self
    }

    pub fn act(mut self, act: Act) -> Self {
        self.acts.push(act);
        self
    }

    pub fn terminal(mut self, id: impl Into<String>) -> Self {
        self.terminal = Some(id.into());
        self
    }

    pub fn tab(mut self, label: impl Into<String>, path: impl Into<String>) -> Self {
        self.tabs.push(Tab { label: label.into(), path: path.into(), badge: String::new() });
        self
    }

    pub fn search(mut self, placeholder: impl Into<String>) -> Self {
        self.search = placeholder.into();
        self
    }

    pub fn section(&mut self, title: impl Into<String>, aside: impl Into<String>) -> &mut Self {
        self.blocks.push(Block::Section {
            title: title.into(),
            aside: aside.into(),
            key: String::new(),
            fold: Fold::No,
            color: None,
            glyph: String::new(),
            badge: String::new(),
            tone: None,
            acts: Vec::new(),
        });
        self
    }

    /// Un bandeau (pliable s'il a une clé et un pliage).
    pub fn band(&mut self, b: Band) -> &mut Self {
        let fold = if b.key.is_empty() { Fold::No } else { b.fold };
        self.blocks.push(Block::Section {
            title: b.title,
            aside: b.aside,
            key: b.key,
            fold,
            color: b.color,
            glyph: b.glyph,
            badge: b.badge,
            tone: b.tone,
            acts: b.acts,
        });
        self
    }

    pub fn row(&mut self, row: Row) -> &mut Self {
        self.blocks.push(Block::Row(row));
        self
    }

    pub fn note(&mut self, text: impl Into<String>, tone: Tone) -> &mut Self {
        self.blocks.push(Block::Note { text: text.into(), tone, progress: None, acts: Vec::new() });
        self
    }

    /// Une opération en cours : `percent` (`None` : indéterminée), et `cancel` pour l'arrêter.
    pub fn progress(&mut self, text: impl Into<String>, percent: Option<u8>, cancel: Option<Act>) -> &mut Self {
        self.blocks.push(Block::Note {
            text: text.into(),
            tone: Tone::Info,
            progress: Some(percent.map_or(PROGRESS_UNKNOWN, |p| p.min(100))),
            acts: cancel.into_iter().collect(),
        });
        self
    }

    pub fn meter(&mut self, label: impl Into<String>, value: f32, text: impl Into<String>, tone: Tone) -> &mut Self {
        let value = if value.is_finite() { value.clamp(0.0, 1.0) } else { 0.0 };
        self.blocks.push(Block::Meter { label: label.into(), value, text: text.into(), tone });
        self
    }

    pub fn text(&mut self, text: impl Into<String>, mono: bool) -> &mut Self {
        self.blocks.push(Block::Text { text: text.into(), mono });
        self
    }

    pub fn empty(&mut self, text: impl Into<String>) -> &mut Self {
        self.blocks.push(Block::Empty { text: text.into() });
        self
    }

    pub fn diff(&mut self, lines: Vec<(u8, String)>) -> &mut Self {
        self.blocks.push(Block::Diff { lines, numbers: Vec::new() });
        self
    }

    /// Un diff avec ses numéros de ligne (un par ligne).
    pub fn diff_numbered(&mut self, lines: Vec<(u8, String)>, numbers: Vec<(u32, u32)>) -> &mut Self {
        let numbers = if numbers.len() == lines.len() { numbers } else { Vec::new() };
        self.blocks.push(Block::Diff { lines, numbers });
        self
    }

    pub fn buttons(&mut self, acts: Vec<Act>) -> &mut Self {
        self.blocks.push(Block::Buttons { acts });
        self
    }

    /// Toutes les actions de l'écran (barre, bandeaux, lignes, notes, boutons).
    pub fn all_acts(&self) -> Vec<&Act> {
        let mut out: Vec<&Act> = self.acts.iter().collect();
        for b in &self.blocks {
            match b {
                Block::Section { acts, .. } | Block::Note { acts, .. } | Block::Buttons { acts } => {
                    out.extend(acts.iter())
                }
                Block::Row(r) => {
                    out.extend(r.tap.iter());
                    out.extend(r.trailing.iter());
                    out.extend(r.acts.iter());
                }
                _ => {}
            }
        }
        out
    }

    /// Toutes les actions envoyables au PC que cet écran propose : la liste blanche du pont.
    pub fn action_ids(&self) -> BTreeSet<String> {
        self.all_acts().into_iter().filter(|a| a.reaches_pc()).map(|a| a.id.clone()).collect()
    }

    /// Les sous-écrans que cet écran ouvre (lignes, onglets, actions d'écran).
    pub fn opens(&self) -> BTreeSet<String> {
        let rows = self.blocks.iter().filter_map(|b| match b {
            Block::Row(r) => r.open.clone(),
            _ => None,
        });
        let tabs = self.tabs.iter().map(|t| t.path.clone());
        let acts = self.all_acts().into_iter().filter_map(|a| a.open.clone());
        rows.chain(tabs).chain(acts).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_whitelist_lists_every_action_sent_to_the_pc_and_no_link() {
        let mut s = Screen::new("Patronus").act(Act::new("pause", "Pause"));
        s.section("Tâches", "2")
            .row(Row::new("build").tap(Act::new("lancer:build", "Lancer")).act(Act::new("arreter:build", "Arrêter")))
            .row(Row::new("PR 12").act(Act::link("Ouvrir", "https://exemple.org")).open("pr/12"))
            .buttons(vec![Act::new("tout_arreter", "Tout arrêter").danger()]);
        let ids: Vec<String> = s.action_ids().into_iter().collect();
        assert_eq!(ids, ["arreter:build", "lancer:build", "pause", "tout_arreter"]);
        assert_eq!(s.opens().into_iter().collect::<Vec<_>>(), ["pr/12"]);
    }

    /// Les actions des bandeaux, des notes et les boutons visibles des lignes entrent dans la liste blanche ; une
    /// action qui ouvre un écran du téléphone n'y entre pas, mais son écran compte parmi ceux que l'écran ouvre.
    #[test]
    fn bands_notes_and_trailing_buttons_are_whitelisted_and_local_screens_are_not() {
        let mut s = Screen::new("Fawkes").tab("Terminaux", "").tab("Récents", "recents");
        s.band(
            Band::new("g1", "Ozmo")
                .act(Act::new("veille:g1", "Tout mettre en veille"))
                .act(Act::screen("Nouveau terminal ici…", "nouveau/g/1")),
        )
        .row(Row::new("pwsh").trailing(Act::new("lancer:1", "Lancer")))
        .progress("Fetch en cours…", Some(40), Some(Act::new("stopper", "Annuler")));
        let ids: Vec<String> = s.action_ids().into_iter().collect();
        assert_eq!(ids, ["lancer:1", "stopper", "veille:g1"]);
        assert_eq!(s.opens().into_iter().collect::<Vec<_>>(), ["", "nouveau/g/1", "recents"]);
    }

    #[test]
    fn a_band_without_a_key_does_not_fold() {
        let mut s = Screen::new("x");
        s.band(Band::new("", "Titre").open(false)).band(Band::new("k", "Pliée").open(false));
        let folds: Vec<Fold> = s
            .blocks
            .iter()
            .filter_map(|b| match b {
                Block::Section { fold, .. } => Some(*fold),
                _ => None,
            })
            .collect();
        assert_eq!(folds, [Fold::No, Fold::Closed]);
    }

    #[test]
    fn a_meter_stays_between_zero_and_one() {
        let mut s = Screen::new("x");
        s.meter("5 h", 1.7, "170 %", Tone::Danger).meter("7 j", f32::NAN, "?", Tone::Info);
        let values: Vec<f32> = s
            .blocks
            .iter()
            .filter_map(|b| match b {
                Block::Meter { value, .. } => Some(*value),
                _ => None,
            })
            .collect();
        assert_eq!(values, [1.0, 0.0]);
    }

    /// Une version plus ancienne qui écrit un écran sans les champs ajoutés depuis se lit encore, et une plus récente
    /// qui en écrit de nouveaux aussi : MessagePack à champs nommés.
    #[test]
    fn screens_written_without_the_new_fields_still_read() {
        #[derive(Serialize)]
        struct OldRow {
            title: String,
        }
        #[derive(Serialize)]
        enum OldBlock {
            Section { title: String, aside: String },
            Row(OldRow),
            Note { text: String, tone: Tone },
        }
        #[derive(Serialize)]
        struct OldScreen {
            title: String,
            blocks: Vec<OldBlock>,
        }
        let old = OldScreen {
            title: "DragonTree".into(),
            blocks: vec![
                OldBlock::Section { title: "Dépôts".into(), aside: "2".into() },
                OldBlock::Row(OldRow { title: "Eyrie".into() }),
                OldBlock::Note { text: "Fetch…".into(), tone: Tone::Info },
            ],
        };
        let bytes = rmp_serde::to_vec_named(&old).unwrap();
        let s: Screen = rmp_serde::from_slice(&bytes).unwrap();
        assert_eq!(s.title, "DragonTree");
        assert!(matches!(&s.blocks[0], Block::Section { fold: Fold::No, key, .. } if key.is_empty()));
        assert!(matches!(&s.blocks[1], Block::Row(r) if r.title == "Eyrie" && r.graph.is_none()));
        assert!(matches!(&s.blocks[2], Block::Note { progress: None, .. }));
        assert!(s.tabs.is_empty() && s.search.is_empty());
    }

    #[test]
    fn a_commit_row_survives_messagepack() {
        let row = Row::new("Corrige le push")
            .key("abc123")
            .chip(Chip::new("main").glyph("🌿").strong())
            .graph(GraphCell {
                lanes: 2,
                node: 0,
                color: 3,
                kind: NODE_HEAD,
                lines: vec![(GRAPH_BOTTOM, 0, 1, 3), (GRAPH_THROUGH, 1, 1, 4)],
            })
            .progress(Some(140));
        let mut s = Screen::new("Historique").search("Chercher un commit");
        s.row(row.clone());
        let back: Screen = rmp_serde::from_slice(&rmp_serde::to_vec_named(&s).unwrap()).unwrap();
        assert_eq!(back, s);
        assert_eq!(row.progress, Some(100));
    }
}
