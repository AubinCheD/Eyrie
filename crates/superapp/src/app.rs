//! L'application de la suite : l'hôte des modules.
//!
//! À chaque image :
//! 1. `logic` : polices rétablies si un module les a remplacées, file de l'hôte (notifications,
//!    événements, demandes entre modules, « montre-moi »), puis la logique de **tous** les modules
//!    actifs, affichés ou non, chacun dans le bac à sable ;
//! 2. `ui` : raccourcis de la suite, barre de menus, barre d'activité, cases de la fenêtre
//!    principale, fenêtres détachées (viewports immédiats), modules masqués qui ont besoin de leur
//!    `ui`, puis palette, réglages et bulles par-dessus.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use eframe::egui::{self, ViewportCommand};
use egui_tiles::Tree;
use suite_sdk::fonts::FontPlan;
use suite_sdk::i18n::{self, Lang};
use suite_sdk::module::DataMode;
use suite_sdk::shortcut::{Owner, Registry, Shortcut};
use suite_sdk::theme::{self, ThemeChoice, ThemeId};
use suite_sdk::{Host, Request, Tone, Unavailable};

use crate::accounts::Accounts;
use crate::core::{self, CallKind, Focus, Journal, Notifications, Slot, SlotState};
use crate::i18n::{mark, tr};
use crate::layout::{self, Pane, Preset, SuiteView};
use crate::persist::{self, ModuleData, SavedLayout, SuiteState};
use crate::sandbox::{Policy, WindowWish};
use crate::screens::Screens;
use crate::shell::ShellState;
use crate::tailnet::Tailnet;
use crate::windows::Detached;
use crate::{capture, registry, settings};

pub const TITLE: &str = "SuperApp";

/// Ce que l'interface demande, traité en fin d'image (hors des emprunts des arbres de cases).
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    /// Montre la case (l'ouvre dans la fenêtre principale si elle n'est nulle part).
    Reveal(Pane),
    /// Ouvre la case dans une nouvelle fenêtre (ou la montre si elle est déjà ouverte).
    OpenInNewWindow(Pane),
    ClosePane(Pane),
    Detach(Pane),
    MoveToMain(Pane),
    AttachWindow(u64),
    AttachAll,
    Concentrate(Option<Pane>),
    Preset(Preset),
    LoadLayout(String),
    SaveLayout(String),
    DeleteLayout(String),
    RunCommand { module: String, command: String },
    OpenSettings(settings::Page),
    TogglePalette,
    SetTheme(ThemeChoice),
    SetLang(Option<Lang>),
    CycleCase(bool),
    FocusModule(usize),
    /// Demande poliment à l'exe autonome d'un module de se fermer.
    CloseExe(&'static str),
    /// Reconstruit le module (arrêté, indisponible, ou désactivé : il est alors activé).
    StartModule(String),
    /// Arrête le module et le désactive.
    StopModule(String),
    Restart,
    Quit,
}

/// La fermeture de la suite.
///
/// Certains modules, voyant la demande de fermeture, s'engagent tout de suite (DragonTree arrête
/// ses opérations, Menagerie arrête ou détache ses services) ; d'autres l'annulent pour poser leur
/// question. Les modules la voient donc **un par un**, une image chacun, ceux qui posent une
/// question d'abord, DragonTree en dernier : si l'un annule, les suivants n'ont rien vu. Quand il
/// répond (son `Close`), la séquence reprend après lui.
#[derive(Default)]
pub struct Quit {
    /// Modules qui ont demandé à fermer la fenêtre pendant la dernière image, hors de leur tour.
    pub asked: Vec<String>,
    /// L'utilisateur a confirmé : fermer malgré le travail en cours (modules natifs).
    pub forced: bool,
    /// Ce qui retient la fermeture (module, raison), montré dans un dialogue.
    pub busy: Vec<(String, String)>,
    /// Relancer la suite une fois fermée.
    pub restart: bool,
    pub sequence: Option<CloseSequence>,
    /// Le module qui voit la demande de fermeture pendant cette image.
    pub now: Option<String>,
    /// Ce module l'a annulée (pour poser sa question).
    pub cancelled: bool,
    /// Le tour de `now` reste à conclure (fin de l'image, ou début de la suivante si l'image n'a
    /// pas été dessinée : fenêtre réduite).
    step_pending: bool,
}

pub struct CloseSequence {
    order: Vec<String>,
    next: usize,
    /// Le module qui a annulé pour poser sa question, et depuis quand.
    held_by: Option<(String, Instant)>,
}

impl CloseSequence {
    /// Tous les modules ont vu la fermeture et aucun ne la retient.
    pub fn is_done(&self) -> bool {
        self.next >= self.order.len() && self.held_by.is_none()
    }
}

/// Au-delà, une question laissée sans réponse (« Revenir ») abandonne la fermeture.
const HOLD_EXPIRES: Duration = Duration::from_secs(300);

impl Quit {
    /// Le module `id` voit-il la demande de fermeture pendant cette image ?
    pub fn sees(&self, id: &str) -> bool {
        self.now.as_deref() == Some(id)
    }

    /// Ce qu'un appel de module a montré : `Close` hors de son tour (une croix dans sa case : ne
    /// ferme pas la suite), ou annulation pendant son tour.
    pub fn observe(&mut self, id: &str, o: &crate::sandbox::Observed) {
        if self.sees(id) {
            self.cancelled |= o.cancels_close && !o.wants_close;
        } else if o.wants_close && !self.asked.iter().any(|m| m == id) {
            self.asked.push(id.to_string());
        }
    }

    fn abandon(&mut self) {
        self.sequence = None;
        self.now = None;
        self.forced = false;
        self.restart = false;
    }
}

