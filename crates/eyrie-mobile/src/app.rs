//! Le contrôleur : il tient l'état de l'application (PC, modules, écrans reçus, navigation, terminal suivi), reçoit
//! les messages du PC et les gestes de l'utilisateur, et met l'interface à jour. Il vit dans le fil de l'interface ;
//! les fils réseau lui écrivent par `slint::invoke_from_event_loop`.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};
use suite_mobile::msg::{CommandCard, ModuleCard, Note, Rights, ToPc, ToPhone, Waiting};
use suite_mobile::pairing::{self, Ticket};
use suite_mobile::term::{Key, TermInput};
use suite_mobile::ui::{Screen, Tone};

use crate::net::{Link, NetEvent, PairEvent};
use crate::store::{self, Pc, Store};
use crate::tailnet::{self, Tailnet};
use crate::termbuf::{self, TermBuf};
use crate::{
    AppWindow, CommandItem, ModuleItem, NavItem, NoteItem, PcItem, TermRow, TermSeg, Theme, UiAct, WaitItem, platform,
    view,
};

/// Les modules proposés dans la barre du bas tant que l'utilisateur n'a rien choisi.
const DEFAULT_PINS: [&str; 8] =
    ["fawkes", "patronus", "owlery", "menagerie", "gringotts", "dragontree", "grimoire", "carnet"];
const EXTRAS: [&str; 3] = ["notes", "commandes", "reglages"];
const TEXT_SIZES: [f32; 3] = [1.0, 1.35, 1.7];

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// L'initiale d'un module (son avatar quand il n'a pas de logo) : toujours dessinable, contrairement à certains
/// glyphes de la barre d'activité du PC, absents des polices du téléphone.
fn initial(name: &str) -> String {
    name.chars().find(|c| c.is_alphanumeric()).map(|c| c.to_uppercase().collect()).unwrap_or_else(|| "•".into())
}

/// Ce que le PC suit pour ce téléphone : l'écran `(module, chemin)` et le terminal `(module, id)`.
type Followed = (Option<(String, String)>, Option<(String, String)>);

/// Ce qui a déjà été signalé à l'utilisateur.
#[derive(Default)]
struct Signalled {
    /// Notifications du PC vues jusqu'à cet instant (secondes Unix) ; `None` : rien reçu depuis la connexion.
    notes_until: Option<u64>,
    /// Ce qui attendait au dernier relevé ; `None` : rien reçu depuis la connexion.
    waiting: Option<std::collections::HashSet<String>>,
}

/// Où l'appairage en cours se connecte.
#[derive(Clone)]
struct Target {
    addrs: Vec<String>,
    host: String,
    ip: String,
    port: u16,
    secret: Vec<u8>,
    key: Option<[u8; 32]>,
    label: String,
}

pub struct Controller {
    ui: slint::Weak<AppWindow>,
    store: Store,
    link: Option<Link>,
    online: bool,
    rights: Rights,
    pc_name: String,
    eyrie: String,
    cards: Vec<ModuleCard>,
    logos: HashMap<String, slint::Image>,
    waiting: Vec<Waiting>,
    notes: Vec<Note>,
    unread: u32,
    commands: Vec<CommandCard>,
    screens: HashMap<(String, String), Screen>,
    missing: HashMap<(String, String), String>,
    tab: String,
    stacks: HashMap<String, Vec<String>>,
    followed: Option<Followed>,
    term: TermBuf,
    term_model: Rc<VecModel<TermRow>>,
    term_dark: Option<bool>,
    req: u32,
    target: Option<Target>,
    /// Ce qui a déjà été signalé (bulle ou notification du système) : rien deux fois, et rien à la première connexion
    /// à un PC (l'état initial n'est pas une nouveauté). Après une coupure, ou un arrêt de l'application par Android,
    /// ce qui est arrivé sur le PC entre-temps l'est.
    signalled: Signalled,
    /// Démonstration (captures) : rien n'est envoyé.
    demo: bool,
    goto: Option<String>,
    /// Le nœud Tailscale de l'application, au dernier relevé ; relevé chaque seconde (sauf sur le bureau, sans nœud).
    tailnet: Tailnet,
    tailnet_timer: slint::Timer,
}

thread_local! {
    static CTRL: RefCell<Option<Controller>> = const { RefCell::new(None) };
}

/// Appelle le contrôleur (dans le fil de l'interface).
pub fn with<R>(f: impl FnOnce(&mut Controller) -> R) -> Option<R> {
    CTRL.with(|c| c.try_borrow_mut().ok().and_then(|mut c| c.as_mut().map(f)))
}

/// Depuis un autre fil.
fn post(f: impl FnOnce(&mut Controller) + Send + 'static) {
    let _ = slint::invoke_from_event_loop(move || {
        with(f);
    });
}

impl Controller {
    pub fn install(ui: &AppWindow, store: Store, demo: bool) {
        let term_model = Rc::new(VecModel::<TermRow>::default());
        ui.set_term_rows(ModelRc::from(term_model.clone()));
        ui.set_app_version(env!("CARGO_PKG_VERSION").into());
        ui.set_can_scan(platform::can_scan());
        ui.set_logo(crate::logo());
        ui.set_fingerprint(pairing::fingerprint(&store.identity.public).into());
        ui.global::<Theme>().set_choice(store.prefs.theme);
        ui.set_text_size(store.prefs.text_size);
        ui.set_term_zoom(TEXT_SIZES[store.prefs.text_size.clamp(0, 2) as usize]);
        let name =
            if store.prefs.device_name.is_empty() { platform::device_name() } else { store.prefs.device_name.clone() };
        ui.set_device_name(name.into());
        let tn = tailnet::state();
        let (state, text) = tn.display();
        ui.set_tailnet_state(state);
        ui.set_tailnet_text(text.into());
        let c = Controller {
            ui: ui.as_weak(),
            store,
            link: None,
            online: false,
            rights: Rights::default(),
            pc_name: String::new(),
            eyrie: String::new(),
            cards: Vec::new(),
            logos: HashMap::new(),
            waiting: Vec::new(),
            notes: Vec::new(),
            unread: 0,
            commands: Vec::new(),
            screens: HashMap::new(),
            missing: HashMap::new(),
            tab: "ensemble".into(),
            stacks: HashMap::new(),
            followed: None,
            term: TermBuf::default(),
            term_model,
            term_dark: None,
            req: 0,
            target: None,
            signalled: Signalled::default(),
            demo,
            goto: None,
            tailnet: tn,
            tailnet_timer: slint::Timer::default(),
        };
        CTRL.with(|cell| *cell.borrow_mut() = Some(c));
        wire(ui);
        with(|c| {
            if c.tailnet != Tailnet::Absent {
                c.tailnet_timer.start(slint::TimerMode::Repeated, Duration::from_secs(1), || {
                    with(|c| c.refresh_tailnet());
                });
            }
            c.start();
        });
    }

