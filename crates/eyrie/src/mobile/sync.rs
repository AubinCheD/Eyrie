//! À chaque image : vider la boîte de réception du pont, vérifier et exécuter les demandes des téléphones, pousser ce
//! qui a changé.
//!
//! Ce qui part vers un téléphone n'est envoyé que s'il a changé depuis le dernier envoi (empreinte) :
//! - toutes les 700 ms : cartes des modules (pastilles, ligne d'état), vue d'ensemble, notifications, commandes ;
//! - toutes les 300 ms, ou aussitôt après une action : l'écran suivi ;
//! - toutes les 50 ms : le terminal suivi, en différences.
//!
//! Une action n'est exécutée que si l'appareil a le droit d'agir et si le dernier écran envoyé la proposait (même
//! module, même chemin, même identifiant) ; celle qui ouvre un terminal (`Act::terminal`) demande aussi le droit
//! « Terminaux », comme une commande de la palette qui le fait (`CommandInfo::terminal`, envoyée seule ou touchée sur
//! un écran générique). Les actions qui commencent par `@` sont celles de l'hôte (« Montrer sur le PC », « Relancer »
//! un module arrêté), comme celles des écrans génériques (`cmd:<id>` : une commande de la palette du module). Un refus
//! est noté au journal, au plus dix fois par minute et par téléphone ([`Refusals`]).
//!
//! Une action qui demande un dossier du PC (`input.folder`) garde sa saisie avec la liste blanche : le téléphone ne
//! parcourt les dossiers ([`ToPc::Browse`], lu dans un fil par [`browse`]) que pour elle, avec le droit « Dossiers du
//! PC », et le chemin choisi est revérifié avant d'appeler le module, qui reçoit le chemin résolu.

use std::collections::{BTreeMap, BTreeSet};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::net::{Shutdown, TcpStream};
use std::sync::mpsc;
use std::time::{Duration, Instant, UNIX_EPOCH};

use eframe::egui;
use serde::Serialize;
use suite_mobile::msg::{Badge, CommandCard, ModuleCard, Note, Rights, ToPc, ToPhone, Waiting};
use suite_mobile::term::{TermFrame, TermView};
use suite_mobile::ui::{self as mui, Act, FolderInput, Row, Screen};
use suite_sdk::{Cx, Module, Notification, Tone};

use super::browse;
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
/// Les refus d'un téléphone sont notés au journal au plus tant de fois par minute ([`Refusals`]).
const REFUSALS_NOTED: u32 = 10;
const REFUSALS_EVERY: Duration = Duration::from_secs(60);
/// L'action de l'écran générique qui relance un module arrêté ou indisponible.
const RESTART: &str = "@relancer";
/// Ce que dit le PC à un téléphone qui n'a pas le droit « Agir ».
const NO_ACT: &str = "Ce téléphone n'a plus le droit d'agir (réglage du PC).";

/// L'écran que suit un téléphone.
struct Followed {
    module: String,
    path: String,
    hash: u64,
    /// La liste blanche : identifiant → libellé, du dernier écran envoyé.
    allowed: BTreeMap<String, String>,
    /// Celles de ces actions qui demandent un dossier du PC, et ce qu'elles en attendent.
    folders: BTreeMap<String, FolderInput>,
    /// Celles qui ouvrent un terminal sur le PC : seulement avec le droit « Terminaux ».
    terminals: BTreeSet<String>,
    /// Les sélecteurs de dossiers ouverts sur cet écran : une ligne au journal pour chacun, pas une par lecture.
    browsed: BTreeSet<String>,
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

/// Les refus d'un téléphone (action non proposée, droit retiré, commande inconnue) notés au journal : au plus
/// [`REFUSALS_NOTED`] par minute, pour qu'un téléphone qui sonde laisse une trace sans remplir le journal (qui ne garde
/// qu'un ancien fichier).
#[derive(Default)]
struct Refusals {
    since: Option<Instant>,
    noted: u32,
    /// Refus tus depuis la dernière ligne.
    silent: u32,
}

impl Refusals {
    /// Le texte à noter pour ce refus, ou `None` s'il est tu ; la première ligne après des refus tus en dit le nombre.
    fn note(&mut self, text: String) -> Option<String> {
        if self.since.is_none_or(|at| at.elapsed() >= REFUSALS_EVERY) {
            self.since = Some(Instant::now());
            self.noted = 0;
        }
        if self.noted >= REFUSALS_NOTED {
            self.silent += 1;
            return None;
        }
        self.noted += 1;
        Some(match std::mem::take(&mut self.silent) {
            0 => text,
            n => format!("{text} (et {n} refus non notés avant)"),
        })
    }
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
    /// Les lectures de dossiers de ce téléphone.
    browse: browse::Browser,
    refusals: Refusals,
}

impl Session {
    pub fn send(&self, m: ToPhone) -> bool {
        self.sender.send(m).is_ok()
    }

