//! DragonTree dans la suite.
//!
//! - même instance unique que `dragontree.exe` (mutex et canal nommé du profil) : lancé pendant que
//!   la suite tourne, `dragontree.exe <dépôt>` ouvre le dépôt dans la suite ;
//! - ses polices (Segoe UI, Cascadia, **Phosphor** pour ses icônes) entrent dans celles de la suite :
//!   il remplace toutes les polices au démarrage et quand la police du diff change, la suite les
//!   rétablit en gardant les siennes ;
//! - **thème et langue de la suite** : passés comme `--theme` et `--lang` (`set_host_overrides`),
//!   ils remplacent ses réglages pour la session sans y être écrits ; ses fonds, son texte et ses sélections prennent
//!   les couleurs de la suite (`set_host_palette`), comme les autres modules ;
//! - **réglages** : les siens sont dans ceux d'Eyrie (page DragonTree), et « Réglages… » y mène ;
//! - **comptes** : ceux de toute la suite, que DragonTree lit lui-même ; « Comptes… » ouvre la page Comptes d'Eyrie,
//!   et Thème et Langue disparaissent de ses réglages et de ses menus (`set_hosted`) ;
//! - le titre « <dépôt> — DragonTree » devient le titre du module ;
//! - **téléphone** (`mobile`) : ses dépôts et un client git presque complet, menés par DragonTree lui-même (ses
//!   onglets, sa file d'opérations, ses comptes, ses notifications et son bandeau de conflits sur le PC).

pub mod branches;
pub mod depot;
pub mod historique;
pub mod liste;
pub mod mobile;

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use dragontree_app::settings::ThemeChoice;
use dragontree_app::single_instance::{self, Forwarded, Startup};
use dragontree_app::{DragonTreeApp, StartOptions};
use dragontree_i18n::Lang;
use eframe::egui::{self, Color32, FontData};
use suite_sdk::embed::{Embedded, KeyCommand};
use suite_sdk::fonts::FontPlan;
use suite_sdk::{BuildCx, DataMode, Module, ModuleFactory, ModuleInfo, Origin, Unavailable};

pub const ID: &str = "dragontree";

pub fn info() -> ModuleInfo {
    ModuleInfo {
        id: ID,
        name: "DragonTree",
        icon: "⎇",
        logo: None,
        color: Color32::from_rgb(0xe0, 0x8a, 0x5c),
        summary: "Client git : historique, changements, branches, plusieurs dépôts en onglets",
        origin: Origin::Embedded { exe: "dragontree.exe", repo: "../DragonTree" },
    }
}

pub fn factory() -> ModuleFactory {
    ModuleFactory { info: info(), build, fonts: Some(fonts) }
}

/// Les icônes de DragonTree sont dans la zone privée d'Unicode : Phosphor juste après la police
/// principale, avant Segoe UI Symbol qui y a d'autres dessins.
fn fonts(plan: &mut FontPlan) {
    let phosphor =
        Arc::new(FontData::from_static(include_bytes!("../../../../../DragonTree/assets/fonts/Phosphor.ttf")));
    plan.add_second(egui::FontFamily::Proportional, "phosphor", phosphor.clone());
    plan.add_second(egui::FontFamily::Monospace, "phosphor", phosphor.clone());
    plan.add_second(egui::FontFamily::Name(suite_sdk::fonts::BOLD.into()), "phosphor", phosphor);
}

/// La palette de DragonTree dans les couleurs de la suite : fonds (case, fenêtres, barres), bordures, texte, accent,
/// sélection et survol viennent d'Eyrie ; les couleurs métier (diff, branches, étiquettes) restent les siennes.
pub fn palette_of(s: &suite_sdk::theme::Palette) -> dragontree_app::theme::Palette {
    let own = if s.dark { dragontree_app::theme::Palette::dark() } else { dragontree_app::theme::Palette::light() };
    dragontree_app::theme::Palette {
        bg: s.panel,
        panel: s.window,
        sidebar: s.sidebar,
        border: s.border,
        text: s.text,
        weak: s.text_dim,
        accent: s.accent,
        selection: s.selection,
        hover: s.surface_hover,
        row_alt: s.panel.lerp_to_gamma(s.surface, 0.3),
        gutter: s.panel.lerp_to_gamma(s.surface, 0.5),
        ..own
    }
}

fn commands() -> Vec<KeyCommand> {
    vec![
        KeyCommand::new("onglet", "Nouvel onglet", "Ctrl+T"),
        KeyCommand::new("ouvrir", "Ouvrir un dépôt…", "Ctrl+O"),
        KeyCommand::new("cloner", "Cloner…", "Ctrl+Maj+C"),
        KeyCommand::new("fermer", "Fermer l'onglet", "Ctrl+W"),
        KeyCommand::new("rouvrir", "Rouvrir l'onglet fermé", "Ctrl+Maj+T"),
        KeyCommand::new("rapide", "Ouverture rapide", "Ctrl+P"),
        KeyCommand::new("liste", "Afficher ou masquer la liste des dépôts", "Ctrl+B"),
        KeyCommand::new("fetch", "Fetch", "Ctrl+Maj+F"),
        KeyCommand::new("pull", "Pull", "Ctrl+Maj+U"),
        KeyCommand::new("push", "Push", "Ctrl+Maj+P"),
        KeyCommand::new("branche", "Nouvelle branche…", "Ctrl+Maj+B"),
        KeyCommand::new("stash", "Remiser (stash)", "Ctrl+Maj+H"),
        KeyCommand::new("journal", "Journal des commandes git", "Ctrl+Maj+L"),
        KeyCommand::new("actualiser", "Actualiser", "F5"),
        KeyCommand::new("reglages", "Réglages de DragonTree", "Ctrl+,"),
    ]
}