    fn ui(&self) -> Option<AppWindow> {
        self.ui.upgrade()
    }

    fn start(&mut self) {
        let Some(ui) = self.ui() else { return };
        ui.set_has_pc(self.store.current().is_some());
        if self.demo {
            ui.set_page(4);
            return;
        }
        match self.store.current().cloned() {
            Some(pc) => {
                self.pc_name = pc.name.clone();
                ui.set_page(4);
                self.connect(&pc);
            }
            None => ui.set_page(0),
        }
        self.refresh_all();
    }

    fn device(&self) -> (String, String) {
        let name = self.ui().map(|u| u.get_device_name().to_string()).unwrap_or_default();
        let name = if name.trim().is_empty() { platform::device_name() } else { name.trim().to_string() };
        (name, platform::device_model())
    }

    fn connect(&mut self, pc: &Pc) {
        self.link = None;
        self.online = false;
        self.signalled = Signalled { notes_until: (pc.notes_seen > 0).then_some(pc.notes_seen), waiting: None };
        let Some(key) = pc.key_bytes() else { return };
        let (device, model) = self.device();
        self.link = Some(Link::start(pc.addresses(), key, self.store.identity.clone(), device, model, |ev| {
            post(move |c| c.on_net(ev));
        }));
        self.set_link(0, &format!("Connexion à {}…", pc.name));
    }

    fn send(&self, m: ToPc) {
        if let Some(l) = &self.link {
            l.send(m);
        }
    }

    fn set_link(&self, state: i32, text: &str) {
        if let Some(ui) = self.ui() {
            ui.set_link_state(state);
            ui.set_link_text(text.into());
            ui.set_pc_name(self.pc_name.as_str().into());
        }
    }

    pub fn toast(&self, text: &str, tone: i32) {
        if let Some(ui) = self.ui() {
            ui.set_toast_visible(false);
            ui.set_toast_text(text.into());
            ui.set_toast_tone(tone);
            ui.set_toast_visible(true);
        }
    }

    // --- Le réseau ---

    /// Le nœud Tailscale de l'application : son état à l'écran ; une fois connecté, la liaison réessaie tout de suite.
    fn refresh_tailnet(&mut self) {
        let now = tailnet::state();
        if now == self.tailnet {
            return;
        }
        let ready = now.ready() && !self.tailnet.ready();
        let (state, text) = now.display();
        if let Some(ui) = self.ui() {
            ui.set_tailnet_state(state);
            ui.set_tailnet_text(text.into());
            // Déconnecté du compte en cours d'usage : le dire, la connexion se refait dans les réglages.
            if matches!(&now, Tailnet::NeedsLogin(u) if !u.is_empty()) && ui.get_page() == 4 {
                self.toast("Ce téléphone n'est plus connecté à Tailscale : Réglages › Tailscale.", 3);
            }
        }
        self.tailnet = now;
        if ready
            && !self.online
            && let Some(l) = &self.link
        {
            l.kick();
        }
    }

    /// Le bouton du nœud : la connexion au compte dans le navigateur, ou un nouvel essai après une erreur.
    fn tailnet_login(&self) {
        match &self.tailnet {
            Tailnet::NeedsLogin(url) if !url.is_empty() => platform::open_url(url),
            Tailnet::Failed(_) => {
                tailnet::start(platform::data_dir().join("tailnet"), tailnet::hostname(&self.device().0))
            }
            _ => {}
        }
    }

    fn tailnet_logout(&self) {
        let _ = std::thread::Builder::new().name("eyrie-tailnet".into()).spawn(|| {
            if let Err(e) = tailnet::logout() {
                post(move |c| c.toast(&format!("Tailscale : {e}"), 4));
            }
        });
    }

    fn on_net(&mut self, ev: NetEvent) {
        match ev {
            NetEvent::Connecting => {
                if !self.online {
                    self.set_link(0, &format!("Connexion à {}…", self.pc_label()));
                }
            }
            NetEvent::Online(w) => {
                self.online = true;
                self.rights = w.rights;
                self.pc_name = w.pc;
                self.eyrie = w.eyrie;
                if let Some(pc) = self.store.prefs.current.clone()
                    && let Some(p) = self.store.pcs.iter_mut().find(|p| p.id == pc)
                    && p.name != self.pc_name
                {
                    p.name = self.pc_name.clone();
                    self.store.save();
                }
                self.set_link(1, "Connecté");
                // Tout est renvoyé par le PC ; l'écran suivi est redemandé. Ce qui a déjà été signalé est gardé : ce
                // qui est arrivé pendant la coupure le sera.
                self.followed = None;
                platform::ask_notifications();
                if let Some(goto) = self.goto.take() {
                    let (tab, path) = goto.split_once('/').unwrap_or((&goto, ""));
                    self.tab = tab.to_string();
                    let mut stack = vec![String::new()];
                    if !path.is_empty() {
                        stack.push(path.to_string());
                    }
                    self.stacks.insert(tab.to_string(), stack);
                }
                self.refresh_all();
            }
            NetEvent::Offline { reason, retry_in } => {
                self.online = false;
                let secs = retry_in.as_secs().max(1);
                self.set_link(2, &format!("Hors ligne ({reason}) · nouvel essai dans {secs} s"));
            }
            NetEvent::Refused(reason) => {
                self.online = false;
                self.set_link(3, &reason);
            }
            NetEvent::Message(m) => self.on_message(*m),
        }
    }

