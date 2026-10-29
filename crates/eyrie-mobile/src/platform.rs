//! Ce qui dépend du système : dossier de données, nom de l'appareil, liens, vibration, caméra. Le bureau sert au
//! développement (fenêtre de la taille d'un téléphone) ; Android est dans `android.rs`.

/// Une image de la caméra, prête pour l'interface.
pub type Frame = slint::SharedPixelBuffer<slint::Rgba8Pixel>;

/// Une notification du système (application en arrière-plan) ; la toucher rouvre l'application. Sa forme discrète, sans
/// rien du texte du PC (le nom du module et ce qui attend), est la seule que lit qui tient le téléphone verrouillé.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Notice {
    /// Une même étiquette remplace la notification précédente au lieu de s'ajouter.
    pub tag: String,
    pub title: String,
    pub body: String,
    pub discreet_title: String,
    pub discreet_body: String,
}

/// La visibilité d'une notification sur l'écran verrouillé (les valeurs d'Android) : montrée telle quelle, ou pas du
/// tout.
pub const VISIBILITY_PUBLIC: i32 = 1;
pub const VISIBILITY_SECRET: i32 = -1;

impl Notice {
    /// Ce que la notification montre, et sa visibilité sur l'écran verrouillé. Quelqu'un regarde le téléphone (écran
    /// allumé, déverrouillé) : le détail, que l'écran verrouillé ne montrera pas ensuite. Sinon : la forme discrète,
    /// même avec le réglage d'Android le plus courant (« afficher tout le contenu des notifications »), qui montrerait
    /// le détail à qui tient le téléphone verrouillé.
    pub fn shown(&self, looking: bool) -> (&str, &str, i32) {
        if looking {
            (&self.title, &self.body, VISIBILITY_SECRET)
        } else {
            (&self.discreet_title, &self.discreet_body, VISIBILITY_PUBLIC)
        }
    }
}

#[cfg(target_os = "android")]
pub use crate::android::{
    ask_notifications, can_scan, data_dir, device_model, device_name, haptic, in_background, notify, open_url,
    start_scan, stop_scan,
};

#[cfg(not(target_os = "android"))]
mod desktop {
    use std::path::PathBuf;

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

    /// Sur le bureau, l'application est toujours « au premier plan » : les bulles suffisent.
    pub fn in_background() -> bool {
        false
    }

    pub fn notify(_notice: &Notice) {}

    pub fn ask_notifications() {}
}

#[cfg(not(target_os = "android"))]
pub use desktop::*;

#[cfg(test)]
mod tests {
    use super::*;

    /// Téléphone verrouillé ou écran éteint : la forme discrète, montrée sur l'écran verrouillé. Quelqu'un le regarde
    /// déverrouillé : le détail, que l'écran verrouillé ne montrera pas une fois le téléphone reverrouillé.
    #[test]
    fn the_lock_screen_only_gets_the_discreet_form() {
        let n = Notice {
            tag: "attente:fawkes:Claude attend : eyrie".into(),
            title: "Terminaux".into(),
            body: "Claude attend : eyrie".into(),
            discreet_title: "Terminaux".into(),
            discreet_body: "Quelque chose vous attend sur le PC.".into(),
        };
        assert_eq!(n.shown(false), ("Terminaux", "Quelque chose vous attend sur le PC.", VISIBILITY_PUBLIC));
        assert_eq!(n.shown(true), ("Terminaux", "Claude attend : eyrie", VISIBILITY_SECRET));
        assert_eq!((VISIBILITY_PUBLIC, VISIBILITY_SECRET), (1, -1), "les valeurs d'Android");
    }
}
