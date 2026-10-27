//! Le canal chiffré : poignée de main Noise `XX_25519_ChaChaPoly_BLAKE2s` puis trames chiffrées.
//!
//! Une trame : sa longueur (u16, gros-boutiste) puis un message Noise. Un message applicatif (MessagePack) est
//! découpé en morceaux d'au plus [`CHUNK`] octets, chacun précédé d'un octet « la suite arrive » (1) ou « fin » (0).
//! Les deux sens ont chacun leur compteur de nonces : le canal se coupe en un [`Receiver`] et un [`Sender`] qui vivent
//! dans deux fils sans verrou.

use std::io::{self, Read, Write};
use std::sync::Arc;

use serde::Serialize;
use serde::de::DeserializeOwned;

const PATTERN: &str = "Noise_XX_25519_ChaChaPoly_BLAKE2s";
const PROLOGUE: &[u8] = b"eyrie-mobile/1";
const NOISE_MAX: usize = 65535;
const TAG: usize = 16;
/// Morceau d'un message applicatif.
pub const CHUNK: usize = 60_000;
/// Taille maximale d'un message applicatif.
pub const MESSAGE_MAX: usize = 16 * 1024 * 1024;

/// Une paire de clés X25519.
#[derive(Clone, PartialEq, Eq)]
pub struct Keypair {
    pub private: [u8; 32],
    pub public: [u8; 32],
}

impl std::fmt::Debug for Keypair {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Keypair").field("public", &crate::pairing::fingerprint(&self.public)).finish_non_exhaustive()
    }
}

impl Keypair {
    pub fn generate() -> Keypair {
        let k = builder().generate_keypair().expect("clés X25519");
        Keypair {
            private: k.private.as_slice().try_into().expect("clé privée de 32 octets"),
            public: k.public.as_slice().try_into().expect("clé publique de 32 octets"),
        }
    }

    /// 64 octets : la clé privée puis la publique (stockage).
    pub fn to_bytes(&self) -> Vec<u8> {
        [self.private.as_slice(), self.public.as_slice()].concat()
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Keypair> {
        if bytes.len() != 64 {
            return None;
        }
        Some(Keypair { private: bytes[..32].try_into().ok()?, public: bytes[32..].try_into().ok()? })
    }
}

fn builder() -> snow::Builder<'static> {
    snow::Builder::new(PATTERN.parse().expect("motif Noise"))
}

/// L'autre bout, tel que la poignée de main l'a montré.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Peer {
    /// Sa clé publique statique.
    pub key: [u8; 32],
    /// Le hachage de la poignée de main : les preuves d'appairage et le code de vérification en dépendent.
    pub hash: Vec<u8>,
}

fn invalid(e: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, e.to_string())
}

fn write_frame(w: &mut impl Write, bytes: &[u8]) -> io::Result<()> {
    let len = u16::try_from(bytes.len()).map_err(invalid)?;
    let mut out = Vec::with_capacity(2 + bytes.len());
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(bytes);
    w.write_all(&out)?;
    w.flush()
}

fn read_frame(r: &mut impl Read, buf: &mut Vec<u8>) -> io::Result<usize> {
    let mut len = [0u8; 2];
    r.read_exact(&mut len)?;
    let len = u16::from_be_bytes(len) as usize;
    buf.resize(len, 0);
    r.read_exact(&mut buf[..len])?;
    Ok(len)
}

/// Le côté qui attend la connexion (le PC).
pub fn accept<S: Read + Write>(stream: &mut S, me: &Keypair) -> io::Result<(Transport, Peer)> {
    let mut hs = builder()
        .local_private_key(&me.private)
        .and_then(|b| b.prologue(PROLOGUE))
        .and_then(|b| b.build_responder())
        .map_err(invalid)?;
    let mut frame = Vec::new();
    let mut scratch = vec![0u8; NOISE_MAX];
    let n = read_frame(stream, &mut frame)?;
    hs.read_message(&frame[..n], &mut scratch).map_err(invalid)?;
    let n = hs.write_message(&[], &mut scratch).map_err(invalid)?;
    write_frame(stream, &scratch[..n])?;
    let n = read_frame(stream, &mut frame)?;
    hs.read_message(&frame[..n], &mut scratch).map_err(invalid)?;
    finish(hs)
}

