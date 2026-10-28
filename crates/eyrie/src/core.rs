//! Le cœur de l'hôte : les modules (leur case de vie), le journal, les notifications, le clavier.

use std::collections::{BTreeSet, VecDeque};
use std::time::{Duration, Instant, SystemTime};

use eframe::egui;
use suite_sdk::host::Notification;
use suite_sdk::{CloseCheck, Cx, Host, Module, ModuleInfo, Tone, Unavailable};

use crate::i18n::{mark, tr};
use crate::sandbox::{self, Observed, Policy, WindowWish};

/// Où en est un module.
pub enum SlotState {
    Running(Box<dyn Module>),
    /// Désactivé dans les réglages : jamais construit, ne coûte rien.
    Disabled,
    Unavailable(Unavailable),
    /// A paniqué : plus jamais appelé jusqu'au prochain lancement de la suite.
    Crashed(String),
}

/// Temps passé dans un module (un seul processus pour tous : utile de savoir qui coûte).
#[derive(Clone, Debug, Default)]
pub struct Stats {
    pub logic_avg: f32,
    pub ui_avg: f32,
    pub peak: Duration,
    pub calls: u64,
    pub panics: u32,
    pub style_restored: u64,
    pub commands_countered: u64,
}

impl Stats {
    fn record(&mut self, kind: CallKind, o: &Observed) {
        let ms = o.elapsed.as_secs_f32() * 1000.0;
        let avg = match kind {
            CallKind::Logic => &mut self.logic_avg,
            _ => &mut self.ui_avg,
        };
        *avg = if self.calls == 0 { ms } else { *avg * 0.95 + ms * 0.05 };
        self.peak = self.peak.max(o.elapsed);
        self.calls += 1;
        self.style_restored += o.touched_style as u64;
        self.commands_countered += o.commands.len() as u64;
    }
}

pub struct Slot {
    pub info: ModuleInfo,
    pub build: fn(&suite_sdk::BuildCx) -> Result<Box<dyn Module>, Unavailable>,
    pub state: SlotState,
    pub stats: Stats,
    /// Débordements déjà signalés au journal (une fois par sorte).
    noted: BTreeSet<&'static str>,
}

impl Slot {
    pub fn new(info: ModuleInfo, build: fn(&suite_sdk::BuildCx) -> Result<Box<dyn Module>, Unavailable>) -> Self {
        Slot { info, build, state: SlotState::Disabled, stats: Stats::default(), noted: BTreeSet::new() }
    }

