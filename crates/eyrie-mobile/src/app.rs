//! Le contrôleur : il tient l'état de l'application (PC, modules, écrans reçus, navigation, terminal suivi), reçoit
//! les messages du PC et les gestes de l'utilisateur, et met l'interface à jour. Il vit dans le fil de l'interface ;
//! les fils réseau lui écrivent par `slint::invoke_from_event_loop`.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};
use suite_mobile::msg::{CommandCard, ModuleCard, Note, Rights, ToPc, ToPhone, Waiting, module_target};
use suite_mobile::pairing::{self, Ticket};
use suite_mobile::term::{Key, TermInput};
use suite_mobile::ui::{Act, Block, Fold, Screen, Tab, Tone};

use crate::net::{Link, NetEvent, PairEvent};
use crate::store::{self, Pc, Store};
use crate::tailnet::{self, Tailnet};
use crate::termbuf::{self, TermBuf};
use crate::{
    AppWindow, CommandItem, ModuleItem, NavItem, NoteItem, PcItem, TabItem, TermRow, TermSeg, Theme, UiAct, UiRow,
    WaitItem, platform, view,
};

/// Les modules proposés dans la barre du bas tant que l'utilisateur n'a rien choisi.
const DEFAULT_PINS: [&str; 8] =
    ["fawkes", "patronus", "owlery", "menagerie", "gringotts", "dragontree", "grimoire", "carnet"];
const EXTRAS: [&str; 3] = ["notes", "commandes", "reglages"];
const TEXT_SIZES: [f32; 3] = [1.0, 1.35, 1.7];
/// Une action sans résultat du PC passé ce délai : « Le PC n'a pas répondu ».
const ANSWER_WITHIN: Duration = Duration::from_secs(15);
/// Le temps qu'un bandeau garde son pliage après une action de son menu (et après son résultat).
const HOLD_FOLD: Duration = Duration::from_secs(6);

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

/// Bulles en attente au plus (les plus anciennes, hors erreurs, cèdent leur place).
const BUBBLES_MAX: usize = 4;

/// Les bulles : une à la fois, chacune pour tout son temps (une nouvelle ne coupe pas celle qui est montrée).
#[derive(Default)]
struct Bubbles {
    queue: std::collections::VecDeque<(String, i32)>,
    showing: Option<(String, i32)>,
}

impl Bubbles {
    /// Ajoute une bulle (une seule fois la même) ; `true` : rien n'est montré, elle peut l'être tout de suite.
    fn push(&mut self, text: &str, tone: i32) -> bool {
        let same = |(t, _): &(String, i32)| t == text;
        if self.showing.as_ref().is_some_and(same) || self.queue.iter().any(same) {
            return false;
        }
        if self.queue.len() >= BUBBLES_MAX {
            let drop = self.queue.iter().position(|(_, tone)| *tone != 4).unwrap_or(0);
            self.queue.remove(drop);
        }
        self.queue.push_back((text.to_string(), tone));
        self.showing.is_none()
    }

    /// La bulle suivante, désormais montrée.
    fn next(&mut self) -> Option<(String, i32)> {
        self.showing = self.queue.pop_front();
        self.showing.clone()
    }
}

/// Le temps d'une bulle : une erreur reste plus longtemps.
fn bubble_time(tone: i32) -> Duration {
    Duration::from_millis(if tone == 4 { 6000 } else { 3200 })
}

/// Une action envoyée au PC, en attente de son résultat : son bouton est grisé, avec un indicateur.
#[derive(Debug, PartialEq)]
struct Pending {
    module: String,
    path: String,
    act: String,
}

/// Les demandes au PC : leur numéro (actions et commandes), et les actions envoyées sans résultat encore.
#[derive(Default)]
struct Pendings {
    req: u32,
    sent: HashMap<u32, Pending>,
}

impl Pendings {
    /// Un nouveau numéro de demande.
    fn next(&mut self) -> u32 {
        self.req += 1;
        self.req
    }

    /// Une action part : son numéro ; `None` si la même attend encore son résultat (deux touchers : un seul envoi,
    /// pas deux Pull dans la file de DragonTree).
    fn send(&mut self, module: &str, path: &str, act: &str) -> Option<u32> {
        if self.sent.values().any(|p| p.module == module && p.path == path && p.act == act) {
            return None;
        }
        let req = self.next();
        self.sent.insert(req, Pending { module: module.into(), path: path.into(), act: act.into() });
        Some(req)
    }

    /// Le résultat d'une demande, ou son délai passé : l'action qu'elle portait, une seule fois (un résultat arrivé
    /// après le délai ne fait plus rien).
    fn answer(&mut self, req: u32) -> Option<Pending> {
        self.sent.remove(&req)
    }

    /// Les actions en cours dans cet écran.
    fn acts(&self, key: &(String, String)) -> HashSet<String> {
        let here = self.sent.values().filter(|p| p.module == key.0 && p.path == key.1);
        here.map(|p| p.act.clone()).collect()
    }
}

/// Les bandeaux dont le menu vient de servir : leur pliage du moment, gardé quelques secondes après l'action et après
/// son résultat, même si l'état de départ que donne le PC change (« Tout mettre en veille » : le groupe tout en veille,
/// plié par défaut, ne se replie pas sous le doigt). Le choix de l'utilisateur, lui, l'emporte toujours.
#[derive(Default)]
struct Holds(HashMap<(String, String, String), Held>);

struct Held {
    open: bool,
    act: String,
    until: Instant,
}

impl Holds {
    /// Le bandeau `band` de l'écran `(module, path)` garde ce pliage pendant que l'action `act` part.
    fn hold(&mut self, module: &str, path: &str, band: &str, open: bool, act: &str, now: Instant) {
        let key = (module.to_string(), path.to_string(), band.to_string());
        self.0.insert(key, Held { open, act: act.to_string(), until: now + HOLD_FOLD });
    }

    /// Le résultat de l'action est arrivé : ses bandeaux gardent leur pliage encore un peu (le PC change leur état de
    /// départ après l'action). `true` : il y en avait.
    fn answered(&mut self, module: &str, path: &str, act: &str, now: Instant) -> bool {
        let mut any = false;
        for ((m, p, _), h) in self.0.iter_mut() {
            if m == module && p == path && h.act == act && h.until > now {
                h.until = now + HOLD_FOLD;
                any = true;
            }
        }
        any
    }

