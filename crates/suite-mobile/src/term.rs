//! Un terminal suivi depuis le téléphone.
//!
//! Le PC lit la grille de l'émulateur (écran et un peu d'historique) en une [`TermView`] ; il n'envoie que la
//! différence avec la dernière vue envoyée ([`TermFrame`]) : combien de lignes sont sorties par le haut, et les lignes
//! changées. Les couleurs restent symboliques (par défaut, une des 256, RVB) : le téléphone les résout dans son thème.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use serde::{Deserialize, Serialize};

/// Couleur d'une cellule.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Color {
    #[default]
    Default,
    Indexed(u8),
    Rgb(u8, u8, u8),
}

pub const BOLD: u8 = 1;
pub const ITALIC: u8 = 2;
pub const UNDERLINE: u8 = 4;
pub const INVERSE: u8 = 8;
pub const DIM: u8 = 16;
pub const STRIKE: u8 = 32;

/// Une suite de cellules de même style.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Run {
    #[serde(rename = "t")]
    pub text: String,
    #[serde(rename = "f", default)]
    pub fg: Color,
    #[serde(rename = "b", default)]
    pub bg: Color,
    #[serde(rename = "s", default)]
    pub flags: u8,
}

/// Une ligne de la grille ; les blancs de fin sont retirés.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Line {
    #[serde(rename = "r", default)]
    pub runs: Vec<Run>,
}

impl Line {
    pub fn plain(text: impl Into<String>) -> Self {
        Line { runs: vec![Run { text: text.into(), ..Default::default() }] }
    }

    pub fn text(&self) -> String {
        self.runs.iter().map(|r| r.text.as_str()).collect()
    }

    fn key(&self) -> u64 {
        let mut h = DefaultHasher::new();
        self.hash(&mut h);
        h.finish()
    }
}

/// Touches nommées.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Key {
    Enter,
    /// Saut de ligne sans envoyer (Maj+Entrée : une nouvelle ligne dans la saisie de Claude).
    ShiftEnter,
    Escape,
    Tab,
    BackTab,
    Backspace,
    Delete,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    /// Ctrl + une lettre (`c` : interrompre).
    Ctrl(char),
}

/// Ce que le téléphone envoie à un terminal.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TermInput {
    /// Texte frappé (sans retour à la ligne : Entrée est une touche).
    Text(String),
    Key(Key),
    /// Texte collé (encadré si le programme le demande).
    Paste(String),
}

/// Une réponse proposée en bouton (une session Claude qui attend : « 1. Yes »).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct QuickReply {
    pub label: String,
    pub input: TermInput,
}

/// Ce que le PC lit d'un terminal.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TermView {
    pub cols: u16,
    pub rows: u16,
    /// L'historique gardé puis l'écran ; la dernière ligne est le bas de l'écran.
    pub lines: Vec<Line>,
    /// Curseur visible : (indice dans `lines`, colonne).
    pub cursor: Option<(u32, u16)>,
    pub title: String,
    /// Écran alternatif (éditeur, programme plein écran) : pas d'historique.
    pub alt_screen: bool,
    /// Une session Claude attend une réponse.
    pub waiting: bool,
    pub replies: Vec<QuickReply>,
    /// Le programme est terminé (terminal en veille ou fermé).
    pub ended: bool,
}

/// Une image du terminal envoyée au téléphone : la différence avec la précédente.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TermFrame {
    pub module: String,
    pub id: String,
    /// Lignes sorties par le haut depuis l'image précédente.
    pub shift: u32,
    /// Nombre de lignes de la vue.
    pub len: u32,
    /// Lignes qui changent (indice dans la nouvelle vue).
    pub changed: Vec<(u32, Line)>,
    pub cols: u16,
    pub rows: u16,
    pub cursor: Option<(u32, u16)>,
    pub title: String,
    pub alt_screen: bool,
    pub waiting: bool,
    pub replies: Vec<QuickReply>,
    pub ended: bool,
}

impl TermFrame {
    fn header(module: &str, id: &str, view: &TermView) -> Self {
        TermFrame {
            module: module.to_string(),
            id: id.to_string(),
            shift: 0,
            len: view.lines.len() as u32,
            changed: Vec::new(),
            cols: view.cols,
            rows: view.rows,
            cursor: view.cursor,
            title: view.title.clone(),
            alt_screen: view.alt_screen,
            waiting: view.waiting,
            replies: view.replies.clone(),
            ended: view.ended,
        }
    }

    /// Toute la vue (première image).
    pub fn full(module: &str, id: &str, view: &TermView) -> Self {
        let mut f = Self::header(module, id, view);
        f.changed = view.lines.iter().enumerate().map(|(i, l)| (i as u32, l.clone())).collect();
        f
    }

    /// La différence entre deux vues : le décalage qui aligne le mieux l'ancienne sur la nouvelle (la sortie qui
    /// défile), puis les lignes qui diffèrent encore.
    pub fn diff(module: &str, id: &str, old: &TermView, new: &TermView) -> Self {
        let mut f = Self::header(module, id, new);
        let o: Vec<u64> = old.lines.iter().map(Line::key).collect();
        let n: Vec<u64> = new.lines.iter().map(Line::key).collect();
        let shift = if old.alt_screen != new.alt_screen { 0 } else { best_shift(&o, &n) };
        f.shift = shift as u32;
        for (i, key) in n.iter().enumerate() {
            if o.get(i + shift) != Some(key) {
                f.changed.push((i as u32, new.lines[i].clone()));
            }
        }
        f
    }

