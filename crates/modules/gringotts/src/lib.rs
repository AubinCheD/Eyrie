//! Gringotts : la consommation de Claude Code, conversation par conversation, et les limites des abonnements.
//!
//! - **Conversations** : toutes celles du disque (chaque profil de Claude Code), avec leurs jetons, leurs modèles,
//!   leur coût équivalent API et leur part des limites ; filtres par période, dépôt, compte, modèle, texte.
//! - **Limites** : l'historique des fenêtres de 5 h et des semaines (maximum atteint, heure des 100 %,
//!   dépassement), relevé par le relais (`relay`), et les limites en cours dans la barre d'état d'Eyrie.
//! - **Dans chaque terminal** où tourne Claude, la ligne d'état (le relais) montre la part de la conversation.
//!
//! Sorti de Grimoire (onglets Consommation et Limites) pour en faire un module à part. Ses données sont dans
//! `EyrieSuite\gringotts` : historique des relevés, résumé des conversations, parts déposées pour le relais.

pub mod analysis;
pub mod prices;
pub mod readings;
pub mod relay;
pub mod transcripts;
mod ui;

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use eframe::egui::{self, Color32};
use grimoire::profiles::Profile;
use suite_sdk::{
    Badge, BuildCx, CommandInfo, Cx, Host, Module, ModuleFactory, ModuleInfo, ModuleStatus, Origin, StatusText, Tone,
    Unavailable, ViewInfo,
};

use crate::analysis::Analysis;
use crate::readings::{Kind, Reading};
use crate::transcripts::{Cache, Conversation};

pub const ID: &str = "gringotts";

pub fn info() -> ModuleInfo {
    ModuleInfo {
        id: ID,
        name: "Gringotts",
        icon: "💰",
        logo: None,
        color: Color32::from_rgb(0x3f, 0xb9, 0x8a),
        summary: "Consommation de Claude : conversations, coûts, limites des abonnements",
        origin: Origin::Native,
    }
}

pub fn factory() -> ModuleFactory {
    ModuleFactory { info: info(), build, fonts: None }
}

fn build(bcx: &BuildCx) -> Result<Box<dyn Module>, Unavailable> {
    // En essai, ses fichiers vont dans le dossier d'essai (le relais, lancé par Claude Code, écrit toujours dans le vrai).
    let dir = match &bcx.data {
        suite_sdk::DataMode::Shared => relay::data_dir(),
        suite_sdk::DataMode::Trial(d) => d.clone(),
    };
    Ok(Box::new(Gringotts::open(bcx.host.clone(), dir, known_profiles(), true)))
}

pub(crate) fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// Les profils de Claude Code : ceux de Grimoire (lus sans y toucher), le profil par défaut, les dossiers
/// `~/.claude-*` trouvés.
pub fn known_profiles() -> Vec<Profile> {
    let mut v: Vec<Profile> = grimoire::paths::read_json(&grimoire::state::data_dir().join("state.json"))
        .ok()
        .flatten()
        .and_then(|s| s.get("profiles").cloned())
        .and_then(|p| serde_json::from_value(p).ok())
        .unwrap_or_default();
    if !v.iter().any(|p| p.config_dir.is_none()) {
        v.insert(0, Profile::default_profile());
    }
    for dir in grimoire::profiles::discover(&grimoire::paths::home_dir(), &v) {
        let name = dir.file_name().map(|n| n.to_string_lossy().trim_start_matches('.').to_string()).unwrap_or_default();
        v.push(Profile { id: grimoire::paths::key(&dir), name, config_dir: Some(dir) });
    }
    v
}

/// La lecture des transcriptions, dans un fil : relit ce qui a changé toutes les minutes, ou sur demande.
struct Scanner {
    rx: mpsc::Receiver<Vec<Conversation>>,
    wake: mpsc::Sender<Vec<PathBuf>>,
}

impl Scanner {
    fn spawn(cache_file: PathBuf, configs: Vec<PathBuf>, ctx: egui::Context) -> Scanner {
        let (tx, rx) = mpsc::channel();
        let (wake, wake_rx) = mpsc::channel::<Vec<PathBuf>>();
        std::thread::Builder::new()
            .name("gringotts-lecture".into())
            .spawn(move || {
                let mut cache = Cache::load(&cache_file);
                let mut configs = configs;
                // Ce qui est déjà connu, tout de suite ; puis la relecture.
                if !cache.files.is_empty() && tx.send(cache.conversations()).is_err() {
                    return;
                }
                loop {
                    let read = cache.refresh(&configs);
                    if read > 0 {
                        let _ = cache.save(&cache_file);
                    }
                    if tx.send(cache.conversations()).is_err() {
                        return;
                    }
                    ctx.request_repaint();
                    match wake_rx.recv_timeout(Duration::from_secs(60)) {
                        Ok(new) => configs = new,
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                        Err(mpsc::RecvTimeoutError::Disconnected) => return,
                    }
                }
            })
            .expect("fil de lecture");
        Scanner { rx, wake }
    }
}

