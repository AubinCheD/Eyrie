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
    let d =
        std::env::temp_dir().join(format!("eyrie-essai-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// La suite avec seulement Carnet activé.
fn suite() -> Harness<'static, Suite> {
    suite_of_size([1400.0, 900.0])
}

/// La même, dans une fenêtre de cette taille.
fn suite_of_size(size: [f32; 2]) -> Harness<'static, Suite> {
    let mut st = SuiteState { welcomed: true, ..Default::default() };
    for f in registry::factories() {
        st.modules.insert(f.info.id.to_string(), ModuleSettings { enabled: f.info.id == "carnet" });
    }
    let dir = temp_dir();
    let mut h = Harness::builder().with_size(size).build_eframe(move |cc| {
        Suite::new(cc, dir, suite_sdk::state::Loaded { value: st, problems: Vec::new(), writable: true })
    });
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
    let count = layout::panes(&s.main)
        .iter()
        .chain(s.windows.iter().flat_map(|w| layout::panes(&w.tree)).collect::<Vec<_>>().iter())
        .filter(|(_, p)| *p == notes())
        .count();
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
    h.state().core.host.publish(Event {
        source: "essai".into(),
        kind: "pr_ouverte".into(),
        data: serde_json::json!({}),
    });
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
    ModuleInfo {
        id: "boum",
        name: "Boum",
        icon: "!",
        color: egui::Color32::RED,
        summary: "essai",
        origin: Origin::Native,
    }
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
        let modules: [(ModuleInfo, Build); 2] =
            [(closing_info("questionneur"), build_asker), (closing_info("veilleur"), build_watcher)];
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
    assert!(
        CLOSE_LOG.with(|l| l.borrow().seen.iter().all(|(m, _)| *m != "veilleur")),
        "le veilleur ne voit rien tant que la question est posée"
    );
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
    assert!(s.core.journal.lines.iter().any(|l| l.text.contains("Eyrie reste ouvert")));
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

// --- Réglages de la suite -----------------------------------------------------------------------

/// Les textes des réglages restent courts et parlent à l'utilisateur, pas de la mécanique de la suite.
#[test]
fn settings_texts_have_no_implementation_jargon() {
    let keys = crate::i18n::keys_in(include_str!("settings.rs"));
    for jargon in
        ["sans changer son code", "Style rétabli", "commandes contenues", "verrou", "bac à sable", "injectable"]
    {
        let hits: Vec<&String> = keys.iter().filter(|k| k.contains(jargon)).collect();
        assert!(hits.is_empty(), "« {jargon} » : {hits:#?}");
    }
    let long: Vec<&String> = keys.iter().filter(|k| k.chars().count() > 140).collect();
    assert!(long.is_empty(), "aides trop longues : {long:#?}");
}

thread_local! {
    static RUN: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Un module qui a des réglages propres (commande `reglages`).
struct Tuned {
    info: ModuleInfo,
}

impl Module for Tuned {
    fn info(&self) -> &ModuleInfo {
        &self.info
    }

    fn logic(&mut self, _ctx: &egui::Context, _cx: &mut Cx) {}

    fn ui(&mut self, _ui: &mut egui::Ui, _view: &str, _cx: &mut Cx) {}

    fn commands(&self) -> Vec<suite_sdk::CommandInfo> {
        vec![suite_sdk::CommandInfo { id: "reglages".into(), title: "Réglages".into(), shortcut: None }]
    }

    fn run_command(&mut self, id: &str, _cx: &mut Cx) {
        RUN.with(|r| r.borrow_mut().push(id.to_string()));
    }

    fn settings_ui(&mut self, ui: &mut egui::Ui, _host: &suite_sdk::Host) -> bool {
        draw_tuned_settings(ui);
        true
    }

    fn settings_left(&mut self) {
        RUN.with(|r| r.borrow_mut().push("page quittée".into()));
    }
}

fn tuned_info() -> ModuleInfo {
    ModuleInfo {
        id: "regle",
        name: "Réglé",
        icon: "⚙",
        color: egui::Color32::LIGHT_BLUE,
        summary: "essai",
        origin: Origin::Native,
    }
}

fn build_tuned(_: &suite_sdk::BuildCx) -> Result<Box<dyn Module>, suite_sdk::Unavailable> {
    Ok(Box::new(Tuned { info: tuned_info() }))
}

/// Le module dessine ses réglages dans la page que lui donne Eyrie.
fn draw_tuned_settings(ui: &mut egui::Ui) {
    ui.label("réglage-témoin");
}

fn add_slot(
    h: &mut Harness<'static, Suite>,
    info: ModuleInfo,
    build: fn(&suite_sdk::BuildCx) -> Result<Box<dyn Module>, suite_sdk::Unavailable>,
) {
    let ctx = h.ctx.clone();
    let s = h.state_mut();
    let mut slot = Slot::new(info, build);
    let cc = eframe::CreationContext::_new_kittest(ctx);
    slot.start(&cc, &s.core.host, suite_sdk::DataMode::Shared, &mut s.core.journal);
    s.core.slots.push(slot);
}

#[test]
fn module_page_draws_the_module_settings() {
    use egui_kittest::kittest::Queryable;
    let mut h = suite();
    add_slot(&mut h, tuned_info(), build_tuned);
    act(&mut h, Action::OpenSettings(crate::settings::Page::Module("regle".into())));
    assert!(h.query_by_label("réglage-témoin").is_some(), "les réglages du module sont dans ceux d'Eyrie");
}

/// Un module dont le panneau de réglages panique.
struct Fragile {
    info: ModuleInfo,
}

impl Module for Fragile {
    fn info(&self) -> &ModuleInfo {
        &self.info
    }

    fn logic(&mut self, _ctx: &egui::Context, _cx: &mut Cx) {}

    fn ui(&mut self, _ui: &mut egui::Ui, _view: &str, _cx: &mut Cx) {}

    fn settings_ui(&mut self, ui: &mut egui::Ui, _host: &suite_sdk::Host) -> bool {
        ui.ctx().set_visuals(egui::Visuals::light());
        panic!("réglages cassés");
    }
}

fn fragile_info() -> ModuleInfo {
    ModuleInfo { id: "fragile", name: "Fragile", ..tuned_info() }
}

fn build_fragile(_: &suite_sdk::BuildCx) -> Result<Box<dyn Module>, suite_sdk::Unavailable> {
    Ok(Box::new(Fragile { info: fragile_info() }))
}

#[test]
fn a_panicking_settings_panel_stops_only_its_module() {
    crate::sandbox::install_panic_hook();
    let mut h = suite();
    add_slot(&mut h, fragile_info(), build_fragile);
    act(&mut h, Action::OpenSettings(crate::settings::Page::Module("fragile".into())));
    let s = h.state();
    assert!(matches!(s.core.slot("fragile").unwrap().state, SlotState::Crashed(_)));
    assert!(s.core.slot("carnet").unwrap().is_running());
    assert!(s.shell.settings.open, "les réglages d'Eyrie restent ouverts");
    assert!(h.ctx.global_style().visuals.dark_mode, "le style d'Eyrie est rétabli");
}

#[test]
fn settings_of_a_detached_window_list_only_its_modules() {
    let mut tiles = egui_tiles::Tiles::default();
    let a = tiles.insert_pane(Pane::module("carnet", "notes"));
    let b = tiles.insert_pane(Pane::Suite { view: SuiteView::Overview });
    let c = tiles.insert_pane(Pane::module("carnet", "bus"));
    let root = tiles.insert_tab_tile(vec![a, b, c]);
    let tree = egui_tiles::Tree::new(egui::Id::unique("essai"), root, tiles);
    assert_eq!(crate::settings::window_modules(&tree), ["carnet"]);
}

/// Carnet n'est plus que dans une fenêtre détachée (aucune case dans la principale) : son identifiant.
fn carnet_only_in_a_detached_window(h: &mut Harness<'static, Suite>) -> u64 {
    act(h, Action::Detach(notes()));
    let in_main: Vec<Pane> = layout::panes(&h.state().main)
        .into_iter()
        .map(|(_, p)| p)
        .filter(|p| p.module_id() == Some("carnet"))
        .collect();
    for pane in in_main {
        act(h, Action::ClosePane(pane));
    }
    h.state().windows.first().map(|w| w.id).expect("une fenêtre détachée")
}

#[test]
fn leaving_a_module_settings_page_tells_the_module() {
    let mut h = suite();
    add_slot(&mut h, tuned_info(), build_tuned);
    let left = || RUN.with(|r| r.borrow().iter().filter(|c| *c == "page quittée").count());
    act(&mut h, Action::OpenSettings(crate::settings::Page::Module("regle".into())));
    h.run_steps(2);
    assert_eq!(left(), 0, "page montrée : rien à abandonner");
    act(&mut h, Action::OpenSettings(crate::settings::Page::General));
    assert_eq!(left(), 1, "autre page : le module abandonne sa saisie et son brouillon");
    act(&mut h, Action::OpenSettings(crate::settings::Page::Module("regle".into())));
    h.state_mut().shell.settings.open = false;
    h.run_steps(3);
    assert_eq!(left(), 2, "fenêtre fermée : aussi");
}

/// La barre d'activité, la palette et la vue d'ensemble de la fenêtre principale passent par `Action::OpenSettings`.
#[test]
fn settings_asked_in_the_main_window_open_there() {
    let mut h = suite();
    carnet_only_in_a_detached_window(&mut h);
    act(&mut h, Action::OpenSettings(crate::settings::Page::Module("carnet".into())));
    let st = &h.state().shell.settings;
    assert!(st.open);
    assert_eq!(st.page, crate::settings::Page::Module("carnet".into()));
    assert_eq!(st.window, None, "demandés dans la principale : ouverts dans la principale");
}

#[test]
fn a_detached_module_opens_the_accounts_page_where_it_is() {
    let mut h = suite();
    let window = carnet_only_in_a_detached_window(&mut h);
    h.state_mut().core.focus.module = Some("carnet".into());
    h.state().core.host.open_settings("comptes");
    h.run_steps(3);
    let st = &h.state().shell.settings;
    assert_eq!(st.page, crate::settings::Page::Accounts);
    assert_eq!(st.window, Some(window), "« Comptes… » depuis sa fenêtre : dans sa fenêtre");
}

#[test]
fn a_module_opens_its_settings_page_where_it_is() {
    let mut h = suite();
    let window = carnet_only_in_a_detached_window(&mut h);
    h.state().core.host.open_settings("module:carnet");
    h.run_steps(3);
    let st = &h.state().shell.settings;
    assert!(st.open);
    assert_eq!(st.page, crate::settings::Page::Module("carnet".into()));
    assert_eq!(st.window, Some(window), "dans la fenêtre du module, pas dans la principale");
}

#[test]
fn removing_an_account_asks_for_confirmation() {
    use egui_kittest::kittest::Queryable;
    let mut h = suite();
    let dir = tempfile::tempdir().unwrap();
    let store = suite_accounts::Store::new(dir.path(), std::sync::Arc::new(suite_accounts::MemoryVault::default()));
    let ada = suite_accounts::Account {
        id: "aaaaaaaaaaaaaaaa".into(),
        forge: "github".into(),
        host: "github.com".into(),
        username: "ada".into(),
        ..Default::default()
    };
    store.add(ada, "t").unwrap();
    h.state_mut().core.accounts = crate::accounts::Accounts::new(store.clone());
    act(&mut h, Action::OpenSettings(crate::settings::Page::Accounts));
    h.get_by_label(crate::i18n::tr("Retirer")).click();
    h.run_steps(3);
    assert_eq!(store.list().len(), 1, "rien n'est retiré sans confirmation");
    h.get_by_label(crate::i18n::tr("Oui, retirer")).click();
    h.run_steps(3);
    assert!(store.list().is_empty());
}

/// L'application s'appelle Eyrie : « la suite » ne la désigne plus ; « toute la suite » (comptes, Tailscale) désigne
/// bien l'ensemble des applications.
#[test]
fn interface_texts_name_eyrie() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut wrong = Vec::new();
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        if path.file_name().is_some_and(|n| n == "i18n.rs" || n == "tests.rs") {
            continue;
        }
        for key in crate::i18n::keys_in(&std::fs::read_to_string(&path).unwrap()) {
            let lower = key.to_lowercase();
            let about_the_app = lower.replace("toute la suite", "").contains("la suite") || key == "Suite";
            if about_the_app || lower.contains("superapp") {
                wrong.push(key);
            }
        }
    }
    assert!(wrong.is_empty(), "textes qui nomment encore « la suite » ou SuperApp : {wrong:#?}");
}

