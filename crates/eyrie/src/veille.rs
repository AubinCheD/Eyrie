//! Démarrer Eyrie depuis le téléphone quand elle est fermée : le côté d'Eyrie (le veilleur est dans `compagnon/`,
//! voir `tube.go` et `veille.go`).
//!
//! Le veilleur (`eyrie-tailnet.exe --veille`) garde le nœud d'Eyrie sur le tailnet quand elle est fermée, et démarre
//! Eyrie à la demande d'un téléphone appairé. Eyrie :
//! - s'y rattache par son tube au lieu de lancer un compagnon ([`crate::tailnet`]) ;
//! - l'inscrit à l'ouverture de session (valeur `EyrieVeille` de `HKCU\…\Run`), seulement si son dossier est à l'abri
//!   des autres comptes du poste (le veilleur lance l'`eyrie.exe` rangé à côté de lui), selon les règles du veilleur
//!   ([`exeplace`]) ;
//! - lui écrit son réglage, `mobile\veille.dpapi`, chiffré par DPAPI : actif ou non, sa clé, le verrou d'Eyrie ;
//! - donne à chaque téléphone son secret de démarrage dans `Welcome` (tiré de cette clé et de sa clé publique).
//!
//! Tout ce qui touche au système (verrou du nœud, tube, processus, registre, DPAPI, droits d'un dossier) passe par
//! [`Os`] : les essais n'approchent ni le vrai registre, ni le vrai nœud, ni les vraies données.

mod exeplace;

use std::ffi::OsString;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use suite_sdk::instance::dir_key;

/// La valeur de `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` qui lance le veilleur à l'ouverture de session.
pub const RUN_VALUE: &str = "EyrieVeille";
#[cfg(windows)]
const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
/// Où le Gestionnaire des tâches (onglet Démarrage) garde les valeurs Run désactivées.
#[cfg(windows)]
const APPROVED_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";
/// Le réglage du veilleur, dans le dossier du pont mobile.
pub const CONFIG: &str = "veille.dpapi";
/// Pourquoi le veilleur n'a pas démarré la dernière fois, dans le dossier du nœud (écrit par lui).
pub const FAILURE: &str = "veille-echec.json";
/// Le nom du tube du veilleur qui tient le nœud, dans le dossier du nœud (écrit par lui : `pipeFile` de
/// `compagnon/tube.go`).
pub const PIPE_FILE: &str = "tube.txt";
/// Entropie DPAPI de `veille.dpapi` (la même dans `compagnon/veille.go`).
pub const ENTROPY: &[u8] = b"eyrie/veille";

/// Le verrou du nœud dont l'état est dans `node` : tenu par le compagnon ou le veilleur qui le fait tourner
/// (`compagnon/verrou.go`).
pub fn lock_name(node: &Path) -> String {
    format!("Local\\EyrieTailnet:{}", dir_key(node))
}

/// Le tube du veilleur du nœud `node`, lu dans [`PIPE_FILE`] : `\\.\pipe\eyrie-tailnet-` suivi de 32 chiffres
/// hexadécimaux, tirés au hasard à chaque démarrage du veilleur (les tubes sont communs à tout le poste : un nom
/// qu'un autre compte devinerait, il le prendrait avant lui). Rien d'autre n'est ouvert : ni un tube d'un autre
/// poste, ni un fichier.
pub fn standby_pipe(node: &Path) -> Option<String> {
    let raw = std::fs::read_to_string(node.join(PIPE_FILE)).ok()?;
    let name = raw.trim();
    let suffix = name.strip_prefix(r"\\.\pipe\eyrie-tailnet-")?;
    (suffix.len() == 32 && suffix.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))).then(|| name.to_string())
}

/// Le verrou d'instance d'Eyrie sur le dossier `dir`, tenu tant qu'elle est ouverte (le veilleur le teste).
pub fn instance_lock(dir: &Path) -> String {
    format!("Local\\Eyrie-instance:{}", dir_key(dir))
}

/// Un argument de la ligne de commande, entre guillemets si besoin, selon les règles de `CommandLineToArgvW` (celles
/// que suit le compagnon en Go) : les barres obliques inverses avant un guillemet ou avant le guillemet final sont
/// doublées.
fn quote(arg: &str, always: bool) -> String {
    if !always && !arg.is_empty() && !arg.contains([' ', '\t', '"']) {
        return arg.to_string();
    }
    let mut out = String::from("\"");
    let mut slashes = 0;
    for c in arg.chars() {
        match c {
            '\\' => slashes += 1,
            '"' => {
                out.extend(std::iter::repeat_n('\\', slashes * 2 + 1));
                out.push('"');
                slashes = 0;
            }
            c => {
                out.extend(std::iter::repeat_n('\\', slashes));
                out.push(c);
                slashes = 0;
            }
        }
    }
    out.extend(std::iter::repeat_n('\\', slashes * 2));
    out.push('"');
    out
}

/// Les arguments du veilleur : ceux du compagnon d'Eyrie (même nœud, même nom, sans quoi tsnet renommerait le nœud),
/// plus `--veille --mobile`.
pub fn standby_args(node: &Path, mobile: &Path, hostname: &str) -> Vec<OsString> {
    vec![
        "--veille".into(),
        "--dir".into(),
        node.into(),
        "--mobile".into(),
        mobile.into(),
        "--hostname".into(),
        hostname.into(),
        "--port".into(),
        suite_mobile::PORT.to_string().into(),
    ]
}

/// La commande inscrite à l'ouverture de session :
/// `"<dossier>\eyrie-tailnet.exe" --veille --dir "<nœud>" --mobile "<pont>" --hostname <nom> --port 4747`.
pub fn run_line(companion: &Path, node: &Path, mobile: &Path, hostname: &str) -> String {
    let path = |p: &Path| quote(&p.to_string_lossy(), true);
    format!(
        "{} --veille --dir {} --mobile {} --hostname {} --port {}",
        path(companion),
        path(node),
        path(mobile),
        quote(hostname, false),
        suite_mobile::PORT
    )
}

/// L'exécutable que lance une commande inscrite par [`run_line`] : son premier argument, toujours entre guillemets
/// (un chemin n'en contient pas).
pub fn run_exe(line: &str) -> Option<PathBuf> {
    let rest = line.trim_start().strip_prefix('"')?;
    let (exe, _) = rest.split_once('"')?;
    (!exe.is_empty()).then(|| PathBuf::from(exe))
}

/// `mobile\veille.dpapi`, déchiffré (`wakeConfig` de `compagnon/veille.go`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Config {
    pub v: u32,
    /// Faux : le veilleur ne démarre pas Eyrie (il répond `acces-coupe`).
    pub actif: bool,
    /// La clé du veilleur (32 octets, hexadécimal) : les secrets des appareils en sont tirés.
    pub cle: String,
    /// Le verrou d'instance d'Eyrie ([`instance_lock`]).
    pub instance: String,
    /// Eyrie démarrée à distance s'ouvre réduite.
    pub reduite: bool,
}

/// `veille-echec.json` : une ligne JSON.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Failure {
    /// Secondes Unix.
    pub at: u64,
    pub message: String,
}

