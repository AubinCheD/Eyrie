//! Les relevés des limites de l'abonnement, et la part de chaque conversation.
//!
//! Claude Code ne donne le taux des limites (fenêtre de 5 h, semaine) qu'à sa ligne d'état, dans l'objet JSON qu'il
//! lui passe (`rate_limits.five_hour` et `seven_day` : `used_percentage`, `resets_at`), avec la session en cours.
//! Le relais de Gringotts (la ligne d'état) range chaque relevé qui change quelque chose dans `releves.jsonl` :
//! c'est l'historique des limites, et la base de la part de chaque conversation.
//!
//! Part d'une conversation, telle que le relais la montre dans le terminal : la hausse du taux entre deux relevés
//! d'un même compte va à la conversation du second relevé (celle qui vient de travailler). Gringotts, qui lit aussi
//! les transcriptions, partage plus finement (`analysis`) et dépose ses chiffres dans `parts.json`, que le relais
//! préfère quand ils sont frais.

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Durée des deux fenêtres, en secondes.
pub const FIVE_HOURS: i64 = 5 * 3600;
pub const SEVEN_DAYS: i64 = 7 * 86_400;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Kind {
    FiveHour,
    SevenDay,
}

impl Kind {
    pub const ALL: [Kind; 2] = [Kind::FiveHour, Kind::SevenDay];

    pub fn duration(self) -> i64 {
        match self {
            Kind::FiveHour => FIVE_HOURS,
            Kind::SevenDay => SEVEN_DAYS,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Kind::FiveHour => "5 h",
            Kind::SevenDay => "semaine",
        }
    }
}

/// Une fenêtre relevée : part utilisée (0 à 100, plus en dépassement) et heure de remise à zéro.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Window {
    pub pct: f64,
    pub resets_at: i64,
}

/// Un relevé de la ligne d'état.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Reading {
    /// Secondes Unix.
    pub t: i64,
    /// Compte (`accountUuid`), sinon clé du dossier de configuration.
    pub account: String,
    pub org: Option<String>,
    pub email: Option<String>,
    pub org_name: Option<String>,
    /// Dossier de configuration de la session (clé de `paths::key`).
    pub config: Option<String>,
    pub session: Option<String>,
    pub model: Option<String>,
    pub cwd: Option<String>,
    pub five_hour: Option<Window>,
    pub seven_day: Option<Window>,
}

impl Reading {
    /// Compte et organisation : un même compte dans deux organisations a deux abonnements.
    pub fn key(&self) -> String {
        match &self.org {
            Some(o) => format!("{}_{o}", self.account),
            None => self.account.clone(),
        }
    }

    pub fn window(&self, kind: Kind) -> Option<Window> {
        match kind {
            Kind::FiveHour => self.five_hour,
            Kind::SevenDay => self.seven_day,
        }
    }

    /// Nom à montrer : l'adresse et l'organisation.
    pub fn label(&self) -> String {
        let who = self.email.clone().unwrap_or_else(|| self.account.chars().take(8).collect());
        match &self.org_name {
            Some(o) => format!("{who} · {o}"),
            None => who,
        }
    }
}

/// Relevé tiré de l'entrée de la ligne d'état : `None` sans `rate_limits` lisible ou sans compte connu (les champs du
/// compte sont pris comme le relais de Grimoire les prend : `oauthAccount` de `.claude.json`).
pub fn from_status(input: &Value, claude_json: Option<&Value>, config_dir: Option<&Path>, now: i64) -> Option<Reading> {
    let r = grimoire::limits::record_from_status(input, claude_json, config_dir, now)?;
    let text = |v: Option<&Value>| v.and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()).map(String::from);
    let model = input.get("model").and_then(|m| text(m.get("id")).or_else(|| text(Some(m))));
    let cwd = text(input.get("workspace").and_then(|w| w.get("current_dir"))).or_else(|| text(input.get("cwd")));
    let window =
        |w: Option<grimoire::limits::Window>| w.map(|w| Window { pct: w.used_percentage, resets_at: w.resets_at });
    Some(Reading {
        t: now,
        account: r.account,
        org: r.organization_uuid,
        email: r.email,
        org_name: r.organization,
        config: r.config_dir.map(grimoire::paths::key),
        session: text(input.get("session_id")),
        model,
        cwd,
        five_hour: window(r.five_hour),
        seven_day: window(r.seven_day),
    })
}

pub fn history_file(dir: &Path) -> PathBuf {
    dir.join("releves.jsonl")
}

/// Ajoute un relevé à l'historique (une ligne, écrite d'un seul coup).
pub fn append(dir: &Path, r: &Reading) -> std::io::Result<()> {
    fs::create_dir_all(dir)?;
    let mut line = serde_json::to_vec(r).map_err(std::io::Error::other)?;
    line.push(b'\n');
    fs::OpenOptions::new().create(true).append(true).open(history_file(dir))?.write_all(&line)
}