    /// Construit le module (au démarrage avec le `CreationContext` d'eframe ; plus tard, pour le
    /// relancer ou l'activer, avec un contexte sans fenêtre : les modules n'en lisent que
    /// `egui_ctx`, sauf l'icône de notification de Patronus, alors absente).
    pub fn start(
        &mut self,
        cc: &eframe::CreationContext<'_>,
        host: &Host,
        data: suite_sdk::DataMode,
        journal: &mut Journal,
    ) {
        self.stop();
        let bcx = suite_sdk::BuildCx { cc, host: host.clone(), data };
        let started = std::time::Instant::now();
        let build = self.build;
        self.state = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| build(&bcx))) {
            Ok(Ok(m)) => {
                journal.push(
                    self.info.id,
                    Tone::Ok,
                    format!("{} prêt ({} ms)", self.info.name, started.elapsed().as_millis()),
                );
                SlotState::Running(m)
            }
            Ok(Err(u)) => {
                journal.push(self.info.id, Tone::Warn, format!("{} indisponible : {u}", self.info.name));
                SlotState::Unavailable(u)
            }
            Err(_) => {
                let msg = "panique pendant la construction".to_string();
                journal.push(self.info.id, Tone::Danger, format!("{} : {msg}", self.info.name));
                SlotState::Crashed(msg)
            }
        };
    }

    /// Arrête le module (ses verrous et fichiers sont rendus), s'il tourne.
    pub fn stop(&mut self) {
        if let SlotState::Running(m) = &mut self.state {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| m.on_exit()));
        }
        // Un module arrêté par une panique est simplement libéré.
        self.state = SlotState::Disabled;
    }

    pub fn module(&self) -> Option<&dyn Module> {
        match &self.state {
            SlotState::Running(m) => Some(m.as_ref()),
            _ => None,
        }
    }

    pub fn module_mut(&mut self) -> Option<&mut (dyn Module + 'static)> {
        match &mut self.state {
            SlotState::Running(m) => Some(m.as_mut()),
            _ => None,
        }
    }

    pub fn is_running(&self) -> bool {
        matches!(self.state, SlotState::Running(_))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CallKind {
    Logic,
    Ui,
    HiddenUi,
    Other,
}

/// Une ligne du journal de la suite.
#[derive(Clone, Debug)]
pub struct LogLine {
    pub at: SystemTime,
    pub module: String,
    pub tone: Tone,
    pub text: String,
}

#[derive(Default)]
pub struct Journal {
    pub lines: VecDeque<LogLine>,
}

impl Journal {
    const MAX: usize = 500;

    pub fn push(&mut self, module: &str, tone: Tone, text: impl Into<String>) {
        if self.lines.len() == Self::MAX {
            self.lines.pop_front();
        }
        self.lines.push_back(LogLine { at: SystemTime::now(), module: module.to_string(), tone, text: text.into() });
    }
}

/// Le centre de notifications.
#[derive(Default)]
pub struct Notifications {
    pub items: VecDeque<(Notification, bool)>,
    /// Bulles affichées en bas à droite : (indice dans `items` à l'arrivée, apparition).
    pub toasts: Vec<(Notification, Instant)>,
}

impl Notifications {
    const MAX: usize = 200;

    pub fn push(&mut self, n: Notification) {
        if self.items.len() == Self::MAX {
            self.items.pop_back();
        }
        self.toasts.push((n.clone(), Instant::now()));
        self.items.push_front((n, false));
    }

    pub fn unread(&self) -> usize {
        self.items.iter().filter(|(_, read)| !read).count()
    }

    pub fn mark_all_read(&mut self) {
        for (_, read) in &mut self.items {
            *read = true;
        }
    }
}

/// Qui a le clavier : le module de la case qui a reçu le dernier clic, ou la suite (palette,
/// réglages, barre de titre).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Focus {
    pub module: Option<String>,
}

/// Appelle un module dans le bac à sable et tire les conséquences (pastille, plantage, journal).
#[allow(clippy::too_many_arguments)]
pub fn call(
    slot: &mut Slot,
    host: &Host,
    frame: &mut eframe::Frame,
    ctx: &egui::Context,
    kind: CallKind,
    policy: Policy,
    wish: &WindowWish,
    journal: &mut Journal,
    f: impl FnOnce(&mut dyn Module, &mut Cx),
) -> Option<Observed> {
    let SlotState::Running(module) = &mut slot.state else { return None };
    let mut cx = Cx { host, frame, active: policy.keyboard, visible: policy.visible };
    let observed = sandbox::call(ctx, policy, wish, || f(module.as_mut(), &mut cx));
    conclude(slot, host, kind, journal, &observed);
    Some(observed)
}

/// Dessine la page de réglages d'un module dans `ui` (réglages d'Eyrie), dans le bac à sable comme ses autres appels :
/// une panique l'arrête, lui seul. Vrai s'il a une page.
pub fn settings(slot: &mut Slot, host: &Host, ui: &mut egui::Ui, wish: &WindowWish, journal: &mut Journal) -> bool {
    let SlotState::Running(module) = &mut slot.state else { return false };
    let ctx = ui.ctx().clone();
    let mut shown = false;
    // Les champs de ses réglages reçoivent le clavier : la fenêtre de réglages l'a.
    let policy = Policy { keyboard: true, visible: true, sees_close: false };
    let observed = sandbox::call(&ctx, policy, wish, || shown = module.settings_ui(ui, host));
    conclude(slot, host, CallKind::Other, journal, &observed);
    shown && slot.is_running()
}

