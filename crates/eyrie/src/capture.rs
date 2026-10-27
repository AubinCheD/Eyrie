//! Capture d'écran d'essai : `EYRIE_CAPTURE=<fichier.png>` enregistre la fenêtre principale après
//! `EYRIE_CAPTURE_AFTER_MS` (4 s par défaut), puis ferme la suite. `EYRIE_CAPTURE_SCENE`
//! prépare la scène : `palette`, `reglages`, `detacher`, `ensemble`, `concentration`, `telephones`, `appairer`.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use eframe::egui;

pub struct Capture {
    pub path: PathBuf,
    after: Duration,
    started: Instant,
    requested: bool,
    done: Arc<AtomicBool>,
    pub scene: Option<String>,
}

impl Capture {
    pub fn from_env() -> Option<Capture> {
        let path = std::env::var_os("EYRIE_CAPTURE").map(PathBuf::from)?;
        let after = std::env::var("EYRIE_CAPTURE_AFTER_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(4000);
        Some(Capture {
            path,
            after: Duration::from_millis(after),
            started: Instant::now(),
            requested: false,
            done: Arc::new(AtomicBool::new(false)),
            scene: std::env::var("EYRIE_CAPTURE_SCENE").ok(),
        })
    }

    /// `true` quand la capture est enregistrée : la suite peut se fermer.
    pub fn poll(&mut self, ctx: &egui::Context) -> bool {
        if self.done.load(Ordering::Relaxed) {
            return true;
        }
        if !self.requested && self.started.elapsed() >= self.after {
            self.requested = true;
            let (path, done, wake) = (self.path.clone(), self.done.clone(), ctx.clone());
            ctx.request_screenshot(move |img| {
                let [w, h] = img.size;
                let bytes: Vec<u8> = img.pixels.iter().flat_map(|c| c.to_array()).collect();
                match image::RgbaImage::from_raw(w as u32, h as u32, bytes) {
                    Some(buf) => {
                        if let Err(e) = buf.save(&path) {
                            eprintln!("capture : {e}");
                        }
                    }
                    None => eprintln!("capture : image vide"),
                }
                done.store(true, Ordering::Relaxed);
                wake.request_repaint();
            });
        }
        ctx.request_repaint_after(Duration::from_millis(100));
        false
    }
}
