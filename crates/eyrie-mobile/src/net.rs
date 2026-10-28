//! La liaison avec le PC, dans son propre fil : connexion, salut, lecture, envoi, `Ping` toutes les 15 s (45 s sans
//! rien du PC : coupée), et reconnexion avec un délai croissant (1 s, 2 s… 30 s), immédiate quand l'application
//! revient au premier plan ou que le téléphone se reconnecte à Tailscale (`kick`). Un refus du PC (appareil retiré,
//! autre compte Tailscale) n'est pas réessayé en boucle.
//!
//! Coupée, la liaison dit ce qu'elle sait du PC (voir [`crate::veilleur`]) : après une connexion ratée, le nœud du
//! téléphone (hors ligne ? PC hors du tailnet ?), puis le veilleur du PC (Eyrie fermée, en démarrage, accès coupé), au
//! plus toutes les quelques secondes ; Eyrie fermée, elle réessaie sans marteler le PC. « Démarrer Eyrie » se fait
//! dans ce fil, pendant l'attente.
//!
//! Et l'appairage, en une fois : connexion, preuve, code de vérification, attente du clic sur le PC.
//!
//! Sur Android, les connexions passent par le nœud Tailscale de l'application (`tailnet`).

use std::io;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

use suite_mobile::channel::Keypair;
use suite_mobile::client::{self, Failure, Welcome};
use suite_mobile::msg::{ToPc, ToPhone, WakeKey};
use suite_mobile::veille;

use crate::tailnet;
use crate::veilleur::{self, Answer, Away, Eyrie, PACE, Pace, Reach, Watch};

const CONNECT: Duration = Duration::from_secs(6);
const PING: Duration = Duration::from_secs(15);
/// Sans rien du PC pendant ce temps (il répond `Pong` à chaque `Ping`), la liaison est tenue pour coupée : une
/// coupure sans fermeture (changement de réseau, PC en veille) se voit en moins d'une minute.
const SILENCE: Duration = Duration::from_secs(45);

#[derive(Debug)]
pub enum NetEvent {
    Connecting,
    Online(Welcome),
    Message(Box<ToPhone>),
    /// Coupé ; nouvel essai dans ce délai ; ce que l'on sait du PC.
    Offline {
        reason: String,
        retry_in: Duration,
        away: Away,
    },
    /// Le PC refuse : pas de nouvel essai sans action de l'utilisateur.
    Refused(String),
    /// La réponse du veilleur à « Démarrer Eyrie », ou pourquoi elle n'est pas venue.
    Started(Result<Answer, String>),
}

enum Cmd {
    Send(ToPc),
    Kick,
    /// « Démarrer Eyrie » (hors ligne seulement).
    Start,
    Stop,
    /// La lecture a reçu un message (un réveil de l'envoi).
    Got,
    Dropped,
}

pub struct Link {
    tx: mpsc::Sender<Cmd>,
}

impl Link {
    /// `wake` : la clé de démarrage gardée pour ce PC ; chaque `Welcome` la remplace.
    pub fn start(
        addrs: Vec<String>,
        pc_key: [u8; 32],
        me: Keypair,
        device: String,
        model: String,
        wake: Option<WakeKey>,
        on: impl Fn(NetEvent) + Send + 'static,
    ) -> Link {
        let pc = Peer { addrs, pc_key, me, device, model, wake };
        Self::start_with(pc, Tuning { silence: SILENCE, pace: PACE }, on)
    }

    fn start_with(pc: Peer, tuning: Tuning, on: impl Fn(NetEvent) + Send + 'static) -> Link {
        let (tx, rx) = mpsc::channel::<Cmd>();
        let back = tx.clone();
        let _ = std::thread::Builder::new().name("eyrie-liaison".into()).spawn(move || run(pc, tuning, rx, back, on));
        Link { tx }
    }

    pub fn send(&self, m: ToPc) {
        let _ = self.tx.send(Cmd::Send(m));
    }

    /// Réessaie tout de suite (retour au premier plan, bouton « Réessayer »).
    pub fn kick(&self) {
        let _ = self.tx.send(Cmd::Kick);
    }

    /// Demande au veilleur du PC de démarrer Eyrie ; la réponse arrive en [`NetEvent::Started`].
    pub fn start_eyrie(&self) {
        let _ = self.tx.send(Cmd::Start);
    }
}

impl Drop for Link {
    fn drop(&mut self) {
        let _ = self.tx.send(Cmd::Stop);
    }
}