#[test]
fn a_pending_removal_is_forgotten_when_the_page_is_left() {
    use egui_kittest::kittest::Queryable;
    let mut h = suite();
    let dir = tempfile::tempdir().unwrap();
    let store = suite_accounts::Store::new(dir.path(), std::sync::Arc::new(suite_accounts::MemoryVault::default()));
    let ada = suite_accounts::Account {
        id: "aaaaaaaaaaaaaaaa".into(),
        forge: "github".into(),
        host: "github.com".into(),
        username: "ada".into(),
        ..Default::default()
    };
    store.add(ada, "t").unwrap();
    h.state_mut().core.accounts = crate::accounts::Accounts::new(store);
    let confirm = crate::i18n::tr("Oui, retirer");
    // Autre page, puis fenêtre fermée.
    for close in [false, true] {
        act(&mut h, Action::OpenSettings(crate::settings::Page::Accounts));
        h.get_by_label(crate::i18n::tr("Retirer")).click();
        h.run_steps(3);
        assert!(h.query_by_label(confirm).is_some(), "confirmation demandée");
        if close {
            h.state_mut().shell.settings.open = false;
            h.run_steps(3);
        } else {
            act(&mut h, Action::OpenSettings(crate::settings::Page::General));
        }
        act(&mut h, Action::OpenSettings(crate::settings::Page::Accounts));
        assert!(h.query_by_label(confirm).is_none(), "page quittée : la demande est oubliée");
    }
}

