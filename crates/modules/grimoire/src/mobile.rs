//! Grimoire sur le téléphone, en lecture : les profils de Claude Code (compte connecté), les limites relevées par son
//! relais (5 h et semaine par compte, comme Gringotts ; quand Gringotts tourne, une ligne y mène au lieu de les montrer
//! deux fois), les serveurs MCP du profil actif hors projet (gérés, utilisateur, plugins), dans un bandeau plié ; et
//! « Ouvrir sur le PC » sur un profil, ou l'onglet MCP (au menu de son bandeau, qui se voit plié), par le canal des
//! demandes de Grimoire (`instance::Request`).
//!
//! Tout se lit par la bibliothèque de Grimoire, sans toucher à l'application. `.claude.json` et la configuration
//! peuvent être longs à lire : relus dans un fil, au plus toutes les 10 s tant que le téléphone regarde.

use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use grimoire::instance::Request;
use grimoire::limits::Record;
use grimoire::mcp::{Server, Status};
use grimoire::profiles::{Account, Profile};
use suite_sdk::mobile::msg::MODULE_PREFIX;
use suite_sdk::mobile::ui::{Act, Band, Row, Screen, Tone};

const READ_EVERY: Duration = Duration::from_secs(10);

fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// Ce que le téléphone montre, lu dans un fil.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Data {
    pub profiles: Vec<(Profile, Option<Account>)>,
    /// Identifiant du profil actif.
    pub active: String,
    pub limits: Vec<Record>,
    /// Serveurs MCP du profil actif, hors projet.
    pub mcp: Vec<Server>,
}

/// Lit les profils (dans l'état de Grimoire, sans le réécrire), leurs comptes, les limites et les serveurs MCP.
pub fn read() -> Data {
    let state = grimoire::paths::read_json(&grimoire::state::data_dir().join("state.json")).ok().flatten();
    let mut profiles: Vec<Profile> = state
        .as_ref()
        .and_then(|s| s.get("profiles").cloned())
        .and_then(|p| serde_json::from_value(p).ok())
        .unwrap_or_default();
    if profiles.is_empty() {
        profiles.push(Profile::default_profile());
    }
    let active = state
        .as_ref()
        .and_then(|s| s.get("active_profile")?.as_str().map(String::from))
        .filter(|id| profiles.iter().any(|p| &p.id == id))
        .unwrap_or_else(|| profiles[0].id.clone());
    let paths = profiles.iter().find(|p| p.id == active).unwrap_or(&profiles[0]).paths();
    let mcp = grimoire::config::Config::load(paths, None).mcp;
    Data {
        profiles: profiles
            .into_iter()
            .map(|p| {
                let a = grimoire::profiles::account(&p.paths());
                (p, a)
            })
            .collect(),
        active,
        limits: grimoire::limits::load_all(&grimoire::state::local_data_dir()),
        mcp,
    }
}

fn ago(t: i64, now: i64) -> String {
    let s = (now - t).max(0);
    match s {
        0..60 => "à l'instant".into(),
        60..3600 => format!("il y a {} min", s / 60),
        3600..86_400 => format!("il y a {} h", s / 3600),
        _ => format!("il y a {} j", s / 86_400),
    }
}

fn pct_tone(pct: f64) -> Tone {
    if pct >= 100.0 {
        Tone::Danger
    } else if pct >= 80.0 {
        Tone::Warn
    } else {
        Tone::Ok
    }
}

fn record_label(r: &Record) -> String {
    let who = r.email.clone().unwrap_or_else(|| r.account.chars().take(8).collect());
    match &r.organization {
        Some(o) => format!("{who} · {o}"),
        None => who,
    }
}