pub struct Core {
    pub host: Host,
    pub dir: PathBuf,
    pub st: SuiteState,
    pub writable: bool,
    pub slots: Vec<Slot>,
    pub journal: Journal,
    pub notes: Notifications,
    pub focus: Focus,
    /// Modules affichés pendant l'image en cours, et pendant la précédente.
    pub shown: BTreeSet<String>,
    pub shown_prev: BTreeSet<String>,
    pub wish: WindowWish,
    pub fonts: FontPlan,
    pub quit: Quit,
    pub actions: Vec<Action>,
    /// Un réglage ne prend effet qu'au prochain lancement (données des modules).
    pub restart_needed: bool,
    /// Où les modules rangent leur état pendant cette session (choisi au lancement).
    pub launch_data: ModuleData,
    /// Mode concentration : une seule case, en grand.
    pub concentrate: Option<Pane>,
    pub accounts: Accounts,
    pub tailnet: Tailnet,
    pub screens: Screens,
    pub registry: Registry,
    pub capture: Option<capture::Capture>,
    theme_now: ThemeId,
    last_save: Instant,
    saved_json: String,
    /// Vues dessinées pendant cette image : une vue n'est jamais dessinée deux fois.
    pub drawn: BTreeSet<(String, String)>,
    /// Dernière case cliquée dans chaque fenêtre (0 : principale), pour y ouvrir les suivantes.
    pub last_tile: std::collections::BTreeMap<u64, egui_tiles::TileId>,
}

impl Core {
    pub fn slot(&self, id: &str) -> Option<&Slot> {
        self.slots.iter().find(|s| s.info.id == id)
    }

    pub fn act(&mut self, a: Action) {
        self.actions.push(a);
    }

    pub fn palette(&self) -> &'static theme::Palette {
        self.host.palette()
    }

    pub fn pane_title(&self, pane: &Pane) -> (String, &'static str, egui::Color32) {
        match pane {
            Pane::Suite { view } => (tr(view.title()).to_string(), view.icon(), self.palette().accent),
            Pane::Module { module, view } => match self.slot(module) {
                Some(s) => {
                    let views = s.module().map(|m| m.views()).unwrap_or_default();
                    let title = match views.iter().find(|v| &v.id == view) {
                        Some(v) if views.len() > 1 => format!("{} · {}", s.info.name, v.title),
                        _ => s.info.name.to_string(),
                    };
                    (title, s.info.icon, theme::readable(self.palette(), s.info.color))
                }
                None => (module.clone(), "?", self.palette().text_dim),
            },
        }
    }
}

pub struct Suite {
    pub core: Core,
    pub main: Tree<Pane>,
    pub windows: Vec<Detached>,
    pub shell: ShellState,
}

/// Les raccourcis de la suite : ils passent avant ceux des modules.
pub fn suite_shortcuts(st: &SuiteState) -> Vec<(&'static str, &'static str, Shortcut)> {
    let mut v = vec![
        ("palette", mark("Palette de commandes"), Shortcut::key(egui::Key::F1)),
        ("case_suivante", mark("Case suivante"), Shortcut::ctrl(egui::Key::F6)),
        ("case_precedente", mark("Case précédente"), Shortcut::ctrl_shift(egui::Key::F6)),
        ("concentration", mark("Agrandir ou restaurer la case"), Shortcut::new(false, true, true, egui::Key::Enter)),
        ("detacher", mark("Détacher la case dans une fenêtre"), Shortcut::new(false, true, true, egui::Key::D)),
        ("ensemble", mark("Vue d'ensemble"), Shortcut::new(false, true, true, egui::Key::O)),
    ];
    const DIGITS: [egui::Key; 9] = [
        egui::Key::Num1,
        egui::Key::Num2,
        egui::Key::Num3,
        egui::Key::Num4,
        egui::Key::Num5,
        egui::Key::Num6,
        egui::Key::Num7,
        egui::Key::Num8,
        egui::Key::Num9,
    ];
    const IDS: [&str; 9] = ["module_1", "module_2", "module_3", "module_4", "module_5", "module_6", "module_7", "module_8", "module_9"];
    for (i, k) in DIGITS.iter().enumerate() {
        v.push((IDS[i], mark("Aller au module n"), Shortcut::alt(*k)));
    }
    for (id, _, sc) in &mut v {
        if let Some(custom) = st.shortcuts.get(*id).and_then(|s| Shortcut::parse(s)) {
            *sc = custom;
        }
    }
    v
}

