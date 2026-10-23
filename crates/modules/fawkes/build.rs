//! Fawkes n'a pas encore de `lib.rs` : ses modules sont déclarés dans `src/main.rs`. On relit ces
//! déclarations et on écrit, dans `OUT_DIR`, les mêmes `mod` avec un `#[path]` vers le dépôt voisin.
//! Les sources de Fawkes sont compilées telles quelles, à la racine de cette crate (leurs chemins
//! `crate::…` restent justes) ; rien n'est écrit dans `../Fawkes`.

use std::path::{Path, PathBuf};

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let fawkes = std::env::var_os("SUITE_FAWKES_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| manifest.join("../../../../Fawkes"));
    let fawkes = fawkes.canonicalize().unwrap_or_else(|_| panic!("dépôt Fawkes introuvable : {}", fawkes.display()));
    let src = fawkes.join("src");
    let main_rs = src.join("main.rs");
    println!("cargo:rerun-if-changed={}", main_rs.display());
    println!("cargo:rerun-if-changed={}", fawkes.join("Cargo.toml").display());
    println!("cargo:rerun-if-env-changed=SUITE_FAWKES_DIR");

    let text = std::fs::read_to_string(&main_rs).expect("lecture de Fawkes/src/main.rs");
    let mut out = String::from("// Écrit par build.rs : les modules de Fawkes/src/main.rs, lus dans le dépôt voisin.\n");
    for line in text.lines() {
        let Some(name) = line.trim().strip_prefix("mod ").and_then(|r| r.strip_suffix(';')) else { continue };
        let file = module_file(&src, name);
        out += &format!(
            "#[allow(dead_code, unused_imports, unused_variables, unused_mut, clippy::all)]\n#[path = {:?}]\npub mod {name};\n",
            file.display().to_string()
        );
    }
    std::fs::write(PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("fawkes_mods.rs"), out).unwrap();

    check_version(&fawkes);
}

fn module_file(src: &Path, name: &str) -> PathBuf {
    let flat = src.join(format!("{name}.rs"));
    if flat.exists() { flat } else { src.join(name).join("mod.rs") }
}

/// Le code de Fawkes lit `CARGO_PKG_VERSION` : cette crate doit porter la même version.
fn check_version(fawkes: &Path) {
    let toml = std::fs::read_to_string(fawkes.join("Cargo.toml")).unwrap_or_default();
    let theirs = toml.lines().find_map(|l| l.strip_prefix("version = ")).map(|v| v.trim_matches('"').to_string());
    let ours = std::env::var("CARGO_PKG_VERSION").unwrap();
    if theirs.as_deref() != Some(ours.as_str()) {
        println!(
            "cargo:warning=Fawkes est en version {theirs:?}, suite-fawkes en {ours} : aligner crates/modules/fawkes/Cargo.toml"
        );
    }
}