    pub fn close(&self, reason: &str) {
        self.close_with(reason, false);
    }

    /// `closing` : Eyrie se ferme (le téléphone dira « Eyrie est fermée »).
    pub fn close_with(&self, reason: &str, closing: bool) {
        let _ = self.sender.send(ToPhone::Bye { reason: reason.to_string(), closing });
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

    /// Essais : les lectures de dossiers de ce téléphone.
    #[cfg(test)]
    pub(super) fn browser(&mut self) -> &mut browse::Browser {
        &mut self.browse
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

/// Une ligne d'état écrite pour la barre du PC, telle que le téléphone la montre : ↻ (l'heure où les limites de
/// Gringotts se remettent à zéro) n'est pas un émoji, la police du téléphone ne l'a pas et en faisait un carré vide.
fn status_for_phone(text: String) -> String {
    text.replace('↻', "🔄")
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
    // Une demande d'appairage sans réponse au bout du délai tombe (le fil a déjà répondu non au téléphone), comme celle
    // que le téléphone a abandonnée : sa confirmation disparaît.
    let mobile = &mut core.mobile;
    mobile.requests.retain(|r| !r.closed() && r.at.elapsed() < super::server::DECISION);
    if mobile.ticket.as_ref().is_some_and(|t| t.remaining().is_zero()) {
        mobile.cancel_ticket();
    }
    if mobile.sessions.is_empty() {
        return commands;
    }
    pump_folders(core, ctx);
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
        Inbound::PairAbandoned { device, ticket } => {
            core.mobile.store.journal(&device, "appairage", "abandonné par le téléphone");
            mirror(core, &device, "appairage abandonné");
            core.mobile.ticket_abandoned(ticket);
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
                browse: browse::Browser::default(),
                refusals: Refusals::default(),
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

/// Refuse la demande `req` d'un téléphone (`e` : ce qu'il en dit) et le note au journal (`note`), au plus
/// [`REFUSALS_NOTED`] fois par minute pour ce téléphone.
fn refuse(core: &mut Core, id: u64, req: u32, note: String, e: &str) {
    let noted = session_mut(core, id).and_then(|s| Some((s.name.clone(), s.refusals.note(note)?)));
    if let Some((name, text)) = noted {
        core.mobile.store.journal(&name, "refus", text);
    }
    reply(core, id, req, Err(e.to_string()));
}

/// Un identifiant venu du téléphone, tel qu'il est noté au journal : coupé au-delà de 80 caractères.
fn shown(raw: &str) -> String {
    match raw.char_indices().nth(80) {
        Some((at, _)) => format!("{}…", &raw[..at]),
        None => raw.to_string(),
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
        // Lister un dossier du PC : seulement pour une action de l'écran suivi qui en demande un, avec les droits
        // « Agir » et « Dossiers du PC ».
        ToPc::Browse { req, module, path, action, dir, filter, offset, hidden } => {
            let offered = s.screen.as_ref().filter(|f| f.module == module && f.path == path).and_then(|f| {
                Some((f.folders.get(&action).cloned()?, f.allowed.get(&action).cloned().unwrap_or_default()))
            });
            let refused = cannot_browse(rights);
            let why = refused.unwrap_or("Cet écran a changé entre-temps : réessayez.");
            let Some((spec, label)) = offered.filter(|_| refused.is_none()) else {
                s.send(ToPhone::Folder { req, listing: browse::refused(dir, why) });
                return;
            };
            let opened = s.screen.as_mut().is_some_and(|f| f.browsed.insert(action));
            let recents = core.mobile.store.recent_folders(&module);
            let module = module_name(core, &module);
            if opened {
                core.mobile.store.journal(&name, "dossiers", format!("{module} › {label} : sélecteur de dossiers"));
            }
            let ask = browse::Ask { req, dir, filter, offset, hidden, spec, module, recents };
            let Some(s) = session_mut(core, id) else { return };
            if let Some(old) = s.browse.ask(ask) {
                s.send(ToPhone::Folder {
                    req: old.req,
                    listing: browse::refused(old.dir, "Demande remplacée par une plus récente."),
                });
            }
            pump_folders(core, ctx);
        }
        // Les notifications que le téléphone montre, lues sur le PC aussi : sans « Agir » (le téléphone ne fait que les
        // lire), mais noté au journal quand il y en avait de non lues.
        ToPc::NotesRead => {
            let unread = core.notes.unread();
            if unread > 0 {
                let text = format!("{unread} marquées lues depuis le téléphone");
                core.mobile.store.journal(&name, "notifications", text);
            }
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
                    folders: BTreeMap::new(),
                    terminals: BTreeSet::new(),
                    browsed: BTreeSet::new(),
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
        // Chaque refus est noté au journal ([`refuse`]) : un téléphone qui sonde des actions laisse une trace.
        ToPc::Action { req, module, path, action, input } => {
            let offered = s.screen.as_ref().and_then(|f| {
                let folder = f.folders.get(&action).cloned();
                let terminal = f.terminals.contains(&action);
                (f.module == module && f.path == path)
                    .then(|| (f.allowed.get(&action).cloned(), f.generic, folder, terminal))
            });
            // L'action telle que l'écran la nommait ; sinon son identifiant, tel que le téléphone l'a envoyé.
            let asked = offered.as_ref().and_then(|o| o.0.clone()).unwrap_or_else(|| shown(&action));
            let mut text = format!("{} › {asked}", shown(&module_name(core, &module)));
            if !rights.act {
                refuse(core, id, req, format!("{text} : {NO_ACT}"), NO_ACT);
                return;
            }
            let Some((Some(label), generic, folder, terminal)) = offered else {
                let note = format!("{text} : action que l'écran suivi ne proposait pas");
                refuse(core, id, req, note, "Cet écran a changé entre-temps : réessayez.");
                return;
            };
            // Ouvrir un terminal (un shell, une session Claude) fait partie du droit « Terminaux », comme y écrire.
            if terminal && !rights.terminals {
                let e = "Ce téléphone n'a pas le droit d'ouvrir de terminal (réglage du PC).";
                refuse(core, id, req, format!("{text} : {e}"), e);
                return;
            }
            // Un dossier du PC (choisi dans le sélecteur, ou saisi sur une application plus ancienne) : revérifié ici,
            // avant le module, qui reçoit le chemin résolu ; écrit en clair au journal avec l'action, et gardé parmi les
            // récents du sélecteur si l'action réussit.
            let mut chosen = None;
            let input = match folder {
                None => input,
                Some(spec) => {
                    let env = core.mobile.folder_env();
                    match browse::chosen(&env, input.as_deref(), &spec) {
                        Ok(dir) => {
                            let dir = dir.to_string_lossy().into_owned();
                            text = format!("{text} : {dir}");
                            chosen = Some(dir.clone());
                            Some(dir)
                        }
                        Err(e) => {
                            refuse(core, id, req, format!("{text} : {e}"), &e);
                            return;
                        }
                    }
                }
            };
            let result = if action.starts_with('@') || generic {
                host_action(core, ctx, &module, &action, &label, commands)
            } else {
                with_module(core, &module, frame, ctx, |m, cx| m.mobile_action(&path, &action, input.as_deref(), cx))
                    .unwrap_or_else(|| Err(format!("{} ne tourne pas sur le PC", module_name(core, &module))))
            };
            match &result {
                Ok(_) => {
                    core.mobile.store.journal(&name, "action", &text);
                    mirror(core, &name, &text);
                    if let Some(dir) = &chosen {
                        core.mobile.store.remember_folder(&module, dir);
                    }
                }
                Err(e) => core.mobile.store.journal(&name, "action", format!("{text} : échec ({e})")),
            }
            // « @ouvrir <chemin> » : le téléphone ouvre cet écran du module (le terminal qu'il vient de créer…).
            // « @module <id> [<chemin>] » : celui d'un autre module (Owlery → Fawkes après « Corriger avec Claude »).
            let open = match &result {
                Ok(Some(m)) => m
                    .strip_prefix(suite_mobile::msg::OPEN_PREFIX)
                    .map(|p| (module.clone(), p.to_string()))
                    .or_else(|| suite_mobile::msg::module_target(m).map(|(m, p)| (m.to_string(), p.to_string()))),
                _ => None,
            };
            let result = if open.is_some() { Ok(None) } else { result };
            reply(core, id, req, result);
            if let Some((module, path)) = open
                && let Some(s) = session_mut(core, id)
            {
                s.send(ToPhone::Open { module, path });
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
            let asked = format!("{} › commande {}", shown(&module_name(core, &module)), shown(&command));
            if !rights.act {
                refuse(core, id, req, format!("{asked} : {NO_ACT}"), NO_ACT);
                return;
            }
            let found = core
                .slot(&module)
                .and_then(|s| s.module())
                .and_then(|m| m.commands().into_iter().find(|c| c.id == command))
                .map(|c| (c.title, c.terminal));
            let Some((title, terminal)) = found else {
                refuse(core, id, req, format!("{asked} : inconnue sur le PC"), "Commande inconnue sur le PC.");
                return;
            };
            let text = format!("{} › {title} (sur le PC)", module_name(core, &module));
            // « Nouveau terminal » en ouvre un : comme une action d'écran qui le fait, avec le droit « Terminaux ».
            if terminal && !rights.terminals {
                let e = "Ce téléphone n'a pas le droit d'ouvrir de terminal (réglage du PC).";
                refuse(core, id, req, format!("{text} : {e}"), e);
                return;
            }
            core.mobile.store.journal(&name, "action", &text);
            mirror(core, &name, &text);
            core.host.show(&module);
            commands.push((module, command));
            reply(core, id, req, Ok(None));
        }
    }
}

/// Les actions de l'hôte : `@pc` montre le module sur le PC ; `cmd:<id>` exécute une commande de sa palette (`label` :
/// son titre) ; `@relancer` reconstruit un module arrêté ou indisponible, comme « Relancer le module » et « Réessayer »
/// dans sa case.
fn host_action(
    core: &mut Core,
    ctx: &egui::Context,
    module: &str,
    action: &str,
    label: &str,
    commands: &mut Vec<(String, String)>,
) -> Result<Option<String>, String> {
    if action == "@pc" {
        core.host.show(module);
        return Ok(Some("Montré sur le PC".into()));
    }
    if let Some(command) = action.strip_prefix("cmd:") {
        core.host.show(module);
        commands.push((module.to_string(), command.to_string()));
        return Ok(Some(format!("« {label} » lancée sur le PC")));
    }
    if action == RESTART {
        return restart(core, ctx, module);
    }
    Err("action inconnue".into())
}

/// Relance un module arrêté ou indisponible par le chemin de l'hôte ([`Core::start_module`]), et dit ce qu'il en est :
/// un module qui tourne n'est pas reconstruit, un module désactivé ne s'active pas depuis le téléphone.
fn restart(core: &mut Core, ctx: &egui::Context, module: &str) -> Result<Option<String>, String> {
    let name = module_name(core, module);
    match core.slot(module).map(|s| &s.state) {
        Some(SlotState::Unavailable(_) | SlotState::Crashed(_)) => {}
        Some(SlotState::Running(_)) => return Ok(Some(format!("{name} tourne déjà sur le PC"))),
        Some(SlotState::Disabled) => return Err(format!("{name} est désactivé dans les réglages d'Eyrie")),
        None => return Err(format!("{name} n'existe pas sur le PC")),
    }
    core.start_module(ctx, module);
    match core.slot(module).map(|s| &s.state) {
        Some(SlotState::Running(_)) => Ok(Some(format!("{name} relancé"))),
        Some(SlotState::Unavailable(u)) => {
            Err(format!("{name} n'est toujours pas disponible : {}", crate::app::unavailable_text(u)))
        }
        Some(SlotState::Crashed(msg)) => Err(format!("{name} s'est de nouveau arrêté : {msg}")),
        _ => Err(format!("{name} n'a pas redémarré")),
    }
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
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    (m.status(), m.commands(), m.mobile_waiting(), m.mobile_status())
                }))
                .ok()
            });
            let (status, module_commands, waiting, phone_line) = status.unwrap_or_default();
            let reason = match &slot.state {
                SlotState::Running(_) => String::new(),
                SlotState::Disabled => "Désactivé sur le PC".into(),
                SlotState::Unavailable(u) => crate::app::unavailable_text(u),
                SlotState::Crashed(msg) => format!("Arrêté après une erreur : {msg}"),
            };
            if waiting.is_empty() {
                for w in &status.waiting {
                    overview.push(Waiting { module: slot.info.id.into(), text: w.clone(), path: None });
                }
            }
            for (text, path) in waiting {
                overview.push(Waiting { module: slot.info.id.into(), text, path });
            }
            cmds.extend(module_commands.into_iter().map(|c| CommandCard {
                module: slot.info.id.into(),
                id: c.id,
                title: c.title,
                terminal: c.terminal,
            }));
            cards.push(ModuleCard {
                id: slot.info.id.into(),
                name: slot.info.name.into(),
                glyph: slot.info.icon.into(),
                color: rgb(slot.info.color),
                running: slot.is_running(),
                reason,
                badge: status.badge.map(|b| Badge { count: b.count, tone: tone(b.tone), tip: b.tip }),
                // La ligne écrite pour la carte du téléphone, sinon celle de la barre du PC.
                status: phone_line.or(status.status_line.map(|l| l.text)).map(status_for_phone).unwrap_or_default(),
                summary: slot.info.summary.into(),
            });
        }
        let notes: Vec<Note> = core
            .notes
            .items
            .iter()
            .take(60)
            .map(|(n, read)| {
                // « @module <id> [<chemin>] » : elle mène à l'écran d'un autre module (la suite, à celui d'un module
                // arrêté) ; le téléphone ouvre l'écran du module de la note.
                let (module, path) = match n.path.as_deref().and_then(suite_mobile::msg::module_target) {
                    Some((m, p)) => (m.to_string(), Some(p.to_string())),
                    None => (n.module.clone(), n.path.clone()),
                };
                Note {
                    module,
                    title: n.title.clone(),
                    body: n.body.clone(),
                    tone: tone(n.tone),
                    at: n.at.duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0),
                    read: *read,
                    path,
                }
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
                s.note(status_for_phone(text), tone(line.tone));
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
                    let mut act = Act::new(format!("cmd:{}", c.id), c.title.clone()).glyph("▶️");
                    act.terminal = c.terminal;
                    s.row(Row::new(c.title).subtitle("S'exécute dans Eyrie, sur le PC").tap(act));
                }
            }
        }
        SlotState::Disabled => {
            s.note("Ce module est désactivé dans les réglages d'Eyrie.", mui::Tone::Info);
        }
        // Les boutons de sa case sur le PC : un module indisponible (son exe lancé seul…) se réessaie, un module arrêté
        // se relance.
        SlotState::Unavailable(u) => {
            s.note(crate::app::unavailable_text(u), mui::Tone::Warn);
            s.buttons(vec![Act::new(RESTART, "Réessayer").glyph("🔄")]);
        }
        SlotState::Crashed(msg) => {
            s.note(format!("Ce module s'est arrêté après une erreur : {msg}"), mui::Tone::Danger);
            s.buttons(vec![Act::new(RESTART, "Relancer le module").glyph("🔄").primary()]);
        }
    }
    Some(s)
}

