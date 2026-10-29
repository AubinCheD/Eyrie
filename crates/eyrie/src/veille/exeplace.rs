//! L'emplacement du veilleur et de l'`eyrie.exe` qu'il lance, contrôlé avec les règles du veilleur
//! (`compagnon/exeplace.go`, refait avant chaque lancement) : Eyrie n'inscrit pas un veilleur qui refuserait ensuite
//! de la démarrer, et ne garde pas une commande inscrite moins sûre. Un autre compte du poste qui pourrait remplacer
//! l'exécutable, déposer une DLL dans son dossier ou renommer un dossier au-dessus (pour mettre le sien à la place)
//! ferait lancer son code sous le compte de l'utilisateur à un appui sur le téléphone. Le propriétaire de chacun compte
//! aussi : il peut toujours en changer les droits. Le chemin écrit et le vrai sont contrôlés : une jonction mène
//! ailleurs. `fawkes::exeplace::check` applique les mêmes règles à ses démarrages : les changer ensemble.
#![cfg_attr(not(windows), allow(dead_code))]

use std::path::{Path, PathBuf};

/// Écrire, ajouter un fichier ou un sous-dossier, supprimer, changer les droits ou le propriétaire.
const WRITE_RIGHTS: u32 = 0x2 | 0x4 | 0x40 | 0x1_0000 | 0x4_0000 | 0x8_0000 | 0x1000_0000 | 0x4000_0000;
/// Sur un dossier au-dessus : le renommer (supprimer), renommer ce qu'il contient (supprimer un enfant), changer les
/// droits ou le propriétaire. Y créer un dossier ne suffit pas (c'est le droit de tous à la racine de C:).
const ABOVE_RIGHTS: u32 = 0x40 | 0x1_0000 | 0x4_0000 | 0x8_0000 | 0x1000_0000;
/// Supprimer : la racine d'un disque ne se renomme pas.
const DELETE_RIGHT: u32 = 0x1_0000;

/// ACCESS_ALLOWED_ACE_TYPE.
const ALLOWED: u8 = 0x0;
/// ACCESS_ALLOWED_CALLBACK_ACE_TYPE : un droit accordé sous condition, compté comme accordé.
const ALLOWED_CALLBACK: u8 = 0x9;
/// INHERIT_ONLY_ACE : l'ACE ne vaut que pour ce qui sera créé dedans.
const INHERIT_ONLY: u8 = 0x8;

/// Les comptes du système, sûrs comme l'utilisateur, propriétaires compris.
const SYSTEM_SIDS: [&str; 3] = [
    "S-1-5-18",                                                       // SYSTEM
    "S-1-5-32-544",                                                   // Administrateurs
    "S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464", // TrustedInstaller
];

/// Dans une ACE, aussi le propriétaire : sûr, ou refusé avant ([`untrusted_owner`]).
const OWNER_SIDS: [&str; 2] = [
    "S-1-3-0", // CREATOR OWNER (le créateur d'un fichier, donc l'utilisateur ici)
    "S-1-3-4", // OWNER RIGHTS
];

/// Une entrée de la liste de contrôle d'un fichier ou d'un dossier.
#[derive(Clone, Debug, PartialEq)]
pub struct Ace {
    pub kind: u8,
    pub flags: u8,
    pub mask: u32,
    /// Le compte, en texte (`S-1-5-11`).
    pub sid: String,
}

/// Les droits d'un fichier ou d'un dossier.
#[derive(Clone, Debug, PartialEq)]
pub struct Rights {
    /// `None` : pas de propriétaire lisible.
    pub owner: Option<String>,
    /// `None` : pas de liste de contrôle, tout le monde a tous les droits.
    pub dacl: Option<Vec<Ace>>,
}

