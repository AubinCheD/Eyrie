//! Les fichiers qu'un téléphone joint à la saisie d'un terminal ([`ToPc::Upload`](suite_mobile::msg::ToPc::Upload)) :
//! rangés dans un dossier d'Eyrie (`<dossier du pont>\televerses\<date>\`), jamais dans un dossier de projet, sous un nom
//! nettoyé que rien d'autre ne porte (`143205-capture.png`) ; le téléphone reçoit le chemin et l'insère dans la saisie,
//! rien n'est envoyé au terminal sans que l'utilisateur le fasse. Les dossiers de plus de [`KEEP_DAYS`] jours sont
//! effacés au passage, et un jour en reçoit [`DAY_MAX`] octets au plus.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Les dossiers des jours plus anciens sont effacés.
pub const KEEP_DAYS: i64 = 7;
/// Ce qu'un jour reçoit au plus, tous téléphones confondus : un téléphone qui enverrait sans fin ne remplit pas le disque.
pub const DAY_MAX: u64 = 200 * 1024 * 1024;
/// Longueur d'un nom gardé, en caractères.
const NAME_MAX: usize = 80;

/// Un nom de fichier sûr sous Windows, tiré de celui du téléphone : sans dossier, ni caractère réservé (`<>:"/\|?*`, ce
/// qui écarte aussi les flux de données secondaires), ni caractère de contrôle, ni point ou espace en tête ou au bout, ni
/// nom réservé (`CON`, `NUL`, `COM1`…) ; 80 caractères au plus, l'extension gardée ; vide : `fichier`.
pub fn clean_name(raw: &str) -> String {
    let base = raw.rsplit(['/', '\\']).next().unwrap_or_default();
    let kept: String =
        base.chars().map(|c| if c.is_control() || "<>:\"/\\|?*".contains(c) { '_' } else { c }).collect();
    let mut name = kept.trim_matches(|c: char| c == '.' || c.is_whitespace()).to_string();
    if name.chars().count() > NAME_MAX {
        let ext: String = match name.rsplit_once('.') {
            Some((_, e)) if (1..=10).contains(&e.chars().count()) => format!(".{e}"),
            _ => String::new(),
        };
        let stem: String = name.chars().take(NAME_MAX - ext.chars().count()).collect();
        name = format!("{}{ext}", stem.trim_end_matches(|c: char| c == '.' || c.is_whitespace()));
    }
    let stem = name.split('.').next().unwrap_or_default().trim_end().to_ascii_uppercase();
    let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$")
        || (stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem[3..].chars().all(|c| c.is_ascii_digit() || "¹²³".contains(c)));
    if name.is_empty() {
        "fichier".into()
    } else if reserved {
        format!("_{name}")
    } else {
        name
    }
}

/// Jour civil (année, mois, jour) d'un nombre de jours depuis le 1er janvier 1970.
fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// Le dossier d'un jour : `2026-10-30`.
fn day_name(days: i64) -> String {
    let (y, m, d) = civil(days);
    format!("{y:04}-{m:02}-{d:02}")
}

/// Le nombre de jours depuis 1970 d'un nom de dossier de jour, s'il en est un.
fn day_of(name: &str) -> Option<i64> {
    let mut parts = name.split('-');
    let (y, m, d) = (parts.next()?.parse::<i64>().ok()?, parts.next()?.parse().ok()?, parts.next()?.parse().ok()?);
    if parts.next().is_some() || name.len() != 10 {
        return None;
    }
    // Les jours se suivent : le premier dont le nom correspond, autour de l'estimation.
    let guess = (y - 1970) * 365 + (y - 1969) / 4;
    (guess - 2..guess + 370).find(|&n| civil(n) == (y, m, d))
}

/// Efface les dossiers de jours plus vieux que [`KEEP_DAYS`] (les autres entrées de `dir` restent).
fn sweep(dir: &Path, today: i64) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let old = day_of(&name).is_some_and(|d| today - d > KEEP_DAYS);
        // Jamais au travers d'un lien : seulement un vrai dossier.
        if old && e.file_type().is_ok_and(|t| t.is_dir()) {
            let _ = fs::remove_dir_all(e.path());
        }
    }
}

