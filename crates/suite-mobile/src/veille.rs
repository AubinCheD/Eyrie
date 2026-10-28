//! Démarrer Eyrie depuis le téléphone quand elle est fermée : le défi-réponse entre l'application et le veilleur du
//! PC (`eyrie-tailnet.exe --veille`, voir `docs/mobile.md` §3).
//!
//! Le veilleur garde le PC sur le tailnet quand Eyrie est fermée et écoute [`PORT`]. Il ne parle pas Noise et n'a
//! pas la clé du PC : il peut seulement démarrer Eyrie. Une demande se fait en lignes JSON :
//!
//! 1. veilleur → téléphone : `{"v":1,"defi":"<64 hex>","eyrie":"fermee|demarrage|ouverte|acces-coupe"}` ;
//! 2. téléphone → veilleur : `{"appareil":"<id>","demande":"demarrer","preuve":"<64 hex>"}` ;
//! 3. veilleur → téléphone : `{"ok":true,"etat":"demarrage"}` ou `{"ok":false,"raison":"…"}`.
//!
//! Chaque appareil a son secret, tiré d'une clé du PC (32 octets, chiffrée par DPAPI) et de sa clé publique : le PC le
//! lui donne dans `Welcome` ([`crate::msg::WakeKey`], avec l'identifiant de l'appareil), le veilleur le recalcule.
//! Retirer l'appareil le révoque. Le compagnon (Go) refait ces calculs ; les vecteurs des essais sont les mêmes des
//! deux côtés.

use blake2::Blake2sMac256;
use blake2::digest::{KeyInit, Mac};

/// Port du veilleur sur le tailnet.
pub const PORT: u16 = 4748;

/// Seule demande connue.
pub const START: &str = "demarrer";

fn mac(key: &[u8]) -> Blake2sMac256 {
    <Blake2sMac256 as KeyInit>::new_from_slice(key).expect("clé de 32 octets au plus")
}

/// Un champ de longueur variable, précédé de sa longueur (gros-boutiste) : aucune concaténation ambiguë.
fn field(m: &mut Blake2sMac256, bytes: &[u8]) {
    m.update(&(bytes.len() as u32).to_be_bytes());
    m.update(bytes);
}

/// Le secret de démarrage d'un appareil : MAC de sa clé publique par la clé du veilleur.
pub fn device_secret(pc_key: &[u8; 32], device_public: &[u8; 32]) -> [u8; 32] {
    let mut m = mac(pc_key);
    m.update(b"eyrie-veille/appareil");
    m.update(device_public);
    m.finalize().into_bytes().into()
}

/// La preuve que l'appareil `device` connaît son secret, liée au défi du veilleur et à la demande.
pub fn proof(secret: &[u8; 32], challenge: &[u8; 32], device: &str, request: &str) -> [u8; 32] {
    let mut m = mac(secret);
    m.update(b"eyrie-veille/1");
    m.update(challenge);
    field(&mut m, device.as_bytes());
    field(&mut m, request.as_bytes());
    m.finalize().into_bytes().into()
}

/// Vérifie une preuve, en temps constant.
pub fn verify(secret: &[u8; 32], challenge: &[u8; 32], device: &str, request: &str, candidate: &[u8]) -> bool {
    let mut m = mac(secret);
    m.update(b"eyrie-veille/1");
    m.update(challenge);
    field(&mut m, device.as_bytes());
    field(&mut m, request.as_bytes());
    m.verify_slice(candidate).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    /// Vecteurs partagés avec `compagnon/veille_test.go` : clé du PC 0x01…, clé publique 0x02…, défi 0x03….
    #[test]
    fn vectors_match_the_companion() {
        let secret = device_secret(&[1; 32], &[2; 32]);
        assert_eq!(hex(&secret), "12dd85dc64a26b78466d6343ca4f8ffdd0a74de58ab34ee9d4ce7b1650f86431");
        let p = proof(&secret, &[3; 32], "appareil-1", START);
        assert_eq!(hex(&p), "ad8399515aa176a2c4cd1622d9763ab2df25c9133b54198cbd680b04c4310953");
    }

    #[test]
    fn a_proof_holds_only_for_its_challenge_device_and_request() {
        let secret = device_secret(&[7; 32], &[9; 32]);
        let p = proof(&secret, &[1; 32], "tel", START);
        assert!(verify(&secret, &[1; 32], "tel", START, &p));
        assert!(!verify(&secret, &[2; 32], "tel", START, &p), "autre défi");
        assert!(!verify(&secret, &[1; 32], "autre", START, &p), "autre appareil");
        assert!(!verify(&secret, &[1; 32], "tel", "arreter", &p), "autre demande");
        assert!(!verify(&device_secret(&[8; 32], &[9; 32]), &[1; 32], "tel", START, &p), "autre clé du PC");
        assert!(!verify(&secret, &[1; 32], "tel", START, &p[..31]), "preuve tronquée");
    }
}
