//! Ce que le pont garde sur le disque : la clé du PC (chiffrée par DPAPI), les appareils appairés (clés publiques
//! seulement) et le journal. Le tout dans `%LOCALAPPDATA%\EyrieSuite\Eyrie\mobile` : rien de cela ne doit suivre un
//! profil itinérant ; un Eyrie sur un autre dossier (essais) garde le sien.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use suite_mobile::channel::Keypair;
use suite_mobile::msg::Rights;

/// Un appareil appairé.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Device {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub model: String,
    /// Clé publique (hexadécimal).
    pub key: String,
    /// Compte Tailscale vu à l'appairage : exigé ensuite (vide : appairé hors du tailnet, en essai).
    #[serde(default)]
    pub login: String,
    /// Appareil Tailscale vu à l'appairage (pour l'affichage).
    #[serde(default)]
    pub node: String,
    pub paired_at: u64,
    #[serde(default)]
    pub last_seen: u64,
    #[serde(default)]
    pub rights: Rights,
}

impl Device {
    pub fn key_bytes(&self) -> Option<[u8; 32]> {
        from_hex(&self.key)?.try_into().ok()
    }
}

/// Une ligne du journal du pont.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct JournalLine {
    pub at: u64,
    /// Appareil (nom), ou vide.
    #[serde(default)]
    pub device: String,
    /// `connexion`, `deconnexion`, `refus`, `appairage`, `action`, `terminal`, `acces`, `dossiers` (un sélecteur de
    /// dossiers ouvert), `veille` (écrite par le veilleur du compagnon, qui démarre Eyrie).
    pub kind: String,
    pub text: String,
}

pub fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
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

/// Le dossier du pont pour un Eyrie dont le dossier est `dir`.
pub fn mobile_dir(dir: &Path) -> PathBuf {
    if dir == crate::persist::default_data_dir() {
        suite_accounts::local_suite_dir().join("Eyrie").join("mobile")
    } else {
        dir.join("mobile")
    }
}

pub struct Store {
    dir: PathBuf,
    /// Créée au premier usage (`key`) : un Eyrie qui n'a jamais servi au téléphone n'en a pas.
    key: Option<Keypair>,
    /// La clé existe mais ne se lit pas : jamais remplacée en silence (les téléphones appairés ne la reconnaîtraient
    /// plus) ; les appairages et connexions sont suspendus, la page le dit.
    broken_key: Option<String>,
    pub devices: Vec<Device>,
    /// Ce qui n'a pas pu être lu ou écrit (affiché dans la page Téléphones).
    pub problem: Option<String>,
}

const JOURNAL_MAX: u64 = 1024 * 1024;

