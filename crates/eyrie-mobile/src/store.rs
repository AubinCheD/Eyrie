//! Ce que le téléphone garde : sa clé (son identité auprès des PC), les PC appairés et leurs clés, les préférences.
//! Dans le stockage privé de l'application (inaccessible aux autres applications sur Android).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use suite_mobile::channel::Keypair;
use suite_mobile::msg::WakeKey;

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
    /// De quoi demander au veilleur du PC de démarrer Eyrie quand elle est fermée : donné par le PC à chaque
    /// connexion (`Welcome.wake`, remplacé ou retiré à chaque fois), parti avec le PC oublié.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wake: Option<Wake>,
}

/// La clé de démarrage d'un PC (voir `suite_mobile::veille`), gardée comme l'identité du téléphone : dans le stockage
/// privé de l'application, exclu des sauvegardes.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Wake {
    /// Le port du veilleur sur le tailnet.
    pub port: u16,
    /// Le secret de cet appareil (hexadécimal).
    pub secret: String,
    /// L'identifiant que le PC a donné à cet appareil (le champ `appareil` de la demande).
    #[serde(default)]
    pub device: String,
}

impl Wake {
    pub fn from_key(k: &WakeKey) -> Wake {
        Wake { port: k.port, secret: hex(&k.secret), device: k.device.clone() }
    }

    /// La clé, si elle sert : un secret de 32 octets et l'identifiant de l'appareil.
    pub fn key(&self) -> Option<WakeKey> {
        let secret = from_hex(&self.secret).filter(|s| s.len() == 32)?;
        if self.port == 0 || self.device.is_empty() {
            return None;
        }
        Some(WakeKey { port: self.port, secret, device: self.device.clone() })
    }
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

/// Les bandeaux pliés ou dépliés gardés au plus (les moins récemment touchés partent).
pub const FOLDS_MAX: usize = 300;

/// Un bandeau plié ou déplié par l'utilisateur : le téléphone retient son choix, rien n'est envoyé au PC.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Folded {
    pub open: bool,
    /// Le ton du bandeau à ce moment (0 aucun… 4 danger) : un bandeau plié qui devient plus grave se rouvre ; plié, il
    /// baisse quand le bandeau se calme.
    pub tone: i32,
    /// Ordre des choix : le plus ancien part au-delà de [`FOLDS_MAX`].
    pub seq: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    /// 0 : comme le téléphone ; 1 : clair ; 2 : sombre.
    pub theme: i32,
    /// 0 : ajusté ; 1 : plus grand ; 2 : très grand.
    pub text_size: i32,
    /// Le zoom du terminal laissé par le dernier pincement (1 : ajusté à la largeur) ; 0 : celui de `text_size`.
    pub term_zoom: f32,
    /// Modules de la barre du bas (trois au plus) ; vide : les premiers proposés.
    pub pinned: Vec<String>,
    pub device_name: String,
    pub current: Option<String>,
    /// Bandeaux pliés ou dépliés, par PC, module, écran et clé (voir [`Store::folded`]).
    pub folds: BTreeMap<String, Folded>,
    /// « Envoyer la dictée aussitôt » : dans le terminal, le texte dicté part avec Entrée, sans relecture (voir
    /// `dictee.rs`). Non par défaut.
    pub dictation_send: bool,
}

