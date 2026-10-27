//! La connexion côté téléphone : se connecter, saluer un PC appairé, ou s'appairer. Utilisée par l'application et
//! par les essais d'Eyrie (un faux téléphone).

use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

use crate::channel::{self, Keypair, Peer, Receiver, Sender};
use crate::msg::{PROTOCOL, Rights, ToPc, ToPhone};
use crate::pairing::{self, Role};

/// Une connexion établie (poignée de main faite).
pub struct Connection {
    pub rx: Receiver<TcpStream>,
    pub tx: Sender<TcpStream>,
    pub peer: Peer,
}

/// Ce que dit le PC à un appareil appairé.
#[derive(Clone, Debug, PartialEq)]
pub struct Welcome {
    pub pc: String,
    pub eyrie: String,
    pub rights: Rights,
}

/// Se connecte à la première adresse qui répond, puis fait la poignée de main. `timeout` borne chaque connexion et la
/// poignée de main ; ensuite la lecture n'a plus de délai (le téléphone envoie `Ping`).
pub fn open(addrs: &[String], me: &Keypair, timeout: Duration) -> Result<Connection, String> {
    let mut last = String::from("aucune adresse");
    for addr in addrs {
        let resolved = match addr.to_socket_addrs() {
            Ok(r) => r.collect::<Vec<_>>(),
            Err(e) => {
                last = format!("{addr} : {e}");
                continue;
            }
        };
        for sa in resolved {
            match TcpStream::connect_timeout(&sa, timeout) {
                Ok(mut s) => {
                    let _ = s.set_nodelay(true);
                    let _ = s.set_read_timeout(Some(timeout));
                    let _ = s.set_write_timeout(Some(timeout));
                    let (t, peer) = channel::connect(&mut s, me).map_err(|e| format!("poignée de main : {e}"))?;
                    let _ = s.set_read_timeout(None);
                    let r = s.try_clone().map_err(|e| e.to_string())?;
                    let (rx, tx) = t.split(r, s);
                    return Ok(Connection { rx, tx, peer });
                }
                Err(e) => last = format!("{addr} : {e}"),
            }
        }
    }
    Err(last)
}

/// Salue un PC appairé : sa clé doit être celle gardée à l'appairage.
pub fn hello(conn: &mut Connection, pc_key: &[u8; 32], device: &str, model: &str) -> Result<Welcome, String> {
    if &conn.peer.key != pc_key {
        return Err("ce n'est pas le PC appairé (clé différente) : connexion refusée".into());
    }
    conn.tx
        .send(&ToPc::Hello {
            protocol: PROTOCOL,
            app: env!("CARGO_PKG_VERSION").into(),
            device: device.into(),
            model: model.into(),
        })
        .map_err(|e| e.to_string())?;
    match conn.rx.recv::<ToPhone>().map_err(|e| e.to_string())? {
        ToPhone::Welcome { protocol, pc, eyrie, rights } if protocol == PROTOCOL => Ok(Welcome { pc, eyrie, rights }),
        ToPhone::Welcome { .. } => Err("Eyrie et l'application n'ont pas la même version du protocole".into()),
        ToPhone::Refused { reason } | ToPhone::Bye { reason } => Err(reason),
        other => Err(format!("réponse inattendue : {other:?}")),
    }
}

/// Demande l'appairage avec le secret du ticket (ou du code saisi). Vérifie la preuve du PC, et sa clé si le QR la
/// donne. Renvoie le code de vérification à afficher ; le PC attend alors le clic de l'utilisateur ([`wait_paired`]).
pub fn pair(
    conn: &mut Connection,
    secret: &[u8],
    pc_key: Option<&[u8; 32]>,
    device: &str,
    model: &str,
) -> Result<String, String> {
    if let Some(k) = pc_key
        && &conn.peer.key != k
    {
        return Err("la clé du PC ne correspond pas au QR code : appairage arrêté".into());
    }
    let proof = pairing::proof(secret, &conn.peer.hash, Role::Phone).to_vec();
    conn.tx
        .send(&ToPc::Pair { protocol: PROTOCOL, proof, device: device.into(), model: model.into() })
        .map_err(|e| e.to_string())?;
    match conn.rx.recv::<ToPhone>().map_err(|e| e.to_string())? {
        ToPhone::PairPending { proof, sas } => {
            if !pairing::verify(secret, &conn.peer.hash, Role::Pc, &proof) {
                return Err("le PC n'a pas prouvé qu'il connaît le code : appairage arrêté".into());
            }
            if sas != pairing::sas(&conn.peer.hash) {
                return Err("codes de vérification différents : appairage arrêté".into());
            }
            Ok(sas)
        }
        ToPhone::Refused { reason } | ToPhone::Bye { reason } => Err(reason),
        other => Err(format!("réponse inattendue : {other:?}")),
    }
}

/// Attend la décision de l'utilisateur sur le PC.
pub fn wait_paired(conn: &mut Connection) -> Result<String, String> {
    match conn.rx.recv::<ToPhone>().map_err(|e| e.to_string())? {
        ToPhone::Paired { pc } => Ok(pc),
        ToPhone::Refused { reason } | ToPhone::Bye { reason } => Err(reason),
        other => Err(format!("réponse inattendue : {other:?}")),
    }
}
