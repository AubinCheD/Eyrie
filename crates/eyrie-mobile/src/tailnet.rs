//! Le téléphone sur le tailnet par Eyrie elle-même : un nœud Tailscale dans l'application (bibliothèque Go
//! `compagnon/telephone`, tsnet en espace utilisateur : ni VPN Android, ni application Tailscale). Les connexions
//! vers le PC passent par lui ([`dial`]) ; le compte Tailscale se connecte une fois, dans le navigateur.
//!
//! Sur le bureau (développement), pas de nœud : la connexion est directe, par le réseau du poste.

use std::path::PathBuf;
use std::time::Duration;

use suite_mobile::client::Stream;

/// L'état du nœud, tel que l'interface le montre.
#[derive(Clone, Debug, PartialEq)]
pub enum Tailnet {
    /// Pas de nœud dans l'application (bureau : connexion directe).
    Absent,
    Starting,
    /// Le lien de connexion à ouvrir dans le navigateur (vide tant que Tailscale ne l'a pas donné).
    NeedsLogin(String),
    Running {
        name: String,
        ip: String,
    },
    Failed(String),
}

impl Tailnet {
    /// L'état rendu par la bibliothèque : `{"state": …, "url"|"name"|"ip"|"message": …}`.
    pub fn parse(json: &str) -> Tailnet {
        let v: std::collections::HashMap<String, String> = serde_json::from_str(json).unwrap_or_default();
        let get = |k: &str| v.get(k).cloned().unwrap_or_default();
        match get("state").as_str() {
            "needs_login" => Tailnet::NeedsLogin(get("url")),
            "running" => Tailnet::Running { name: get("name"), ip: get("ip") },
            "error" => Tailnet::Failed(get("message")),
            _ => Tailnet::Starting,
        }
    }

    /// Le téléphone peut-il joindre le PC ?
    pub fn ready(&self) -> bool {
        matches!(self, Tailnet::Absent | Tailnet::Running { .. })
    }

    /// Pour l'interface : 0 absent, 1 démarrage, 2 connexion à faire, 3 connecté, 4 erreur ; et le texte.
    pub fn display(&self) -> (i32, String) {
        match self {
            Tailnet::Absent => (0, String::new()),
            Tailnet::Starting => (1, "Connexion au tailnet…".into()),
            Tailnet::NeedsLogin(url) if url.is_empty() => (1, "Connexion au tailnet…".into()),
            Tailnet::NeedsLogin(_) => (2, "Ce téléphone n'est pas encore connecté à votre compte Tailscale.".into()),
            Tailnet::Running { name, .. } => {
                (3, format!("Sur le tailnet : {}", name.split('.').next().unwrap_or(name)))
            }
            Tailnet::Failed(e) => (4, format!("Tailscale : {e}")),
        }
    }
}

/// Ce que le nœud du téléphone sait du PC (`EyrieTailnetPeer`), quand la liaison ne passe pas.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Peer {
    /// Un appareil du tailnet a l'adresse du PC.
    pub known: bool,
    /// Il est en ligne sur Tailscale.
    pub online: bool,
    /// Sa dernière connexion à Tailscale (secondes Unix ; maintenant s'il est en ligne, 0 si on l'ignore).
    pub seen: i64,
}

impl Peer {
    /// La réponse de la bibliothèque : `{"connu":bool,"enligne":bool,"vu":<secondes>}`.
    pub fn parse(json: &str) -> Option<Peer> {
        #[derive(serde::Deserialize)]
        struct Raw {
            connu: bool,
            enligne: bool,
            #[serde(default)]
            vu: i64,
        }
        let r: Raw = serde_json::from_str(json).ok()?;
        Some(Peer { known: r.connu, online: r.enligne, seen: r.vu })
    }

    /// Le PC est-il en ligne sur le tailnet ?
    pub fn reachable(&self) -> bool {
        self.known && self.online
    }
}