pub struct Gringotts {
    info: ModuleInfo,
    host: Host,
    dir: PathBuf,
    profiles: Vec<Profile>,
    scanner: Option<Scanner>,
    conversations: Vec<Conversation>,
    /// Heure de la dernière lecture reçue.
    scanned: Option<Instant>,
    readings: Vec<Reading>,
    /// Taille et date de l'historique des relevés lu.
    readings_stamp: Option<(u64, SystemTime)>,
    last_check: Option<Instant>,
    analysis: Analysis,
    stale: bool,
    parts_saved: Option<Instant>,
    view: ui::ViewState,
    /// Résultat de la dernière installation ou du dernier retrait du relais.
    message: Option<(String, bool)>,
}

impl Gringotts {
    /// `scan` : lire les transcriptions dans un fil (faux dans les essais, qui donnent les conversations).
    pub fn open(host: Host, dir: PathBuf, profiles: Vec<Profile>, scan: bool) -> Self {
        let configs = profiles.iter().map(|p| p.paths().config_dir).collect();
        let scanner = scan.then(|| Scanner::spawn(dir.join("conversations.json"), configs, host.ctx().clone()));
        Gringotts {
            info: info(),
            host,
            dir,
            profiles,
            scanner,
            conversations: Vec::new(),
            scanned: None,
            readings: Vec::new(),
            readings_stamp: None,
            last_check: None,
            analysis: Analysis::default(),
            stale: true,
            parts_saved: None,
            view: ui::ViewState::default(),
            message: None,
        }
    }

    /// Essais : des conversations et des relevés donnés.
    pub fn set_data(&mut self, conversations: Vec<Conversation>, readings: Vec<Reading>) {
        self.conversations = conversations;
        self.readings = readings;
        self.scanned = Some(Instant::now());
        self.stale = true;
        self.refresh_analysis();
    }

    pub fn analysis(&self) -> &Analysis {
        &self.analysis
    }

    fn refresh_analysis(&mut self) {
        if self.stale {
            self.analysis = analysis::analyze(&self.readings, &self.conversations, now());
            self.stale = false;
        }
    }

    /// Relit l'historique des relevés s'il a changé.
    fn check_readings(&mut self) {
        let file = readings::history_file(&self.dir);
        let stamp = std::fs::metadata(&file).ok().and_then(|m| Some((m.len(), m.modified().ok()?)));
        if stamp != self.readings_stamp {
            self.readings = readings::load(&self.dir);
            self.readings_stamp = stamp;
            self.stale = true;
        }
    }

    fn rescan(&mut self) {
        if let Some(s) = &self.scanner {
            let _ = s.wake.send(self.profiles.iter().map(|p| p.paths().config_dir).collect());
        }
    }

    fn install(&mut self, profile: &Profile) {
        let result = std::env::current_exe().map_err(|e| e.to_string()).and_then(|exe| {
            // Le relais de Grimoire garde ce qu'il rejoue dans son dossier local (`grimoire-statusline.exe`).
            relay::install(&profile.paths(), &relay::command(&exe), &self.dir, &grimoire::state::local_data_dir())
        });
        self.message = Some(match result {
            Ok(backup) => (
                format!(
                    "Relais installé pour « {} ». Il relève les limites à chaque mise à jour de la ligne d'état de Claude \
                     Code (dans les sessions ouvertes après l'installation).{}",
                    profile.name,
                    backup.map(|b| format!(" Copie des réglages d'avant : {}", b.display())).unwrap_or_default()
                ),
                true,
            ),
            Err(e) => (format!("Installation impossible pour « {} » : {e}", profile.name), false),
        });
    }

    fn uninstall(&mut self, profile: &Profile) {
        self.message = Some(match relay::uninstall(&profile.paths(), &self.dir) {
            Ok(()) => (format!("Relais retiré de « {} » : sa ligne d'état d'avant est remise.", profile.name), true),
            Err(e) => (format!("Retrait impossible pour « {} » : {e}", profile.name), false),
        });
    }

    /// Les limites en cours, pour la barre d'état : le compte relevé en dernier, et le détail de tous en infobulle.
    fn status_text(&self) -> StatusText {
        let now = now();
        let Some(a) = self.analysis.accounts.first() else {
            return StatusText {
                text: "Limites Claude : relais à installer".into(),
                tone: Tone::Info,
                tip: "Gringotts relève les limites par la ligne d'état de Claude Code : Réglages › Gringotts.".into(),
            };
        };
        let mut worst: f64 = 0.0;
        let mut parts = Vec::new();
        for kind in Kind::ALL {
            if let Some(w) = a.last.window(kind) {
                let p = current(w, now);
                worst = worst.max(p);
                parts.push(format!("{} {}", kind.label(), readings::pct_text(p)));
            }
        }
        if let Some(w) = a.last.five_hour.filter(|w| w.resets_at > now) {
            parts.push(format!("↻ {}", readings::local(w.resets_at, "%H:%M")));
        }
        let tone = if worst >= 100.0 {
            Tone::Danger
        } else if worst >= 80.0 {
            Tone::Warn
        } else {
            Tone::Ok
        };
        let tip = self
            .analysis
            .accounts
            .iter()
            .map(|a| {
                let windows: Vec<String> = Kind::ALL
                    .iter()
                    .filter_map(|k| {
                        let w = a.last.window(*k)?;
                        let fmt = if *k == Kind::FiveHour { "%H:%M" } else { "%a %d/%m %H:%M" };
                        Some(format!(
                            "{} : {} (remise à zéro {})",
                            k.label(),
                            readings::pct_text(current(w, now)),
                            readings::local(w.resets_at, fmt)
                        ))
                    })
                    .collect();
                format!("{}\n  {}\n  relevé {}", a.label, windows.join("\n  "), ui::ago(a.last.t, now))
            })
            .collect::<Vec<_>>()
            .join("\n");
        StatusText { text: format!("Claude {}", parts.join(" · ")), tone, tip }
    }
}

