//! Ressources Windows de l'exécutable : version (Propriétés > Détails, Gestionnaire des tâches),
//! tirée de Cargo.toml, et icône (le nid de `src/nest.rs`, à plusieurs tailles). Repris de Fawkes.
//!
//! La toolchain GNU de ce poste n'a ni `rc.exe` ni `windres` : on écrit directement le fichier
//! `.res` (format binaire documenté de Win32) et on le donne à l'éditeur de liens (rust-lld), qui
//! sait l'intégrer. Formats : « Resource File Formats », « VS_VERSIONINFO » et « Icons » sur
//! learn.microsoft.com.

use std::path::PathBuf;

#[path = "src/nest.rs"]
mod nest;

/// Tailles de l'icône : petites (barre de titre, listes) à 256 (grandes icônes de l'Explorateur).
const ICON_SIZES: [u32; 8] = [16, 20, 24, 32, 40, 48, 64, 256];
const RT_ICON: u16 = 3;
const RT_GROUP_ICON: u16 = 14;
const RT_VERSION: u16 = 16;

/// Français (France), table de chaînes Unicode.
const LANG: u16 = 0x040C;
const CODEPAGE: u16 = 1200;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=Cargo.toml");
    println!("cargo:rerun-if-changed=src/nest.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let env = |k: &str| std::env::var(k).unwrap_or_default();
    let name = env("CARGO_PKG_NAME");
    let product: String = name.chars().take(1).flat_map(char::to_uppercase).chain(name.chars().skip(1)).collect();
    let version = env("CARGO_PKG_VERSION");
    let authors = env("CARGO_PKG_AUTHORS").replace(':', ", ");
    let num = |k: &str| env(k).parse::<u32>().unwrap_or(0);
    let (major, minor, patch) =
        (num("CARGO_PKG_VERSION_MAJOR"), num("CARGO_PKG_VERSION_MINOR"), num("CARGO_PKG_VERSION_PATCH"));

    let strings = [
        ("CompanyName", authors.clone()),
        ("FileDescription", product.clone()),
        ("FileVersion", version.clone()),
        ("InternalName", name.clone()),
        ("LegalCopyright", format!("© {authors}")),
        ("OriginalFilename", format!("{name}.exe")),
        ("ProductName", product),
        ("ProductVersion", version),
        ("Comments", env("CARGO_PKG_DESCRIPTION")),
    ];
    let icons: Vec<(u32, Vec<u8>)> = ICON_SIZES.iter().map(|&s| (s, icon_image(s))).collect();
    let mut resources = vec![(RT_VERSION, 1, 0x0030, version_info([major, minor, patch, 0], &strings))];
    for (i, (_, img)) in icons.iter().enumerate() {
        resources.push((RT_ICON, i as u16 + 1, 0x1010, img.clone()));
    }
    resources.push((RT_GROUP_ICON, 1, 0x1030, icon_dir(&icons, |i| (i as u16 + 1).to_le_bytes().to_vec())));

    let out_dir = PathBuf::from(env("OUT_DIR"));
    let out = out_dir.join("eyrie.res");
    std::fs::write(&out, res_file(&resources)).expect("écriture de eyrie.res");
    // Le même jeu d'icônes en fichier .ico, pour l'aperçu.
    std::fs::write(out_dir.join("eyrie.ico"), ico_file(&icons)).expect("écriture de eyrie.ico");
    // Pour tous les exécutables du paquet, tests compris (`exe_has_version_info`, `exe_has_icon`).
    println!("cargo:rustc-link-arg={}", out.display());
}

/// Image d'icône 32 bits : BITMAPINFOHEADER, pixels BGRA (lignes de bas en haut), puis masque
/// de transparence 1 bit, encore lu par d'anciens moteurs de rendu.
fn icon_image(size: u32) -> Vec<u8> {
    let rgba = nest::render(size as usize);
    let s = size as usize;
    let mask_stride = s.div_ceil(32) * 4;
    let mut b = Vec::new();
    // biSize, biWidth, biHeight (couleurs + masque)
    for v in [40, size, size * 2] {
        b.extend(v.to_le_bytes());
    }
    b.extend(1u16.to_le_bytes()); // biPlanes
    b.extend(32u16.to_le_bytes()); // biBitCount
    // biCompression (BI_RGB), biSizeImage, résolutions, couleurs utilisées / importantes
    for v in [0, (s * s * 4 + mask_stride * s) as u32, 0, 0, 0, 0] {
        b.extend(v.to_le_bytes());
    }
    for y in (0..s).rev() {
        for x in 0..s {
            let p = &rgba[(y * s + x) * 4..][..4];
            b.extend([p[2], p[1], p[0], p[3]]);
        }
    }
    for y in (0..s).rev() {
        let mut row = vec![0u8; mask_stride];
        for x in (0..s).filter(|x| rgba[(y * s + x) * 4 + 3] == 0) {
            row[x / 8] |= 0x80 >> (x % 8);
        }
        b.extend(row);
    }
    b
}

