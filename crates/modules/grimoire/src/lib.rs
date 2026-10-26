//! Grimoire dans la suite.
//!
//! La bibliothèque de Grimoire est déjà pensée comme module (« l'exe est une enveloppe fine »,
//! `Grimoire/src/lib.rs`) : on refait ici ce que fait son `main.rs`, sans `run_native`.
//! - même verrou d'instance et même canal nommé que `grimoire.exe` : lancé pendant que la suite
//!   tourne, l'exe passe sa demande à la suite ;
//! - son travail de fond (demandes reçues, relevés) est sondé dans `ui` : appelé même masqué ;
//! - **réglages** : les siens sont dans ceux d'Eyrie (sa page), et son bouton de réglages y mène ;
//! - `grimoire-statusline.exe` reste un exe à part, lancé par Claude Code.

use std::sync::mpsc;

use eframe::egui::Color32;
use grimoire::state::ThemeId;
use suite_sdk::embed::{Embedded, KeyCommand};
use suite_sdk::{BuildCx, Module, ModuleFactory, ModuleInfo, Origin, Unavailable};

pub const ID: &str = "grimoire";

pub fn info() -> ModuleInfo {
    ModuleInfo {
        id: ID,
        name: "Grimoire",
        icon: "📖",
        logo: None,
        color: Color32::from_rgb(0xb0, 0x7c, 0xe0),
        summary: "Configuration de Claude Code, conversations et consommation",
        origin: Origin::Embedded { exe: "grimoire.exe", repo: "../Grimoire" },
    }
}

pub fn factory() -> ModuleFactory {
    ModuleFactory { info: info(), build, fonts: None }
}

fn commands() -> Vec<KeyCommand> {
    vec![
        KeyCommand::new("palette", "Palette de Grimoire", "Ctrl+P"),
        KeyCommand::new("dossier", "Choisir un dossier…", "Ctrl+O"),
        KeyCommand::new("relire", "Relire la configuration", "F5"),
        KeyCommand::new("rechercher", "Rechercher une conversation…", "Ctrl+F"),
        KeyCommand::new("preferences", "Préférences de Grimoire", "Ctrl+,"),
        KeyCommand::new("instructions", "Onglet Instructions", "Ctrl+1"),
        KeyCommand::new("reglages", "Onglet Réglages", "Ctrl+2"),
        KeyCommand::new("skills", "Onglet Skills", "Ctrl+3"),
        KeyCommand::new("crochets", "Onglet Crochets", "Ctrl+4"),
        KeyCommand::new("mcp", "Onglet MCP", "Ctrl+5"),
        KeyCommand::new("conversations", "Onglet Conversations", "Ctrl+6"),
        KeyCommand::new("consommation", "Onglet Consommation", "Ctrl+7"),
        KeyCommand::new("diagnostic", "Onglet Diagnostic", "Ctrl+8"),
        KeyCommand::new("profils", "Profils Claude…", "Ctrl+Maj+O"),
        KeyCommand::new("session", "Nouvelle session Claude ici", "Ctrl+Maj+N"),
    ]
}

fn build(bcx: &BuildCx) -> Result<Box<dyn Module>, Unavailable> {
    // `GRIMOIRE_DATA_DIR` est posé par la suite en mode essai (avant tout fil).
    let custom = grimoire::instance::custom_data_dir();
    let guard =
        grimoire::instance::acquire(custom.as_deref()).ok_or(Unavailable::AlreadyRunning { exe: "grimoire.exe" })?;
    let (tx, rx) = mpsc::channel();
    let pipe = grimoire::instance::pipe_name(custom.as_deref());
    let wake = bcx.cc.egui_ctx.clone();
    std::thread::Builder::new()
        .name("grimoire-canal".into())
        .spawn(move || grimoire::instance::serve(pipe, tx, Box::new(move || wake.request_repaint())))
        .map_err(|e| Unavailable::Failed(format!("canal nommé : {e}")))?;
    let loaded = grimoire::state::load();
    grimoire::state::remove_roaming_index();
    let mut app = grimoire::app::App::new(&bcx.cc.egui_ctx, loaded, None, rx);
    // La fenêtre est celle de la suite : sa taille n'entre pas dans l'état de Grimoire.
    app.set_hosted(true);
    // Ses réglages sont dans ceux d'Eyrie : son bouton de réglages y mène, sur sa page.
    let link = bcx.host.clone();
    app.set_settings_link(Box::new(move || link.open_settings(&format!("module:{ID}"))));
    let module = Embedded::new(info(), bcx.host.clone(), app)
        .hidden_ui(true)
        .key_commands(commands())
        .settings_panel(|app, ui| app.settings_ui(ui))
        .on_settings_left(|app| app.settings_left())
        // Ses couleurs suivent le thème clair ou sombre de la suite (sans toucher à son réglage).
        .before_logic(|app, cx| {
            let t = if cx.host.palette().dark { ThemeId::Sombre } else { ThemeId::Clair };
            app.set_theme_override(Some(t));
        })
        .keep(guard);
    Ok(Box::new(module))
}

#[cfg(test)]
mod tests {
    /// Une commande par identifiant : sinon deux entrées dans la palette, et deux fois le même raccourci au registre.
    #[test]
    fn commands_are_unique() {
        let ids: Vec<&str> = super::commands().iter().map(|k| k.id).collect();
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), ids.len(), "en double : {ids:?}");
    }
}
