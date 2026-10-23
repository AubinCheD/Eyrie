//! Les polices de la suite.
//!
//! `Context::set_fonts` remplace **toutes** les polices : un module embarqué qui l'appelle efface
//! celles des autres (et Fawkes panique si ses familles `term*` disparaissent). L'hôte installe donc
//! un [`FontPlan`] qui couvre les besoins de tous les modules, et le rétablit au début de chaque
//! image si un module l'a remplacé ([`guard`]), avant que le moindre texte soit mis en page.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use eframe::egui::{self, FontData, FontDefinitions, FontFamily};

/// Familles nommées attendues par les modules.
pub const TERM: &str = "term";
pub const TERM_PRIMARY: &str = "term-primary";
pub const BOLD: &str = "bold";
pub const DIFF: &str = "diff";

/// Ce que l'hôte garantit.
#[derive(Clone, Default)]
pub struct FontPlan {
    data: BTreeMap<String, Arc<FontData>>,
    /// Familles qui appartiennent à la suite : rétablies à l'identique.
    owned: BTreeMap<FontFamily, Vec<String>>,
    /// Familles nommées des modules : ajoutées si elles manquent, laissées sinon.
    ensured: BTreeMap<FontFamily, Vec<String>>,
}

fn font_dirs() -> Vec<PathBuf> {
    let mut v = vec![PathBuf::from(std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into())).join("Fonts")];
    if let Some(local) = dirs::data_local_dir() {
        v.push(local.join(r"Microsoft\Windows\Fonts"));
    }
    v
}

fn load_font(names: &[&str]) -> Option<Vec<u8>> {
    let dirs = font_dirs();
    names.iter().find_map(|n| dirs.iter().find_map(|d| std::fs::read(d.join(n)).ok()))
}

impl FontPlan {
    /// Segoe UI pour l'interface, Segoe UI Semibold en gras, Cascadia Mono (ou Consolas) pour le
    /// code et les terminaux, Segoe UI Symbol puis les emoji monochromes d'egui en repli.
    pub fn suite() -> Self {
        let defaults = FontDefinitions::default();
        let mut plan = FontPlan::default();
        let fallbacks: Vec<String> = ["egui-icons", "NotoEmoji-Regular", "emoji-icon-font"]
            .into_iter()
            .filter_map(|n| defaults.font_data.get(n).map(|d| (n, d.clone())))
            .map(|(n, d)| {
                plan.data.insert(n.to_string(), d);
                n.to_string()
            })
            .collect();
        let mut add = |key: &str, files: &[&str], fallback: &str| -> String {
            match load_font(files) {
                Some(bytes) => {
                    plan.data.insert(key.to_string(), Arc::new(FontData::from_owned(bytes)));
                    key.to_string()
                }
                None => {
                    if let Some(d) = defaults.font_data.get(fallback) {
                        plan.data.insert(fallback.to_string(), d.clone());
                    }
                    fallback.to_string()
                }
            }
        };
        let ui = add("suite-ui", &["segoeui.ttf"], "Ubuntu-Light");
        let bold = add("suite-bold", &["seguisb.ttf", "segoeuib.ttf"], &ui);
        let mono = add("suite-mono", &["CascadiaMono.ttf", "consola.ttf"], "Hack");
        let symbols = load_font(&["seguisym.ttf"]).map(|bytes| {
            plan.data.insert("suite-symbols".into(), Arc::new(FontData::from_owned(bytes)));
            "suite-symbols".to_string()
        });
        let chain = |first: &str| -> Vec<String> {
            std::iter::once(first.to_string()).chain(symbols.clone()).chain(fallbacks.iter().cloned()).collect()
        };
        plan.owned.insert(FontFamily::Proportional, chain(&ui));
        plan.owned.insert(FontFamily::Monospace, chain(&mono));
        plan.ensured.insert(FontFamily::Name(TERM.into()), chain(&mono));
        plan.ensured.insert(FontFamily::Name(TERM_PRIMARY.into()), vec![mono.clone()]);
        plan.ensured.insert(FontFamily::Name(BOLD.into()), chain(&bold));
        plan.ensured.insert(FontFamily::Name(DIFF.into()), chain(&mono));
        plan
    }

    /// Une police de repli ajoutée en fin de famille.
    pub fn add_fallback(&mut self, family: FontFamily, key: &str, data: Arc<FontData>) {
        self.add(family, key, data, usize::MAX);
    }

