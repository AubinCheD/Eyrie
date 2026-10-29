//! L'écoute du pont, dans ses propres fils : le port local (127.0.0.1) où le compagnon Tailscale relaie les
//! téléphones, l'en-tête du compagnon (son jeton, et qui se connecte selon Tailscale), la poignée de main Noise,
//! l'appairage, puis la lecture et l'écriture des messages. Rien ici ne touche aux modules : tout ce qui arrive est
//! mis dans la boîte de réception, que l'hôte vide à chaque image (`mobile::poll`).

use std::collections::VecDeque;
use std::io::{ErrorKind, Read};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use suite_mobile::channel::{self, Keypair};
use suite_mobile::msg::{PROTOCOL, Rights, ToPc, ToPhone, WakeKey};
use suite_mobile::pairing::{self, Role};
use suite_mobile::veille;

/// Qui se connecte, selon Tailscale (l'en-tête du compagnon).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Identity {
    pub login: String,
    pub node: String,
    pub addr: String,
}

/// Ce qu'un fil de session sait d'un appareil appairé.
#[derive(Clone, Debug, PartialEq)]
pub struct DeviceAuth {
    pub id: String,
    pub name: String,
    pub key: [u8; 32],
    pub login: String,
    pub rights: Rights,
}

struct Ticket {
    group: u64,
    secret: Vec<u8>,
    expires: Instant,
    attempts: u32,
}

/// Une demande d'appairage, en attente du clic sur le PC.
pub struct PairRequest {
    pub name: String,
    pub model: String,
    pub key: [u8; 32],
    pub identity: Identity,
    pub sas: String,
    pub at: Instant,
    reply: mpsc::Sender<bool>,
    /// [`WAITING`], [`ACCEPTED`] ou [`CLOSED`] : le premier qui la tranche, du PC ou du fil, l'emporte.
    state: Arc<AtomicU8>,
}

/// La demande attend le clic sur le PC.
const WAITING: u8 = 0;
/// Acceptée sur le PC.
const ACCEPTED: u8 = 1;
/// Close par son fil : le téléphone est parti, ou le délai est passé.
const CLOSED: u8 = 2;

impl PairRequest {
    pub fn answer(&self, yes: bool) {
        let _ = self.reply.send(yes);
    }

    /// Retient l'accord du PC, si la demande attend encore ; `false` si son fil l'a close (téléphone parti, délai
    /// passé) : rien n'est alors à appairer.
    pub fn accept(&self) -> bool {
        self.state.compare_exchange(WAITING, ACCEPTED, Ordering::AcqRel, Ordering::Acquire).is_ok()
    }

    /// Le téléphone ne l'attend plus (parti, ou délai passé).
    pub fn closed(&self) -> bool {
        self.state.load(Ordering::Acquire) == CLOSED
    }
}

/// Ce que les fils remettent à l'hôte.
pub enum Inbound {
    Connected {
        session: u64,
        device: String,
        model: String,
        app: String,
        identity: Identity,
        sender: mpsc::Sender<ToPhone>,
        stream: TcpStream,
    },
    Message {
        session: u64,
        msg: ToPc,
    },
    Disconnected {
        session: u64,
        reason: String,
    },
    Pair(PairRequest),
    /// Le téléphone est parti (retour, autre QR) pendant que sa demande d'appairage attendait : son fil l'a close.
    PairAbandoned {
        device: String,
    },
    /// À écrire au journal (refus, erreurs).
    Log {
        device: String,
        kind: &'static str,
        text: String,
    },
}

/// Ce que partagent l'hôte et les fils.
pub struct Shared {
    pub key: Keypair,
    pub pc_name: String,
    pub eyrie: String,
    /// Jeton que le compagnon doit présenter ; `None` : connexions locales directes, sans en-tête (essais,
    /// développement avec `EYRIE_MOBILE_LOCAL`).
    pub token: Option<String>,
    devices: Mutex<Vec<DeviceAuth>>,
    /// La clé du veilleur, quand « Démarrer Eyrie depuis le téléphone » est voulu et possible : chaque appareil en
    /// reçoit son secret dans `Welcome`.
    standby: Mutex<Option<[u8; 32]>>,
    tickets: Mutex<Vec<Ticket>>,
    inbox: Mutex<VecDeque<Inbound>>,
    wake: Box<dyn Fn() + Send + Sync>,
    next: AtomicU64,
    stop: AtomicBool,
}