/// Les limites d'un compte, comme dans Gringotts : un bandeau (en alerte au-delà de 80 %) et ses deux jauges.
fn limits(s: &mut Screen, r: &Record, now: i64) {
    let windows = [("5 h", &r.five_hour), ("Semaine", &r.seven_day)].map(|(label, w)| {
        let current = w.as_ref().filter(|w| w.resets_at > now);
        (label, current)
    });
    let worst = windows.iter().filter_map(|(_, w)| w.map(|w| w.used_percentage)).fold(0.0, f64::max);
    let mut band = Band::new(format!("compte/{}", r.account), record_label(r))
        .aside(format!("relevé {}", ago(r.recorded_at, now)))
        .glyph("📊");
    if worst >= 80.0 {
        band = band.badge(format!("{worst:.0} %")).tone(pct_tone(worst));
    }
    s.band(band);
    for (label, w) in windows {
        match w {
            Some(w) => {
                let fmt = if label == "5 h" { "%H:%M" } else { "%d/%m à %H:%M" };
                let reset = chrono::DateTime::from_timestamp(w.resets_at, 0)
                    .map(|t| t.with_timezone(&chrono::Local).format(fmt).to_string())
                    .unwrap_or_default();
                let text = format!("{:.0} % · remise à zéro {reset}", w.used_percentage);
                s.meter(label, (w.used_percentage / 100.0) as f32, text, pct_tone(w.used_percentage));
            }
            None => {
                s.meter(label, 0.0, "0 % · pas de fenêtre en cours", Tone::Ok);
            }
        }
    }
}

/// L'accueil ; `data` : `None` pendant la première lecture ; `gringotts` : Gringotts tourne (il montre les limites).
pub fn home(data: Option<&Data>, now: i64, gringotts: bool) -> Screen {
    let mut s = Screen::new("Grimoire");
    let Some(d) = data else {
        s.empty("Lecture de la configuration…");
        return s;
    };
    if let Some((p, _)) = d.profiles.iter().find(|(p, _)| p.id == d.active) {
        s.subtitle = format!("Profil actif : {}", p.name);
    }

    s.band(Band::new("profils", "Profils").aside(d.profiles.len().to_string()).glyph("👤"));
    for (p, account) in &d.profiles {
        let sub = account.as_ref().map(Account::label).unwrap_or_else(|| "non connecté".into());
        let mut row = Row::new(&p.name).key(format!("profil/{}", p.id)).subtitle(sub).glyph("👤");
        if p.id == d.active {
            row = row.badge("actif").tone(Tone::Ok);
        }
        row = row.act(Act::new(format!("ouvrir:{}", p.id), "Ouvrir sur le PC").glyph("🖥️"));
        s.row(row);
    }

    if gringotts {
        s.band(Band::new("limites", "Limites").glyph("📊"));
        s.row(
            Row::new("Limites des comptes")
                .subtitle("5 h et semaine, par compte, et ce que coûte chaque conversation")
                .glyph("📊")
                .tap(Act::new("gringotts", "Voir dans Gringotts").glyph("💰")),
        );
    } else if d.limits.is_empty() {
        s.band(Band::new("limites", "Limites").glyph("📊"));
        s.empty("Aucun relevé : le relais des limites s'installe depuis Grimoire, sur le PC.");
    }
    if !gringotts {
        for r in &d.limits {
            limits(&mut s, r, now);
        }
    }

    // L'onglet MCP du PC s'ouvre du menu du bandeau : à portée même plié.
    s.band(
        Band::new("mcp", "Serveurs MCP")
            .aside(d.mcp.len().to_string())
            .glyph("🔌")
            .open(false)
            .act(Act::new("mcp", "Ouvrir l'onglet MCP sur le PC").glyph("🖥️")),
    );
    if d.mcp.is_empty() {
        s.empty("Aucun serveur MCP hors projet.");
    }
    for m in &d.mcp {
        let mut sub = format!("{} · {}", m.scope, m.transport);
        if let Some(by) = &m.shadowed_by {
            sub = format!("{sub} · remplacé par celui de « {by} »");
        }
        let tone = match &m.status {
            Status::Active if m.shadowed_by.is_none() => Tone::Ok,
            Status::Active | Status::Pending => Tone::Warn,
            Status::Off(_) => Tone::Info,
        };
        s.row(
            Row::new(&m.name)
                .key(format!("mcp/{}/{}", m.scope, m.name))
                .subtitle(sub)
                .detail(m.status.label())
                .tone(tone)
                .glyph("🔌"),
        );
    }
    s
}