    /// Le pliage gardé de ce bandeau, s'il l'est encore (les gardes passées sont oubliées).
    fn open(&mut self, module: &str, path: &str, band: &str, now: Instant) -> Option<bool> {
        self.0.retain(|_, h| h.until > now);
        self.0.get(&(module.to_string(), path.to_string(), band.to_string())).map(|h| h.open)
    }
}

/// Ce qu'un geste de navigation demande à la liste.
#[derive(Debug, PartialEq)]
enum Moved {
    /// Un autre écran, à montrer.
    Screen,
    /// Le même écran, sa liste remontée en haut.
    Top,
}

/// La navigation : l'onglet du bas, la pile d'écrans de chaque module, la position de la liste et le texte cherché
/// de chaque écran. Sans interface : le contrôleur la suit et l'affiche.
struct Nav {
    tab: String,
    stacks: HashMap<String, Vec<String>>,
    /// La position de la liste de chaque écran quitté, rendue au retour ; un écran ouvert de nouveau part du haut.
    scroll: HashMap<(String, String), f32>,
    /// Le texte cherché dans chaque écran qui a une recherche (filtrée sur le téléphone).
    search: HashMap<(String, String), String>,
}

impl Nav {
    fn new() -> Nav {
        Nav { tab: "ensemble".into(), stacks: HashMap::new(), scroll: HashMap::new(), search: HashMap::new() }
    }

    fn is_module_tab(&self) -> bool {
        self.tab != "ensemble" && self.tab != "plus" && !EXTRAS.contains(&self.tab.as_str())
    }

    fn top_path(&self) -> String {
        self.stacks.get(&self.tab).and_then(|s| s.last().cloned()).unwrap_or_default()
    }

    /// L'écran affiché : (module, chemin).
    fn key(&self) -> (String, String) {
        (self.tab.clone(), self.top_path())
    }

    /// Cet écran est celui qui est affiché.
    fn is_shown(&self, key: &(String, String)) -> bool {
        self.is_module_tab() && key.0 == self.tab && key.1 == self.top_path()
    }

    fn stack(&mut self) -> &mut Vec<String> {
        self.stacks.entry(self.tab.clone()).or_insert_with(|| vec![String::new()])
    }

    /// Retient la position `y` de la liste de `shown` (l'écran dont les lignes sont montrées), s'il est affiché.
    fn remember(&mut self, shown: Option<&(String, String)>, y: f32) {
        if let Some(key) = shown
            && self.is_shown(key)
        {
            self.scroll.insert(key.clone(), y);
        }
    }

    /// La position où rendre la liste de cet écran (0 : en haut).
    fn scroll_of(&self, key: &(String, String)) -> f32 {
        self.scroll.get(key).copied().unwrap_or(0.0)
    }

    fn search_of(&self, key: &(String, String)) -> String {
        self.search.get(key).cloned().unwrap_or_default()
    }

    /// Toucher un onglet du bas. Celui du module affiché remonte d'abord en haut de sa liste (`listed` : une liste
    /// est montrée ; `at_top` : elle y est déjà), puis ramène à l'accueil du module.
    fn select_bottom(&mut self, key: &str, listed: bool, at_top: bool) -> Moved {
        if key == self.tab && self.is_module_tab() {
            if listed && !at_top {
                self.scroll.remove(&self.key());
                return Moved::Top;
            }
            self.stacks.insert(key.to_string(), vec![String::new()]);
        }
        self.tab = key.to_string();
        if self.is_module_tab() {
            self.stack();
        }
        Moved::Screen
    }

    /// Ouvre un écran d'un module (vide : son accueil, la pile de cet onglet repart de là) ; un écran poussé part du
    /// haut.
    fn open_module_path(&mut self, module: &str, path: &str) {
        self.tab = module.to_string();
        let stack = self.stack();
        if path.is_empty() {
            *stack = vec![String::new()];
        } else if stack.last().map(String::as_str) != Some(path) {
            stack.push(path.to_string());
            self.scroll.remove(&(module.to_string(), path.to_string()));
        }
    }

    /// Ouvre un sous-écran du module affiché : il part du haut. `false` : pas de module affiché.
    fn open_path(&mut self, path: &str) -> bool {
        if !self.is_module_tab() {
            return false;
        }
        self.stack().push(path.to_string());
        self.scroll.remove(&(self.tab.clone(), path.to_string()));
        true
    }

    /// Ouvre ce que désigne un toucher (`Row.open`, `Act.open`) : un sous-écran du module affiché, ou l'écran d'un autre
    /// module (« @module <id> [<chemin>] » : son onglet, puis cet écran ; rien n'est envoyé au PC). `false` : rien.
    fn open(&mut self, open: &str) -> bool {
        match module_target(open) {
            Some((module, path)) => {
                self.open_module_path(module, path);
                true
            }
            None => self.open_path(open),
        }
    }

    /// Passe à un onglet de l'écran : il remplace l'écran affiché dans la pile (le retour mène à ce qu'il y avait
    /// dessous) et garde sa position dans la liste ; l'onglet affiché remonte en haut. Un onglet vers un autre module
    /// (« @module … ») l'ouvre, comme un lien. `None` : pas de module affiché.
    fn select_tab(&mut self, path: &str) -> Option<Moved> {
        if !self.is_module_tab() {
            return None;
        }
        if let Some((module, path)) = module_target(path) {
            self.open_module_path(module, path);
            return Some(Moved::Screen);
        }
        if self.top_path() == path {
            self.scroll.remove(&self.key());
            return Some(Moved::Top);
        }
        let stack = self.stack();
        match stack.last_mut() {
            Some(top) => *top = path.to_string(),
            None => stack.push(path.to_string()),
        }
        Some(Moved::Screen)
    }

    /// Retour : l'écran d'avant dans le module, sinon la vue d'ensemble ; d'une page de « Plus », « Plus ». `false` :
    /// rien à quitter.
    fn back(&mut self) -> bool {
        if self.is_module_tab() {
            let stack = self.stack();
            if stack.len() > 1 {
                stack.pop();
            } else {
                self.tab = "ensemble".into();
            }
            return true;
        }
        match self.tab.as_str() {
            t if EXTRAS.contains(&t) => self.tab = "plus".into(),
            "plus" => self.tab = "ensemble".into(),
            _ => return false,
        }
        true
    }

