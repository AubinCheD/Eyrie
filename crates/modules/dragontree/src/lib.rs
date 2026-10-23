//! DragonTree dans la suite.
//!
//! - même instance unique que `dragontree.exe` (mutex et canal nommé du profil) : lancé pendant que
//!   la suite tourne, `dragontree.exe <dépôt>` ouvre le dépôt dans la suite ;
//! - ses polices (Segoe UI, Cascadia, **Phosphor** pour ses icônes) entrent dans celles de la suite :
//!   il remplace toutes les polices au démarrage et quand la police du diff change, la suite les
//!   rétablit en gardant les siennes ;
//! - sa palette suit `dark_mode` d'egui : elle suit le thème de la suite ;
//! - le titre « <dépôt> — DragonTree » devient le titre du module.

use std::sync::Arc;
use std::time::Duration;

use dragontree_app::single_instance::{self, Forwarded, Startup};
use dragontree_app::{DragonTreeApp, StartOptions};
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
    let phosphor = Arc::new(FontData::from_static(include_bytes!("../../../../../DragonTree/assets/fonts/Phosphor.ttf")));
    plan.add_second(egui::FontFamily::Proportional, "phosphor", phosphor.clone());
    plan.add_second(egui::FontFamily::Monospace, "phosphor", phosphor.clone());
    plan.add_second(egui::FontFamily::Name(suite_sdk::fonts::BOLD.into()), "phosphor", phosphor);
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
    let start = StartOptions { screenshot_after: Duration::from_millis(2500), profile_dir: profile_dir.clone(), ..Default::default() };
    let profile = profile_dir.unwrap_or_else(dragontree_app::settings::config_dir);
    let key = single_instance::profile_key(&profile);
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
    Ok(Box::new(Embedded::new(info(), bcx.host.clone(), app).key_commands(commands())))
}
