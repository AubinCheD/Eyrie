//! Eyrie — les modules de la suite EyrieSuite dans une seule application.
//!
//! `eyrie.exe` ouvre la suite. Les arguments que les modules embarqués donnent à leur propre
//! exécutable quand ils se relancent eux-mêmes (`current_exe()`) sont pris en charge ici :
//! `--relais …` (Menagerie), `lancer <tâche>` et `planifie <tâche>` (Patronus, Planificateur de
//! Windows).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod accounts;
mod app;
mod capture;
mod core;
mod i18n;
mod icon;
mod layout;
mod migrate;
mod palette;
mod persist;
mod registry;
mod sandbox;
mod screens;
mod settings;
mod shell;
mod tailnet;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_deep;
mod tiles;
mod views;
mod windows;

use std::time::Duration;

use eframe::egui;

/// Relance la suite une fois celle-ci fermée (le nouveau processus attend la fin de celui-ci).
pub fn restart() {
    let Ok(exe) = std::env::current_exe() else { return };
    let mut args: Vec<String> = std::env::args().skip(1).filter(|a| !a.starts_with("--apres=")).collect();
    args.push(format!("--apres={}", std::process::id()));
    let _ = std::process::Command::new(exe).args(args).spawn();
}

/// `--apres=<pid>` : attendre la fin de l'ancienne suite (elle tient le verrou de chaque module).
fn wait_for_previous(args: &[String]) {
    let Some(pid) = args.iter().find_map(|a| a.strip_prefix("--apres=")).and_then(|p| p.parse::<u32>().ok()) else {
        return;
    };
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject};
        let h = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
        if !h.is_null() {
            WaitForSingleObject(h, 20_000);
            CloseHandle(h);
        }
    }
}

/// Mode essai : chaque module embarqué lit et écrit son état dans `<suite>\essai\<module>`, par la
/// variable d'environnement que son exe connaît déjà. Posées avant le moindre fil ; héritées par les
/// processus que les modules lancent (un `fawkes.exe` lancé depuis un terminal de la suite en mode
/// essai utiliserait aussi le dossier d'essai).
fn trial_dirs(dir: &std::path::Path) {
    for (module, var) in [
        ("fawkes", "FAWKES_DATA_DIR"),
        ("owlery", "OWLERY_DATA_DIR"),
        ("grimoire", "GRIMOIRE_DATA_DIR"),
        ("menagerie", "MENAGERIE_DATA_DIR"),
        ("patronus", "PATRONUS_DATA_DIR"),
    ] {
        let d = dir.join("essai").join(module);
        let _ = std::fs::create_dir_all(&d);
        if std::env::var_os(var).is_none() {
            unsafe { std::env::set_var(var, &d) };
        }
    }
}

fn message_box(text: &str) {
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::UI::WindowsAndMessaging::{MB_ICONINFORMATION, MB_OK, MessageBoxW};
        let w = |s: &str| s.encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>();
        MessageBoxW(std::ptr::null_mut(), w(text).as_ptr(), w(app::TITLE).as_ptr(), MB_OK | MB_ICONINFORMATION);
    }
    #[cfg(not(windows))]
    eprintln!("{text}");
}

fn main() -> eframe::Result {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // Menagerie relance son exe en relais (`current_exe() --relais …`) : dans la suite, c'est celui-ci.
    if args.first().map(String::as_str) == Some("--relais") {
        std::process::exit(menagerie::relay::main(&args));
    }
    // Patronus inscrit `current_exe() planifie <tâche>` dans le Planificateur de Windows.
    if let Some(code) = suite_patronus::cli(&args) {
        std::process::exit(code);
    }
    wait_for_previous(&args);

    let dir = persist::data_dir();
    let lock_name = format!("Local\\Eyrie-instance:{}", suite_sdk::instance::dir_key(&dir));
    let Some(_instance) = suite_sdk::instance::NamedMutex::try_acquire(&lock_name) else {
        message_box("Eyrie est déjà ouvert.");
        return Ok(());
    };
    // Reprise des données de SuperApp, pour le dossier de tous les jours seulement (pas pour un Eyrie isolé par
    // `EYRIE_DATA_DIR` ou `EYRIESUITE_DIR`).
    let mut problems = Vec::new();
    if dir == persist::default_data_dir() && std::env::var_os(suite_accounts::DIR_ENV).is_none() {
        let old = dirs::config_dir().unwrap_or_else(std::env::temp_dir).join("SuperApp");
        let old_vault = suite_accounts::OsVault::new("SuperApp");
        if let Err(e) = migrate::from_superapp(&old, &dir, &old_vault, &suite_accounts::Store::open()) {
            problems.push(format!("Reprise des données de SuperApp impossible : {e}"));
        }
    }
    let _ = std::fs::create_dir_all(&dir);
    if std::env::var_os("FAWKES_TAILNET_EXE").is_none()
        && let Some(companion) = tailnet::find_companion()
    {
        // Avant tout autre fil : le compagnon de Fawkes est cherché par cette variable.
        unsafe { std::env::set_var("FAWKES_TAILNET_EXE", companion) };
    }
    sandbox::install_panic_hook();

    let mut loaded = persist::load(&dir);
    loaded.problems.extend(problems);
    if loaded.value.module_data == persist::ModuleData::Trial {
        trial_dirs(&dir);
    }
    let geom = loaded.value.window;
    let mut viewport = egui::ViewportBuilder::default()
        .with_title(app::TITLE)
        .with_app_id("eyrie")
        .with_inner_size(geom.size)
        .with_min_inner_size([900.0, 560.0])
        .with_maximized(geom.maximized)
        .with_icon(std::sync::Arc::new(icon::window_icon()));
    let pos = geom.pos.filter(|p| screens::on_some_monitor(*p));
    if let Some(p) = pos {
        viewport = viewport.with_position(p);
    }
    let options = eframe::NativeOptions {
        viewport,
        centered: pos.is_none(),
        renderer: eframe::Renderer::Glow,
        // La suite enregistre elle-même sa fenêtre et ses dispositions.
        persist_window: false,
        persistence_path: Some(dir.join("egui.ron")),
        ..Default::default()
    };
    eframe::run_native(
        app::TITLE,
        options,
        Box::new(move |cc| {
            let app = app::Suite::new(cc, dir, loaded);
            cc.egui_ctx.request_repaint_after(Duration::from_millis(16));
            Ok(Box::new(app))
        }),
    )
}