fn build(bcx: &BuildCx) -> Result<Box<dyn Module>, Unavailable> {
    let profile_dir = match &bcx.data {
        DataMode::Shared => None,
        DataMode::Trial(dir) => Some(dir.clone()),
    };
    // Eyrie sur un profil isolé : ses jetons sont dans un coffre à lui ; DragonTree n'approche ni celui de la suite, ni
    // son ancien coffre « DragonTree ».
    if let Some(service) = bcx.host.accounts_vault() {
        dragontree_forges::secrets::use_vaults(
            Arc::new(suite_accounts::OsVault::new(&service)),
            Arc::new(suite_accounts::MemoryVault::default()),
        );
    }
    let start = StartOptions {
        screenshot_after: Duration::from_millis(2500),
        profile_dir: profile_dir.clone(),
        // Les comptes de toute la suite, dans le dossier donné par Eyrie.
        accounts_dir: bcx.host.accounts_dir(),
        ..Default::default()
    };
    // La liste de son dernier scan est dans le profil d'essai, sinon dans son dossier de cache.
    let scan = match &profile_dir {
        Some(dir) => dir.join("repos-cache.json"),
        None => dragontree_app::settings::cache_dir().join("repos-cache.json"),
    };
    let profile = profile_dir.unwrap_or_else(dragontree_app::settings::config_dir);
    let key = single_instance::profile_key(&profile);
    // Le téléphone lit la liste des dépôts dans les mêmes fichiers que DragonTree (réglages et liste du scan, dans
    // `StartOptions::profile_dir`, sinon le profil par défaut) ; le reste, il le demande à DragonTree.
    let phone = Rc::new(RefCell::new(mobile::Phone::new(profile.join("settings.json"), scan)));
    let (phone_actions, phone_logic) = (phone.clone(), phone.clone());
    let instance = match single_instance::start(&key, &Forwarded { open: Vec::new() }, Duration::from_secs(3)) {
        Startup::Primary(i) => Some(i),
        // Une autre instance tient ce profil : elle a reçu la demande et passe au premier plan.
        Startup::Forwarded => return Err(Unavailable::AlreadyRunning { exe: "dragontree.exe" }),
        Startup::Alone(_) => None,
    };
    let mut app = DragonTreeApp::new(bcx.cc, start);
    if let Some(i) = instance {
        app.set_instance(i);
    }
    app.set_hosted(true);
    let link = bcx.host.clone();
    app.set_accounts_link(Box::new(move || link.open_settings("comptes")));
    // Ses réglages sont dans ceux d'Eyrie : « Réglages… » y mène, sur sa page.
    let settings = bcx.host.clone();
    app.set_settings_link(Box::new(move || settings.open_settings(&format!("module:{ID}"))));
    // Le dépôt de l'onglet actif devient le contexte partagé de la suite (Carnet, Fawkes le suivent).
    let mut published: Option<std::path::PathBuf> = None;
    let module = Embedded::new(info(), bcx.host.clone(), app)
        .key_commands(commands())
        .settings_panel(|app, ui| app.settings_ui(ui))
        .on_settings_left(|app| app.settings_left())
        .mobile_screens(move |app, path, _| phone.borrow_mut().screen(app, path))
        .mobile_actions(move |app, path, action, input, cx| {
            let (text, show) = phone_actions.borrow_mut().action(app, path, action, input)?;
            if show {
                cx.host.show(ID);
            }
            cx.host.repaint();
            Ok(Some(text))
        })
        .after_logic(move |app, cx| {
            phone_logic.borrow_mut().after_logic(app);
            let now = app.active_repo_dir().map(|p| p.to_path_buf());
            if now != published {
                if let Some(dir) = &now {
                    let label = dir
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| dir.display().to_string());
                    cx.host.set_context(Some(suite_sdk::host::SharedContext {
                        dir: dir.clone(),
                        label,
                        source: ID.into(),
                    }));
                }
                published = now;
            }
        });
    let module = module.before_logic(|app, cx| {
        // Ses fonds, son texte et ses sélections sont ceux de la suite : il se fond dans les autres modules.
        dragontree_app::theme::set_host_palette(cx.host.ctx(), Some(palette_of(cx.host.palette())));
        let theme = if cx.host.palette().dark { ThemeChoice::Dark } else { ThemeChoice::Light };
        let lang = match cx.host.lang() {
            suite_sdk::i18n::Lang::Fr => Lang::Fr,
            suite_sdk::i18n::Lang::En => Lang::En,
        };
        app.set_host_overrides(Some(theme), Some(lang));
    });
    Ok(Box::new(module))
}

#[cfg(test)]
mod tests {
    use suite_sdk::theme::{FLUENT, GRAPHITE, NUIT};

    /// Le fond principal et les barres de DragonTree sont ceux de la suite, dans chaque thème ; ses couleurs de diff
    /// restent les siennes.
    #[test]
    fn dragontree_takes_the_suite_backgrounds() {
        for s in [&GRAPHITE, &FLUENT, &NUIT] {
            let p = super::palette_of(s);
            assert_eq!((p.dark, p.bg, p.sidebar, p.text), (s.dark, s.panel, s.sidebar, s.text));
            let own =
                if s.dark { dragontree_app::theme::Palette::dark() } else { dragontree_app::theme::Palette::light() };
            assert_eq!(p.added_bg, own.added_bg, "diff inchangé");
        }
    }
}