/// Le PC à joindre, et qui le joint.
struct Peer {
    addrs: Vec<String>,
    pc_key: [u8; 32],
    me: Keypair,
    device: String,
    model: String,
    wake: Option<WakeKey>,
}

/// Les délais de la liaison (courts dans les essais).
struct Tuning {
    silence: Duration,
    pace: Pace,
}

/// Une connexion au PC par le nœud Tailscale de l'application (sur le bureau : directe).
fn open(addrs: &[String], me: &Keypair) -> Result<client::Connection, String> {
    client::open_with(addrs, me, CONNECT, &tailnet::dial)
}

/// Ce que l'on sait du PC après une connexion ratée : le nœud du téléphone d'abord (hors ligne, PC hors du tailnet),
/// puis le veilleur du PC, au port `port`.
fn reach(addrs: &[String], port: u16) -> Reach {
    let peer = || addrs.first().and_then(|a| tailnet::peer(a));
    if let Some(r) = veilleur::from_node(&tailnet::state(), peer) {
        return r;
    }
    match veilleur::ask_state(&veilleur::standby_addrs(addrs, port), &tailnet::dial) {
        Ok(e) => Reach::Standby(e),
        Err(_) => Reach::Unknown,
    }
}

/// Pourquoi l'attente s'arrête.
enum Woken {
    Go,
    Start,
    Stop,
}

/// Attend `delay`, un réveil (`kick`), « Démarrer Eyrie » ou l'arrêt.
fn wait(rx: &mpsc::Receiver<Cmd>, delay: Duration) -> Woken {
    let until = Instant::now() + delay;
    loop {
        match rx.recv_timeout(until.saturating_duration_since(Instant::now())) {
            Ok(Cmd::Stop) | Err(RecvTimeoutError::Disconnected) => return Woken::Stop,
            Ok(Cmd::Start) => return Woken::Start,
            Ok(Cmd::Kick) | Err(RecvTimeoutError::Timeout) => return Woken::Go,
            // Les envois hors ligne sont perdus : l'écran est renvoyé à la reconnexion. Les signaux de la lecture d'avant
            // (ses derniers messages, sa fin) n'avancent pas l'essai : une Eyrie qui se ferme n'est pas martelée.
            Ok(Cmd::Send(_) | Cmd::Got | Cmd::Dropped) => {}
        }
    }
}

/// Comment une connexion s'est terminée.
enum Ended {
    Stop,
    Lost(String),
    /// Le PC a dit qu'Eyrie se fermait.
    Closing(String),
}

fn run(mut pc: Peer, tuning: Tuning, rx: mpsc::Receiver<Cmd>, back: mpsc::Sender<Cmd>, on: impl Fn(NetEvent)) {
    let mut watch = Watch::new(tuning.pace);
    loop {
        on(NetEvent::Connecting);
        let attempt = open(&pc.addrs, &pc.me).map_err(Failure::Broken).and_then(|mut c| {
            let w = client::hello(&mut c, &pc.pc_key, &pc.device, &pc.model)?;
            Ok((c, w))
        });
        let reason = match attempt {
            Ok((conn, welcome)) => {
                watch.online();
                pc.wake = welcome.wake.clone();
                on(NetEvent::Online(welcome));
                match online(conn, tuning.silence, &rx, &back, &on) {
                    Ended::Stop => return,
                    Ended::Lost(reason) => reason,
                    Ended::Closing(reason) => {
                        watch.closing();
                        reason
                    }
                }
            }
            Err(Failure::Refused(reason)) => {
                on(NetEvent::Refused(reason));
                // Seul l'utilisateur relance (Réessayer) ; un arrêt met fin au fil.
                loop {
                    match rx.recv() {
                        Ok(Cmd::Kick) => break,
                        Ok(Cmd::Stop) | Err(_) => return,
                        Ok(_) => {}
                    }
                }
                continue;
            }
            Err(Failure::Broken(reason)) => {
                // Une connexion ratée : le veilleur dit pourquoi, s'il répond (au plus toutes les quelques secondes).
                if watch.may_probe(Instant::now()) {
                    let port = pc.wake.as_ref().map_or(veille::PORT, |w| w.port);
                    let r = reach(&pc.addrs, port);
                    watch.probed(r, Instant::now());
                }
                reason
            }
        };
        // Hors ligne jusqu'au prochain essai ; « Démarrer Eyrie » se fait pendant l'attente.
        loop {
            let delay = watch.delay(Instant::now());
            on(NetEvent::Offline { reason: reason.clone(), retry_in: delay, away: watch.away(Instant::now()) });
            match wait(&rx, delay) {
                Woken::Stop => return,
                Woken::Go => break,
                Woken::Start => {
                    let answer = match &pc.wake {
                        Some(w) => veilleur::ask_start(&veilleur::standby_addrs(&pc.addrs, w.port), w, &tailnet::dial),
                        None => Err("ce téléphone n'a pas de clé de démarrage pour ce PC".into()),
                    };
                    if let Ok(a) = &answer {
                        watch.started(a, Instant::now());
                    }
                    // Déjà ouverte : on s'y connecte tout de suite.
                    let open = matches!(answer, Ok(Answer::NotClosed(Eyrie::Ouverte)));
                    on(NetEvent::Started(answer));
                    if open {
                        break;
                    }
                }
            }
        }
    }
}

