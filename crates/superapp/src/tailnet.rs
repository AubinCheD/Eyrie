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

/// Où trouver le compagnon : `FAWKES_TAILNET_EXE`, à côté de `superapp.exe`, ou compilé dans le
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
        candidates.push(repo.join(profile).join(fawkes_tailnet::COMPANION));
    }
    candidates.into_iter().find(|p| p.is_file())
}

pub fn hostname(settings: &TailnetSettings) -> String {
    let chosen = if settings.hostname.trim().is_empty() { format!("suite-{}", fawkes_tailnet::computer_name()) } else { settings.hostname.clone() };
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
}