    fn pc_label(&self) -> String {
        if self.pc_name.is_empty() {
            self.store.current().map(|p| p.name.clone()).unwrap_or_default()
        } else {
            self.pc_name.clone()
        }
    }

    pub fn on_message(&mut self, m: ToPhone) {
        match m {
            ToPhone::Modules { modules } => {
                self.cards = modules;
                self.refresh_modules();
                self.refresh_screen();
            }
            ToPhone::Logos { logos } => {
                for (id, png) in logos {
                    if let Some(img) = crate::png_image(&png) {
                        self.logos.insert(id, img);
                    }
                }
                self.refresh_modules();
            }
            ToPhone::Overview { waiting } => {
                self.signal_waiting(&waiting);
                self.waiting = waiting;
                self.refresh_modules();
            }
            ToPhone::Notes { notes, unread } => {
                self.signal_notes(&notes);
                self.notes = notes;
                self.unread = unread;
                self.refresh_modules();
            }
            ToPhone::Commands { commands } => {
                self.commands = commands;
                self.refresh_commands();
            }
            ToPhone::Screen { module, path, screen } => {
                let key = (module, path);
                self.missing.remove(&key);
                self.screens.insert(key, screen);
                self.refresh_screen();
            }
            ToPhone::NoScreen { module, path, reason } => {
                // L'écran ouvert a disparu (terminal fermé, entrée traitée) : retour à l'écran d'avant.
                let stack = self.stacks.get_mut(&module);
                if self.tab == module
                    && let Some(stack) = stack
                    && stack.len() > 1
                    && stack.last() == Some(&path)
                {
                    stack.pop();
                    self.toast(&reason, 1);
                } else {
                    self.missing.insert((module, path), reason);
                }
                self.refresh_screen();
            }
            ToPhone::Term(f) => {
                if self.followed_term().is_some_and(|(m, id)| m == f.module && id == f.id) {
                    let change = self.term.apply(&f);
                    self.refresh_term(change);
                }
            }
            ToPhone::Result { ok, message, .. } => {
                if !ok {
                    self.toast(&message, 4);
                } else if !message.is_empty() {
                    self.toast(&message, 2);
                }
            }
            ToPhone::Open { module, path } => self.open_module_path(&module, &path),
            ToPhone::Toast { text, tone } => self.toast(&text, view::tone(Some(tone))),
            ToPhone::Bye { reason, .. } => self.set_link(2, &reason),
            ToPhone::Welcome { .. }
            | ToPhone::PairPending { .. }
            | ToPhone::Paired { .. }
            | ToPhone::Refused { .. }
            | ToPhone::Folder { .. }
            | ToPhone::Pong => {}
        }
    }

    // --- Ce qui arrive : bulle au premier plan, notification du système en arrière-plan ---

    /// Ce qui se met à attendre (une session Claude…) : une notification si l'application est en arrière-plan ; au
    /// premier plan, la pastille et la vue d'ensemble suffisent.
    fn signal_waiting(&mut self, waiting: &[Waiting]) {
        let now: std::collections::HashSet<String> =
            waiting.iter().map(|w| format!("{}\u{1f}{}", w.module, w.text)).collect();
        if let Some(before) = &self.signalled.waiting
            && platform::in_background()
        {
            for w in waiting {
                if !before.contains(&format!("{}\u{1f}{}", w.module, w.text)) {
                    let module = self.card(&w.module).map(|c| c.name.clone()).unwrap_or_else(|| w.module.clone());
                    platform::notify(&format!("attente:{}:{}", w.module, w.text), &module, &w.text);
                }
            }
        }
        self.signalled.waiting = Some(now);
    }

    /// Une notification du PC arrive : une bulle au premier plan ; une notification du système en arrière-plan pour
    /// ce qui demande de l'attention (attention, échec), hors Fawkes (ses sessions qui attendent sont déjà signalées).
    fn signal_notes(&mut self, notes: &[Note]) {
        let latest = notes.iter().map(|n| n.at).max().unwrap_or(0);
        if let Some(until) = self.signalled.notes_until {
            let fresh: Vec<&Note> = notes.iter().filter(|n| n.at > until && !n.read).collect();
            if platform::in_background() {
                for n in fresh.iter().filter(|n| {
                    matches!(n.tone, suite_mobile::ui::Tone::Warn | suite_mobile::ui::Tone::Danger)
                        && n.module != "fawkes"
                }) {
                    platform::notify(&format!("note:{}:{}", n.at, n.title), &n.title, &n.body);
                }
            } else if let Some((text, tone)) = notes_toast(&fresh) {
                self.toast(&text, view::tone(Some(tone)));
            }
        }
        // 1 au moins : connecté une fois, même sans notification.
        let until = self.signalled.notes_until.unwrap_or(0).max(latest).max(1);
        self.signalled.notes_until = Some(until);
        // Gardé avec le PC : après un arrêt de l'application, la reconnexion signale ce qui est arrivé entre-temps.
        if let Some(id) = self.store.prefs.current.clone()
            && let Some(p) = self.store.pcs.iter_mut().find(|p| p.id == id)
            && p.notes_seen < until
        {
            p.notes_seen = until;
            self.store.save();
        }
    }

    // --- Ce que l'on montre ---

    fn card(&self, id: &str) -> Option<&ModuleCard> {
        self.cards.iter().find(|c| c.id == id)
    }

    fn pins(&self) -> Vec<String> {
        let known = |id: &str| self.cards.iter().any(|c| c.id == id);
        let chosen: Vec<String> = self.store.prefs.pinned.iter().filter(|p| known(p)).cloned().collect();
        if !chosen.is_empty() {
            return chosen;
        }
        let running = |id: &str| self.cards.iter().any(|c| c.id == id && c.running);
        DEFAULT_PINS.iter().filter(|p| running(p)).take(3).map(|p| p.to_string()).collect()
    }