#[derive(Default)]
struct Cache {
    data: Option<Data>,
    read_at: Option<Instant>,
    reading: bool,
}

/// Ce que le téléphone garde entre deux appels : la dernière lecture, et le canal des demandes de Grimoire.
pub struct Phone {
    cache: Arc<Mutex<Cache>>,
    requests: Sender<Request>,
}

fn lock(c: &Mutex<Cache>) -> MutexGuard<'_, Cache> {
    c.lock().unwrap_or_else(|e| e.into_inner())
}

impl Phone {
    pub fn new(requests: Sender<Request>) -> Phone {
        Phone { cache: Arc::default(), requests }
    }

    /// `gringotts` : Gringotts tourne sur le PC (les limites y sont).
    pub fn screen(&mut self, path: &str, gringotts: bool) -> Option<Screen> {
        if !path.is_empty() {
            return None;
        }
        let mut c = lock(&self.cache);
        if !c.reading && c.read_at.is_none_or(|t| t.elapsed() >= READ_EVERY) {
            c.reading = true;
            let cache = self.cache.clone();
            std::thread::spawn(move || {
                let data = read();
                let mut c = lock(&cache);
                c.data = Some(data);
                c.read_at = Some(Instant::now());
                c.reading = false;
            });
        }
        Some(home(c.data.as_ref(), now(), gringotts))
    }

