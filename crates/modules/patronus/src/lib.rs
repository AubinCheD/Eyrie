//! Patronus dans la suite.
//!
//! - même verrou d'instance que `patronus.exe` (`Local\Patronus-single-instance:<dossier>`) : deux
//!   fenêtres sur le même dossier lanceraient chaque tâche deux fois ;
//! - la synchro du Planificateur de Windows est dans `ui` : appelé même masqué ;
//! - le Planificateur et la clé `Run` visent `current_exe()` : dans la suite, `eyrie.exe`, qui
//!   comprend `lancer <tâche>` et `planifie <tâche>` ([`cli`]) ;
//! - **réglages** : les siens sont dans ceux d'Eyrie (page Patronus), et « Réglages » y mène ;
//! - le titre « (N) Patronus » devient la pastille.

mod cli;

use std::path::PathBuf;

use eframe::egui::Color32;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use suite_sdk::embed::{Embedded, KeyCommand, count_in_parens};
use suite_sdk::instance::{NamedMutex, dir_key};
use suite_sdk::{Badge, BuildCx, DataMode, Module, ModuleFactory, ModuleInfo, Origin, Tone, Unavailable};

pub use cli::cli;

pub const ID: &str = "patronus";

pub fn info() -> ModuleInfo {
    ModuleInfo {
        id: ID,
        name: "Patronus",
        icon: "✦",
        color: Color32::from_rgb(0x9f, 0xc5, 0xff),
        summary: "Agents autonomes et tâches récurrentes",
        origin: Origin::Embedded { exe: "patronus.exe", repo: "../Patronus" },
    }
}

pub fn factory() -> ModuleFactory {
    ModuleFactory { info: info(), build, fonts: None }
}

/// Repris de `Patronus/crates/patronus-app/src/main.rs` (`instance_name`).
pub fn instance_name(data_dir: &std::path::Path) -> String {
    format!("Local\\Patronus-single-instance:{}", dir_key(data_dir))
}

pub fn data_dir(mode: &DataMode) -> PathBuf {
    match mode {
        DataMode::Shared => patronus_core::fsutil::data_dir(),
        DataMode::Trial(dir) => dir.clone(),
    }
}

fn commands() -> Vec<KeyCommand> {
    vec![
        KeyCommand::new("nouvelle", "Nouvelle tâche", "Ctrl+N"),
        KeyCommand::new("boite", "Boîte de réception", "Ctrl+B"),
        KeyCommand::new("lancer", "Lancer la tâche sélectionnée", "F5"),
        KeyCommand::new("arreter", "Tout arrêter", "Ctrl+Maj+X"),
        KeyCommand::new("palette", "Palette de Patronus", "Ctrl+P"),
        KeyCommand::new("reglages", "Réglages de Patronus", "Ctrl+,"),
        KeyCommand::new("non_lu", "Résultat non lu suivant", "Ctrl+J"),
        KeyCommand::new("pause", "Tout mettre en pause", "Ctrl+Maj+P"),
    ]
}

fn badge(title: &str) -> Option<Badge> {
    count_in_parens(title).filter(|n| *n > 0).map(|n| Badge::count(n, Tone::Info, format!("{n} résultat(s) non lu(s)")))
}

/// Retire l'icône de Patronus de la zone de notification à la fermeture (comme son `main`).
struct TrayCleanup;

impl Drop for TrayCleanup {
    fn drop(&mut self) {
        patronus_app::notify::cleanup();
    }
}

fn build(bcx: &BuildCx) -> Result<Box<dyn Module>, Unavailable> {
    let dir = data_dir(&bcx.data);
    let guard =
        NamedMutex::try_acquire(&instance_name(&dir)).ok_or(Unavailable::AlreadyRunning { exe: "patronus.exe" })?;
    let (st, problems) = patronus_app::state::load(&dir);
    // L'icône de notification de Patronus, sur la fenêtre de la suite (aucun autre module ne pose la
    // sienne : Fawkes cherche sa fenêtre par son titre, Owlery passe par le centre de la suite).
    let tray = match bcx.cc.window_handle().map(|h| h.as_raw()) {
        Ok(RawWindowHandle::Win32(w)) => {
            patronus_app::notify::init(w.hwnd.get());
            Some(TrayCleanup)
        }
        _ => None,
    };
    let mut app = patronus_app::app::App::new(bcx.cc, dir, st, problems);
    // La fenêtre est celle de la suite : sa taille n'entre pas dans l'état de Patronus. Sa palette
    // suit déjà le thème clair ou sombre d'egui, donc celui de la suite.
    app.set_hosted(true);
    // Ses réglages sont dans ceux d'Eyrie : « Réglages » y mène, sur sa page.
    let link = bcx.host.clone();
    app.set_settings_link(Box::new(move || link.open_settings(&format!("module:{ID}"))));
    Ok(Box::new(
        Embedded::new(info(), bcx.host.clone(), app)
            .hidden_ui(true)
            .title_badge(badge)
            .key_commands(commands())
            .settings_panel(|app, ui| app.settings_ui(ui))
            .on_settings_left(|app| app.settings_left())
            .keep(guard)
            .keep(tray),
    ))
}

#[cfg(test)]
mod tests {
    #[test]
    fn badge_from_title() {
        assert_eq!(super::badge("(3) Patronus").and_then(|b| b.count), Some(3));
        assert_eq!(super::badge("Patronus"), None);
    }

    #[test]
    fn same_lock_as_the_exe() {
        let n = super::instance_name(std::path::Path::new(r"C:\Temp\Patronus-Essai\"));
        assert_eq!(n, r"Local\Patronus-single-instance:c:/temp/patronus-essai");
    }
}