#[test]
fn the_settings_window_fits_a_small_window() {
    let mut h = suite_of_size([520.0, 340.0]);
    act(&mut h, Action::OpenSettings(crate::settings::Page::General));
    h.run_steps(3);
    let window = h.ctx.memory(|m| m.area_rect(egui::Id::unique("suite-reglages"))).expect("fenêtre de réglages");
    let screen = h.ctx.content_rect().expand(1.0);
    assert!(screen.contains_rect(window), "{window:?} hors de {screen:?}");
}

#[test]
fn settings_of_a_detached_window_with_two_modules_list_both_only() {
    use egui_kittest::kittest::Queryable;
    let mut h = suite();
    add_slot(&mut h, tuned_info(), build_tuned);
    add_slot(&mut h, fragile_info(), build_fragile);
    let window = carnet_only_in_a_detached_window(&mut h);
    // Réglé rejoint Carnet dans la fenêtre détachée ; Fragile tourne, sans case.
    let w = h.state_mut().windows.iter_mut().find(|w| w.id == window).unwrap();
    let pane = w.tree.tiles.insert_pane(Pane::module("regle", suite_sdk::ViewInfo::MAIN));
    let root = w.tree.root.unwrap();
    if let Some(egui_tiles::Tile::Container(c)) = w.tree.tiles.get_mut(root) {
        c.add_child(pane);
    }
    act(&mut h, Action::OpenSettingsIn(crate::settings::Page::General, Some(window)));
    let entry = |s: &crate::core::Slot| format!("{}  {}", s.info.icon, s.info.name);
    let carnet = entry(h.state().core.slot("carnet").unwrap());
    assert!(h.query_by_label(&carnet).is_some(), "Carnet, dans la fenêtre");
    assert!(h.query_by_label("⚙  Réglé").is_some(), "Réglé, dans la fenêtre");
    assert!(h.query_by_label("⚙  Fragile").is_none(), "Fragile n'est pas dans cette fenêtre");
}

