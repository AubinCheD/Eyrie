//! Essais de l'hôte de bout en bout (egui_kittest), avec le seul module natif : aucun module
//! embarqué n'est construit (aucun terminal, aucun verrou des exe, aucun réseau).

use std::sync::atomic::{AtomicU32, Ordering};

use eframe::egui;
use egui_kittest::Harness;
use suite_sdk::host::Event;
use suite_sdk::{Cx, Module, ModuleInfo, Origin};

use crate::app::{Action, Suite};
use crate::core::{Slot, SlotState};
use crate::layout::{self, Pane, Preset, SuiteView};
use crate::persist::{ModuleSettings, SuiteState};
use crate::registry;

static N: AtomicU32 = AtomicU32::new(0);

fn temp_dir() -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("superapp-essai-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// La suite avec seulement Carnet activé.
fn suite() -> Harness<'static, Suite> {
    let mut st = SuiteState { welcomed: true, ..Default::default() };
    for f in registry::factories() {
        st.modules.insert(f.info.id.to_string(), ModuleSettings { enabled: f.info.id == "carnet" });
    }
    let dir = temp_dir();
    let mut h = Harness::builder()
        .with_size([1400.0, 900.0])
        .build_eframe(move |cc| Suite::new(cc, dir, suite_sdk::state::Loaded { value: st, problems: Vec::new(), writable: true }));
    h.run_steps(3);
    h
}

fn notes() -> Pane {
    Pane::module("carnet", "notes")
}

fn act(h: &mut Harness<'static, Suite>, a: Action) {
    h.state_mut().core.act(a);
    h.run_steps(3);
}

#[test]
fn only_enabled_modules_are_built() {
    let h = suite();
    let s = h.state();
    for slot in &s.core.slots {
        let running = slot.is_running();
        assert_eq!(running, slot.info.id == "carnet", "{}", slot.info.id);
    }
    // Disposition de départ : la vue d'ensemble et Carnet (Fawkes absent : désactivé).
    let panes: Vec<Pane> = layout::panes(&s.main).into_iter().map(|(_, p)| p).collect();
    assert!(panes.contains(&Pane::Suite { view: SuiteView::Overview }));
    assert!(panes.contains(&Pane::module("carnet", suite_sdk::ViewInfo::MAIN)) || panes.contains(&notes()));
}

#[test]
fn a_pane_can_be_detached_and_attached_again() {
    let mut h = suite();
    act(&mut h, Action::Reveal(notes()));
    assert_eq!(h.state().locate(&notes()).map(|(w, _)| w), Some(0));
    act(&mut h, Action::Detach(notes()));
    let (w, _) = h.state().locate(&notes()).expect("toujours ouverte");
    assert_ne!(w, 0, "dans une fenêtre détachée");
    assert_eq!(h.state().windows.len(), 1);
    act(&mut h, Action::AttachAll);
    assert_eq!(h.state().locate(&notes()).map(|(w, _)| w), Some(0));
    assert!(h.state().windows.is_empty());
}

#[test]
fn a_view_is_never_open_twice() {
    let mut h = suite();
    act(&mut h, Action::Reveal(notes()));
    act(&mut h, Action::Reveal(notes()));
    act(&mut h, Action::OpenInNewWindow(notes()));
    act(&mut h, Action::OpenInNewWindow(notes()));
    let s = h.state();
    let count = layout::panes(&s.main).iter().chain(s.windows.iter().flat_map(|w| layout::panes(&w.tree)).collect::<Vec<_>>().iter()).filter(|(_, p)| *p == notes()).count();
    assert_eq!(count, 1);
}

#[test]
fn presets_keep_every_pane() {
    let mut h = suite();
    act(&mut h, Action::Reveal(Pane::module("carnet", "bus")));
    let before = layout::panes(&h.state().main).len();
    for kind in Preset::ALL {
        act(&mut h, Action::Preset(kind));
        assert_eq!(layout::panes(&h.state().main).len(), before, "{kind:?}");
    }
}

#[test]
fn layouts_are_saved_and_loaded_by_name() {
    let mut h = suite();
    act(&mut h, Action::Preset(Preset::Columns));
    act(&mut h, Action::SaveLayout("Essai".into()));
    act(&mut h, Action::ClosePane(Pane::Suite { view: SuiteView::Overview }));
    assert!(h.state().locate(&Pane::Suite { view: SuiteView::Overview }).is_none());
    act(&mut h, Action::LoadLayout("Essai".into()));
    assert!(h.state().locate(&Pane::Suite { view: SuiteView::Overview }).is_some());
}

#[test]
fn bus_events_reach_the_other_modules() {
    let mut h = suite();
    h.state().core.host.publish(Event { source: "essai".into(), kind: "pr_ouverte".into(), data: serde_json::json!({}) });
    h.run_steps(2);
    let badge = h.state().core.slot("carnet").and_then(|s| s.module()).and_then(|m| m.status().badge);
    assert_eq!(badge.and_then(|b| b.count), Some(1));
}

