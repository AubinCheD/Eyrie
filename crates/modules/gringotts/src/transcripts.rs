//! Les conversations de Claude Code sur le disque, pour ce qu'elles ont consommé.
//!
//! Claude Code écrit une transcription par session : `<config>/projects/<dossier encodé>/<id>.jsonl`, une entrée JSON
//! par ligne, et celles de ses sous-agents sous `<id>/subagents/`. On n'en lit que l'heure, le modèle et les jetons
//! de chaque réponse (`message.usage`), le dossier, la branche et le titre ; le reste est ignoré. Claude Code efface
//! les transcriptions au bout de `cleanupPeriodDays` (30 jours par défaut) : le résumé de chacune est gardé dans
//! `conversations.json`, et une conversation dont le fichier a disparu reste comptée, marquée effacée.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Une réponse de l'API.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Reply {
    /// `message.id` : une session reprise recopie des réponses déjà comptées.
    pub id: String,
    /// Secondes Unix (0 : horodatage illisible).
    pub at: i64,
    pub model: String,
    pub input: u64,
    pub output: u64,
    pub write_5m: u64,
    pub write_1h: u64,
    pub read: u64,
}

impl Reply {
    pub fn tokens(&self) -> u64 {
        self.input + self.output + self.write_5m + self.write_1h + self.read
    }

    /// Coût équivalent API, en dollars (0 pour un modèle inconnu).
    pub fn cost(&self) -> f64 {
        let prompt = self.input + self.write_5m + self.write_1h + self.read;
        let Some(p) = crate::prices::price(&self.model, prompt) else { return 0.0 };
        (self.input as f64 * p.input
            + self.write_5m as f64 * p.write_5m
            + self.write_1h as f64 * p.write_1h
            + self.read as f64 * p.read
            + self.output as f64 * p.output)
            / 1_000_000.0
    }
}

/// Une conversation (session principale, ses sous-agents compris).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Conversation {
    pub id: String,
    /// Dossier de configuration de Claude Code où elle est rangée (clé de `paths::key`).
    pub config: String,
    /// Dossier de travail (dépôt).
    pub cwd: String,
    pub title: String,
    pub branch: Option<String>,
    pub started: i64,
    pub ended: i64,
    /// Demandes tapées par l'utilisateur.
    pub prompts: u32,
    pub subagents: u32,
    pub replies: Vec<Reply>,
    /// La transcription a été effacée par Claude Code : seul son résumé reste.
    pub gone: bool,
}

impl Conversation {
    pub fn cost(&self) -> f64 {
        self.replies.iter().map(Reply::cost).sum()
    }

    pub fn tokens(&self) -> u64 {
        self.replies.iter().map(Reply::tokens).sum()
    }

    /// Modèles utilisés, du plus coûteux au moins coûteux.
    pub fn models(&self) -> Vec<String> {
        let mut by: BTreeMap<String, f64> = BTreeMap::new();
        for r in &self.replies {
            *by.entry(crate::prices::short_name(&r.model)).or_default() += r.cost();
        }
        let mut v: Vec<(String, f64)> = by.into_iter().filter(|(m, _)| !m.starts_with('<')).collect();
        v.sort_by(|a, b| b.1.total_cmp(&a.1));
        v.into_iter().map(|(m, _)| m).collect()
    }

    /// Nom du dépôt : le dernier dossier du chemin de travail.
    pub fn repo(&self) -> String {
        let t = self.cwd.trim_end_matches(['/', '\\']);
        t.rsplit(['/', '\\']).next().filter(|s| !s.is_empty()).unwrap_or("(sans dossier)").to_string()
    }
}

/// Ce qu'un fichier de transcription apporte.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FileSummary {
    /// Session principale (le nom du fichier, ou celui de la session d'un sous-agent).
    pub session: String,
    pub subagent: bool,
    pub cwd: String,
    pub custom_title: Option<String>,
    pub ai_title: Option<String>,
    pub first_prompt: Option<String>,
    pub branch: Option<String>,
    pub started: i64,
    pub ended: i64,
    pub prompts: u32,
    pub replies: Vec<Reply>,
}

