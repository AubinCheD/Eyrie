//! À chaque image : vider la boîte de réception du pont, vérifier et exécuter les demandes des téléphones, pousser ce
//! qui a changé.
//!
//! Ce qui part vers un téléphone n'est envoyé que s'il a changé depuis le dernier envoi (empreinte) :
//! - toutes les 700 ms : cartes des modules (pastilles, ligne d'état), vue d'ensemble, notifications, commandes ;
//! - toutes les 300 ms, ou aussitôt après une action : l'écran suivi ;
//! - toutes les 50 ms : le terminal suivi, en différences.
//!
//! Une action n'est exécutée que si l'appareil a le droit d'agir et si le dernier écran envoyé la proposait (même
//! module, même chemin, même identifiant). Les actions qui commencent par `@` sont celles de l'hôte (« Montrer sur le
//! PC »), comme celles des écrans génériques (`cmd:<id>` : une commande de la palette du module).

use std::collections::BTreeMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::net::{Shutdown, TcpStream};
use std::sync::mpsc;
use std::time::{Duration, Instant, UNIX_EPOCH};

use eframe::egui;
use serde::Serialize;
use suite_mobile::msg::{Badge, CommandCard, ModuleCard, Note, Rights, ToPc, ToPhone, Waiting};
use suite_mobile::term::{TermFrame, TermView};
use suite_mobile::ui::{self as mui, Act, Row, Screen};
use suite_sdk::{Cx, Module, Notification, Tone};

use super::server::{Identity, Inbound};
use crate::app::Core;
use crate::core::{self, CallKind, SlotState};
use crate::sandbox::Policy;

const LIGHT_EVERY: Duration = Duration::from_millis(700);
const SCREEN_EVERY: Duration = Duration::from_millis(300);
const TERM_EVERY: Duration = Duration::from_millis(50);
/// Lignes d'historique envoyées en plus de l'écran d'un terminal.
pub const TERM_HISTORY: usize = 500;
/// Une saisie de terminal est notée au journal au plus une fois par minute et par terminal.
const TERM_JOURNAL_EVERY: Duration = Duration::from_secs(60);

/// L'écran que suit un téléphone.
struct Followed {
    module: String,
    path: String,
    hash: u64,
    /// La liste blanche : identifiant → libellé, du dernier écran envoyé.
    allowed: BTreeMap<String, String>,
    /// Écran fait par l'hôte (module sans écran mobile, ou arrêté) : ses actions sont celles de l'hôte.
    generic: bool,
    due: Instant,
}

struct FollowedTerm {
    module: String,
    id: String,
    view: Option<TermView>,
    due: Instant,
    journaled: Option<Instant>,
}

/// Un téléphone connecté.
pub struct Session {
    pub id: u64,
    pub device: String,
    pub name: String,
    pub model: String,
    pub identity: Identity,
    pub since: Instant,
    pub rights: Rights,
    sender: mpsc::Sender<ToPhone>,
    stream: TcpStream,
    sent: [u64; 4],
    logos: bool,
    screen: Option<Followed>,
    term: Option<FollowedTerm>,
}

impl Session {
    pub fn send(&self, m: ToPhone) -> bool {
        self.sender.send(m).is_ok()
    }

    pub fn close(&self, reason: &str) {
        let _ = self.sender.send(ToPhone::Bye { reason: reason.to_string() });
        // Laisse le fil d'envoi écrire l'adieu, puis coupe : le fil de lecture s'arrête.
        let stream = self.stream.try_clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            if let Ok(s) = stream {
                let _ = s.shutdown(Shutdown::Both);
            }
        });
    }

    /// Ce que suit le téléphone, pour la page Téléphones.
    pub fn watching(&self) -> Option<(&str, &str)> {
        if let Some(t) = &self.term {
            return Some((&t.module, &t.id));
        }
        self.screen.as_ref().map(|s| (s.module.as_str(), s.path.as_str()))
    }
}

/// Ce qui est commun à tous les téléphones, recalculé toutes les 700 ms.
#[derive(Default)]
pub struct State {
    at: Option<Instant>,
    cards: Vec<ModuleCard>,
    overview: Vec<Waiting>,
    notes: (Vec<Note>, u32),
    commands: Vec<CommandCard>,
    hashes: [u64; 4],
}

