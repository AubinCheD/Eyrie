//! Le relais : la ligne d'état de Claude Code, qui relève les limites et montre la part de la conversation.
//!
//! C'est `eyrie.exe --ligne-d-etat`, réglé comme `statusLine` d'un profil de Claude Code : toujours là où Eyrie est,
//! sans exécutable à part. Claude Code lui passe un objet JSON sur son entrée ; il range le relevé (`readings`), puis
//! affiche la ligne d'état d'avant l'installation (rejouée avec la même entrée) suivie de la sienne. Il ne doit
//! jamais faire échouer la ligne d'état : toute erreur est avalée.
//!
//! L'installation écrit la clé `statusLine` des réglages utilisateur du profil (copie avant écriture, par Grimoire)
//! et garde l'ancienne valeur dans `relais.json` : le relais la rejoue, le retrait la remet. Une ancienne ligne
//! d'état qui était le relais de Grimoire est remplacée par ce que celui-ci rejouait.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use grimoire::paths::ClaudePaths;
use serde_json::{Map, Value};

use crate::readings::{self, Parts};

/// L'argument qui fait d'`eyrie.exe` le relais.
pub const ARG: &str = "--ligne-d-etat";

/// Posée pour la commande rejouée : un relais lancé par un relais ne rejoue rien.
const NESTED_ENV: &str = "GRINGOTTS_RELAIS";
const MAX_INPUT: u64 = 1024 * 1024;
const MAX_OUTPUT: u64 = 1024 * 1024;
const CHAIN_TIMEOUT: Duration = Duration::from_secs(5);

/// Dossier des données de Gringotts : celui de la suite (`EyrieSuite`, ou `EYRIESUITE_DIR`), le même pour Eyrie et
/// pour le relais lancé par Claude Code.
pub fn data_dir() -> PathBuf {
    suite_accounts::suite_dir().join("gringotts")
}

fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// `eyrie.exe --ligne-d-etat` : lit l'entrée, travaille, écrit la ligne. Rend toujours 0.
pub fn main() -> i32 {
    let mut input = Vec::new();
    let _ = std::io::stdin().take(MAX_INPUT).read_to_end(&mut input);
    let config = std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from);
    let line = run(&input, &data_dir(), config.as_deref(), now());
    let mut out = std::io::stdout();
    let _ = out.write_all(line.as_bytes());
    let _ = out.flush();
    0
}

/// Tout le travail du relais, sans entrée ni sortie standard (essais).
pub fn run(stdin: &[u8], dir: &Path, env_config_dir: Option<&Path>, now: i64) -> String {
    let bytes = stdin.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(stdin);
    let input: Value = serde_json::from_slice(bytes).unwrap_or(Value::Null);
    let p = ClaudePaths::for_config_dir(env_config_dir.map(Path::to_path_buf));
    let mine = record(&input, &p, dir, now);
    let nested = std::env::var_os(NESTED_ENV).is_some();
    let before =
        previous_command(dir, &p.config_dir).filter(|_| !nested).and_then(|c| replay(&c, stdin, CHAIN_TIMEOUT));
    match (before, mine) {
        (Some(b), Some(m)) => format!("{b} │ {m}"),
        (Some(b), None) => b,
        (None, Some(m)) => m,
        (None, None) => grimoire::limits::short_line(&input),
    }
}

/// Range le relevé et rend la ligne de Gringotts (`None` sans limites dans l'entrée).
fn record(input: &Value, p: &ClaudePaths, dir: &Path, now: i64) -> Option<String> {
    input.get("rate_limits")?;
    let claude_json = read_account(&p.claude_json).ok().flatten();
    let r = readings::from_status(input, claude_json.as_ref(), Some(&p.config_dir), now)?;
    let key = r.key();
    let mut state = readings::load_state(dir, &key);
    if readings::accumulate(&mut state, &r) {
        let _ = readings::append(dir, &r);
    }
    let _ = readings::save_state(dir, &key, &state);
    Some(readings::line(&r, &state, &Parts::load(dir), now))
}

/// `oauthAccount` de `.claude.json`, seul (le fichier peut être gros).
fn read_account(path: &Path) -> Result<Option<Value>, String> {
    #[derive(serde::Deserialize)]
    struct OnlyAccount {
        #[serde(rename = "oauthAccount")]
        oauth_account: Option<Value>,
    }
    let raw = match std::fs::read(path) {
        Ok(r) => r,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.to_string()),
    };
    let bytes = raw.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&raw);
    let parsed: OnlyAccount = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    Ok(Some(serde_json::json!({ "oauthAccount": parsed.oauth_account })))
}

fn chain_file(dir: &Path) -> PathBuf {
    dir.join("relais.json")
}

fn read_chain(dir: &Path) -> Map<String, Value> {
    std::fs::read(chain_file(dir))
        .ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default()
}