impl Shared {
    pub fn new(key: Keypair, pc_name: String, token: Option<String>, wake: Box<dyn Fn() + Send + Sync>) -> Self {
        Shared {
            key,
            pc_name,
            eyrie: env!("CARGO_PKG_VERSION").into(),
            token,
            devices: Mutex::new(Vec::new()),
            standby: Mutex::new(None),
            tickets: Mutex::new(Vec::new()),
            inbox: Mutex::new(VecDeque::new()),
            wake,
            next: AtomicU64::new(1),
            stop: AtomicBool::new(false),
        }
    }

    pub fn set_devices(&self, devices: Vec<DeviceAuth>) {
        *lock(&self.devices) = devices;
    }

    pub fn set_standby_key(&self, key: Option<[u8; 32]>) {
        *lock(&self.standby) = key;
    }

    /// Ce que reçoit l'appareil `dev` pour demander au veilleur de démarrer Eyrie (rien sans veilleur, ni sans le droit
    /// « Agir »).
    pub fn wake_key(&self, dev: &DeviceAuth) -> Option<WakeKey> {
        let key = (*lock(&self.standby))?;
        wake_key(&key, dev)
    }

    /// Un nouveau ticket (QR et code saisi ensemble : l'un utilisé, l'autre tombe).
    pub fn add_ticket(&self, secrets: &[Vec<u8>], lifetime: Duration) {
        let group = self.next.fetch_add(1, Ordering::Relaxed);
        let mut t = lock(&self.tickets);
        t.clear();
        for s in secrets {
            t.push(Ticket {
                group,
                secret: s.clone(),
                expires: Instant::now() + lifetime,
                attempts: pairing::ATTEMPTS,
            });
        }
    }

    pub fn clear_tickets(&self) {
        lock(&self.tickets).clear();
    }

    #[cfg(test)]
    pub fn has_ticket(&self) -> bool {
        let now = Instant::now();
        lock(&self.tickets).iter().any(|t| t.expires > now && t.attempts > 0)
    }

    pub fn take_inbox(&self) -> Vec<Inbound> {
        lock(&self.inbox).drain(..).collect()
    }

    fn push(&self, i: Inbound) {
        lock(&self.inbox).push_back(i);
        (self.wake)();
    }

    fn log(&self, device: &str, kind: &'static str, text: impl Into<String>) {
        self.push(Inbound::Log { device: device.to_string(), kind, text: text.into() });
    }

    fn device(&self, key: &[u8; 32]) -> Option<DeviceAuth> {
        lock(&self.devices).iter().find(|d| &d.key == key).cloned()
    }

    /// Le secret du ticket que cette preuve démontre ; le ticket (et son groupe) est alors consommé. Une preuve
    /// fausse coûte un essai à chaque ticket en cours.
    fn take_ticket(&self, proof: &[u8], hash: &[u8]) -> Option<Vec<u8>> {
        let now = Instant::now();
        let mut t = lock(&self.tickets);
        t.retain(|t| t.expires > now && t.attempts > 0);
        if let Some(found) = t.iter().find(|t| pairing::verify(&t.secret, hash, Role::Phone, proof)) {
            let (group, secret) = (found.group, found.secret.clone());
            t.retain(|t| t.group != group);
            return Some(secret);
        }
        for ticket in t.iter_mut() {
            ticket.attempts -= 1;
        }
        t.retain(|t| t.attempts > 0);
        None
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Le secret de démarrage de l'appareil `dev`, tiré de la clé du veilleur et de sa clé publique ; `device` est son
/// identifiant (le veilleur le cherche dans `appareils.json`). Démarrer Eyrie est une action : rien pour un appareil
/// sans le droit « Agir » (le veilleur le refuse aussi).
pub fn wake_key(key: &[u8; 32], dev: &DeviceAuth) -> Option<WakeKey> {
    dev.rights.act.then(|| WakeKey {
        port: veille::PORT,
        secret: veille::device_secret(key, &dev.key).to_vec(),
        device: dev.id.clone(),
    })
}

/// L'écoute en cours.
pub struct Server {
    pub addr: SocketAddr,
    shared: Arc<Shared>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Server {
    /// Écoute sur 127.0.0.1 (`port` 0 : un port libre).
    pub fn start(shared: Arc<Shared>, port: u16) -> std::io::Result<Server> {
        let listener = TcpListener::bind(("127.0.0.1", port))?;
        listener.set_nonblocking(true)?;
        let addr = listener.local_addr()?;
        let s = shared.clone();
        let thread = std::thread::Builder::new().name("mobile-ecoute".into()).spawn(move || {
            while !s.stop.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((stream, peer)) => {
                        // Le compagnon et les essais se connectent depuis la boucle locale ; rien d'autre.
                        if !peer.ip().is_loopback() {
                            continue;
                        }
                        let s = s.clone();
                        let _ =
                            std::thread::Builder::new().name("mobile-session".into()).spawn(move || serve(stream, s));
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(100))
                    }
                    Err(_) => std::thread::sleep(Duration::from_millis(250)),
                }
            }
        })?;
        Ok(Server { addr, shared, thread: Some(thread) })
    }