/// Part utilisée d'une fenêtre maintenant : 0 si elle s'est remise à zéro depuis le relevé.
pub fn current(w: readings::Window, now: i64) -> f64 {
    if w.resets_at <= now { 0.0 } else { w.pct }
}

impl Module for Gringotts {
    fn info(&self) -> &ModuleInfo {
        &self.info
    }

    fn views(&self) -> Vec<ViewInfo> {
        vec![ViewInfo::new(ViewInfo::MAIN, "Conversations"), ViewInfo::new("limites", "Limites")]
    }

    fn logic(&mut self, ctx: &egui::Context, cx: &mut Cx) {
        if let Some(s) = &self.scanner {
            let mut latest = None;
            while let Ok(c) = s.rx.try_recv() {
                latest = Some(c);
            }
            if let Some(c) = latest {
                self.conversations = c;
                self.scanned = Some(Instant::now());
                self.stale = true;
            }
        }
        if self.last_check.is_none_or(|t| t.elapsed() >= Duration::from_secs(2)) {
            self.last_check = Some(Instant::now());
            self.check_readings();
        }
        self.refresh_analysis();
        // Les parts des fenêtres en cours, pour le relais (toutes les 30 s).
        if self.parts_saved.is_none_or(|t| t.elapsed() >= Duration::from_secs(30)) && !self.analysis.accounts.is_empty()
        {
            self.parts_saved = Some(Instant::now());
            let mut parts = self.analysis.parts.clone();
            parts.written_at = now();
            let _ = parts.save(&self.dir);
        }
        // La barre d'état et les vues suivent les relevés (et les remises à zéro).
        ctx.request_repaint_after(if cx.visible { Duration::from_secs(2) } else { Duration::from_secs(20) });
    }

    fn ui(&mut self, ui: &mut egui::Ui, view: &str, _cx: &mut Cx) {
        let p = self.host.palette();
        let action = egui::Frame::new().inner_margin(egui::Margin::same(10)).show(ui, |ui| {
            let data = ui::Data {
                conversations: &self.conversations,
                analysis: &self.analysis,
                profiles: &self.profiles,
                scanned: self.scanned,
                scanning: self.scanner.is_some() && self.scanned.is_none(),
            };
            match view {
                "limites" => ui::limits(ui, p, &data, &mut self.view),
                _ => ui::conversations(ui, p, &data, &mut self.view),
            }
        });
        match action.inner {
            Some(ui::Action::Rescan) => self.rescan(),
            Some(ui::Action::Install(i)) => {
                if let Some(pr) = self.profiles.get(i).cloned() {
                    self.install(&pr);
                }
            }
            Some(ui::Action::OpenSettings) => self.host.open_settings(&format!("module:{ID}")),
            None => {}
        }
    }

    fn status(&self) -> ModuleStatus {
        let now = now();
        let hot = self
            .analysis
            .accounts
            .iter()
            .any(|a| Kind::ALL.iter().any(|k| a.last.window(*k).is_some_and(|w| current(w, now) >= 90.0)));
        ModuleStatus {
            title: None,
            badge: hot.then(|| Badge {
                count: None,
                tone: Tone::Warn,
                tip: "Une limite de Claude dépasse 90 %".into(),
            }),
            waiting: Vec::new(),
            status_line: Some(self.status_text()),
        }
    }

    fn commands(&self) -> Vec<CommandInfo> {
        vec![CommandInfo { id: "relire".into(), title: "Relire les conversations".into(), shortcut: None }]
    }

    fn run_command(&mut self, id: &str, _cx: &mut Cx) {
        if id == "relire" {
            self.rescan();
        }
    }

    fn settings_ui(&mut self, ui: &mut egui::Ui, host: &Host) -> bool {
        let p = host.palette();
        let profiles = self.profiles.clone();
        match ui::settings(ui, p, &profiles, &self.dir, self.message.as_ref()) {
            Some(ui::SettingsAction::Install(i)) => self.install(&profiles[i]),
            Some(ui::SettingsAction::Uninstall(i)) => self.uninstall(&profiles[i]),
            None => {}
        }
        true
    }
}

/// Le dossier `dir` existe-t-il comme dossier de configuration de Claude Code ?
pub fn is_config_dir(dir: &Path) -> bool {
    dir.join("projects").is_dir() || dir.join("settings.json").is_file()
}

#[cfg(test)]
mod tests;