/// Une valeur de `StartupApproved\Run` désactive la valeur Run du même nom quand son premier octet est impair (`03`,
/// `07` : désactivée dans le Gestionnaire des tâches ; `02`, `06` : activée).
pub fn startup_disabled(approved: &[u8]) -> bool {
    approved.first().is_some_and(|b| b & 1 == 1)
}

pub fn read_failure(node: &Path) -> Option<Failure> {
    serde_json::from_slice(&std::fs::read(node.join(FAILURE)).ok()?).ok()
}

/// Ce que les réglages demandent au veilleur ; comparé d'une image à l'autre, rien n'est refait sans changement.
#[derive(Clone, Debug, PartialEq)]
pub struct Want {
    /// « Démarrer Eyrie depuis le téléphone », Tailscale allumé.
    pub on: bool,
    /// « Accès depuis le téléphone » : coupé, le veilleur refuse de démarrer Eyrie (il répond `acces-coupe`).
    pub active: bool,
    /// L'Eyrie de tous les jours (son dossier par défaut). Une seule valeur Run pour le compte : une Eyrie d'essai
    /// (`EYRIE_DATA_DIR`) n'y touche jamais.
    pub main: bool,
    pub hostname: String,
    /// Le dossier du nœud (`--dir`) et celui du pont mobile (`--mobile`).
    pub node: PathBuf,
    pub mobile: PathBuf,
    /// Le verrou d'instance d'Eyrie ([`instance_lock`]).
    pub instance: String,
}

/// Le réglage du veilleur tel qu'Eyrie l'a appliqué : valeur Run, `veille.dpapi`, clé.
#[derive(Default)]
pub struct Veille {
    applied: Option<Want>,
    /// La clé du veilleur, lue dans `veille.dpapi` ou créée une fois.
    key: Option<[u8; 32]>,
    /// Le compagnon que le veilleur fait tourner : `Some` quand il est voulu et possible.
    companion: Option<PathBuf>,
    /// Pourquoi le veilleur voulu n'est pas possible (emplacement d'Eyrie, compagnon introuvable…).
    pub problem: Option<String>,
    /// Le veilleur voulu, que cette Eyrie ne peut pas lancer, mais qu'une autre `eyrie.exe` a inscrit à l'ouverture de
    /// session depuis un dossier à l'abri (une Eyrie de `target\debug` à côté de celle de `target\release`) : son
    /// exécutable. Il reste inscrit, son réglage suit l'accès depuis le téléphone, sa clé sert aux téléphones.
    pub kept: Option<PathBuf>,
    /// Ce qui n'a pas pu être écrit (registre, `veille.dpapi`).
    pub error: Option<String>,
    /// La commande inscrite à l'ouverture de session, lue après écriture.
    pub run: Option<String>,
}

/// Le veilleur est-il possible : le compagnon d'Eyrie, rangé à côté d'`eyrie.exe` (le veilleur ne démarre que
/// celui-là, et ne sert que lui), dans un dossier à l'abri des autres comptes du poste ?
fn usable(os: &dyn Os, want: &Want) -> Result<PathBuf, String> {
    if !want.main {
        return Err("seule l'Eyrie de tous les jours (son dossier par défaut) se démarre depuis le téléphone".into());
    }
    let companion = os
        .companion()
        .ok_or_else(|| format!("{} introuvable : compilez-le avec tools\\compagnon.ps1", crate::tailnet::COMPANION))?;
    if !companion.file_name().is_some_and(|n| n.eq_ignore_ascii_case(crate::tailnet::COMPANION)) {
        return Err(format!(
            "le compagnon trouvé est celui de Fawkes ({}) ; le veilleur est celui d'Eyrie, {}",
            companion.display(),
            crate::tailnet::COMPANION
        ));
    }
    let eyrie = os.eyrie().ok_or("emplacement d'eyrie.exe illisible")?;
    let (Some(here), Some(there)) = (eyrie.parent(), companion.parent()) else {
        return Err("emplacement d'eyrie.exe illisible".into());
    };
    if dir_key(here) != dir_key(there) {
        return Err(format!(
            "le veilleur démarre l'eyrie.exe rangé à côté de lui : {} doit être dans {} (il est dans {})",
            crate::tailnet::COMPANION,
            here.display(),
            there.display()
        ));
    }
    os.place_safe(&companion)?;
    os.place_safe(&eyrie)?;
    Ok(companion)
}

/// Le veilleur que lance la commande inscrite `line`, s'il se garde : le compagnon d'Eyrie, lui et l'`eyrie.exe`
/// rangé à côté de lui à l'abri des autres comptes du poste (un exécutable effacé ne l'est pas).
fn registered(os: &dyn Os, line: &str) -> Option<PathBuf> {
    let exe = run_exe(line)?;
    if !exe.file_name().is_some_and(|n| n.eq_ignore_ascii_case(crate::tailnet::COMPANION)) {
        return None;
    }
    os.place_safe(&exe).ok()?;
    os.place_safe(&exe.with_file_name("eyrie.exe")).ok()?;
    Some(exe)
}

/// La clé du veilleur gardée dans `veille.dpapi`, si elle s'y lit.
fn read_key(os: &dyn Os, path: &Path) -> Option<[u8; 32]> {
    let plain = os.unseal(&std::fs::read(path).ok()?).ok()?;
    let config: Config = serde_json::from_slice(&plain).ok()?;
    crate::mobile::store::from_hex(&config.cle)?.try_into().ok()
}

impl Veille {
    /// Applique les réglages s'ils ont changé depuis la dernière fois : la valeur Run (inscrite pour un veilleur
    /// voulu et possible, retirée quand il n'est plus voulu) et `veille.dpapi` (écrit dès qu'il sert, ou réécrit
    /// `actif: false` s'il existe). Les exécutables ne sont cherchés qu'alors. Une Eyrie qui ne peut pas lancer le
    /// veilleur voulu (rangée ailleurs que lui) laisse celui qu'une autre a inscrit ([`Veille::kept`]).
    pub fn apply(&mut self, os: &dyn Os, want: &Want) {
        if self.applied.as_ref() == Some(want) {
            return;
        }
        self.applied = Some(want.clone());
        self.problem = None;
        self.error = None;
        self.companion = None;
        self.kept = None;
        if want.on {
            match usable(os, want) {
                Ok(c) => self.companion = Some(c),
                Err(e) => self.problem = Some(e),
            }
        }
        if want.main {
            let current = os.run_value();
            if want.on && self.companion.is_none() {
                self.kept = current.as_deref().and_then(|l| registered(os, l));
            }
            let line = match (&self.companion, &self.kept) {
                (Some(c), _) => Some(run_line(c, &want.node, &want.mobile, &want.hostname)),
                (None, Some(_)) => current.clone(),
                (None, None) => None,
            };
            if current != line
                && let Err(e) = os.set_run_value(line.as_deref())
            {
                self.error = Some(format!("démarrage à l'ouverture de session : {e}"));
            }
            self.run = os.run_value();
        }
        let path = want.mobile.join(CONFIG);
        if (self.companion.is_some() || self.kept.is_some() || path.exists())
            && let Err(e) = self.write_config(os, want, &path)
        {
            self.error = Some(format!("{CONFIG} non enregistré : {e}"));
        }
    }