    /// Une police juste après la police principale de la famille, avant les symboles : pour des
    /// icônes dans la zone privée d'Unicode (Phosphor de DragonTree), que Segoe UI Symbol couvre
    /// aussi avec d'autres dessins.
    pub fn add_second(&mut self, family: FontFamily, key: &str, data: Arc<FontData>) {
        self.add(family, key, data, 1);
    }

    fn add(&mut self, family: FontFamily, key: &str, data: Arc<FontData>, at: usize) {
        self.data.insert(key.to_string(), data);
        for list in self.owned.get_mut(&family).into_iter().chain(self.ensured.get_mut(&family)) {
            if !list.iter().any(|k| k == key) {
                list.insert(at.min(list.len()), key.to_string());
            }
        }
    }

    /// Une famille nommée propre à un module.
    pub fn ensure_family(&mut self, family: &str, keys: Vec<String>) {
        self.ensured.insert(FontFamily::Name(family.into()), keys);
    }

    /// Les polices en place couvrent-elles le plan ?
    pub fn satisfied_by(&self, defs: &FontDefinitions) -> bool {
        self.data.keys().all(|k| defs.font_data.contains_key(k))
            && self.owned.iter().all(|(f, keys)| defs.families.get(f) == Some(keys))
            && self.ensured.keys().all(|f| defs.families.contains_key(f))
    }

    /// Les polices en place (celles d'un module, s'il vient de les remplacer), complétées par le plan.
    pub fn merged(&self, current: &FontDefinitions) -> FontDefinitions {
        let mut defs = current.clone();
        for (k, d) in &self.data {
            defs.font_data.entry(k.clone()).or_insert_with(|| d.clone());
        }
        for (f, keys) in &self.owned {
            defs.families.insert(f.clone(), keys.clone());
        }
        for (f, keys) in &self.ensured {
            defs.families.entry(f.clone()).or_insert_with(|| keys.clone());
        }
        defs
    }

    pub fn definitions(&self) -> FontDefinitions {
        self.merged(&FontDefinitions::empty())
    }
}

/// À appeler au début de chaque image, avant toute mise en page : rétablit le plan si un module a
/// remplacé les polices. Les nouvelles polices s'appliquent tout de suite (rien n'a encore été mis
/// en page dans cette image). Renvoie `true` si le plan a dû être rétabli.
pub fn guard(ctx: &egui::Context, plan: &FontPlan) -> bool {
    let current = ctx.fonts(|f| (!plan.satisfied_by(f.definitions())).then(|| f.definitions().clone()));
    match current {
        Some(defs) => {
            ctx.set_fonts(plan.merged(&defs));
            true
        }
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_module_replacing_all_fonts_is_completed() {
        let plan = FontPlan::suite();
        // Ce que ferait DragonTree : ses propres polices, sans les familles de Fawkes.
        let mut theirs = FontDefinitions::default();
        theirs.families.insert(FontFamily::Name(BOLD.into()), vec!["Ubuntu-Light".into()]);
        assert!(!plan.satisfied_by(&theirs));
        let merged = plan.merged(&theirs);
        assert!(plan.satisfied_by(&merged));
        assert!(merged.families.contains_key(&FontFamily::Name(TERM.into())));
        // La famille du module est gardée telle quelle.
        assert_eq!(merged.families[&FontFamily::Name(BOLD.into())], vec!["Ubuntu-Light".to_string()]);
    }

    #[test]
    fn fallbacks_go_last() {
        let mut plan = FontPlan::suite();
        let data = FontDefinitions::default().font_data.values().next().unwrap().clone();
        plan.add_fallback(FontFamily::Proportional, "phosphor", data);
        let defs = plan.definitions();
        assert_eq!(defs.families[&FontFamily::Proportional].last().map(String::as_str), Some("phosphor"));
    }

    #[test]
    fn guard_restores_the_plan() {
        let ctx = egui::Context::default();
        let plan = FontPlan::suite();
        // Sans moteur de rendu : les textures produites sont jetées.
        let run = |f: &mut dyn FnMut(&mut egui::Ui)| ctx.run_ui(Default::default(), |ui| f(ui)).textures_delta.clear();
        run(&mut |_| {});
        ctx.set_fonts(FontDefinitions::default());
        run(&mut |ui| assert!(guard(ui.ctx(), &plan)));
        run(&mut |ui| assert!(!guard(ui.ctx(), &plan), "le plan tient d'une image à l'autre"));
    }
}
