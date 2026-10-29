//! Les dossiers du PC que le téléphone n'atteint jamais, sous quelque chemin que ce soit : une seule liste, pour le pont
//! (le sélecteur et le contrôle d'un chemin) et pour les modules qui revérifient ce qu'ils reçoivent (la destination
//! d'un clonage de DragonTree). S'y ajoutent les dossiers de configuration de l'utilisateur, que des outils lisent
//! d'eux-mêmes ([`in_dot_folder`], [`powershell_dirs`]) : un dépôt choisi depuis le téléphone n'y atterrit pas.

use std::path::{Path, PathBuf};

/// Noms de dossiers refusés où qu'ils soient, en minuscules : secrets (clés, jetons, configurations qui lancent des
/// commandes) et dossiers du système.
pub const PROTECTED_NAMES: [&str; 9] = [
    ".ssh",
    ".gnupg",
    ".aws",
    ".azure",
    ".kube",
    ".docker",
    ".password-store",
    "$recycle.bin",
    "system volume information",
];

/// `name`, un nom de dossier, est l'un des [`PROTECTED_NAMES`] (sans casse).
pub fn protected_name(name: &str) -> bool {
    PROTECTED_NAMES.contains(&name.to_lowercase().as_str())
}

/// `p` est un dossier de configuration de l'utilisateur, ou dedans : un dossier de son profil `home` dont le nom
/// commence par un point (`.config`, `.claude`, `.cargo`…). Git, les shells, Claude Code, cargo, les éditeurs les
/// lisent d'eux-mêmes, et certains y trouvent des commandes à lancer. Sans casse, `\` ou `/`.
pub fn in_dot_folder(p: &Path, home: &Path) -> bool {
    let key = |p: &Path| p.to_string_lossy().replace('\\', "/").trim_end_matches('/').to_lowercase();
    let (p, home) = (key(p), key(home));
    let rest = p.strip_prefix(&home).and_then(|r| r.strip_prefix('/'));
    !home.is_empty() && rest.is_some_and(|r| r.starts_with('.'))
}

/// Les dossiers des profils de PowerShell dans les Documents `documents` (ceux du profil, ou ceux que Windows connaît,
/// dans OneDrive) : le `profile.ps1` qu'ils contiennent est lancé à chaque démarrage de PowerShell.
pub fn powershell_dirs(documents: &Path) -> [PathBuf; 2] {
    [documents.join("PowerShell"), documents.join("WindowsPowerShell")]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_protected_name_is_recognized_whatever_its_case() {
        for name in PROTECTED_NAMES.into_iter().chain([".AWS", "System Volume Information", "$Recycle.Bin"]) {
            assert!(protected_name(name), "{name}");
        }
        for name in ["aws", ".aws2", "kube", "depot", ""] {
            assert!(!protected_name(name), "{name}");
        }
    }

    /// Un dossier à point du profil, et tout ce qu'il contient, est une configuration ; le profil lui-même, ses autres
    /// dossiers, un dossier à point ailleurs, un autre profil au nom proche, non.
    #[test]
    fn a_dot_folder_of_the_profile_is_a_configuration() {
        let home = Path::new(r"C:\Users\ada");
        let inside = [r"C:\Users\ada\.config", "c:/users/ADA/.config/git", r"C:\Users\ada\.claude\commands\"];
        for p in inside.into_iter().chain([r"C:\Users\ada\.x"]) {
            assert!(in_dot_folder(Path::new(p), home), "{p}");
        }
        let outside = [r"C:\Users\ada", r"C:\Users\ada\DEV\.outils", r"C:\Users\adam\.config", r"C:\DEV\.config"];
        for p in outside.into_iter().chain([r"C:\Users"]) {
            assert!(!in_dot_folder(Path::new(p), home), "{p}");
        }
        assert!(!in_dot_folder(Path::new(r"C:\Users\ada\.config"), Path::new("")), "sans profil connu");
        let docs = Path::new(r"C:\Users\ada\OneDrive\Documents");
        assert_eq!(powershell_dirs(docs), [docs.join("PowerShell"), docs.join("WindowsPowerShell")]);
    }
}