    fn write_config(&mut self, os: &dyn Os, want: &Want, path: &Path) -> Result<(), String> {
        let key = match self.key {
            Some(k) => k,
            None => *self.key.insert(read_key(os, path).unwrap_or_else(suite_mobile::pairing::random::<32>)),
        };
        let config = Config {
            v: 1,
            actif: (self.companion.is_some() || self.kept.is_some()) && want.active,
            cle: crate::mobile::store::hex(&key),
            instance: want.instance.clone(),
            reduite: true,
        };
        let sealed = os.seal(&serde_json::to_vec(&config).map_err(|e| e.to_string())?)?;
        let dir = path.parent().unwrap_or(Path::new("."));
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        // Remplacé d'un coup : le veilleur le relit à chaque demande (il l'ouvre en partageant la suppression).
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, sealed).and_then(|_| std::fs::rename(&tmp, path)).map_err(|e| e.to_string())
    }

    /// Réapplique à la prochaine image (« Réessayer » : l'exécutable a pu être compilé ou déplacé).
    pub fn retry(&mut self) {
        self.applied = None;
    }

    /// Le veilleur voulu et possible : son exécutable et ses arguments.
    pub fn standby(&self) -> Option<(PathBuf, Vec<OsString>)> {
        let want = self.applied.as_ref()?;
        Some((self.companion.clone()?, standby_args(&want.node, &want.mobile, &want.hostname)))
    }

    /// La clé dont les téléphones tirent leur secret de démarrage : seulement pour un veilleur voulu, et possible ou
    /// gardé.
    pub fn key(&self) -> Option<[u8; 32]> {
        (self.companion.is_some() || self.kept.is_some()).then_some(self.key).flatten()
    }
}

/// Un tube ouvert vers le veilleur : ses lignes d'état, nos consignes.
pub struct Pipe {
    pub reader: Box<dyn Read + Send>,
    pub writer: Box<dyn Write + Send>,
}

/// Pourquoi le tube du veilleur ne s'ouvre pas.
#[derive(Debug, PartialEq)]
pub enum Attach {
    /// Pas de tube (ou occupé un instant) : un veilleur peut être en train de démarrer, ou de s'arrêter.
    Absent,
    /// Un tube tenu par un autre compte ou un autre exécutable que le compagnon d'Eyrie.
    Refused(String),
}

/// Un processus lancé : encore là ?
pub trait Process: Send {
    fn running(&mut self) -> bool;
    fn kill(&mut self);
}

/// Le compagnon lancé par Eyrie : son entrée (les consignes ; fermée, il s'arrête), sa sortie (les lignes d'état).
pub struct Spawned {
    pub stdin: Box<dyn Write + Send>,
    pub stdout: Box<dyn Read + Send>,
    pub process: Box<dyn Process>,
}

/// Ce que le compagnon et le veilleur demandent au système.
pub trait Os: Send + Sync {
    /// Le compagnon à lancer ([`crate::tailnet::find_companion`]).
    fn companion(&self) -> Option<PathBuf>;
    /// L'exécutable d'Eyrie.
    fn eyrie(&self) -> Option<PathBuf>;
    /// Un compagnon (lancé par Eyrie ou veilleur) tient-il le nœud de ce verrou ?
    fn node_held(&self, lock: &str) -> bool;
    /// Ouvre le tube du veilleur du nœud `node` ([`standby_pipe`], relu à chaque fois : le veilleur l'écrit une fois
    /// son tube ouvert) en se laissant identifier (le veilleur vérifie qui l'ouvre), et vérifie qui le tient : le même
    /// compte Windows, l'exécutable `companion`.
    fn attach(&self, node: &Path, companion: &Path) -> Result<Pipe, Attach>;
    /// Lance le compagnon d'Eyrie, sans fenêtre, entrée et sortie par des tubes.
    fn spawn_companion(&self, exe: &Path, args: &[OsString]) -> io::Result<Spawned>;
    /// Lance le veilleur, détaché d'Eyrie : il lui survit, piloté par son tube.
    fn spawn_standby(&self, exe: &Path, args: &[OsString]) -> io::Result<Box<dyn Process>>;
    /// La commande inscrite à l'ouverture de session ([`RUN_VALUE`]).
    fn run_value(&self) -> Option<String>;
    /// L'inscrit, ou la retire (`None`).
    fn set_run_value(&self, line: Option<&str>) -> Result<(), String>;
    /// L'utilisateur l'a-t-il désactivée dans le Gestionnaire des tâches (onglet Démarrage) ? Eyrie le laisse faire.
    fn run_disabled(&self) -> bool;
    /// Chiffre pour le compte de l'utilisateur sur ce poste (DPAPI, entropie [`ENTROPY`]).
    fn seal(&self, plain: &[u8]) -> Result<Vec<u8>, String>;
    fn unseal(&self, sealed: &[u8]) -> Result<Vec<u8>, String>;
    /// L'exécutable, son dossier et les dossiers au-dessus sont-ils à l'abri des autres comptes du poste, selon les
    /// règles du veilleur ([`exeplace`]) ? Sinon, pourquoi.
    fn place_safe(&self, exe: &Path) -> Result<(), String>;
    /// Eyrie tourne-t-elle en administrateur (jeton élevé) ? Le veilleur qu'elle lancerait le serait aussi.
    fn elevated(&self) -> bool;
}

/// Le vrai système.
pub struct System;

impl Process for std::process::Child {
    fn running(&mut self) -> bool {
        matches!(self.try_wait(), Ok(None))
    }

    fn kill(&mut self) {
        let _ = std::process::Child::kill(self);
        let _ = self.wait();
    }
}

impl Os for System {
    fn companion(&self) -> Option<PathBuf> {
        crate::tailnet::find_companion()
    }

    fn eyrie(&self) -> Option<PathBuf> {
        std::env::current_exe().ok()
    }

    fn node_held(&self, lock: &str) -> bool {
        #[cfg(windows)]
        unsafe {
            use windows_sys::Win32::Foundation::{CloseHandle, ERROR_FILE_NOT_FOUND, GetLastError};
            use windows_sys::Win32::System::Threading::{OpenMutexW, SYNCHRONIZATION_SYNCHRONIZE};
            let name: Vec<u16> = lock.encode_utf16().chain([0]).collect();
            let h = OpenMutexW(SYNCHRONIZATION_SYNCHRONIZE, 0, name.as_ptr());
            if !h.is_null() {
                CloseHandle(h);
                return true;
            }
            // Un verrou qui existe mais ne s'ouvre pas (droits) est tenu aussi.
            GetLastError() != ERROR_FILE_NOT_FOUND
        }
        #[cfg(not(windows))]
        {
            let _ = lock;
            false
        }
    }

    #[cfg(windows)]
    fn attach(&self, node: &Path, companion: &Path) -> Result<Pipe, Attach> {
        // Pas encore de nom (un veilleur qui démarre) : comme pas de tube.
        win::open_pipe(&standby_pipe(node).ok_or(Attach::Absent)?, companion)
    }

    #[cfg(not(windows))]
    fn attach(&self, _node: &Path, _companion: &Path) -> Result<Pipe, Attach> {
        Err(Attach::Absent)
    }