fn write_chain(dir: &Path, chain: &Map<String, Value>) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let text = serde_json::to_vec_pretty(&Value::Object(chain.clone())).map_err(|e| e.to_string())?;
    grimoire::state::write_atomic(&chain_file(dir), &text).map_err(|e| e.to_string())
}

fn previous_command(dir: &Path, config_dir: &Path) -> Option<String> {
    let wanted = grimoire::paths::key(config_dir);
    let chain = read_chain(dir);
    let (_, previous) = chain.iter().find(|(k, _)| grimoire::paths::key(k) == wanted)?;
    let cmd = previous.get("command")?.as_str()?.trim();
    (!cmd.is_empty()).then(|| cmd.to_string())
}

/// Rejoue `cmd` par l'interpréteur de Claude Code, `input` sur son entrée : sa sortie, sans la fin de ligne finale.
fn replay(cmd: &str, input: &[u8], timeout: Duration) -> Option<String> {
    let deadline = Instant::now() + timeout;
    let mut c = grimoire::shell::command(&grimoire::shell::claude_shell(), cmd);
    c.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).env(NESTED_ENV, "1");
    grimoire::shell::hide_window(&mut c);
    let mut child = c.spawn().ok()?;
    if let Some(mut stdin) = child.stdin.take() {
        let input = input.to_vec();
        std::thread::spawn(move || {
            let _ = stdin.write_all(&input);
        });
    }
    let (tx, rx) = mpsc::channel();
    if let Some(stdout) = child.stdout.take() {
        std::thread::spawn(move || {
            let mut out = Vec::new();
            let _ = stdout.take(MAX_OUTPUT).read_to_end(&mut out);
            let _ = tx.send(out);
        });
    }
    let out = rx.recv_timeout(deadline.saturating_duration_since(Instant::now()));
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            _ => break None,
        }
    };
    let Some(status) = status else {
        grimoire::shell::kill_tree(&mut child);
        let _ = child.wait();
        return None;
    };
    let out = out.ok().filter(|_| status.success())?;
    let text = String::from_utf8_lossy(&out);
    let text = text.trim_end_matches(['\r', '\n']);
    (!text.trim().is_empty()).then(|| text.to_string())
}

/// Une valeur de `statusLine` lance-t-elle ce relais ?
pub fn is_relay(v: &Value) -> bool {
    v.get("command").and_then(Value::as_str).is_some_and(|c| c.contains(ARG))
}

/// Le relais est-il la ligne d'état des réglages utilisateur de ce profil ?
pub fn installed(p: &ClaudePaths) -> bool {
    grimoire::paths::read_json(&p.user_settings())
        .ok()
        .flatten()
        .and_then(|s| s.get("statusLine").cloned())
        .is_some_and(|v| is_relay(&v))
}

/// La commande à régler : l'exécutable `exe` (Eyrie), cité pour l'interpréteur de Claude Code, et l'argument.
pub fn command(exe: &Path) -> String {
    format!("{} {ARG}", grimoire::relay::command_for(exe, &grimoire::shell::claude_shell()))
}

/// Installe le relais (`command`) comme ligne d'état du profil de `p`. `grimoire_data` : le dossier de Grimoire, pour
/// reprendre ce que son relais rejouait. Rend le chemin de la copie de sauvegarde, s'il y en a une.
pub fn install(p: &ClaudePaths, command: &str, dir: &Path, grimoire_data: &Path) -> Result<Option<PathBuf>, String> {
    let file = p.user_settings();
    let mut settings = grimoire::paths::read_json(&file)?.unwrap_or_else(|| Value::Object(Map::new()));
    if !settings.is_object() {
        return Err(format!("{} ne contient pas un objet JSON", file.display()));
    }
    let fp = grimoire::write::fingerprint(&file);
    let before = settings.get("statusLine").cloned();
    if before.as_ref().is_some_and(is_relay) {
        return Ok(None);
    }
    // Le relais de Grimoire : on reprend la ligne d'état qu'il rejouait (le sien n'apporte plus rien).
    let previous = match &before {
        Some(b) if grimoire::relay::is_relay(b) => grimoire::relay::previous(p, grimoire_data),
        other => other.clone(),
    };
    let mut chain = read_chain(dir);
    let key = grimoire::paths::key(&p.config_dir);
    chain.retain(|k, _| grimoire::paths::key(k) != key);
    if let Some(prev) = previous {
        chain.insert(key, prev);
    }
    write_chain(dir, &chain)?;
    let mut after = Map::new();
    after.insert("type".into(), Value::String("command".into()));
    after.insert("command".into(), Value::String(command.into()));
    for k in ["padding", "refreshInterval"] {
        if let Some(v) = before.as_ref().and_then(|b| b.get(k)) {
            after.insert(k.into(), v.clone());
        }
    }
    settings.as_object_mut().expect("objet vérifié").insert("statusLine".into(), Value::Object(after));
    let saved = grimoire::write::save_json_in(p, &file, &settings, fp.as_ref(), &dir.join("copies"))?;
    Ok(saved.backup)
}