/// Le nom du téléphone dans le tailnet : `eyrie-` puis son nom, en lettres, chiffres et tirets.
pub fn hostname(device: &str) -> String {
    let mut slug = String::new();
    for c in device.to_lowercase().chars() {
        let c = match c {
            'à' | 'â' | 'ä' => 'a',
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'î' | 'ï' => 'i',
            'ô' | 'ö' => 'o',
            'ù' | 'û' | 'ü' => 'u',
            'ç' => 'c',
            c => c,
        };
        if c.is_ascii_alphanumeric() {
            slug.push(c);
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let slug: String = slug.trim_end_matches('-').chars().take(40).collect();
    let slug = slug.trim_end_matches('-');
    if slug.is_empty() { "eyrie-telephone".into() } else { format!("eyrie-{slug}") }
}

#[cfg(target_os = "android")]
mod node {
    use std::ffi::{CStr, CString, c_char, c_int};
    use std::os::fd::IntoRawFd;
    use std::os::unix::net::UnixStream;
    use std::path::Path;
    use std::sync::Mutex;
    use std::time::Duration;

    use suite_mobile::client::Stream;

    use super::{Peer, Tailnet};

    #[link(name = "eyrie_tailnet")]
    unsafe extern "C" {
        fn EyrieTailnetStart(dir: *const c_char, host: *const c_char) -> *mut c_char;
        fn EyrieTailnetState() -> *mut c_char;
        fn EyrieTailnetDial(addr: *const c_char, fd: c_int, timeout_ms: c_int) -> *mut c_char;
        fn EyrieTailnetPeer(addr: *const c_char) -> *mut c_char;
        fn EyrieTailnetLogout() -> *mut c_char;
        fn EyrieTailnetFree(p: *mut c_char);
    }

    /// L'échec du démarrage, s'il y en a eu un : l'état le montre.
    static START_ERROR: Mutex<Option<String>> = Mutex::new(None);

    /// La chaîne rendue par la bibliothèque (NULL : rien), libérée.
    fn take(p: *mut c_char) -> Option<String> {
        if p.is_null() {
            return None;
        }
        // SAFETY : chaîne terminée par zéro, allouée par la bibliothèque et rendue à elle une seule fois.
        let s = unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned();
        unsafe { EyrieTailnetFree(p) };
        Some(s)
    }

    pub fn start(dir: &Path, hostname: &str) -> Result<(), String> {
        *START_ERROR.lock().unwrap_or_else(|e| e.into_inner()) = None;
        let d = CString::new(dir.to_string_lossy().as_bytes()).map_err(|e| e.to_string())?;
        let h = CString::new(hostname).map_err(|e| e.to_string())?;
        // SAFETY : chaînes valables pendant l'appel ; la bibliothèque les copie.
        let r = match take(unsafe { EyrieTailnetStart(d.as_ptr(), h.as_ptr()) }) {
            None => Ok(()),
            Some(e) => Err(e),
        };
        *START_ERROR.lock().unwrap_or_else(|e| e.into_inner()) = r.clone().err();
        r
    }

    pub fn state() -> Tailnet {
        if let Some(e) = START_ERROR.lock().unwrap_or_else(|e| e.into_inner()).clone() {
            return Tailnet::Failed(e);
        }
        // SAFETY : sans argument ; la chaîne rendue est libérée par `take`.
        take(unsafe { EyrieTailnetState() }).map(|j| Tailnet::parse(&j)).unwrap_or(Tailnet::Starting)
    }

    pub fn dial(addr: &str, timeout: Duration) -> Result<Stream, String> {
        match state() {
            Tailnet::Running { .. } => {}
            Tailnet::Failed(e) => return Err(format!("Tailscale : {e}")),
            Tailnet::NeedsLogin(_) => return Err("ce téléphone n'est pas connecté à Tailscale".into()),
            _ => return Err("Tailscale démarre sur ce téléphone".into()),
        }
        let (mine, theirs) = UnixStream::pair().map_err(|e| e.to_string())?;
        let a = CString::new(addr).map_err(|e| e.to_string())?;
        let ms = timeout.as_millis().min(c_int::MAX as u128) as c_int;
        // SAFETY : la bibliothèque devient propriétaire de `theirs` (fermé par elle en cas d'échec).
        match take(unsafe { EyrieTailnetDial(a.as_ptr(), theirs.into_raw_fd(), ms) }) {
            None => Ok(Stream::Unix(mine)),
            Some(e) => Err(format!("{addr} : {e}")),
        }
    }

    pub fn peer(addr: &str) -> Option<Peer> {
        let a = CString::new(addr).ok()?;
        // SAFETY : chaîne valable pendant l'appel ; la chaîne rendue est libérée par `take`.
        take(unsafe { EyrieTailnetPeer(a.as_ptr()) }).and_then(|j| Peer::parse(&j))
    }

    pub fn logout() -> Result<(), String> {
        // SAFETY : sans argument ; la chaîne rendue est libérée par `take`.
        match take(unsafe { EyrieTailnetLogout() }) {
            None => Ok(()),
            Some(e) => Err(e),
        }
    }
}

#[cfg(not(target_os = "android"))]
mod node {
    use std::path::Path;
    use std::time::Duration;

    use suite_mobile::client::{self, Stream};

    use super::{Peer, Tailnet};

    pub fn start(_: &Path, _: &str) -> Result<(), String> {
        Ok(())
    }

    pub fn state() -> Tailnet {
        Tailnet::Absent
    }

    pub fn dial(addr: &str, timeout: Duration) -> Result<Stream, String> {
        client::dial_tcp(addr, timeout)
    }

    pub fn peer(_: &str) -> Option<Peer> {
        None
    }

    pub fn logout() -> Result<(), String> {
        Ok(())
    }
}

/// Démarre le nœud (une fois ; de nouveau après un échec), son état dans `dir`, dans un fil : c'est lent. Un échec
/// se lit dans [`state`].
pub fn start(dir: PathBuf, hostname: String) {
    let _ = std::thread::Builder::new().name("eyrie-tailnet".into()).spawn(move || {
        if let Err(e) = node::start(&dir, &hostname) {
            eprintln!("tailnet : {e}");
        }
    });
}

pub fn state() -> Tailnet {
    node::state()
}

/// Se connecte à `addr` (`hôte:port` du tailnet) par le nœud de l'application.
pub fn dial(addr: &str, timeout: Duration) -> Result<Stream, String> {
    node::dial(addr, timeout)
}

/// Ce que le nœud sait du PC d'adresse `addr` (celle que la liaison compose) ; `None` : le nœud n'est pas en ligne
/// (ce qu'il sait du PC serait périmé), son état est illisible, ou il n'y a pas de nœud (bureau). Lent (3 s au plus) :
/// jamais dans le fil de l'interface.
pub fn peer(addr: &str) -> Option<Peer> {
    node::peer(addr)
}

/// Déconnecte le téléphone du compte Tailscale ; un nouveau lien de connexion suit.
pub fn logout() -> Result<(), String> {
    node::logout()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_node_state_is_read_from_the_library() {
        assert_eq!(Tailnet::parse(r#"{"state":"starting"}"#), Tailnet::Starting);
        assert_eq!(
            Tailnet::parse(r#"{"state":"needs_login","url":"https://login.tailscale.com/a/x"}"#),
            Tailnet::NeedsLogin("https://login.tailscale.com/a/x".into())
        );
        let running = Tailnet::parse(r#"{"state":"running","name":"eyrie-pixel-8.tail1.ts.net","ip":"100.1.2.3"}"#);
        assert!(running.ready());
        assert_eq!(running.display(), (3, "Sur le tailnet : eyrie-pixel-8".into()));
        assert_eq!(Tailnet::parse(r#"{"state":"error","message":"non"}"#).display().0, 4);
        assert_eq!(Tailnet::parse("illisible"), Tailnet::Starting);
        // Pas encore de lien : rien à ouvrir, on attend.
        assert_eq!(Tailnet::NeedsLogin(String::new()).display().0, 1);
        assert!(!Tailnet::NeedsLogin("u".into()).ready() && Tailnet::Absent.ready());
    }

    #[test]
    fn what_the_node_knows_of_the_pc_is_read_from_the_library() {
        let p = Peer::parse(r#"{"connu":true,"enligne":false,"vu":1761600000}"#);
        assert_eq!(p, Some(Peer { known: true, online: false, seen: 1_761_600_000 }));
        assert!(!p.unwrap().reachable());
        assert!(Peer::parse(r#"{"connu":true,"enligne":true,"vu":0}"#).unwrap().reachable());
        assert!(!Peer::parse(r#"{"connu":false,"enligne":false}"#).unwrap().reachable(), "inconnu du tailnet");
        assert_eq!(Peer::parse("illisible"), None);
    }

    #[test]
    fn the_tailnet_name_is_made_of_the_device_name() {
        assert_eq!(hostname("Pixel 8 Pro"), "eyrie-pixel-8-pro");
        assert_eq!(hostname("  Téléphone d'Aubin !"), "eyrie-telephone-d-aubin");
        assert_eq!(hostname("—"), "eyrie-telephone");
        assert!(hostname(&"a".repeat(80)).len() <= "eyrie-".len() + 40);
    }
}