/// Les actions d'un écran qui demandent un dossier du PC, et ce qu'elles en attendent (la première saisie trouvée,
/// pour un identifiant proposé plusieurs fois).
fn folders_of(screen: &Screen) -> BTreeMap<String, FolderInput> {
    let mut out = BTreeMap::new();
    for a in screen.all_acts().into_iter().filter(|a| a.reaches_pc()) {
        if let Some(spec) = a.input.as_ref().and_then(|i| i.folder.as_ref()) {
            out.entry(a.id.clone()).or_insert_with(|| spec.clone());
        }
    }
    out
}

/// Les actions d'un écran qui ouvrent un terminal sur le PC (`Act::terminal`) : un identifiant proposé plusieurs fois
/// en est dès qu'une de ses actions l'est.
fn terminals_of(screen: &Screen) -> BTreeSet<String> {
    screen.all_acts().into_iter().filter(|a| a.reaches_pc() && a.terminal).map(|a| a.id.clone()).collect()
}

/// Pourquoi ce téléphone ne parcourt pas les dossiers du PC (droits « Agir » et « Dossiers du PC ») ; `None` : il le
/// peut.
fn cannot_browse(rights: Rights) -> Option<&'static str> {
    if !rights.act {
        Some(NO_ACT)
    } else if !rights.files {
        Some("Ce téléphone n'a pas le droit de parcourir les dossiers du PC (réglage du PC).")
    } else {
        None
    }
}

