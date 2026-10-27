//! Eyrie sur le téléphone : piloter le PC par Tailscale (voir `docs/mobile.md`).
//!
//! - [`app`] : le contrôleur (état, navigation, gestes, messages du PC) ;
//! - [`net`] : la liaison avec le PC (reconnexion) et l'appairage ;
//! - [`termbuf`], [`view`] : ce que le PC envoie, prêt à dessiner ;
//! - [`store`] : identité, PC appairés, préférences ;
//! - [`platform`] : ce qui dépend du système (Android, ou le bureau pour le développement) ;
//! - `ui/*.slint` : l'interface.

slint::include_modules!();

#[cfg(target_os = "android")]
mod android;
pub mod app;
mod demo;
pub mod net;
pub mod platform;
pub mod store;
pub mod termbuf;
pub mod view;

#[path = "../../eyrie/src/nest.rs"]
#[allow(dead_code)]
mod nest;

/// Le nid d'Eyrie, en image.
pub fn logo() -> slint::Image {
    const SIZE: usize = 256;
    let px = nest::render(SIZE);
    let buf = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(&px, SIZE as u32, SIZE as u32);
    slint::Image::from_rgba8(buf)
}

/// Une image PNG reçue du PC (logo d'un module).
pub fn png_image(png: &[u8]) -> Option<slint::Image> {
    let img = image::load_from_memory_with_format(png, image::ImageFormat::Png).ok()?.to_rgba8();
    let (w, h) = img.dimensions();
    let buf = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(img.as_raw(), w, h);
    Some(slint::Image::from_rgba8(buf))
}

/// Lance l'application (la plate-forme est déjà prête).
pub fn run() -> Result<(), slint::PlatformError> {
    let ui = AppWindow::new()?;
    let demo = std::env::var("EYRIE_MOBILE_DEMO").ok().filter(|d| !d.is_empty());
    let dir = platform::data_dir();
    let store = match &demo {
        // La démonstration n'écrit rien dans les données de tous les jours.
        Some(_) => store::Store::open(&std::env::temp_dir().join("eyrie-mobile-demo")),
        None => store::Store::open(&dir),
    };
    app::Controller::install(&ui, store, demo.is_some());
    if let Some(scene) = demo {
        demo::play(&ui, &scene);
    }
    // Développement : un lien d'appairage à utiliser tout de suite, l'onglet à ouvrir une fois connecté.
    if let Ok(uri) = std::env::var("EYRIE_MOBILE_PAIR")
        && !uri.is_empty()
    {
        ui.set_code_link(uri.into());
        app::with(|c| c.submit_code());
    }
    if let Ok(goto) = std::env::var("EYRIE_MOBILE_GOTO") {
        app::with(|c| c.goto_when_online(goto));
    }
    #[cfg(not(target_os = "android"))]
    if let Some(path) = std::env::var_os("EYRIE_MOBILE_CAPTURE") {
        capture(&ui, std::path::PathBuf::from(path));
    }
    ui.run()
}

/// Capture de la fenêtre puis fermeture (essais, documentation).
#[cfg(not(target_os = "android"))]
fn capture(ui: &AppWindow, path: std::path::PathBuf) {
    let weak = ui.as_weak();
    let delay = std::env::var("EYRIE_MOBILE_CAPTURE_AFTER_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(1500);
    slint::Timer::single_shot(std::time::Duration::from_millis(delay), move || {
        let Some(ui) = weak.upgrade() else { return };
        match ui.window().take_snapshot() {
            Ok(buf) => {
                let img = image::RgbaImage::from_raw(buf.width(), buf.height(), buf.as_bytes().to_vec());
                if let Some(img) = img
                    && let Err(e) = img.save(&path)
                {
                    eprintln!("capture : {e}");
                }
            }
            Err(e) => eprintln!("capture : {e}"),
        }
        let _ = slint::quit_event_loop();
    });
}

#[cfg(target_os = "android")]
#[unsafe(no_mangle)]
fn android_main(app: slint::android::AndroidApp) {
    android::init(app.clone());
    let listener = |event: &slint::android::android_activity::PollEvent<'_>| {
        if let slint::android::android_activity::PollEvent::Main(
            slint::android::android_activity::MainEvent::Resume { .. },
        ) = event
        {
            app::with(|c| c.resumed());
        }
    };
    if slint::android::init_with_event_listener(app, listener).is_ok() {
        let _ = run();
    }
}