    fn spawn_companion(&self, exe: &Path, args: &[OsString]) -> io::Result<Spawned> {
        use std::process::{Command, Stdio};
        let mut cmd = Command::new(exe);
        cmd.args(args).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW);
        }
        let mut child = cmd.spawn()?;
        let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
            Process::kill(&mut child);
            return Err(io::Error::other("entrée ou sortie du compagnon indisponible"));
        };
        Ok(Spawned { stdin: Box::new(stdin), stdout: Box::new(stdout), process: Box::new(child) })
    }

    fn spawn_standby(&self, exe: &Path, args: &[OsString]) -> io::Result<Box<dyn Process>> {
        use std::process::{Command, Stdio};
        let mut cmd = Command::new(exe);
        cmd.args(args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        if let Some(dir) = exe.parent() {
            cmd.current_dir(dir);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            use windows_sys::Win32::System::Threading::{CREATE_NEW_PROCESS_GROUP, DETACHED_PROCESS};
            cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
        }
        Ok(Box::new(cmd.spawn()?))
    }

    fn run_value(&self) -> Option<String> {
        #[cfg(windows)]
        return win::read_run();
        #[cfg(not(windows))]
        None
    }

    fn set_run_value(&self, line: Option<&str>) -> Result<(), String> {
        #[cfg(windows)]
        return win::write_run(line);
        #[cfg(not(windows))]
        {
            let _ = line;
            Err("seulement sous Windows".into())
        }
    }

    fn run_disabled(&self) -> bool {
        #[cfg(windows)]
        return win::read_approved().is_some_and(|v| startup_disabled(&v));
        #[cfg(not(windows))]
        false
    }

    fn seal(&self, plain: &[u8]) -> Result<Vec<u8>, String> {
        crate::mobile::store::dpapi::protect(plain, ENTROPY)
    }

    fn unseal(&self, sealed: &[u8]) -> Result<Vec<u8>, String> {
        crate::mobile::store::dpapi::unprotect(sealed, ENTROPY)
    }

    fn place_safe(&self, exe: &Path) -> Result<(), String> {
        // Celles du veilleur (propriétaire, droits sous condition, dossiers au-dessus), puis celles de Fawkes.
        exeplace::check(exe).and_then(|()| fawkes::exeplace::check(exe))
    }

    fn elevated(&self) -> bool {
        #[cfg(windows)]
        return win::elevated();
        #[cfg(not(windows))]
        false
    }
}

