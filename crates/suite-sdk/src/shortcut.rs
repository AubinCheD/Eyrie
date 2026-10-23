//! Raccourcis clavier : lecture (« Ctrl+Maj+P »), affichage, détection, injection, et registre
//! commun qui repère les conflits entre la suite et les modules.

use std::fmt;

use eframe::egui::{self, Event, Key, Modifiers};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Shortcut {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub key: Key,
}

/// Noms français des touches dont le nom egui est anglais.
const KEY_NAMES: &[(Key, &str)] = &[
    (Key::Enter, "Entrée"),
    (Key::Escape, "Échap"),
    (Key::Space, "Espace"),
    (Key::Tab, "Tab"),
    (Key::Delete, "Suppr"),
    (Key::Backspace, "Retour"),
    (Key::ArrowLeft, "←"),
    (Key::ArrowRight, "→"),
    (Key::ArrowUp, "↑"),
    (Key::ArrowDown, "↓"),
    (Key::PageUp, "Page↑"),
    (Key::PageDown, "Page↓"),
    (Key::Home, "Début"),
    (Key::End, "Fin"),
    (Key::Comma, ","),
    (Key::Period, "."),
    (Key::Plus, "+"),
    (Key::Minus, "-"),
    (Key::Equals, "="),
];

impl Shortcut {
    pub const fn new(ctrl: bool, shift: bool, alt: bool, key: Key) -> Self {
        Shortcut { ctrl, shift, alt, key }
    }

    pub const fn key(key: Key) -> Self {
        Shortcut::new(false, false, false, key)
    }

    pub const fn ctrl(key: Key) -> Self {
        Shortcut::new(true, false, false, key)
    }

    pub const fn ctrl_shift(key: Key) -> Self {
        Shortcut::new(true, true, false, key)
    }

    pub const fn alt(key: Key) -> Self {
        Shortcut::new(false, false, true, key)
    }

    /// « Ctrl+Maj+P », « Alt+1 », « F5 » (Maj, Shift, Ctrl, Alt, sans casse).
    pub fn parse(s: &str) -> Option<Shortcut> {
        let mut sc = Shortcut::key(Key::A);
        let parts: Vec<&str> = s.split('+').map(str::trim).collect();
        // « Ctrl++ » : la touche plus.
        let (mods, key) = match parts.as_slice() {
            [rest @ .., "", ""] => (rest, "+"),
            [rest @ .., key] => (rest, *key),
            [] => return None,
        };
        for m in mods {
            match m.to_lowercase().as_str() {
                "ctrl" => sc.ctrl = true,
                "maj" | "shift" => sc.shift = true,
                "alt" => sc.alt = true,
                _ => return None,
            }
        }
        sc.key = KEY_NAMES
            .iter()
            .find(|(_, n)| n.eq_ignore_ascii_case(key))
            .map(|(k, _)| *k)
            .or_else(|| Key::from_name(key))
            .or_else(|| Key::from_name(&key.to_uppercase()))?;
        Some(sc)
    }

    pub fn modifiers(&self) -> Modifiers {
        Modifiers { alt: self.alt, ctrl: self.ctrl, shift: self.shift, mac_cmd: false, command: self.ctrl }
    }

    /// L'événement est-il cette combinaison, enfoncée ?
    pub fn matches(&self, event: &Event) -> bool {
        matches!(event, Event::Key { key, pressed: true, modifiers, .. }
            if *key == self.key
                && modifiers.ctrl == self.ctrl
                && modifiers.shift == self.shift
                && modifiers.alt == self.alt)
    }

    /// Retire la combinaison de l'entrée si elle y est ; `true` si elle a été pressée.
    pub fn consume(&self, input: &mut egui::InputState) -> bool {
        let before = input.events.len();
        input.events.retain(|e| !self.matches(e));
        input.events.len() != before
    }

    /// La frappe complète (enfoncée puis relâchée), pour exécuter une commande d'un module
    /// embarqué depuis la palette de la suite.
    pub fn to_events(&self) -> [Event; 2] {
        let ev = |pressed| Event::Key {
            key: self.key,
            physical_key: Some(self.key),
            pressed,
            repeat: false,
            modifiers: self.modifiers(),
        };
        [ev(true), ev(false)]
    }
}