/// La connexion établie : la lecture dans un fil (chaque message part vers l'interface), l'envoi et `Ping` ici.
fn online(
    conn: client::Connection,
    silence: Duration,
    rx: &mpsc::Receiver<Cmd>,
    back: &mpsc::Sender<Cmd>,
    on: &impl Fn(NetEvent),
) -> Ended {
    let client::Connection { rx: mut reader, tx: mut writer, .. } = conn;
    let socket = writer.get_mut().try_clone().ok();
    let _ = reader.get_ref().set_read_timeout(Some(silence));
    // La lecture, dans un fil : chaque message part vers l'interface ; une coupure réveille l'envoi.
    let (msg_tx, msg_rx) = mpsc::channel::<Result<ToPhone, String>>();
    let dropped = back.clone();
    let _ = std::thread::Builder::new().name("eyrie-lecture".into()).spawn(move || {
        loop {
            let r = reader.recv::<ToPhone>().map_err(|e| match e.kind() {
                io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut => {
                    format!("rien reçu du PC depuis {} s", silence.as_secs())
                }
                _ => e.to_string(),
            });
            let end = r.is_err();
            if msg_tx.send(r).is_err() || end {
                break;
            }
            let _ = dropped.send(Cmd::Got);
        }
        let _ = dropped.send(Cmd::Dropped);
    });
    // Un message du PC part vers l'interface ; `Bye` ou une erreur terminent la connexion.
    let take = |r: Result<ToPhone, String>| -> Option<Ended> {
        match r {
            Ok(ToPhone::Bye { reason, closing: true }) => Some(Ended::Closing(reason)),
            Ok(ToPhone::Bye { reason, .. }) => Some(Ended::Lost(reason)),
            Ok(m) => {
                on(NetEvent::Message(Box::new(m)));
                None
            }
            Err(e) => Some(Ended::Lost(e)),
        }
    };
    let ended = 'online: loop {
        // Les messages reçus d'abord (le réveil `Got` n'est qu'un signal).
        while let Ok(r) = msg_rx.try_recv() {
            if let Some(end) = take(r) {
                break 'online end;
            }
        }
        match rx.recv_timeout(PING) {
            Ok(Cmd::Send(m)) => {
                if let Err(e) = writer.send(&m) {
                    break Ended::Lost(e.to_string());
                }
            }
            Ok(Cmd::Kick | Cmd::Got | Cmd::Start) => {}
            Ok(Cmd::Dropped) => {
                // Dernier tour : ce qui restait à lire, puis la fin (un `Bye` d'abord, sinon l'erreur).
                let mut end = None;
                while let Ok(r) = msg_rx.try_recv() {
                    if let Some(e) = take(r) {
                        end.get_or_insert(e);
                    }
                }
                break end.unwrap_or_else(|| Ended::Lost("connexion perdue".into()));
            }
            Ok(Cmd::Stop) | Err(RecvTimeoutError::Disconnected) => break Ended::Stop,
            Err(RecvTimeoutError::Timeout) => {
                if let Err(e) = writer.send(&ToPc::Ping) {
                    break Ended::Lost(e.to_string());
                }
            }
        }
    };
    if let Some(s) = &socket {
        let _ = s.shutdown(std::net::Shutdown::Both);
    }
    ended
}

