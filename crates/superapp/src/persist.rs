//! État de la suite : `%APPDATA%\SuperApp\state.json` (ou `SUPERAPP_DATA_DIR`).
//!
//! Seulement ce qui appartient à la suite : réglages communs, modules activés, dispositions,
//! fenêtres. L'état de chaque module reste dans son propre dossier, lu et écrit de la même façon
//! seul ou dans la suite.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use egui_tiles::Tree;
use serde::{Deserialize, Serialize};
use suite_sdk::i18n::Lang;
use suite_sdk::theme::ThemeChoice;

use crate::layout::Pane;

pub const VERSION: u32 = 1;

pub fn data_dir() -> PathBuf {
    std::env::var_os("SUPERAPP_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| dirs::config_dir().unwrap_or_else(std::env::temp_dir).join("SuperApp"))
}

/// Où les modules embarqués rangent leur état dans la suite.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModuleData {
    /// Le dossier de leur exécutable autonome (les mêmes groupes, tâches, filtres des deux côtés).
    #[default]
    Shared,
    /// `<suite>\essai\<module>` : pour essayer la suite sans toucher aux données de tous les jours.
    Trial,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ModuleSettings {
    pub enabled: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SavedWindow {
    pub id: u64,
    pub tree: Tree<Pane>,
    pub pos: Option<[f32; 2]>,
    pub size: [f32; 2],
}

/// Une disposition : l'arbre de la fenêtre principale et les fenêtres détachées.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SavedLayout {
    pub main: Tree<Pane>,
    #[serde(default)]
    pub windows: Vec<SavedWindow>,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowGeometry {
    pub size: [f32; 2],
    pub pos: Option<[f32; 2]>,
    pub maximized: bool,
}

impl Default for WindowGeometry {
    fn default() -> Self {
        WindowGeometry { size: [1500.0, 920.0], pos: None, maximized: false }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TailnetSettings {
    pub enabled: bool,
    /// Nom du nœud ; vide : `suite-<poste>`.
    pub hostname: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SuiteState {
    /// `None` : la langue de Windows.
    pub lang: Option<Lang>,
    pub theme: ThemeChoice,
    pub zoom: f32,
    pub module_data: ModuleData,
    pub modules: BTreeMap<String, ModuleSettings>,
    pub layout: Option<SavedLayout>,
    pub layouts: BTreeMap<String, SavedLayout>,
    /// Configuration d'écrans (« 2 écrans : 2560×1440 + 1920×1080 ») → disposition à appliquer.
    pub screen_layouts: BTreeMap<String, String>,
    pub window: WindowGeometry,
    /// Raccourcis de la suite changés par l'utilisateur : commande → combinaison.
    pub shortcuts: BTreeMap<String, String>,
    pub tailnet: TailnetSettings,
    /// Les cases suivent le contexte partagé (dépôt courant).
    pub follow_context: bool,
    /// Le premier lancement est passé (accueil, import des comptes proposé).
    pub welcomed: bool,
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

impl Default for SuiteState {
    fn default() -> Self {
        SuiteState {
            lang: None,
            theme: ThemeChoice::Systeme,
            zoom: 1.0,
            module_data: ModuleData::Shared,
            modules: BTreeMap::new(),
            layout: None,
            layouts: BTreeMap::new(),
            screen_layouts: BTreeMap::new(),
            window: WindowGeometry::default(),
            shortcuts: BTreeMap::new(),
            tailnet: TailnetSettings::default(),
            follow_context: true,
            welcomed: false,
            extra: BTreeMap::new(),
        }
    }
}

impl SuiteState {
    pub fn enabled(&self, module: &str) -> bool {
        // Un module jamais vu est activé : la suite montre tout ce qu'elle sait faire au départ.
        self.modules.get(module).is_none_or(|m| m.enabled)
    }
}

pub fn state_path(dir: &Path) -> PathBuf {
    dir.join("state.json")
}

pub fn load(dir: &Path) -> suite_sdk::state::Loaded<SuiteState> {
    suite_sdk::state::load_json(&state_path(dir), VERSION)
}

pub fn save(dir: &Path, st: &SuiteState) -> std::io::Result<()> {
    suite_sdk::state::save_json(&state_path(dir), st, VERSION, env!("CARGO_PKG_VERSION"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_enable_new_modules() {
        let mut st = SuiteState::default();
        assert!(st.enabled("fawkes"));
        st.modules.insert("fawkes".into(), ModuleSettings { enabled: false });
        assert!(!st.enabled("fawkes"));
    }

    #[test]
    fn unknown_fields_are_kept() {
        let st: SuiteState = serde_json::from_str(r#"{"zoom":1.2,"plus_tard":[1,2]}"#).unwrap();
        assert_eq!(st.zoom, 1.2);
        let back = serde_json::to_value(&st).unwrap();
        assert_eq!(back["plus_tard"], serde_json::json!([1, 2]));
    }
}