#[cfg(windows)]
mod win {
    use std::fs::File;
    use std::io::{self, Read, Write};
    use std::os::windows::io::AsRawHandle;
    use std::path::Path;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    use windows_sys::Win32::Foundation::{CloseHandle, ERROR_FILE_NOT_FOUND, ERROR_SUCCESS, HANDLE};
    use windows_sys::Win32::Security::{
        EqualSid, GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TOKEN_USER, TokenElevation, TokenUser,
    };
    use windows_sys::Win32::Storage::FileSystem::{BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle};
    use windows_sys::Win32::System::Pipes::{GetNamedPipeServerProcessId, PeekNamedPipe};
    use windows_sys::Win32::System::Registry::{
        HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_BINARY, RRF_RT_REG_SZ, RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW,
    };
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
        QueryFullProcessImageNameW,
    };

    use super::{APPROVED_KEY, Attach, Pipe, RUN_KEY, RUN_VALUE};

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain([0]).collect()
    }

    /// Ouvre le tube `pipe` en se laissant identifier, et vérifie qui le tient (voir [`super::Os::attach`]).
    pub fn open_pipe(pipe: &str, companion: &Path) -> Result<Pipe, Attach> {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Foundation::ERROR_PIPE_BUSY;
        use windows_sys::Win32::Storage::FileSystem::SECURITY_IDENTIFICATION;
        // `security_qos_flags` ajoute SECURITY_SQOS_PRESENT : au niveau identification, le veilleur lit notre compte
        // sur le tube, même pour une Eyrie lancée en administrateur.
        let file = match std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .security_qos_flags(SECURITY_IDENTIFICATION)
            .open(pipe)
        {
            Ok(f) => f,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Err(Attach::Absent),
            Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY as i32) => return Err(Attach::Absent),
            Err(e) => return Err(Attach::Refused(format!("tube du veilleur : {e}"))),
        };
        check_server(&file, companion).map_err(Attach::Refused)?;
        let reader = file.try_clone().map_err(|e| Attach::Refused(format!("tube du veilleur : {e}")))?;
        let closed = Arc::new(AtomicBool::new(false));
        Ok(Pipe {
            reader: Box::new(PeekReader { file: reader, closed: closed.clone() }),
            writer: Box::new(PipeWriter { file, closed }),
        })
    }

    /// Le compte (TOKEN_USER) d'un processus, dans un tampon aligné.
    pub fn token_user(process: HANDLE) -> Result<Vec<u64>, String> {
        // SAFETY : jeton fermé avant de sortir ; tampon aligné pour TOKEN_USER, de la taille demandée.
        unsafe {
            let mut token: HANDLE = std::ptr::null_mut();
            if OpenProcessToken(process, TOKEN_QUERY, &mut token) == 0 {
                return Err(io::Error::last_os_error().to_string());
            }
            let mut len = 0u32;
            GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut len);
            let mut buf = vec![0u64; (len as usize).div_ceil(8).max(1)];
            let ok = GetTokenInformation(token, TokenUser, buf.as_mut_ptr().cast(), len, &mut len);
            CloseHandle(token);
            if ok == 0 {
                return Err(io::Error::last_os_error().to_string());
            }
            Ok(buf)
        }
    }

    /// Le jeton d'Eyrie est-il élevé (lancée « en tant qu'administrateur ») ? Illisible : on le suppose, par prudence.
    pub fn elevated() -> bool {
        // SAFETY : jeton de notre processus, fermé avant de sortir ; structure de la taille annoncée.
        unsafe {
            let mut token: HANDLE = std::ptr::null_mut();
            if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
                return true;
            }
            let mut elevation = TOKEN_ELEVATION { TokenIsElevated: 0 };
            let mut len = 0u32;
            let ok = GetTokenInformation(
                token,
                TokenElevation,
                (&mut elevation as *mut TOKEN_ELEVATION).cast(),
                std::mem::size_of::<TOKEN_ELEVATION>() as u32,
                &mut len,
            );
            CloseHandle(token);
            ok == 0 || elevation.TokenIsElevated != 0
        }
    }

    /// Le serveur du tube : le même compte que nous, l'exécutable `companion` (les fichiers sont comparés, pas les
    /// chemins : casse, noms courts).
    pub fn check_server(pipe: &File, companion: &Path) -> Result<(), String> {
        let mut pid = 0u32;
        // SAFETY : handle du tube ouvert ; le processus est fermé avant de sortir.
        unsafe {
            if GetNamedPipeServerProcessId(pipe.as_raw_handle(), &mut pid) == 0 {
                return Err(format!("processus du tube illisible : {}", io::Error::last_os_error()));
            }
            let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if process.is_null() {
                return Err(format!("processus {pid} du tube : {}", io::Error::last_os_error()));
            }
            let checked = (|| {
                let theirs = token_user(process).map_err(|e| format!("compte du processus {pid} illisible : {e}"))?;
                let mine = token_user(GetCurrentProcess()).map_err(|e| format!("compte d'Eyrie illisible : {e}"))?;
                let (theirs, mine) = (&*(theirs.as_ptr() as *const TOKEN_USER), &*(mine.as_ptr() as *const TOKEN_USER));
                if EqualSid(theirs.User.Sid, mine.User.Sid) == 0 {
                    return Err(format!("le tube est tenu par le processus {pid} d'un autre compte"));
                }
                let mut buf = vec![0u16; 32_768];
                let mut n = buf.len() as u32;
                if QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut n) == 0 {
                    return Err(format!("exécutable du processus {pid} illisible : {}", io::Error::last_os_error()));
                }
                let image = String::from_utf16_lossy(&buf[..n as usize]);
                if !same_file(Path::new(&image), companion) {
                    return Err(format!("le tube est tenu par {image}, pas par {}", companion.display()));
                }
                Ok(())
            })();
            CloseHandle(process);
            checked
        }
    }

    fn file_id(path: &Path) -> Option<(u32, u32, u32)> {
        let f = File::open(path).ok()?;
        let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
        // SAFETY : handle ouvert, structure de la bonne taille.
        if unsafe { GetFileInformationByHandle(f.as_raw_handle(), &mut info) } == 0 {
            return None;
        }
        Some((info.dwVolumeSerialNumber, info.nFileIndexHigh, info.nFileIndexLow))
    }

    fn same_file(a: &Path, b: &Path) -> bool {
        matches!((file_id(a), file_id(b)), (Some(x), Some(y)) if x == y)
    }

    /// Le temps entre deux coups d'œil au tube.
    const PEEK_EVERY: Duration = Duration::from_millis(100);

    /// La lecture du tube, sans lecture en attente : sur un handle synchrone, une lecture bloquée retiendrait nos
    /// écritures (Windows les met à la suite). Elle ne lit que ce qui est arrivé (PeekNamedPipe) ; fin quand le
    /// veilleur ferme le tube, ou quand Eyrie le lâche.
    pub struct PeekReader {
        pub file: File,
        pub closed: Arc<AtomicBool>,
    }

    impl Read for PeekReader {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            loop {
                if self.closed.load(Ordering::Relaxed) || buf.is_empty() {
                    return Ok(0);
                }
                let mut avail = 0u32;
                // SAFETY : handle du tube ouvert ; rien n'est copié (tampon nul).
                let ok = unsafe {
                    PeekNamedPipe(
                        self.file.as_raw_handle(),
                        std::ptr::null_mut(),
                        0,
                        std::ptr::null_mut(),
                        &mut avail,
                        std::ptr::null_mut(),
                    )
                };
                if ok == 0 {
                    // Tube fermé par le veilleur (ERROR_BROKEN_PIPE) : fin.
                    return Ok(0);
                }
                if avail > 0 {
                    let n = buf.len().min(avail as usize);
                    return self.file.read(&mut buf[..n]);
                }
                std::thread::sleep(PEEK_EVERY);
            }
        }
    }

    /// L'écriture du tube ; lâchée, la lecture s'arrête aussi (le tube se ferme avec son dernier handle).
    pub struct PipeWriter {
        pub file: File,
        pub closed: Arc<AtomicBool>,
    }

    impl Write for PipeWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.file.write(buf)
        }

        fn flush(&mut self) -> io::Result<()> {
            self.file.flush()
        }
    }

    impl Drop for PipeWriter {
        fn drop(&mut self) {
            self.closed.store(true, Ordering::Relaxed);
        }
    }

    /// La valeur `EyrieVeille` de `StartupApproved\Run` (le Gestionnaire des tâches l'écrit), si elle existe.
    pub fn read_approved() -> Option<Vec<u8>> {
        let (key, name) = (wide(APPROVED_KEY), wide(RUN_VALUE));
        let mut buf = [0u8; 64];
        let mut len = buf.len() as u32;
        // SAFETY : chaînes terminées par un zéro ; le tampon a la taille annoncée.
        let r = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                name.as_ptr(),
                RRF_RT_REG_BINARY,
                std::ptr::null_mut(),
                buf.as_mut_ptr().cast(),
                &mut len,
            )
        };
        (r == ERROR_SUCCESS).then(|| buf[..(len as usize).min(buf.len())].to_vec())
    }

    pub fn read_run() -> Option<String> {
        let (key, name) = (wide(RUN_KEY), wide(RUN_VALUE));
        let mut len = 0u32;
        // SAFETY : chaînes terminées par un zéro ; le tampon a la taille annoncée.
        unsafe {
            let size = RegGetValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                name.as_ptr(),
                RRF_RT_REG_SZ,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut len,
            );
            if size != ERROR_SUCCESS || len == 0 {
                return None;
            }
            let mut buf = vec![0u16; (len as usize).div_ceil(2)];
            let r = RegGetValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                name.as_ptr(),
                RRF_RT_REG_SZ,
                std::ptr::null_mut(),
                buf.as_mut_ptr().cast(),
                &mut len,
            );
            if r != ERROR_SUCCESS {
                return None;
            }
            let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
            Some(String::from_utf16_lossy(&buf[..end]))
        }
    }

    pub fn write_run(line: Option<&str>) -> Result<(), String> {
        let (key, name) = (wide(RUN_KEY), wide(RUN_VALUE));
        // SAFETY : chaînes terminées par un zéro ; la donnée est la chaîne avec son zéro final.
        let r = unsafe {
            match line {
                Some(line) => {
                    let data = wide(line);
                    RegSetKeyValueW(
                        HKEY_CURRENT_USER,
                        key.as_ptr(),
                        name.as_ptr(),
                        REG_SZ,
                        data.as_ptr().cast(),
                        (data.len() * 2) as u32,
                    )
                }
                None => match RegDeleteKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), name.as_ptr()) {
                    ERROR_FILE_NOT_FOUND => ERROR_SUCCESS,
                    r => r,
                },
            }
        };
        if r == ERROR_SUCCESS { Ok(()) } else { Err(io::Error::from_raw_os_error(r as i32).to_string()) }
    }
}

/// Un faux système, pour les essais : un registre, un verrou de nœud et des tubes en mémoire. Le « veilleur » et le
/// « compagnon » sont l'autre bout des tubes, que l'essai tient ([`fake::End`]).
#[cfg(test)]
pub mod fake {
    use std::ffi::OsString;
    use std::io::{self, BufRead, BufReader, PipeReader, PipeWriter, Write};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use super::{Attach, Os, Pipe, Process, Spawned};

    /// L'autre bout d'un tube ou d'un compagnon : ce qu'Eyrie écrit, et de quoi lui répondre.
    pub struct End {
        pub from_eyrie: BufReader<PipeReader>,
        pub to_eyrie: PipeWriter,
        pub alive: Arc<AtomicBool>,
    }

    impl End {
        /// La ligne suivante d'Eyrie (`None` : elle a fermé).
        pub fn line(&mut self) -> Option<String> {
            let mut s = String::new();
            (self.from_eyrie.read_line(&mut s).ok()? > 0).then(|| s.trim_end().to_string())
        }

