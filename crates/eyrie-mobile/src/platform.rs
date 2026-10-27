//! Ce qui dépend du système : dossier de données, nom de l'appareil, liens, vibration, caméra. Le bureau sert au
//! développement (fenêtre de la taille d'un téléphone) ; Android est dans `android.rs`.

use std::path::PathBuf;

/// Une image de la caméra, prête pour l'interface.
pub type Frame = slint::SharedPixelBuffer<slint::Rgba8Pixel>;

#[cfg(target_os = "android")]
pub use crate::android::{can_scan, data_dir, device_model, device_name, haptic, open_url, start_scan, stop_scan};

#[cfg(not(target_os = "android"))]
mod desktop {
    use super::*;

    /// `EYRIE_MOBILE_DATA`, sinon `%LOCALAPPDATA%\EyrieMobile`.
    pub fn data_dir() -> PathBuf {
        std::env::var_os("EYRIE_MOBILE_DATA")
            .filter(|d| !d.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| dirs::data_local_dir().unwrap_or_else(std::env::temp_dir).join("EyrieMobile"))
    }

    pub fn device_name() -> String {
        let pc = std::env::var("COMPUTERNAME").unwrap_or_default();
        if pc.is_empty() { "Eyrie mobile (bureau)".into() } else { format!("Eyrie mobile ({pc})") }
    }

    pub fn device_model() -> String {
        format!("bureau, {}", std::env::consts::OS)
    }

    pub fn open_url(url: &str) {
        // Seulement des liens web : jamais un fichier ou un programme local.
        if !(url.starts_with("https://") || url.starts_with("http://")) {
            return;
        }
        #[cfg(windows)]
        let _ = std::process::Command::new("rundll32").args(["url.dll,FileProtocolHandler", url]).spawn();
    }

    pub fn haptic() {}

    pub fn can_scan() -> bool {
        false
    }

    pub fn start_scan(_on_frame: Box<dyn Fn(Frame) + Send>, _on_code: Box<dyn Fn(String) + Send>) {}

    pub fn stop_scan() {}
}

#[cfg(not(target_os = "android"))]
pub use desktop::*;