impl Suite {
    pub fn new(cc: &eframe::CreationContext<'_>, dir: PathBuf, loaded: suite_sdk::state::Loaded<SuiteState>) -> Self {
        let ctx = cc.egui_ctx.clone();
        let st = loaded.value;
        i18n::set_lang(st.lang.unwrap_or_else(Lang::system));
        let host = Host::new(ctx.clone(), dir.clone(), st.theme);
        let mut journal = Journal::default();
        for p in &loaded.problems {
            journal.push("suite", Tone::Warn, p.clone());
        }

        // Les modules : construits une fois, au démarrage (eframe ne donne son `CreationContext`
        // qu'ici). Activer ou désactiver un module prend effet au prochain lancement.
        let mut fonts = FontPlan::suite();
        let mut slots = Vec::new();
        for f in registry::factories() {
            if let Some(add) = f.fonts {
                add(&mut fonts);
            }
            let mut slot = Slot::new(f.info, f.build);
            // Premier lancement : aucun module n'est construit avant le choix de leurs données
            // (celles des exe, ou un dossier d'essai) ; la suite se relance ensuite.
            if st.enabled(slot.info.id) && (st.welcomed || capture::Capture::from_env().is_some()) {
                slot.start(cc, &host, data_mode(st.module_data, &dir, slot.info.id), &mut journal);
            }
            slots.push(slot);
        }
        host.set_running(slots.iter().filter(|s| s.is_running()).map(|s| s.info.id.to_string()).collect());

        // Après tous les modules : le dernier `set_fonts` est celui de la suite.
        ctx.set_fonts(fonts.definitions());
        theme::apply(&ctx, host.palette(), st.zoom);
        let theme_now = theme::resolve(st.theme, &ctx);

        let mut registry = Registry::default();
        for (id, title, sc) in suite_shortcuts(&st) {
            registry.add(Owner::Suite, id, tr(title), sc);
        }
        for s in &slots {
            if let Some(m) = s.module() {
                for c in m.commands() {
                    if let Some(sc) = c.shortcut {
                        registry.add(Owner::Module(s.info.id.to_string()), c.id, c.title, sc);
                    }
                }
            }
        }

        let accounts = Accounts::load(&dir);
        accounts.publish(&host);
        let tailnet = Tailnet::new(&dir);

        let core = Core {
            host,
            dir,
            writable: loaded.writable,
            slots,
            journal,
            notes: Notifications::default(),
            focus: Focus::default(),
            shown: BTreeSet::new(),
            shown_prev: BTreeSet::new(),
            wish: WindowWish { title: TITLE.into(), decorations: true },
            fonts,
            quit: Quit::default(),
            actions: Vec::new(),
            restart_needed: false,
            launch_data: st.module_data,
            concentrate: None,
            accounts,
            tailnet,
            screens: Screens::default(),
            registry,
            capture: capture::Capture::from_env(),
            theme_now,
            last_save: Instant::now(),
            saved_json: String::new(),
            last_tile: Default::default(),
            drawn: BTreeSet::new(),
            st,
        };

        let (main, windows) = match core.st.layout.clone() {
            Some(l) => (l.main, l.windows.into_iter().map(Detached::from_saved).collect()),
            None => (default_layout(&core), Vec::new()),
        };
        let mut suite = Suite { core, main, windows, shell: ShellState::default() };
        suite.drop_unknown_panes();
        suite.core.saved_json = format!("{}{}", suite.layout_json(), suite.state_json());
        if !suite.core.st.welcomed && suite.core.capture.is_none() {
            suite.shell.welcome = true;
        }
        if let Some(scene) = suite.core.capture.as_ref().and_then(|c| c.scene.clone()) {
            suite.capture_scene(&scene);
        }
        suite
    }

    /// Prépare la scène d'une capture d'essai (`SUPERAPP_CAPTURE_SCENE`).
    fn capture_scene(&mut self, scene: &str) {
        let first = self.core.slots.iter().find(|s| s.is_running()).map(|s| Pane::module(s.info.id, suite_sdk::ViewInfo::MAIN));
        for part in scene.split(',') {
            match part.trim() {
                "palette" => self.shell.palette.toggle(),
                "reglages" => self.core.act(Action::OpenSettings(settings::Page::General)),
                "modules" => self.core.act(Action::OpenSettings(settings::Page::Modules)),
                "raccourcis" => self.core.act(Action::OpenSettings(settings::Page::Shortcuts)),
                "ensemble" => self.core.act(Action::Reveal(Pane::Suite { view: SuiteView::Overview })),
                "ressources" => self.core.act(Action::Reveal(Pane::Suite { view: SuiteView::Resources })),
                "journal" => self.core.act(Action::Reveal(Pane::Suite { view: SuiteView::Journal })),
                "grille" => self.core.act(Action::Preset(Preset::Grid)),
                "colonnes" => self.core.act(Action::Preset(Preset::Columns)),
                "detacher" => {
                    if let Some(p) = first.clone() {
                        self.core.act(Action::Detach(p));
                    }
                }
                "concentration" => self.core.act(Action::Concentrate(first.clone())),
                // Le contrat local, en appel Rust : un module demande à Fawkes d'ouvrir un terminal.
                "terminal" => {
                    let dir = std::env::current_dir().unwrap_or_default().display().to_string();
                    self.core.host.send(Request {
                        from: "carnet".into(),
                        to: "fawkes".into(),
                        demande: "ouvrir".into(),
                        data: serde_json::json!({ "dir": dir, "titre": "Ouvert par la suite" }),
                    });
                }
                m if m.starts_with("module:") => self.core.act(Action::Reveal(Pane::module(&m[7..], suite_sdk::ViewInfo::MAIN))),
                m if m.starts_with("theme:") => {
                    let t = match &m[6..] {
                        "fluent" => ThemeChoice::Fluent,
                        "nuit" => ThemeChoice::Nuit,
                        _ => ThemeChoice::Graphite,
                    };
                    self.core.act(Action::SetTheme(t));
                }
                _ => {}
            }
        }
    }

    /// Une disposition enregistrée peut citer un module qui n'existe plus.
    fn drop_unknown_panes(&mut self) {
        let known: BTreeSet<&str> = self.core.slots.iter().map(|s| s.info.id).collect();
        let unknown = |t: &Tree<Pane>| -> Vec<egui_tiles::TileId> {
            layout::panes(t).into_iter().filter(|(_, p)| p.module_id().is_some_and(|m| !known.contains(m))).map(|(id, _)| id).collect()
        };
        for id in unknown(&self.main) {
            layout::remove_pane(&mut self.main, id);
        }
        for w in &mut self.windows {
            for id in unknown(&w.tree) {
                layout::remove_pane(&mut w.tree, id);
            }
        }
        self.windows.retain(|w| !w.tree.is_empty());
    }

    pub fn saved_layout(&self) -> SavedLayout {
        SavedLayout { main: self.main.clone(), windows: self.windows.iter().map(Detached::to_saved).collect() }
    }