/// Tout l'historique, dans l'ordre des heures ; les lignes illisibles sont ignorées.
pub fn load(dir: &Path) -> Vec<Reading> {
    let Ok(text) = fs::read_to_string(history_file(dir)) else { return Vec::new() };
    let mut v: Vec<Reading> = text.lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
    v.sort_by_key(|r| r.t);
    v
}

/// Ce que le relais garde d'un compte entre deux relevés.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AccountState {
    pub last_t: i64,
    pub last_session: Option<String>,
    pub windows: BTreeMap<String, WindowState>,
}

/// Une fenêtre en cours : son dernier taux, et la part de chaque session depuis sa remise à zéro.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowState {
    pub resets_at: i64,
    pub pct: f64,
    pub sessions: BTreeMap<String, f64>,
}

fn kind_key(kind: Kind) -> &'static str {
    match kind {
        Kind::FiveHour => "five_hour",
        Kind::SevenDay => "seven_day",
    }
}

/// Un relevé arrive : la hausse depuis le précédent va à sa session. Rend vrai si le relevé apporte quelque chose à
/// l'historique (un taux a bougé, une autre session, une fenêtre nouvelle, ou cinq minutes depuis le dernier).
pub fn accumulate(state: &mut AccountState, r: &Reading) -> bool {
    let mut changed = state.last_session != r.session || r.t - state.last_t >= 300;
    for kind in Kind::ALL {
        let Some(w) = r.window(kind) else { continue };
        let ws = state.windows.entry(kind_key(kind).into()).or_default();
        let rise = if ws.resets_at == w.resets_at {
            (w.pct - ws.pct).max(0.0)
        } else {
            // Nouvelle fenêtre : tout ce qui est consommé depuis sa remise à zéro.
            ws.sessions.clear();
            changed = true;
            w.pct.max(0.0)
        };
        changed |= (w.pct - ws.pct).abs() > 1e-9;
        if rise > 0.0
            && let Some(s) = &r.session
        {
            *ws.sessions.entry(s.clone()).or_default() += rise;
        }
        ws.resets_at = w.resets_at;
        ws.pct = w.pct;
    }
    state.last_t = r.t;
    state.last_session = r.session.clone();
    changed
}

/// Nom de fichier sûr pour une clé de compte.
pub fn file_stem(key: &str) -> String {
    let safe = |c: char| c.is_ascii_alphanumeric() || c == '-' || c == '_';
    let flat: String = key.chars().map(|c| if safe(c) { c } else { '-' }).take(100).collect();
    if flat == key { flat } else { format!("{flat}-{:016x}", fnv1a(key.as_bytes())) }
}

fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, b| (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3))
}

pub fn state_file(dir: &Path, key: &str) -> PathBuf {
    dir.join("etat").join(format!("{}.json", file_stem(key)))
}