/// Ce que l'appairage annonce à l'interface.
#[derive(Debug)]
pub enum PairEvent {
    /// Le code à comparer avec celui du PC.
    Sas(String),
    /// Appairé : le nom du PC et sa clé.
    Paired {
        pc: String,
        key: [u8; 32],
    },
    Failed(String),
}

/// L'appairage, dans un fil : `secret` du QR code ou du code saisi ; `pc_key` si le QR la donne.
pub fn pair(
    addrs: Vec<String>,
    secret: Vec<u8>,
    pc_key: Option<[u8; 32]>,
    me: Keypair,
    device: String,
    model: String,
    on: impl Fn(PairEvent) + Send + 'static,
) {
    let _ = std::thread::Builder::new().name("eyrie-appairage".into()).spawn(move || {
        let mut conn = match open(&addrs, &me) {
            Ok(c) => c,
            Err(e) => {
                on(PairEvent::Failed(format!(
                    "PC injoignable ({e}). Vérifiez que ce téléphone est connecté à Tailscale (accueil d'Eyrie) et que \
                     le PC affiche le code."
                )));
                return;
            }
        };
        let key = conn.peer.key;
        match client::pair(&mut conn, &secret, pc_key.as_ref(), &device, &model) {
            Ok(sas) => on(PairEvent::Sas(sas)),
            Err(e) => {
                on(PairEvent::Failed(e.to_string()));
                return;
            }
        }
        match client::wait_paired(&mut conn) {
            Ok(pc) => on(PairEvent::Paired { pc, key }),
            Err(e) => on(PairEvent::Failed(e.to_string())),
        }
    });
}

#[cfg(test)]
mod tests {
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

    use suite_mobile::channel;
    use suite_mobile::msg::{PROTOCOL, Rights};

    use super::*;
    use crate::store::{from_hex, hex};

    /// Les délais de la liaison coupée, raccourcis.
    const QUICK: Pace = Pace {
        first: Duration::from_millis(50),
        max: Duration::from_millis(400),
        closing: Duration::from_millis(500),
        closed: Duration::from_millis(300),
        probe: Duration::from_millis(100),
        starting: Duration::from_millis(100),
        starting_for: Duration::from_secs(5),
    };

    fn peer(addr: String, pc_key: [u8; 32], me: Keypair) -> Peer {
        Peer { addrs: vec![addr], pc_key, me, device: "x".into(), model: "y".into(), wake: None }
    }

    fn welcome(wake: Option<WakeKey>) -> ToPhone {
        ToPhone::Welcome { protocol: PROTOCOL, pc: "PC".into(), eyrie: "x".into(), rights: Rights::default(), wake }
    }