    fn module_item(&self, c: &ModuleCard, pins: &[String]) -> ModuleItem {
        let logo = self.logos.get(&c.id).cloned();
        let badge = c.badge.as_ref().map(|b| match b.count {
            Some(n) => n.to_string(),
            None => "•".into(),
        });
        ModuleItem {
            id: c.id.as_str().into(),
            name: c.name.as_str().into(),
            glyph: initial(&c.name).into(),
            color: view::rgb(c.color),
            has_logo: logo.is_some(),
            logo: logo.unwrap_or_default(),
            badge: badge.unwrap_or_default().into(),
            badge_tone: c.badge.as_ref().map(|b| view::tone(Some(b.tone))).unwrap_or(3),
            status: if c.running { c.status.as_str().into() } else { c.reason.as_str().into() },
            running: c.running,
            pinned: pins.contains(&c.id),
            summary: c.summary.as_str().into(),
        }
    }

    fn refresh_modules(&self) {
        let Some(ui) = self.ui() else { return };
        let pins = self.pins();
        let items: Vec<ModuleItem> = self.cards.iter().map(|c| self.module_item(c, &pins)).collect();
        let waiting_count = self.waiting.len();
        let mut nav = vec![NavItem {
            key: "ensemble".into(),
            label: "Ensemble".into(),
            icon: "home".into(),
            badge: if waiting_count > 0 { waiting_count.to_string().into() } else { SharedString::new() },
            badge_tone: 3,
            ..Default::default()
        }];
        for id in &pins {
            if let Some(c) = self.card(id) {
                let m = self.module_item(c, &pins);
                nav.push(NavItem {
                    key: m.id.clone(),
                    label: m.name.clone(),
                    badge: m.badge.clone(),
                    badge_tone: m.badge_tone,
                    module: m,
                    is_module: true,
                    ..Default::default()
                });
            }
        }
        nav.push(NavItem {
            key: "plus".into(),
            label: "Plus".into(),
            icon: "apps".into(),
            badge: if self.unread > 0 { self.unread.to_string().into() } else { SharedString::new() },
            badge_tone: 1,
            ..Default::default()
        });
        let waiting: Vec<WaitItem> = self
            .waiting
            .iter()
            .map(|w| {
                let c = self.card(&w.module);
                WaitItem {
                    module: w.module.as_str().into(),
                    module_name: c.map(|c| c.name.as_str()).unwrap_or(&w.module).into(),
                    glyph: initial(c.map(|c| c.name.as_str()).unwrap_or(&w.module)).into(),
                    color: view::rgb(c.map(|c| c.color).unwrap_or(0x5b8def)),
                    text: w.text.as_str().into(),
                    path: w.path.clone().unwrap_or_default().into(),
                }
            })
            .collect();
        let t = now();
        let notes: Vec<NoteItem> = self
            .notes
            .iter()
            .map(|n| NoteItem {
                module_name: self.card(&n.module).map(|c| c.name.as_str()).unwrap_or(&n.module).into(),
                title: n.title.as_str().into(),
                body: n.body.as_str().into(),
                tone: view::tone(Some(n.tone)),
                when: view::ago(n.at, t).into(),
                read: n.read,
            })
            .collect();
        ui.set_modules(ModelRc::new(VecModel::from(items)));
        ui.set_nav(ModelRc::new(VecModel::from(nav)));
        ui.set_waiting(ModelRc::new(VecModel::from(waiting)));
        ui.set_recent_notes(ModelRc::new(VecModel::from(notes.iter().take(3).cloned().collect::<Vec<_>>())));
        ui.set_notes(ModelRc::new(VecModel::from(notes)));
        ui.set_unread(self.unread as i32);
    }

    fn refresh_commands(&self) {
        let Some(ui) = self.ui() else { return };
        let filter = ui.get_command_filter().to_lowercase();
        let items: Vec<CommandItem> = self
            .commands
            .iter()
            .filter_map(|c| {
                let module_name = self.card(&c.module).map(|m| m.name.clone()).unwrap_or_else(|| c.module.clone());
                let hit = filter.is_empty()
                    || c.title.to_lowercase().contains(&filter)
                    || module_name.to_lowercase().contains(&filter);
                hit.then(|| CommandItem {
                    module: c.module.as_str().into(),
                    module_name: module_name.into(),
                    id: c.id.as_str().into(),
                    title: c.title.as_str().into(),
                })
            })
            .collect();
        ui.set_commands(ModelRc::new(VecModel::from(items)));
    }

    fn is_module_tab(&self) -> bool {
        self.tab != "ensemble" && self.tab != "plus" && !EXTRAS.contains(&self.tab.as_str())
    }

    fn top_path(&self) -> String {
        self.stacks.get(&self.tab).and_then(|s| s.last().cloned()).unwrap_or_default()
    }

    fn followed_term(&self) -> Option<(String, String)> {
        self.followed.as_ref().and_then(|(_, t)| t.clone())
    }

    fn refresh_all(&mut self) {
        self.refresh_modules();
        self.refresh_commands();
        self.refresh_settings();
        self.refresh_screen();
    }

    fn refresh_settings(&self) {
        let Some(ui) = self.ui() else { return };
        let current = self.store.prefs.current.clone().unwrap_or_default();
        let pcs: Vec<PcItem> = self
            .store
            .pcs
            .iter()
            .map(|p| PcItem {
                id: p.id.as_str().into(),
                name: p.name.as_str().into(),
                host: p.host.as_str().into(),
                current: p.id == current,
            })
            .collect();
        ui.set_pcs(ModelRc::new(VecModel::from(pcs)));
        ui.set_pc_host(self.store.current().map(|p| p.host.clone()).unwrap_or_default().into());
        ui.set_eyrie_version(self.eyrie.as_str().into());
        let rights = match (self.rights.act, self.rights.terminals) {
            (true, true) => "Agir, terminaux",
            (true, false) => "Agir",
            (false, true) => "Terminaux",
            (false, false) => "Lecture seule",
        };
        ui.set_rights_text(rights.into());
        ui.set_term_can_type(self.rights.terminals);
        ui.set_has_pc(self.store.current().is_some());
    }

