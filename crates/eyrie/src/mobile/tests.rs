//! Le pont de bout en bout : une vraie suite (egui_kittest) avec un module factice, et un faux téléphone dans un
//! fil, qui s'appaire, suit un écran, agit, suit un terminal et y écrit.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use eframe::egui;
use egui_kittest::Harness;
use suite_mobile::channel::Keypair;
use suite_mobile::client;
use suite_mobile::msg::{ToPc, ToPhone};
use suite_mobile::pairing::Ticket;
use suite_mobile::term::{Line, TermFrame, TermInput, TermView};
use suite_mobile::ui::{Act, Row, Screen};
use suite_sdk::{Badge, CommandInfo, Cx, Module, ModuleInfo, ModuleStatus, Origin, Tone};

use crate::app::Suite;
use crate::core::Slot;
use crate::persist::{ModuleSettings, SuiteState};
use crate::registry;

static DONE: AtomicU32 = AtomicU32::new(0);
static TYPED: Mutex<Vec<TermInput>> = Mutex::new(Vec::new());

struct Factice {
    info: ModuleInfo,
}

fn info() -> ModuleInfo {
    ModuleInfo {
        id: "factice",
        name: "Factice",
        icon: "F",
        logo: None,
        color: egui::Color32::from_rgb(0x12, 0x34, 0x56),
        summary: "essai du pont",
        origin: Origin::Native,
    }
}

fn build(_: &suite_sdk::BuildCx) -> Result<Box<dyn Module>, suite_sdk::Unavailable> {
    Ok(Box::new(Factice { info: info() }))
}

impl Module for Factice {
    fn info(&self) -> &ModuleInfo {
        &self.info
    }

    fn logic(&mut self, _ctx: &egui::Context, _cx: &mut Cx) {}

    fn ui(&mut self, _ui: &mut egui::Ui, _view: &str, _cx: &mut Cx) {}

    fn status(&self) -> ModuleStatus {
        ModuleStatus {
            badge: Some(Badge::count(1, Tone::Warn, "une chose attend")),
            waiting: vec!["Une chose attend".into()],
            ..Default::default()
        }
    }

    fn commands(&self) -> Vec<CommandInfo> {
        vec![CommandInfo { id: "bonjour".into(), title: "Dire bonjour".into(), shortcut: None }]
    }

    fn mobile_screen(&mut self, path: &str, _cx: &mut Cx) -> Option<Screen> {
        match path {
            "" => {
                let mut s = Screen::new("Factice");
                s.section("Choses", "1").row(
                    Row::new(format!("Faite {} fois", DONE.load(Ordering::Relaxed)))
                        .tap(Act::new("faire", "Faire"))
                        .open("detail"),
                );
                Some(s)
            }
            "detail" => Some(Screen::new("Détail")),
            _ => None,
        }
    }

    fn mobile_action(
        &mut self,
        _path: &str,
        action: &str,
        _input: Option<&str>,
        _cx: &mut Cx,
    ) -> Result<Option<String>, String> {
        match action {
            "faire" => {
                DONE.fetch_add(1, Ordering::Relaxed);
                Ok(Some("fait".into()))
            }
            _ => Err("inconnue".into()),
        }
    }

    fn mobile_term(&mut self, id: &str, _history: usize) -> Option<TermView> {
        (id == "1").then(|| {
            let typed = TYPED.lock().unwrap().len();
            TermView {
                cols: 40,
                rows: 2,
                lines: vec![Line::plain("$ bonjour"), Line::plain(format!("{typed} saisies"))],
                ..Default::default()
            }
        })
    }

    fn mobile_term_input(&mut self, id: &str, input: &TermInput, _cx: &mut Cx) -> Result<(), String> {
        if id != "1" {
            return Err("pas ce terminal".into());
        }
        TYPED.lock().unwrap().push(input.clone());
        Ok(())
    }
}

