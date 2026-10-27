//! Tailscale de la suite (synchro mobile) : **un seul nœud** pour la suite, au nom générique
//! (`suite-<poste>`), état dans `<suite>\tailnet`. Le compagnon de Fawkes (`fawkes-tailnet.exe`, en
//! Go avec tsnet) devient celui de la suite : son pilotage est repris tel quel de Fawkes
//! (`suite_fawkes::tailnet`, compilé depuis le dépôt voisin). Fawkes lancé seul garde son nœud.

use std::path::{Path, PathBuf};

use eframe::egui::{self, Color32};
use suite_fawkes::tailnet::{self as fawkes_tailnet, TailnetState};

use crate::i18n::tr;
use crate::persist::TailnetSettings;

pub struct Tailnet {
    running: Option<(fawkes_tailnet::Tailnet, String)>,
}

/// Où trouver le compagnon : `FAWKES_TAILNET_EXE`, à côté de `eyrie.exe`, ou compilé dans le
/// dépôt voisin de Fawkes (`tools\compagnon.ps1`).
pub fn find_companion() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("FAWKES_TAILNET_EXE") {
        return Some(PathBuf::from(p));
    }
    let mut candidates = Vec::new();
    if let Some(dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(Path::to_path_buf)) {
        candidates.push(dir.join(fawkes_tailnet::COMPANION));
    }
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../Fawkes/target");
    for profile in ["release", "debug"] {
        let built = repo.join(profile).join(fawkes_tailnet::COMPANION);
        // Le dépôt voisin, à son chemin de compilation, seulement s'il est dans le profil de l'utilisateur : ailleurs
        // (`D:\build\…`, `C:\agent\…`), un autre compte du poste pourrait créer ce chemin et y déposer un faux
        // compagnon, qu'Eyrie lancerait sous le compte de l'utilisateur.
        if dirs::home_dir().is_some_and(|home| inside(&built, &home)) {
            candidates.push(built);
        }
    }
    candidates.into_iter().find(|p| p.is_file())
}

/// `path` est-il dans `dir` (sans `..`, casse ignorée comme sous Windows) ?
fn inside(path: &Path, dir: &Path) -> bool {
    let norm = |p: &Path| {
        let mut out = Vec::new();
        for c in p.components() {
            match c {
                std::path::Component::ParentDir => {
                    out.pop();
                }
                std::path::Component::CurDir => {}
                c => out.push(c.as_os_str().to_string_lossy().to_lowercase()),
            }
        }
        out
    };
    norm(path).starts_with(&norm(dir))
}

pub fn hostname(settings: &TailnetSettings) -> String {
    let chosen = if settings.hostname.trim().is_empty() {
        format!("suite-{}", fawkes_tailnet::computer_name())
    } else {
        settings.hostname.clone()
    };
    fawkes_tailnet::hostname_for(&chosen, &fawkes_tailnet::computer_name())
}

impl Tailnet {
    pub fn new(_dir: &Path) -> Self {
        Tailnet { running: None }
    }

    /// Démarre, arrête ou redémarre le compagnon selon les réglages.
    pub fn poll(&mut self, settings: &TailnetSettings, dir: &Path, ctx: &egui::Context) {
        let wanted = settings.enabled.then(|| hostname(settings));
        let current = self.running.as_ref().map(|(_, h)| h.clone());
        if wanted == current {
            return;
        }
        self.stop();
        if let Some(h) = wanted {
            let state_dir = dir.join("tailnet");
            let _ = std::fs::create_dir_all(&state_dir);
            self.running = Some((fawkes_tailnet::Tailnet::start(&state_dir, &h, ctx.clone()), h));
        }
    }

    pub fn state(&self) -> Option<TailnetState> {
        self.running.as_ref().map(|(t, _)| t.state())
    }

    pub fn stop(&mut self) {
        if let Some((mut t, _)) = self.running.take() {
            t.stop();
        }
    }

    pub fn status_line(&self, p: &suite_sdk::theme::Palette) -> (String, Color32) {
        match self.state() {
            None => (format!("⛓ {}", tr("Tailscale coupé")), p.text_dim),
            Some(TailnetState::Starting) => (format!("⛓ {}", tr("Tailscale : démarrage")), p.text_muted),
            Some(TailnetState::NeedsLogin(_)) => (format!("⛓ {}", tr("Tailscale : connexion à faire")), p.warn),
            Some(TailnetState::Running { name, .. }) => (format!("⛓ {name}"), p.ok),
            Some(TailnetState::Failed(_)) => (format!("⛓ {}", tr("Tailscale : erreur")), p.danger),
        }
    }
}

impl Drop for Tailnet {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_node_name_is_generic() {
        let h = hostname(&TailnetSettings { enabled: true, hostname: String::new() });
        assert!(h.starts_with("suite"), "{h}");
        assert_eq!(hostname(&TailnetSettings { enabled: true, hostname: "Mon Poste".into() }), "mon-poste");
    }

    /// Le compagnon compilé dans le dépôt voisin n'est cherché que dans le profil de l'utilisateur.
    #[test]
    fn the_built_companion_is_looked_for_in_the_profile_only() {
        let home = Path::new(r"C:\Users\aubin");
        assert!(inside(Path::new(r"C:\Users\Aubin\DEV\Eyrie\crates\eyrie\../../../Fawkes/target/release/x.exe"), home));
        assert!(!inside(Path::new(r"D:\build\Eyrie\crates\eyrie\../../../Fawkes/target/release/x.exe"), home));
        assert!(!inside(Path::new(r"C:\Users\aubin\..\autre\Fawkes\x.exe"), home));
        assert!(!inside(Path::new(r"C:\Users\aubinette\x.exe"), home));
    }
}