    pub fn shared(&self) -> &Arc<Shared> {
        &self.shared
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// L'en-tête du compagnon : une ligne JSON avant la poignée de main.
fn preamble(stream: &mut TcpStream, token: &str) -> Result<Identity, String> {
    let mut line = Vec::new();
    let mut byte = [0u8; 1];
    while line.len() < 2048 {
        stream.read_exact(&mut byte).map_err(|e| format!("en-tête du compagnon : {e}"))?;
        if byte[0] == b'\n' {
            let v: serde_json::Value =
                serde_json::from_slice(&line).map_err(|_| "en-tête du compagnon illisible".to_string())?;
            let field = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or_default().to_string();
            if !constant_eq(field("jeton").as_bytes(), token.as_bytes()) {
                return Err("jeton du compagnon faux".into());
            }
            return Ok(Identity { login: field("login"), node: field("noeud"), addr: field("adresse") });
        }
        line.push(byte[0]);
    }
    Err("en-tête du compagnon trop long".into())
}

fn constant_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Délai de la poignée de main et du premier message.
const OPENING: Duration = Duration::from_secs(10);
/// Sans message du téléphone (il envoie `Ping` toutes les 15 s), la connexion est fermée.
const IDLE: Duration = Duration::from_secs(45);
/// Le clic sur le PC est attendu au plus deux minutes.
pub const DECISION: Duration = Duration::from_secs(120);
/// Pendant cette attente, le PC regarde à ce pas si le téléphone est encore là.
const WATCH: Duration = Duration::from_millis(200);

/// Attend le clic sur le PC (`Some`), en regardant sur `watch`, une copie de sa connexion, si le téléphone est parti
/// (`None` : retour, autre QR, il a fermé sa connexion). Le premier des deux tranche la demande : close, un accord
/// arrivé ensuite n'appaire rien. Au bout de `within` ([`DECISION`]), elle est close aussi, et vaut un refus.
fn wait_decision(
    decision: &mpsc::Receiver<bool>,
    state: &AtomicU8,
    watch: std::io::Result<TcpStream>,
    within: Duration,
) -> Option<bool> {
    // Une copie à elle : sous Windows, son délai court ne touche pas la lecture des messages ; ailleurs, il est commun
    // à toutes les copies, d'où le délai d'ouverture rendu à la fin. Sans copie (plus de descripteurs), le clic est
    // attendu sans regarder le téléphone, et la demande est close au bout du délai comme les autres.
    let watch = watch.ok();
    if let Some(w) = &watch {
        let _ = w.set_read_timeout(Some(Duration::from_millis(1)));
    }
    let deadline = Instant::now() + within;
    let mut byte = [0u8; 1];
    let answer = loop {
        match decision.recv_timeout(WATCH) {
            Ok(yes) => break Some(yes),
            // La demande a disparu du PC sans réponse (accès coupé, délai passé) : non.
            Err(mpsc::RecvTimeoutError::Disconnected) => break Some(false),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        // Regardé sans être lu : ce que le téléphone aurait déjà envoyé reste aux messages.
        let gone = watch.as_ref().is_some_and(|w| match w.peek(&mut byte) {
            Ok(n) => n == 0,
            Err(e) => !matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut),
        });
        let late = Instant::now() >= deadline;
        // Acceptée entre-temps, la demande ne se ferme plus : la réponse arrive au tour suivant.
        if (gone || late) && state.compare_exchange(WAITING, CLOSED, Ordering::AcqRel, Ordering::Acquire).is_ok() {
            break if gone { None } else { Some(false) };
        }
    };
    if let Some(w) = &watch {
        let _ = w.set_read_timeout(Some(OPENING));
    }
    answer
}

fn serve(mut stream: TcpStream, shared: Arc<Shared>) {
    // Sous Windows, une connexion acceptée par une écoute non bloquante l'est aussi.
    if stream.set_nonblocking(false).is_err() {
        return;
    }
    let _ = stream.set_nodelay(true);
    let _ = stream.set_read_timeout(Some(OPENING));
    let _ = stream.set_write_timeout(Some(OPENING));
    let identity = match &shared.token {
        Some(token) => match preamble(&mut stream, token) {
            Ok(i) => i,
            Err(e) => {
                shared.log("", "refus", e);
                return;
            }
        },
        None => Identity { login: String::new(), node: "ce PC".into(), addr: "127.0.0.1".into() },
    };
    let who = if identity.node.is_empty() { identity.addr.clone() } else { identity.node.clone() };
    let (transport, peer) = match channel::accept(&mut stream, &shared.key) {
        Ok(x) => x,
        Err(e) => {
            shared.log(&who, "refus", format!("poignée de main refusée ({e})"));
            return;
        }
    };
    let Ok(reader) = stream.try_clone() else { return };
    let Ok(writer) = stream.try_clone() else { return };
    let (mut rx, mut tx) = transport.split(reader, writer);
    let session = shared.next.fetch_add(1, Ordering::Relaxed);
    let refuse = |tx: &mut channel::Sender<TcpStream>, reason: &str| {
        let _ = tx.send(&ToPhone::Refused { reason: reason.to_string() });
        let _ = stream.shutdown(Shutdown::Both);
    };
    loop {
        let first = match rx.recv::<ToPc>() {
            Ok(m) => m,
            Err(_) => return,
        };
        match first {
            ToPc::Hello { protocol, app, device, model, .. } => {
                if protocol != PROTOCOL {
                    shared.log(&device, "refus", format!("protocole {protocol} (Eyrie : {PROTOCOL})"));
                    refuse(&mut tx, "L'application et Eyrie n'ont pas la même version : mettez-les à jour.");
                    return;
                }
                let Some(dev) = shared.device(&peer.key) else {
                    shared.log(&device, "refus", format!("appareil inconnu ({who})"));
                    refuse(&mut tx, "Ce téléphone n'est pas appairé à ce PC : appairez-le depuis Eyrie.");
                    return;
                };
                if !dev.login.is_empty() && dev.login != identity.login {
                    shared.log(
                        &dev.name,
                        "refus",
                        format!("compte Tailscale « {} » au lieu de « {} »", identity.login, dev.login),
                    );
                    refuse(&mut tx, "Ce téléphone se connecte avec un autre compte Tailscale que lors de l'appairage.");
                    return;
                }
                let welcome = ToPhone::Welcome {
                    protocol: PROTOCOL,
                    pc: shared.pc_name.clone(),
                    eyrie: shared.eyrie.clone(),
                    rights: dev.rights,
                    wake: shared.wake_key(&dev),
                };
                if tx.send(&welcome).is_err() {
                    return;
                }
                // Sous Windows, les délais sont propres à chaque descripteur : ceux des copies de lecture et d'écriture
                // (faites avant la poignée de main) sont réglés elles-mêmes.
                let _ = rx.get_ref().set_read_timeout(Some(IDLE));
                let _ = tx.get_mut().set_write_timeout(Some(IDLE));
                let (out, queue) = mpsc::channel::<ToPhone>();
                let closer = stream.try_clone();
                let _ = std::thread::Builder::new().name("mobile-envoi".into()).spawn(move || {
                    for m in queue {
                        if tx.send(&m).is_err() {
                            break;
                        }
                    }
                    if let Ok(s) = closer {
                        let _ = s.shutdown(Shutdown::Both);
                    }
                });
                let Ok(handle) = stream.try_clone() else { return };
                shared.push(Inbound::Connected {
                    session,
                    device: dev.id,
                    model,
                    app,
                    identity: identity.clone(),
                    sender: out,
                    stream: handle,
                });
                let reason = loop {
                    match rx.recv::<ToPc>() {
                        Ok(msg) => shared.push(Inbound::Message { session, msg }),
                        Err(e) => break e.to_string(),
                    }
                    if shared.stop.load(Ordering::Relaxed) {
                        break "accès coupé".into();
                    }
                };
                shared.push(Inbound::Disconnected { session, reason });
                let _ = stream.shutdown(Shutdown::Both);
                return;
            }
            ToPc::Pair { protocol, proof, device, model } => {
                if protocol != PROTOCOL {
                    refuse(&mut tx, "L'application et Eyrie n'ont pas la même version : mettez-les à jour.");
                    return;
                }
                let Some(secret) = shared.take_ticket(&proof, &peer.hash) else {
                    shared.log(&device, "refus", format!("appairage : code faux ou expiré ({who})"));
                    refuse(&mut tx, "Code faux ou expiré : demandez un nouveau code dans Eyrie.");
                    return;
                };
                let sas = pairing::sas(&peer.hash);
                let pc_proof = pairing::proof(&secret, &peer.hash, Role::Pc).to_vec();
                if tx.send(&ToPhone::PairPending { proof: pc_proof, sas: sas.clone() }).is_err() {
                    return;
                }
                let (reply, decision) = mpsc::channel();
                let state = Arc::new(AtomicU8::new(WAITING));
                let name = device.trim().chars().take(60).collect::<String>();
                let name = if name.is_empty() { "Téléphone".to_string() } else { name };
                shared.push(Inbound::Pair(PairRequest {
                    name: name.clone(),
                    model: model.chars().take(80).collect(),
                    key: peer.key,
                    identity: identity.clone(),
                    sas,
                    at: Instant::now(),
                    reply,
                    state: state.clone(),
                }));

                match wait_decision(&decision, &state, stream.try_clone(), DECISION) {
                    Some(true) => {
                        if tx.send(&ToPhone::Paired { pc: shared.pc_name.clone() }).is_err() {
                            return;
                        }
                        // Le téléphone enchaîne avec `Hello` sur la même connexion (ou se reconnecte).
                    }
                    Some(false) => {
                        refuse(&mut tx, "Appairage refusé sur le PC.");
                        return;
                    }
                    None => {
                        shared.push(Inbound::PairAbandoned { device: name });
                        let _ = stream.shutdown(Shutdown::Both);
                        return;
                    }
                }
            }
            _ => {
                refuse(&mut tx, "Message inattendu.");
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use suite_mobile::client;

    fn shared(token: Option<&str>) -> Arc<Shared> {
        Arc::new(Shared::new(Keypair::generate(), "PC d'essai".into(), token.map(str::to_string), Box::new(|| {})))
    }

    fn wait_inbox(s: &Shared, mut pred: impl FnMut(&Inbound) -> bool) -> Inbound {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut seen = Vec::new();
        loop {
            for i in s.take_inbox() {
                if pred(&i) {
                    return i;
                }
                seen.push(match &i {
                    Inbound::Log { text, .. } => text.clone(),
                    _ => "autre".into(),
                });
            }
            assert!(Instant::now() < deadline, "rien d'attendu dans la boîte : {seen:?}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn open(server: &Server, me: &Keypair) -> client::Connection {
        client::open(&[server.addr.to_string()], me, Duration::from_secs(5)).unwrap()
    }

    #[test]
    fn a_phone_pairs_after_the_pc_confirms_then_says_hello() {
        let s = shared(None);
        let server = Server::start(s.clone(), 0).unwrap();
        let secret = pairing::new_secret();
        s.add_ticket(std::slice::from_ref(&secret), Duration::from_secs(60));
        let phone = Keypair::generate();
        let pc_key = s.key.public;
        let s2 = s.clone();
        let decider = std::thread::spawn(move || {
            let Inbound::Pair(req) = wait_inbox(&s2, |i| matches!(i, Inbound::Pair(_))) else { unreachable!() };
            assert_eq!(req.name, "Pixel");
            // Quelques tours de surveillance du téléphone avant le clic : sa connexion reste bonne pour la suite.
            std::thread::sleep(WATCH * 3);
            assert!(req.accept(), "le téléphone attend encore");
            // Ce que fait l'hôte au clic : l'appareil d'abord, la réponse ensuite.
            s2.set_devices(vec![DeviceAuth {
                id: "a1".into(),
                name: req.name.clone(),
                key: req.key,
                login: String::new(),
                rights: Rights::default(),
            }]);
            req.answer(true);
            req.sas.clone()
        });
        let mut c = open(&server, &phone);
        let sas = client::pair(&mut c, &secret, Some(&pc_key), "Pixel", "Pixel 8").unwrap();
        assert_eq!(client::wait_paired(&mut c).unwrap(), "PC d'essai");
        assert_eq!(decider.join().unwrap(), sas, "le même code des deux côtés");
        let w = client::hello(&mut c, &pc_key, "Pixel", "Pixel 8").unwrap();
        assert_eq!(w.pc, "PC d'essai");
        assert!(matches!(wait_inbox(&s, |i| matches!(i, Inbound::Connected { .. })), Inbound::Connected { .. }));
        assert!(!s.has_ticket(), "ticket consommé");
    }

    /// Les copies de lecture gardent le délai d'ouverture sous Windows s'il n'est pas changé sur elles : une session
    /// calme (le téléphone envoie `Ping` toutes les 15 s) ne doit pas tomber au bout de 10 s.
    #[test]
    fn an_idle_session_outlives_the_opening_delay() {
        let s = shared(None);
        let server = Server::start(s.clone(), 0).unwrap();
        let phone = Keypair::generate();
        s.set_devices(vec![DeviceAuth {
            id: "a1".into(),
            name: "Pixel".into(),
            key: phone.public,
            login: String::new(),
            rights: Rights::default(),
        }]);
        let mut c = open(&server, &phone);
        client::hello(&mut c, &s.key.public, "x", "y").unwrap();
        // Gardé comme l'hôte le garde : lâcher son envoi ferme la connexion, qui tomberait alors à la place du `Ping`.
        let _connected = wait_inbox(&s, |i| matches!(i, Inbound::Connected { .. }));
        std::thread::sleep(OPENING + Duration::from_secs(2));
        c.tx.send(&ToPc::Ping).unwrap();
        let Inbound::Message { msg, .. } =
            wait_inbox(&s, |i| matches!(i, Inbound::Message { .. } | Inbound::Disconnected { .. }))
        else {
            panic!("session fermée pendant qu'elle était calme");
        };
        assert_eq!(msg, ToPc::Ping);
    }

    /// Chaque appareil reçoit dans `Welcome` son propre secret de démarrage (celui que le veilleur recalcule) et son
    /// identifiant ; sans veilleur, rien.
    #[test]
    fn each_device_gets_its_own_wake_key_in_welcome() {
        let s = shared(None);
        let server = Server::start(s.clone(), 0).unwrap();
        let (pixel, tab) = (Keypair::generate(), Keypair::generate());
        let auth = |id: &str, k: &Keypair| DeviceAuth {
            id: id.into(),
            name: id.into(),
            key: k.public,
            login: String::new(),
            rights: Rights::default(),
        };
        s.set_devices(vec![auth("a1", &pixel), auth("b2", &tab)]);
        let mut c = open(&server, &pixel);
        assert_eq!(client::hello(&mut c, &s.key.public, "x", "y").unwrap().wake, None, "pas de veilleur");
        s.set_standby_key(Some([5; 32]));
        let mut seen = Vec::new();
        for (id, k) in [("a1", &pixel), ("b2", &tab)] {
            let mut c = open(&server, k);
            let w = client::hello(&mut c, &s.key.public, "x", "y").unwrap().wake.expect("clé du veilleur");
            assert_eq!(w.port, veille::PORT);
            assert_eq!(w.device, id);
            assert_eq!(w.secret, veille::device_secret(&[5; 32], &k.public).to_vec());
            seen.push(w.secret);
        }
        assert_ne!(seen[0], seen[1], "un secret par appareil");
    }

    /// Démarrer Eyrie est une action : un appareil sans le droit « Agir » ne reçoit pas de secret de démarrage.
    #[test]
    fn a_device_without_the_act_right_gets_no_wake_key() {
        let s = shared(None);
        let server = Server::start(s.clone(), 0).unwrap();
        let phone = Keypair::generate();
        let auth = |act| DeviceAuth {
            id: "a1".into(),
            name: "Pixel".into(),
            key: phone.public,
            login: String::new(),
            rights: Rights { act, ..Rights::default() },
        };
        s.set_standby_key(Some([5; 32]));
        s.set_devices(vec![auth(false)]);
        let mut c = open(&server, &phone);
        let w = client::hello(&mut c, &s.key.public, "x", "y").unwrap();
        assert!(!w.rights.act);
        assert_eq!(w.wake, None, "pas de secret sans « Agir »");
        s.set_devices(vec![auth(true)]);
        let mut c = open(&server, &phone);
        assert!(client::hello(&mut c, &s.key.public, "x", "y").unwrap().wake.is_some(), "avec « Agir »");
    }

    #[test]
    fn a_wrong_or_expired_code_is_refused() {
        let s = shared(None);
        let server = Server::start(s.clone(), 0).unwrap();
        s.add_ticket(&[pairing::new_secret()], Duration::from_secs(60));
        let mut c = open(&server, &Keypair::generate());
        let err = client::pair(&mut c, &pairing::new_secret(), None, "x", "y").unwrap_err().to_string();
        assert!(err.contains("Code faux"), "{err}");
        let secret = pairing::new_secret();
        s.add_ticket(std::slice::from_ref(&secret), Duration::from_millis(1));
        std::thread::sleep(Duration::from_millis(20));
        let mut c = open(&server, &Keypair::generate());
        assert!(client::pair(&mut c, &secret, None, "x", "y").is_err(), "ticket expiré");
    }

    #[test]
    fn five_wrong_codes_burn_the_ticket() {
        let s = shared(None);
        let server = Server::start(s.clone(), 0).unwrap();
        let secret = pairing::new_secret();
        s.add_ticket(std::slice::from_ref(&secret), Duration::from_secs(60));
        for _ in 0..pairing::ATTEMPTS {
            let mut c = open(&server, &Keypair::generate());
            assert!(client::pair(&mut c, &pairing::new_secret(), None, "x", "y").is_err());
        }
        assert!(!s.has_ticket());
        let mut c = open(&server, &Keypair::generate());
        assert!(client::pair(&mut c, &secret, None, "x", "y").is_err(), "même le bon code ne passe plus");
    }

    #[test]
    fn the_pc_saying_no_refuses_the_phone() {
        let s = shared(None);
        let server = Server::start(s.clone(), 0).unwrap();
        let secret = pairing::new_secret();
        s.add_ticket(std::slice::from_ref(&secret), Duration::from_secs(60));
        let s2 = s.clone();
        std::thread::spawn(move || {
            if let Inbound::Pair(req) = wait_inbox(&s2, |i| matches!(i, Inbound::Pair(_))) {
                req.answer(false);
            }
        });
        let mut c = open(&server, &Keypair::generate());
        client::pair(&mut c, &secret, None, "x", "y").unwrap();
        assert!(client::wait_paired(&mut c).unwrap_err().to_string().contains("refusé"));
    }

    /// Le téléphone ferme sa connexion pendant que le PC attend le clic : le fil le voit, clôt la demande (un accord
    /// ensuite n'y fait rien) et le dit à l'hôte.
    #[test]
    fn a_phone_leaving_closes_its_pairing_request() {
        let s = shared(None);
        let server = Server::start(s.clone(), 0).unwrap();
        let secret = pairing::new_secret();
        s.add_ticket(std::slice::from_ref(&secret), Duration::from_secs(60));
        let mut c = open(&server, &Keypair::generate());
        client::pair(&mut c, &secret, None, "Pixel", "y").unwrap();
        let Inbound::Pair(req) = wait_inbox(&s, |i| matches!(i, Inbound::Pair(_))) else { unreachable!() };
        drop(c);
        let Inbound::PairAbandoned { device } = wait_inbox(&s, |i| matches!(i, Inbound::PairAbandoned { .. })) else {
            unreachable!()
        };
        assert_eq!(device, "Pixel");
        assert!(req.closed());
        assert!(!req.accept(), "trop tard");
    }

    /// Sans copie de la connexion pour regarder le téléphone (plus de descripteurs), le PC attend quand même le clic ;
    /// au bout du délai, la demande est close comme les autres : un « Appairer » ensuite n'appaire pas un téléphone à
    /// qui le refus est déjà parti.
    #[test]
    fn without_a_copy_to_watch_the_phone_the_click_is_awaited_and_the_delay_closes_the_request() {
        let request = || {
            let (reply, decision) = mpsc::channel();
            let req = PairRequest {
                name: "Pixel".into(),
                model: String::new(),
                key: [0; 32],
                identity: Identity::default(),
                sas: String::new(),
                at: Instant::now(),
                reply,
                state: Arc::new(AtomicU8::new(WAITING)),
            };
            (req, decision)
        };
        let lost = || Err(std::io::Error::other("plus de descripteurs"));
        let (req, decision) = request();
        let state = req.state.clone();
        let click = std::thread::spawn(move || {
            std::thread::sleep(WATCH * 2);
            assert!(req.accept(), "le téléphone attend encore");
            req.answer(true);
        });
        assert_eq!(wait_decision(&decision, &state, lost(), Duration::from_secs(10)), Some(true), "le clic appaire");
        click.join().unwrap();
        let (req, decision) = request();
        assert_eq!(wait_decision(&decision, &req.state, lost(), WATCH * 2), Some(false), "délai passé : refus");
        assert!(req.closed(), "la confirmation disparaît du PC");
        assert!(!req.accept(), "trop tard");
    }

    #[test]
    fn an_unknown_device_or_another_tailscale_account_is_refused() {
        let s = shared(Some("jeton"));
        let server = Server::start(s.clone(), 0).unwrap();
        let phone = Keypair::generate();
        let with_header = |login: &str| {
            let mut stream = TcpStream::connect(server.addr).unwrap();
            writeln!(stream, r#"{{"jeton":"jeton","login":"{login}","noeud":"pixel","adresse":"100.1.2.3:5"}}"#)
                .unwrap();
            let (t, peer) = channel::connect(&mut stream, &phone).unwrap();
            let (rx, tx) = t.split(client::Stream::Tcp(stream.try_clone().unwrap()), client::Stream::Tcp(stream));
            client::Connection { rx, tx, peer }
        };
        let mut c = with_header("aubin@exemple.org");
        let err = client::hello(&mut c, &s.key.public, "x", "y").unwrap_err().to_string();
        assert!(err.contains("pas appairé"), "{err}");
        s.set_devices(vec![DeviceAuth {
            id: "a1".into(),
            name: "Pixel".into(),
            key: phone.public,
            login: "aubin@exemple.org".into(),
            rights: Rights::default(),
        }]);
        let mut c = with_header("intrus@exemple.org");
        assert!(client::hello(&mut c, &s.key.public, "x", "y").unwrap_err().to_string().contains("autre compte"));
        let mut c = with_header("aubin@exemple.org");
        assert!(client::hello(&mut c, &s.key.public, "x", "y").is_ok());
    }

    #[test]
    fn a_wrong_companion_token_closes_the_connection() {
        let s = shared(Some("bon-jeton"));
        let server = Server::start(s.clone(), 0).unwrap();
        let mut stream = TcpStream::connect(server.addr).unwrap();
        writeln!(stream, r#"{{"jeton":"mauvais","login":"x"}}"#).unwrap();
        let r = channel::connect(&mut stream, &Keypair::generate());
        assert!(r.is_err(), "pas de poignée de main sans le jeton");
        let Inbound::Log { text, .. } = wait_inbox(&s, |i| matches!(i, Inbound::Log { .. })) else { unreachable!() };
        assert!(text.contains("jeton"), "{text}");
    }

    #[test]
    fn the_phone_must_meet_the_pc_key_of_the_qr_code() {
        let s = shared(None);
        let server = Server::start(s.clone(), 0).unwrap();
        let secret = pairing::new_secret();
        s.add_ticket(std::slice::from_ref(&secret), Duration::from_secs(60));
        let mut c = open(&server, &Keypair::generate());
        let other_pc = Keypair::generate().public;
        assert!(
            client::pair(&mut c, &secret, Some(&other_pc), "x", "y").unwrap_err().to_string().contains("clé du PC")
        );
        assert!(s.has_ticket(), "rien n'a été envoyé : le ticket reste");
    }
}