impl Store {
    pub fn open(dir: &Path) -> Store {
        let mut problem = None;
        let (key, broken_key) = match load_key(&dir.join("cle")) {
            Ok(k) => (k, None),
            Err(e) => (None, Some(format!("clé du PC illisible ({e}) : appairages et connexions suspendus"))),
        };
        let devices = match std::fs::read(dir.join("appareils.json")) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|e| {
                problem = Some(format!("appareils.json illisible : {e}"));
                Vec::new()
            }),
            Err(_) => Vec::new(),
        };
        Store { dir: dir.to_path_buf(), key, broken_key, devices, problem }
    }

    /// La clé du PC, créée et scellée au premier appel ; `None` si elle est illisible ou n'a pas pu être écrite.
    pub fn key(&mut self) -> Option<&Keypair> {
        if self.broken_key.is_some() {
            return None;
        }
        if self.key.is_none() {
            let _ = std::fs::create_dir_all(&self.dir);
            let k = Keypair::generate();
            match save_key(&self.dir.join("cle"), &k) {
                Ok(()) => self.key = Some(k),
                Err(e) => {
                    self.problem = Some(format!("clé du PC non enregistrée : {e}"));
                    return None;
                }
            }
        }
        self.key.as_ref()
    }

    /// Ce qui empêche le pont de marcher, s'il y a lieu.
    pub fn blocking_problem(&self) -> Option<&str> {
        self.broken_key.as_deref()
    }

    pub fn save(&mut self) {
        let _ = std::fs::create_dir_all(&self.dir);
        let path = self.dir.join("appareils.json");
        let tmp = self.dir.join("appareils.json.tmp");
        let json = serde_json::to_vec_pretty(&self.devices).unwrap_or_default();
        let r = std::fs::write(&tmp, json).and_then(|_| std::fs::rename(&tmp, &path));
        if let Err(e) = r {
            self.problem = Some(format!("appareils.json non enregistré : {e}"));
        }
    }

    #[cfg(test)]
    pub fn device(&self, id: &str) -> Option<&Device> {
        self.devices.iter().find(|d| d.id == id)
    }

    pub fn device_mut(&mut self, id: &str) -> Option<&mut Device> {
        self.devices.iter_mut().find(|d| d.id == id)
    }

    /// Ajoute (ou remplace, même clé) un appareil ; renvoie son identifiant.
    pub fn add(&mut self, name: &str, model: &str, key: &[u8; 32], login: &str, node: &str) -> String {
        let hex_key = hex(key);
        self.devices.retain(|d| d.key != hex_key);
        let id = hex(&suite_mobile::pairing::random::<6>());
        self.devices.push(Device {
            id: id.clone(),
            name: name.to_string(),
            model: model.to_string(),
            key: hex_key,
            login: login.to_string(),
            node: node.to_string(),
            paired_at: now(),
            last_seen: now(),
            rights: Rights::default(),
        });
        self.save();
        id
    }

    pub fn remove(&mut self, id: &str) -> Option<Device> {
        let i = self.devices.iter().position(|d| d.id == id)?;
        let d = self.devices.remove(i);
        self.save();
        Some(d)
    }

    pub fn journal(&self, device: &str, kind: &str, text: impl Into<String>) {
        let line = JournalLine { at: now(), device: device.to_string(), kind: kind.to_string(), text: text.into() };
        let _ = std::fs::create_dir_all(&self.dir);
        let path = self.dir.join("journal.jsonl");
        // Au-delà d'un mégaoctet, l'ancien journal devient `journal.1.jsonl` (un seul gardé).
        if std::fs::metadata(&path).is_ok_and(|m| m.len() > JOURNAL_MAX) {
            let _ = std::fs::rename(&path, self.dir.join("journal.1.jsonl"));
        }
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path)
            && let Ok(json) = serde_json::to_string(&line)
        {
            let _ = writeln!(f, "{json}");
        }
    }

    /// Les `max` dernières lignes du journal, la plus récente d'abord.
    pub fn read_journal(&self, max: usize) -> Vec<JournalLine> {
        let text = std::fs::read_to_string(self.dir.join("journal.jsonl")).unwrap_or_default();
        text.lines().rev().filter_map(|l| serde_json::from_str(l).ok()).take(max).collect()
    }
}

fn load_key(path: &Path) -> Result<Option<Keypair>, String> {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.to_string()),
    };
    let plain = dpapi::unprotect(&bytes, KEY_ENTROPY)?;
    Keypair::from_bytes(&plain).map(Some).ok_or_else(|| "taille inattendue".into())
}

fn save_key(path: &Path, key: &Keypair) -> Result<(), String> {
    let sealed = dpapi::protect(&key.to_bytes(), KEY_ENTROPY)?;
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, sealed).and_then(|_| std::fs::rename(&tmp, path)).map_err(|e| e.to_string())
}

/// L'entropie DPAPI de la clé du PC.
const KEY_ENTROPY: &[u8] = b"Eyrie mobile: cle du PC";

/// Chiffrement lié au compte Windows (DPAPI), comme la clé du nœud Tailscale ; `entropy` sépare les usages (clé du
/// PC, réglage du veilleur).
#[cfg(windows)]
pub(crate) mod dpapi {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{
        CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
    };

