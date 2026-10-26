//! Essais approfondis de l'hôte.
//!
//! - `random_actions_keep_the_layout_sound` : des centaines d'actions tirées au hasard (graine
//!   fixe), avec des clics à des endroits quelconques, et des invariants vérifiés après chacune ;
//! - `real_modules_*` (ignorés par défaut, lents, lancent de vrais terminaux) : les huit modules
//!   réels, en mode essai, dans un dossier temporaire. À lancer seuls :
//!   `cargo test -p eyrie -- --ignored --test-threads=1`.

use std::collections::BTreeSet;

use eframe::egui;
use egui_kittest::Harness;
use suite_sdk::host::Event;

use crate::app::{Action, Suite};
use crate::core::SlotState;
use crate::layout::{self, Pane, Preset, SuiteView};
use crate::persist::{ModuleData, ModuleSettings, SuiteState};
use crate::registry;

/// Générateur pseudo-aléatoire minimal (xorshift), pour une séquence reproductible.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }

    fn pick<'a, T>(&mut self, v: &'a [T]) -> &'a T {
        &v[self.below(v.len())]
    }
}

fn temp_dir(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("eyrie-approfondi-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn harness(st: SuiteState, dir: std::path::PathBuf) -> Harness<'static, Suite> {
    let mut h = Harness::builder().with_size([1500.0, 950.0]).build_eframe(move |cc| {
        Suite::new(cc, dir, suite_sdk::state::Loaded { value: st, problems: Vec::new(), writable: true })
    });
    h.run_steps(3);
    h
}

/// Toutes les cases ouvertes, fenêtre par fenêtre.
fn all_panes(s: &Suite) -> Vec<(u64, Pane)> {
    let mut v: Vec<(u64, Pane)> = layout::panes(&s.main).into_iter().map(|(_, p)| (0, p)).collect();
    for w in &s.windows {
        v.extend(layout::panes(&w.tree).into_iter().map(|(_, p)| (w.id, p)));
    }
    v
}

/// Ce qui doit toujours tenir, quoi qu'on ait fait.
fn check_invariants(s: &Suite, step: usize, what: &str) {
    let panes = all_panes(s);
    let mut seen = std::collections::HashSet::new();
    for (w, p) in &panes {
        assert!(seen.insert(p.clone()), "pas {step} ({what}) : {p:?} ouverte deux fois (fenêtre {w})");
        if let Some(m) = p.module_id() {
            assert!(s.core.slot(m).is_some(), "pas {step} ({what}) : module inconnu {m}");
        }
    }
    for w in &s.windows {
        assert!(!w.tree.is_empty(), "pas {step} ({what}) : fenêtre {} vide", w.id);
    }
    let ids: BTreeSet<u64> = s.windows.iter().map(|w| w.id).collect();
    assert_eq!(ids.len(), s.windows.len(), "pas {step} ({what}) : deux fenêtres ont le même numéro");
    if let Some(c) = &s.core.concentrate {
        assert_eq!(
            s.locate(c).map(|(w, _)| w),
            Some(0),
            "pas {step} ({what}) : case agrandie hors de la fenêtre principale"
        );
    }
    assert!(
        s.core.slots.iter().all(|sl| !matches!(sl.state, SlotState::Crashed(_))),
        "pas {step} ({what}) : un module s'est arrêté"
    );
}

#[test]
fn random_actions_keep_the_layout_sound() {
    for seed in [0x5eed_1234_abcd_0042, 0x0dd_c0ffee, 0x1234_5678_9abc_def1, 0xfeed_beef] {
        random_actions(seed, 400);
    }
}

fn random_actions(seed: u64, steps: usize) {
    let mut st = SuiteState { welcomed: true, ..Default::default() };
    for f in registry::factories() {
        st.modules.insert(f.info.id.to_string(), ModuleSettings { enabled: f.info.id == "carnet" });
    }
    let mut h = harness(st, temp_dir(&format!("hasard-{seed:x}")));
    let panes = vec![
        Pane::module("carnet", "notes"),
        Pane::module("carnet", "bus"),
        Pane::Suite { view: SuiteView::Overview },
        Pane::Suite { view: SuiteView::Notifications },
        Pane::Suite { view: SuiteView::Resources },
        Pane::Suite { view: SuiteView::Journal },
        // Un module désactivé : sa case montre qu'il l'est.
        Pane::module("fawkes", "principal"),
    ];
    let mut rng = Rng(seed);
    for step in 0..steps {
        let p = rng.pick(&panes).clone();
        let window = h.state().windows.get(rng.below(3)).map(|w| w.id).unwrap_or(99);
        let action = match rng.below(19) {
            0 | 1 => Action::Reveal(p),
            2 => Action::OpenInNewWindow(p),
            3 => Action::ClosePane(p),
            4 | 5 => Action::Detach(p),
            6 => Action::MoveToMain(p),
            7 => Action::AttachWindow(window),
            8 => Action::AttachAll,
            9 => Action::Concentrate(if rng.below(2) == 0 { Some(p) } else { None }),
            10 => Action::Preset(*rng.pick(&Preset::ALL)),
            11 => Action::SaveLayout(format!("essai-{}", rng.below(3))),
            12 => Action::LoadLayout(format!("essai-{}", rng.below(3))),
            13 => Action::TogglePalette,
            14 => Action::SetTheme(*rng.pick(&suite_sdk::theme::ThemeChoice::ALL)),
            15 => Action::CycleCase(rng.below(2) == 0),
            16 => Action::FocusModule(rng.below(3)),
            17 => {
                if rng.below(2) == 0 {
                    Action::StopModule("carnet".into())
                } else {
                    Action::StartModule("carnet".into())
                }
            }
            _ => Action::LoadLayout(rng.pick(&["Dev", "Revue", "Exploitation"]).to_string()),
        };
        let what = format!("graine {seed:#x} : {action:?}");
        h.state_mut().core.act(action);
        // Parfois un événement du bus, parfois un clic quelque part.
        if rng.below(4) == 0 {
            h.state().core.host.publish(Event {
                source: "essai".into(),
                kind: "essai".into(),
                data: serde_json::json!({ "pas": step }),
            });
        }
        if rng.below(3) == 0 {
            let pos = egui::pos2(rng.below(1500) as f32, rng.below(950) as f32);
            let input = h.input_mut();
            input.events.push(egui::Event::PointerMoved(pos));
            input.events.push(egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: Default::default(),
            });
            input.events.push(egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: Default::default(),
            });
        }
        h.run_steps(2);
        check_invariants(h.state(), step, &what);
    }
}