    /// Rien n'a changé.
    pub fn is_empty_against(&self, old: &TermView) -> bool {
        self.shift == 0
            && self.changed.is_empty()
            && self.len as usize == old.lines.len()
            && self.cursor == old.cursor
            && self.cols == old.cols
            && self.rows == old.rows
            && self.title == old.title
            && self.alt_screen == old.alt_screen
            && self.waiting == old.waiting
            && self.replies == old.replies
            && self.ended == old.ended
    }

    /// Applique l'image aux lignes gardées par le téléphone.
    pub fn apply(&self, lines: &mut Vec<Line>) {
        let shift = (self.shift as usize).min(lines.len());
        lines.drain(..shift);
        lines.resize(self.len as usize, Line::default());
        for (i, l) in &self.changed {
            if let Some(slot) = lines.get_mut(*i as usize) {
                *slot = l.clone();
            }
        }
    }
}

/// Le décalage `k` tel que `old[k + i] == new[i]` pour le plus de lignes ; à égalité, le plus petit.
fn best_shift(old: &[u64], new: &[u64]) -> usize {
    let mut best = (0usize, 0usize);
    let score = |k: usize| old[k..].iter().zip(new).filter(|(a, b)| a == b).count();
    for k in 0..old.len() {
        // Ce qui reste à aligner ne peut plus battre le meilleur.
        if old.len() - k <= best.1 {
            break;
        }
        let s = score(k);
        if s > best.1 {
            best = (k, s);
        }
    }
    best.0
}

/// Les choix numérotés d'une question (« ❯ 1. Yes », « 2. No ») dans les dernières lignes : les réponses rapides
/// d'une session Claude qui attend.
pub fn numbered_choices(lines: &[Line]) -> Vec<QuickReply> {
    let mut out: Vec<QuickReply> = Vec::new();
    for line in lines.iter().rev().take(24).collect::<Vec<_>>().into_iter().rev() {
        let text = line.text();
        let t = text.trim_start_matches(|c: char| c.is_whitespace() || "❯>│|·•".contains(c));
        let digits: String = t.chars().take_while(|c| c.is_ascii_digit()).collect();
        if digits.is_empty() || digits.len() > 1 {
            continue;
        }
        let rest = &t[digits.len()..];
        let Some(label) = rest.strip_prefix('.').or_else(|| rest.strip_prefix(')')) else { continue };
        let label = label.trim().trim_end_matches('│').trim();
        if label.is_empty() {
            continue;
        }
        // Une nouvelle liste commence à « 1 » : seule la dernière compte.
        if digits == "1" {
            out.clear();
        }
        out.push(QuickReply { label: format!("{digits}. {label}"), input: TermInput::Text(digits) });
    }
    if out.len() < 2 { Vec::new() } else { out }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(lines: &[&str]) -> TermView {
        TermView { cols: 80, rows: 3, lines: lines.iter().map(|l| Line::plain(*l)).collect(), ..Default::default() }
    }

    #[test]
    fn scrolling_output_is_sent_as_a_shift_and_the_new_lines() {
        let old = view(&["a", "b", "c", "d", "e"]);
        let new = view(&["d", "e", "f", "g", "h"]);
        let f = TermFrame::diff("fawkes", "1", &old, &new);
        assert_eq!(f.shift, 3);
        assert_eq!(f.changed.iter().map(|(i, _)| *i).collect::<Vec<_>>(), [2, 3, 4]);
        let mut kept = old.lines.clone();
        f.apply(&mut kept);
        assert_eq!(kept, new.lines);
    }

    #[test]
    fn a_line_changed_in_place_is_the_only_one_sent() {
        let old = view(&["a", "b", "c"]);
        let new = view(&["a", "B", "c"]);
        let f = TermFrame::diff("fawkes", "1", &old, &new);
        assert_eq!(f.shift, 0);
        assert_eq!(f.changed, vec![(1, Line::plain("B"))]);
        let mut kept = old.lines.clone();
        f.apply(&mut kept);
        assert_eq!(kept, new.lines);
    }

    #[test]
    fn scrolling_and_changing_the_last_line_together() {
        let old = view(&["1", "2", "3", "4", "$ ma"]);
        let new = view(&["3", "4", "$ make", "ok", "$ "]);
        let f = TermFrame::diff("fawkes", "1", &old, &new);
        let mut kept = old.lines.clone();
        f.apply(&mut kept);
        assert_eq!(kept, new.lines);
        assert!(f.changed.len() <= 3, "{f:?}");
    }

    #[test]
    fn an_unchanged_view_gives_an_empty_frame() {
        let v = view(&["a", "b"]);
        assert!(TermFrame::diff("fawkes", "1", &v, &v).is_empty_against(&v));
    }

    #[test]
    fn a_growing_view_keeps_its_top() {
        let old = view(&["a", "b"]);
        let new = view(&["a", "b", "c"]);
        let f = TermFrame::diff("fawkes", "1", &old, &new);
        assert_eq!((f.shift, f.changed.len()), (0, 1));
        let mut kept = old.lines.clone();
        f.apply(&mut kept);
        assert_eq!(kept, new.lines);
    }

    #[test]
    fn claude_choices_become_quick_replies() {
        let lines = view(&[
            "Do you want to make this edit to main.rs?",
            " ❯ 1. Yes",
            "   2. Yes, allow all edits during this session (shift+tab)",
            "   3. No, and tell Claude what to do differently (esc)",
        ])
        .lines;
        let r = numbered_choices(&lines);
        assert_eq!(r.len(), 3);
        assert_eq!(r[0].label, "1. Yes");
        assert_eq!(r[2].input, TermInput::Text("3".into()));
        assert!(numbered_choices(&view(&["1. seul"]).lines).is_empty(), "un seul choix : pas une question");
    }
}