    pub fn action(&mut self, action: &str) -> Result<Option<String>, String> {
        if action == "gringotts" {
            return Ok(Some(format!("{MODULE_PREFIX}gringotts")));
        }
        let request = match action.split_once(':') {
            Some(("ouvrir", id)) => Request::Open { dir: None, tab: None, profile: Some(id.to_string()) },
            _ if action == "mcp" => {
                let active = lock(&self.cache).data.as_ref().map(|d| d.active.clone());
                Request::Open { dir: None, tab: Some("mcp".into()), profile: active }
            }
            _ => return Err("action inconnue".into()),
        };
        self.requests.send(request).map_err(|_| "Grimoire ne répond plus.".to_string())?;
        Ok(Some("Ouvert sur le PC".into()))
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::mpsc;

    use grimoire::limits::Window;
    use suite_sdk::mobile::ui::{Block, Fold};

    use super::*;

    /// Les bandeaux d'un écran : (clé, pliage, pastille, ton).
    fn bands(s: &Screen) -> Vec<(&str, Fold, String, Option<Tone>)> {
        s.blocks
            .iter()
            .filter_map(|b| match b {
                Block::Section { key, fold, badge, tone, .. } => Some((key.as_str(), *fold, badge.clone(), *tone)),
                _ => None,
            })
            .collect()
    }

    fn server(name: &str, scope: &str, status: Status, shadowed: Option<&str>) -> Server {
        Server {
            name: name.into(),
            scope: scope.into(),
            path: PathBuf::from(r"C:\Users\ada\.claude.json"),
            transport: "stdio".into(),
            target: "npx serveur".into(),
            raw_target: "npx serveur".into(),
            env: Vec::new(),
            headers: Vec::new(),
            status,
            shadowed_by: shadowed.map(String::from),
        }
    }

    fn sample(now: i64) -> Data {
        let work = Profile {
            id: "equipe".into(),
            name: "Équipe".into(),
            config_dir: Some(r"C:\Users\ada\.claude-equipe".into()),
        };
        Data {
            profiles: vec![
                (
                    Profile::default_profile(),
                    Some(Account {
                        email: Some("ada@contoso.example".into()),
                        plan: Some("Max 5x".into()),
                        ..Default::default()
                    }),
                ),
                (work, None),
            ],
            active: "equipe".into(),
            limits: vec![Record {
                account: "u-1".into(),
                email: Some("ada@contoso.example".into()),
                organization: Some("PDLC".into()),
                organization_uuid: None,
                config_dir: None,
                recorded_at: now - 120,
                five_hour: Some(Window { used_percentage: 85.0, resets_at: now + 3600 }),
                seven_day: Some(Window { used_percentage: 40.0, resets_at: now - 10 }),
            }],
            mcp: vec![
                server("github", "utilisateur", Status::Active, None),
                server("docs", "plugin outils", Status::Off("désactivé".into()), None),
            ],
        }
    }

    #[test]
    fn the_home_shows_profiles_limits_and_mcp_servers() {
        let now = 1_800_000_000;
        let s = home(Some(&sample(now)), now, false);
        assert_eq!(s.subtitle, "Profil actif : Équipe");
        let meters: Vec<(&str, f32, Tone)> = s
            .blocks
            .iter()
            .filter_map(|b| match b {
                Block::Meter { label, value, tone, .. } => Some((label.as_str(), *value, *tone)),
                _ => None,
            })
            .collect();
        assert_eq!(meters, [("5 h", 0.85, Tone::Warn), ("Semaine", 0.0, Tone::Ok)], "semaine remise à zéro");
        let rows: Vec<&Row> = s
            .blocks
            .iter()
            .filter_map(|b| match b {
                Block::Row(r) => Some(r),
                _ => None,
            })
            .collect();
        assert_eq!(rows[0].subtitle, "ada@contoso.example · Max 5x");
        assert_eq!((rows[1].subtitle.as_str(), rows[1].badge.as_str()), ("non connecté", "actif"));
        assert_eq!((rows[2].title.as_str(), rows[2].tone), ("github", Some(Tone::Ok)));
        assert_eq!(rows[3].detail, "désactivé");
        // Un bandeau par compte, comme Gringotts, en alerte au-delà de 80 % ; les serveurs MCP pliés.
        assert_eq!(
            bands(&s),
            [
                ("profils", Fold::Open, String::new(), None),
                ("compte/u-1", Fold::Open, "85 %".into(), Some(Tone::Warn)),
                ("mcp", Fold::Closed, String::new(), None)
            ]
        );
        let ids: Vec<String> = s.action_ids().into_iter().collect();
        assert_eq!(ids, ["mcp", "ouvrir:defaut", "ouvrir:equipe"]);
        // « Ouvrir l'onglet MCP » est au menu du bandeau MCP, plié : plus de bouton caché sous ses serveurs.
        let mcp = s.blocks.iter().find_map(|b| match b {
            Block::Section { key, acts, .. } if key == "mcp" => Some(acts),
            _ => None,
        });
        assert_eq!(mcp.map(|a| a.iter().map(|a| a.id.as_str()).collect::<Vec<_>>()), Some(vec!["mcp"]));
        assert!(!s.blocks.iter().any(|b| matches!(b, Block::Buttons { .. })));

        let loading = home(None, now, false);
        assert!(loading.action_ids().is_empty());
    }

    /// Quand Gringotts tourne, les limites n'y sont pas montrées deux fois : une ligne y mène.
    #[test]
    fn with_gringotts_running_the_limits_lead_there() {
        let now = 1_800_000_000;
        let s = home(Some(&sample(now)), now, true);
        assert!(!s.blocks.iter().any(|b| matches!(b, Block::Meter { .. })));
        assert!(s.action_ids().contains("gringotts"));
        let (tx, _rx) = mpsc::channel();
        let mut phone = Phone::new(tx);
        assert_eq!(phone.action("gringotts"), Ok(Some(format!("{MODULE_PREFIX}gringotts"))));
    }

    #[test]
    fn open_on_the_pc_goes_through_grimoire_requests() {
        let (tx, rx) = mpsc::channel();
        let mut phone = Phone::new(tx);
        assert!(phone.screen("autre", false).is_none());
        assert_eq!(phone.action("ouvrir:equipe"), Ok(Some("Ouvert sur le PC".into())));
        assert_eq!(rx.try_recv().ok(), Some(Request::Open { dir: None, tab: None, profile: Some("equipe".into()) }));
        assert!(phone.action("mcp").is_ok());
        assert!(matches!(rx.try_recv(), Ok(Request::Open { tab: Some(t), .. }) if t == "mcp"));
        assert!(phone.action("autre").is_err());
    }
}