// --- Les huit modules réels -------------------------------------------------------------------

/// La suite avec tous les modules, en mode essai dans un dossier temporaire (variables posées
/// avant toute construction, comme `eyrie.exe`), Owlery en démonstration.
fn real_suite(name: &str) -> (Harness<'static, Suite>, std::path::PathBuf) {
    let dir = temp_dir(name);
    for (module, var) in [
        ("fawkes", "FAWKES_DATA_DIR"),
        ("owlery", "OWLERY_DATA_DIR"),
        ("grimoire", "GRIMOIRE_DATA_DIR"),
        ("menagerie", "MENAGERIE_DATA_DIR"),
        ("patronus", "PATRONUS_DATA_DIR"),
    ] {
        let d = dir.join("essai").join(module);
        std::fs::create_dir_all(&d).unwrap();
        unsafe { std::env::set_var(var, &d) };
    }
    unsafe { std::env::set_var("OWLERY_DEMO", "1") };
    let st = SuiteState { welcomed: true, module_data: ModuleData::Trial, ..Default::default() };
    let h = harness(st, dir.clone());
    (h, dir)
}

fn running(s: &Suite) -> Vec<&'static str> {
    s.core.slots.iter().filter(|sl| sl.is_running()).map(|sl| sl.info.id).collect()
}

#[test]
#[ignore = "lent : les huit modules réels (terminaux, relevés) ; à lancer seul"]
fn real_modules_survive_every_layout_operation() {
    let (mut h, _dir) = real_suite("reels");
    h.run_steps(10);
    let started = running(h.state());
    assert_eq!(
        started.len(),
        8,
        "tous les modules démarrent en mode essai : {started:?} ; journal : {:?}",
        h.state().core.journal.lines.iter().map(|l| l.text.clone()).collect::<Vec<_>>()
    );
    let mains: Vec<Pane> =
        started.iter().filter(|m| **m != "carnet").map(|m| Pane::module(m, suite_sdk::ViewInfo::MAIN)).collect();

    // Toutes les dispositions, chacune dessinée plusieurs images.
    for kind in Preset::ALL {
        h.state_mut().core.act(Action::Preset(kind));
        h.run_steps(4);
        check_invariants(h.state(), 0, &format!("{kind:?}"));
    }
    // Chaque module agrandi, détaché, rattaché.
    for p in &mains {
        for a in [
            Action::Concentrate(Some(p.clone())),
            Action::Concentrate(None),
            Action::Detach(p.clone()),
            Action::MoveToMain(p.clone()),
        ] {
            let what = format!("{a:?}");
            h.state_mut().core.act(a);
            h.run_steps(3);
            check_invariants(h.state(), 0, &what);
        }
    }
    // Tous détachés à la fois, puis rattachés.
    for p in &mains {
        h.state_mut().core.act(Action::Detach(p.clone()));
    }
    h.run_steps(5);
    assert_eq!(h.state().windows.len(), mains.len());
    h.state_mut().core.act(Action::AttachAll);
    h.run_steps(3);
    assert!(h.state().windows.is_empty());
    // Les trois thèmes : le style de la suite tient malgré les modules qui posent le leur.
    for t in [
        suite_sdk::theme::ThemeChoice::Fluent,
        suite_sdk::theme::ThemeChoice::Nuit,
        suite_sdk::theme::ThemeChoice::Graphite,
    ] {
        h.state_mut().core.act(Action::SetTheme(t));
        h.state_mut().core.act(Action::Preset(Preset::Grid));
        h.run_steps(4);
        let want = suite_sdk::theme::palette(suite_sdk::theme::resolve(t, &h.ctx));
        assert_eq!(h.ctx.global_style().visuals.window_fill, want.window, "{t:?}");
        assert!((h.ctx.zoom_factor() - 1.0).abs() < 1e-3, "zoom {} après {t:?}", h.ctx.zoom_factor());
    }
    check_invariants(h.state(), 0, "thèmes");
}