pub fn load_state(dir: &Path, key: &str) -> AccountState {
    fs::read(state_file(dir, key)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

pub fn save_state(dir: &Path, key: &str, s: &AccountState) -> std::io::Result<()> {
    let path = state_file(dir, key);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    grimoire::state::write_atomic(&path, &serde_json::to_vec(s).map_err(std::io::Error::other)?)
}

/// Les parts calculées par Gringotts, déposées pour le relais : session → fenêtre → (remise à zéro, part).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Parts {
    pub written_at: i64,
    pub sessions: BTreeMap<String, BTreeMap<String, (i64, f64)>>,
}

pub fn parts_file(dir: &Path) -> PathBuf {
    dir.join("parts.json")
}

impl Parts {
    pub fn load(dir: &Path) -> Parts {
        fs::read(parts_file(dir)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    pub fn save(&self, dir: &Path) -> std::io::Result<()> {
        fs::create_dir_all(dir)?;
        grimoire::state::write_atomic(&parts_file(dir), &serde_json::to_vec(self).map_err(std::io::Error::other)?)
    }

    pub fn set(&mut self, session: &str, kind: Kind, resets_at: i64, pct: f64) {
        self.sessions.entry(session.to_string()).or_default().insert(kind_key(kind).into(), (resets_at, pct));
    }

    fn get(&self, session: &str, kind: Kind, resets_at: i64) -> Option<f64> {
        let (at, pct) = *self.sessions.get(session)?.get(kind_key(kind))?;
        (at == resets_at).then_some(pct)
    }
}

/// « 23 % » ; « 0,4 % » sous 1 %.
pub fn pct_text(p: f64) -> String {
    if p > 0.0 && p < 1.0 { format!("{:.1} %", p).replace('.', ",") } else { format!("{:.0} %", p) }
}

/// L'heure locale d'un instant Unix, au format `fmt`.
pub fn local(ts: i64, fmt: &str) -> String {
    chrono::DateTime::from_timestamp(ts, 0)
        .map(|d| d.with_timezone(&chrono::Local).format(fmt).to_string())
        .unwrap_or_default()
}

/// Ligne montrée dans le terminal : `5 h 23 % (conv. 4 %) · semaine 41 % (conv. 1 %) · ↻ 14:05`. La part de la
/// conversation vient de `parts` s'ils sont frais (moins de 10 minutes), sinon du relais (`state`).
pub fn line(r: &Reading, state: &AccountState, parts: &Parts, now: i64) -> String {
    let mut out = Vec::new();
    for kind in Kind::ALL {
        let Some(w) = r.window(kind) else { continue };
        let fresh = now - parts.written_at < 600;
        let mine = r.session.as_deref().and_then(|s| {
            fresh
                .then(|| parts.get(s, kind, w.resets_at))
                .flatten()
                .or_else(|| state.windows.get(kind_key(kind)).and_then(|ws| ws.sessions.get(s).copied()))
        });
        let mut part = format!("{} {}", kind.label(), pct_text(w.pct));
        if let Some(m) = mine {
            part.push_str(&format!(" (conv. {})", pct_text(m)));
        }
        out.push(part);
    }
    if let Some(w) = r.five_hour.filter(|w| w.resets_at > now) {
        out.push(format!("↻ {}", local(w.resets_at, "%H:%M")));
    }
    out.join(" · ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    pub fn status(session: &str, five: f64, week: f64) -> Value {
        json!({
            "session_id": session,
            "model": {"id": "claude-opus-5-5", "display_name": "Opus 5.5"},
            "workspace": {"current_dir": "C:\\dev\\ozmo"},
            "rate_limits": {
                "five_hour": {"used_percentage": five, "resets_at": 20_000},
                "seven_day": {"used_percentage": week, "resets_at": 600_000}
            }
        })
    }

    fn account() -> Value {
        json!({"oauthAccount": {"accountUuid": "u-1", "emailAddress": "ada@contoso.example", "organizationUuid": "o-1", "organizationName": "PDLC"}})
    }

    #[test]
    fn a_status_input_becomes_a_reading() {
        let r = from_status(&status("s1", 23.0, 41.0), Some(&account()), None, 1_000).unwrap();
        assert_eq!((r.account.as_str(), r.org.as_deref(), r.session.as_deref()), ("u-1", Some("o-1"), Some("s1")));
        assert_eq!(r.five_hour, Some(Window { pct: 23.0, resets_at: 20_000 }));
        assert_eq!(r.model.as_deref(), Some("claude-opus-5-5"));
        assert_eq!(r.cwd.as_deref(), Some(r"C:\dev\ozmo"));
        assert_eq!(r.label(), "ada@contoso.example · PDLC");
        assert!(from_status(&json!({"session_id": "s1"}), Some(&account()), None, 1).is_none(), "sans limites");
    }

    #[test]
    fn each_rise_goes_to_the_session_that_reports_it() {
        let mut st = AccountState::default();
        let read = |s: &str, five: f64, t: i64| {
            let mut r = from_status(&status(s, five, 10.0), Some(&account()), None, t).unwrap();
            r.t = t;
            r
        };
        assert!(accumulate(&mut st, &read("a", 10.0, 100)));
        assert!(accumulate(&mut st, &read("a", 14.0, 110)));
        assert!(accumulate(&mut st, &read("b", 15.0, 120)), "autre session");
        assert!(!accumulate(&mut st, &read("b", 15.0, 130)), "rien de nouveau");
        let five = &st.windows["five_hour"];
        assert_eq!((five.sessions["a"], five.sessions["b"]), (14.0, 1.0));
        let line = line(&read("a", 15.0, 140), &st, &Parts::default(), 140);
        assert!(line.starts_with("5 h 15 % (conv. 14 %) · semaine 10 % (conv. 10 %)"), "{line}");
        // Les parts de Gringotts, fraîches, l'emportent.
        let mut parts = Parts { written_at: 140, ..Default::default() };
        parts.set("a", Kind::FiveHour, 20_000, 9.5);
        let line2 = super::line(&read("a", 15.0, 140), &st, &parts, 140);
        assert!(line2.starts_with("5 h 15 % (conv. 10 %)"), "{line2}");
    }

    #[test]
    fn a_new_window_starts_from_zero() {
        let mut st = AccountState::default();
        let mut r = from_status(&status("a", 80.0, 10.0), Some(&account()), None, 100).unwrap();
        accumulate(&mut st, &r);
        r.five_hour = Some(Window { pct: 5.0, resets_at: 40_000 });
        r.session = Some("b".into());
        accumulate(&mut st, &r);
        let five = &st.windows["five_hour"];
        assert_eq!(five.sessions.len(), 1, "l'ancienne fenêtre est oubliée");
        assert_eq!(five.sessions["b"], 5.0);
    }

    #[test]
    fn history_is_appended_and_read_back_in_order() {
        let t = tempfile::tempdir().unwrap();
        let mut r = from_status(&status("a", 1.0, 2.0), Some(&account()), None, 200).unwrap();
        append(t.path(), &r).unwrap();
        r.t = 100;
        append(t.path(), &r).unwrap();
        let all = load(t.path());
        assert_eq!(all.iter().map(|r| r.t).collect::<Vec<_>>(), [100, 200]);
        assert_eq!(file_stem("u-1_o-1"), "u-1_o-1");
        assert!(file_stem("c:/users/x").starts_with("c--users-x-"));
    }
}