/// Rend une erreur, en français, si l'exécutable `exe` ou son dossier peut être modifié par un autre compte que
/// l'utilisateur, SYSTEM, les administrateurs ou TrustedInstaller, ou un dossier au-dessus renommé par lui, par le
/// chemin écrit comme par le vrai ([`checked_levels`]), si `exe` n'existe pas ou si leurs droits sont illisibles
/// (`exePlaceCheck`).
#[cfg(windows)]
pub fn check(exe: &Path) -> Result<(), String> {
    if let Err(e) = std::fs::metadata(exe)
        && e.kind() == std::io::ErrorKind::NotFound
    {
        return Err(format!("{} introuvable", exe.display()));
    }
    let me = user_sid()?;
    for (path, wanted) in checked_levels(exe)? {
        if let Some(who) = other_writer(&read_rights(&path)?, &me, wanted) {
            return Err(format!(
                "{} peut être modifié par un autre compte du poste ({who}) : le veilleur lancerait ce qu'il y dépose. \
                 Installez Eyrie dans %LOCALAPPDATA%\\Programs ou dans Program Files.",
                path.display()
            ));
        }
    }
    Ok(())
}

#[cfg(not(windows))]
pub fn check(_exe: &Path) -> Result<(), String> {
    Ok(())
}

/// Les chemins contrôlés et les droits qu'aucun autre compte ne doit y avoir : l'exécutable `exe` et son dossier
/// ([`WRITE_RIGHTS`]), puis chaque dossier au-dessus jusqu'à la racine du disque ([`ABOVE_RIGHTS`], sans le droit de
/// supprimer à la racine) (`placeLevels`).
pub fn levels(exe: &Path) -> Vec<(PathBuf, u32)> {
    let mut levels = vec![(exe.to_path_buf(), WRITE_RIGHTS)];
    let Some(dir) = exe.parent() else { return levels };
    levels.push((dir.to_path_buf(), WRITE_RIGHTS));
    let mut above = dir.parent();
    while let Some(path) = above {
        above = path.parent();
        let rights = if above.is_none() { ABOVE_RIGHTS & !DELETE_RIGHT } else { ABOVE_RIGHTS };
        levels.push((path.to_path_buf(), rights));
    }
    levels
}

/// Les niveaux de `exe` tel qu'écrit, puis ceux de son vrai chemin ([`real_path`]) qui n'y sont pas déjà
/// (`checkedLevels`). Windows lit les droits d'une jonction, pas ceux du dossier où elle mène : sans le vrai chemin, ce
/// dossier et ceux au-dessus de lui échapperaient ; sans le chemin écrit, la jonction elle-même, qu'un autre compte
/// pourrait remplacer.
pub fn checked_levels(exe: &Path) -> Result<Vec<(PathBuf, u32)>, String> {
    let mut checked = levels(exe);
    for level in levels(&real_path(exe)?) {
        if !checked.contains(&level) {
            checked.push(level);
        }
    }
    Ok(checked)
}

/// Le vrai chemin de `path`, jonctions et liens résolus : `canonicalize`, soit GetFinalPathNameByHandleW sur un
/// handle ouvert avec FILE_FLAG_BACKUP_SEMANTICS. Sans le préfixe `\\?\` quand c'est possible ([`win32_path`])
/// (`realPath`).
fn real_path(path: &Path) -> Result<PathBuf, String> {
    let real = std::fs::canonicalize(path).map_err(|e| format!("chemin réel de {} illisible : {e}", path.display()))?;
    Ok(win32_path(&real).unwrap_or(real))
}