    fn layout_json(&self) -> String {
        serde_json::to_string(&self.saved_layout()).unwrap_or_default()
    }

    fn save(&mut self) {
        if !self.core.writable {
            return;
        }
        self.core.st.layout = Some(self.saved_layout());
        if let Err(e) = persist::save(&self.core.dir, &self.core.st) {
            self.core.journal.push("suite", Tone::Danger, format!("Sauvegarde impossible : {e}"));
        }
        self.core.saved_json = format!("{}{}", self.layout_json(), self.state_json());
        self.core.last_save = Instant::now();
    }

    /// Où est la case : fenêtre (0 : principale) et tuile.
    pub fn locate(&self, pane: &Pane) -> Option<(u64, egui_tiles::TileId)> {
        if let Some(t) = layout::find(&self.main, pane) {
            return Some((0, t));
        }
        self.windows.iter().find_map(|w| layout::find(&w.tree, pane).map(|t| (w.id, t)))
    }

    fn tree_mut(&mut self, window: u64) -> Option<&mut Tree<Pane>> {
        if window == 0 { Some(&mut self.main) } else { self.windows.iter_mut().find(|w| w.id == window).map(|w| &mut w.tree) }
    }

    fn reveal(&mut self, ctx: &egui::Context, pane: Pane) {
        if let Some(c) = &self.core.concentrate
            && *c != pane
        {
            self.core.concentrate = None;
        }
        match self.locate(&pane) {
            Some((window, tile)) => {
                if let Some(t) = self.tree_mut(window) {
                    layout::activate(t, tile);
                }
                if window != 0 {
                    ctx.send_viewport_cmd_to(crate::windows::viewport_id(window), ViewportCommand::Focus);
                }
            }
            None => {
                let near = self.core.last_tile.get(&0).copied();
                layout::add_pane(&mut self.main, pane.clone(), near);
            }
        }
        if let Some(m) = pane.module_id() {
            self.core.focus.module = Some(m.to_string());
        }
    }

    fn take_pane(&mut self, pane: &Pane) -> bool {
        match self.locate(pane) {
            Some((window, tile)) => {
                if let Some(t) = self.tree_mut(window) {
                    layout::remove_pane(t, tile);
                }
                self.windows.retain(|w| !w.tree.is_empty());
                true
            }
            None => false,
        }
    }

    fn detach(&mut self, pane: Pane) {
        self.take_pane(&pane);
        let id = self.windows.iter().map(|w| w.id).max().unwrap_or(0) + 1;
        self.windows.push(Detached::new(id, pane.clone()));
        if self.core.concentrate.as_ref() == Some(&pane) {
            self.core.concentrate = None;
        }
    }

    fn attach_window(&mut self, id: u64) {
        let Some(i) = self.windows.iter().position(|w| w.id == id) else { return };
        let w = self.windows.remove(i);
        for (_, p) in layout::panes(&w.tree) {
            let near = self.core.last_tile.get(&0).copied();
            layout::add_pane(&mut self.main, p, near);
        }
    }

    fn apply_layout(&mut self, l: SavedLayout) {
        self.main = l.main;
        self.windows = l.windows.into_iter().map(Detached::from_saved).collect();
        self.core.concentrate = None;
        self.drop_unknown_panes();
    }

    fn process_actions(&mut self, ctx: &egui::Context) {
        self.apply_actions(ctx);
        // La case agrandie a pu être fermée (croix de son onglet, action, module arrêté) ou partir
        // dans une autre fenêtre : la concentration s'arrête avec elle.
        if let Some(c) = &self.core.concentrate
            && self.locate(c).map(|(w, _)| w) != Some(0)
        {
            self.core.concentrate = None;
        }
    }

