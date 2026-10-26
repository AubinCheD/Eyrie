//! Prix de l'API par modèle, en dollars par million de jetons : entrée, écriture de cache (5 min et 1 h), lecture de
//! cache, sortie. Relevés le 26/10/2026 sur <https://platform.claude.com/docs/en/about-claude/pricing>. Ni le mode
//! rapide, ni les suppléments de contexte long, ni l'inférence localisée ne sont comptés : c'est un équivalent API.

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Price {
    pub input: f64,
    pub write_5m: f64,
    pub write_1h: f64,
    pub read: f64,
    pub output: f64,
}

const fn p(input: f64, write_5m: f64, write_1h: f64, read: f64, output: f64) -> Price {
    Price { input, write_5m, write_1h, read, output }
}

/// Le prix d'un modèle d'après son identifiant (`claude-opus-5-5`, `claude-3-5-haiku-20241022`…) ; `prompt` : jetons
/// d'entrée de la requête (Haiku 5.5 coûte plus au-delà de 100 000). `None` pour un modèle inconnu (`<synthetic>`).
pub fn price(model: &str, prompt: u64) -> Option<Price> {
    let m = model.to_ascii_lowercase();
    let has = |s: &str| m.contains(s);
    let price = if has("fable") || has("mythos") {
        if has("5-1") { p(10.0, 12.5, 20.0, 0.25, 50.0) } else { p(10.0, 12.5, 20.0, 1.0, 50.0) }
    } else if has("opus") {
        if has("opus-5-5") {
            p(4.0, 5.0, 8.0, 0.2, 20.0)
        } else if ["opus-5", "opus-4-8", "opus-4-7", "opus-4-6", "opus-4-5"].iter().any(|v| has(v)) {
            p(5.0, 6.25, 10.0, 0.5, 25.0)
        } else {
            // Opus 4 et 4.1, Opus 3.
            p(15.0, 18.75, 30.0, 1.5, 75.0)
        }
    } else if has("sonnet") {
        if has("sonnet-5-5") {
            p(2.0, 2.5, 4.0, 0.1, 10.0)
        } else if has("sonnet-5") {
            p(2.0, 2.5, 4.0, 0.2, 10.0)
        } else {
            p(3.0, 3.75, 6.0, 0.3, 15.0)
        }
    } else if has("haiku") {
        if has("haiku-5-5") {
            if prompt > 100_000 { p(0.5, 0.625, 1.0, 0.05, 2.5) } else { p(0.1, 0.125, 0.2, 0.01, 0.5) }
        } else if has("haiku-4-5") {
            p(1.0, 1.25, 2.0, 0.1, 5.0)
        } else if has("3-5-haiku") {
            p(0.8, 1.0, 1.6, 0.08, 4.0)
        } else {
            p(0.25, 0.3, 0.5, 0.03, 1.25)
        }
    } else {
        return None;
    };
    Some(price)
}

/// Nom court d'un modèle : « Opus 5.5 », « Sonnet 4.6 », « Haiku 3.5 » ; l'identifiant tel quel sinon.
pub fn short_name(model: &str) -> String {
    let m = model.to_ascii_lowercase();
    for family in ["fable", "mythos", "opus", "sonnet", "haiku"] {
        let Some(at) = m.find(family) else { continue };
        let name = format!("{}{}", family[..1].to_uppercase(), &family[1..]);
        // « claude-opus-4-6 » : la version suit ; « claude-3-5-haiku » : elle précède.
        let after: Vec<&str> = m[at + family.len()..].split('-').filter(|s| !s.is_empty()).collect();
        let digits = |parts: &[&str]| -> Vec<String> {
            parts
                .iter()
                .take_while(|s| s.len() <= 2 && s.chars().all(|c| c.is_ascii_digit()))
                .map(|s| s.to_string())
                .collect()
        };
        let mut version = digits(&after);
        if version.is_empty() {
            let before: Vec<&str> = m[..at].split('-').filter(|s| !s.is_empty() && *s != "claude").collect();
            version = digits(&before);
        }
        return if version.is_empty() { name } else { format!("{name} {}", version.join(".")) };
    }
    model.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_and_older_models_have_their_prices() {
        assert_eq!(price("claude-opus-5-5", 0).unwrap().output, 20.0);
        assert_eq!(price("claude-opus-4-6", 0).unwrap().input, 5.0);
        assert_eq!(price("claude-opus-4-1-20250805", 0).unwrap().input, 15.0);
        assert_eq!(price("claude-sonnet-5-5", 0).unwrap().read, 0.1);
        assert_eq!(price("claude-sonnet-5", 0).unwrap().read, 0.2);
        assert_eq!(price("claude-sonnet-4-5-20250929", 0).unwrap().output, 15.0);
        assert_eq!(price("claude-haiku-5-5", 10).unwrap().input, 0.1);
        assert_eq!(price("claude-haiku-5-5", 200_000).unwrap().input, 0.5, "plus cher au-delà de 100 000");
        assert_eq!(price("claude-3-5-haiku-20241022", 0).unwrap().output, 4.0);
        assert_eq!(price("claude-fable-5-1", 0).unwrap().read, 0.25);
        assert_eq!(price("claude-fable-5", 0).unwrap().read, 1.0);
        assert!(price("<synthetic>", 0).is_none());
    }

    #[test]
    fn short_names_read_like_the_product() {
        assert_eq!(short_name("claude-opus-5-5"), "Opus 5.5");
        assert_eq!(short_name("claude-sonnet-4-5-20250929"), "Sonnet 4.5");
        assert_eq!(short_name("claude-3-5-haiku-20241022"), "Haiku 3.5");
        assert_eq!(short_name("claude-opus-4-20250514"), "Opus 4");
        assert_eq!(short_name("<synthetic>"), "<synthetic>");
    }
}
