//! Menagerie dans la suite.
//!
//! - même verrou d'instance que `menagerie.exe` ;
//! - la supervision (arrêts et relances, santé, sauvegarde) est dans `ui` : appelé même masqué ;
//! - le relais `--relais` (`current_exe()`) est pris en charge par `eyrie.exe` ;
//! - le titre « Menagerie — N à voir » devient la pastille ;
//! - **réglages** : les siens sont dans ceux d'Eyrie (sa page), et son bouton de réglages y mène ;
//! - embarqué (`set_hosted`) : il n'enregistre pas la géométrie de la fenêtre de la suite ; ses
//!   couleurs suivent le thème clair ou sombre de la suite (`set_theme_override`).

use std::sync::atomic::{AtomicBool, Ordering};

use eframe::egui::Color32;
use menagerie::persist::ThemeId;
use suite_sdk::embed::{Embedded, KeyCommand, first_number};
use suite_sdk::{Badge, BuildCx, Module, ModuleFactory, ModuleInfo, Origin, Tone, Unavailable};

pub const ID: &str = "menagerie";

/// `menagerie::instance::acquire` garde son verrou jusqu'à la fin du processus : on ne le prend
/// qu'une fois (un module relancé dans la même suite l'a déjà).
static HELD: AtomicBool = AtomicBool::new(false);

pub fn info() -> ModuleInfo {
    ModuleInfo {
        id: ID,
        name: "Menagerie",
        icon: "🖥",
        color: Color32::from_rgb(0x4c, 0xb8, 0x72),
        summary: "Services locaux, ports, Docker, mails de test et environnements",
        origin: Origin::Embedded { exe: "menagerie.exe", repo: "../Menagerie" },
    }
}

pub fn factory() -> ModuleFactory {
    ModuleFactory { info: info(), build, fonts: None }
}

fn commands() -> Vec<KeyCommand> {
    vec![
        KeyCommand::new("actualiser", "Actualiser", "F5"),
        KeyCommand::new("services", "Onglet Services", "Ctrl+1"),
        KeyCommand::new("docker", "Onglet Docker", "Ctrl+2"),
        KeyCommand::new("mails", "Onglet Mails de test", "Ctrl+3"),
        KeyCommand::new("environnements", "Onglet Environnements", "Ctrl+4"),
        KeyCommand::new("nouveau", "Nouveau service…", "Ctrl+N"),
        KeyCommand::new("reglages", "Réglages de Menagerie", "Ctrl+,"),
        KeyCommand::new("rechercher", "Rechercher dans les services", "Ctrl+F"),
    ]
}

fn badge(title: &str) -> Option<Badge> {
    first_number(title).filter(|n| *n > 0).map(|n| Badge::count(n, Tone::Warn, format!("{n} service(s) à voir")))
}

fn build(bcx: &BuildCx) -> Result<Box<dyn Module>, Unavailable> {
    if !HELD.load(Ordering::Relaxed) {
        if !menagerie::instance::acquire() {
            return Err(Unavailable::AlreadyRunning { exe: "menagerie.exe" });
        }
        HELD.store(true, Ordering::Relaxed);
    }
    let menagerie::persist::Loaded { state, problems } = menagerie::persist::load();
    let mut app = menagerie::app::App::new(bcx.cc, state, problems);
    // La fenêtre est celle de la suite : sa géométrie n'entre pas dans l'état de Menagerie.
    app.set_hosted(true);
    // Ses réglages sont dans ceux d'Eyrie : son bouton de réglages y mène, sur sa page.
    let link = bcx.host.clone();
    app.set_settings_link(Box::new(move || link.open_settings(&format!("module:{ID}"))));
    let module = Embedded::new(info(), bcx.host.clone(), app)
        .hidden_ui(true)
        .title_badge(badge)
        .key_commands(commands())
        .settings_panel(|app, ui| app.settings_ui(ui))
        // Ses couleurs suivent le thème clair ou sombre de la suite (sans toucher à son réglage).
        .before_logic(|app, cx| {
            let t = if cx.host.palette().dark { ThemeId::Dark } else { ThemeId::Light };
            app.set_theme_override(Some(t));
        });
    Ok(Box::new(module))
}

#[cfg(test)]
mod tests {
    #[test]
    fn badge_from_title() {
        assert_eq!(super::badge("Menagerie — 2 à voir").and_then(|b| b.count), Some(2));
        assert_eq!(super::badge("Menagerie"), None);
    }
}