/// Messages techniques que Claude Code écrit comme des demandes de l'utilisateur.
const TECHNICAL: &[&str] = &[
    "<command-name>",
    "<command-message>",
    "<command-args>",
    "<local-command-stdout>",
    "<local-command-stderr>",
    "<local-command-caveat>",
    "<system-reminder>",
    "<bash-input>",
    "<bash-stdout>",
    "<bash-stderr>",
    "<task-notification>",
    "<user-prompt-submit-hook>",
    "<cross-session-message",
    "<agent-message",
];

pub fn timestamp(ts: &str) -> i64 {
    chrono::DateTime::parse_from_rfc3339(ts).map(|d| d.timestamp()).unwrap_or(0)
}

fn one_line(t: &str, max: usize) -> String {
    let t = t.split_whitespace().collect::<Vec<_>>().join(" ");
    if t.chars().count() <= max { t } else { format!("{}…", t.chars().take(max).collect::<String>()) }
}

/// Texte tapé par l'utilisateur ; ni retour d'outil, ni message technique.
fn user_text(content: &Value) -> Option<String> {
    let text = match content {
        Value::String(s) => s.clone(),
        Value::Array(a) => {
            if a.iter().any(|b| b.get("type").and_then(Value::as_str) == Some("tool_result")) {
                return None;
            }
            a.iter()
                .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
                .filter_map(|b| b.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("\n")
        }
        _ => return None,
    };
    let t = text.trim();
    (!t.is_empty() && !TECHNICAL.iter().any(|tag| t.starts_with(tag))).then(|| t.to_string())
}

/// Lit une transcription ; `session` : la session principale, `subagent` : c'est le fichier d'un sous-agent.
pub fn parse_lines(session: &str, subagent: bool, lines: impl Iterator<Item = String>) -> FileSummary {
    let mut s = FileSummary { session: session.to_string(), subagent, ..Default::default() };
    // Une réponse découpée en plusieurs entrées répète son `message.id` : on garde la plus complète.
    let mut by_id: HashMap<String, usize> = HashMap::new();
    for line in lines {
        let interesting =
            ["\"type\":\"user\"", "\"type\":\"assistant\"", "aiTitle", "customTitle"].iter().any(|k| line.contains(k));
        if !interesting {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
        if let Some(t) = v.get("customTitle").and_then(Value::as_str) {
            s.custom_title = Some(t.to_string());
        }
        if let Some(t) = v.get("aiTitle").and_then(Value::as_str) {
            s.ai_title = Some(t.to_string());
        }
        let kind = v.get("type").and_then(Value::as_str).unwrap_or("");
        if kind != "user" && kind != "assistant" {
            continue;
        }
        let at = v.get("timestamp").and_then(Value::as_str).map(timestamp).unwrap_or(0);
        if at > 0 {
            if s.started == 0 || at < s.started {
                s.started = at;
            }
            s.ended = s.ended.max(at);
        }
        if s.cwd.is_empty()
            && let Some(c) = v.get("cwd").and_then(Value::as_str)
        {
            s.cwd = c.to_string();
        }
        if let Some(b) = v.get("gitBranch").and_then(Value::as_str).filter(|b| !b.is_empty()) {
            s.branch = Some(b.to_string());
        }
        let flag = |k: &str| v.get(k).and_then(Value::as_bool) == Some(true);
        let msg = v.get("message").unwrap_or(&Value::Null);
        if kind == "user" {
            if subagent || flag("isMeta") || flag("isCompactSummary") || flag("isSidechain") {
                continue;
            }
            if let Some(text) = user_text(msg.get("content").unwrap_or(&Value::Null)) {
                s.prompts += 1;
                if s.first_prompt.is_none() {
                    s.first_prompt = Some(one_line(&text, 100));
                }
            }
            continue;
        }
        let Some(u) = msg.get("usage") else { continue };
        let n = |k: &str| u.get(k).and_then(Value::as_u64).unwrap_or(0);
        let creation = n("cache_creation_input_tokens");
        // Les deux durées de cache, quand Claude Code les donne ; sinon tout en 5 min.
        let (w1h, w5m) = match u.get("cache_creation") {
            Some(c) => {
                let one = c.get("ephemeral_1h_input_tokens").and_then(Value::as_u64).unwrap_or(0);
                let five =
                    c.get("ephemeral_5m_input_tokens").and_then(Value::as_u64).unwrap_or(creation.saturating_sub(one));
                (one, five)
            }
            None => (0, creation),
        };
        let reply = Reply {
            id: msg.get("id").and_then(Value::as_str).unwrap_or("").to_string(),
            at,
            model: msg.get("model").and_then(Value::as_str).unwrap_or("?").to_string(),
            input: n("input_tokens"),
            output: n("output_tokens"),
            write_5m: w5m,
            write_1h: w1h,
            read: n("cache_read_input_tokens"),
        };
        match (reply.id.is_empty(), by_id.get(&reply.id)) {
            (false, Some(&i)) => {
                if reply.output > s.replies[i].output {
                    s.replies[i] = reply;
                }
            }
            _ => {
                if !reply.id.is_empty() {
                    by_id.insert(reply.id.clone(), s.replies.len());
                }
                s.replies.push(reply);
            }
        }
    }
    s
}

pub fn parse_file(path: &Path, session: &str, subagent: bool) -> std::io::Result<FileSummary> {
    let f = fs::File::open(path)?;
    let lines = BufReader::new(f).split(b'\n').map_while(Result::ok).map(|b| String::from_utf8_lossy(&b).into_owned());
    Ok(parse_lines(session, subagent, lines))
}

/// Fichiers de transcription d'un dossier de configuration : (chemin, session principale, sous-agent).
pub fn files(config_dir: &Path) -> Vec<(PathBuf, String, bool)> {
    let mut out = Vec::new();
    let Ok(projects) = fs::read_dir(config_dir.join("projects")) else { return out };
    for project in projects.flatten().map(|e| e.path()).filter(|p| p.is_dir()) {
        let Ok(entries) = fs::read_dir(&project) else { continue };
        for e in entries.flatten().map(|e| e.path()) {
            let stem = e.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            if e.is_file() && e.extension().is_some_and(|x| x.eq_ignore_ascii_case("jsonl")) {
                out.push((e, stem, false));
            } else if e.is_dir()
                && let Ok(subs) = fs::read_dir(e.join("subagents"))
            {
                let session = e.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                for sub in subs.flatten().map(|s| s.path()) {
                    if sub.extension().is_some_and(|x| x.eq_ignore_ascii_case("jsonl")) {
                        out.push((sub, session.clone(), true));
                    }
                }
            }
        }
    }
    out
}

/// Le cache des résumés : par fichier, sa taille et sa date, et ce qu'il apportait.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Cache {
    pub version: u32,
    /// Chemin → (dossier de configuration, taille, date, résumé).
    pub files: BTreeMap<String, CachedFile>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CachedFile {
    pub config: String,
    pub len: u64,
    pub modified: u64,
    pub summary: FileSummary,
    /// Disparu du disque.
    pub gone: bool,
}

pub const CACHE_VERSION: u32 = 1;

impl Cache {
    pub fn load(path: &Path) -> Cache {
        let c: Cache = fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        if c.version == CACHE_VERSION { c } else { Cache { version: CACHE_VERSION, ..Default::default() } }
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let bytes = serde_json::to_vec(self).map_err(std::io::Error::other)?;
        grimoire::state::write_atomic(path, &bytes)
    }

    /// Relit les fichiers nouveaux ou changés des dossiers de configuration `configs` ; un fichier disparu est gardé,
    /// marqué. Rend le nombre de fichiers relus.
    pub fn refresh(&mut self, configs: &[PathBuf]) -> usize {
        self.version = CACHE_VERSION;
        let mut seen = HashSet::new();
        let mut read = 0;
        for dir in configs {
            let config = grimoire::paths::key(dir);
            for (path, session, subagent) in files(dir) {
                let key = path.display().to_string();
                seen.insert(key.clone());
                let Ok(meta) = fs::metadata(&path) else { continue };
                let modified =
                    meta.modified().ok().and_then(|m| m.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs());
                if let Some(c) = self.files.get(&key)
                    && c.len == meta.len()
                    && c.modified == modified
                    && !c.gone
                {
                    continue;
                }
                let Ok(summary) = parse_file(&path, &session, subagent) else { continue };
                read += 1;
                self.files.insert(
                    key,
                    CachedFile { config: config.clone(), len: meta.len(), modified, summary, gone: false },
                );
            }
        }
        // Les dossiers lus ce coup-ci : un fichier qui y manque a été effacé (un dossier non lu ne dit rien).
        let read_configs: HashSet<String> = configs.iter().map(grimoire::paths::key).collect();
        for (key, c) in self.files.iter_mut() {
            if read_configs.contains(&c.config) && !seen.contains(key) {
                c.gone = true;
            }
        }
        read
    }

    /// Les conversations : sessions principales et leurs sous-agents réunis, une réponse comptée une seule fois.
    pub fn conversations(&self) -> Vec<Conversation> {
        /// Titre choisi par l'utilisateur, titre donné par Claude, première demande.
        type Titles = (Option<String>, Option<String>, Option<String>);
        let mut by: BTreeMap<(String, String), Conversation> = BTreeMap::new();
        let mut titles: HashMap<(String, String), Titles> = HashMap::new();
        for c in self.files.values() {
            let s = &c.summary;
            let k = (c.config.clone(), s.session.clone());
            let conv = by.entry(k.clone()).or_insert_with(|| Conversation {
                id: s.session.clone(),
                config: c.config.clone(),
                gone: true,
                ..Default::default()
            });
            conv.gone &= c.gone || s.subagent;
            if s.subagent {
                conv.subagents += 1;
            } else {
                conv.prompts += s.prompts;
                let t = titles.entry(k).or_default();
                t.0 = t.0.take().or(s.custom_title.clone());
                t.1 = t.1.take().or(s.ai_title.clone());
                t.2 = t.2.take().or(s.first_prompt.clone());
                if conv.branch.is_none() {
                    conv.branch = s.branch.clone();
                }
            }
            if conv.cwd.is_empty() {
                conv.cwd = s.cwd.clone();
            }
            if s.started > 0 && (conv.started == 0 || s.started < conv.started) {
                conv.started = s.started;
            }
            conv.ended = conv.ended.max(s.ended);
            conv.replies.extend(s.replies.iter().cloned());
        }
        let mut out: Vec<Conversation> = by
            .into_iter()
            .map(|(k, mut c)| {
                let t = titles.remove(&k).unwrap_or_default();
                c.title = t.0.or(t.1).or(t.2).map(|t| one_line(&t, 100)).unwrap_or_else(|| "(sans titre)".into());
                // Une conversation faite seulement de sous-agents (fichier principal effacé) reste effacée.
                c
            })
            .collect();
        // Une réponse présente dans deux conversations (session reprise) n'est gardée que dans la plus ancienne.
        out.sort_by_key(|c| c.started);
        let mut ids = HashSet::new();
        for c in &mut out {
            c.replies.retain(|r| r.id.is_empty() || ids.insert(r.id.clone()));
            c.replies.sort_by_key(|r| r.at);
        }
        out.retain(|c| !c.replies.is_empty() || c.prompts > 0);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAIN: &str = r#"{"type":"summary","summary":"x"}
{"type":"user","message":{"role":"user","content":"Corrige l'export CSV"},"timestamp":"2026-10-20T09:00:00Z","cwd":"C:\\dev\\ozmo","gitBranch":"main","sessionId":"s1"}
{"type":"assistant","message":{"id":"m1","model":"claude-opus-5-5","usage":{"input_tokens":10,"output_tokens":3,"cache_creation_input_tokens":100,"cache_read_input_tokens":1000}},"timestamp":"2026-10-20T09:00:05Z"}
{"type":"assistant","message":{"id":"m1","model":"claude-opus-5-5","usage":{"input_tokens":10,"output_tokens":50,"cache_creation_input_tokens":100,"cache_creation":{"ephemeral_5m_input_tokens":40,"ephemeral_1h_input_tokens":60},"cache_read_input_tokens":1000}},"timestamp":"2026-10-20T09:00:06Z"}
{"type":"user","message":{"role":"user","content":[{"type":"tool_result","content":"ok"}]},"timestamp":"2026-10-20T09:00:07Z"}
{"type":"user","message":{"role":"user","content":"<system-reminder>rien</system-reminder>"},"timestamp":"2026-10-20T09:00:08Z"}
{"type":"assistant","message":{"id":"m2","model":"claude-sonnet-5-5","usage":{"input_tokens":5,"output_tokens":5}},"timestamp":"2026-10-20T09:01:00Z"}
{"aiTitle":"Export CSV corrigé"}
"#;

    const SUB: &str = r#"{"type":"user","isSidechain":true,"message":{"role":"user","content":"Consigne"},"timestamp":"2026-10-20T09:00:30Z"}
{"type":"assistant","message":{"id":"a1","model":"claude-haiku-5-5","usage":{"input_tokens":1000,"output_tokens":100}},"timestamp":"2026-10-20T09:00:40Z"}
"#;

    fn lines(t: &str) -> impl Iterator<Item = String> + '_ {
        t.lines().map(String::from)
    }

    #[test]
    fn a_transcript_gives_its_replies_title_and_prompts() {
        let s = parse_lines("s1", false, lines(MAIN));
        assert_eq!(s.replies.len(), 2, "une réponse découpée n'est comptée qu'une fois");
        let m1 = &s.replies[0];
        assert_eq!((m1.output, m1.write_5m, m1.write_1h, m1.read), (50, 40, 60, 1000), "la plus complète");
        assert_eq!(s.prompts, 1, "ni retour d'outil ni message technique");
        assert_eq!(s.ai_title.as_deref(), Some("Export CSV corrigé"));
        assert_eq!(s.cwd, r"C:\dev\ozmo");
        assert_eq!(s.started, timestamp("2026-10-20T09:00:00Z"));
        // 10 × 4 + 40 × 5 + 60 × 8 + 1000 × 0,2 + 50 × 20, par million.
        let cost = m1.cost();
        assert!((cost - (40.0 + 200.0 + 480.0 + 200.0 + 1000.0) / 1e6).abs() < 1e-12, "{cost}");
    }

    #[test]
    fn subagents_join_their_conversation_and_erased_files_are_kept() {
        let t = tempfile::tempdir().unwrap();
        let config = t.path().join(".claude");
        let project = config.join("projects").join("C--dev-ozmo");
        fs::create_dir_all(project.join("s1").join("subagents")).unwrap();
        fs::write(project.join("s1.jsonl"), MAIN).unwrap();
        fs::write(project.join("s1").join("subagents").join("agent-a.jsonl"), SUB).unwrap();
        let mut cache = Cache::default();
        assert_eq!(cache.refresh(std::slice::from_ref(&config)), 2);
        assert_eq!(cache.refresh(std::slice::from_ref(&config)), 0, "rien n'a changé");
        let convs = cache.conversations();
        assert_eq!(convs.len(), 1);
        let c = &convs[0];
        assert_eq!((c.title.as_str(), c.subagents, c.replies.len()), ("Export CSV corrigé", 1, 3));
        assert_eq!(c.repo(), "ozmo");
        assert_eq!(c.models(), ["Opus 5.5", "Haiku 5.5", "Sonnet 5.5"]);
        assert!(!c.gone);
        // Claude Code efface la transcription : la conversation reste, marquée.
        fs::remove_file(project.join("s1.jsonl")).unwrap();
        fs::remove_dir_all(project.join("s1")).unwrap();
        cache.refresh(std::slice::from_ref(&config));
        let convs = cache.conversations();
        assert_eq!(convs.len(), 1);
        assert!(convs[0].gone, "transcription effacée");
        assert_eq!(convs[0].replies.len(), 3, "sa consommation reste comptée");
    }

    #[test]
    fn a_resumed_session_does_not_count_old_replies_twice() {
        let mut cache = Cache::default();
        let mut add = |name: &str, text: &str| {
            let summary = parse_lines(name, false, lines(text));
            cache.files.insert(name.into(), CachedFile { config: "c".into(), summary, ..Default::default() });
        };
        add("s1", MAIN);
        add("s2", &MAIN.replace("09:00:00Z", "11:00:00Z"));
        let convs = cache.conversations();
        let counts: Vec<usize> = convs.iter().map(|c| c.replies.len()).collect();
        assert_eq!(counts.iter().sum::<usize>(), 2, "{counts:?}");
    }
}