fn digest<T: Serialize>(v: &T) -> u64 {
    let mut h = DefaultHasher::new();
    rmp_serde::to_vec(v).unwrap_or_default().hash(&mut h);
    h.finish()
}

fn tone(t: Tone) -> mui::Tone {
    match t {
        Tone::Info => mui::Tone::Info,
        Tone::Ok => mui::Tone::Ok,
        Tone::Warn => mui::Tone::Warn,
        Tone::Danger => mui::Tone::Danger,
    }
}

fn rgb(c: egui::Color32) -> u32 {
    ((c.r() as u32) << 16) | ((c.g() as u32) << 8) | c.b() as u32
}

/// Appelle un module dans le bac à sable de l'hôte ; `None` s'il ne tourne pas (ou vient de planter).
fn with_module<R>(
    core: &mut Core,
    id: &str,
    frame: &mut eframe::Frame,
    ctx: &egui::Context,
    f: impl FnOnce(&mut dyn Module, &mut Cx) -> R,
) -> Option<R> {
    let Core { slots, host, journal, wish, shown_prev, .. } = core;
    let slot = slots.iter_mut().find(|s| s.info.id == id)?;
    let policy = Policy { keyboard: false, visible: shown_prev.contains(id), sees_close: false };
    let mut out = None;
    core::call(slot, host, frame, ctx, CallKind::Other, policy, wish, journal, |m, cx| out = Some(f(m, cx)));
    out
}

/// À chaque image, depuis la logique de l'hôte. Renvoie les commandes de module à exécuter (palette du PC).
pub fn poll(core: &mut Core, frame: &mut eframe::Frame, ctx: &egui::Context) -> Vec<(String, String)> {
    core.mobile.ensure(core.st.mobile.enabled);
    let mut commands = Vec::new();
    let inbound = core.mobile.server.as_ref().map(|s| s.shared().take_inbox()).unwrap_or_default();
    for i in inbound {
        receive(core, i, frame, ctx, &mut commands);
    }
    // Une demande d'appairage sans réponse au bout du délai tombe (le fil a déjà répondu non au téléphone).
    let mobile = &mut core.mobile;
    mobile.requests.retain(|r| r.at.elapsed() < super::server::DECISION);
    if mobile.ticket.as_ref().is_some_and(|t| t.remaining().is_zero()) {
        mobile.cancel_ticket();
    }
    if mobile.sessions.is_empty() {
        return commands;
    }
    push_light(core);
    push_screens(core, frame, ctx);
    push_terms(core, frame, ctx);
    let wake = if core.mobile.sessions.iter().any(|s| s.term.is_some()) { TERM_EVERY } else { SCREEN_EVERY };
    ctx.request_repaint_after(wake);
    commands
}

fn receive(
    core: &mut Core,
    i: Inbound,
    frame: &mut eframe::Frame,
    ctx: &egui::Context,
    commands: &mut Vec<(String, String)>,
) {
    match i {
        Inbound::Log { device, kind, text } => core.mobile.store.journal(&device, kind, text),
        Inbound::Pair(r) => {
            core.mobile.store.journal(&r.name, "appairage", format!("demandé depuis {}", r.identity.node));
            ctx.send_viewport_cmd(egui::ViewportCommand::RequestUserAttention(egui::UserAttentionType::Critical));
            core.mobile.requests.push(r);
        }
        Inbound::Connected { session, device, model, app, identity, sender, stream } => {
            let Some(d) = core.mobile.store.device_mut(&device) else {
                let _ = stream.shutdown(Shutdown::Both);
                return;
            };
            d.last_seen = super::store::now();
            let (name, rights) = (d.name.clone(), d.rights);
            core.mobile.store.save();
            // Un téléphone qui revient remplace sa connexion précédente.
            core.mobile.sessions.retain(|s| {
                let keep = s.device != device;
                if !keep {
                    s.close("Nouvelle connexion de ce téléphone.");
                }
                keep
            });
            let via = if identity.login.is_empty() {
                identity.node.clone()
            } else {
                format!("{} ({})", identity.node, identity.login)
            };
            core.mobile.store.journal(&name, "connexion", format!("depuis {via}, application {app}"));
            mirror(core, &name, &format!("connecté ({model})"));
            core.mobile.sessions.push(Session {
                id: session,
                device,
                name,
                model,
                identity,
                since: Instant::now(),
                rights,
                sender,
                stream,
                sent: [0; 4],
                logos: false,
                screen: None,
                term: None,
            });
            // Tout part tout de suite.
            core.mobile.sync.at = None;
        }
        Inbound::Disconnected { session, reason } => {
            if let Some(i) = core.mobile.sessions.iter().position(|s| s.id == session) {
                let s = core.mobile.sessions.remove(i);
                core.mobile.store.journal(&s.name, "deconnexion", reason);
            }
        }
        Inbound::Message { session, msg } => {
            if core.mobile.sessions.iter().any(|s| s.id == session) {
                message(core, session, msg, frame, ctx, commands);
            }
        }
    }
}