    fn blob(b: &[u8]) -> CRYPT_INTEGER_BLOB {
        CRYPT_INTEGER_BLOB { cbData: b.len() as u32, pbData: b.as_ptr() as *mut u8 }
    }

    pub fn protect(plain: &[u8], entropy: &[u8]) -> Result<Vec<u8>, String> {
        let input = blob(plain);
        let entropy = blob(entropy);
        let mut out = CRYPT_INTEGER_BLOB { cbData: 0, pbData: std::ptr::null_mut() };
        // SAFETY : blobs valides le temps de l'appel ; la sortie est rendue par LocalFree.
        let ok = unsafe {
            CryptProtectData(
                &input,
                std::ptr::null(),
                &entropy,
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut out,
            )
        };
        if ok == 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        // SAFETY : `out` vient de CryptProtectData.
        let bytes = unsafe { std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec() };
        unsafe { LocalFree(out.pbData as _) };
        Ok(bytes)
    }

    pub fn unprotect(sealed: &[u8], entropy: &[u8]) -> Result<Vec<u8>, String> {
        let input = blob(sealed);
        let entropy = blob(entropy);
        let mut out = CRYPT_INTEGER_BLOB { cbData: 0, pbData: std::ptr::null_mut() };
        // SAFETY : comme `protect`.
        let ok = unsafe {
            CryptUnprotectData(
                &input,
                std::ptr::null_mut(),
                &entropy,
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut out,
            )
        };
        if ok == 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        // SAFETY : `out` vient de CryptUnprotectData.
        let bytes = unsafe { std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec() };
        unsafe { LocalFree(out.pbData as _) };
        Ok(bytes)
    }
}

#[cfg(not(windows))]
pub(crate) mod dpapi {
    pub fn protect(plain: &[u8], _entropy: &[u8]) -> Result<Vec<u8>, String> {
        Ok(plain.to_vec())
    }

    pub fn unprotect(sealed: &[u8], _entropy: &[u8]) -> Result<Vec<u8>, String> {
        Ok(sealed.to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pc_key_is_kept_sealed_and_read_back() {
        let dir = tempfile::tempdir().unwrap();
        let mut first = Store::open(dir.path());
        assert!(!dir.path().join("cle").exists(), "pas de clé avant le premier usage");
        let key = first.key().cloned().unwrap();
        assert!(first.problem.is_none(), "{:?}", first.problem);
        let sealed = std::fs::read(dir.path().join("cle")).unwrap();
        assert!(!sealed.windows(32).any(|w| w == key.private), "clé privée en clair sur le disque");
        assert_eq!(Store::open(dir.path()).key().cloned(), Some(key));
    }

    #[test]
    fn an_unreadable_key_is_never_replaced_silently() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("cle"), b"abime").unwrap();
        let mut s = Store::open(dir.path());
        assert!(s.key().is_none() && s.blocking_problem().is_some());
        assert_eq!(std::fs::read(dir.path().join("cle")).unwrap(), b"abime");
    }

    #[test]
    fn devices_and_journal_survive_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = Store::open(dir.path());
        let id = s.add("Pixel 8", "Google Pixel 8", &[7; 32], "aubin@exemple.org", "pixel-8");
        s.journal("Pixel 8", "appairage", "appairé");
        let back = Store::open(dir.path());
        assert_eq!(back.device(&id).map(|d| d.name.as_str()), Some("Pixel 8"));
        assert_eq!(back.device(&id).and_then(Device::key_bytes), Some([7; 32]));
        assert_eq!(back.read_journal(10)[0].kind, "appairage");
        // Même clé : l'appareil est remplacé, pas doublé.
        let mut s = back;
        s.add("Pixel 8 (nouveau nom)", "", &[7; 32], "", "");
        assert_eq!(s.devices.len(), 1);
    }
}
