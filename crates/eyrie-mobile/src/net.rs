//! La liaison avec le PC, dans son propre fil : connexion, salut, lecture, envoi, `Ping` toutes les 15 s, et
//! reconnexion avec un délai croissant (1 s, 2 s… 30 s), immédiate quand l'application revient au premier plan
//! (`kick`). Un refus du PC (appareil retiré, autre compte Tailscale) n'est pas réessayé en boucle.
//!
//! Et l'appairage, en une fois : connexion, preuve, code de vérification, attente du clic sur le PC.
//!
//! Sur Android, les connexions passent par le nœud Tailscale de l'application (`tailnet`).

use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::Duration;

use suite_mobile::channel::Keypair;
use suite_mobile::client::{self, Failure, Welcome};
use suite_mobile::msg::{ToPc, ToPhone};

const CONNECT: Duration = Duration::from_secs(6);
const PING: Duration = Duration::from_secs(15);
const MAX_BACKOFF: Duration = Duration::from_secs(30);

#[derive(Debug)]
pub enum NetEvent {
    Connecting,
    Online(Welcome),
    Message(Box<ToPhone>),
    /// Coupé ; nouvel essai dans ce délai.
    Offline {
        reason: String,
        retry_in: Duration,
    },
    /// Le PC refuse : pas de nouvel essai sans action de l'utilisateur.
    Refused(String),
}

enum Cmd {
    Send(ToPc),
    Kick,
    Stop,
    Dropped,
}

pub struct Link {
    tx: mpsc::Sender<Cmd>,
}

impl Link {
    pub fn start(
        addrs: Vec<String>,
        pc_key: [u8; 32],
        me: Keypair,
        device: String,
        model: String,
        on: impl Fn(NetEvent) + Send + 'static,
    ) -> Link {
        let (tx, rx) = mpsc::channel::<Cmd>();
        let back = tx.clone();
        let _ = std::thread::Builder::new()
            .name("eyrie-liaison".into())
            .spawn(move || run(addrs, pc_key, me, device, model, rx, back, on));
        Link { tx }
    }

    pub fn send(&self, m: ToPc) {
        let _ = self.tx.send(Cmd::Send(m));
    }

    /// Réessaie tout de suite (retour au premier plan, bouton « Réessayer »).
    pub fn kick(&self) {
        let _ = self.tx.send(Cmd::Kick);
    }
}

impl Drop for Link {
    fn drop(&mut self) {
        let _ = self.tx.send(Cmd::Stop);
    }
}

/// Une connexion au PC par le nœud Tailscale de l'application (sur le bureau : directe).
fn open(addrs: &[String], me: &Keypair) -> Result<client::Connection, String> {
    client::open_with(addrs, me, CONNECT, &crate::tailnet::dial)
}

/// Attend `delay` ou un réveil ; `false` : il faut s'arrêter.
fn wait(rx: &mpsc::Receiver<Cmd>, delay: Duration) -> bool {
    match rx.recv_timeout(delay) {
        Ok(Cmd::Stop) | Err(RecvTimeoutError::Disconnected) => false,
        // Les envois hors ligne sont perdus : l'écran est renvoyé à la reconnexion.
        Ok(_) | Err(RecvTimeoutError::Timeout) => true,
    }
}

#[allow(clippy::too_many_arguments)]
fn run(
    addrs: Vec<String>,
    pc_key: [u8; 32],
    me: Keypair,
    device: String,
    model: String,
    rx: mpsc::Receiver<Cmd>,
    back: mpsc::Sender<Cmd>,
    on: impl Fn(NetEvent),
) {
    let mut backoff = Duration::from_secs(1);
    loop {
        on(NetEvent::Connecting);
        let attempt = open(&addrs, &me).map_err(Failure::Broken).and_then(|mut c| {
            let w = client::hello(&mut c, &pc_key, &device, &model)?;
            Ok((c, w))
        });
        let (conn, welcome) = match attempt {
            Ok(x) => x,
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
                on(NetEvent::Offline { reason, retry_in: backoff });
                if !wait(&rx, backoff) {
                    return;
                }
                backoff = (backoff * 2).min(MAX_BACKOFF);
                continue;
            }
        };
        backoff = Duration::from_secs(1);
        on(NetEvent::Online(welcome));
        let client::Connection { rx: mut reader, tx: mut writer, .. } = conn;
        let socket = writer.get_mut().try_clone().ok();
        // La lecture, dans un fil : chaque message part vers l'interface ; une coupure réveille l'envoi.
        let (msg_tx, msg_rx) = mpsc::channel::<Result<ToPhone, String>>();
        let dropped = back.clone();
        let _ = std::thread::Builder::new().name("eyrie-lecture".into()).spawn(move || {
            loop {
                let r = reader.recv::<ToPhone>().map_err(|e| e.to_string());
                let end = r.is_err();
                if msg_tx.send(r).is_err() || end {
                    break;
                }
                let _ = dropped.send(Cmd::Kick);
            }
            let _ = dropped.send(Cmd::Dropped);
        });
        let mut reason = String::from("connexion perdue");
        'online: loop {
            // Les messages reçus d'abord (le réveil `Kick` n'est qu'un signal).
            while let Ok(r) = msg_rx.try_recv() {
                match r {
                    Ok(ToPhone::Bye { reason: why }) => {
                        reason = why;
                        break 'online;
                    }
                    Ok(m) => on(NetEvent::Message(Box::new(m))),
                    Err(e) => {
                        reason = e;
                        break 'online;
                    }
                }
            }
            match rx.recv_timeout(PING) {
                Ok(Cmd::Send(m)) => {
                    if let Err(e) = writer.send(&m) {
                        reason = e.to_string();
                        break;
                    }
                }
                Ok(Cmd::Kick) => {}
                Ok(Cmd::Dropped) => {
                    // Dernier tour : ce qui restait à lire, puis l'erreur.
                    while let Ok(r) = msg_rx.try_recv() {
                        match r {
                            Ok(m) => on(NetEvent::Message(Box::new(m))),
                            Err(e) => reason = e,
                        }
                    }
                    break;
                }
                Ok(Cmd::Stop) | Err(RecvTimeoutError::Disconnected) => {
                    if let Some(s) = &socket {
                        let _ = s.shutdown(std::net::Shutdown::Both);
                    }
                    return;
                }
                Err(RecvTimeoutError::Timeout) => {
                    if let Err(e) = writer.send(&ToPc::Ping) {
                        reason = e.to_string();
                        break;
                    }
                }
            }
        }
        if let Some(s) = &socket {
            let _ = s.shutdown(std::net::Shutdown::Both);
        }
        on(NetEvent::Offline { reason, retry_in: backoff });
        if !wait(&rx, backoff) {
            return;
        }
    }
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