fn session_mut(core: &mut Core, id: u64) -> Option<&mut Session> {
    core.mobile.sessions.iter_mut().find(|s| s.id == id)
}

fn reply(core: &mut Core, id: u64, req: u32, r: Result<Option<String>, String>) {
    let (ok, message) = match r {
        Ok(m) => (true, m.unwrap_or_default()),
        Err(e) => (false, e),
    };
    if let Some(s) = session_mut(core, id) {
        s.send(ToPhone::Result { req, ok, message });
    }
}

/// Une action venue d'un téléphone se voit sur le PC : une bulle discrète, si le réglage le veut.
fn mirror(core: &mut Core, device: &str, text: &str) {
    if core.st.mobile.show_actions {
        core.notes.toasts.push((Notification::new("suite", format!("📱 {device}"), text, Tone::Info), Instant::now()));
    }
}

fn module_name(core: &Core, id: &str) -> String {
    core.slot(id).map(|s| s.info.name.to_string()).unwrap_or_else(|| id.to_string())
}

fn message(
    core: &mut Core,
    id: u64,
    msg: ToPc,
    frame: &mut eframe::Frame,
    ctx: &egui::Context,
    commands: &mut Vec<(String, String)>,
) {
    let Some(s) = session_mut(core, id) else { return };
    let (name, rights) = (s.name.clone(), s.rights);
    match msg {
        ToPc::Ping => {
            s.send(ToPhone::Pong);
        }
        ToPc::Hello { .. } | ToPc::Pair { .. } => {}
        ToPc::NotesRead => {
            core.notes.mark_all_read();
            core.mobile.sync.at = None;
        }
        ToPc::Follow { screen, terminal } => {
            let now = Instant::now();
            let same_screen = s.screen.as_ref().map(|f| (f.module.clone(), f.path.clone())) == screen;
            if !same_screen {
                s.screen = screen.map(|(module, path)| Followed {
                    module,
                    path,
                    hash: 0,
                    allowed: BTreeMap::new(),
                    generic: false,
                    due: now,
                });
            }
            let same_term = s.term.as_ref().map(|t| (t.module.clone(), t.id.clone())) == terminal;
            if !same_term {
                s.term =
                    terminal.map(|(module, id)| FollowedTerm { module, id, view: None, due: now, journaled: None });
            }
        }
        ToPc::Action { req, module, path, action, input } => {
            if !rights.act {
                reply(core, id, req, Err("Ce téléphone n'a plus le droit d'agir (réglage du PC).".into()));
                return;
            }
            let offered = s.screen.as_ref().and_then(|f| {
                (f.module == module && f.path == path).then(|| (f.allowed.get(&action).cloned(), f.generic))
            });
            let Some((Some(label), generic)) = offered else {
                reply(core, id, req, Err("Cet écran a changé entre-temps : réessayez.".into()));
                return;
            };
            let result = if action.starts_with('@') || generic {
                host_action(core, &module, &action, commands)
            } else {
                with_module(core, &module, frame, ctx, |m, cx| m.mobile_action(&path, &action, input.as_deref(), cx))
                    .unwrap_or_else(|| Err(format!("{} ne tourne pas sur le PC", module_name(core, &module))))
            };
            let text = format!("{} › {label}", module_name(core, &module));
            match &result {
                Ok(_) => {
                    core.mobile.store.journal(&name, "action", &text);
                    mirror(core, &name, &text);
                }
                Err(e) => core.mobile.store.journal(&name, "action", format!("{text} : échec ({e})")),
            }
            // « @ouvrir <chemin> » : le téléphone ouvre cet écran du module (le terminal qu'il vient de créer…).
            let open = match &result {
                Ok(Some(m)) => m.strip_prefix(suite_mobile::msg::OPEN_PREFIX).map(str::to_string),
                _ => None,
            };
            let result = if open.is_some() { Ok(None) } else { result };
            reply(core, id, req, result);
            if let Some(path) = open
                && let Some(s) = session_mut(core, id)
            {
                s.send(ToPhone::Open { module: module.clone(), path });
            }
            if let Some(f) = session_mut(core, id).and_then(|s| s.screen.as_mut()) {
                f.due = Instant::now();
            }
            core.mobile.sync.at = None;
        }
        ToPc::Terminal { module, id: term, input } => {
            if !rights.terminals {
                s.send(ToPhone::Toast {
                    text: "Ce téléphone n'a pas le droit d'écrire dans les terminaux.".into(),
                    tone: mui::Tone::Warn,
                });
                return;
            }
            let follows = s.term.as_ref().is_some_and(|t| t.module == module && t.id == term);
            if !follows {
                return;
            }
            let r = with_module(core, &module, frame, ctx, |m, cx| m.mobile_term_input(&term, &input, cx));
            let Some(s) = session_mut(core, id) else { return };
            match r {
                Some(Ok(())) => {
                    if let Some(t) = s.term.as_mut() {
                        t.due = Instant::now();
                        let note = t.journaled.is_none_or(|at| at.elapsed() > TERM_JOURNAL_EVERY);
                        if note {
                            t.journaled = Some(Instant::now());
                            let text = format!("saisie dans le terminal {term} ({})", module_name(core, &module));
                            core.mobile.store.journal(&name, "terminal", text);
                        }
                    }
                }
                Some(Err(e)) => {
                    s.send(ToPhone::Toast { text: e, tone: mui::Tone::Warn });
                }
                None => {
                    s.send(ToPhone::Toast {
                        text: "Le module ne tourne plus sur le PC.".into(),
                        tone: mui::Tone::Warn,
                    });
                }
            }
        }
        ToPc::Command { req, module, id: command } => {
            if !rights.act {
                reply(core, id, req, Err("Ce téléphone n'a plus le droit d'agir (réglage du PC).".into()));
                return;
            }
            let title = core
                .slot(&module)
                .and_then(|s| s.module())
                .and_then(|m| m.commands().into_iter().find(|c| c.id == command))
                .map(|c| c.title);
            let Some(title) = title else {
                reply(core, id, req, Err("Commande inconnue sur le PC.".into()));
                return;
            };
            let text = format!("{} › {title} (sur le PC)", module_name(core, &module));
            core.mobile.store.journal(&name, "action", &text);
            mirror(core, &name, &text);
            core.host.show(&module);
            commands.push((module, command));
            reply(core, id, req, Ok(None));
        }
    }
}