    fn apply_actions(&mut self, ctx: &egui::Context) {
        let actions = std::mem::take(&mut self.core.actions);
        for a in actions {
            match a {
                Action::Reveal(p) => self.reveal(ctx, p),
                Action::OpenInNewWindow(p) => {
                    if self.locate(&p).is_some_and(|(w, _)| w != 0) {
                        self.reveal(ctx, p);
                    } else {
                        self.detach(p);
                    }
                }
                Action::ClosePane(p) => {
                    self.take_pane(&p);
                }
                Action::Detach(p) => self.detach(p),
                Action::MoveToMain(p) => {
                    self.take_pane(&p);
                    self.reveal(ctx, p);
                }
                Action::AttachWindow(id) => self.attach_window(id),
                Action::AttachAll => {
                    for id in self.windows.iter().map(|w| w.id).collect::<Vec<_>>() {
                        self.attach_window(id);
                    }
                }
                Action::Concentrate(p) => {
                    if let Some(p) = &p {
                        // Détachée : elle revient d'abord dans la fenêtre principale (sinon dessinée
                        // deux fois).
                        if self.locate(p).is_some_and(|(w, _)| w != 0) {
                            self.take_pane(p);
                        }
                        self.reveal(ctx, p.clone());
                    }
                    self.core.concentrate = p;
                }
                Action::Preset(kind) => {
                    let panes: Vec<Pane> = layout::panes(&self.main).into_iter().map(|(_, p)| p).collect();
                    let panes = if panes.is_empty() {
                        default_panes(&self.core).into_iter().filter(|p| self.locate(p).is_none()).collect()
                    } else {
                        panes
                    };
                    self.main = layout::preset(0, kind, panes);
                    self.core.concentrate = None;
                }
                Action::LoadLayout(name) => {
                    if let Some(l) = self.core.st.layouts.get(&name).cloned() {
                        self.apply_layout(l);
                    } else if let Some(l) = named_default(&self.core, &name) {
                        self.apply_layout(l);
                    }
                }
                Action::SaveLayout(name) => {
                    let l = self.saved_layout();
                    self.core.st.layouts.insert(name, l);
                }
                Action::DeleteLayout(name) => {
                    self.core.st.layouts.remove(&name);
                    self.core.st.screen_layouts.retain(|_, v| *v != name);
                }
                Action::RunCommand { module, command } => {
                    let main = self.core.slot(&module).and_then(|s| s.module()).and_then(|m| m.views().first().map(|v| v.id.clone()));
                    if let Some(view) = main {
                        self.reveal(ctx, Pane::module(&module, &view));
                    }
                    // Exécutée au début de l'image suivante, avec le clavier : le module embarqué
                    // reçoit alors la frappe de son raccourci.
                    self.shell.pending_commands.push((module, command));
                }
                Action::OpenSettings(page) => {
                    self.shell.settings.open = true;
                    self.shell.settings.page = page;
                    self.core.focus.module = None;
                }
                Action::TogglePalette => {
                    self.shell.palette.toggle();
                    if self.shell.palette.open {
                        self.core.focus.module = None;
                    }
                }
                Action::SetTheme(t) => {
                    self.core.st.theme = t;
                    self.core.host.set_theme(t);
                    theme::apply(ctx, self.core.host.palette(), self.core.st.zoom);
                    self.core.theme_now = theme::resolve(t, ctx);
                }
                Action::SetLang(l) => {
                    self.core.st.lang = l;
                    i18n::set_lang(l.unwrap_or_else(Lang::system));
                }
                Action::CycleCase(forward) => {
                    let visible: Vec<egui_tiles::TileId> =
                        self.main.active_tiles().into_iter().filter(|t| matches!(self.main.tiles.get(*t), Some(egui_tiles::Tile::Pane(_)))).collect();
                    let current = self.core.focus.module.as_ref().and_then(|m| {
                        visible.iter().copied().find(|t| matches!(self.main.tiles.get(*t), Some(egui_tiles::Tile::Pane(p)) if p.module_id() == Some(m)))
                    });
                    if let Some(next) = layout::cycle(&visible, current, forward)
                        && let Some(egui_tiles::Tile::Pane(p)) = self.main.tiles.get(next)
                    {
                        self.core.focus.module = p.module_id().map(str::to_string);
                    }
                }
                Action::FocusModule(n) => {
                    let running: Vec<Pane> = self
                        .core
                        .slots
                        .iter()
                        .filter(|s| !matches!(s.state, SlotState::Disabled))
                        .map(|s| Pane::module(s.info.id, suite_sdk::ViewInfo::MAIN))
                        .collect();
                    if let Some(p) = running.get(n) {
                        self.reveal(ctx, p.clone());
                    }
                }
                Action::CloseExe(exe) => {
                    let pids = suite_sdk::instance::find_processes(exe);
                    let n: usize = pids.iter().map(|p| suite_sdk::instance::ask_to_close(*p)).sum();
                    let msg = if pids.is_empty() {
                        format!("{exe} : {}", tr("aucun processus trouvé"))
                    } else {
                        format!("{exe} : {} ({n})", tr("fermeture demandée"))
                    };
                    self.core.journal.push("suite", Tone::Info, msg);
                }
                Action::StartModule(id) => {
                    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
                    // Le mode du lancement, pas le réglage changé depuis : les variables
                    // d'environnement du mode essai sont posées au démarrage seulement.
                    let data = data_mode(self.core.launch_data, &self.core.dir, &id);
                    let Core { slots, host, journal, st, .. } = &mut self.core;
                    if let Some(slot) = slots.iter_mut().find(|s| s.info.id == id) {
                        slot.start(&cc, host, data, journal);
                        st.modules.insert(id.clone(), persist::ModuleSettings { enabled: true });
                    }
                    // Son constructeur a pu poser son thème et son zoom : ceux de la suite reviennent
                    // (ses polices sont rétablies au début de l'image suivante).
                    theme::apply(ctx, self.core.host.palette(), self.core.st.zoom);
                    self.refresh_registry();
                }
                Action::StopModule(id) => {
                    let Core { slots, journal, st, .. } = &mut self.core;
                    if let Some(slot) = slots.iter_mut().find(|s| s.info.id == id) {
                        slot.stop();
                        journal.push(&id, Tone::Info, format!("{} {}", slot.info.name, tr("arrêté et désactivé")));
                        st.modules.insert(id.clone(), persist::ModuleSettings { enabled: false });
                    }
                    self.refresh_registry();
                }
                Action::Restart => {
                    self.core.quit.restart = true;
                    self.core.quit.forced = true;
                    ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, ViewportCommand::Close);
                }
                Action::Quit => ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, ViewportCommand::Close),
            }
        }
    }

    /// Raccourcis et modules actifs, après un démarrage ou un arrêt de module.
    fn refresh_registry(&mut self) {
        let mut registry = Registry::default();
        for (id, title, sc) in suite_shortcuts(&self.core.st) {
            registry.add(Owner::Suite, id, tr(title), sc);
        }
        for s in &self.core.slots {
            if let Some(m) = s.module() {
                for c in m.commands() {
                    if let Some(sc) = c.shortcut {
                        registry.add(Owner::Module(s.info.id.to_string()), c.id, c.title, sc);
                    }
                }
            }
        }
        self.core.registry = registry;
        self.core.host.set_running(self.core.slots.iter().filter(|s| s.is_running()).map(|s| s.info.id.to_string()).collect());
    }

    /// La file de l'hôte : notifications, événements, demandes, « montre-moi », attention.
    fn process_outbox(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        let out = self.core.host.take_outbox();
        for n in out.notifications {
            self.core.notes.push(n);
        }
        for m in out.show {
            if let Some(view) = self.core.slot(&m).and_then(|s| s.module()).and_then(|m| m.views().first().map(|v| v.id.clone())) {
                self.reveal(ctx, Pane::module(&m, &view));
                ctx.send_viewport_cmd(ViewportCommand::Focus);
            }
        }
        if !out.attention.is_empty() {
            ctx.send_viewport_cmd(ViewportCommand::RequestUserAttention(egui::UserAttentionType::Informational));
        }
        let Core { slots, host, journal, wish, shown_prev, .. } = &mut self.core;
        for e in &out.events {
            for slot in slots.iter_mut().filter(|s| s.info.id != e.source && s.is_running()) {
                let policy = Policy { keyboard: false, visible: shown_prev.contains(slot.info.id), sees_close: false };
                core::call(slot, host, frame, ctx, CallKind::Other, policy, wish, journal, |m, cx| m.on_event(e, cx));
            }
        }
        for r in out.requests {
            let Some(slot) = slots.iter_mut().find(|s| s.info.id == r.to) else {
                journal.push(&r.from, Tone::Warn, format!("{} → {} : module absent ({})", r.from, r.to, r.demande));
                continue;
            };
            let policy = Policy { keyboard: false, visible: shown_prev.contains(slot.info.id), sees_close: false };
            let mut reply = Err(String::new());
            let req: &Request = &r;
            core::call(slot, host, frame, ctx, CallKind::Other, policy, wish, journal, |m, cx| reply = m.on_request(req, cx));
            match reply {
                Ok(_) => journal.push(&r.from, Tone::Ok, format!("{} → {} : {}", r.from, r.to, r.demande)),
                Err(e) if !e.is_empty() => journal.push(&r.from, Tone::Warn, format!("{} → {} : {} ({e})", r.from, r.to, r.demande)),
                Err(_) => {}
            }
        }
    }

    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        let shortcuts = suite_shortcuts(&self.core.st);
        let pressed: Vec<&'static str> =
            ctx.input_mut(|i| shortcuts.iter().filter(|(_, _, sc)| sc.consume(i)).map(|(id, _, _)| *id).collect());
        for id in pressed {
            let a = match id {
                "palette" => Action::TogglePalette,
                "case_suivante" => Action::CycleCase(true),
                "case_precedente" => Action::CycleCase(false),
                "concentration" => Action::Concentrate(match &self.core.concentrate {
                    Some(_) => None,
                    None => self.active_pane(),
                }),
                "detacher" => match self.active_pane() {
                    Some(p) => Action::Detach(p),
                    None => continue,
                },
                "ensemble" => Action::Reveal(Pane::Suite { view: SuiteView::Overview }),
                m if m.starts_with("module_") => Action::FocusModule(m[7..].parse::<usize>().unwrap_or(1) - 1),
                _ => continue,
            };
            self.core.act(a);
        }
    }

    /// La case du module qui a le clavier.
    pub fn active_pane(&self) -> Option<Pane> {
        let m = self.core.focus.module.as_ref()?;
        let all = layout::panes(&self.main).into_iter().chain(self.windows.iter().flat_map(|w| layout::panes(&w.tree)));
        all.map(|(_, p)| p).find(|p| p.module_id() == Some(m))
    }

    fn handle_close(&mut self, ctx: &egui::Context) {
        // Le tour de l'image précédente n'a pas été conclu (pas d'affichage : fenêtre réduite).
        if self.core.quit.step_pending {
            self.finish_close_step(ctx);
        }
        let close = ctx.input(|i| i.viewport().close_requested());
        let asked = std::mem::take(&mut self.core.quit.asked);
        let quit = &mut self.core.quit;
        quit.now = None;
        quit.cancelled = false;
        if let Some(seq) = &quit.sequence
            && let Some((held, since)) = &seq.held_by
            && since.elapsed() > HOLD_EXPIRES
        {
            self.core.journal.push("suite", Tone::Info, format!("{held} : {}", tr("question restée sans réponse, fermeture abandonnée.")));
            self.core.quit.abandon();
        }
        if !close {
            return;
        }
        let running: Vec<String> = self.core.slots.iter().filter(|s| s.is_running()).map(|s| s.info.id.to_string()).collect();
        let quit = &mut self.core.quit;
        match &mut quit.sequence {
            None => {
                if !asked.is_empty() && !quit.forced {
                    // La croix de la barre de titre de Fawkes, « Quitter » d'un module… : dans la
                    // suite, cela ne ferme pas la suite.
                    ctx.send_viewport_cmd(ViewportCommand::CancelClose);
                    let who = asked.join(", ");
                    self.core.journal.push("suite", Tone::Info, format!("{who} : {}", tr("a demandé à fermer la fenêtre ; la suite reste ouverte.")));
                    return;
                }
                if !quit.forced {
                    let busy = core::close_checks(&mut self.core.slots);
                    if !busy.is_empty() {
                        ctx.send_viewport_cmd(ViewportCommand::CancelClose);
                        self.core.quit.busy = busy;
                        return;
                    }
                }
                let mut order = running;
                // DragonTree s'engage dès qu'il voit la fermeture (il arrête ses opérations) : en dernier.
                order.sort_by_key(|id| id == "dragontree");
                self.core.quit.sequence = Some(CloseSequence { order, next: 0, held_by: None });
            }
            Some(seq) => {
                if let Some((held, _)) = seq.held_by.clone() {
                    if asked.contains(&held) {
                        // Sa réponse : il accepte de fermer ; on continue après lui.
                        seq.held_by = None;
                    } else if asked.is_empty() {
                        // L'utilisateur referme la suite : on repose la question au même module,
                        // et ce n'est plus une relance.
                        seq.next = seq.order.iter().position(|m| *m == held).unwrap_or(seq.next);
                        seq.held_by = None;
                        quit.restart = false;
                    } else {
                        ctx.send_viewport_cmd(ViewportCommand::CancelClose);
                        return;
                    }
                }
            }
        }
        let quit = &mut self.core.quit;
        let Some(seq) = &mut quit.sequence else { return };
        if seq.next < seq.order.len() {
            quit.now = Some(seq.order[seq.next].clone());
            seq.next += 1;
            quit.step_pending = true;
            // Cette demande-ci est pour ce seul module ; une nouvelle sera faite pour le suivant.
            ctx.send_viewport_cmd(ViewportCommand::CancelClose);
        }
        // Sinon tous ont accepté : la suite se ferme (pas d'annulation).
    }

    /// Fin du tour d'un module dans la fermeture : il a annulé (sa question est posée, on attend sa
    /// réponse) ou accepté (au suivant, ou fermeture).
    fn finish_close_step(&mut self, ctx: &egui::Context) {
        let quit = &mut self.core.quit;
        if !quit.step_pending {
            return;
        }
        quit.step_pending = false;
        let Some(seq) = &mut quit.sequence else { return };
        match quit.now.clone() {
            Some(id) if quit.cancelled => {
                seq.held_by = Some((id, Instant::now()));
                // Un module dessiné dans une fenêtre détachée a annulé sur la sienne : c'est la
                // fenêtre principale qui se ferme.
                ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, ViewportCommand::CancelClose);
            }
            Some(_) => ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, ViewportCommand::Close),
            None => {}
        }
    }

    fn run_logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        let Core { slots, host, journal, wish, shown_prev, focus, quit, .. } = &mut self.core;
        for slot in slots.iter_mut() {
            let policy = Policy {
                keyboard: focus.module.as_deref() == Some(slot.info.id),
                visible: shown_prev.contains(slot.info.id),
                sees_close: quit.sees(slot.info.id),
            };
            if let Some(o) = core::call(slot, host, frame, ctx, CallKind::Logic, policy, wish, journal, |m, cx| m.logic(ctx, cx)) {
                quit.observe(slot.info.id, &o);
            }
        }
    }

    fn pending_commands(&mut self, frame: &mut eframe::Frame, ctx: &egui::Context) {
        let pending = std::mem::take(&mut self.shell.pending_commands);
        let Core { slots, host, journal, wish, .. } = &mut self.core;
        for (module, command) in pending {
            if let Some(slot) = slots.iter_mut().find(|s| s.info.id == module) {
                let policy = Policy { keyboard: true, visible: true, sees_close: false };
                core::call(slot, host, frame, ctx, CallKind::Other, policy, wish, journal, |m, cx| m.run_command(&command, cx));
            }
        }
    }

    /// Les modules masqués dont le travail de fond est resté dans `ui` (Menagerie, Patronus,
    /// Grimoire) : dessinés hors de l'écran, dans un `Ui` invisible et sans clavier.
    fn hidden_ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let Core { slots, host, journal, wish, shown, quit, .. } = &mut self.core;
        let offscreen = egui::Rect::from_min_size(egui::pos2(-30_000.0, -30_000.0), egui::vec2(1200.0, 800.0));
        for slot in slots.iter_mut() {
            if shown.contains(slot.info.id) || !slot.module().is_some_and(|m| m.needs_hidden_ui()) {
                continue;
            }
            let view = slot.module().and_then(|m| m.views().first().map(|v| v.id.clone())).unwrap_or_default();
            let mut child = ui.new_child(egui::UiBuilder::new().id_salt(("module-masque", slot.info.id)).max_rect(offscreen).invisible());
            let policy = Policy { keyboard: false, visible: false, sees_close: quit.sees(slot.info.id) };
            if let Some(o) =
                core::call(slot, host, frame, &ctx, CallKind::HiddenUi, policy, wish, journal, |m, cx| m.ui(&mut child, &view, cx))
            {
                quit.observe(slot.info.id, &o);
            }
        }
    }

    /// Taille, position et agrandissement de la fenêtre principale, pour le prochain lancement.
    fn track_window(&mut self, ctx: &egui::Context) {
        let (outer, inner, maximized) = ctx.input(|i| (i.viewport().outer_rect, i.viewport().inner_rect, i.viewport().maximized));
        let w = &mut self.core.st.window;
        w.maximized = maximized.unwrap_or(false);
        if !w.maximized {
            if let Some(r) = inner {
                w.size = [r.width().round(), r.height().round()];
            }
            if let Some(r) = outer {
                w.pos = Some([r.min.x.round(), r.min.y.round()]);
            }
        }
    }

    fn watch_theme(&mut self, ctx: &egui::Context) {
        let now = theme::resolve(self.core.st.theme, ctx);
        if now != self.core.theme_now {
            self.core.theme_now = now;
            self.core.host.set_theme(self.core.st.theme);
            theme::apply(ctx, self.core.host.palette(), self.core.st.zoom);
        }
    }

    fn watch_screens(&mut self) {
        if let Some(signature) = self.core.screens.changed() {
            self.core.journal.push("suite", Tone::Info, format!("{} : {signature}", tr("Écrans")));
            if let Some(name) = self.core.st.screen_layouts.get(&signature).cloned() {
                self.core.journal.push("suite", Tone::Info, format!("{} « {name} »", tr("Disposition appliquée :")));
                self.core.act(Action::LoadLayout(name));
            }
        }
    }

    fn state_json(&self) -> String {
        let mut st = self.core.st.clone();
        st.layout = None;
        serde_json::to_string(&st).unwrap_or_default()
    }

    fn autosave(&mut self) {
        if self.core.last_save.elapsed() < Duration::from_secs(5) {
            return;
        }
        let json = format!("{}{}", self.layout_json(), self.state_json());
        if json != self.core.saved_json {
            self.save();
        } else {
            self.core.last_save = Instant::now();
        }
    }
}