#[test]
#[ignore = "lent : les huit modules réels ; à lancer seul"]
fn real_modules_can_be_stopped_and_restarted() {
    let (mut h, _dir) = real_suite("relance");
    h.run_steps(6);
    for id in running(h.state()) {
        h.state_mut().core.act(Action::StopModule(id.to_string()));
        h.run_steps(3);
        assert!(matches!(h.state().core.slot(id).unwrap().state, SlotState::Disabled), "{id} arrêté");
        h.state_mut().core.act(Action::StartModule(id.to_string()));
        h.state_mut().core.act(Action::Reveal(Pane::module(id, suite_sdk::ViewInfo::MAIN)));
        h.run_steps(4);
        let state = &h.state().core.slot(id).unwrap().state;
        assert!(!matches!(state, SlotState::Crashed(_)), "{id} relancé sans panique");
        assert!(
            matches!(state, SlotState::Running(_)),
            "{id} relancé : {}",
            match state {
                SlotState::Unavailable(u) => crate::app::unavailable_text(u),
                _ => String::new(),
            }
        );
    }
}

#[test]
#[ignore = "lent : les huit modules réels ; à lancer seul"]
fn real_modules_let_the_suite_close() {
    let (mut h, _dir) = real_suite("fermeture");
    h.run_steps(6);
    let n = running(h.state()).len();
    // La croix : chaque module voit la fermeture à son tour ; en mode essai, aucun n'a de travail
    // en cours, la séquence va jusqu'au bout.
    let mut seen = Vec::new();
    for _ in 0..(n + 3) {
        h.input_mut().viewports.entry(egui::ViewportId::ROOT).or_default().events.push(egui::ViewportEvent::Close);
        h.step();
        if let Some(m) = h.state().core.quit.now.clone() {
            seen.push(m);
        }
    }
    assert_eq!(seen.len(), n, "chaque module voit la fermeture une fois : {seen:?}");
    assert_eq!(seen.last().map(String::as_str), Some("dragontree"), "DragonTree en dernier : {seen:?}");
    let seq = h.state().core.quit.sequence.as_ref().expect("séquence en cours");
    assert!(seq.is_done(), "personne n'a retenu la fermeture");
}

