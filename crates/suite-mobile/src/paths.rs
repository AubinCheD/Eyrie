//! Les dossiers du PC que le téléphone n'atteint jamais, sous quelque chemin que ce soit : une seule liste, pour le pont
//! (le sélecteur et le contrôle d'un chemin) et pour les modules qui revérifient ce qu'ils reçoivent (la destination
//! d'un clonage de DragonTree).

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
}