/// Où le module `id` range son état, selon les réglages de la suite.
pub fn data_mode(mode: ModuleData, dir: &std::path::Path, id: &str) -> DataMode {
    match mode {
        ModuleData::Shared => DataMode::Shared,
        ModuleData::Trial => DataMode::Trial(dir.join("essai").join(id)),
    }
}

/// Toutes les vues principales des modules disponibles, plus la vue d'ensemble.
fn default_panes(core: &Core) -> Vec<Pane> {
    let mut v: Vec<Pane> = core
        .slots
        .iter()
        .filter(|s| !matches!(s.state, SlotState::Disabled))
        .map(|s| Pane::module(s.info.id, suite_sdk::ViewInfo::MAIN))
        .collect();
    v.insert(0, Pane::Suite { view: SuiteView::Overview });
    v
}

/// Au premier lancement : la vue d'ensemble et les modules en onglets, à gauche ; Fawkes à droite.
fn default_layout(core: &Core) -> Tree<Pane> {
    let panes = default_panes(core);
    let (right, left): (Vec<Pane>, Vec<Pane>) = panes.into_iter().partition(|p| p.module_id() == Some("fawkes"));
    if right.is_empty() {
        return layout::preset(0, Preset::Tabs, left);
    }
    let mut tiles = egui_tiles::Tiles::default();
    let left: Vec<egui_tiles::TileId> = left.into_iter().map(|p| tiles.insert_pane(p)).collect();
    let right: Vec<egui_tiles::TileId> = right.into_iter().map(|p| tiles.insert_pane(p)).collect();
    let (l, r) = (tiles.insert_tab_tile(left), tiles.insert_tab_tile(right));
    let root = tiles.insert_horizontal_tile(vec![l, r]);
    if let Some(egui_tiles::Tile::Container(egui_tiles::Container::Linear(lin))) = tiles.get_mut(root) {
        lin.shares.set_share(l, 3.0);
        lin.shares.set_share(r, 2.0);
    }
    Tree::new(layout::tree_id(0), root, tiles)
}