/// Les actions de l'hôte : `@pc` montre le module sur le PC ; `cmd:<id>` exécute une commande de sa palette.
fn host_action(
    core: &mut Core,
    module: &str,
    action: &str,
    commands: &mut Vec<(String, String)>,
) -> Result<Option<String>, String> {
    if action == "@pc" {
        core.host.show(module);
        return Ok(Some("Montré sur le PC".into()));
    }
    if let Some(command) = action.strip_prefix("cmd:") {
        core.host.show(module);
        commands.push((module.to_string(), command.to_string()));
        return Ok(None);
    }
    Err("action inconnue".into())
}

/// Cartes, vue d'ensemble, notifications, commandes.
fn push_light(core: &mut Core) {
    let due = core.mobile.sync.at.is_none_or(|at| at.elapsed() >= LIGHT_EVERY);
    if due {
        let mut cards = Vec::new();
        let mut overview = Vec::new();
        let mut cmds = Vec::new();
        for slot in &core.slots {
            let status = slot.module().and_then(|m| {
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (m.status(), m.commands()))).ok()
            });
            let (status, module_commands) = status.unwrap_or_default();
            let reason = match &slot.state {
                SlotState::Running(_) => String::new(),
                SlotState::Disabled => "Désactivé sur le PC".into(),
                SlotState::Unavailable(u) => crate::app::unavailable_text(u),
                SlotState::Crashed(msg) => format!("Arrêté après une erreur : {msg}"),
            };
            for w in &status.waiting {
                overview.push(Waiting { module: slot.info.id.into(), text: w.clone(), path: None });
            }
            cmds.extend(module_commands.into_iter().map(|c| CommandCard {
                module: slot.info.id.into(),
                id: c.id,
                title: c.title,
            }));
            cards.push(ModuleCard {
                id: slot.info.id.into(),
                name: slot.info.name.into(),
                glyph: slot.info.icon.into(),
                color: rgb(slot.info.color),
                running: slot.is_running(),
                reason,
                badge: status.badge.map(|b| Badge { count: b.count, tone: tone(b.tone), tip: b.tip }),
                status: status.status_line.map(|l| l.text).unwrap_or_default(),
                summary: slot.info.summary.into(),
            });
        }
        let notes: Vec<Note> = core
            .notes
            .items
            .iter()
            .take(60)
            .map(|(n, read)| Note {
                module: n.module.clone(),
                title: n.title.clone(),
                body: n.body.clone(),
                tone: tone(n.tone),
                at: n.at.duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0),
                read: *read,
            })
            .collect();
        let unread = core.notes.unread() as u32;
        let st = &mut core.mobile.sync;
        st.hashes = [digest(&cards), digest(&overview), digest(&(&notes, unread)), digest(&cmds)];
        st.cards = cards;
        st.overview = overview;
        st.notes = (notes, unread);
        st.commands = cmds;
        st.at = Some(Instant::now());
    }
    let logos: Vec<(String, Vec<u8>)> = if core.mobile.sessions.iter().any(|s| !s.logos) {
        core.slots.iter().filter_map(|s| Some((s.info.id.to_string(), logo_png(s.info.logo?)?))).collect()
    } else {
        Vec::new()
    };
    let st = &core.mobile.sync;
    for s in &mut core.mobile.sessions {
        if !s.logos {
            s.logos = true;
            s.send(ToPhone::Logos { logos: logos.clone() });
        }
        if s.sent[0] != st.hashes[0] {
            s.sent[0] = st.hashes[0];
            s.send(ToPhone::Modules { modules: st.cards.clone() });
        }
        if s.sent[1] != st.hashes[1] {
            s.sent[1] = st.hashes[1];
            s.send(ToPhone::Overview { waiting: st.overview.clone() });
        }
        if s.sent[2] != st.hashes[2] {
            s.sent[2] = st.hashes[2];
            s.send(ToPhone::Notes { notes: st.notes.0.clone(), unread: st.notes.1 });
        }
        if s.sent[3] != st.hashes[3] {
            s.sent[3] = st.hashes[3];
            s.send(ToPhone::Commands { commands: st.commands.clone() });
        }
    }
}