fn suite() -> Harness<'static, Suite> {
    let mut st = SuiteState { welcomed: true, ..Default::default() };
    for f in registry::factories() {
        st.modules.insert(f.info.id.to_string(), ModuleSettings { enabled: false });
    }
    st.mobile.enabled = true;
    st.mobile.show_actions = true;
    static N: AtomicU32 = AtomicU32::new(0);
    let dir =
        std::env::temp_dir().join(format!("eyrie-mobile-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut h = Harness::builder().with_size([1200.0, 800.0]).build_eframe(move |cc| {
        let mut s = Suite::new(cc, dir, suite_sdk::state::Loaded { value: st, problems: Vec::new(), writable: true });
        s.core.mobile.listen_locally();
        s
    });
    let ctx = h.ctx.clone();
    {
        let s = h.state_mut();
        let mut slot = Slot::new(info(), build);
        let cc = eframe::CreationContext::_new_kittest(ctx);
        slot.start(&cc, &s.core.host, suite_sdk::DataMode::Shared, &mut s.core.journal);
        s.core.slots.push(slot);
    }
    h.run_steps(3);
    h
}

/// Fait tourner la suite jusqu'à ce que `done` dise oui (le téléphone travaille dans son fil).
fn spin(h: &mut Harness<'static, Suite>, mut done: impl FnMut(&mut Harness<'static, Suite>) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !done(h) {
        assert!(Instant::now() < deadline, "délai dépassé");
        h.run_steps(1);
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Lit jusqu'au message que `pick` retient.
fn until<T>(c: &mut client::Connection, mut pick: impl FnMut(ToPhone) -> Option<T>) -> T {
    loop {
        let m: ToPhone = c.rx.recv().expect("message du PC");
        if let Some(t) = pick(m) {
            return t;
        }
    }
}

#[test]
fn a_phone_pairs_follows_acts_and_types_and_the_pc_sees_it() {
    let mut h = suite();
    assert!(h.state().core.mobile.listening_on().is_some(), "{:?}", h.state().core.mobile.error);
    h.state_mut().core.mobile.new_ticket(None).unwrap();
    let uri = h.state().core.mobile.ticket.as_ref().unwrap().uri.clone();
    let ticket = Ticket::from_uri(&uri).unwrap();

    let phone = std::thread::spawn(move || {
        let me = Keypair::generate();
        let mut c = client::open(&ticket.addresses(), &me, Duration::from_secs(5)).unwrap();
        let sas = client::pair(&mut c, &ticket.secret, Some(&ticket.key), "Pixel", "Google Pixel 8").unwrap();
        client::wait_paired(&mut c).unwrap();
        let welcome = client::hello(&mut c, &ticket.key, "Pixel", "Google Pixel 8").unwrap();
        assert!(welcome.rights.act && welcome.rights.terminals);

        // Les cartes arrivent d'elles-mêmes, avec la pastille du module.
        let cards = until(&mut c, |m| match m {
            ToPhone::Modules { modules } => Some(modules),
            _ => None,
        });
        let card = cards.iter().find(|c| c.id == "factice").expect("carte du module");
        assert!(card.running);
        assert_eq!(card.color, 0x123456);
        assert_eq!(card.badge.as_ref().and_then(|b| b.count), Some(1));

        // L'écran suivi, avec « Montrer sur le PC » ajouté par l'hôte.
        c.tx.send(&ToPc::Follow { screen: Some(("factice".into(), "".into())), terminal: None }).unwrap();
        let screen = until(&mut c, |m| match m {
            ToPhone::Screen { module, screen, .. } if module == "factice" => Some(screen),
            _ => None,
        });
        assert!(screen.action_ids().contains("faire") && screen.action_ids().contains("@pc"));

        // Une action proposée est faite ; l'écran revient changé.
        c.tx.send(&ToPc::Action {
            req: 1,
            module: "factice".into(),
            path: "".into(),
            action: "faire".into(),
            input: None,
        })
        .unwrap();
        let r = until(&mut c, |m| match m {
            ToPhone::Result { req: 1, ok, message } => Some((ok, message)),
            _ => None,
        });
        assert_eq!(r, (true, "fait".to_string()));
        let again = until(&mut c, |m| match m {
            ToPhone::Screen { screen, .. } => Some(screen),
            _ => None,
        });
        assert!(format!("{again:?}").contains("Faite 1 fois"));

        // Une action que l'écran ne proposait pas est refusée sans atteindre le module.
        c.tx.send(&ToPc::Action {
            req: 2,
            module: "factice".into(),
            path: "".into(),
            action: "effacer".into(),
            input: None,
        })
        .unwrap();
        let r = until(&mut c, |m| match m {
            ToPhone::Result { req: 2, ok, .. } => Some(ok),
            _ => None,
        });
        assert!(!r);

        // Le terminal : une image complète, puis la différence après une saisie.
        c.tx.send(&ToPc::Follow { screen: None, terminal: Some(("factice".into(), "1".into())) }).unwrap();
        let first: TermFrame = until(&mut c, |m| match m {
            ToPhone::Term(f) => Some(f),
            _ => None,
        });
        assert_eq!(first.changed.len(), 2);
        let mut lines = Vec::new();
        first.apply(&mut lines);
        c.tx.send(&ToPc::Terminal { module: "factice".into(), id: "1".into(), input: TermInput::Text("ls".into()) })
            .unwrap();
        let next: TermFrame = until(&mut c, |m| match m {
            ToPhone::Term(f) => Some(f),
            _ => None,
        });
        next.apply(&mut lines);
        assert_eq!(lines[1].text(), "1 saisies");
        assert_eq!(next.changed.len(), 1, "seule la ligne changée part");
        // Une saisie pour un terminal que le téléphone ne suit pas est ignorée.
        c.tx.send(&ToPc::Terminal { module: "factice".into(), id: "2".into(), input: TermInput::Text("x".into()) })
            .unwrap();
        c.tx.send(&ToPc::Ping).unwrap();
        until(&mut c, |m| matches!(m, ToPhone::Pong).then_some(()));
        sas
    });

    // Le PC : la demande d'appairage apparaît, l'utilisateur confirme.
    spin(&mut h, |h| !h.state().core.mobile.requests.is_empty());
    let shown_sas = h.state().core.mobile.requests[0].sas.clone();
    h.state_mut().core.mobile.decide(0, true);
    spin(&mut h, |_| phone.is_finished());
    let sas = phone.join().expect("le téléphone a tout vu");
    assert_eq!(sas, shown_sas, "le même code des deux côtés");

    let s = h.state();
    assert_eq!(s.core.mobile.store.devices.len(), 1);
    assert_eq!(DONE.load(Ordering::Relaxed), 1);
    assert_eq!(
        *TYPED.lock().unwrap(),
        vec![TermInput::Text("ls".into())],
        "la saisie pour le terminal 2 n'est pas passée"
    );
    let journal = s.core.mobile.store.read_journal(50);
    assert!(journal.iter().any(|l| l.kind == "action" && l.text.contains("Factice › Faire")), "{journal:?}");
    assert!(journal.iter().any(|l| l.kind == "terminal"));
    assert!(s.core.notes.toasts.iter().any(|(n, _)| n.body.contains("Factice › Faire")), "bulle sur le PC");
}

#[test]
fn turning_access_off_closes_everything() {
    let mut h = suite();
    h.state_mut().core.mobile.new_ticket(None).unwrap();
    assert!(h.state().core.mobile.ticket.is_some());
    h.state_mut().core.st.mobile.enabled = false;
    h.run_steps(2);
    let m = &mut h.state_mut().core.mobile;
    assert!(m.listening_on().is_none());
    assert!(m.ticket.is_none(), "plus de ticket");
    assert!(m.new_ticket(None).is_err(), "pas de ticket sans accès");
}