/// Sa page de réglages quittée (`Module::settings_left`), dans le bac à sable comme ses autres appels.
pub fn settings_left(slot: &mut Slot, host: &Host, ctx: &egui::Context, wish: &WindowWish, journal: &mut Journal) {
    let SlotState::Running(module) = &mut slot.state else { return };
    let policy = Policy { keyboard: false, visible: false, sees_close: false };
    let observed = sandbox::call(ctx, policy, wish, || module.settings_left());
    conclude(slot, host, CallKind::Other, journal, &observed);
}

/// Les conséquences d'un appel : statistiques, commandes de fenêtre relevées, style touché, plantage.
fn conclude(slot: &mut Slot, host: &Host, kind: CallKind, journal: &mut Journal, observed: &Observed) {
    slot.stats.record(kind, observed);
    if !observed.commands.is_empty() {
        if let SlotState::Running(module) = &mut slot.state {
            module.observe_viewport_commands(&observed.commands);
        }
        for c in &observed.commands {
            let what: Option<(&'static str, &'static str)> = match c {
                egui::ViewportCommand::Title(_) => Some((
                    "titre",
                    mark("a voulu renommer la fenêtre : Eyrie garde son titre, celui du module va dans sa case."),
                )),
                egui::ViewportCommand::Minimized(true) => {
                    Some(("reduire", mark("a voulu réduire la fenêtre : ignoré.")))
                }
                egui::ViewportCommand::Maximized(_) => {
                    Some(("agrandir", mark("a voulu agrandir ou restaurer la fenêtre : ignoré.")))
                }
                egui::ViewportCommand::Decorations(_) => {
                    Some(("decorations", mark("a voulu changer la barre de titre de la fenêtre : ignoré.")))
                }
                egui::ViewportCommand::Close => {
                    Some(("fermer", mark("a voulu fermer la fenêtre : ignoré (fermez plutôt sa case).")))
                }
                _ => None,
            };
            if let Some((kind, text)) = what
                && slot.noted.insert(kind)
            {
                journal.push(slot.info.id, Tone::Info, format!("{} {}", slot.info.name, tr(text)));
            }
        }
    }
    if observed.touched_style && slot.noted.insert("style") {
        journal.push(
            slot.info.id,
            Tone::Info,
            format!(
                "{} {}",
                slot.info.name,
                tr("a changé le style global (thème, visuels ou zoom) : rétabli après chacun de ses appels.")
            ),
        );
    }
    if let Some(msg) = &observed.panic {
        slot.stats.panics += 1;
        journal.push(slot.info.id, Tone::Danger, format!("{} s'est arrêté : {msg}", slot.info.name));
        // Sur le téléphone, elle mène à l'écran du module : « Relancer le module ».
        host.notify(
            Notification::new("suite", format!("{} {}", slot.info.name, tr("s'est arrêté")), msg.clone(), Tone::Danger)
                .path(format!("@module {}", slot.info.id)),
        );
        slot.state = SlotState::Crashed(msg.clone());
    }
}

/// Les raisons de ne pas fermer tout de suite.
pub fn close_checks(slots: &mut [Slot]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for s in slots.iter_mut() {
        if let Some(m) = s.module_mut() {
            let check = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| m.can_close()));
            if let Ok(CloseCheck::Busy(why)) = check {
                out.push((s.info.name.to_string(), why));
            }
        }
    }
    out
}

pub fn ago(at: SystemTime) -> String {
    let secs = SystemTime::now().duration_since(at).map(|d| d.as_secs()).unwrap_or(0);
    match secs {
        0..=59 => tr("à l'instant").to_string(),
        60..=3599 => format!("{} min", secs / 60),
        3600..=86_399 => format!("{} h", secs / 3600),
        _ => format!("{} j", secs / 86_400),
    }
}