/// Le logo d'un module en PNG de 96 px.
fn logo_png(render: fn(u32) -> Vec<u8>) -> Option<Vec<u8>> {
    const SIZE: u32 = 96;
    let img = image::RgbaImage::from_raw(SIZE, SIZE, render(SIZE))?;
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png).ok()?;
    Some(out.into_inner())
}

/// L'écran générique d'un module (sans écran mobile, désactivé, indisponible ou arrêté).
fn generic_screen(core: &Core, module: &str) -> Option<Screen> {
    let slot = core.slot(module)?;
    let mut s = Screen::new(slot.info.name).subtitle(slot.info.summary);
    match &slot.state {
        SlotState::Running(m) => {
            let st = m.status();
            if let Some(t) = st.title.filter(|t| !t.is_empty()) {
                s.subtitle = t;
            }
            if let Some(line) = st.status_line {
                let text = if line.tip.is_empty() { line.text } else { format!("{}\n{}", line.text, line.tip) };
                s.note(text, tone(line.tone));
            }
            if !st.waiting.is_empty() {
                s.section("Ce qui attend", st.waiting.len().to_string());
                for w in st.waiting {
                    s.row(Row::new(w).glyph("•").tone(mui::Tone::Warn));
                }
            }
            let cmds = m.commands();
            if !cmds.is_empty() {
                s.section("Sur le PC", "");
                for c in cmds {
                    let act = Act::new(format!("cmd:{}", c.id), c.title.clone()).glyph("▶️");
                    s.row(Row::new(c.title).subtitle("S'exécute dans Eyrie, sur le PC").tap(act));
                }
            }
        }
        SlotState::Disabled => {
            s.note("Ce module est désactivé dans les réglages d'Eyrie.", mui::Tone::Info);
        }
        SlotState::Unavailable(u) => {
            s.note(crate::app::unavailable_text(u), mui::Tone::Warn);
        }
        SlotState::Crashed(msg) => {
            s.note(format!("Ce module s'est arrêté après une erreur : {msg}"), mui::Tone::Danger);
        }
    }
    Some(s)
}