/// `\\?\C:\…` en `C:\…`, `\\?\UNC\serveur\…` en `\\serveur\…`, si Windows lit chaque niveau pareil sans le préfixe
/// (GetFullPathNameW, par `std::path::absolute`) ; sinon `None`. Sans lui, Windows retire un point ou une espace en fin
/// de nom (`Outils.` devient `Outils`) et prend `NUL` pour un périphérique : un autre chemin serait contrôlé
/// (`winPath`).
fn win32_path(real: &Path) -> Option<PathBuf> {
    let real = real.to_str()?;
    let short = match real.strip_prefix(r"\\?\UNC\") {
        Some(rest) => PathBuf::from(format!(r"\\{rest}")),
        None => PathBuf::from(real.strip_prefix(r"\\?\")?),
    };
    short
        .ancestors()
        .all(|path| std::path::absolute(path).is_ok_and(|full| full.as_os_str() == path.as_os_str()))
        .then_some(short)
}

/// Le propriétaire de `rights`, s'il n'est ni l'utilisateur `me` ni le système (« propriétaire <SID> »), sinon le
/// premier compte autre que l'utilisateur et le système qui a l'un des droits `wanted`, ou `None` (`writableByOthers`).
pub fn other_writer(rights: &Rights, me: &str, wanted: u32) -> Option<String> {
    if let Some(who) = untrusted_owner(rights.owner.as_deref(), me) {
        return Some(who);
    }
    let Some(dacl) = &rights.dacl else {
        return Some("tout le monde".into());
    };
    dacl.iter()
        .find(|ace| {
            // Une ACE conditionnelle a la même tête, le même masque et le même SID, suivis de sa condition.
            matches!(ace.kind, ALLOWED | ALLOWED_CALLBACK)
                && ace.flags & INHERIT_ONLY == 0
                && ace.mask & wanted != 0
                && ace.sid != me
                && !SYSTEM_SIDS.contains(&ace.sid.as_str())
                && !OWNER_SIDS.contains(&ace.sid.as_str())
        })
        .map(|ace| ace.sid.clone())
}

/// « propriétaire <SID> » si `owner` n'est ni l'utilisateur `me` ni un compte du système, sinon `None`. Le
/// propriétaire d'un fichier ou d'un dossier peut toujours en changer les droits : un autre compte qui l'aurait créé
/// (un dossier préparé pour l'utilisateur) s'y donnerait plus tard le droit d'écrire.
pub fn untrusted_owner(owner: Option<&str>, me: &str) -> Option<String> {
    match owner {
        None => Some("propriétaire inconnu".into()),
        Some(o) if o == me || SYSTEM_SIDS.contains(&o) => None,
        Some(o) => Some(format!("propriétaire {o}")),
    }
}

/// La forme texte d'un SID en mémoire (`S-1-5-18`), comme `ConvertSidToStringSidW` : révision, nombre de
/// sous-autorités, autorité (6 octets, gros-boutiste), sous-autorités (4 octets chacune, petit-boutiste).
pub fn sid_text(sid: &[u8]) -> Option<String> {
    let (head, subs) = sid.split_at_checked(8)?;
    let (subs, _) = subs.as_chunks::<4>();
    let count = usize::from(head[1]);
    if head[0] != 1 || subs.len() < count {
        return None;
    }
    let authority = head[2..].iter().fold(0u64, |a, &b| (a << 8) | u64::from(b));
    let mut text = if authority >> 32 == 0 { format!("S-1-{authority}") } else { format!("S-1-{authority:#014x}") };
    for sub in &subs[..count] {
        text += &format!("-{}", u32::from_le_bytes(*sub));
    }
    Some(text)
}

/// Une ACE en mémoire, sur ses `AceSize` octets : type, drapeaux, taille, droits, puis le SID (pour celles qui
/// accordent ou refusent, sous condition ou non ; il est illisible pour les autres, qui n'accordent rien ici). Un SID
/// illisible reste un compte inconnu, donc un autre.
pub fn parse_ace(ace: &[u8]) -> Option<Ace> {
    let (head, sid) = ace.split_at_checked(8)?;
    Some(Ace {
        kind: head[0],
        flags: head[1],
        mask: u32::from_le_bytes([head[4], head[5], head[6], head[7]]),
        sid: sid_text(sid).unwrap_or_else(|| "SID illisible".into()),
    })
}

/// Le compte d'Eyrie, en texte.
#[cfg(windows)]
pub fn user_sid() -> Result<String, String> {
    use windows_sys::Win32::Security::{GetLengthSid, IsValidSid, TOKEN_USER};
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    let unreadable = || "compte de l'utilisateur illisible".to_string();
    // SAFETY : pseudo-handle de notre processus.
    let buf = super::win::token_user(unsafe { GetCurrentProcess() }).map_err(|e| format!("{} : {e}", unreadable()))?;
    // SAFETY : `buf` contient un TOKEN_USER, dont le SID (valide, vérifié) pointe dans `buf`.
    let sid = unsafe {
        let sid = (*(buf.as_ptr() as *const TOKEN_USER)).User.Sid;
        if IsValidSid(sid) == 0 {
            return Err(unreadable());
        }
        std::slice::from_raw_parts(sid as *const u8, GetLengthSid(sid) as usize)
    };
    sid_text(sid).ok_or_else(unreadable)
}

/// Le propriétaire et la liste de contrôle de `path`.
#[cfg(windows)]
pub fn read_rights(path: &Path) -> Result<Rights, String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Security::{
        ACE_HEADER, ACL, DACL_SECURITY_INFORMATION, GetAce, GetFileSecurityW, GetLengthSid, GetSecurityDescriptorDacl,
        GetSecurityDescriptorOwner, IsValidSid, OWNER_SECURITY_INFORMATION, PSID,
    };
    let unreadable = |e: std::io::Error| format!("droits de {} illisibles : {e}", path.display());
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
    let what = OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION;
    let mut len = 0u32;
    // SAFETY : chemin terminé par un zéro ; sans tampon, seule la taille est rendue.
    unsafe { GetFileSecurityW(wide.as_ptr(), what, std::ptr::null_mut(), 0, &mut len) };
    if len == 0 {
        return Err(unreadable(std::io::Error::last_os_error()));
    }
    // Aligné : la description rendue est d'un seul bloc (auto-relative), propriétaire et liste dedans.
    let mut buf = vec![0u64; (len as usize).div_ceil(8)];
    let size = buf.len() * 8;
    let start = buf.as_ptr() as usize;
    // SAFETY : tampon de `size` octets ; le propriétaire, la liste et chaque ACE rendus pointent dedans, ce qui est
    // vérifié avant de les lire.
    unsafe {
        let sd = buf.as_mut_ptr().cast();
        if GetFileSecurityW(wide.as_ptr(), what, sd, size as u32, &mut len) == 0 {
            return Err(unreadable(std::io::Error::last_os_error()));
        }
        let inside = |p: usize, n: usize| p >= start && p.checked_add(n).is_some_and(|end| end <= start + size);
        let mut defaulted = 0;
        let mut owner: PSID = std::ptr::null_mut();
        if GetSecurityDescriptorOwner(sd, &mut owner, &mut defaulted) == 0 {
            return Err(unreadable(std::io::Error::last_os_error()));
        }
        let owner = (!owner.is_null() && IsValidSid(owner) != 0)
            .then(|| (owner as usize, GetLengthSid(owner) as usize))
            .filter(|&(p, n)| inside(p, n))
            .and_then(|(p, n)| sid_text(std::slice::from_raw_parts(p as *const u8, n)));
        let (mut present, mut dacl) = (0, std::ptr::null_mut::<ACL>());
        if GetSecurityDescriptorDacl(sd, &mut present, &mut dacl, &mut defaulted) == 0 {
            return Err(unreadable(std::io::Error::last_os_error()));
        }
        if present == 0 || dacl.is_null() {
            return Ok(Rights { owner, dacl: None });
        }
        let mut aces = Vec::new();
        for i in 0..u32::from((*dacl).AceCount) {
            let mut ace = std::ptr::null_mut();
            if GetAce(dacl, i, &mut ace) == 0 {
                return Err(unreadable(std::io::Error::last_os_error()));
            }
            let n = inside(ace as usize, size_of::<ACE_HEADER>())
                .then(|| usize::from((*(ace as *const ACE_HEADER)).AceSize))
                .filter(|&n| inside(ace as usize, n));
            let parsed = n.and_then(|n| parse_ace(std::slice::from_raw_parts(ace as *const u8, n)));
            aces.push(parsed.ok_or_else(|| format!("droits de {} illisibles : ACE {i} illisible", path.display()))?);
        }
        Ok(Rights { owner, dacl: Some(aces) })
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    const ME: &str = "S-1-12-1-1968130991-1128242788-4605104-3573629080";
    const AU: &str = "S-1-5-11";

    fn ace(kind: u8, flags: u8, mask: u32, sid: &str) -> Ace {
        Ace { kind, flags, mask, sid: sid.into() }
    }

    fn mine(dacl: Vec<Ace>) -> Rights {
        Rights { owner: Some(ME.into()), dacl: Some(dacl) }
    }

    fn icacls(path: &Path, args: &[&str]) {
        let out = std::process::Command::new("icacls").arg(path).args(args).output().unwrap();
        assert!(out.status.success(), "icacls {args:?} : {}", String::from_utf8_lossy(&out.stdout));
    }

    /// Du fichier à la racine du disque, comme le veilleur : la racine ne se renomme pas.
    #[test]
    fn the_levels_go_from_the_exe_to_the_root_of_the_disk() {
        let exe = Path::new(r"C:\Outils\Eyrie\eyrie.exe");
        assert_eq!(
            levels(exe),
            [
                (PathBuf::from(r"C:\Outils\Eyrie\eyrie.exe"), WRITE_RIGHTS),
                (PathBuf::from(r"C:\Outils\Eyrie"), WRITE_RIGHTS),
                (PathBuf::from(r"C:\Outils"), ABOVE_RIGHTS),
                (PathBuf::from(r"C:\"), ABOVE_RIGHTS & !DELETE_RIGHT),
            ]
        );
        assert_eq!(levels(Path::new(r"\\?\D:\eyrie.exe")).last(), Some(&(PathBuf::from(r"\\?\D:\"), WRITE_RIGHTS)));
    }

    /// Le propriétaire peut toujours se donner tous les droits : seuls l'utilisateur (ici un compte Entra ID,
    /// `S-1-12-1-…`) et le système sont des propriétaires sûrs.
    #[test]
    fn only_the_user_or_the_system_owns_a_safe_place() {
        for owner in [ME, "S-1-5-18", "S-1-5-32-544", "S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464"]
        {
            assert_eq!(untrusted_owner(Some(owner), ME), None, "{owner}");
        }
        let other = "S-1-5-21-1004336348-1177238915-682003330-1001";
        assert_eq!(untrusted_owner(Some(other), ME), Some(format!("propriétaire {other}")));
        // OWNER RIGHTS, sûr dans une ACE, ne l'est pas comme propriétaire ; sans propriétaire, rien n'est sûr.
        for owner in [Some("S-1-3-4"), Some(AU), None] {
            assert!(untrusted_owner(owner, ME).is_some(), "{owner:?}");
        }
        let theirs = Rights { owner: Some(other.into()), dacl: Some(vec![]) };
        assert_eq!(other_writer(&theirs, ME, WRITE_RIGHTS), Some(format!("propriétaire {other}")));
    }

    /// Un autre compte qui a l'un des droits est trouvé ; pas une ACE qui ne vaut que pour ce qui sera créé dedans, ni
    /// un refus, ni l'utilisateur ou le système. Un droit accordé sous condition compte comme accordé ; pas de liste
    /// de contrôle, tout le monde a tous les droits.
    #[test]
    fn another_account_with_one_of_the_rights_is_found() {
        let modify = 0x1301bf;
        assert_eq!(other_writer(&mine(vec![ace(ALLOWED, 0x3, modify, AU)]), ME, WRITE_RIGHTS), Some(AU.into()));
        assert_eq!(other_writer(&mine(vec![ace(ALLOWED_CALLBACK, 0, modify, AU)]), ME, WRITE_RIGHTS), Some(AU.into()));
        assert_eq!(other_writer(&mine(vec![ace(ALLOWED, INHERIT_ONLY | 0x3, modify, AU)]), ME, WRITE_RIGHTS), None);
        assert_eq!(other_writer(&mine(vec![ace(0x1, 0, modify, AU)]), ME, WRITE_RIGHTS), None);
        assert_eq!(other_writer(&mine(vec![ace(ALLOWED, 0, 0x1200a9, AU)]), ME, WRITE_RIGHTS), None);
        let trusted = [ME, "S-1-5-18", "S-1-5-32-544", "S-1-3-0", "S-1-3-4"].map(|s| ace(ALLOWED, 0x13, 0x1f01ff, s));
        assert_eq!(other_writer(&mine(trusted.to_vec()), ME, WRITE_RIGHTS), None);
        let open = Rights { owner: Some(ME.into()), dacl: None };
        assert_eq!(other_writer(&open, ME, WRITE_RIGHTS).as_deref(), Some("tout le monde"));
        // Le premier dans l'ordre de la liste.
        let two = mine(vec![ace(ALLOWED, 0, 0x2, "S-1-1-0"), ace(ALLOWED, 0, 0x2, AU)]);
        assert_eq!(other_writer(&two, ME, WRITE_RIGHTS).as_deref(), Some("S-1-1-0"));
    }

    /// Les droits relevés sur le poste de l'utilisateur le 29/10/2026 (`target\release` dans son profil, compte
    /// Entra ID) : son profil (SYSTEM, accès à une capacité d'application), `C:\Users` (lecture pour tous) et `C:\`
    /// (TrustedInstaller ; créer un dossier pour les utilisateurs authentifiés, et pour ce qu'ils y créent le droit de
    /// le supprimer, qui ne vaut que dedans) n'ont rien qu'un autre compte puisse renommer. Créer un dossier dans celui
    /// d'Eyrie, en revanche, y déposerait une DLL.
    #[test]
    fn the_rights_seen_on_the_users_pc_are_safe() {
        const TI: &str = "S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464";
        const FA: u32 = 0x1f01ff;
        let system = |owner: &str, extra: Vec<Ace>| Rights {
            owner: Some(owner.into()),
            dacl: Some(
                [vec![ace(ALLOWED, 0x3, FA, "S-1-5-18"), ace(ALLOWED, 0x3, FA, "S-1-5-32-544")], extra].concat(),
            ),
        };
        let profile = system(
            "S-1-5-18",
            vec![
                ace(ALLOWED, 0x3, FA, ME),
                ace(ALLOWED, 0, 0x100020, "S-1-15-3-65536-599108337-2355189375-1353122160-3480128286-3345335107"),
            ],
        );
        let users = system(
            "S-1-5-18",
            vec![
                ace(ALLOWED, 0, 0x1200a9, "S-1-5-32-545"),
                ace(ALLOWED, 0xb, 0xa0000000, "S-1-5-32-545"),
                ace(ALLOWED, 0, 0x1200a9, "S-1-1-0"),
                ace(ALLOWED, 0xb, 0xa0000000, "S-1-1-0"),
            ],
        );
        let root = system(
            TI,
            vec![
                ace(ALLOWED, 0x3, 0x1200a9, "S-1-5-32-545"),
                ace(ALLOWED, 0xb, 0xe0010000, AU),
                ace(ALLOWED, 0, 0x4, AU),
            ],
        );
        let walk = levels(Path::new(r"C:\Users\x\DEV\Eyrie\target\release\eyrie.exe"));
        let rights_of = |p: &str| walk.iter().find(|(path, _)| path == Path::new(p)).unwrap().1;
        assert_eq!(other_writer(&profile, ME, rights_of(r"C:\Users\x")), None);
        assert_eq!(other_writer(&users, ME, rights_of(r"C:\Users")), None);
        assert_eq!(other_writer(&root, ME, rights_of(r"C:\")), None);
        let release = mine(vec![ace(ALLOWED, 0x13, FA, "S-1-5-18"), ace(ALLOWED, 0x13, FA, ME)]);
        assert_eq!(other_writer(&release, ME, WRITE_RIGHTS), None);
        assert_eq!(other_writer(&mine(vec![ace(ALLOWED, 0, 0x4, AU)]), ME, WRITE_RIGHTS), Some(AU.into()));
        // Le même droit de supprimer, accordé au dossier lui-même et non à ce qui sera créé dedans : renommable, sauf à
        // la racine.
        let renamable = system(TI, vec![ace(ALLOWED, 0x3, 0xe0010000, AU)]);
        assert_eq!(other_writer(&renamable, ME, ABOVE_RIGHTS), Some(AU.into()));
        assert_eq!(other_writer(&renamable, ME, rights_of(r"C:\")), None);
    }

    /// Les SID en mémoire, tels que Windows les écrit en texte ; un SID tronqué ou d'une autre révision est illisible.
    #[test]
    fn sids_read_like_windows_writes_them() {
        assert_eq!(sid_text(&[1, 1, 0, 0, 0, 0, 0, 5, 18, 0, 0, 0]).as_deref(), Some("S-1-5-18"));
        let me = [1, 5, 0, 0, 0, 0, 0, 12, 1, 0, 0, 0]
            .into_iter()
            .chain([1968130991u32, 1128242788, 4605104, 3573629080].into_iter().flat_map(u32::to_le_bytes))
            .collect::<Vec<u8>>();
        assert_eq!(sid_text(&me).as_deref(), Some(ME));
        assert_eq!(sid_text(&[1, 0, 0, 0, 0, 0, 0, 0]).as_deref(), Some("S-1-0"));
        assert_eq!(sid_text(&[1, 0, 0, 1, 0, 0, 0, 0]).as_deref(), Some("S-1-0x000100000000"));
        for bad in [&[1, 2, 0, 0, 0, 0, 0, 5, 32, 0, 0, 0][..], &[2, 0, 0, 0, 0, 0, 0, 5], &[1, 1, 0]] {
            assert_eq!(sid_text(bad), None, "{bad:?}");
        }
    }

    /// Une ACE en mémoire : tête, droits, SID ; celle d'un droit sous condition a sa condition après le SID.
    #[test]
    fn aces_are_read_from_their_bytes() {
        let au = [1, 1, 0, 0, 0, 0, 0, 5, 11, 0, 0, 0];
        let allowed = [&[ALLOWED, 0x3, 20, 0][..], &0x1301bfu32.to_le_bytes(), &au].concat();
        assert_eq!(parse_ace(&allowed), Some(ace(ALLOWED, 0x3, 0x1301bf, AU)));
        let callback = [&[ALLOWED_CALLBACK, 0, 28, 0][..], &0x1f01ffu32.to_le_bytes(), &au, b"artx"].concat();
        assert_eq!(parse_ace(&callback), Some(ace(ALLOWED_CALLBACK, 0, 0x1f01ff, AU)));
        // Un SID illisible est un compte inconnu, donc un autre ; trop court pour une tête, l'ACE est illisible.
        let unreadable = mine(vec![parse_ace(&allowed[..14]).unwrap()]);
        assert_eq!(other_writer(&unreadable, ME, WRITE_RIGHTS).as_deref(), Some("SID illisible"));
        assert_eq!(parse_ace(&allowed[..6]), None);
    }

    /// Les droits d'un vrai dossier : celui de l'essai, créé par l'utilisateur, est à lui ; ouvert en modification
    /// aux utilisateurs authentifiés, il ne l'est plus pour eux, ni l'exécutable qu'il contient.
    #[test]
    fn a_folder_others_can_write_to_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("eyrie.exe");
        std::fs::write(&exe, b"").unwrap();
        let me = user_sid().unwrap();
        assert_eq!(read_rights(dir.path()).unwrap().owner.as_deref(), Some(me.as_str()));
        assert_eq!(check(&exe), Ok(()));
        icacls(dir.path(), &["/grant", "*S-1-5-11:(OI)(CI)M"]);
        let refused = check(&exe);
        icacls(dir.path(), &["/remove:g", "*S-1-5-11"]);
        let err = refused.unwrap_err();
        assert!(err.contains("S-1-5-11") && err.contains("Program Files"), "{err}");
    }

    /// Un dossier au-dessus qu'un autre compte peut renommer, ou dont il peut renommer le contenu, ne protège rien :
    /// il mettrait son propre dossier à la place. Y créer un dossier ne suffit pas pour cela.
    #[test]
    fn a_folder_above_others_can_rename_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let above = tmp.path().join("Outils");
        let exe = above.join("Eyrie").join("eyrie.exe");
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
        std::fs::write(&exe, b"").unwrap();
        // Des droits sur ce seul dossier : ni Eyrie ni eyrie.exe n'en héritent.
        for right in ["DE", "DC", "WDAC", "WO"] {
            icacls(&above, &["/grant", &format!("*S-1-5-11:({right})")]);
            let refused = check(&exe);
            icacls(&above, &["/remove:g", "*S-1-5-11"]);
            let err = refused.expect_err(right);
            assert!(
                err.starts_with(&format!("{} peut", above.display())) && err.contains("S-1-5-11"),
                "{right} : {err}"
            );
        }
        icacls(&above, &["/grant", "*S-1-5-11:(AD)"]);
        let created = check(&exe);
        icacls(&above, &["/remove:g", "*S-1-5-11"]);
        assert_eq!(created, Ok(()), "créer un dossier au-dessus");
    }

    /// Une jonction dans le chemin (`Outils` qui mène à `disque\Outils`) : Windows en lit les droits à elle, pas ceux
    /// du dossier où elle mène. Ce vrai dossier, qu'un autre compte peut renommer, est refusé aussi à travers elle, et
    /// nommé sans `\\?\`.
    #[test]
    fn a_junction_does_not_hide_the_real_folder() {
        let tmp = tempfile::tempdir().unwrap();
        let real = tmp.path().join("disque").join("Outils");
        std::fs::create_dir_all(real.join("Eyrie")).unwrap();
        std::fs::write(real.join("Eyrie").join("eyrie.exe"), b"").unwrap();
        let junction = tmp.path().join("Outils");
        let made =
            std::process::Command::new("cmd").args(["/c", "mklink", "/J"]).arg(&junction).arg(&real).output().unwrap();
        assert!(made.status.success(), "mklink /J : {made:?}");
        let exe = junction.join("Eyrie").join("eyrie.exe");
        assert_eq!(check(&exe), Ok(()));
        icacls(&real, &["/grant", "*S-1-5-11:(DE)"]);
        let refused = check(&exe);
        icacls(&real, &["/remove:g", "*S-1-5-11"]);
        let err = refused.unwrap_err();
        assert!(err.contains(r"\disque\Outils peut") && !err.starts_with(r"\\?\") && err.contains("S-1-5-11"), "{err}");
    }

    /// Le vrai chemin perd son `\\?\` quand Windows lit chaque niveau pareil sans ; pas quand il en changerait un
    /// (point ou espace final retiré, `NUL` pris pour un périphérique) : un autre dossier serait contrôlé.
    #[test]
    fn the_real_path_drops_the_prefix_only_when_windows_reads_it_the_same() {
        for (real, short) in [
            (r"\\?\C:\Outils\Eyrie\eyrie.exe", r"C:\Outils\Eyrie\eyrie.exe"),
            (r"\\?\UNC\serveur\partage\eyrie.exe", r"\\serveur\partage\eyrie.exe"),
        ] {
            assert_eq!(win32_path(Path::new(real)).as_deref(), Some(Path::new(short)), "{real}");
        }
        for real in [
            r"C:\Outils\Eyrie\eyrie.exe",
            r"\\?\C:\Outils.\Eyrie\eyrie.exe",
            r"\\?\C:\Outils\Eyrie \eyrie.exe",
            r"\\?\C:\Outils\NUL\eyrie.exe",
            r"\\?\Volume{0a1b2c3d}\Outils\eyrie.exe",
        ] {
            assert_eq!(win32_path(Path::new(real)), None, "{real}");
        }
    }

    #[test]
    fn a_missing_exe_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let err = check(&dir.path().join("eyrie.exe")).unwrap_err();
        assert!(err.contains("introuvable"), "{err}");
    }
}