/// Le côté qui se connecte (le téléphone).
pub fn connect<S: Read + Write>(stream: &mut S, me: &Keypair) -> io::Result<(Transport, Peer)> {
    let mut hs = builder()
        .local_private_key(&me.private)
        .and_then(|b| b.prologue(PROLOGUE))
        .and_then(|b| b.build_initiator())
        .map_err(invalid)?;
    let mut frame = Vec::new();
    let mut scratch = vec![0u8; NOISE_MAX];
    let n = hs.write_message(&[], &mut scratch).map_err(invalid)?;
    write_frame(stream, &scratch[..n])?;
    let n = read_frame(stream, &mut frame)?;
    hs.read_message(&frame[..n], &mut scratch).map_err(invalid)?;
    let n = hs.write_message(&[], &mut scratch).map_err(invalid)?;
    write_frame(stream, &scratch[..n])?;
    finish(hs)
}

fn finish(hs: snow::HandshakeState) -> io::Result<(Transport, Peer)> {
    let key: [u8; 32] =
        hs.get_remote_static().ok_or_else(|| invalid("clé de l'autre bout absente"))?.try_into().map_err(invalid)?;
    let hash = hs.get_handshake_hash().to_vec();
    let state = hs.into_stateless_transport_mode().map_err(invalid)?;
    Ok((Transport { state: Arc::new(state) }, Peer { key, hash }))
}

/// Le canal établi, avant d'être coupé en deux sens.
pub struct Transport {
    state: Arc<snow::StatelessTransportState>,
}

impl Transport {
    pub fn split<R: Read, W: Write>(self, r: R, w: W) -> (Receiver<R>, Sender<W>) {
        (
            Receiver { r, state: self.state.clone(), nonce: 0, frame: Vec::new(), plain: vec![0u8; NOISE_MAX] },
            Sender { w, state: self.state, nonce: 0, out: vec![0u8; NOISE_MAX] },
        )
    }
}

/// Le sens qui écrit.
pub struct Sender<W: Write> {
    w: W,
    state: Arc<snow::StatelessTransportState>,
    nonce: u64,
    out: Vec<u8>,
}

impl<W: Write> Sender<W> {
    pub fn send<T: Serialize>(&mut self, msg: &T) -> io::Result<()> {
        let bytes = rmp_serde::to_vec_named(msg).map_err(invalid)?;
        self.send_bytes(&bytes)
    }

    pub fn send_bytes(&mut self, bytes: &[u8]) -> io::Result<()> {
        if bytes.len() > MESSAGE_MAX {
            return Err(invalid("message trop long"));
        }
        let mut chunks = bytes.chunks(CHUNK).peekable();
        // Un message vide est une trame « fin » vide.
        if chunks.peek().is_none() {
            return self.send_chunk(&[], false);
        }
        while let Some(c) = chunks.next() {
            let more = chunks.peek().is_some();
            self.send_chunk(c, more)?;
        }
        Ok(())
    }

    fn send_chunk(&mut self, chunk: &[u8], more: bool) -> io::Result<()> {
        let mut plain = Vec::with_capacity(chunk.len() + 1);
        plain.push(more as u8);
        plain.extend_from_slice(chunk);
        debug_assert!(plain.len() + TAG <= NOISE_MAX);
        let n = self.state.write_message(self.nonce, &plain, &mut self.out).map_err(invalid)?;
        self.nonce += 1;
        write_frame(&mut self.w, &self.out[..n])
    }

    pub fn get_mut(&mut self) -> &mut W {
        &mut self.w
    }
}

/// Le sens qui lit.
pub struct Receiver<R: Read> {
    r: R,
    state: Arc<snow::StatelessTransportState>,
    nonce: u64,
    frame: Vec<u8>,
    plain: Vec<u8>,
}

impl<R: Read> Receiver<R> {
    pub fn recv<T: DeserializeOwned>(&mut self) -> io::Result<T> {
        let bytes = self.recv_bytes()?;
        rmp_serde::from_slice(&bytes).map_err(invalid)
    }