fn push_screens(core: &mut Core, frame: &mut eframe::Frame, ctx: &egui::Context) {
    let due: Vec<(u64, String, String)> = core
        .mobile
        .sessions
        .iter()
        .filter_map(|s| {
            let f = s.screen.as_ref()?;
            (f.due <= Instant::now()).then(|| (s.id, f.module.clone(), f.path.clone()))
        })
        .collect();
    // Un même écran demandé par plusieurs téléphones n'est calculé qu'une fois par image.
    let mut cache: BTreeMap<(String, String), Option<(Screen, bool)>> = BTreeMap::new();
    for (id, module, path) in due {
        let key = (module.clone(), path.clone());
        if !cache.contains_key(&key) {
            let running = core.slot(&module).is_some_and(|s| s.is_running());
            let own = if running {
                with_module(core, &module, frame, ctx, |m, cx| m.mobile_screen(&path, cx)).flatten()
            } else {
                None
            };
            let built = match own {
                Some(mut screen) => {
                    if path.is_empty() {
                        screen.acts.push(Act::new("@pc", "Montrer sur le PC").glyph("🖥️"));
                    }
                    Some((screen, false))
                }
                None if path.is_empty() || !running => generic_screen(core, &module).map(|s| (s, true)),
                None => None,
            };
            cache.insert(key.clone(), built);
        }
        let built = cache.get(&key).cloned().flatten();
        let Some(s) = session_mut(core, id) else { continue };
        let Some(f) = s.screen.as_mut() else { continue };
        f.due = Instant::now() + SCREEN_EVERY;
        match built {
            Some((screen, generic)) => {
                let h = digest(&screen);
                if h != f.hash {
                    f.hash = h;
                    f.generic = generic;
                    f.allowed = screen
                        .all_acts()
                        .into_iter()
                        .filter(|a| a.reaches_pc())
                        .map(|a| (a.id.clone(), a.label.clone()))
                        .collect();
                    s.send(ToPhone::Screen { module, path, screen });
                }
            }
            None => {
                if f.hash != 1 {
                    f.hash = 1;
                    f.allowed.clear();
                    s.send(ToPhone::NoScreen { module, path, reason: "Cet écran n'existe plus sur le PC.".into() });
                }
            }
        }
    }
}

fn push_terms(core: &mut Core, frame: &mut eframe::Frame, ctx: &egui::Context) {
    let due: Vec<(u64, String, String)> = core
        .mobile
        .sessions
        .iter()
        .filter_map(|s| {
            let t = s.term.as_ref()?;
            (t.due <= Instant::now()).then(|| (s.id, t.module.clone(), t.id.clone()))
        })
        .collect();
    let mut cache: BTreeMap<(String, String), Option<TermView>> = BTreeMap::new();
    for (id, module, term) in due {
        let key = (module.clone(), term.clone());
        if !cache.contains_key(&key) {
            let view = with_module(core, &module, frame, ctx, |m, _| m.mobile_term(&term, TERM_HISTORY)).flatten();
            cache.insert(key.clone(), view);
        }
        let view = cache.get(&key).cloned().flatten();
        let Some(s) = session_mut(core, id) else { continue };
        let Some(t) = s.term.as_mut() else { continue };
        t.due = Instant::now() + TERM_EVERY;
        let view = view.unwrap_or_else(|| TermView { ended: true, ..t.view.clone().unwrap_or_default() });
        let frame_out = match &t.view {
            None => Some(TermFrame::full(&module, &term, &view)),
            Some(old) => {
                let f = TermFrame::diff(&module, &term, old, &view);
                (!f.is_empty_against(old)).then_some(f)
            }
        };
        if let Some(f) = frame_out {
            s.sender.send(ToPhone::Term(f)).ok();
            t.view = Some(view);
        }
    }
}