#[test]
fn a_module_can_be_stopped_and_started_again() {
    let mut h = suite();
    act(&mut h, Action::StopModule("carnet".into()));
    assert!(matches!(h.state().core.slot("carnet").unwrap().state, SlotState::Disabled));
    act(&mut h, Action::StartModule("carnet".into()));
    assert!(h.state().core.slot("carnet").unwrap().is_running());
}

/// Un module qui panique à l'affichage.
struct Boom {
    info: ModuleInfo,
}

impl Module for Boom {
    fn info(&self) -> &ModuleInfo {
        &self.info
    }

    fn logic(&mut self, _ctx: &egui::Context, _cx: &mut Cx) {}

    fn ui(&mut self, ui: &mut egui::Ui, _view: &str, _cx: &mut Cx) {
        // Le style global et le titre de la fenêtre, puis la panique : rien ne doit rester.
        ui.ctx().set_visuals(egui::Visuals::light());
        ui.ctx().send_viewport_cmd(egui::ViewportCommand::Title("Boum".into()));
        panic!("boum à l'affichage");
    }
}

fn boom_info() -> ModuleInfo {
    ModuleInfo { id: "boum", name: "Boum", icon: "!", color: egui::Color32::RED, summary: "essai", origin: Origin::Native }
}

fn build_boom(_: &suite_sdk::BuildCx) -> Result<Box<dyn Module>, suite_sdk::Unavailable> {
    Ok(Box::new(Boom { info: boom_info() }))
}

#[test]
fn a_panicking_module_is_stopped_and_the_others_go_on() {
    crate::sandbox::install_panic_hook();
    let mut h = suite();
    let ctx = h.ctx.clone();
    {
        let s = h.state_mut();
        let mut slot = Slot::new(boom_info(), build_boom);
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        slot.start(&cc, &s.core.host, suite_sdk::DataMode::Shared, &mut s.core.journal);
        s.core.slots.push(slot);
        s.core.act(Action::Reveal(Pane::module("boum", suite_sdk::ViewInfo::MAIN)));
        s.core.act(Action::Reveal(notes()));
        // Les deux cases visibles en même temps.
        s.core.act(Action::Preset(Preset::Grid));
    }
    h.run_steps(5);
    let s = h.state();
    match &s.core.slot("boum").unwrap().state {
        SlotState::Crashed(msg) => assert!(msg.contains("boum à l'affichage"), "{msg}"),
        _ => panic!("le module aurait dû être arrêté"),
    }
    assert!(s.core.slot("carnet").unwrap().is_running(), "les autres continuent");
    assert!(ctx.global_style().visuals.dark_mode, "le style de la suite est rétabli");
    assert!(s.core.journal.lines.iter().any(|l| l.text.contains("s'est arrêté")));
}

// --- Fermeture en séquence ---------------------------------------------------------------------