        pub fn say(&mut self, line: &str) {
            writeln!(self.to_eyrie, "{line}").unwrap();
        }
    }

    struct Proc(Arc<AtomicBool>);

    /// L'entrée d'un faux compagnon : fermée, il s'arrête (comme le vrai).
    struct Stdin(PipeWriter, Arc<AtomicBool>);

    impl Write for Stdin {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.write(buf)
        }

        fn flush(&mut self) -> io::Result<()> {
            self.0.flush()
        }
    }

    impl Drop for Stdin {
        fn drop(&mut self) {
            self.1.store(false, Ordering::Relaxed);
        }
    }

    impl Process for Proc {
        fn running(&mut self) -> bool {
            self.0.load(Ordering::Relaxed)
        }

        fn kill(&mut self) {
            self.0.store(false, Ordering::Relaxed);
        }
    }

    #[derive(Default)]
    pub struct Fake {
        pub companion: Mutex<Option<PathBuf>>,
        pub eyrie: Mutex<Option<PathBuf>>,
        /// Le verrou du nœud est tenu.
        pub held: Mutex<bool>,
        /// Les clients que le tube du veilleur accepte encore (un à la fois ; aucun une fois arrêté).
        pub piped: Mutex<u32>,
        /// Les tubes ouverts par Eyrie.
        pub attaches: Mutex<u32>,
        /// Le tube est tenu par un autre exécutable.
        pub foreign: Mutex<Option<String>>,
        pub run: Mutex<Option<String>>,
        pub run_writes: Mutex<u32>,
        /// La valeur Run est désactivée dans le Gestionnaire des tâches.
        pub run_disabled: Mutex<bool>,
        /// Le dossier de l'exécutable n'est pas à l'abri.
        pub unsafe_place: Mutex<Option<String>>,
        /// Le veilleur lancé sort aussitôt sans prendre le nœud (son échec est dans veille-echec.json).
        pub standby_fails: Mutex<bool>,
        /// Eyrie tourne en administrateur.
        pub elevated: Mutex<bool>,
        /// Le compagnon ne s'arrête pas quand son entrée se ferme : Eyrie doit le tuer au bout de 3 s.
        pub stubborn: Mutex<bool>,
        pub standby_launches: Mutex<Vec<(PathBuf, Vec<OsString>)>>,
        pub companion_launches: Mutex<Vec<(PathBuf, Vec<OsString>)>>,
        /// Les bouts des tubes du veilleur, puis ceux des compagnons, dans l'ordre d'ouverture.
        pub pipes: Mutex<Vec<End>>,
        pub children: Mutex<Vec<End>>,
    }

    fn ends() -> (PipeReader, PipeWriter, End) {
        let (eyrie_reads, standby_writes) = io::pipe().unwrap();
        let (standby_reads, eyrie_writes) = io::pipe().unwrap();
        let end = End {
            from_eyrie: BufReader::new(standby_reads),
            to_eyrie: standby_writes,
            alive: Arc::new(AtomicBool::new(true)),
        };
        (eyrie_reads, eyrie_writes, end)
    }

    impl Fake {
        /// Attend (10 s au plus) le bout numéro `n` de la liste, et le prend.
        fn wait_end(list: &Mutex<Vec<End>>, n: usize) -> End {
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                {
                    let mut l = list.lock().unwrap();
                    if l.len() > n {
                        return l.remove(n);
                    }
                }
                assert!(Instant::now() < deadline, "rien d'ouvert");
                std::thread::sleep(Duration::from_millis(10));
            }
        }

        /// Le premier tube du veilleur ouvert par Eyrie (attendu).
        pub fn pipe(&self) -> End {
            Self::wait_end(&self.pipes, 0)
        }

        /// Le premier compagnon lancé par Eyrie (attendu).
        pub fn child(&self) -> End {
            Self::wait_end(&self.children, 0)
        }

        /// Le veilleur prend le nœud et ouvre son tube (pour un client).
        pub fn standby_up(&self) {
            *self.held.lock().unwrap() = true;
            *self.piped.lock().unwrap() = 1;
        }

        /// Le veilleur sort : tube fermé, nœud libéré.
        pub fn standby_down(&self) {
            *self.held.lock().unwrap() = false;
            *self.piped.lock().unwrap() = 0;
        }
    }

    impl Os for Fake {
        fn companion(&self) -> Option<PathBuf> {
            self.companion.lock().unwrap().clone()
        }

        fn eyrie(&self) -> Option<PathBuf> {
            self.eyrie.lock().unwrap().clone()
        }

        fn node_held(&self, _lock: &str) -> bool {
            *self.held.lock().unwrap()
        }

        fn attach(&self, _node: &Path, _companion: &Path) -> Result<Pipe, Attach> {
            if let Some(e) = self.foreign.lock().unwrap().clone() {
                return Err(Attach::Refused(e));
            }
            {
                let mut piped = self.piped.lock().unwrap();
                if *piped == 0 {
                    return Err(Attach::Absent);
                }
                *piped -= 1;
            }
            *self.attaches.lock().unwrap() += 1;
            let (reader, writer, end) = ends();
            self.pipes.lock().unwrap().push(end);
            Ok(Pipe { reader: Box::new(reader), writer: Box::new(writer) })
        }

        fn spawn_companion(&self, exe: &Path, args: &[OsString]) -> io::Result<Spawned> {
            self.companion_launches.lock().unwrap().push((exe.to_path_buf(), args.to_vec()));
            let (stdout, stdin, end) = ends();
            let alive = end.alive.clone();
            self.children.lock().unwrap().push(end);
            let closes = if *self.stubborn.lock().unwrap() { Arc::new(AtomicBool::new(true)) } else { alive.clone() };
            Ok(Spawned {
                stdin: Box::new(Stdin(stdin, closes)),
                stdout: Box::new(stdout),
                process: Box::new(Proc(alive)),
            })
        }

        fn spawn_standby(&self, exe: &Path, args: &[OsString]) -> io::Result<Box<dyn Process>> {
            self.standby_launches.lock().unwrap().push((exe.to_path_buf(), args.to_vec()));
            let fails = *self.standby_fails.lock().unwrap();
            if !fails {
                self.standby_up();
            }
            Ok(Box::new(Proc(Arc::new(AtomicBool::new(!fails)))))
        }

        fn run_value(&self) -> Option<String> {
            self.run.lock().unwrap().clone()
        }

        fn set_run_value(&self, line: Option<&str>) -> Result<(), String> {
            *self.run.lock().unwrap() = line.map(str::to_string);
            *self.run_writes.lock().unwrap() += 1;
            Ok(())
        }

        fn run_disabled(&self) -> bool {
            *self.run_disabled.lock().unwrap()
        }

        fn seal(&self, plain: &[u8]) -> Result<Vec<u8>, String> {
            Ok([b"essai:".as_slice(), plain].concat())
        }

        fn unseal(&self, sealed: &[u8]) -> Result<Vec<u8>, String> {
            sealed.strip_prefix(b"essai:").map(<[u8]>::to_vec).ok_or_else(|| "pas scellé".into())
        }

        fn place_safe(&self, _exe: &Path) -> Result<(), String> {
            self.unsafe_place.lock().unwrap().clone().map_or(Ok(()), Err)
        }

        fn elevated(&self) -> bool {
            *self.elevated.lock().unwrap()
        }
    }
}

