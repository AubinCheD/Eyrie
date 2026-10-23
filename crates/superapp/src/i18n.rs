//! Le catalogue de la suite (domaine `suite`) : le texte français du code sert de clé, la
//! traduction anglaise est dans `locales/en.json`.

use suite_sdk::i18n::Catalog;

static CATALOG: Catalog = Catalog::new(include_str!("../locales/en.json"));

pub fn tr(fr: &'static str) -> &'static str {
    CATALOG.tr(fr)
}

/// Marque un texte à traduire plus loin (`tr(variable)`) : la clé est relevée par le test du
/// catalogue.
pub const fn mark(fr: &'static str) -> &'static str {
    fr
}

/// Les clés `tr("…")` et `mark("…")` d'un texte source.
#[cfg(test)]
pub fn keys_in(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    for (needle, skip) in [("tr(\"", 4), ("mark(\"", 6)] {
        let mut from = 0;
        while let Some(i) = text[from..].find(needle).map(|i| i + from) {
            from = i + skip;
            // `from_str("…")`, `to_str("…")` : pas une clé.
            if i > 0 && (bytes[i - 1].is_ascii_alphanumeric() || bytes[i - 1] == b'_') {
                continue;
            }
            let rest = &text[i + skip..];
            let mut key = String::new();
            let mut chars = rest.chars();
            while let Some(c) = chars.next() {
                match c {
                    '\\' => match chars.next() {
                        Some('n') => key.push('\n'),
                        Some(other) => key.push(other),
                        None => break,
                    },
                    '"' => break,
                    c => key.push(c),
                }
            }
            if !key.is_empty() {
                out.push(key);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    /// Chaque `tr("…")` des sources a sa traduction, et le catalogue n'a pas d'entrée orpheline.
    #[test]
    fn catalog_matches_sources() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut used = BTreeSet::new();
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            // Ce fichier-ci ne contient que des exemples.
            if path.file_name().is_some_and(|n| n == "i18n.rs") {
                continue;
            }
            used.extend(super::keys_in(&std::fs::read_to_string(path).unwrap()));
        }
        let catalog: std::collections::BTreeMap<String, String> =
            serde_json::from_str(include_str!("../locales/en.json")).expect("en.json lisible");
        let missing: Vec<&String> = used.iter().filter(|k| !catalog.contains_key(*k)).collect();
        let orphans: Vec<&String> = catalog.keys().filter(|k| !used.contains(*k)).collect();
        assert!(missing.is_empty(), "traductions manquantes : {missing:#?}");
        assert!(orphans.is_empty(), "entrées orphelines : {orphans:#?}");
    }

    #[test]
    fn keys_are_read_like_the_compiler_reads_them() {
        let src = r#"tr("a « b »") mark("c\"d") from_str("x") tr(view.title())"#;
        assert_eq!(super::keys_in(src), vec!["a « b »".to_string(), "c\"d".to_string()]);
    }
}