#[derive(Default)]
struct CloseLog {
    /// (module, image) où le module a vu la demande de fermeture.
    seen: Vec<(&'static str, u32)>,
    /// L'utilisateur a répondu « quitter » dans la question de Questionneur.
    consent: bool,
    frame: u32,
}

thread_local! {
    static CLOSE_LOG: std::cell::RefCell<CloseLog> = std::cell::RefCell::new(CloseLog::default());
}

/// Voit la fermeture : la première fois, l'annule pour poser sa question ; quand l'utilisateur a
/// répondu, demande lui-même la fermeture (comme Grimoire ou Menagerie).
struct Asker {
    info: ModuleInfo,
    asked: bool,
}

/// Voit la fermeture et ne dit rien (comme Owlery).
struct Watcher {
    info: ModuleInfo,
}

fn closing_info(id: &'static str) -> ModuleInfo {
    ModuleInfo { id, name: id, icon: "?", color: egui::Color32::GRAY, summary: "essai", origin: Origin::Native }
}

impl Module for Asker {
    fn info(&self) -> &ModuleInfo {
        &self.info
    }
    fn logic(&mut self, ctx: &egui::Context, _cx: &mut Cx) {
        let frame = CLOSE_LOG.with(|l| l.borrow().frame);
        if ctx.input(|i| i.viewport().close_requested()) {
            CLOSE_LOG.with(|l| l.borrow_mut().seen.push(("questionneur", frame)));
            if !self.asked {
                self.asked = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            }
        }
        if self.asked && CLOSE_LOG.with(|l| std::mem::take(&mut l.borrow_mut().consent)) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
    fn ui(&mut self, _ui: &mut egui::Ui, _view: &str, _cx: &mut Cx) {}
}

impl Module for Watcher {
    fn info(&self) -> &ModuleInfo {
        &self.info
    }
    fn logic(&mut self, ctx: &egui::Context, _cx: &mut Cx) {
        if ctx.input(|i| i.viewport().close_requested()) {
            let frame = CLOSE_LOG.with(|l| l.borrow().frame);
            CLOSE_LOG.with(|l| l.borrow_mut().seen.push(("veilleur", frame)));
        }
    }
    fn ui(&mut self, _ui: &mut egui::Ui, _view: &str, _cx: &mut Cx) {}
}

fn build_asker(_: &suite_sdk::BuildCx) -> Result<Box<dyn Module>, suite_sdk::Unavailable> {
    Ok(Box::new(Asker { info: closing_info("questionneur"), asked: false }))
}

fn build_watcher(_: &suite_sdk::BuildCx) -> Result<Box<dyn Module>, suite_sdk::Unavailable> {
    Ok(Box::new(Watcher { info: closing_info("veilleur") }))
}

/// Une image ; `close` : la demande de fermeture arrive (clic sur la croix, ou `Close` renvoyé par
/// la suite et transformé en événement par eframe).
fn close_frame(h: &mut Harness<'static, Suite>, close: bool) {
    CLOSE_LOG.with(|l| l.borrow_mut().frame += 1);
    if close {
        h.input_mut().viewports.entry(egui::ViewportId::ROOT).or_default().events.push(egui::ViewportEvent::Close);
    }
    h.step();
}

#[test]
fn modules_see_the_close_one_by_one_and_a_question_holds_the_others() {
    let mut h = suite();
    let ctx = h.ctx.clone();
    {
        let s = h.state_mut();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        type Build = fn(&suite_sdk::BuildCx) -> Result<Box<dyn Module>, suite_sdk::Unavailable>;
        let modules: [(ModuleInfo, Build); 2] = [(closing_info("questionneur"), build_asker), (closing_info("veilleur"), build_watcher)];
        for (info, build) in modules {
            let mut slot = Slot::new(info, build);
            slot.start(&cc, &s.core.host, suite_sdk::DataMode::Shared, &mut s.core.journal);
            s.core.slots.push(slot);
        }
    }
    // Ordre : carnet, questionneur, veilleur. Clic sur la croix : Carnet voit la fermeture.
    close_frame(&mut h, true);
    assert_eq!(h.state().core.quit.now.as_deref(), Some("carnet"));
    // La suite redemande la fermeture pour le suivant : le questionneur la voit et l'annule.
    close_frame(&mut h, true);
    // Il pose sa question : rien de plus, le veilleur n'a rien vu.
    close_frame(&mut h, false);
    close_frame(&mut h, false);
    assert!(CLOSE_LOG.with(|l| l.borrow().seen.iter().all(|(m, _)| *m != "veilleur")), "le veilleur ne voit rien tant que la question est posée");
    // L'utilisateur répond « quitter » : le questionneur demande la fermeture, la séquence reprend après lui.
    CLOSE_LOG.with(|l| l.borrow_mut().consent = true);
    close_frame(&mut h, false);
    close_frame(&mut h, true);
    let seen = CLOSE_LOG.with(|l| l.borrow().seen.clone());
    assert_eq!(seen.iter().filter(|(m, _)| *m == "questionneur").count(), 1, "{seen:?}");
    assert_eq!(seen.iter().filter(|(m, _)| *m == "veilleur").count(), 1, "{seen:?}");
}

#[test]
fn a_close_from_inside_a_pane_does_not_close_the_suite() {
    let mut h = suite();
    // Comme la croix de la barre de titre de Fawkes, dans sa case : `Close` hors de son tour.
    h.state_mut().core.quit.asked.push("carnet".into());
    close_frame(&mut h, true);
    let s = h.state();
    assert!(s.core.quit.sequence.is_none());
    assert!(s.core.journal.lines.iter().any(|l| l.text.contains("la suite reste ouverte")));
}

/// Demande un autre zoom à sa première image (comme DragonTree et Patronus à leur construction).
struct Zoomer {
    info: ModuleInfo,
    done: bool,
}

impl Module for Zoomer {
    fn info(&self) -> &ModuleInfo {
        &self.info
    }
    fn logic(&mut self, ctx: &egui::Context, _cx: &mut Cx) {
        if !self.done {
            self.done = true;
            ctx.set_zoom_factor(2.0);
        }
    }
    fn ui(&mut self, _ui: &mut egui::Ui, _view: &str, _cx: &mut Cx) {}
}

fn build_zoomer(_: &suite_sdk::BuildCx) -> Result<Box<dyn Module>, suite_sdk::Unavailable> {
    Ok(Box::new(Zoomer { info: closing_info("zoom"), done: false }))
}

#[test]
fn the_suite_zoom_comes_back_after_a_module_changes_it() {
    let mut h = suite();
    let ctx = h.ctx.clone();
    {
        let s = h.state_mut();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut slot = Slot::new(closing_info("zoom"), build_zoomer);
        slot.start(&cc, &s.core.host, suite_sdk::DataMode::Shared, &mut s.core.journal);
        s.core.slots.push(slot);
    }
    h.run_steps(4);
    assert!((ctx.zoom_factor() - 1.0).abs() < 1e-3, "zoom {}", ctx.zoom_factor());
}
