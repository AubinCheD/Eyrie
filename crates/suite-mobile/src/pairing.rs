//! Appairage d'un téléphone.
//!
//! Le PC tire un secret à usage unique, valable quelques minutes, et le montre en QR code avec son adresse sur le
//! tailnet et sa clé ([`Ticket`]), ou en code de dix caractères à saisir ([`new_code`]). Après la poignée de main
//! Noise, chacun prouve à l'autre qu'il connaît le secret ([`proof`], liée au hachage de la poignée de main : un
//! intermédiaire ne peut pas la rejouer), puis les deux affichent le même code de vérification ([`sas`]) et
//! l'utilisateur confirme sur le PC.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use blake2::digest::{KeyInit, Mac};
use blake2::{Blake2s256, Blake2sMac256, Digest};

/// Le secret d'un ticket de QR code.
pub const SECRET_LEN: usize = 32;
/// Durée de vie d'un ticket.
pub const TICKET_SECS: u64 = 300;
/// Essais permis avec un même ticket.
pub const ATTEMPTS: u32 = 5;

/// Alphabet du code à saisir : base 32 de Crockford (ni I, ni L, ni O, ni U).
const CODE_ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
const CODE_LEN: usize = 10;

/// Ce que montre le QR code.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ticket {
    /// Nom du PC, pour l'affichage.
    pub pc: String,
    /// Nom MagicDNS du nœud d'Eyrie.
    pub host: String,
    /// Adresse IPv4 du nœud sur le tailnet.
    pub ip: String,
    pub port: u16,
    /// Clé publique du PC.
    pub key: [u8; 32],
    pub secret: Vec<u8>,
}

impl Ticket {
    pub fn to_uri(&self) -> String {
        format!(
            "eyrie://appairer?v=1&n={}&h={}&a={}&p={}&k={}&s={}",
            escape(&self.pc),
            escape(&self.host),
            escape(&self.ip),
            self.port,
            B64.encode(self.key),
            B64.encode(&self.secret)
        )
    }

    pub fn from_uri(uri: &str) -> Result<Ticket, String> {
        let query = uri.trim().strip_prefix("eyrie://appairer?").ok_or("ce n'est pas un code d'appairage d'Eyrie")?;
        let mut t = Ticket {
            pc: String::new(),
            host: String::new(),
            ip: String::new(),
            port: 0,
            key: [0; 32],
            secret: Vec::new(),
        };
        let mut version = None;
        for pair in query.split('&') {
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            let v = unescape(v).ok_or("code d'appairage abîmé")?;
            match k {
                "v" => version = Some(v),
                "n" => t.pc = v,
                "h" => t.host = v,
                "a" => t.ip = v,
                "p" => t.port = v.parse().map_err(|_| "port illisible")?,
                "k" => {
                    t.key = B64.decode(v).ok().and_then(|b| b.try_into().ok()).ok_or("clé du PC illisible")?;
                }
                "s" => t.secret = B64.decode(v).map_err(|_| "secret illisible")?,
                _ => {}
            }
        }
        if version.as_deref() != Some("1") {
            return Err("code d'appairage d'une autre version d'Eyrie : mettez l'application à jour".into());
        }
        if t.secret.len() != SECRET_LEN || t.port == 0 || (t.host.is_empty() && t.ip.is_empty()) {
            return Err("code d'appairage incomplet".into());
        }
        Ok(t)
    }

    /// Les adresses à essayer, la plus sûre d'abord (l'adresse IP ne dépend pas de MagicDNS).
    pub fn addresses(&self) -> Vec<String> {
        [&self.ip, &self.host].iter().filter(|a| !a.is_empty()).map(|a| format!("{a}:{}", self.port)).collect()
    }
}

