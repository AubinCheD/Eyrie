//! État d'Eyrie : `%APPDATA%\EyrieSuite\Eyrie\state.json` (ou `EYRIE_DATA_DIR`).
//!
//! Seulement ce qui appartient à Eyrie : réglages communs, modules activés, dispositions,
//! fenêtres. L'état de chaque module reste dans son propre dossier, lu et écrit de la même façon
//! seul ou dans Eyrie ; ce qui est commun à toute la suite (les comptes) est à la racine de
//! `EyrieSuite` (crate `suite-accounts`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use egui_tiles::Tree;
use serde::{Deserialize, Serialize};
use suite_sdk::i18n::Lang;
use suite_sdk::theme::ThemeChoice;

use crate::layout::Pane;

pub const VERSION: u32 = 1;

pub fn data_dir() -> PathBuf {
    std::env::var_os("EYRIE_DATA_DIR").filter(|d| !d.is_empty()).map(PathBuf::from).unwrap_or_else(default_data_dir)
}

/// `<EyrieSuite>\Eyrie` : le dossier d'Eyrie dans celui de la suite.
pub fn default_data_dir() -> PathBuf {
    suite_accounts::suite_dir().join("Eyrie")
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

/// Le pont mobile (`docs/mobile.md`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MobileSettings {
    /// « Accès depuis le téléphone » : l'interrupteur du pont.
    pub enabled: bool,
    /// Une bulle sur le PC pour chaque action faite au téléphone.
    pub show_actions: bool,
    /// « Démarrer Eyrie depuis le téléphone » : le veilleur garde le PC sur le tailnet quand Eyrie est fermée, et la
    /// démarre à la demande d'un téléphone appairé (`crate::veille`).
    pub wake: bool,
}

impl Default for MobileSettings {
    fn default() -> Self {
        MobileSettings { enabled: false, show_actions: true, wake: false }
    }
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
    pub mobile: MobileSettings,
    /// Les cases suivent le contexte partagé (dépôt courant).
    pub follow_context: bool,
    /// Barre de titre de Windows, menus en dessous (sinon Eyrie dessine la sienne, menus compris).
    pub native_title_bar: bool,
    /// Le premier lancement est passé (accueil, choix des données des modules).
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
            mobile: MobileSettings::default(),
            follow_context: true,
            native_title_bar: false,
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

/// Bornes du zoom (celles du réglage).
pub const ZOOM: std::ops::RangeInclusive<f32> = 0.8..=1.6;

pub fn load(dir: &Path) -> suite_sdk::state::Loaded<SuiteState> {
    let mut loaded: suite_sdk::state::Loaded<SuiteState> = suite_sdk::state::load_json(&state_path(dir), VERSION);
    loaded.problems.extend(loaded.value.sanitize());
    loaded
}

impl SuiteState {
    /// Ce qui, relu d'un fichier abîmé ou modifié à la main, empêcherait Eyrie de démarrer : zoom hors de ses bornes
    /// (nul, négatif, infini), disposition qui n'est pas un arbre. Rend ce qui a été écarté.
    fn sanitize(&mut self) -> Vec<String> {
        let mut problems = Vec::new();
        if !self.zoom.is_finite() || !ZOOM.contains(&self.zoom) {
            let zoom = if self.zoom.is_finite() { self.zoom.clamp(*ZOOM.start(), *ZOOM.end()) } else { 1.0 };
            problems.push(format!("Zoom {} hors des bornes : ramené à {zoom}.", self.zoom));
            self.zoom = zoom;
        }
        let sound = |l: &SavedLayout| {
            crate::layout::is_sound(&l.main) && l.windows.iter().all(|w| crate::layout::is_sound(&w.tree))
        };
        if self.layout.as_ref().is_some_and(|l| !sound(l)) {
            self.layout = None;
            problems.push("Disposition enregistrée illisible : disposition par défaut.".into());
        }
        self.layouts.retain(|name, l| {
            let keep = sound(l);
            if !keep {
                problems.push(format!("Disposition « {name} » illisible : retirée."));
            }
            keep
        });
        problems
    }
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

    /// Un `state.json` abîmé ou modifié à la main n'empêche pas Eyrie de démarrer : le zoom revient dans ses bornes, une
    /// disposition qui n'est pas un arbre (boucle) est écartée.
    #[test]
    fn a_damaged_state_is_brought_back_within_bounds() {
        for (zoom, want) in [(0.0, 0.8), (-3.0, 0.8), (40.0, 1.6), (f32::NAN, 1.0), (1.2, 1.2)] {
            let mut st = SuiteState { zoom, ..SuiteState::default() };
            st.sanitize();
            assert_eq!(st.zoom, want, "{zoom}");
        }
        let mut tiles = egui_tiles::Tiles::default();
        let pane = tiles.insert_pane(Pane::module("fawkes", suite_sdk::ViewInfo::MAIN));
        let tabs = tiles.insert_tab_tile(vec![pane]);
        if let Some(egui_tiles::Tile::Container(c)) = tiles.get_mut(tabs) {
            c.add_child(tabs);
        }
        let looped = SavedLayout { main: Tree::new(egui::Id::new("boucle"), tabs, tiles), windows: Vec::new() };
        let fine = SavedLayout { main: Tree::empty(egui::Id::new("vide")), windows: Vec::new() };
        let mut st = SuiteState { layout: Some(looped.clone()), ..SuiteState::default() };
        st.layouts.insert("boucle".into(), looped);
        st.layouts.insert("vide".into(), fine);
        assert_eq!(st.sanitize().len(), 2);
        assert_eq!(st.layout, None);
        assert_eq!(st.layouts.keys().collect::<Vec<_>>(), ["vide"]);
    }
}
