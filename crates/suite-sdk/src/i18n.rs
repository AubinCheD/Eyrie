//! La langue de la suite.
//!
//! Même principe que `dragontree-i18n` : le texte français du code sert de clé, chaque catalogue est
//! un objet JSON plat « français → traduction ». Mais un catalogue **par module** (un domaine), au
//! lieu d'un seul `en.json` pour tout le processus, et une langue globale commune.

use std::collections::HashMap;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU8, Ordering};

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Lang {
    #[default]
    Fr,
    En,
}

impl Lang {
    pub const ALL: [Lang; 2] = [Lang::Fr, Lang::En];

    pub fn label(self) -> &'static str {
        match self {
            Lang::Fr => "Français",
            Lang::En => "English",
        }
    }

    /// La langue de l'interface de Windows, français à défaut d'anglais reconnu.
    pub fn system() -> Lang {
        #[cfg(windows)]
        {
            // LANGID : les 10 bits de poids faible donnent la langue primaire (0x09 anglais).
            let id = unsafe { windows_sys::Win32::Globalization::GetUserDefaultUILanguage() };
            if id & 0x3ff == 0x09 {
                return Lang::En;
            }
        }
        Lang::Fr
    }
}

static CURRENT: AtomicU8 = AtomicU8::new(0);

pub fn lang() -> Lang {
    if CURRENT.load(Ordering::Relaxed) == 1 { Lang::En } else { Lang::Fr }
}

pub fn set_lang(lang: Lang) {
    CURRENT.store(lang as u8, Ordering::Relaxed);
}

/// Le catalogue anglais d'un domaine, lu à la première traduction.
pub struct Catalog {
    json: &'static str,
    en: OnceLock<HashMap<String, &'static str>>,
}

impl Catalog {
    pub const fn new(json: &'static str) -> Self {
        Catalog { json, en: OnceLock::new() }
    }

    fn entries(&self) -> &HashMap<String, &'static str> {
        self.en.get_or_init(|| {
            let map: HashMap<String, String> = serde_json::from_str(self.json).unwrap_or_default();
            // Chaînes vivant autant que le processus : `tr` rend un `&'static str`, comme `tr!`.
            map.into_iter().map(|(k, v)| (k, &*Box::leak(v.into_boxed_str()))).collect()
        })
    }

    /// Le texte dans la langue courante ; le français (la clé) si la traduction manque.
    pub fn tr(&self, fr: &'static str) -> &'static str {
        match lang() {
            Lang::Fr => fr,
            Lang::En => self.entries().get(fr).copied().unwrap_or(fr),
        }
    }

    /// Avec des paramètres `{nom}`.
    pub fn trf(&self, fr: &'static str, args: &[(&str, &dyn std::fmt::Display)]) -> String {
        let mut s = self.tr(fr).to_string();
        for (k, v) in args {
            s = s.replace(&format!("{{{k}}}"), &v.to_string());
        }
        s
    }

    /// Clés du catalogue (test des traductions orphelines).
    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.entries().keys().map(String::as_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    static CAT: Catalog = Catalog::new(r#"{"Réglages":"Settings","{n} modules":"{n} modules"}"#);

    #[test]
    fn translates_and_falls_back_to_french() {
        set_lang(Lang::En);
        assert_eq!(CAT.tr("Réglages"), "Settings");
        assert_eq!(CAT.tr("Inconnu"), "Inconnu");
        assert_eq!(CAT.trf("{n} modules", &[("n", &6)]), "6 modules");
        set_lang(Lang::Fr);
        assert_eq!(CAT.tr("Réglages"), "Réglages");
    }
}