    /// Le titre, les actions et le contenu de l'onglet courant ; ce que le PC doit suivre.
    fn refresh_screen(&mut self) {
        let Some(ui) = self.ui() else { return };
        let tab = self.tab.clone();
        ui.set_tab(tab.as_str().into());
        let mut screen_follow = None;
        let mut term_follow = None;
        let (title, subtitle, can_back, acts) = match tab.as_str() {
            "ensemble" => ("Ensemble".to_string(), String::new(), false, Vec::new()),
            "plus" => ("Plus".into(), String::new(), false, Vec::new()),
            "notes" => (
                "Notifications".into(),
                String::new(),
                true,
                vec![suite_mobile::ui::Act::new("@lu", "Tout marquer lu")],
            ),
            "commandes" => ("Commandes du PC".into(), String::new(), true, Vec::new()),
            "reglages" => ("Réglages".into(), String::new(), true, Vec::new()),
            module => {
                let path = self.top_path();
                let key = (module.to_string(), path.clone());
                let stack_len = self.stacks.get(module).map_or(1, Vec::len);
                screen_follow = Some(key.clone());
                let name = self.card(module).map(|c| c.name.clone()).unwrap_or_else(|| module.to_string());
                match self.screens.get(&key) {
                    Some(s) => {
                        let rows = view::rows(s);
                        ui.set_rows(ModelRc::new(VecModel::from(rows)));
                        if let Some(id) = &s.terminal {
                            term_follow = Some((module.to_string(), id.clone()));
                        }
                        (s.title.clone(), s.subtitle.clone(), stack_len > 1, s.acts.clone())
                    }
                    None => {
                        ui.set_rows(ModelRc::new(VecModel::<crate::UiRow>::default()));
                        (name, String::new(), stack_len > 1, Vec::new())
                    }
                }
            }
        };
        let module_tab = self.is_module_tab();
        let key = (tab.clone(), self.top_path());
        ui.set_loading(module_tab && !self.screens.contains_key(&key));
        ui.set_missing(self.missing.get(&key).cloned().unwrap_or_default().into());
        ui.set_screen_title(title.into());
        ui.set_subtitle(subtitle.into());
        ui.set_can_back(can_back);
        ui.set_show_pc(acts.iter().any(|a| a.id == "@pc"));
        let acts: Vec<_> = acts.into_iter().filter(|a| a.id != "@pc").collect();
        ui.set_screen_acts(view::acts(&acts));
        let terminal_mode = term_follow.is_some();
        if terminal_mode && !self.term.is(&term_follow.as_ref().unwrap().0, &term_follow.as_ref().unwrap().1) {
            let (m, id) = term_follow.clone().unwrap();
            self.term.reset(&m, &id);
            self.term_model.set_vec(Vec::new());
            ui.set_term_draft(SharedString::new());
        }
        ui.set_terminal_mode(terminal_mode);
        let follow = (screen_follow, term_follow);
        if self.followed.as_ref() != Some(&follow) {
            self.followed = Some(follow.clone());
            if !self.demo {
                self.send(ToPc::Follow { screen: follow.0, terminal: follow.1 });
            }
        }
    }

    fn seg(s: &termbuf::Seg) -> TermSeg {
        let c = |v: u32| slint::Color::from_argb_u8((v >> 24) as u8, (v >> 16) as u8, (v >> 8) as u8, v as u8);
        TermSeg {
            col: s.col as i32,
            len: s.len as i32,
            text: s.text.as_str().into(),
            fg: c(s.fg),
            bg: s.bg.map(c).unwrap_or_default(),
            has_bg: s.bg.is_some(),
            bold: s.bold,
            italic: s.italic,
            underline: s.underline,
        }
    }

    fn term_row(line: &suite_mobile::term::Line, p: &termbuf::TermPalette) -> TermRow {
        let segs: Vec<TermSeg> = termbuf::segments(line, p).iter().map(Self::seg).collect();
        TermRow { segs: ModelRc::new(VecModel::from(segs)) }
    }

    fn refresh_term(&mut self, change: termbuf::Change) {
        let Some(ui) = self.ui() else { return };
        let dark = ui.global::<Theme>().get_dark();
        let p = if dark { &termbuf::DARK } else { &termbuf::LIGHT };
        let at_bottom = ui.get_term_at_bottom() || change.full;
        let target = self.term.visible_len();
        let model = &self.term_model;
        if change.full || self.term_dark != Some(dark) {
            self.term_dark = Some(dark);
            model.set_vec(self.term.lines[..target].iter().map(|l| Self::term_row(l, p)).collect::<Vec<_>>());
        } else {
            for _ in 0..change.shifted.min(model.row_count()) {
                model.remove(0);
            }
            let kept = model.row_count();
            while model.row_count() > target {
                model.remove(model.row_count() - 1);
            }
            for i in kept..target {
                model.push(Self::term_row(&self.term.lines[i], p));
            }
            for i in change.changed {
                if i < kept.min(target) {
                    model.set_row_data(i, Self::term_row(&self.term.lines[i], p));
                }
            }
        }
        ui.set_term_cols(self.term.cols.max(20) as i32);
        match self.term.cursor {
            Some((row, col)) => {
                ui.set_term_cursor_row(row as i32);
                ui.set_term_cursor_col(col as i32);
            }
            None => ui.set_term_cursor_row(-1),
        }
        ui.set_term_ended(self.term.ended);
        ui.set_term_waiting(self.term.waiting);
        let replies: Vec<SharedString> = self.term.replies.iter().map(|r| r.label.as_str().into()).collect();
        ui.set_term_replies(ModelRc::new(VecModel::from(replies)));
        if at_bottom {
            // Après la mise en page des nouvelles lignes (la hauteur de la liste n'est connue qu'au dessin suivant).
            for delay in [0, 60, 200] {
                let weak = self.ui.clone();
                slint::Timer::single_shot(Duration::from_millis(delay), move || {
                    if let Some(ui) = weak.upgrade() {
                        ui.invoke_term_scroll_to_end();
                    }
                });
            }
        }
    }