fn fold_key(pc: &str, module: &str, path: &str, band: &str) -> String {
    format!("{pc}\u{1f}{module}\u{1f}{path}\u{1f}{band}")
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

    /// Remplace (ou retire) la clé de démarrage d'un PC, et l'enregistre ; `false` : rien n'a changé.
    pub fn set_wake(&mut self, id: &str, wake: Option<Wake>) -> bool {
        let Some(p) = self.pcs.iter_mut().find(|p| p.id == id) else { return false };
        if p.wake == wake {
            return false;
        }
        p.wake = wake;
        self.save();
        true
    }

    /// Le choix fait pour un bandeau d'un écran de ce PC.
    pub fn folded(&self, pc: &str, module: &str, path: &str, band: &str) -> Option<&Folded> {
        self.prefs.folds.get(&fold_key(pc, module, path, band))
    }

    /// Retient le choix fait pour un bandeau (à enregistrer ensuite) ; [`FOLDS_MAX`] choix au plus.
    pub fn set_folded(&mut self, pc: &str, module: &str, path: &str, band: &str, open: bool, tone: i32) {
        let seq = self.prefs.folds.values().map(|f| f.seq).max().unwrap_or(0) + 1;
        self.prefs.folds.insert(fold_key(pc, module, path, band), Folded { open, tone, seq });
        while self.prefs.folds.len() > FOLDS_MAX {
            let oldest = self.prefs.folds.iter().min_by_key(|(_, f)| f.seq).map(|(k, _)| k.clone());
            match oldest {
                Some(k) => self.prefs.folds.remove(&k),
                None => break,
            };
        }
    }

    /// Oublie un PC ; le suivant devient le PC courant.
    pub fn forget(&mut self, id: &str) {
        self.pcs.retain(|p| p.id != id);
        let mine = format!("{id}\u{1f}");
        self.prefs.folds.retain(|k, _| !k.starts_with(&mine));
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
            wake: None,
        });
        s.prefs.theme = 2;
        assert!(!s.prefs.dictation_send, "la dictée ne part pas seule sans l'avoir choisi");
        s.prefs.dictation_send = true;
        s.pcs[0].notes_seen = 1_700_000_000;
        s.save();
        let back = Store::open(&dir);
        assert_eq!(back.identity, me, "la même identité : le PC la reconnaît");
        assert_eq!(back.current().map(|p| p.addresses()), Some(vec!["100.1.2.3:4747".into(), "suite-pc:4747".into()]));
        assert_eq!(back.prefs.theme, 2);
        assert!(back.prefs.dictation_send);
        assert_eq!(back.current().map(|p| p.notes_seen), Some(1_700_000_000), "vu jusque-là, même après un arrêt");
        let mut s = back;
        s.forget("a");
        assert!(s.current().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// La clé de démarrage d'un PC est gardée avec lui, et part avec lui ; une clé sans secret de 32 octets, sans
    /// port ou sans identifiant d'appareil ne sert pas.
    #[test]
    fn a_pcs_wake_key_is_kept_with_it() {
        let dir = std::env::temp_dir().join(format!("eyrie-mobile-wake-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut s = Store::open(&dir);
        let key = WakeKey { port: 4748, secret: vec![7; 32], device: "appareil-1".into() };
        s.add(Pc {
            id: "a".into(),
            name: "PC".into(),
            host: "suite-pc".into(),
            ip: String::new(),
            port: 4747,
            key: hex(&[3; 32]),
            paired_at: 1,
            notes_seen: 0,
            wake: Some(Wake::from_key(&key)),
        });
        let back = Store::open(&dir);
        assert_eq!(back.current().and_then(|p| p.wake.as_ref()).and_then(Wake::key), Some(key.clone()));
        let json = std::fs::read_to_string(dir.join("pcs.json")).unwrap();
        assert!(json.contains(&hex(&[7; 32])), "dans le stockage privé, avec le PC : {json}");

        assert_eq!(Wake { device: String::new(), ..Wake::from_key(&key) }.key(), None, "sans appareil");
        assert_eq!(Wake { port: 0, ..Wake::from_key(&key) }.key(), None, "sans port");
        assert_eq!(Wake { secret: hex(&[7; 31]), ..Wake::from_key(&key) }.key(), None, "secret tronqué");

        let mut s = back;
        s.forget("a");
        let json = std::fs::read_to_string(dir.join("pcs.json")).unwrap();
        assert!(!json.contains(&hex(&[7; 32])), "oublié avec le PC");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Une clé que le veilleur refuse (« preuve refusée ») est retirée, et le reste après un redémarrage de
    /// l'application : « Démarrer Eyrie » ne revient qu'avec la clé du prochain accueil.
    #[test]
    fn a_refused_wake_key_stays_forgotten_after_a_restart() {
        let dir = std::env::temp_dir().join(format!("eyrie-mobile-wake-refused-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut s = Store::open(&dir);
        let key = WakeKey { port: 4748, secret: vec![7; 32], device: "appareil-1".into() };
        let wake = Some(Wake::from_key(&key));
        let pc = |id: &str| Pc {
            id: id.into(),
            name: "PC".into(),
            host: "suite-pc".into(),
            ip: String::new(),
            port: 4747,
            key: hex(&[id.as_bytes()[0]; 32]),
            paired_at: 1,
            notes_seen: 0,
            wake: wake.clone(),
        };
        s.add(pc("b"));
        s.add(pc("a"));
        assert!(!s.set_wake("a", wake.clone()), "la même : rien à écrire");
        assert!(s.set_wake("a", None));
        assert!(!s.set_wake("inconnu", None));
        let back = Store::open(&dir);
        assert_eq!(back.current().map(|p| p.id.as_str()), Some("a"));
        assert_eq!(back.current().and_then(|p| p.wake.as_ref()), None, "après un redémarrage aussi");
        assert_eq!(
            back.pcs.iter().find(|p| p.id == "b").and_then(|p| p.wake.as_ref()),
            wake.as_ref(),
            "l'autre PC la garde"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Le pliage des bandeaux est gardé par PC, module, écran et clé ; les plus anciens choix partent au-delà de la
    /// limite ; oublier un PC oublie les siens.
    #[test]
    fn folded_bands_are_kept_per_screen_and_bounded() {
        let dir = std::env::temp_dir().join(format!("eyrie-mobile-folds-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut s = Store::open(&dir);
        s.set_folded("pc1", "fawkes", "", "g1", false, 1);
        s.set_folded("pc2", "fawkes", "", "g1", true, 0);
        s.save();
        let back = Store::open(&dir);
        assert_eq!(back.folded("pc1", "fawkes", "", "g1").map(|f| (f.open, f.tone)), Some((false, 1)));
        assert_eq!(back.folded("pc2", "fawkes", "", "g1").map(|f| f.open), Some(true));
        assert!(back.folded("pc1", "fawkes", "groupe", "g1").is_none());

        let mut s = back;
        for i in 0..FOLDS_MAX + 5 {
            s.set_folded("pc1", "owlery", "", &format!("depot{i}"), false, 0);
        }
        assert_eq!(s.prefs.folds.len(), FOLDS_MAX);
        assert!(s.folded("pc1", "fawkes", "", "g1").is_none(), "le plus ancien est parti");
        assert!(s.folded("pc1", "owlery", "", &format!("depot{}", FOLDS_MAX + 4)).is_some());
        s.forget("pc1");
        assert!(s.prefs.folds.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