/// Les dispositions proposées d'office, si l'utilisateur ne les a pas redéfinies.
pub const DEFAULT_LAYOUTS: [(&str, &[&str]); 3] =
    [("Dev", &["dragontree", "fawkes"]), ("Revue", &["owlery", "dragontree"]), ("Exploitation", &["menagerie", "fawkes", "patronus"])];

fn named_default(core: &Core, name: &str) -> Option<SavedLayout> {
    let (_, modules) = DEFAULT_LAYOUTS.iter().find(|(n, _)| *n == name)?;
    let panes: Vec<Pane> = modules
        .iter()
        .filter(|m| core.slot(m).is_some_and(|s| !matches!(s.state, SlotState::Disabled)))
        .map(|m| Pane::module(m, suite_sdk::ViewInfo::MAIN))
        .collect();
    Some(SavedLayout { main: layout::preset(0, Preset::Columns, panes), windows: Vec::new() })
}

impl eframe::App for Suite {
    fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        // Un module a pu remplacer toutes les polices : rétablies avant toute mise en page.
        if suite_sdk::fonts::guard(ctx, &self.core.fonts) {
            self.core.journal.push("suite", Tone::Info, tr("Polices remplacées par un module : rétablies."));
        }
        // `set_zoom_factor` d'un module (DragonTree, Patronus) ne s'applique qu'au début de l'image
        // suivante, hors du bac à sable : le zoom de la suite est redemandé dès qu'il s'en écarte
        // (une image au mauvais zoom).
        if (ctx.zoom_factor() - self.core.st.zoom).abs() > 1e-3 {
            ctx.set_zoom_factor(self.core.st.zoom);
        }
        self.watch_theme(ctx);
        self.handle_close(ctx);
        self.process_outbox(ctx, frame);
        self.run_logic(ctx, frame);
        self.core.tailnet.poll(&self.core.st.tailnet, &self.core.dir, ctx);
        self.core.accounts.poll(&self.core.host);
        self.watch_screens();
        if let Some(c) = &mut self.core.capture
            && c.poll(ctx)
        {
            self.core.capture = None;
            self.core.quit.forced = true;
            ctx.send_viewport_cmd(ViewportCommand::Close);
        }
        self.track_window(ctx);
        self.autosave();
        // Réveil lent : pastilles, bulles qui s'effacent, sauvegarde.
        ctx.request_repaint_after(Duration::from_millis(500));
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.core.shown.clear();
        self.handle_shortcuts(&ctx);
        self.pending_commands(frame, &ctx);
        crate::shell::show(self, ui, frame);
        crate::windows::show_all(self, &ctx, frame);
        self.hidden_ui(ui, frame);
        crate::shell::overlays(self, &ctx, frame);
        self.finish_close_step(&ctx);
        self.process_actions(&ctx);
        self.core.shown_prev = std::mem::take(&mut self.core.shown);
        self.core.drawn.clear();
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        for s in &mut self.core.slots {
            if let Some(m) = s.module_mut() {
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| m.on_exit()));
            }
        }
        self.core.tailnet.stop();
        self.save();
        if self.core.quit.restart {
            crate::restart();
        }
    }
}

/// Message d'un module indisponible.
pub fn unavailable_text(u: &Unavailable) -> String {
    match u {
        Unavailable::AlreadyRunning { exe } => format!("{exe} {}", tr("tourne déjà seul, sur le même dossier d'état.")),
        Unavailable::Failed(e) => e.clone(),
    }
}