    // --- Les gestes ---

    pub(crate) fn nav_select(&mut self, key: &str) {
        if key == self.tab && self.is_module_tab() {
            // Toucher l'onglet du module courant ramène à son accueil.
            self.stacks.insert(key.to_string(), vec![String::new()]);
        }
        self.tab = key.to_string();
        if self.is_module_tab() {
            self.stacks.entry(key.to_string()).or_insert_with(|| vec![String::new()]);
        }
        self.refresh_screen();
    }

    /// Ouvre un écran d'un module (vide : son accueil, la pile de cet onglet repart de là).
    pub(crate) fn open_module_path(&mut self, module: &str, path: &str) {
        self.tab = module.to_string();
        let stack = self.stacks.entry(module.to_string()).or_insert_with(|| vec![String::new()]);
        if path.is_empty() {
            *stack = vec![String::new()];
        } else if stack.last().map(String::as_str) != Some(path) {
            stack.push(path.to_string());
        }
        self.refresh_all();
    }

    pub(crate) fn open_path(&mut self, path: &str) {
        if !self.is_module_tab() {
            return;
        }
        let stack = self.stacks.entry(self.tab.clone()).or_insert_with(|| vec![String::new()]);
        stack.push(path.to_string());
        self.refresh_screen();
    }

    /// Retour (barre du haut ou bouton du système) ; `false` : rien à quitter, l'application se ferme.
    fn back(&mut self) -> bool {
        let Some(ui) = self.ui() else { return false };
        match ui.get_page() {
            1 | 3 => {
                self.cancel_pairing();
                return true;
            }
            2 => return true,
            0 => {
                if self.store.current().is_some() {
                    ui.set_page(4);
                    return true;
                }
                return false;
            }
            _ => {}
        }
        if self.is_module_tab() {
            let stack = self.stacks.entry(self.tab.clone()).or_insert_with(|| vec![String::new()]);
            if stack.len() > 1 {
                stack.pop();
                self.refresh_screen();
                return true;
            }
            self.tab = "ensemble".into();
            self.refresh_screen();
            return true;
        }
        match self.tab.as_str() {
            t if EXTRAS.contains(&t) => self.tab = "plus".into(),
            "plus" => self.tab = "ensemble".into(),
            _ => return false,
        }
        self.refresh_screen();
        true
    }

    fn run_act(&mut self, a: &UiAct, input: &str) {
        if a.id == "@lu" {
            self.send(ToPc::NotesRead);
            return;
        }
        if !self.is_module_tab() {
            return;
        }
        if !self.online {
            self.toast("Hors ligne : l'action n'est pas partie.", 3);
            return;
        }
        self.req += 1;
        let input = (a.has_input).then(|| input.to_string());
        self.send(ToPc::Action {
            req: self.req,
            module: self.tab.clone(),
            path: self.top_path(),
            action: a.id.to_string(),
            input,
        });
    }

    fn run_command(&mut self, c: &CommandItem) {
        if !self.online {
            self.toast("Hors ligne : la commande n'est pas partie.", 3);
            return;
        }
        self.req += 1;
        self.send(ToPc::Command { req: self.req, module: c.module.to_string(), id: c.id.to_string() });
        self.toast(&format!("« {} » envoyée au PC", c.title), 2);
    }

    fn term_input(&self, input: TermInput) {
        let Some((module, id)) = self.followed_term() else { return };
        if !self.online {
            self.toast("Hors ligne : la saisie n'est pas partie.", 3);
            return;
        }
        self.send(ToPc::Terminal { module, id, input });
    }

    fn term_key(&self, name: &str) {
        let key = match name {
            "escape" => Key::Escape,
            "tab" => Key::Tab,
            "backtab" => Key::BackTab,
            "up" => Key::Up,
            "down" => Key::Down,
            "left" => Key::Left,
            "right" => Key::Right,
            "enter" => Key::Enter,
            "shift-enter" => Key::ShiftEnter,
            "backspace" => Key::Backspace,
            "home" => Key::Home,
            "end" => Key::End,
            "pageup" => Key::PageUp,
            "pagedown" => Key::PageDown,
            k if k.starts_with("ctrl-") => match k.chars().last() {
                Some(c) => Key::Ctrl(c),
                None => return,
            },
            _ => return,
        };
        self.term_input(TermInput::Key(key));
    }

    fn term_send_line(&self, text: &str) {
        if !text.is_empty() {
            self.term_input(TermInput::Text(text.to_string()));
        }
        self.term_input(TermInput::Key(Key::Enter));
    }

    fn term_reply(&self, i: usize) {
        if let Some(r) = self.term.replies.get(i) {
            self.term_input(r.input.clone());
        }
    }

    fn toggle_pin(&mut self, id: &str) {
        let mut pins = self.pins();
        if let Some(i) = pins.iter().position(|p| p == id) {
            pins.remove(i);
        } else {
            if pins.len() == 3 {
                pins.pop();
                self.toast("Trois modules au plus dans la barre du bas.", 1);
            }
            pins.push(id.to_string());
        }
        self.store.prefs.pinned = pins;
        self.store.save();
        self.refresh_modules();
    }

    // --- L'appairage ---

    fn begin_pairing(&mut self, t: Target) {
        let Some(ui) = self.ui() else { return };
        platform::stop_scan();
        ui.set_page(2);
        ui.set_pairing_stage(0);
        ui.set_pairing_pc(t.label.as_str().into());
        ui.set_pairing_error(SharedString::new());
        let (device, model) = self.device();
        self.target = Some(t.clone());
        crate::net::pair(t.addrs, t.secret, t.key, self.store.identity.clone(), device, model, |ev| {
            post(move |c| c.on_pair(ev));
        });
    }