/// Le vrai client du tube et le vrai test du verrou, sur des noms propres à l'essai (ni le nœud ni le tube du vrai
/// veilleur) : le serveur est un tube de ce processus.
#[cfg(all(test, windows))]
mod system_tests {
    use std::fs::File;
    use std::io::{BufRead, BufReader, Write};
    use std::os::windows::io::FromRawHandle;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::mpsc;
    use std::time::Duration;

    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::Storage::FileSystem::PIPE_ACCESS_DUPLEX;
    use windows_sys::Win32::System::Pipes::{CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_TYPE_BYTE, PIPE_WAIT};

    use super::*;

    fn unique(what: &str) -> String {
        static N: AtomicU32 = AtomicU32::new(0);
        format!("eyrie-essai-{what}-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed))
    }

    /// Un tube servi par ce processus, prêt pour un client ; le bout serveur est rendu une fois le client entré.
    fn listen(name: &str) -> File {
        let w: Vec<u16> = name.encode_utf16().chain([0]).collect();
        // SAFETY : nom terminé par un zéro ; le handle rendu devient un File, qui le fermera.
        let h = unsafe {
            CreateNamedPipeW(
                w.as_ptr(),
                PIPE_ACCESS_DUPLEX,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
                1,
                4096,
                4096,
                0,
                std::ptr::null(),
            )
        };
        assert_ne!(h, INVALID_HANDLE_VALUE, "{}", io::Error::last_os_error());
        unsafe { File::from_raw_handle(h) }
    }

    /// Le serveur reconnu (même compte, même exécutable), Eyrie écrit sa consigne alors que sa lecture attend une ligne
    /// qui ne vient qu'après : une lecture bloquée sur le handle la retiendrait. Lâché, le tube se ferme.
    #[test]
    fn the_pipe_client_checks_its_server_and_writes_while_it_reads() {
        let name = format!(r"\\.\pipe\{}", unique("tube"));
        let server = listen(&name);
        let me = std::env::current_exe().unwrap();
        let Pipe { reader, mut writer } = win::open_pipe(&name, &me).expect("ce processus tient le tube");
        let serving = std::thread::spawn(move || {
            let mut r = BufReader::new(server.try_clone().unwrap());
            let mut w = server;
            let mut line = String::new();
            r.read_line(&mut line).unwrap();
            writeln!(w, "reçu {}", line.trim()).unwrap();
            // Jusqu'à ce qu'Eyrie lâche le tube.
            let mut rest = String::new();
            while r.read_line(&mut rest).is_ok_and(|n| n > 0) {}
        });
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            for l in BufReader::new(reader).lines() {
                tx.send(l.unwrap()).unwrap();
            }
        });
        std::thread::sleep(Duration::from_millis(300));
        writeln!(writer, "consigne").unwrap();
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), "reçu consigne");
        drop(writer);
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)), Err(mpsc::RecvTimeoutError::Disconnected), "lecture finie");
        serving.join().unwrap();
    }

    /// Un tube tenu par un autre exécutable que le compagnon attendu est refusé ; pas de tube : absent.
    #[test]
    fn a_pipe_held_by_another_exe_is_refused_and_a_missing_one_is_absent() {
        let name = format!(r"\\.\pipe\{}", unique("autre"));
        let _server = listen(&name);
        let dir = tempfile::tempdir().unwrap();
        let other = dir.path().join(crate::tailnet::COMPANION);
        std::fs::write(&other, b"").unwrap();
        match win::open_pipe(&name, &other) {
            Err(Attach::Refused(e)) => assert!(e.contains("pas par"), "{e}"),
            r => panic!("attendu un refus : {:?}", r.err()),
        }
        let missing = format!(r"\\.\pipe\{}", unique("absent"));
        assert_eq!(win::open_pipe(&missing, &other).err(), Some(Attach::Absent));
    }

    /// Eyrie ouvre le tube que le veilleur a nommé dans le dossier du nœud ; sans nom, ou avec le nom d'un autre tube
    /// (même servi), rien n'est ouvert : absent.
    #[test]
    fn the_pipe_named_in_the_node_folder_is_the_one_opened() {
        let node = tempfile::tempdir().unwrap();
        let me = std::env::current_exe().unwrap();
        assert_eq!(System.attach(node.path(), &me).err(), Some(Attach::Absent));
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let name = format!(r"\\.\pipe\eyrie-tailnet-{:032x}", nanos ^ (u128::from(std::process::id()) << 96));
        let _server = listen(&name);
        std::fs::write(node.path().join(PIPE_FILE), format!("{name}\n")).unwrap();
        assert!(System.attach(node.path(), &me).is_ok(), "tube nommé par le veilleur");
        let other = format!(r"\\.\pipe\{}", unique("nomme"));
        let _other_server = listen(&other);
        std::fs::write(node.path().join(PIPE_FILE), &other).unwrap();
        assert_eq!(System.attach(node.path(), &me).err(), Some(Attach::Absent));
    }

    fn icacls(path: &Path, args: &[&str]) {
        let out = std::process::Command::new("icacls").arg(path).args(args).output().unwrap();
        assert!(out.status.success(), "icacls {args:?} : {}", String::from_utf8_lossy(&out.stdout));
    }

    /// Eyrie contrôle l'emplacement comme le veilleur avant chaque lancement (`compagnon/exeplace.go`), sans quoi elle
    /// inscrirait un veilleur qui refuserait ensuite de la démarrer : un dossier au-dessus qu'un autre compte peut
    /// renommer (il mettrait le sien à la place) est refusé, même quand l'exécutable et son dossier sont sûrs.
    #[test]
    fn a_folder_above_others_can_rename_is_refused_as_by_the_standby() {
        let tmp = tempfile::tempdir().unwrap();
        let above = tmp.path().join("Outils");
        let exe = above.join("Eyrie").join(crate::tailnet::COMPANION);
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
        std::fs::write(&exe, b"").unwrap();
        assert_eq!(System.place_safe(&exe), Ok(()));
        icacls(&above, &["/grant", "*S-1-5-11:(DE)"]);
        let refused = System.place_safe(&exe);
        icacls(&above, &["/remove:g", "*S-1-5-11"]);
        let err = refused.expect_err("dossier au-dessus renommable");
        assert!(err.contains(&*above.to_string_lossy()) && err.contains("S-1-5-11"), "{err}");
    }

    /// Contrôle à la main de la vraie Eyrie, en lecture seule (droits et propriétaires : rien n'est lancé ni écrit) :
    /// `EYRIE_PLACE_REELLE` nomme les exécutables, séparés par « ; ». Le même côté veilleur :
    /// `TestTheRealEyrieIsInASafePlace` (`compagnon/exeplace_test.go`).
    ///
    /// `$env:EYRIE_PLACE_REELLE = "C:\…\eyrie.exe;C:\…\eyrie-tailnet.exe"`, puis
    /// `cargo test -p eyrie real_place -- --ignored --nocapture`.
    #[test]
    #[ignore = "à la main : EYRIE_PLACE_REELLE nomme les exécutables à contrôler"]
    fn real_place_of_eyrie_is_safe() {
        let Some(list) = std::env::var_os("EYRIE_PLACE_REELLE") else {
            eprintln!("EYRIE_PLACE_REELLE absent : rien à contrôler");
            return;
        };
        let me = exeplace::user_sid().unwrap();
        let mut refused = Vec::new();
        for exe in std::env::split_paths(&list) {
            for (path, rights) in exeplace::levels(&exe) {
                let read = exeplace::read_rights(&path);
                let who = read.as_ref().map(|r| exeplace::other_writer(r, &me, rights));
                eprintln!("{}\n\t{read:?}\n\tdroits {rights:#x} : autre compte {who:?}", path.display());
            }
            let checked = System.place_safe(&exe);
            eprintln!("{} : {checked:?}", exe.display());
            refused.extend(checked.err());
        }
        assert!(refused.is_empty(), "{refused:#?}");
    }

    #[test]
    fn the_node_lock_is_seen_while_held() {
        let name = format!("Local\\{}", unique("verrou"));
        assert!(!System.node_held(&name));
        let held = suite_sdk::instance::NamedMutex::try_acquire(&name).unwrap();
        assert!(System.node_held(&name));
        drop(held);
        assert!(!System.node_held(&name));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Mêmes noms que `compagnon/verrou_test.go` : Eyrie et le veilleur se trouvent.
    #[test]
    fn the_lock_names_match_the_companion() {
        assert_eq!(lock_name(Path::new(r"C:\X\Tailnet")), r"Local\EyrieTailnet:c:/x/tailnet");
        assert_eq!(instance_lock(Path::new(r"C:\Essai\")), r"Local\Eyrie-instance:c:/essai");
    }

    /// Le nom du tube du veilleur se lit dans le dossier du nœud, tel qu'il l'écrit (`compagnon/tube_test.go`) ; rien
    /// d'autre qu'un nom de tube de veilleur de ce poste n'est rendu.
    #[test]
    fn the_standby_pipe_is_read_from_its_file() {
        let node = tempfile::tempdir().unwrap();
        assert_eq!(standby_pipe(node.path()), None);
        let suffix = "0123456789abcdef".repeat(2);
        let name = format!(r"\\.\pipe\eyrie-tailnet-{suffix}");
        std::fs::write(node.path().join(PIPE_FILE), format!("{name}\n")).unwrap();
        assert_eq!(standby_pipe(node.path()), Some(name));
        for bad in [
            format!(r"\\serveur\pipe\eyrie-tailnet-{suffix}"),
            format!(r"\\.\pipe\autre-{suffix}"),
            format!(r"\\.\pipe\eyrie-tailnet-{}", suffix.to_uppercase()),
            format!(r"\\.\pipe\eyrie-tailnet-{}", &suffix[..16]),
            format!(r"\\.\pipe\eyrie-tailnet-{suffix}\..\autre"),
            String::new(),
        ] {
            std::fs::write(node.path().join(PIPE_FILE), &bad).unwrap();
            assert_eq!(standby_pipe(node.path()), None, "{bad}");
        }
    }

    #[test]
    fn the_run_line_is_the_contract_one_quoted_like_windows() {
        let line = run_line(
            Path::new(r"C:\Users\Aubin Chedotal\AppData\Local\Programs\Eyrie\eyrie-tailnet.exe"),
            Path::new(r"C:\Users\Aubin Chedotal\AppData\Local\EyrieSuite\Eyrie\tailnet"),
            Path::new(r"C:\Users\Aubin Chedotal\AppData\Local\EyrieSuite\Eyrie\mobile"),
            "suite-poste",
        );
        assert_eq!(
            line,
            r#""C:\Users\Aubin Chedotal\AppData\Local\Programs\Eyrie\eyrie-tailnet.exe" --veille --dir "C:\Users\Aubin Chedotal\AppData\Local\EyrieSuite\Eyrie\tailnet" --mobile "C:\Users\Aubin Chedotal\AppData\Local\EyrieSuite\Eyrie\mobile" --hostname suite-poste --port 4747"#
        );
        // Une barre finale ne mange pas le guillemet qui la suit.
        assert_eq!(quote(r"D:\", true), r#""D:\\""#);
        assert_eq!(quote(r#"a\"b"#, true), r#""a\\\"b""#);
        assert_eq!(quote("nom avec espace", false), r#""nom avec espace""#);
        assert_eq!(quote("", false), r#""""#);
        // L'exécutable se relit dans la commande inscrite.
        assert_eq!(
            run_exe(&line).as_deref(),
            Some(Path::new(r"C:\Users\Aubin Chedotal\AppData\Local\Programs\Eyrie\eyrie-tailnet.exe"))
        );
        assert_eq!(run_exe(r"C:\x\eyrie-tailnet.exe --veille"), None);
        // Les mêmes arguments pour le veilleur lancé par Eyrie.
        let args = standby_args(Path::new(r"C:\n"), Path::new(r"C:\m"), "h");
        assert_eq!(
            args,
            ["--veille", "--dir", r"C:\n", "--mobile", r"C:\m", "--hostname", "h", "--port", "4747"]
                .map(OsString::from)
                .to_vec()
        );
    }

    #[test]
    fn the_config_is_the_json_the_standby_reads() {
        let c = Config {
            v: 1,
            actif: true,
            cle: "ab".repeat(32),
            instance: instance_lock(Path::new(r"C:\Essai")),
            reduite: true,
        };
        assert_eq!(
            serde_json::to_string(&c).unwrap(),
            format!(
                r#"{{"v":1,"actif":true,"cle":"{}","instance":"Local\\Eyrie-instance:c:/essai","reduite":true}}"#,
                "ab".repeat(32)
            )
        );
    }

    /// Démarrée réduite par le veilleur, Eyrie relancée depuis le PC (réglages) s'ouvre à l'écran.
    #[test]
    fn a_restart_does_not_open_minimized_again() {
        let args = ["--reduite", "--apres=12", "lancer"].map(String::from);
        assert_eq!(crate::restart_args(args.into_iter(), 34), ["lancer", "--apres=34"]);
    }

    /// Les octets que le Gestionnaire des tâches écrit pour une valeur Run activée ou désactivée.
    #[test]
    fn a_run_value_disabled_in_the_task_manager_is_seen() {
        assert!(startup_disabled(&[3, 0, 0, 0, 0x5c, 0x1a, 0x4e, 0x7d, 0x31, 0x2f, 0xdb, 0x01]));
        assert!(startup_disabled(&[7, 0, 0, 0]));
        assert!(!startup_disabled(&[2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]));
        assert!(!startup_disabled(&[6, 0, 0, 0]));
        assert!(!startup_disabled(&[]));
    }

    #[test]
    fn a_standby_failure_is_read_from_its_file() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(read_failure(dir.path()), None);
        std::fs::write(dir.path().join(FAILURE), "{\"at\":1700000000,\"message\":\"tube de contrôle : refusé\"}\n")
            .unwrap();
        assert_eq!(
            read_failure(dir.path()),
            Some(Failure { at: 1_700_000_000, message: "tube de contrôle : refusé".into() })
        );
    }
}
