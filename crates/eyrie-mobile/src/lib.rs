//! Eyrie sur le téléphone : piloter le PC par Tailscale (voir `docs/mobile.md`).
//!
//! - [`app`] : le contrôleur (état, navigation, gestes, messages du PC) ;
//! - [`net`] : la liaison avec le PC (reconnexion) et l'appairage ;
//! - [`tailnet`] : le nœud Tailscale de l'application, par où passent les connexions (Android) ;
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
pub mod tailnet;
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

/// Les icônes Android dans `res` : le premier plan de l'icône adaptative (le nid dans la zone sûre, 66 dp sur 108)
/// et une icône carrée de secours, à chaque densité.
#[cfg(not(target_os = "android"))]
pub fn write_icons(res: &std::path::Path) -> Result<(), String> {
    for (dir, scale) in [("mdpi", 1.0), ("hdpi", 1.5), ("xhdpi", 2.0), ("xxhdpi", 3.0), ("xxxhdpi", 4.0)] {
        let out = res.join(format!("mipmap-{dir}"));
        std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
        // Premier plan : 108 dp, le nid sur 64 dp au centre.
        let side = (108.0 * scale) as u32;
        let nest = (64.0 * scale) as u32;
        let px = nest::render(nest as usize);
        let nest_img = image::RgbaImage::from_raw(nest, nest, px).ok_or("nid")?;
        let mut fg = image::RgbaImage::new(side, side);
        let off = ((side - nest) / 2) as i64;
        image::imageops::overlay(&mut fg, &nest_img, off, off);
        fg.save(out.join("ic_launcher_foreground.png")).map_err(|e| e.to_string())?;
        // Secours : 48 dp, fond sombre arrondi.
        let side = (48.0 * scale) as u32;
        let nest = (36.0 * scale) as u32;
        let nest_img = image::RgbaImage::from_raw(nest, nest, nest::render(nest as usize)).ok_or("nid")?;
        let r = side as f32 * 0.22;
        let mut legacy = image::RgbaImage::from_fn(side, side, |x, y| {
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            let (cx, cy) = (fx.clamp(r, side as f32 - r), fy.clamp(r, side as f32 - r));
            let inside = (fx - cx).hypot(fy - cy) <= r;
            if inside { image::Rgba([0x16, 0x17, 0x1b, 255]) } else { image::Rgba([0, 0, 0, 0]) }
        });
        let off = ((side - nest) / 2) as i64;
        image::imageops::overlay(&mut legacy, &nest_img, off, off);
        legacy.save(out.join("ic_launcher.png")).map_err(|e| e.to_string())?;
    }
    Ok(())
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
    if demo.is_none() {
        // Le nœud Tailscale (Android) démarre tout de suite : il se connecte pendant que l'interface s'affiche.
        let name =
            if store.prefs.device_name.is_empty() { platform::device_name() } else { store.prefs.device_name.clone() };
        tailnet::start(dir.join("tailnet"), tailnet::hostname(&name));
    }
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
    use slint::android::android_activity::{MainEvent, PollEvent};
    let listener = |event: &PollEvent<'_>| match event {
        PollEvent::Main(MainEvent::Resume { .. }) => {
            android::set_foreground(true);
            app::with(|c| c.resumed());
        }
        PollEvent::Main(MainEvent::Pause) => android::set_foreground(false),
        _ => {}
    };
    if slint::android::init_with_event_listener(app, listener).is_ok() {
        let _ = run();
    }
}
