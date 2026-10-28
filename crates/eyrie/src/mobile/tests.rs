//! Le pont de bout en bout : une vraie suite (egui_kittest) avec un module factice, et un faux téléphone dans un
//! fil, qui s'appaire, suit un écran, agit, suit un terminal et y écrit, parcourt les dossiers du PC et en choisit un.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eframe::egui;
use egui_kittest::Harness;
use suite_mobile::channel::Keypair;
use suite_mobile::client;
use suite_mobile::msg::{ENTRY_DENIED, ENTRY_REPO, Listing, ToPc, ToPhone};
use suite_mobile::pairing::Ticket;
use suite_mobile::term::{Line, TermFrame, TermInput, TermView};
use suite_mobile::ui::{Act, FOLDER_REPO, FolderInput, Row, Screen};
use suite_sdk::{Badge, CommandInfo, Cx, Module, ModuleInfo, ModuleStatus, Notification, Origin, Tone, Unavailable};

use super::browse;
use crate::app::Suite;
use crate::core::{Slot, SlotState};
use crate::persist::{ModuleSettings, SuiteState};
use crate::registry;

static DONE: AtomicU32 = AtomicU32::new(0);
static TYPED: Mutex<Vec<TermInput>> = Mutex::new(Vec::new());
/// Les dossiers reçus par l'action « choisir ».
static CHOSEN: Mutex<Vec<String>> = Mutex::new(Vec::new());

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

    fn mobile_waiting(&self) -> Vec<(String, Option<String>)> {
        vec![("Une chose attend".into(), Some("detail".into()))]
    }

    fn commands(&self) -> Vec<CommandInfo> {
        vec![CommandInfo { id: "bonjour".into(), title: "Dire bonjour".into(), shortcut: None }]
    }

    fn mobile_screen(&mut self, path: &str, _cx: &mut Cx) -> Option<Screen> {
        match path {
            "" => {
                let mut s = Screen::new("Factice");
                s.section("Choses", "1")
                    .row(
                        Row::new(format!("Faite {} fois", DONE.load(Ordering::Relaxed)))
                            .tap(Act::new("faire", "Faire"))
                            .open("detail"),
                    )
                    .row(
                        Row::new("Un dépôt").tap(
                            Act::new("choisir", "Choisir un dépôt")
                                .folder("Dépôt", FolderInput { want: FOLDER_REPO, ..Default::default() }),
                        ),
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
        input: Option<&str>,
        _cx: &mut Cx,
    ) -> Result<Option<String>, String> {
        match action {
            "faire" => {
                DONE.fetch_add(1, Ordering::Relaxed);
                Ok(Some("fait".into()))
            }
            "choisir" => {
                CHOSEN.lock().unwrap().push(input.unwrap_or_default().to_string());
                Ok(None)
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
        // Ce qui attend mène à son écran.
        let waiting = until(&mut c, |m| match m {
            ToPhone::Overview { waiting } => Some(waiting),
            _ => None,
        });
        let w = waiting.iter().find(|w| w.module == "factice").expect("ce qui attend");
        assert_eq!((w.text.as_str(), w.path.as_deref()), ("Une chose attend", Some("detail")));

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

/// Un téléphone appairé à la suite, qui fait `phone` dans son fil une fois connecté ; `paired` agit sur le PC juste
/// après l'appairage, avant que la connexion du téléphone n'y soit reçue.
fn with_phone<T: Send + 'static>(
    h: &mut Harness<'static, Suite>,
    paired: impl FnOnce(&mut Suite),
    phone: impl FnOnce(&mut client::Connection) -> T + Send + 'static,
) -> T {
    h.state_mut().core.mobile.new_ticket(None).unwrap();
    let uri = h.state().core.mobile.ticket.as_ref().unwrap().uri.clone();
    let ticket = Ticket::from_uri(&uri).unwrap();
    let t = std::thread::spawn(move || {
        let me = Keypair::generate();
        let mut c = client::open(&ticket.addresses(), &me, Duration::from_secs(5)).unwrap();
        client::pair(&mut c, &ticket.secret, Some(&ticket.key), "Pixel", "Google Pixel 8").unwrap();
        client::wait_paired(&mut c).unwrap();
        client::hello(&mut c, &ticket.key, "Pixel", "Google Pixel 8").unwrap();
        phone(&mut c)
    });
    spin(h, |h| !h.state().core.mobile.requests.is_empty());
    h.state_mut().core.mobile.decide(0, true);
    paired(h.state_mut());
    spin(h, |_| t.is_finished());
    t.join().expect("le téléphone a tout vu")
}

/// Suit l'accueil du module factice et attend son écran.
fn follow_home(c: &mut client::Connection) -> Screen {
    c.tx.send(&ToPc::Follow { screen: Some(("factice".into(), "".into())), terminal: None }).unwrap();
    until(c, |m| match m {
        ToPhone::Screen { module, screen, .. } if module == "factice" => Some(screen),
        _ => None,
    })
}

/// Liste `dir` (`None` : la racine) pour l'action `action` de l'accueil du module factice.
fn list_folder(c: &mut client::Connection, req: u32, action: &str, dir: Option<&Path>) -> Listing {
    c.tx.send(&ToPc::Browse {
        req,
        module: "factice".into(),
        path: "".into(),
        action: action.into(),
        dir: dir.map(|d| d.to_string_lossy().into_owned()),
        filter: String::new(),
        offset: 0,
        hidden: false,
    })
    .unwrap();
    until(c, |m| match m {
        ToPhone::Folder { req: r, listing } if r == req => Some(listing),
        _ => None,
    })
}

fn act(c: &mut client::Connection, req: u32, action: &str, input: &str) -> (bool, String) {
    let module = "factice".into();
    c.tx.send(&ToPc::Action { req, module, path: "".into(), action: action.into(), input: Some(input.into()) })
        .unwrap();
    until(c, |m| match m {
        ToPhone::Result { req: r, ok, message } if r == req => Some((ok, message)),
        _ => None,
    })
}

#[test]
fn a_phone_browses_the_pc_only_for_a_folder_action_and_its_choice_is_checked() {
    let tree = tempfile::tempdir().unwrap();
    let root: PathBuf = tree.path().to_path_buf();
    for d in ["depot/.git", "simple", "zone"] {
        std::fs::create_dir_all(root.join(d)).unwrap();
    }
    let mut h = suite();
    // Le dossier temporaire est sous AppData, interdit pour de vrai : ici, seul `zone` l'est.
    h.state_mut().core.mobile.folders = Some(Arc::new(browse::Env::for_tests(vec![root.join("zone")])));
    let depot = with_phone(
        &mut h,
        |_| {},
        move |c| {
            assert!(follow_home(c).action_ids().contains("choisir"));
            // Rien pour une action qui ne demande pas de dossier.
            let l = list_folder(c, 1, "faire", Some(&root));
            assert!(l.error.contains("changé") && l.entries.is_empty(), "{l:?}");
            // L'arbre, ses drapeaux, et pourquoi il ne se choisit pas : ce n'est pas un dépôt.
            let l = list_folder(c, 2, "choisir", Some(&root));
            assert_eq!(l.error, "");
            let flags = |name: &str| l.entries.iter().find(|e| e.name == name).map(|e| e.flags);
            assert_eq!((flags("depot"), flags("zone")), (Some(ENTRY_REPO), Some(ENTRY_DENIED)));
            assert_eq!(l.choose, None);
            let l = list_folder(c, 3, "choisir", Some(&root.join("depot")));
            assert_eq!(l.choose.as_deref(), Some("Choisir ce dépôt"));
            let depot = l.dir;
            assert!(list_folder(c, 4, "choisir", Some(&root.join("zone"))).error.contains("protégé"));
            // Le choix est revérifié avant le module : ni un dossier qui n'est pas un dépôt, ni un chemin réseau.
            let (ok, message) = act(c, 5, "choisir", &root.join("simple").to_string_lossy());
            assert!(!ok && message.contains("pas un dépôt git"), "{message}");
            let (ok, message) = act(c, 6, "choisir", r"\\serveur\partage\depot");
            assert!(!ok && message.contains("réseau"), "{message}");
            let (ok, message) = act(c, 7, "choisir", &root.join("depot").to_string_lossy());
            assert!(ok, "{message}");
            depot
        },
    );
    assert_eq!(*CHOSEN.lock().unwrap(), vec![depot.clone()], "le module reçoit le chemin résolu, et lui seul");
    let journal = h.state().core.mobile.store.read_journal(50);
    let of = |kind: &str| journal.iter().filter(|l| l.kind == kind).map(|l| l.text.as_str()).collect::<Vec<_>>();
    assert_eq!(of("dossiers"), ["Factice › Choisir un dépôt : sélecteur de dossiers"], "pas une ligne par lecture");
    assert!(of("action").contains(&format!("Factice › Choisir un dépôt : {depot}").as_str()), "{journal:?}");
    assert_eq!(of("refus").len(), 2, "{journal:?}");
}

#[test]
fn without_the_folders_right_the_phone_cannot_browse() {
    let mut h = suite();
    let l = with_phone(
        &mut h,
        |s| {
            let id = s.core.mobile.store.devices[0].id.clone();
            let rights = suite_mobile::msg::Rights { files: false, ..Default::default() };
            s.core.mobile.set_rights(&id, rights);
        },
        |c| {
            follow_home(c);
            list_folder(c, 1, "choisir", None)
        },
    );
    assert!(l.error.contains("parcourir les dossiers") && l.groups.is_empty(), "{l:?}");
    let journal = h.state().core.mobile.store.read_journal(50);
    assert!(journal.iter().any(|l| l.kind == "acces" && l.text.ends_with("dossiers non")), "{journal:?}");
    assert!(!journal.iter().any(|l| l.kind == "dossiers"));
}

#[test]
fn a_notification_leads_the_phone_to_its_screen() {
    let mut h = suite();
    let host = h.state().core.host.clone();
    host.notify(Notification::new("factice", "Une chose attend", "à voir", Tone::Warn).path("detail"));
    host.notify(Notification::new("factice", "Pour information", "rien à ouvrir", Tone::Info));
    h.run_steps(2);
    let notes = with_phone(
        &mut h,
        |_| {},
        |c| {
            until(c, |m| match m {
                ToPhone::Notes { notes, .. } if notes.len() >= 2 => Some(notes),
                _ => None,
            })
        },
    );
    let path = |title: &str| notes.iter().find(|n| n.title == title).map(|n| n.path.clone());
    assert_eq!(path("Une chose attend"), Some(Some("detail".into())), "{notes:?}");
    assert_eq!(path("Pour information"), Some(None), "nulle part : le téléphone la déplie");
}

/// Se construit seulement quand on le lui permet ; sans écran mobile : le pont montre son écran générique.
static FRAGILE_BUILDS: AtomicBool = AtomicBool::new(false);

struct Fragile {
    info: ModuleInfo,
}

fn fragile_info() -> ModuleInfo {
    ModuleInfo { id: "fragile", name: "Fragile", icon: "f", summary: "essai de l'écran générique", ..info() }
}

fn build_fragile(_: &suite_sdk::BuildCx) -> Result<Box<dyn Module>, Unavailable> {
    if !FRAGILE_BUILDS.load(Ordering::Relaxed) {
        return Err(Unavailable::Failed("pas encore prêt".into()));
    }
    Ok(Box::new(Fragile { info: fragile_info() }))
}

impl Module for Fragile {
    fn info(&self) -> &ModuleInfo {
        &self.info
    }

    fn logic(&mut self, _ctx: &egui::Context, _cx: &mut Cx) {}

    fn ui(&mut self, _ui: &mut egui::Ui, _view: &str, _cx: &mut Cx) {}

    fn commands(&self) -> Vec<CommandInfo> {
        vec![CommandInfo { id: "bonjour".into(), title: "Dire bonjour".into(), shortcut: None }]
    }
}

/// Suit l'accueil du module fragile et attend un écran qui propose `action`.
fn fragile_screen_with(c: &mut client::Connection, action: &str) -> Screen {
    c.tx.send(&ToPc::Follow { screen: Some(("fragile".into(), "".into())), terminal: None }).unwrap();
    until(c, |m| match m {
        ToPhone::Screen { module, screen, .. } if module == "fragile" && screen.action_ids().contains(action) => {
            Some(screen)
        }
        _ => None,
    })
}

fn fragile_act(c: &mut client::Connection, req: u32, action: &str) -> (bool, String) {
    let module = "fragile".into();
    c.tx.send(&ToPc::Action { req, module, path: "".into(), action: action.into(), input: None }).unwrap();
    until(c, |m| match m {
        ToPhone::Result { req: r, ok, message } if r == req => Some((ok, message)),
        _ => None,
    })
}

#[test]
fn the_generic_screen_restarts_a_stopped_module_and_says_a_command_left() {
    let mut h = suite();
    {
        let ctx = h.ctx.clone();
        let s = h.state_mut();
        let mut slot = Slot::new(fragile_info(), build_fragile);
        let cc = eframe::CreationContext::_new_kittest(ctx);
        slot.start(&cc, &s.core.host, suite_sdk::DataMode::Shared, &mut s.core.journal);
        slot.state = SlotState::Crashed("boum".into());
        s.core.slots.push(slot);
    }
    let labels = with_phone(
        &mut h,
        |_| {},
        |c| {
            let label = |s: &Screen| s.all_acts().iter().find(|a| a.id == "@relancer").map(|a| a.label.clone());
            // Arrêté : « Relancer le module », par le chemin de l'hôte ; il ne se construit pas encore.
            let crashed = label(&fragile_screen_with(c, "@relancer"));
            let (ok, message) = fragile_act(c, 1, "@relancer");
            assert!(!ok && message.contains("toujours pas disponible : pas encore prêt"), "{message}");
            // Indisponible : « Réessayer », qui le relance.
            let unavailable = label(&fragile_screen_with(c, "@relancer"));
            FRAGILE_BUILDS.store(true, Ordering::Relaxed);
            let (ok, message) = fragile_act(c, 2, "@relancer");
            assert_eq!((ok, message.as_str()), (true, "Fragile relancé"));
            // Il tourne : ses commandes, plus de « Relancer » ; une commande lancée dit qu'elle est partie.
            let running = fragile_screen_with(c, "cmd:bonjour");
            assert!(!running.action_ids().contains("@relancer"), "{running:?}");
            let (ok, message) = fragile_act(c, 3, "cmd:bonjour");
            assert_eq!((ok, message.as_str()), (true, "« Dire bonjour » lancée sur le PC"));
            // Ce qu'il ne propose plus est refusé.
            let (ok, _) = fragile_act(c, 4, "@relancer");
            assert!(!ok);
            (crashed, unavailable)
        },
    );
    assert_eq!(labels, (Some("Relancer le module".into()), Some("Réessayer".into())));
    let s = h.state();
    assert!(s.core.slot("fragile").is_some_and(|s| s.is_running()));
    assert!(s.core.host.is_running("fragile"), "l'hôte sait qu'il tourne (raccourcis, modules actifs)");
    let journal = s.core.mobile.store.read_journal(50);
    let actions: Vec<&str> = journal.iter().filter(|l| l.kind == "action").map(|l| l.text.as_str()).collect();
    assert!(actions.iter().any(|t| t.starts_with("Fragile › Relancer le module : échec")), "{actions:?}");
    assert!(actions.contains(&"Fragile › Réessayer"), "{actions:?}");
}

/// La clé du veilleur arrive pendant qu'un téléphone est connecté : il reçoit un nouveau `Welcome` avec son secret et
/// son identifiant ; Eyrie qui se ferme lui dit `Bye { closing: true }` (« Eyrie est fermée », pas « hors ligne »).
#[test]
fn a_connected_phone_gets_its_wake_key_and_hears_eyrie_close() {
    let mut h = suite();
    h.state_mut().core.mobile.new_ticket(None).unwrap();
    let uri = h.state().core.mobile.ticket.as_ref().unwrap().uri.clone();
    let ticket = Ticket::from_uri(&uri).unwrap();
    let me = Keypair::generate();
    let public = me.public;
    let phone = std::thread::spawn(move || {
        let mut c = client::open(&ticket.addresses(), &me, Duration::from_secs(5)).unwrap();
        client::pair(&mut c, &ticket.secret, Some(&ticket.key), "Pixel", "Google Pixel 8").unwrap();
        client::wait_paired(&mut c).unwrap();
        let first = client::hello(&mut c, &ticket.key, "Pixel", "Google Pixel 8").unwrap();
        let wake = until(&mut c, |m| match m {
            ToPhone::Welcome { wake, .. } => Some(wake),
            _ => None,
        });
        let bye = until(&mut c, |m| match m {
            ToPhone::Bye { reason, closing } => Some((reason, closing)),
            _ => None,
        });
        (first.wake, wake, bye)
    });
    spin(&mut h, |h| !h.state().core.mobile.requests.is_empty());
    h.state_mut().core.mobile.decide(0, true);
    spin(&mut h, |h| !h.state().core.mobile.sessions.is_empty());
    // Sans image entre les deux : la logique de l'hôte rendrait au pont la clé (absente) du vrai réglage.
    h.state_mut().core.mobile.set_wake(Some([5; 32]));
    h.state_mut().core.mobile.exit();
    spin(&mut h, |_| phone.is_finished());
    let (first, wake, (reason, closing)) = phone.join().expect("le téléphone a tout vu");
    assert_eq!(first, None, "pas de veilleur à la connexion");
    let wake = wake.expect("le secret de démarrage");
    let id = h.state().core.mobile.store.devices[0].id.clone();
    assert_eq!(wake.device, id);
    assert_eq!(wake.port, suite_mobile::veille::PORT);
    assert_eq!(wake.secret, suite_mobile::veille::device_secret(&[5; 32], &public).to_vec());
    assert!(closing, "{reason}");
}