    /// Un écran a disparu sur le PC (terminal fermé, entrée traitée) : s'il était affiché au-dessus d'un autre, retour
    /// à celui-ci (`true`).
    fn gone(&mut self, module: &str, path: &str) -> bool {
        if self.tab != module {
            return false;
        }
        let stack = self.stack();
        if stack.len() > 1 && stack.last().map(String::as_str) == Some(path) {
            stack.pop();
            return true;
        }
        false
    }

    /// Développement : l'onglet et le chemin à ouvrir (`fawkes/t/3`).
    fn goto(&mut self, goto: &str) {
        let (tab, path) = goto.split_once('/').unwrap_or((goto, ""));
        self.tab = tab.to_string();
        let mut stack = vec![String::new()];
        if !path.is_empty() {
            stack.push(path.to_string());
        }
        self.stacks.insert(tab.to_string(), stack);
    }
}

/// Les onglets du dernier écran qui en avait, avec son titre : montrés le temps qu'un onglet voisin arrive du PC.
struct Tabs {
    module: String,
    tabs: Vec<Tab>,
    title: String,
    subtitle: String,
}

/// Deux vibrations brèves : un échec.
fn haptic_error() {
    platform::haptic();
    slint::Timer::single_shot(Duration::from_millis(140), platform::haptic);
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
    nav: Nav,
    followed: Option<Followed>,
    term: TermBuf,
    term_model: Rc<VecModel<TermRow>>,
    term_dark: Option<bool>,
    /// Les listes de l'interface, mises à jour sur place (jamais remplacées) : une ligne touchée survit aux messages.
    models: Models,
    /// L'écran dont les lignes sont affichées, et leurs clés et empreintes (voir [`view::patch`]).
    shown_for: Option<(String, String)>,
    shown: Vec<(String, u64)>,
    scroll_seq: i32,
    /// Les lignes des diffs passent à la ligne (le temps de la session).
    diff_wrap: bool,
    /// Ce qui attend a été reçu au moins une fois : sinon, la vue d'ensemble attend le PC.
    got_overview: bool,
    /// Le dernier message du PC (secondes Unix) : hors ligne, la vue d'ensemble dit de quand datent ses données.
    data_at: Option<u64>,
    age_timer: slint::Timer,
    /// Les notifications dépliées (sans écran où mener, le toucher montre leur texte entier).
    expanded_notes: std::collections::HashSet<String>,
    /// Les bulles à montrer, une à la fois, et le temps de celle qui l'est.
    bubbles: RefCell<Bubbles>,
    bubble_timer: slint::Timer,
    last_tabs: Option<Tabs>,
    /// Les actions envoyées sans résultat du PC : pas de second envoi de la même.
    pendings: Pendings,
    /// Les bandeaux dont le menu vient de servir, qui gardent leur pliage un moment.
    holds: Holds,
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

/// Les modèles des listes de l'interface.
struct Models {
    rows: Rc<VecModel<UiRow>>,
    acts: Rc<VecModel<UiAct>>,
    bar: Rc<VecModel<UiAct>>,
    tabs: Rc<VecModel<TabItem>>,
    nav: Rc<VecModel<NavItem>>,
    modules: Rc<VecModel<ModuleItem>>,
    waiting: Rc<VecModel<WaitItem>>,
    notes: Rc<VecModel<NoteItem>>,
    recent_notes: Rc<VecModel<NoteItem>>,
    commands: Rc<VecModel<CommandItem>>,
}

impl Models {
    fn install(ui: &AppWindow) -> Models {
        let m = Models {
            rows: Rc::default(),
            acts: Rc::default(),
            bar: Rc::default(),
            tabs: Rc::default(),
            nav: Rc::default(),
            modules: Rc::default(),
            waiting: Rc::default(),
            notes: Rc::default(),
            recent_notes: Rc::default(),
            commands: Rc::default(),
        };
        ui.set_rows(ModelRc::from(m.rows.clone()));
        ui.set_screen_acts(ModelRc::from(m.acts.clone()));
        ui.set_bar_acts(ModelRc::from(m.bar.clone()));
        ui.set_tabs(ModelRc::from(m.tabs.clone()));
        ui.set_nav(ModelRc::from(m.nav.clone()));
        ui.set_modules(ModelRc::from(m.modules.clone()));
        ui.set_waiting(ModelRc::from(m.waiting.clone()));
        ui.set_notes(ModelRc::from(m.notes.clone()));
        ui.set_recent_notes(ModelRc::from(m.recent_notes.clone()));
        ui.set_commands(ModelRc::from(m.commands.clone()));
        m
    }
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
        let models = Models::install(ui);
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
            nav: Nav::new(),
            followed: None,
            term: TermBuf::default(),
            term_model,
            term_dark: None,
            models,
            shown_for: None,
            shown: Vec::new(),
            scroll_seq: 0,
            diff_wrap: false,
            got_overview: false,
            data_at: None,
            age_timer: slint::Timer::default(),
            expanded_notes: Default::default(),
            bubbles: RefCell::default(),
            bubble_timer: slint::Timer::default(),
            last_tabs: None,
            pendings: Pendings::default(),
            holds: Holds::default(),
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
            c.age_timer.start(slint::TimerMode::Repeated, Duration::from_secs(20), || {
                with(|c| c.refresh_age());
            });
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
        self.refresh_age();
    }

    /// Hors ligne, de quand datent les données montrées.
    fn refresh_age(&self) {
        let Some(ui) = self.ui() else { return };
        let age = match self.data_at {
            Some(at) if !self.online => view::data_age(at, now()),
            _ => String::new(),
        };
        ui.set_data_age(age.into());
    }

    /// Une bulle : montrée tout de suite, ou après celles qui attendent (chacune garde tout son temps).
    pub fn toast(&self, text: &str, tone: i32) {
        if self.bubbles.borrow_mut().push(text, tone) {
            self.next_bubble();
        }
    }

    fn next_bubble(&self) {
        let Some(ui) = self.ui() else { return };
        let next = self.bubbles.borrow_mut().next();
        let Some((text, tone)) = next else {
            ui.set_toast_visible(false);
            return;
        };
        ui.set_toast_text(text.into());
        ui.set_toast_tone(tone);
        ui.set_toast_expanded(false);
        ui.set_toast_visible(true);
        self.bubble_timer.start(slint::TimerMode::SingleShot, bubble_time(tone), || {
            with(|c| c.next_bubble());
        });
    }

    /// Toucher une bulle : une erreur se déplie pour être lue en entière (elle reste alors, 30 s au plus) ; sinon, ou
    /// déjà dépliée, elle s'en va.
    pub(crate) fn toast_tapped(&self) {
        let Some(ui) = self.ui() else { return };
        let tone = self.bubbles.borrow().showing.as_ref().map(|(_, tone)| *tone);
        if tone == Some(4) && !ui.get_toast_expanded() {
            ui.set_toast_expanded(true);
            self.bubble_timer.start(slint::TimerMode::SingleShot, Duration::from_secs(30), || {
                with(|c| c.next_bubble());
            });
        } else {
            self.bubble_timer.stop();
            self.next_bubble();
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
                    self.nav.goto(&goto);
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
        self.data_at = Some(now());
        match m {
            ToPhone::Modules { modules } => {
                self.cards = modules;
                self.refresh_modules();
                // Les lignes de l'écran ne changent pas ; son titre, s'il n'est pas encore arrivé, est le nom du module.
                if self.nav.is_module_tab() && !self.screens.contains_key(&self.nav.key()) {
                    self.refresh_screen();
                }
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
                self.got_overview = true;
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
                let shown = self.nav.is_shown(&key);
                self.screens.insert(key, screen);
                // Un écran qui n'est pas affiché (envoyé avant un changement d'écran) ne touche pas à la liste.
                if shown {
                    self.refresh_screen();
                }
            }
            ToPhone::NoScreen { module, path, reason } => {
                // L'écran ouvert a disparu (terminal fermé, entrée traitée) : retour à l'écran d'avant.
                if self.nav.gone(&module, &path) {
                    self.toast(&reason, 1);
                } else {
                    let shown = self.nav.is_shown(&(module.clone(), path.clone()));
                    self.missing.insert((module, path), reason);
                    if !shown {
                        return;
                    }
                }
                self.refresh_screen();
            }
            ToPhone::Term(f) => {
                if self.followed_term().is_some_and(|(m, id)| m == f.module && id == f.id) {
                    let change = self.term.apply(&f);
                    self.refresh_term(change);
                }
            }
            ToPhone::Result { req, ok, message } => {
                let done = self.pendings.answer(req);
                if let Some(p) = &done
                    && self.holds.answered(&p.module, &p.path, &p.act, Instant::now())
                {
                    self.settle_holds();
                }
                if !ok {
                    self.toast(&message, 4);
                    haptic_error();
                } else {
                    if done.is_some() {
                        platform::haptic();
                    }
                    if !message.is_empty() {
                        self.toast(&message, 2);
                    }
                }
                if let Some(p) = done
                    && self.nav.is_shown(&(p.module, p.path))
                {
                    self.refresh_screen();
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
            .map(|n| {
                let key = view::note_key(n.at, &n.module, &n.title);
                NoteItem {
                    module: n.module.as_str().into(),
                    module_name: self.card(&n.module).map(|c| c.name.as_str()).unwrap_or(&n.module).into(),
                    title: n.title.as_str().into(),
                    body: n.body.as_str().into(),
                    tone: view::tone(Some(n.tone)),
                    when: view::ago(n.at, t).into(),
                    read: n.read,
                    path: n.path.clone().unwrap_or_default().into(),
                    has_path: n.path.is_some(),
                    expanded: self.expanded_notes.contains(&key),
                    key: key.into(),
                }
            })
            .collect();
        view::sync(&self.models.modules, items);
        view::sync(&self.models.nav, nav);
        view::sync(&self.models.waiting, waiting);
        view::sync(&self.models.recent_notes, notes.iter().take(3).cloned().collect());
        view::sync(&self.models.notes, notes);
        ui.set_unread(self.unread as i32);
        ui.set_waiting_known(self.got_overview);
    }

    fn refresh_commands(&self) {
        let Some(ui) = self.ui() else { return };
        let filter = view::plain(ui.get_command_filter().trim());
        let items: Vec<CommandItem> = self
            .commands
            .iter()
            .filter_map(|c| {
                let module_name = self.card(&c.module).map(|m| m.name.clone()).unwrap_or_else(|| c.module.clone());
                let hit = filter.is_empty()
                    || view::plain(&c.title).contains(&filter)
                    || view::plain(&module_name).contains(&filter);
                hit.then(|| CommandItem {
                    module: c.module.as_str().into(),
                    module_name: module_name.into(),
                    id: c.id.as_str().into(),
                    title: c.title.as_str().into(),
                })
            })
            .collect();
        view::sync(&self.models.commands, items);
    }

    /// Ce que le téléphone ajoute à cet écran : ses actions en cours, le pliage de ses bandeaux. Un bandeau plié qui
    /// devient plus grave se rouvre (« l'alerte gagne »), un bandeau plié qui se calme retient son nouveau ton (la
    /// prochaine alerte le rouvrira) : ces changements sont retenus.
    fn local(&mut self, key: &(String, String)) -> view::Local {
        let mut local = view::Local { pending: self.pendings.acts(key), wrap: self.diff_wrap, ..Default::default() };
        if self.screens.get(key).is_some_and(|s| !s.search.is_empty()) {
            local.search = self.nav.search_of(key);
        }
        let pc = self.store.prefs.current.clone().unwrap_or_default();
        let mut changed = Vec::new();
        let now = Instant::now();
        for (band, default_open, tone) in bands(self.screens.get(key)) {
            let chosen = self.store.folded(&pc, &key.0, &key.1, &band).map(|f| (f.open, f.tone));
            let (open, keep) = view::band_open(chosen, default_open, tone);
            if chosen.is_some() {
                local.open.insert(band.clone(), open);
            } else if let Some(held) = self.holds.open(&key.0, &key.1, &band, now) {
                // Son menu vient de servir : l'état de départ du PC attend un peu.
                local.open.insert(band.clone(), held);
            }
            if let Some(keep) = keep {
                changed.push((band, keep));
            }
        }
        for (band, (open, tone)) in &changed {
            self.store.set_folded(&pc, &key.0, &key.1, band, *open, *tone);
        }
        if !changed.is_empty() {
            self.store.save();
        }
        local
    }

    /// Une recherche filtre cet écran : ses bandeaux restent ouverts, sans se plier.
    fn searching(&self, key: &(String, String)) -> bool {
        self.screens.get(key).is_some_and(|s| !s.search.is_empty()) && !self.nav.search_of(key).trim().is_empty()
    }

    /// Plie ou déplie des bandeaux de l'écran affiché (`None` : celui-ci seulement, à l'inverse de son état), sur le
    /// téléphone seulement. Rien pendant une recherche : le pliage, caché, changerait sans que rien ne bouge.
    fn fold_bands(&mut self, only: Option<&str>, open: bool) {
        let Some(key) = self.shown_for.clone() else { return };
        if self.searching(&key) {
            return;
        }
        let local = self.local(&key);
        let pc = self.store.prefs.current.clone().unwrap_or_default();
        for (band, default_open, tone) in bands(self.screens.get(&key)) {
            match only {
                Some(b) if b == band => {
                    let now = local.open.get(&band).copied().unwrap_or(default_open);
                    self.store.set_folded(&pc, &key.0, &key.1, &band, !now, tone);
                }
                Some(_) => {}
                None => self.store.set_folded(&pc, &key.0, &key.1, &band, open, tone),
            }
        }
        self.store.save();
        self.refresh_screen();
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
        let tab = self.nav.tab.clone();
        ui.set_tab(tab.as_str().into());
        let mut screen_follow = None;
        let mut term_follow = None;
        let mut search = String::new();
        let mut tabs = Vec::new();
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
                let path = self.nav.top_path();
                let key = (module.to_string(), path.clone());
                let stack_len = self.nav.stacks.get(module).map_or(1, Vec::len);
                screen_follow = Some(key.clone());
                let name = self.card(module).map(|c| c.name.clone()).unwrap_or_else(|| module.to_string());
                match self.screens.get(&key) {
                    Some(s) => {
                        if let Some(id) = &s.terminal {
                            term_follow = Some((module.to_string(), id.clone()));
                        }
                        search = s.search.clone();
                        tabs = s.tabs.clone();
                        (s.title.clone(), s.subtitle.clone(), stack_len > 1, s.acts.clone())
                    }
                    // Un onglet voisin pas encore arrivé : les onglets et le titre de l'écran d'où l'on vient.
                    None => match &self.last_tabs {
                        Some(t) if t.module == module && t.tabs.iter().any(|t| t.path == path) => {
                            tabs = t.tabs.clone();
                            (t.title.clone(), t.subtitle.clone(), stack_len > 1, Vec::new())
                        }
                        _ => (name, String::new(), stack_len > 1, Vec::new()),
                    },
                }
            }
        };
        let module_tab = self.nav.is_module_tab();
        let key = self.nav.key();
        ui.set_loading(module_tab && !self.screens.contains_key(&key));
        ui.set_missing(self.missing.get(&key).cloned().unwrap_or_default().into());
        ui.set_screen_title(title.as_str().into());
        ui.set_subtitle(subtitle.as_str().into());
        ui.set_search_hint(search.into());
        let path = self.nav.top_path();
        let items = tabs.iter().map(|t| TabItem {
            label: t.label.as_str().into(),
            path: t.path.as_str().into(),
            badge: t.badge.as_str().into(),
            active: t.path == path,
        });
        view::sync(&self.models.tabs, items.collect());
        if !tabs.is_empty() && self.screens.contains_key(&(tab.clone(), path)) {
            self.last_tabs = Some(Tabs { module: tab.clone(), tabs, title, subtitle });
        }
        ui.set_can_back(can_back);
        ui.set_show_pc(acts.iter().any(|a| a.id == "@pc"));
        let local = screen_follow.as_ref().map(|k| self.local(k)).unwrap_or_default();
        let acts: Vec<Act> = acts.into_iter().filter(|a| a.id != "@pc").collect();
        let mut mine = Vec::new();
        if screen_follow.as_ref().is_some_and(|k| bands(self.screens.get(k)).len() >= 2 && !self.searching(k)) {
            mine.push(Act::new("@replier", "Tout replier").glyph("▸"));
            mine.push(Act::new("@deplier", "Tout déplier").glyph("▾"));
        }
        let (icons, more) = view::bar(&acts, &mine);
        view::sync(&self.models.bar, icons.into_iter().map(|a| view::act_in(a, &local)).collect());
        view::sync(&self.models.acts, more.into_iter().map(|a| view::act_in(a, &local)).collect());
        self.refresh_rows(screen_follow.as_ref().filter(|_| term_follow.is_none()));
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

    /// Les lignes de l'écran affiché (`None` : pas d'écran de module en liste), mises à jour sur place.
    fn refresh_rows(&mut self, key: Option<&(String, String)>) {
        let Some(key) = key.filter(|k| self.screens.contains_key(*k)).cloned() else {
            if self.models.rows.row_count() > 0 {
                self.models.rows.set_vec(Vec::new());
            }
            if self.shown_for.take().is_some() {
                self.scroll_to(0.0);
            }
            self.shown.clear();
            return;
        };
        let local = self.local(&key);
        let items = view::items(&self.screens[&key], &local);
        if self.shown_for.as_ref() == Some(&key) {
            view::patch(&self.models.rows, &mut self.shown, &items, |i| view::ui_row(i, &local));
            return;
        }
        // Un autre écran : la liste repart de zéro, à la position où on l'avait laissée, avec sa recherche.
        if let Some(ui) = self.ui() {
            ui.set_search_text(local.search.as_str().into());
        }
        self.models.rows.set_vec(items.iter().map(|i| view::ui_row(i, &local)).collect::<Vec<_>>());
        self.shown = items.iter().map(|i| (i.key.clone(), i.sig)).collect();
        let y = self.nav.scroll_of(&key);
        self.shown_for = Some(key);
        self.scroll_to(y);
    }

    /// Les lignes des diffs passent à la ligne, ou plus.
    pub(crate) fn toggle_wrap(&mut self) {
        self.diff_wrap = !self.diff_wrap;
        self.refresh_screen();
    }

    /// Le texte cherché dans l'écran affiché a changé : la liste est filtrée sur le téléphone, depuis son début (et le
    /// menu ⋯ perd « Tout replier » le temps de la recherche).
    pub(crate) fn search_edited(&mut self, text: &str) {
        let Some(key) = self.shown_for.clone() else { return };
        self.nav.search.insert(key, text.to_string());
        self.refresh_screen();
        self.scroll_to(0.0);
    }

    /// Retient la position de la liste de l'écran affiché, avant d'en changer.
    fn remember_scroll(&mut self) {
        let Some(ui) = self.ui() else { return };
        self.nav.remember(self.shown_for.as_ref(), ui.get_screen_scroll());
    }

    /// Met la liste de l'écran à cette position (0 : en haut).
    fn scroll_to(&mut self, y: f32) {
        let Some(ui) = self.ui() else { return };
        self.scroll_seq = self.scroll_seq.wrapping_add(1);
        ui.set_scroll_want(y);
        ui.set_scroll_seq(self.scroll_seq);
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
        self.remember_scroll();
        // Toucher l'onglet du module courant remonte en haut de la liste, puis ramène à son accueil.
        let at_top = self.ui().is_none_or(|u| u.get_screen_scroll() > -1.0);
        match self.nav.select_bottom(key, self.shown_for.is_some(), at_top) {
            Moved::Top => self.scroll_to(0.0),
            Moved::Screen => self.refresh_screen(),
        }
    }

    /// Ouvre un écran d'un module (vide : son accueil, la pile de cet onglet repart de là).
    pub(crate) fn open_module_path(&mut self, module: &str, path: &str) {
        self.remember_scroll();
        self.nav.open_module_path(module, path);
        self.refresh_all();
    }

    /// Ouvre un sous-écran du module courant (il part du haut), ou l'écran d'un autre module (« @module <id>
    /// [<chemin>] »), sans rien envoyer au PC.
    pub(crate) fn open_path(&mut self, path: &str) {
        self.remember_scroll();
        if self.nav.open(path) {
            self.refresh_screen();
        }
    }

    /// Passe à un onglet de l'écran : il remplace l'écran affiché dans la pile (le retour mène à ce qu'il y avait
    /// dessous) et garde sa position dans la liste. L'onglet affiché : remonte en haut.
    pub(crate) fn select_tab(&mut self, path: &str) {
        self.remember_scroll();
        match self.nav.select_tab(path) {
            Some(Moved::Top) => self.scroll_to(0.0),
            Some(Moved::Screen) => self.refresh_screen(),
            None => {}
        }
    }

    /// Toucher une notification : l'écran du module où elle mène ; sans écran, son texte entier (ou de nouveau
    /// replié).
    pub(crate) fn open_note(&mut self, n: &NoteItem) {
        if n.has_path {
            return self.open_module_path(&n.module, &n.path);
        }
        let key = n.key.to_string();
        if !self.expanded_notes.remove(&key) {
            self.expanded_notes.insert(key);
        }
        self.refresh_modules();
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
        self.remember_scroll();
        if !self.nav.back() {
            return false;
        }
        self.refresh_screen();
        true
    }

    pub(crate) fn run_act(&mut self, a: &UiAct, input: &str) {
        match a.id.as_str() {
            "@lu" => return self.send(ToPc::NotesRead),
            "@replier" => return self.fold_bands(None, false),
            "@deplier" => return self.fold_bands(None, true),
            _ => {}
        }
        if !self.nav.is_module_tab() || a.busy {
            return;
        }
        if !self.online {
            self.toast("Hors ligne : l'action n'est pas partie.", 3);
            return;
        }
        let (module, path) = self.nav.key();
        // Une seule fois tant que le PC n'a pas répondu.
        let Some(req) = self.pendings.send(&module, &path, &a.id) else { return };
        self.hold_bands(&module, &path, &a.id);
        let input = (a.has_input).then(|| input.to_string());
        self.send(ToPc::Action { req, module, path, action: a.id.to_string(), input });
        slint::Timer::single_shot(ANSWER_WITHIN, move || {
            with(|c| c.no_answer(req));
        });
        self.refresh_screen();
    }

    /// Une action part du menu d'un bandeau : il garde son pliage du moment quelques secondes.
    fn hold_bands(&mut self, module: &str, path: &str, act: &str) {
        let key = (module.to_string(), path.to_string());
        let local = self.local(&key);
        let Some(screen) = self.screens.get(&key) else { return };
        let now = Instant::now();
        let mut any = false;
        for b in &screen.blocks {
            if let Block::Section { key: band, fold, acts, .. } = b
                && !band.is_empty()
                && *fold != Fold::No
                && acts.iter().any(|a| a.id == act)
            {
                let open = local.open.get(band).copied().unwrap_or(*fold == Fold::Open);
                self.holds.hold(module, path, band, open, act, now);
                any = true;
            }
        }
        if any {
            self.settle_holds();
        }
    }

    /// La garde finie, l'état de départ du PC revient.
    fn settle_holds(&self) {
        slint::Timer::single_shot(HOLD_FOLD + Duration::from_millis(100), || {
            with(|c| c.refresh_screen());
        });
    }

    /// Le PC n'a pas donné le résultat d'une action à temps : elle redevient possible.
    fn no_answer(&mut self, req: u32) {
        let Some(p) = self.pendings.answer(req) else { return };
        self.toast("Le PC n'a pas répondu.", 4);
        haptic_error();
        if self.nav.is_shown(&(p.module, p.path)) {
            self.refresh_screen();
        }
    }

    fn run_command(&mut self, c: &CommandItem) {
        if !self.online {
            self.toast("Hors ligne : la commande n'est pas partie.", 3);
            return;
        }
        let req = self.pendings.next();
        self.send(ToPc::Command { req, module: c.module.to_string(), id: c.id.to_string() });
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
        self.got_overview = false;
        self.data_at = None;
        self.expanded_notes.clear();
        self.screens.clear();
        self.missing.clear();
        self.nav = Nav::new();
        self.pendings.sent.clear();
        self.holds = Holds::default();
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

    /// Démonstration : la liaison à ce PC dans cet état (0 connexion en cours, 2 hors ligne, 3 refusée), avant toute
    /// donnée.
    pub(crate) fn demo_link(&mut self, pc: &str, state: i32, text: &str) {
        self.online = false;
        self.pc_name = pc.to_string();
        self.set_link(state, text);
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

/// Les bandeaux pliables d'un écran : clé, ouvert au départ, ton.
fn bands(screen: Option<&Screen>) -> Vec<(String, bool, i32)> {
    let blocks = screen.map(|s| s.blocks.as_slice()).unwrap_or_default();
    blocks
        .iter()
        .filter_map(|b| match b {
            Block::Section { key, fold, tone, .. } if !key.is_empty() && *fold != Fold::No => {
                Some((key.clone(), *fold == Fold::Open, view::tone(*tone)))
            }
            _ => None,
        })
        .collect()
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
    ui.on_fold_band(|band| {
        with(|c| c.fold_bands(Some(&band), false));
    });
    ui.on_toggle_wrap(|| {
        with(|c| c.toggle_wrap());
    });
    ui.on_search_edited(|text| {
        with(|c| c.search_edited(&text));
    });
    ui.on_select_tab(|path| {
        with(|c| c.select_tab(&path));
    });
    ui.on_open_url(|u| platform::open_url(&u));
    ui.on_haptic(platform::haptic);
    ui.on_toggle_pin(|id| {
        with(|c| c.toggle_pin(&id));
    });
    ui.on_open_extra(|what| {
        with(|c| c.nav_select(&what));
    });
    ui.on_toast_tapped(|| {
        with(|c| c.toast_tapped());
    });
    ui.on_open_note(|n| {
        with(|c| c.open_note(&n));
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
            // Entrée puis la sortie du champ, ou rien de changé : une seule fois, ou rien.
            let prefs = &c.store.prefs.device_name;
            let current = if prefs.is_empty() { platform::device_name() } else { prefs.clone() };
            if current == name.trim() {
                return;
            }
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

    /// Les bulles passent l'une après l'autre, sans doublon ; quand trop attendent, les plus anciennes cèdent leur
    /// place, sauf les erreurs ; une erreur reste plus longtemps.
    #[test]
    fn bubbles_wait_their_turn() {
        let mut b = Bubbles::default();
        assert!(b.push("Fetch terminé", 2), "rien n'est montré : tout de suite");
        assert_eq!(b.next(), Some(("Fetch terminé".into(), 2)));
        assert!(!b.push("Push refusé", 4), "attend la fin de la précédente");
        assert!(!b.push("Push refusé", 4), "une seule fois");
        assert!(!b.push("Fetch terminé", 2), "déjà montrée");
        for i in 0..4 {
            b.push(&format!("info {i}"), 1);
        }
        let waiting: Vec<&str> = b.queue.iter().map(|(t, _)| t.as_str()).collect();
        assert_eq!(waiting, ["Push refusé", "info 1", "info 2", "info 3"], "l'erreur reste");
        assert_eq!(b.next().map(|(t, _)| t), Some("Push refusé".into()));
        assert_eq!(bubble_time(4), Duration::from_secs(6));
        assert!(bubble_time(2) < bubble_time(4));
    }

    fn key(module: &str, path: &str) -> (String, String) {
        (module.into(), path.into())
    }

    /// Une action ne part qu'une fois tant que le PC n'a pas répondu (deux touchers : un seul Pull) ; la même ailleurs
    /// part ; le résultat la rend possible de nouveau, et un résultat arrivé après le délai ne fait plus rien.
    #[test]
    fn an_action_is_sent_once_until_its_result() {
        let mut p = Pendings::default();
        assert_eq!(p.send("dragontree", "depot/eyrie", "pull"), Some(1));
        assert_eq!(p.send("dragontree", "depot/eyrie", "pull"), None, "déjà en cours");
        assert_eq!(p.send("dragontree", "depot/ozmo", "pull"), Some(2), "un autre dépôt");
        assert_eq!(p.send("dragontree", "depot/eyrie", "push"), Some(3));
        let acts = p.acts(&key("dragontree", "depot/eyrie"));
        assert_eq!(acts, HashSet::from(["pull".to_string(), "push".to_string()]), "grisées dans cet écran");

        let done = p.answer(1);
        assert_eq!(done.map(|d| (d.module, d.path, d.act)), Some(key3("dragontree", "depot/eyrie", "pull")));
        assert_eq!(p.acts(&key("dragontree", "depot/eyrie")), HashSet::from(["push".to_string()]));
        assert_eq!(p.send("dragontree", "depot/eyrie", "pull"), Some(4), "de nouveau possible");
        assert_eq!(p.answer(1), None, "le délai passé, puis le résultat : une seule fois");
        assert_eq!(p.next(), 5, "les commandes ont leur numéro à elles, jamais celui d'une action en cours");
    }

    fn key3(a: &str, b: &str, c: &str) -> (String, String, String) {
        (a.into(), b.into(), c.into())
    }

    fn stack(nav: &Nav, module: &str) -> Vec<String> {
        nav.stacks.get(module).cloned().unwrap_or_default()
    }

    /// Un onglet d'écran remplace l'écran affiché dans la pile : le retour mène à ce qu'il y avait dessous ; toucher
    /// l'onglet affiché remonte en haut de sa liste.
    #[test]
    fn a_screen_tab_replaces_the_top_of_the_stack() {
        let mut nav = Nav::new();
        nav.open_module_path("dragontree", "depot/eyrie");
        assert_eq!(stack(&nav, "dragontree"), ["", "depot/eyrie"]);
        assert_eq!(nav.select_tab("depot/eyrie/historique"), Some(Moved::Screen));
        assert_eq!(stack(&nav, "dragontree"), ["", "depot/eyrie/historique"]);
        assert!(nav.is_shown(&key("dragontree", "depot/eyrie/historique")));
        assert!(!nav.is_shown(&key("dragontree", "depot/eyrie")), "une liste reçue pour l'autre onglet attend");
        assert_eq!(nav.select_tab("depot/eyrie/historique"), Some(Moved::Top));
        assert!(nav.back());
        assert_eq!((nav.tab.as_str(), nav.top_path()), ("dragontree", String::new()), "l'accueil du module");
        assert!(nav.back());
        assert_eq!(nav.tab, "ensemble");
        assert_eq!(nav.select_tab("x"), None, "pas d'onglet hors d'un module");
        assert!(!nav.back(), "rien à quitter : l'application se ferme");
    }

    /// Toucher l'onglet du bas du module affiché remonte d'abord en haut de sa liste, puis ramène à son accueil.
    #[test]
    fn retapping_the_bottom_tab_goes_up_then_home() {
        let mut nav = Nav::new();
        assert_eq!(nav.select_bottom("fawkes", false, true), Moved::Screen);
        assert!(nav.open_path("t/3"));
        nav.remember(Some(&key("fawkes", "t/3")), -400.0);
        assert_eq!(nav.select_bottom("fawkes", true, false), Moved::Top);
        assert_eq!(stack(&nav, "fawkes"), ["", "t/3"], "on reste sur l'écran");
        assert_eq!(nav.scroll_of(&key("fawkes", "t/3")), 0.0);
        assert_eq!(nav.select_bottom("fawkes", true, true), Moved::Screen);
        assert_eq!(stack(&nav, "fawkes"), [""], "en haut : l'accueil du module");
        assert_eq!(nav.select_bottom("plus", true, false), Moved::Screen, "un autre onglet");
        assert!(!nav.open_path("t/3"), "pas de sous-écran hors d'un module");
        assert!(nav.back());
        assert_eq!(nav.tab, "ensemble");
    }

    /// Chaque écran garde sa position dans la liste et son texte cherché, rendus au retour ; un écran ouvert de
    /// nouveau part du haut ; chaque onglet d'écran garde la sienne ; seule la liste affichée est retenue.
    #[test]
    fn each_screen_keeps_its_scroll_and_a_new_one_starts_at_the_top() {
        let mut nav = Nav::new();
        nav.select_bottom("fawkes", false, true);
        let home = key("fawkes", "");
        nav.remember(Some(&home), -300.0);
        nav.search.insert(home.clone(), "claude".into());
        nav.open_path("t/3");
        assert_eq!(nav.scroll_of(&key("fawkes", "t/3")), 0.0, "un écran poussé part du haut");
        nav.remember(Some(&key("fawkes", "t/3")), -50.0);
        nav.remember(Some(&home), -999.0);
        assert_eq!(nav.scroll_of(&home), -300.0, "pas affiché : rien de retenu");
        nav.back();
        assert_eq!((nav.scroll_of(&home), nav.search_of(&home).as_str()), (-300.0, "claude"));
        nav.open_path("t/3");
        assert_eq!(nav.scroll_of(&key("fawkes", "t/3")), 0.0, "ouvert de nouveau : du haut");

        nav.open_module_path("dragontree", "depot/eyrie");
        nav.remember(Some(&key("dragontree", "depot/eyrie")), -120.0);
        nav.select_tab("depot/eyrie/historique");
        nav.remember(Some(&key("dragontree", "depot/eyrie/historique")), -80.0);
        nav.select_tab("depot/eyrie");
        assert_eq!(nav.scroll_of(&key("dragontree", "depot/eyrie")), -120.0, "chaque onglet garde la sienne");
        assert_eq!(nav.scroll_of(&key("dragontree", "depot/eyrie/historique")), -80.0);
    }

    /// Un écran affiché qui disparaît sur le PC ramène à celui d'en dessous ; un autre ne change rien.
    #[test]
    fn a_vanished_screen_returns_to_the_one_below() {
        let mut nav = Nav::new();
        nav.open_module_path("fawkes", "t/3");
        assert!(!nav.gone("fawkes", "t/4"));
        assert!(!nav.gone("patronus", "t/3"));
        assert!(nav.gone("fawkes", "t/3"));
        assert_eq!(stack(&nav, "fawkes"), [""]);
        assert!(!nav.gone("fawkes", ""), "l'accueil reste");
        nav.goto("owlery/pr/12");
        assert_eq!((nav.tab.as_str(), stack(&nav, "owlery")), ("owlery", vec![String::new(), "pr/12".into()]));
    }

    /// « @module <id> [<chemin>] » dans `Row.open`, `Act.open` ou `Tab.path` : l'onglet de cet autre module, puis cet
    /// écran (son accueil sans chemin) ; la pile du module d'où l'on vient reste, et rien d'autre n'est un lien.
    #[test]
    fn a_link_opens_the_screen_of_another_module() {
        let mut nav = Nav::new();
        nav.open_module_path("grimoire", "profil/perso");
        assert!(nav.open("@module gringotts compte/perso"));
        assert_eq!(
            (nav.tab.as_str(), stack(&nav, "gringotts")),
            ("gringotts", vec![String::new(), "compte/perso".into()])
        );
        assert_eq!(stack(&nav, "grimoire"), ["", "profil/perso"], "le module d'où l'on vient garde sa pile");
        assert!(nav.back());
        assert_eq!((nav.tab.as_str(), nav.top_path()), ("gringotts", String::new()), "retour : l'accueil du module");

        nav.open_module_path("owlery", "item/3");
        assert_eq!(nav.select_tab("@module fawkes"), Some(Moved::Screen), "un onglet vers un autre module");
        assert_eq!((nav.tab.as_str(), stack(&nav, "fawkes")), ("fawkes", vec![String::new()]));
        assert!(nav.open("t/3"), "un chemin du module affiché");
        assert_eq!(stack(&nav, "fawkes"), ["", "t/3"]);
        assert!(nav.open("@module fawkes nouveau"), "le même module : l'écran s'empile");
        assert_eq!(stack(&nav, "fawkes"), ["", "t/3", "nouveau"]);
    }

    /// Le menu d'un bandeau vient de servir : son pliage du moment tient quelques secondes, prolongées au résultat de
    /// l'action, puis l'état de départ du PC revient ; un autre bandeau, ou une autre action, n'y change rien.
    #[test]
    fn a_band_keeps_its_fold_for_a_while_after_its_menu_was_used() {
        let t = Instant::now();
        let mut h = Holds::default();
        h.hold("fawkes", "", "g2", true, "veille:g2", t);
        assert_eq!(h.open("fawkes", "", "g2", t + Duration::from_secs(2)), Some(true));
        assert_eq!(h.open("fawkes", "", "g1", t), None, "un autre bandeau");
        assert_eq!(h.open("fawkes", "nouveau", "g2", t), None, "un autre écran");
        assert!(!h.answered("fawkes", "", "reveiller:g2", t + Duration::from_secs(4)), "une autre action");
        assert!(h.answered("fawkes", "", "veille:g2", t + Duration::from_secs(4)));
        assert_eq!(h.open("fawkes", "", "g2", t + HOLD_FOLD + Duration::from_secs(1)), Some(true), "prolongée");
        assert_eq!(h.open("fawkes", "", "g2", t + Duration::from_secs(4) + HOLD_FOLD), None, "puis finie");
        assert!(!h.answered("fawkes", "", "veille:g2", t + Duration::from_secs(20)), "oubliée");
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
