//! L'écran générique : ce qu'un module montre sur le téléphone, décrit par le PC et dessiné par l'application avec
//! les mêmes composants pour tous les modules.
//!
//! Un écran a un chemin dans son module (`""` : l'accueil du module, `t/12` : le terminal 12…). Une ligne ouvre un
//! sous-écran (son chemin) ou porte des actions ; une action peut demander une confirmation ou une saisie. Le PC
//! n'accepte une action que s'il l'a proposée dans le dernier écran envoyé ([`Screen::action_ids`]).

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
}

impl Act {
    pub fn new(id: impl Into<String>, label: impl Into<String>) -> Self {
        Act { id: id.into(), label: label.into(), ..Default::default() }
    }

    /// Ouvre `url` sur le téléphone.
    pub fn link(label: impl Into<String>, url: impl Into<String>) -> Self {
        Act { label: label.into(), url: Some(url.into()), glyph: "↗".into(), ..Default::default() }
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

    /// Envoyée au PC (et non lien local).
    pub fn reaches_pc(&self) -> bool {
        self.url.is_none() && !self.id.is_empty()
    }
}

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
}

/// Un bloc d'écran.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Block {
    /// Titre de section ; `aside` : texte court à droite (« 3 »).
    Section {
        title: String,
        aside: String,
    },
    Row(Row),
    /// Bandeau d'information ou d'alerte.
    Note {
        text: String,
        tone: Tone,
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

    pub fn section(&mut self, title: impl Into<String>, aside: impl Into<String>) -> &mut Self {
        self.blocks.push(Block::Section { title: title.into(), aside: aside.into() });
        self
    }

    pub fn row(&mut self, row: Row) -> &mut Self {
        self.blocks.push(Block::Row(row));
        self
    }

    pub fn note(&mut self, text: impl Into<String>, tone: Tone) -> &mut Self {
        self.blocks.push(Block::Note { text: text.into(), tone });
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

    pub fn buttons(&mut self, acts: Vec<Act>) -> &mut Self {
        self.blocks.push(Block::Buttons { acts });
        self
    }

    /// Toutes les actions envoyables au PC que cet écran propose : la liste blanche du pont.
    pub fn action_ids(&self) -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        let mut add = |a: &Act| {
            if a.reaches_pc() {
                out.insert(a.id.clone());
            }
        };
        self.acts.iter().for_each(&mut add);
        for b in &self.blocks {
            match b {
                Block::Row(r) => {
                    r.tap.iter().for_each(&mut add);
                    r.acts.iter().for_each(&mut add);
                }
                Block::Buttons { acts } => acts.iter().for_each(&mut add),
                _ => {}
            }
        }
        out
    }

    /// Les sous-écrans que cet écran ouvre.
    pub fn opens(&self) -> BTreeSet<String> {
        self.blocks
            .iter()
            .filter_map(|b| match b {
                Block::Row(r) => r.open.clone(),
                _ => None,
            })
            .collect()
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
}