/// En-tête de groupe d'icônes, commun à la ressource RT_GROUP_ICON et au fichier .ico : seule la
/// fin de chaque entrée diffère (n° de ressource, ou position dans le fichier).
fn icon_dir(icons: &[(u32, Vec<u8>)], tail: impl Fn(usize) -> Vec<u8>) -> Vec<u8> {
    let mut b: Vec<u8> = [0u16, 1, icons.len() as u16].iter().flat_map(|w| w.to_le_bytes()).collect();
    for (i, (size, img)) in icons.iter().enumerate() {
        // 0 signifie 256.
        let dim = if *size >= 256 { 0 } else { *size as u8 };
        b.extend([dim, dim, 0, 0]); // largeur, hauteur, couleurs de palette, réservé
        b.extend(1u16.to_le_bytes()); // plans
        b.extend(32u16.to_le_bytes()); // bits par pixel
        b.extend((img.len() as u32).to_le_bytes());
        b.extend(tail(i));
    }
    b
}

fn ico_file(icons: &[(u32, Vec<u8>)]) -> Vec<u8> {
    let header = 6 + 16 * icons.len();
    let offsets: Vec<usize> =
        icons.iter().scan(header, |at, (_, img)| Some(std::mem::replace(at, *at + img.len()))).collect();
    let mut b = icon_dir(icons, |i| (offsets[i] as u32).to_le_bytes().to_vec());
    for (_, img) in icons {
        b.extend(img);
    }
    b
}

fn pad4(b: &mut Vec<u8>) {
    while !b.len().is_multiple_of(4) {
        b.push(0);
    }
}

fn utf16z(s: &str) -> Vec<u8> {
    s.encode_utf16().chain([0]).flat_map(u16::to_le_bytes).collect()
}

/// Bloc de VS_VERSIONINFO : wLength, wValueLength, wType, clé, valeur, enfants (alignés sur 4 octets).
fn block(key: &str, text: bool, value: &[u8], value_len: u16, children: &[Vec<u8>]) -> Vec<u8> {
    let mut b = vec![0, 0];
    b.extend(value_len.to_le_bytes());
    b.extend(u16::from(text).to_le_bytes());
    b.extend(utf16z(key));
    pad4(&mut b);
    b.extend(value);
    for c in children {
        pad4(&mut b);
        b.extend(c);
    }
    let len = u16::try_from(b.len()).expect("ressource de version trop grande");
    b[..2].copy_from_slice(&len.to_le_bytes());
    b
}

fn version_info(v: [u32; 4], strings: &[(&str, String)]) -> Vec<u8> {
    let ms = (v[0] << 16) | v[1];
    let ls = (v[2] << 16) | v[3];
    // VS_FIXEDFILEINFO : signature, version de structure, versions fichier/produit, masque, drapeaux,
    // VOS_NT_WINDOWS32, VFT_APP, sous-type, date.
    let fixed: Vec<u8> = [0xFEEF_04BD, 0x0001_0000, ms, ls, ms, ls, 0x3F, 0, 0x0004_0004, 1, 0, 0, 0]
        .iter()
        .flat_map(|d: &u32| d.to_le_bytes())
        .collect();
    let entries: Vec<Vec<u8>> = strings
        .iter()
        .filter(|(_, v)| !v.is_empty())
        .map(|(k, v)| block(k, true, &utf16z(v), (v.encode_utf16().count() + 1) as u16, &[]))
        .collect();
    let table = block(&format!("{LANG:04X}{CODEPAGE:04X}"), true, &[], 0, &entries);
    let string_info = block("StringFileInfo", true, &[], 0, &[table]);
    let translation: Vec<u8> = [LANG, CODEPAGE].iter().flat_map(|w| w.to_le_bytes()).collect();
    let var_info = block("VarFileInfo", true, &[], 0, &[block("Translation", false, &translation, 4, &[])]);
    block("VS_VERSION_INFO", false, &fixed, fixed.len() as u16, &[string_info, var_info])
}

/// Fichier `.res` : une entrée vide obligatoire, puis chaque ressource (type, n°, options, données).
fn res_file(resources: &[(u16, u16, u16, Vec<u8>)]) -> Vec<u8> {
    let entry = |data: &[u8], rtype: u16, id: u16, flags: u16, lang: u16| {
        let mut b = Vec::new();
        b.extend((data.len() as u32).to_le_bytes()); // DataSize
        b.extend(32u32.to_le_bytes()); // HeaderSize
        b.extend([0xFFFF, rtype, 0xFFFF, id].iter().flat_map(|w: &u16| w.to_le_bytes())); // TYPE, NAME
        b.extend(0u32.to_le_bytes()); // DataVersion
        b.extend(flags.to_le_bytes()); // MemoryFlags
        b.extend(lang.to_le_bytes()); // LanguageId
        b.extend(0u64.to_le_bytes()); // Version, Characteristics
        b.extend(data);
        pad4(&mut b);
        b
    };
    let mut res = entry(&[], 0, 0, 0, 0);
    for (rtype, id, flags, data) in resources {
        res.extend(entry(data, *rtype, *id, *flags, LANG));
    }
    res
}