impl fmt::Display for Shortcut {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.ctrl {
            f.write_str("Ctrl+")?;
        }
        if self.alt {
            f.write_str("Alt+")?;
        }
        if self.shift {
            f.write_str("Maj+")?;
        }
        match KEY_NAMES.iter().find(|(k, _)| *k == self.key) {
            Some((_, n)) => f.write_str(n),
            None => f.write_str(self.key.name()),
        }
    }
}

/// À qui appartient un raccourci.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Owner {
    Suite,
    Module(String),
}

#[derive(Clone, Debug)]
pub struct Binding {
    pub owner: Owner,
    pub command: String,
    pub title: String,
    pub shortcut: Shortcut,
}

/// Un conflit : la même combinaison pour plusieurs propriétaires.
#[derive(Clone, Debug, PartialEq)]
pub struct Conflict {
    pub shortcut: Shortcut,
    pub owners: Vec<(Owner, String)>,
    /// Un raccourci de la suite est pris par un module : la suite gagne toujours.
    pub suite_wins: bool,
}

/// Les raccourcis de la suite et des modules.
#[derive(Clone, Debug, Default)]
pub struct Registry {
    pub bindings: Vec<Binding>,
}

impl Registry {
    pub fn add(&mut self, owner: Owner, command: impl Into<String>, title: impl Into<String>, shortcut: Shortcut) {
        self.bindings.push(Binding { owner, command: command.into(), title: title.into(), shortcut });
    }

    /// Les combinaisons revendiquées par plusieurs propriétaires. Deux modules différents ne sont
    /// pas en conflit entre eux tant que l'un seulement a le clavier, mais on les montre : la même
    /// combinaison fera deux choses différentes selon la case active.
    pub fn conflicts(&self) -> Vec<Conflict> {
        let mut by_key: Vec<(Shortcut, Vec<(Owner, String)>)> = Vec::new();
        for b in &self.bindings {
            match by_key.iter_mut().find(|(s, _)| *s == b.shortcut) {
                Some((_, owners)) => {
                    if !owners.iter().any(|(o, _)| *o == b.owner) {
                        owners.push((b.owner.clone(), b.title.clone()));
                    }
                }
                None => by_key.push((b.shortcut, vec![(b.owner.clone(), b.title.clone())])),
            }
        }
        by_key
            .into_iter()
            .filter(|(_, owners)| owners.len() > 1)
            .map(|(shortcut, owners)| {
                let suite_wins = owners.iter().any(|(o, _)| *o == Owner::Suite);
                Conflict { shortcut, owners, suite_wins }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_and_display_round_trip() {
        for s in ["Ctrl+Maj+P", "F1", "Alt+1", "Ctrl+,", "Ctrl+F6", "Alt+Maj+Entrée", "Ctrl++"] {
            let sc = Shortcut::parse(s).unwrap_or_else(|| panic!("{s}"));
            assert_eq!(Shortcut::parse(&sc.to_string()), Some(sc), "{s} → {sc}");
        }
        assert_eq!(Shortcut::parse("ctrl+shift+p"), Shortcut::parse("Ctrl+Maj+P"));
        assert_eq!(Shortcut::parse("Hyper+P"), None);
    }

    #[test]
    fn matches_exact_modifiers() {
        let sc = Shortcut::ctrl(Key::P);
        let [down, up] = sc.to_events();
        assert!(sc.matches(&down));
        assert!(!sc.matches(&up));
        assert!(!Shortcut::ctrl_shift(Key::P).matches(&down));
    }

    #[test]
    fn conflicts_are_reported() {
        let mut r = Registry::default();
        r.add(Owner::Suite, "palette", "Palette", Shortcut::key(Key::F1));
        r.add(Owner::Module("fawkes".into()), "settings", "Réglages", Shortcut::ctrl(Key::Comma));
        r.add(Owner::Module("owlery".into()), "settings", "Réglages", Shortcut::ctrl(Key::Comma));
        r.add(Owner::Module("grimoire".into()), "help", "Aide", Shortcut::key(Key::F1));
        let c = r.conflicts();
        assert_eq!(c.len(), 2);
        assert!(c.iter().any(|c| c.shortcut == Shortcut::key(Key::F1) && c.suite_wins));
        assert!(c.iter().any(|c| c.shortcut == Shortcut::ctrl(Key::Comma) && !c.suite_wins));
    }
}