    fn on_pair(&mut self, ev: PairEvent) {
        let Some(ui) = self.ui() else { return };
        if ui.get_page() != 2 {
            return;
        }
        match ev {
            PairEvent::Sas(sas) => {
                ui.set_pairing_sas(sas.into());
                ui.set_pairing_stage(1);
                platform::haptic();
            }
            PairEvent::Paired { pc, key } => {
                let Some(t) = self.target.clone() else { return };
                self.store.add(Pc {
                    id: store::hex(&key[..8]),
                    name: pc.clone(),
                    host: t.host,
                    ip: t.ip,
                    port: t.port,
                    key: store::hex(&key),
                    paired_at: now(),
                    notes_seen: 0,
                });
                self.pc_name = pc;
                ui.set_pairing_stage(2);
                platform::haptic();
                let weak = self.ui.clone();
                slint::Timer::single_shot(Duration::from_millis(1100), move || {
                    if weak.upgrade().is_some_and(|u| u.get_page() == 2) {
                        with(|c| c.pairing_done());
                    }
                });
            }
            PairEvent::Failed(e) => {
                ui.set_pairing_error(e.into());
                ui.set_pairing_stage(3);
            }
        }
    }

    fn pairing_done(&mut self) {
        let Some(ui) = self.ui() else { return };
        ui.set_code_code(SharedString::new());
        ui.set_code_link(SharedString::new());
        ui.set_page(4);
        self.reset_session();
        if let Some(pc) = self.store.current().cloned() {
            self.connect(&pc);
        }
        self.refresh_all();
    }

    fn reset_session(&mut self) {
        self.cards.clear();
        self.logos.clear();
        self.waiting.clear();
        self.notes.clear();
        self.unread = 0;
        self.commands.clear();
        self.screens.clear();
        self.missing.clear();
        self.stacks.clear();
        self.tab = "ensemble".into();
        self.followed = None;
        self.term = TermBuf::default();
        self.term_model.set_vec(Vec::new());
    }

    fn cancel_pairing(&mut self) {
        platform::stop_scan();
        self.target = None;
        if let Some(ui) = self.ui() {
            ui.set_code_error(SharedString::new());
            ui.set_page(if self.store.current().is_some() && ui.get_page() != 1 && ui.get_page() != 3 { 4 } else { 0 });
        }
    }

    pub(crate) fn submit_code(&mut self) {
        let Some(ui) = self.ui() else { return };
        let link = ui.get_code_link().trim().to_string();
        let target = if !link.is_empty() {
            match Ticket::from_uri(&link) {
                Ok(t) => target_of(&t),
                Err(e) => {
                    ui.set_code_error(e.into());
                    return;
                }
            }
        } else {
            let host = ui.get_code_host().trim().trim_end_matches('.').to_lowercase();
            let Some(secret) = pairing::code_secret(&ui.get_code_code()) else {
                ui.set_code_error("Code incomplet ou mal saisi : dix caractères, lettres et chiffres.".into());
                return;
            };
            if host.is_empty() || host.contains(['/', ' ', ':']) {
                ui.set_code_error("Indiquez le nom du PC sur Tailscale, tel qu'Eyrie l'affiche.".into());
                return;
            }
            Target {
                addrs: vec![format!("{host}:{}", suite_mobile::PORT)],
                host: host.clone(),
                ip: String::new(),
                port: suite_mobile::PORT,
                secret,
                key: None,
                label: host,
            }
        };
        ui.set_code_error(SharedString::new());
        self.begin_pairing(target);
    }

    fn start_scan(&mut self) {
        let Some(ui) = self.ui() else { return };
        ui.set_page(3);
        platform::start_scan(
            Box::new(|frame| {
                post(move |c| {
                    if let Some(ui) = c.ui() {
                        ui.set_scanner_frame(slint::Image::from_rgba8(frame));
                    }
                })
            }),
            Box::new(|text| post(move |c| c.on_scanned(&text))),
        );
    }

    fn on_scanned(&mut self, text: &str) {
        let Some(ui) = self.ui() else { return };
        if ui.get_page() != 3 {
            return;
        }
        match Ticket::from_uri(text) {
            Ok(t) => {
                platform::haptic();
                self.begin_pairing(target_of(&t));
            }
            Err(e) => ui.set_scanner_hint(e.into()),
        }
    }

    // --- Réglages ---

    fn switch_pc(&mut self, id: &str) {
        if self.store.pcs.iter().any(|p| p.id == id) {
            self.store.prefs.current = Some(id.to_string());
            self.store.save();
            self.reset_session();
            self.pc_name.clear();
            if let Some(pc) = self.store.current().cloned() {
                self.pc_name = pc.name.clone();
                self.connect(&pc);
            }
            self.refresh_all();
        }
    }

    fn forget_pc(&mut self) {
        let Some(id) = self.store.prefs.current.clone() else { return };
        self.link = None;
        self.store.forget(&id);
        self.reset_session();
        self.pc_name.clear();
        match self.store.current().cloned() {
            Some(pc) => {
                self.pc_name = pc.name.clone();
                self.connect(&pc);
                self.refresh_all();
            }
            None => {
                if let Some(ui) = self.ui() {
                    ui.set_has_pc(false);
                    ui.set_page(0);
                }
            }
        }
    }

    /// Développement : l'onglet (et le chemin, `fawkes/t/3`) à ouvrir à la première connexion.
    pub(crate) fn goto_when_online(&mut self, goto: String) {
        self.goto = Some(goto);
    }

    /// Démonstration : comme connecté à ce PC.
    pub(crate) fn demo_online(&mut self, pc: &str) {
        self.online = true;
        self.pc_name = pc.to_string();
        self.eyrie = env!("CARGO_PKG_VERSION").into();
        self.set_link(1, "Connecté");
        self.refresh_all();
    }

    pub(crate) fn demo_offline(&mut self) {
        self.online = false;
        self.set_link(2, "Hors ligne (PC injoignable) · nouvel essai dans 4 s");
    }

    /// L'application revient au premier plan : on se reconnecte tout de suite si besoin.
    pub fn resumed(&self) {
        if !self.online
            && let Some(l) = &self.link
        {
            l.kick();
        }
    }
}