/// Retire le relais : remet la ligne d'état d'avant, ou retire la clé s'il n'y en avait pas.
pub fn uninstall(p: &ClaudePaths, dir: &Path) -> Result<(), String> {
    let file = p.user_settings();
    let Some(mut settings) = grimoire::paths::read_json(&file)? else { return Ok(()) };
    if !settings.get("statusLine").is_some_and(is_relay) {
        return Ok(());
    }
    let fp = grimoire::write::fingerprint(&file);
    let key = grimoire::paths::key(&p.config_dir);
    let mut chain = read_chain(dir);
    let previous = chain.iter().find(|(k, _)| grimoire::paths::key(k) == key).map(|(_, v)| v.clone());
    let obj = settings.as_object_mut().ok_or("réglages illisibles")?;
    match previous {
        Some(v) => {
            obj.insert("statusLine".into(), v);
        }
        None => {
            obj.remove("statusLine");
        }
    }
    grimoire::write::save_json_in(p, &file, &settings, fp.as_ref(), &dir.join("copies"))?;
    chain.retain(|k, _| grimoire::paths::key(k) != key);
    write_chain(dir, &chain)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs;

    fn setup(settings: Option<Value>) -> (tempfile::TempDir, ClaudePaths, PathBuf) {
        let t = tempfile::tempdir().unwrap();
        let p = ClaudePaths::under(t.path());
        fs::create_dir_all(&p.config_dir).unwrap();
        if let Some(s) = settings {
            fs::write(p.user_settings(), s.to_string()).unwrap();
        }
        // Le relais lit `.claude.json` du dossier de configuration de la session (`CLAUDE_CONFIG_DIR`).
        let account = json!({"oauthAccount": {"accountUuid": "u-1", "emailAddress": "ada@contoso.example"}});
        fs::write(p.config_dir.join(".claude.json"), account.to_string()).unwrap();
        let dir = t.path().join("gringotts");
        (t, p, dir)
    }

    fn settings(p: &ClaudePaths) -> Value {
        serde_json::from_slice(&fs::read(p.user_settings()).unwrap()).unwrap()
    }

    #[test]
    fn install_keeps_the_previous_line_and_uninstall_puts_it_back() {
        let before = json!({"type": "command", "command": "echo avant", "padding": 1});
        let (t, p, dir) = setup(Some(json!({"model": "opus", "statusLine": before})));
        install(&p, "C:/outils/eyrie.exe --ligne-d-etat", &dir, &t.path().join("grimoire")).unwrap();
        let s = settings(&p);
        assert_eq!(s["statusLine"]["command"], "C:/outils/eyrie.exe --ligne-d-etat");
        assert_eq!(s["statusLine"]["padding"], 1, "gardé");
        assert_eq!(s["model"], "opus", "le reste ne bouge pas");
        assert!(installed(&p));
        assert_eq!(previous_command(&dir, &p.config_dir).as_deref(), Some("echo avant"));
        uninstall(&p, &dir).unwrap();
        assert_eq!(settings(&p)["statusLine"], before);
        assert!(!installed(&p));
    }

    #[test]
    fn without_a_previous_line_uninstall_removes_the_key() {
        let (t, p, dir) = setup(None);
        install(&p, "eyrie.exe --ligne-d-etat", &dir, &t.path().join("grimoire")).unwrap();
        uninstall(&p, &dir).unwrap();
        assert!(settings(&p).get("statusLine").is_none());
    }

    #[test]
    fn the_relay_records_and_shows_the_share_of_the_conversation() {
        let (_t, p, dir) = setup(None);
        let input = |s: &str, five: f64| {
            json!({"session_id": s, "rate_limits": {"five_hour": {"used_percentage": five, "resets_at": 2_000_000_000i64}}})
                .to_string()
        };
        let a = run(input("a", 10.0).as_bytes(), &dir, Some(&p.config_dir), 100);
        assert!(a.starts_with("5 h 10 % (conv. 10 %)"), "{a}");
        let b = run(input("b", 12.0).as_bytes(), &dir, Some(&p.config_dir), 110);
        assert!(b.starts_with("5 h 12 % (conv. 2 %)"), "{b}");
        let all = readings::load(&dir);
        assert_eq!(all.len(), 2);
        assert_eq!(all[1].email.as_deref(), Some("ada@contoso.example"));
        // Sans limites (clé d'API) : la ligne courte, rien de rangé.
        let api = run(br#"{"model":{"display_name":"Opus"}}"#, &dir, Some(&p.config_dir), 120);
        assert_eq!(api, "Opus");
        assert_eq!(readings::load(&dir).len(), 2);
    }
}