/// Range `data`, reçu sous le nom `name`, dans le dossier du jour sous `dir` (créé au besoin) ; rend le chemin écrit.
/// Le nom porte l'heure (`143205-capture.png`) et n'écrase jamais rien : `-2`, `-3`… si un fichier l'a déjà.
pub fn save(dir: &Path, name: &str, data: &[u8], now: SystemTime) -> Result<PathBuf, String> {
    let secs = now.duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let today = secs.div_euclid(86_400);
    sweep(dir, today);
    let day = dir.join(day_name(today));
    fs::create_dir_all(&day).map_err(|e| format!("Dossier des fichiers reçus impossible à créer : {e}"))?;
    let used: u64 = fs::read_dir(&day)
        .map(|r| r.flatten().filter_map(|e| e.metadata().ok()).filter(|m| m.is_file()).map(|m| m.len()).sum())
        .unwrap_or(0);
    if used + data.len() as u64 > DAY_MAX {
        return Err("Trop de fichiers reçus aujourd'hui (200 Mo) : refusé.".into());
    }
    let clock = secs.rem_euclid(86_400);
    let stamp = format!("{:02}{:02}{:02}", clock / 3600, clock / 60 % 60, clock % 60);
    let name = clean_name(name);
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s.to_string(), format!(".{e}")),
        _ => (name.clone(), String::new()),
    };
    for n in 1..100 {
        let file = if n == 1 { format!("{stamp}-{stem}{ext}") } else { format!("{stamp}-{stem}-{n}{ext}") };
        let path = day.join(file);
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut f) => {
                if let Err(e) = f.write_all(data).and_then(|_| f.flush()) {
                    drop(f);
                    let _ = fs::remove_file(&path);
                    return Err(format!("Fichier impossible à écrire : {e}"));
                }
                return Ok(path);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(format!("Fichier impossible à créer : {e}")),
        }
    }
    Err("Trop de fichiers du même nom à la même seconde.".into())
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn a_name_from_the_phone_is_made_safe() {
        assert_eq!(clean_name("IMG_2041.jpg"), "IMG_2041.jpg");
        assert_eq!(clean_name("../../Windows/system32/x.dll"), "x.dll");
        assert_eq!(clean_name(r"..\..\cle"), "cle");
        assert_eq!(clean_name("rapport:flux.txt"), "rapport_flux.txt", "pas de flux secondaire");
        assert_eq!(clean_name("a<b>c|d?e*f\"g.txt"), "a_b_c_d_e_f_g.txt");
        assert_eq!(clean_name("ligne\nsuivante.txt"), "ligne_suivante.txt");
        assert_eq!(clean_name(" .. "), "fichier");
        assert_eq!(clean_name(""), "fichier");
        assert_eq!(clean_name("..."), "fichier");
        assert_eq!(clean_name("nom. "), "nom");
        assert_eq!(clean_name("CON.txt"), "_CON.txt");
        assert_eq!(clean_name("com1"), "_com1");
        assert_eq!(clean_name("Lpt9.log"), "_Lpt9.log");
        assert_eq!(clean_name("console.txt"), "console.txt");
        let long = format!("{}.jpeg", "é".repeat(200));
        let short = clean_name(&long);
        assert_eq!(short.chars().count(), 80);
        assert!(short.ends_with("é.jpeg"));
    }

    #[test]
    fn days_are_named_and_read_back() {
        assert_eq!(day_name(0), "1970-01-01");
        assert_eq!(day_name(20_756), "2026-10-30");
        assert_eq!(day_name(11_016), "2000-02-29");
        for d in [0, 59, 60, 11_016, 20_756, 30_000] {
            assert_eq!(day_of(&day_name(d)), Some(d));
        }
        assert_eq!(day_of("notes"), None);
        assert_eq!(day_of("2026-10-30-x"), None);
    }

    /// Le fichier est rangé dans le dossier du jour, sous l'heure et son nom nettoyé ; un second du même nom à la même
    /// seconde ne l'écrase pas ; les jours de plus d'une semaine sont effacés, pas le reste.
    #[test]
    fn a_file_is_saved_under_its_day_without_overwriting() {
        let dir = std::env::temp_dir().join(format!("eyrie-televerses-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let old = dir.join("2026-10-01");
        fs::create_dir_all(&old).unwrap();
        fs::write(old.join("vieux.txt"), b"x").unwrap();
        fs::create_dir_all(dir.join("autre")).unwrap();
        let now = UNIX_EPOCH + Duration::from_secs(20_756 * 86_400 + 14 * 3600 + 32 * 60 + 5);
        let a = save(&dir, "../capture.png", b"png", now).unwrap();
        assert_eq!(a, dir.join("2026-10-30").join("143205-capture.png"));
        assert_eq!(fs::read(&a).unwrap(), b"png");
        let b = save(&dir, "capture.png", b"autre", now).unwrap();
        assert_eq!(b.file_name().unwrap(), "143205-capture-2.png");
        assert_eq!(fs::read(&a).unwrap(), b"png", "le premier n'est pas écrasé");
        assert!(!old.exists(), "plus d'une semaine : effacé");
        assert!(dir.join("autre").exists(), "ce qui n'est pas un jour reste");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_day_receives_a_bounded_amount() {
        let dir = std::env::temp_dir().join(format!("eyrie-televerses-plein-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let now = UNIX_EPOCH + Duration::from_secs(20_756 * 86_400);
        let day = dir.join("2026-10-30");
        fs::create_dir_all(&day).unwrap();
        let f = fs::File::create(day.join("gros.bin")).unwrap();
        f.set_len(DAY_MAX - 2).unwrap();
        assert!(save(&dir, "a.txt", b"ab", now).is_ok());
        assert!(save(&dir, "b.txt", b"c", now).unwrap_err().contains("200 Mo"));
        let _ = fs::remove_dir_all(&dir);
    }
}
