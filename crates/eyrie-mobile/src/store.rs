//! Ce que le téléphone garde : sa clé (son identité auprès des PC), les PC appairés et leurs clés, les préférences.
//! Dans le stockage privé de l'application (inaccessible aux autres applications sur Android).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use suite_mobile::channel::Keypair;

/// Un PC appairé.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Pc {
    pub id: String,
    pub name: String,
    /// Nom MagicDNS (ou court) sur le tailnet.
    pub host: String,
    /// Adresse IPv4 sur le tailnet (vide : saisie à la main, le nom suffit).
    #[serde(default)]
    pub ip: String,
    pub port: u16,
    /// Clé publique du PC (hexadécimal), vérifiée à chaque connexion.
    pub key: String,
    #[serde(default)]
    pub paired_at: u64,
    /// Notifications du PC déjà vues par le téléphone, jusqu'à cet instant (secondes Unix ; 0 : jamais connecté) :
    /// à la reconnexion, même après un arrêt de l'application, ce qui est arrivé entre-temps est signalé.
    #[serde(default)]
    pub notes_seen: u64,
}

impl Pc {
    pub fn key_bytes(&self) -> Option<[u8; 32]> {
        from_hex(&self.key)?.try_into().ok()
    }

    /// Les adresses à essayer : l'IP d'abord (ne dépend pas de MagicDNS), puis le nom.
    pub fn addresses(&self) -> Vec<String> {
        let mut out = Vec::new();
        if !self.ip.is_empty() {
            out.push(format!("{}:{}", self.ip, self.port));
        }
        if !self.host.is_empty() {
            out.push(format!("{}:{}", self.host, self.port));
        }
        out
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    /// 0 : comme le téléphone ; 1 : clair ; 2 : sombre.
    pub theme: i32,
    /// 0 : ajusté ; 1 : plus grand ; 2 : très grand.
    pub text_size: i32,
    /// Modules de la barre du bas (trois au plus) ; vide : les premiers proposés.
    pub pinned: Vec<String>,
    pub device_name: String,
    pub current: Option<String>,
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn from_hex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok()).collect()
}

pub struct Store {
    dir: PathBuf,
    pub identity: Keypair,
    pub pcs: Vec<Pc>,
    pub prefs: Prefs,
}

fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

impl Store {
    pub fn open(dir: &Path) -> Store {
        let _ = std::fs::create_dir_all(dir);
        let identity = std::fs::read(dir.join("identite")).ok().and_then(|b| Keypair::from_bytes(&b));
        let identity = identity.unwrap_or_else(|| {
            let k = Keypair::generate();
            let _ = write_atomic(&dir.join("identite"), &k.to_bytes());
            k
        });
        let read = |name: &str| std::fs::read(dir.join(name)).ok();
        let pcs = read("pcs.json").and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        let prefs = read("prefs.json").and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        Store { dir: dir.to_path_buf(), identity, pcs, prefs }
    }

    pub fn save(&self) {
        if let Ok(b) = serde_json::to_vec_pretty(&self.pcs) {
            let _ = write_atomic(&self.dir.join("pcs.json"), &b);
        }
        if let Ok(b) = serde_json::to_vec_pretty(&self.prefs) {
            let _ = write_atomic(&self.dir.join("prefs.json"), &b);
        }
    }

    pub fn current(&self) -> Option<&Pc> {
        let id = self.prefs.current.as_deref()?;
        self.pcs.iter().find(|p| p.id == id)
    }

    /// Ajoute (ou remplace, même clé) un PC et en fait le PC courant.
    pub fn add(&mut self, pc: Pc) {
        self.pcs.retain(|p| p.key != pc.key);
        self.prefs.current = Some(pc.id.clone());
        self.pcs.push(pc);
        self.save();
    }

    /// Oublie un PC ; le suivant devient le PC courant.
    pub fn forget(&mut self, id: &str) {
        self.pcs.retain(|p| p.id != id);
        if self.prefs.current.as_deref() == Some(id) {
            self.prefs.current = self.pcs.first().map(|p| p.id.clone());
        }
        self.save();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_pcs_and_prefs_survive_a_restart() {
        let dir = std::env::temp_dir().join(format!("eyrie-mobile-store-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut s = Store::open(&dir);
        let me = s.identity.clone();
        s.add(Pc {
            id: "a".into(),
            name: "PC".into(),
            host: "suite-pc".into(),
            ip: "100.1.2.3".into(),
            port: 4747,
            key: hex(&[3; 32]),
            paired_at: 1,
            notes_seen: 0,
        });
        s.prefs.theme = 2;
        s.pcs[0].notes_seen = 1_700_000_000;
        s.save();
        let back = Store::open(&dir);
        assert_eq!(back.identity, me, "la même identité : le PC la reconnaît");
        assert_eq!(back.current().map(|p| p.addresses()), Some(vec!["100.1.2.3:4747".into(), "suite-pc:4747".into()]));
        assert_eq!(back.prefs.theme, 2);
        assert_eq!(back.current().map(|p| p.notes_seen), Some(1_700_000_000), "vu jusque-là, même après un arrêt");
        let mut s = back;
        s.forget("a");
        assert!(s.current().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