/// Lance, dans leur fil, les lectures de dossiers dont c'est le tour ; la réponse part de là au téléphone. Celle qui
/// attendait son tour quand le droit de parcourir a été retiré n'est pas lue : le téléphone reçoit le refus.
fn pump_folders(core: &mut Core, ctx: &egui::Context) {
    if !core.mobile.sessions.iter().any(|s| s.browse.waiting()) {
        return;
    }
    let env = core.mobile.folder_env();
    for s in &mut core.mobile.sessions {
        if let Some(why) = cannot_browse(s.rights) {
            if let Some(old) = s.browse.cancel() {
                s.send(ToPhone::Folder { req: old.req, listing: browse::refused(old.dir, why) });
            }
            continue;
        }
        let wake = ctx.clone();
        if let Some(wait) = s.browse.pump(&env, &s.sender, move || wake.request_repaint()) {
            ctx.request_repaint_after(wait);
        }
    }
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
                    f.folders = folders_of(&screen);
                    f.terminals = terminals_of(&screen);
                    s.send(ToPhone::Screen { module, path, screen });
                }
            }
            None => {
                if f.hash != 1 {
                    f.hash = 1;
                    f.allowed.clear();
                    f.folders.clear();
                    f.terminals.clear();
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Dix refus notés par minute, les suivants tus ; la première ligne de la minute suivante dit combien l'ont été.
    #[test]
    fn refusals_are_noted_ten_a_minute() {
        let mut r = Refusals::default();
        let noted: Vec<Option<String>> = (0..12).map(|i| r.note(format!("refus {i}"))).collect();
        assert!(noted[..10].iter().all(Option::is_some) && noted[10..].iter().all(Option::is_none), "{noted:?}");
        r.since = Instant::now().checked_sub(REFUSALS_EVERY);
        assert_eq!(r.note("refus 12".into()).as_deref(), Some("refus 12 (et 2 refus non notés avant)"));
        assert_eq!(r.note("refus 13".into()).as_deref(), Some("refus 13"));
    }

    /// Un identifiant envoyé par le téléphone est coupé au journal, sans casser un caractère.
    #[test]
    fn a_long_identifier_is_cut_in_the_journal() {
        assert_eq!(shown("sonde-1"), "sonde-1");
        let cut = shown(&"é".repeat(100));
        assert_eq!((cut.chars().count(), cut.ends_with('…')), (81, true));
    }
}