#[test]
#[ignore = "lent : les huit modules réels ; à lancer seul"]
fn real_dragontree_shares_its_active_repository() {
    let (mut h, dir) = real_suite("contexte");
    h.run_steps(6);
    // Comme `dragontree.exe <dépôt>` lancé à côté : la demande passe par le canal de l'instance
    // unique, que la suite tient pour DragonTree.
    let repo = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap();
    let key = dragontree_app::single_instance::profile_key(&dir.join("essai").join("dragontree"));
    let forwarded = dragontree_app::single_instance::Forwarded { open: vec![repo.clone()] };
    std::thread::spawn(move || {
        let _ = dragontree_app::single_instance::start(&key, &forwarded, std::time::Duration::from_secs(10));
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    let same = |c: &suite_sdk::host::SharedContext| c.dir.canonicalize().ok().as_deref() == Some(repo.as_path());
    while !h.state().core.host.context().is_some_and(|c| same(&c)) {
        assert!(std::time::Instant::now() < deadline, "contexte jamais publié : {:?}", h.state().core.host.context());
        h.step();
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let ctx = h.state().core.host.context().unwrap();
    assert_eq!(ctx.source, "dragontree");
    h.run_steps(3);
    let carnet =
        h.state().core.slot("carnet").and_then(|s| s.module()).map(|m| m.status().title).flatten().unwrap_or_default();
    assert!(carnet.ends_with(&ctx.label), "le Carnet suit le dépôt : {carnet}");
}

/// Les réglages de chaque module réel sont dans ceux d'Eyrie (sa page), sans le thème (celui d'Eyrie) ; et son propre
/// bouton de réglages mène à cette page.
#[test]
#[ignore = "lent : les huit modules réels ; à lancer seul"]
fn real_modules_show_their_settings_in_eyrie() {
    use egui_kittest::kittest::Queryable;
    let (mut h, _dir) = real_suite("reglages");
    h.run_steps(10);
    for (id, marker, command) in [
        ("dragontree", "Enregistrer", "reglages"),
        ("owlery", "Lecture périodique", "reglages"),
        ("fawkes", "Taille de police", "reglages"),
        ("patronus", "Conserver les résultats lus", "reglages"),
        ("menagerie", "Commande des conteneurs", "reglages"),
        ("grimoire", "Chemin de fawkes.exe :", "preferences"),
    ] {
        h.state_mut().core.act(Action::OpenSettings(crate::settings::Page::Module(id.into())));
        h.run_steps(4);
        assert!(h.state().core.slot(id).is_some_and(|s| s.is_running()), "{id} : toujours actif");
        assert!(h.query_by_label(marker).is_some(), "{id} : ses réglages dans ceux d'Eyrie");
        assert!(h.query_by_label("Thème").is_none(), "{id} : le thème est celui d'Eyrie");

        h.state_mut().shell.settings.open = false;
        h.state_mut().core.act(Action::RunCommand { module: id.into(), command: command.into() });
        h.run_steps(8);
        let st = &h.state().shell.settings;
        assert!(st.open, "{id} : son bouton de réglages ouvre ceux d'Eyrie");
        assert_eq!(st.page, crate::settings::Page::Module(id.into()), "{id} : sur sa page");
    }
}

/// Un raccourci en cours de saisie dans la page d'un module réel est abandonné quand la page est quittée : sinon la
/// frappe suivante, dans sa case (le terminal de Fawkes), deviendrait ce raccourci.
#[test]
#[ignore = "lent : les huit modules réels ; à lancer seul"]
fn real_modules_forget_an_unfinished_shortcut_when_their_page_is_left() {
    use egui_kittest::kittest::Queryable;
    let (mut h, _dir) = real_suite("saisie");
    h.run_steps(10);
    // Owlery range ses raccourcis dans une section repliée : la déplier d'abord (le dernier « Raccourcis » : celui de
    // la page, après celui de la navigation d'Eyrie).
    for (id, waiting, unfold) in
        [("fawkes", "Appuyez…", None), ("owlery", "Appuyez… (Échap : annuler)", Some("Raccourcis"))]
    {
        let page = crate::settings::Page::Module(id.into());
        h.state_mut().core.act(Action::OpenSettings(page.clone()));
        h.run_steps(4);
        if let Some(section) = unfold {
            h.query_all_by_label(section)
                .last()
                .unwrap_or_else(|| panic!("{id} : section {section}"))
                .click_accesskit();
            h.run_steps(3);
        }
        // Un emplacement libre (« — ») : cliquer lance la saisie.
        h.query_all_by_role_and_label(egui::accesskit::Role::Button, "—")
            .next()
            .unwrap_or_else(|| panic!("{id} : un raccourci libre"))
            .click_accesskit();
        h.run_steps(3);
        assert!(h.query_by_label(waiting).is_some(), "{id} : saisie lancée");
        h.state_mut().core.act(Action::OpenSettings(crate::settings::Page::General));
        h.run_steps(3);
        h.state_mut().core.act(Action::OpenSettings(page));
        h.run_steps(3);
        assert!(h.query_by_label(waiting).is_none(), "{id} : page quittée, saisie abandonnée");
    }
}

/// Dans la page de Menagerie, rien ne se ferme : le bouton du bas remet les champs aux réglages enregistrés, et le dit.
#[test]
#[ignore = "lent : les huit modules réels ; à lancer seul"]
fn menagerie_in_eyrie_cancels_instead_of_closing() {
    use egui_kittest::kittest::Queryable;
    let (mut h, _dir) = real_suite("menagerie");
    h.run_steps(10);
    // D'autres « Fermer » et « Annuler » peuvent être à l'écran : on compte ceux qu'ajoute la page.
    let count = |h: &egui_kittest::Harness<'static, Suite>, label: &str| h.query_all_by_label(label).count();
    let (close, cancel) = (count(&h, "Fermer"), count(&h, "Annuler"));
    h.state_mut().core.act(Action::OpenSettings(crate::settings::Page::Module("menagerie".into())));
    h.run_steps(4);
    assert!(h.query_by_label("Commande des conteneurs").is_some(), "sa page");
    assert_eq!(count(&h, "Fermer"), close, "rien à fermer dans la page d'Eyrie");
    assert_eq!(count(&h, "Annuler"), cancel + 1);
}