    /// Un PC qui accueille le téléphone puis se tait sans fermer la connexion (réseau coupé, PC en veille) : la
    /// liaison le voit et passe hors ligne pour se reconnecter.
    #[test]
    fn a_silent_pc_is_taken_as_disconnected() {
        let pc = Keypair::generate();
        let pc_key = pc.public;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let server = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let (t, _) = channel::accept(&mut s, &pc).unwrap();
            let (mut r, mut w) = t.split(s.try_clone().unwrap(), s);
            let _hello: ToPc = r.recv().unwrap();
            w.send(&welcome(None)).unwrap();
            // Plus rien : ni `Pong`, ni fermeture.
            std::thread::sleep(Duration::from_secs(3));
        });
        let (tx, rx) = mpsc::channel();
        let tuning = Tuning { silence: Duration::from_millis(400), pace: PACE };
        let link = Link::start_with(peer(addr, pc_key, Keypair::generate()), tuning, move |ev| {
            let _ = tx.send(ev);
        });
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut online = false;
        loop {
            match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                Ok(NetEvent::Online(_)) => online = true,
                Ok(NetEvent::Offline { reason, .. }) if online => {
                    assert!(reason.contains("rien reçu du PC"), "{reason}");
                    break;
                }
                Ok(_) => {}
                Err(_) => panic!("la liaison ne voit pas le silence du PC (en ligne : {online})"),
            }
        }
        drop(link);
        server.join().unwrap();
    }

    /// Un PC dont Eyrie se ferme dès le premier accueil, gardé joignable par son veilleur.
    struct ClosingPc {
        addr: String,
        pc_key: [u8; 32],
        /// La clé de démarrage que le PC donne au téléphone dans `Welcome`.
        wake: WakeKey,
        /// Les sondes (salut lu, rien demandé) et les demandes reçues par le veilleur.
        probes: Arc<AtomicU32>,
        requests: Arc<AtomicU32>,
        /// Les accueils du PC, rendus après le second (Eyrie démarrée).
        sessions: std::thread::JoinHandle<u32>,
    }

    /// `expected` : le secret que le veilleur attend (celui de l'appareil, tiré de sa clé publique) ; `given` : celui
    /// que le PC donne au téléphone.
    fn closing_pc(expected: [u8; 32], given: [u8; 32]) -> ClosingPc {
        let pc = Keypair::generate();
        let pc_key = pc.public;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let standby = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = standby.local_addr().unwrap().port();
        let wake = WakeKey { port, secret: given.to_vec(), device: "appareil-1".into() };
        let open = Arc::new(AtomicBool::new(true));
        let (probes, requests) = (Arc::new(AtomicU32::new(0)), Arc::new(AtomicU32::new(0)));

        // Eyrie : accueille, puis dit qu'elle se ferme ; fermée, toute connexion est coupée (le veilleur ne relaie
        // plus) ; démarrée, elle accueille de nouveau.
        let (o, w) = (open.clone(), wake.clone());
        let sessions = std::thread::spawn(move || {
            let mut welcomed = 0;
            for s in listener.incoming() {
                let Ok(mut s) = s else { continue };
                if !o.load(Ordering::SeqCst) {
                    continue;
                }
                let (t, _) = channel::accept(&mut s, &pc).unwrap();
                let (mut r, mut tx) = t.split(s.try_clone().unwrap(), s);
                let _hello: ToPc = r.recv().unwrap();
                tx.send(&welcome(Some(w.clone()))).unwrap();
                welcomed += 1;
                if welcomed > 1 {
                    std::thread::sleep(Duration::from_millis(200));
                    return welcomed;
                }
                o.store(false, Ordering::SeqCst);
                tx.send(&ToPhone::Bye { reason: "Eyrie se ferme.".into(), closing: true }).unwrap();
            }
            welcomed
        });

        // Le veilleur : son salut (défi 0x05…), puis la demande s'il en vient une, vérifiée comme sur le PC.
        let (o, p, q) = (open, probes.clone(), requests.clone());
        std::thread::spawn(move || {
            for s in standby.incoming() {
                let Ok(s) = s else { continue };
                let challenge = [5u8; 32];
                let state = if o.load(Ordering::SeqCst) { "ouverte" } else { "fermee" };
                let mut w = s.try_clone().unwrap();
                let _ = writeln!(w, "{{\"v\":1,\"defi\":\"{}\",\"eyrie\":\"{state}\"}}", hex(&challenge));
                let _ = s.set_read_timeout(Some(Duration::from_secs(2)));
                let mut line = String::new();
                if BufReader::new(&s).read_line(&mut line).unwrap_or(0) == 0 {
                    p.fetch_add(1, Ordering::SeqCst);
                    continue;
                }
                q.fetch_add(1, Ordering::SeqCst);
                let v: serde_json::Value = serde_json::from_str(&line).unwrap();
                let proof = from_hex(v["preuve"].as_str().unwrap_or_default()).unwrap_or_default();
                let ok = v["appareil"] == "appareil-1"
                    && v["demande"] == veille::START
                    && veille::verify(&expected, &challenge, "appareil-1", veille::START, &proof);
                if ok {
                    o.store(true, Ordering::SeqCst);
                    let _ = writeln!(w, "{{\"ok\":true,\"etat\":\"demarrage\"}}");
                } else {
                    let _ = writeln!(w, "{{\"ok\":false,\"raison\":\"preuve refusée\"}}");
                }
            }
        });
        ClosingPc { addr, pc_key, wake, probes, requests, sessions }
    }

    /// Jusqu'à « Eyrie est fermée », vue par le veilleur : accueilli (avec la clé de démarrage), « Eyrie se ferme »
    /// dit tout de suite, sans sonde ni reconnexion immédiate, puis la sonde après la connexion ratée.
    fn until_closed(pc: &ClosingPc, next: &dyn Fn() -> NetEvent) {
        loop {
            if let NetEvent::Online(w) = next() {
                assert_eq!(w.wake.as_ref(), Some(&pc.wake), "la clé vient de Welcome");
                break;
            }
        }
        let closed_at = loop {
            if let NetEvent::Offline { away, retry_in, .. } = next() {
                assert_eq!(away, Away { reach: Reach::Unknown, closed: true, starting: false });
                assert_eq!(retry_in, QUICK.closing);
                break Instant::now();
            }
        };
        let mut first_try = None;
        loop {
            match next() {
                NetEvent::Connecting => {
                    first_try.get_or_insert(closed_at.elapsed());
                }
                NetEvent::Offline { away, retry_in, .. } if away.reach == Reach::Standby(Eyrie::Fermee) => {
                    assert!(away.closed && !away.starting, "{away:?}");
                    assert_eq!(retry_in, QUICK.closed, "fermée : sans marteler le PC");
                    break;
                }
                _ => {}
            }
        }
        let first_try = first_try.expect("une reconnexion avant la sonde");
        assert!(first_try >= QUICK.closing - Duration::from_millis(30), "reconnexion trop tôt : {first_try:?}");
        // Le veilleur compte la sonde quand le téléphone ferme, un peu après.
        let until = Instant::now() + Duration::from_secs(2);
        while pc.probes.load(Ordering::SeqCst) == 0 && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!((pc.probes.load(Ordering::SeqCst), pc.requests.load(Ordering::SeqCst)), (1, 0));
    }

    /// Eyrie se ferme sur le PC, son veilleur la garde joignable : « Démarrer Eyrie » fait l'échange du veilleur, avec
    /// la preuve qu'il attend ; la liaison réessaie souvent et se connecte dès qu'Eyrie répond.
    #[test]
    fn a_closed_eyrie_is_started_from_the_phone() {
        let me = Keypair::generate();
        let secret = veille::device_secret(&[1; 32], &me.public);
        let pc = closing_pc(secret, secret);
        let (tx, rx) = mpsc::channel();
        let tuning = Tuning { silence: SILENCE, pace: QUICK };
        let link = Link::start_with(peer(pc.addr.clone(), pc.pc_key, me), tuning, move |ev| {
            let _ = tx.send(ev);
        });
        let deadline = Instant::now() + Duration::from_secs(10);
        let next = || rx.recv_timeout(deadline.saturating_duration_since(Instant::now())).expect("rien de la liaison");
        until_closed(&pc, &next);

        link.start_eyrie();
        loop {
            if let NetEvent::Started(answer) = next() {
                assert_eq!(answer, Ok(Answer::Starting));
                break;
            }
        }
        loop {
            match next() {
                NetEvent::Offline { away, retry_in, .. } => {
                    assert!(away.starting, "{away:?}");
                    assert_eq!(retry_in, QUICK.starting);
                }
                NetEvent::Online(_) => break,
                _ => {}
            }
        }
        assert_eq!(pc.requests.load(Ordering::SeqCst), 1);
        drop(link);
        assert_eq!(pc.sessions.join().unwrap(), 2);
    }

    /// Une clé que le veilleur ne reconnaît pas : son refus arrive tel quel, et rien n'est redemandé de soi-même.
    #[test]
    fn a_refused_proof_is_reported_and_not_retried() {
        let me = Keypair::generate();
        let pc = closing_pc(veille::device_secret(&[1; 32], &me.public), [9; 32]);
        let (tx, rx) = mpsc::channel();
        let tuning = Tuning { silence: SILENCE, pace: QUICK };
        let link = Link::start_with(peer(pc.addr.clone(), pc.pc_key, me), tuning, move |ev| {
            let _ = tx.send(ev);
        });
        let deadline = Instant::now() + Duration::from_secs(10);
        let next = || rx.recv_timeout(deadline.saturating_duration_since(Instant::now())).expect("rien de la liaison");
        until_closed(&pc, &next);

        link.start_eyrie();
        loop {
            if let NetEvent::Started(answer) = next() {
                assert_eq!(answer, Ok(Answer::Refused(veilleur::PROOF_REFUSED.into())));
                break;
            }
        }
        // Plusieurs essais de la liaison ensuite : toujours fermée, aucune autre demande.
        let until = Instant::now() + QUICK.closed * 4;
        while let Ok(ev) = rx.recv_timeout(until.saturating_duration_since(Instant::now())) {
            if let NetEvent::Offline { away, .. } = ev {
                assert_eq!(away.reach, Reach::Standby(Eyrie::Fermee));
                assert!(!away.starting);
            }
        }
        assert_eq!(pc.requests.load(Ordering::SeqCst), 1);
        assert!(pc.probes.load(Ordering::SeqCst) >= 2, "la liaison continue de regarder");
        drop(link);
    }
}