/// La bulle pour les notifications arrivées (les plus récentes d'abord) : la notification elle-même, ou leur nombre et
/// la plus récente, au ton de la plus grave.
fn notes_toast(fresh: &[&Note]) -> Option<(String, Tone)> {
    let first = fresh.first()?;
    let grave = |t: &Tone| match t {
        Tone::Danger => 3,
        Tone::Warn => 2,
        Tone::Ok => 1,
        Tone::Info => 0,
    };
    let tone = fresh.iter().map(|n| n.tone).max_by_key(grave).unwrap_or_default();
    let text = match fresh.len() {
        1 if first.body.is_empty() => first.title.clone(),
        1 => format!("{} : {}", first.title, first.body),
        n => format!("{n} nouvelles notifications sur le PC · {}", first.title),
    };
    Some((text, tone))
}

fn target_of(t: &Ticket) -> Target {
    Target {
        addrs: t.addresses(),
        host: t.host.clone(),
        ip: t.ip.clone(),
        port: t.port,
        secret: t.secret.clone(),
        key: Some(t.key),
        label: if t.pc.is_empty() { t.host.clone() } else { t.pc.clone() },
    }
}

/// Relie les rappels de l'interface au contrôleur.
fn wire(ui: &AppWindow) {
    ui.on_nav_select(|k| {
        with(|c| c.nav_select(&k));
    });
    ui.on_open_path(|p| {
        with(|c| c.open_path(&p));
    });
    ui.on_open_module_path(|m, p| {
        with(|c| c.open_module_path(&m, &p));
    });
    ui.on_back_requested(|| with(|c| c.back()).unwrap_or(false));
    ui.on_run_act(|a, input| {
        with(|c| c.run_act(&a, &input));
    });
    ui.on_open_url(|u| platform::open_url(&u));
    ui.on_haptic(platform::haptic);
    ui.on_toggle_pin(|id| {
        with(|c| c.toggle_pin(&id));
    });
    ui.on_open_extra(|what| {
        with(|c| c.nav_select(&what));
    });
    ui.on_notes_read(|| {
        with(|c| c.send(ToPc::NotesRead));
    });
    ui.on_run_command(|cmd| {
        with(|c| c.run_command(&cmd));
    });
    ui.on_filter_edited(|_| {
        with(|c| c.refresh_commands());
    });
    let weak = ui.as_weak();
    ui.on_set_theme(move |i| {
        if let Some(ui) = weak.upgrade() {
            ui.global::<Theme>().set_choice(i);
        }
        with(|c| {
            c.store.prefs.theme = i;
            c.store.save();
            c.term_dark = None;
        });
    });
    let weak = ui.as_weak();
    ui.on_set_text_size(move |i| {
        let i = i.clamp(0, 2);
        if let Some(ui) = weak.upgrade() {
            ui.set_text_size(i);
            ui.set_term_zoom(TEXT_SIZES[i as usize]);
        }
        with(|c| {
            c.store.prefs.text_size = i;
            c.store.save();
        });
    });
    ui.on_rename_device(|name| {
        with(|c| {
            c.store.prefs.device_name = name.trim().to_string();
            c.store.save();
            c.toast("Nom enregistré : il servira aux prochains appairages.", 2);
        });
    });
    ui.on_switch_pc(|id| {
        with(|c| c.switch_pc(&id));
    });
    let weak = ui.as_weak();
    ui.on_pair_another(move || {
        if let Some(ui) = weak.upgrade() {
            ui.set_page(0);
        }
    });
    ui.on_forget_pc(|| {
        with(|c| c.forget_pc());
    });
    ui.on_retry_link(|| {
        with(|c| {
            if let Some(l) = &c.link {
                l.kick();
            }
        });
    });
    ui.on_tailnet_login(|| {
        with(|c| c.tailnet_login());
    });
    ui.on_tailnet_logout(|| {
        with(|c| c.tailnet_logout());
    });
    ui.on_start_scan(|| {
        with(|c| c.start_scan());
    });
    let weak = ui.as_weak();
    ui.on_enter_code(move || {
        platform::stop_scan();
        if let Some(ui) = weak.upgrade() {
            ui.set_page(1);
        }
    });
    ui.on_submit_code(|| {
        with(|c| c.submit_code());
    });
    ui.on_cancel_pairing(|| {
        with(|c| c.cancel_pairing());
    });
    ui.on_retry_pairing(|| {
        with(|c| {
            if let Some(t) = c.target.clone() {
                c.begin_pairing(t);
            }
        });
    });
    ui.on_pairing_done(|| {
        with(|c| c.pairing_done());
    });
    let weak = ui.as_weak();
    ui.on_onboarding_back(move || {
        if let Some(ui) = weak.upgrade() {
            ui.set_page(4);
        }
    });
    ui.on_term_send_line(|t| {
        with(|c| c.term_send_line(&t));
    });
    ui.on_term_key(|k| {
        with(|c| c.term_key(&k));
    });
    ui.on_term_reply(|i| {
        with(|c| c.term_reply(i.max(0) as usize));
    });
    let weak = ui.as_weak();
    ui.on_term_zoom_by(move |f| {
        if let Some(ui) = weak.upgrade() {
            let z = (ui.get_term_zoom() * f).clamp(0.6, 3.0);
            ui.set_term_zoom(z);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(title: &str, body: &str, tone: Tone, at: u64) -> Note {
        Note { module: "patronus".into(), title: title.into(), body: body.into(), tone, at, read: false, path: None }
    }

    #[test]
    fn what_happened_meanwhile_is_told_in_one_bubble() {
        assert_eq!(notes_toast(&[]), None);
        let one = note("Build terminé", "en 3 min", Tone::Ok, 10);
        assert_eq!(notes_toast(&[&one]), Some(("Build terminé : en 3 min".into(), Tone::Ok)));
        // Plusieurs pendant l'absence : leur nombre, la plus récente, le ton de la plus grave.
        let failed = note("Tests en échec", "", Tone::Danger, 8);
        let info = note("Rappel", "", Tone::Info, 5);
        assert_eq!(
            notes_toast(&[&one, &failed, &info]),
            Some(("3 nouvelles notifications sur le PC · Build terminé".into(), Tone::Danger))
        );
    }
}