#[test]
fn a_successful_add_clears_the_form() {
    use egui_kittest::kittest::{NodeT, Queryable};
    let mut h = suite();
    act(&mut h, Action::OpenSettings(crate::settings::Page::Accounts));
    let token = h
        .query_all_by(|n| matches!(n.role(), egui::accesskit::Role::PasswordInput | egui::accesskit::Role::TextInput))
        .next()
        .expect("champ du jeton");
    token.focus();
    h.run_steps(2);
    h.query_all_by(|n| matches!(n.role(), egui::accesskit::Role::PasswordInput | egui::accesskit::Role::TextInput))
        .next()
        .unwrap()
        .type_text("secret-123");
    h.run_steps(2);
    let add = crate::i18n::tr("Vérifier et ajouter");
    assert!(!h.get_by_label(add).accesskit_node().is_disabled(), "jeton saisi : on peut l'ajouter");
    let user = dragontree_forges::VerifiedUser {
        username: "ada".into(),
        display_name: String::new(),
        avatar_url: None,
        orgs: Vec::new(),
    };
    h.state_mut().core.accounts.finish(dragontree_forges::ForgeKind::GitHub, "github.com", Ok(user), "secret-123");
    h.run_steps(3);
    assert!(h.get_by_label(add).accesskit_node().is_disabled(), "ajouté : le jeton a quitté le formulaire");
}
