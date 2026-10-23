//! Fichiers d'état robustes, à la manière de Fawkes, Patronus et Owlery : écriture atomique,
//! version et `saved_by`, copie avant un changement de version, fichier illisible mis de côté
//! (jamais écrasé). Les champs inconnus se gardent avec `#[serde(flatten)] extra` dans le type
//! enregistré : indispensable dès que deux programmes écrivent le même fichier.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use serde::de::DeserializeOwned;

/// Écrit `bytes` à la place de `path` sans jamais laisser un fichier à moitié écrit : fichier
/// temporaire voisin, `sync_all`, puis renommage (qui remplace l'ancien).
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let tmp = path.with_file_name(format!(".{name}.{}.tmp", std::process::id()));
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

pub struct Loaded<T> {
    pub value: T,
    /// Ce qui n'a pas pu être relu, à montrer à l'utilisateur.
    pub problems: Vec<String>,
    /// Faux si le fichier est d'une version plus récente : on ne l'écrase pas.
    pub writable: bool,
}

fn stamp() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    path.with_file_name(format!("{stem}.{suffix}.json"))
}

/// Relit un fichier JSON versionné. Absent : valeur par défaut. Illisible : mis de côté
/// (`<nom>.illisible-<date>.json`). Plus récent que `version` : lu au mieux, mais pas réécrit.
/// Plus ancien : copié (`<nom>.avant-v<version>.json`) avant la première écriture.
pub fn load_json<T: DeserializeOwned + Default>(path: &Path, version: u32) -> Loaded<T> {
    let mut problems = Vec::new();
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Loaded { value: T::default(), problems, writable: true };
        }
        Err(e) => {
            problems.push(format!("{} illisible ({e}) : réglages par défaut pour cette session.", path.display()));
            return Loaded { value: T::default(), problems, writable: false };
        }
    };
    let parsed: Result<serde_json::Value, _> = serde_json::from_str(&text);
    let value = parsed.and_then(|v| {
        let saved = v.get("version").and_then(|n| n.as_u64()).unwrap_or(0) as u32;
        serde_json::from_value::<T>(v).map(|t| (t, saved))
    });
    match value {
        Ok((t, saved)) => {
            let mut writable = true;
            if saved > version {
                problems.push(format!(
                    "{} vient d'une version plus récente d'Eyrie (v{saved}) : il ne sera pas modifié.",
                    path.display()
                ));
                writable = false;
            } else if saved < version && saved > 0 {
                let _ = std::fs::copy(path, sibling(path, &format!("avant-v{version}")));
            }
            Loaded { value: t, problems, writable }
        }
        Err(e) => {
            let aside = sibling(path, &format!("illisible-{}", stamp()));
            let _ = std::fs::rename(path, &aside);
            problems.push(format!("{} illisible ({e}) : mis de côté sous {}.", path.display(), aside.display()));
            Loaded { value: T::default(), problems, writable: true }
        }
    }
}

/// Écrit `value` avec `version` et `saved_by` (version du programme qui écrit).
pub fn save_json<T: Serialize>(path: &Path, value: &T, version: u32, saved_by: &str) -> std::io::Result<()> {
    let mut v = serde_json::to_value(value).map_err(std::io::Error::other)?;
    if let Some(obj) = v.as_object_mut() {
        obj.insert("version".into(), version.into());
        obj.insert("saved_by".into(), saved_by.into());
    }
    let text = serde_json::to_string_pretty(&v).map_err(std::io::Error::other)?;
    write_atomic(path, text.as_bytes())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use serde::Deserialize;

    use super::*;

    #[derive(Default, Serialize, Deserialize, PartialEq, Debug)]
    struct S {
        #[serde(default)]
        a: u32,
        #[serde(flatten)]
        extra: BTreeMap<String, serde_json::Value>,
    }

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("suite-sdk-state-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn unknown_fields_survive_a_round_trip() {
        let d = dir("inconnus");
        let p = d.join("state.json");
        std::fs::write(&p, r#"{"version":1,"a":3,"futur":{"x":1}}"#).unwrap();
        let l: Loaded<S> = load_json(&p, 1);
        assert!(l.problems.is_empty());
        save_json(&p, &l.value, 1, "test").unwrap();
        let back: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
        assert_eq!(back["futur"]["x"], 1);
        assert_eq!(back["a"], 3);
        assert_eq!(back["saved_by"], "test");
    }

    #[test]
    fn unreadable_file_is_set_aside_not_overwritten() {
        let d = dir("illisible");
        let p = d.join("state.json");
        std::fs::write(&p, "{pas du json").unwrap();
        let l: Loaded<S> = load_json(&p, 1);
        assert_eq!(l.value, S::default());
        assert_eq!(l.problems.len(), 1);
        assert!(!p.exists());
        let aside = std::fs::read_dir(&d).unwrap().filter_map(|e| e.ok()).any(|e| {
            let n = e.file_name().to_string_lossy().into_owned();
            n.starts_with("state.illisible-")
        });
        assert!(aside);
    }

    #[test]
    fn newer_file_is_not_writable_and_older_is_copied() {
        let d = dir("versions");
        let p = d.join("state.json");
        std::fs::write(&p, r#"{"version":9,"a":1}"#).unwrap();
        assert!(!load_json::<S>(&p, 2).writable);
        std::fs::write(&p, r#"{"version":1,"a":1}"#).unwrap();
        assert!(load_json::<S>(&p, 2).writable);
        assert!(d.join("state.avant-v2.json").exists());
    }
}