fn escape(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn unescape(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = std::str::from_utf8(bytes.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// Octets aléatoires du système.
pub fn random<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    getrandom::fill(&mut b).expect("générateur aléatoire du système indisponible");
    b
}

/// Un secret de ticket.
pub fn new_secret() -> Vec<u8> {
    random::<SECRET_LEN>().to_vec()
}

/// Un code à saisir : dix caractères (50 bits), montrés en deux groupes (`K7QF2-M9XP4`).
pub fn new_code() -> String {
    let bytes = random::<CODE_LEN>();
    let code: String = bytes.iter().map(|b| CODE_ALPHABET[(b & 31) as usize] as char).collect();
    format!("{}-{}", &code[..5], &code[5..])
}

/// Le code saisi, ramené à sa forme exacte (majuscules, sans tiret ni espace, O → 0, I et L → 1) ; `None` s'il ne
/// peut pas être un code.
pub fn normalize_code(input: &str) -> Option<String> {
    let mut out = String::new();
    for c in input.chars() {
        let c = match c.to_ascii_uppercase() {
            ' ' | '-' | '_' => continue,
            'O' => '0',
            'I' | 'L' => '1',
            c => c,
        };
        if !CODE_ALPHABET.contains(&(c as u8)) || !c.is_ascii() {
            return None;
        }
        out.push(c);
    }
    (out.len() == CODE_LEN).then_some(out)
}

/// Le secret d'un code saisi (sa forme exacte).
pub fn code_secret(code: &str) -> Option<Vec<u8>> {
    normalize_code(code).map(String::into_bytes)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Phone,
    Pc,
}

/// La preuve que `role` connaît `secret`, liée à cette poignée de main.
pub fn proof(secret: &[u8], handshake_hash: &[u8], role: Role) -> [u8; 32] {
    let mut mac = <Blake2sMac256 as KeyInit>::new_from_slice(secret).expect("secret de 32 octets au plus");
    mac.update(b"eyrie-appairage/1");
    mac.update(&[match role {
        Role::Phone => 1,
        Role::Pc => 2,
    }]);
    mac.update(handshake_hash);
    mac.finalize().into_bytes().into()
}

/// Vérifie une preuve, en temps constant.
pub fn verify(secret: &[u8], handshake_hash: &[u8], role: Role, candidate: &[u8]) -> bool {
    let Ok(mut mac) = <Blake2sMac256 as KeyInit>::new_from_slice(secret) else { return false };
    mac.update(b"eyrie-appairage/1");
    mac.update(&[match role {
        Role::Phone => 1,
        Role::Pc => 2,
    }]);
    mac.update(handshake_hash);
    mac.verify_slice(candidate).is_ok()
}

/// Code de vérification à six chiffres (« 482 913 »), le même des deux côtés d'une même poignée de main.
pub fn sas(handshake_hash: &[u8]) -> String {
    let mut h = Blake2s256::new();
    h.update(b"eyrie-sas/1");
    h.update(handshake_hash);
    let d = h.finalize();
    let n = u32::from_be_bytes([d[0], d[1], d[2], d[3]]) % 1_000_000;
    format!("{:03} {:03}", n / 1000, n % 1000)
}

/// Empreinte courte d'une clé, pour l'affichage (« 3F9A·C2E1 »).
pub fn fingerprint(key: &[u8]) -> String {
    let mut h = Blake2s256::new();
    h.update(key);
    let d = h.finalize();
    format!("{:02X}{:02X}·{:02X}{:02X}", d[0], d[1], d[2], d[3])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_ticket_survives_its_uri() {
        let t = Ticket {
            pc: "Poste d'Aubin & co".into(),
            host: "suite-pc.tail1234.ts.net".into(),
            ip: "100.101.102.103".into(),
            port: 4747,
            key: random(),
            secret: new_secret(),
        };
        let uri = t.to_uri();
        assert!(uri.starts_with("eyrie://appairer?v=1&"));
        assert!(!uri.contains(' ') && !uri.contains('\''), "{uri}");
        assert_eq!(Ticket::from_uri(&uri).unwrap(), t);
        assert_eq!(t.addresses(), ["100.101.102.103:4747", "suite-pc.tail1234.ts.net:4747"]);
    }

    #[test]
    fn a_broken_or_foreign_uri_is_refused() {
        assert!(Ticket::from_uri("https://exemple.org").is_err());
        assert!(Ticket::from_uri("eyrie://appairer?v=2&h=x&p=1&k=AAAA&s=AAAA").is_err());
        assert!(Ticket::from_uri("eyrie://appairer?v=1&h=x&p=4747").is_err(), "sans secret");
    }

    #[test]
    fn a_code_is_read_back_whatever_its_typing() {
        let code = new_code();
        assert_eq!(code.len(), 11);
        let exact = normalize_code(&code).unwrap();
        assert_eq!(normalize_code(&code.to_lowercase().replace('-', " ")), Some(exact.clone()));
        assert_eq!(normalize_code("k7qfo-m9xp1"), normalize_code("K7QF0-M9XPL"), "O → 0, L → 1");
        assert_eq!(normalize_code("K7QF2-M9XU4"), None, "U n'est pas dans l'alphabet");
        assert_eq!(normalize_code("K7QF2"), None, "trop court");
    }

    #[test]
    fn proofs_differ_by_role_and_by_handshake() {
        let secret = new_secret();
        let phone = proof(&secret, b"hash", Role::Phone);
        assert!(verify(&secret, b"hash", Role::Phone, &phone));
        assert!(!verify(&secret, b"hash", Role::Pc, &phone), "rejouée dans l'autre sens");
        assert!(!verify(&secret, b"autre", Role::Phone, &phone), "rejouée sur une autre poignée de main");
        assert!(!verify(&new_secret(), b"hash", Role::Phone, &phone), "autre secret");
        assert!(!verify(&secret, b"hash", Role::Phone, &phone[..31]));
    }

    #[test]
    fn the_verification_code_has_six_digits() {
        let s = sas(b"poignee");
        assert_eq!(s.len(), 7);
        assert!(s.chars().all(|c| c.is_ascii_digit() || c == ' '));
        assert_eq!(s, sas(b"poignee"));
        assert_ne!(s, sas(b"autre"));
    }
}