    pub fn recv_bytes(&mut self) -> io::Result<Vec<u8>> {
        let mut message = Vec::new();
        loop {
            let n = read_frame(&mut self.r, &mut self.frame)?;
            let m = self.state.read_message(self.nonce, &self.frame[..n], &mut self.plain).map_err(invalid)?;
            self.nonce += 1;
            let Some((&more, chunk)) = self.plain[..m].split_first() else {
                return Err(invalid("trame vide"));
            };
            if message.len() + chunk.len() > MESSAGE_MAX {
                return Err(invalid("message trop long"));
            }
            message.extend_from_slice(chunk);
            if more == 0 {
                return Ok(message);
            }
        }
    }

    pub fn get_ref(&self) -> &R {
        &self.r
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{TcpListener, TcpStream};

    fn pair() -> ((Receiver<TcpStream>, Sender<TcpStream>, Peer), (Receiver<TcpStream>, Sender<TcpStream>, Peer)) {
        let pc = Keypair::generate();
        let phone = Keypair::generate();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let (t, peer) = accept(&mut s, &pc).unwrap();
            let (r, w) = t.split(s.try_clone().unwrap(), s);
            (r, w, peer)
        });
        let mut c = TcpStream::connect(addr).unwrap();
        let (t, peer) = connect(&mut c, &phone).unwrap();
        let (r, w) = t.split(c.try_clone().unwrap(), c);
        let s = server.join().unwrap();
        (s, (r, w, peer))
    }

    #[test]
    fn both_ends_see_each_other_and_the_same_hash() {
        let pc = Keypair::generate();
        assert_eq!(Keypair::from_bytes(&pc.to_bytes()), Some(pc));
        let ((_, _, seen_by_pc), (_, _, seen_by_phone)) = pair();
        assert_eq!(seen_by_pc.hash, seen_by_phone.hash);
        assert_ne!(seen_by_pc.key, seen_by_phone.key);
    }

    #[test]
    fn messages_go_both_ways_even_long_ones() {
        let ((mut pc_r, mut pc_w, _), (mut ph_r, mut ph_w, _)) = pair();
        ph_w.send(&crate::ToPc::Ping).unwrap();
        assert_eq!(pc_r.recv::<crate::ToPc>().unwrap(), crate::ToPc::Ping);
        let long = crate::ToPhone::Toast { text: "é".repeat(100_000), tone: crate::ui::Tone::Info };
        pc_w.send(&long).unwrap();
        pc_w.send(&crate::ToPhone::Pong).unwrap();
        assert_eq!(ph_r.recv::<crate::ToPhone>().unwrap(), long);
        assert_eq!(ph_r.recv::<crate::ToPhone>().unwrap(), crate::ToPhone::Pong);
    }

    #[test]
    fn a_tampered_frame_is_refused() {
        let pc = Keypair::generate();
        let phone = Keypair::generate();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let (t, _) = accept(&mut s, &pc).unwrap();
            let (mut r, _w) = t.split(s.try_clone().unwrap(), s);
            r.recv::<crate::ToPc>()
        });
        let mut c = TcpStream::connect(addr).unwrap();
        let (t, _) = connect(&mut c, &phone).unwrap();
        // Une trame chiffrée, puis un octet changé.
        let (_, mut w) = t.split(std::io::empty(), Vec::new());
        w.send(&crate::ToPc::Ping).unwrap();
        let mut bytes = std::mem::take(w.get_mut());
        let last = bytes.len() - 1;
        bytes[last] ^= 1;
        c.write_all(&bytes).unwrap();
        assert!(server.join().unwrap().is_err());
    }

    #[test]
    fn a_plain_http_request_is_not_a_handshake() {
        let pc = Keypair::generate();
        let mut req = std::io::Cursor::new(b"GET / HTTP/1.1\r\nHost: x\r\n\r\n".to_vec());
        let mut both = ReadWrite(&mut req, Vec::new());
        assert!(accept(&mut both, &pc).is_err());
    }

    struct ReadWrite<'a>(&'a mut std::io::Cursor<Vec<u8>>, Vec<u8>);
    impl Read for ReadWrite<'_> {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            self.0.read(buf)
        }
    }
    impl Write for ReadWrite<'_> {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.1.write(buf)
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
}
