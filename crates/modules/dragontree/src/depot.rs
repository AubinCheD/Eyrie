//! Un dépôt sur le téléphone : un client git presque complet, mené par DragonTree sur le PC.
//!
//! Les écrans, tous sous `depot/<clé>` :
//! - l'accueil du dépôt : branche, suivi, opération en cours (fusion, rebase…) à poursuivre ou abandonner, fetch,
//!   pull, push, et les rubriques ;
//! - `modifs` : conflits (garder une version, marquer résolu), indexé, non indexé ; indexer, désindexer, annuler,
//!   valider (ou corriger le dernier commit), mettre de côté ; `fichier/<f>` : le diff d'un fichier ;
//! - `historique` : les derniers commits ; `commit/<id>` : message, fichiers, cherry-pick, revert, branche ou
//!   étiquette ici, reset, sortir ce commit ; `commit/<id>/<f>` : le diff d'un fichier du commit ;
//! - `branches` : basculer, fusionner, rebaser, pousser, renommer, supprimer, nouvelle branche, suivre une branche
//!   distante ;
//! - `remises` (stash) : appliquer, appliquer et retirer, jeter ; `etiquettes` : pousser, supprimer, créer ;
//!   `worktrees` : supprimer, élaguer.
//!
//! Ce module ne fait qu'écrire les écrans à partir de ce que DragonTree sait du dépôt ([`RepoView`]) et traduire une
//! action en opération de DragonTree ([`RemoteOp`]) ; `mobile.rs` la lui confie. Un fichier est désigné par une
//! empreinte de son chemin, un commit par son identifiant, une branche ou une étiquette par son nom (git interdit `:`
//! dans un nom de référence).

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};

use dragontree_app::repo::RemoteOp;
use dragontree_git::{
    ChangeKind, Commit, CommitInfo, FileChange, FileDiff, FileState, LineKind, Oid, OpState, RefsSnapshot, ResetMode,
    StashEntry, StatusEntry, StatusSnapshot, Worktree,
};
use suite_sdk::mobile::ui::{Act, DIFF_ADDED, DIFF_CONTEXT, DIFF_HEADER, DIFF_REMOVED, Row, Screen, Tone};

/// Commits montrés dans l'historique.
pub const LOG_COUNT: usize = 60;
/// Un diff montré est coupé au-delà.
const DIFF_LINES: usize = 600;

/// Empreinte courte d'un chemin de fichier : ce qui le désigne dans un chemin d'écran ou une action.
pub fn file_key(path: &str) -> String {
    let mut h = DefaultHasher::new();
    path.hash(&mut h);
    format!("{:x}", h.finish() & 0xffff_ffff)
}

/// Ce qu'on sait d'un dépôt : ce que DragonTree en a lu, et ce qui a été lu à part pour l'écran (diff, commit).
#[derive(Clone, Debug, Default)]
pub struct RepoView {
    pub status: Option<Result<StatusSnapshot, String>>,
    pub refs: Option<RefsSnapshot>,
    pub stashes: Vec<StashEntry>,
    pub worktrees: Vec<Worktree>,
    pub remotes: Vec<String>,
    pub op: Option<OpState>,
    pub last_message: String,
    pub log: Option<Result<Vec<Commit>, String>>,
    /// Commit (identifiant complet) → ses informations et ses fichiers.
    pub commits: HashMap<String, Result<(CommitInfo, Vec<FileChange>), String>>,
    /// Diff rendu : `w:<fichier>` (dossier de travail) ou `c:<commit>:<fichier>`.
    pub diffs: HashMap<String, Result<Vec<(u8, String)>, String>>,
}

impl RepoView {
    fn status(&self) -> Option<&StatusSnapshot> {
        self.status.as_ref().and_then(|s| s.as_ref().ok())
    }

    pub fn current_branch(&self) -> Option<String> {
        self.status().and_then(|s| s.branch.clone())
    }

    /// Le fichier d'empreinte `key` parmi les modifications.
    pub fn entry(&self, key: &str) -> Option<&StatusEntry> {
        self.status()?.entries.iter().find(|e| file_key(&e.path) == key)
    }

    /// Le remote où pousser une branche sans suivi ou une étiquette : celui de la branche suivie, `origin`, le premier.
    pub fn default_remote(&self) -> Option<String> {
        let upstream = self.status().and_then(|s| s.upstream.clone());
        let from_upstream = upstream.and_then(|u| {
            self.remotes.iter().filter(|r| u.starts_with(&format!("{r}/"))).max_by_key(|r| r.len()).cloned()
        });
        from_upstream
            .or_else(|| self.remotes.iter().find(|r| *r == "origin").cloned())
            .or(self.remotes.first().cloned())
    }
}

fn op_name(op: &OpState) -> &'static str {
    match op {
        OpState::None => "",
        OpState::Merge => "Fusion",
        OpState::Rebase { .. } => "Rebase",
        OpState::CherryPick => "Cherry-pick",
        OpState::Revert => "Revert",
    }
}

fn letter(e: &StatusEntry, staged: bool) -> char {
    if e.conflict.is_some() {
        return 'U';
    }
    let state = if staged { e.index } else { e.worktree };
    match state {
        FileState::Modified => 'M',
        FileState::Added => 'A',
        FileState::Deleted => 'D',
        FileState::Renamed => 'R',
        FileState::Copied => 'C',
        FileState::TypeChanged => 'T',
        FileState::Untracked => '?',
        FileState::Ignored => '!',
        FileState::Unmodified => ' ',
    }
}

fn change_letter(c: &FileChange) -> char {
    match c.kind {
        ChangeKind::Added => 'A',
        ChangeKind::Modified => 'M',
        ChangeKind::Deleted => 'D',
        ChangeKind::Renamed => 'R',
        ChangeKind::Copied => 'C',
        ChangeKind::TypeChanged => 'T',
        ChangeKind::Unmerged => 'U',
    }
}

fn ago(t: i64, now: i64) -> String {
    match now.saturating_sub(t) {
        s if s < 60 => "à l'instant".into(),
        s if s < 3600 => format!("il y a {} min", s / 60),
        s if s < 86_400 => format!("il y a {} h", s / 3600),
        s => format!("il y a {} j", s / 86_400),
    }
}

fn sync(ahead: u32, behind: u32) -> String {
    match (ahead, behind) {
        (0, 0) => "à jour".into(),
        (a, 0) => format!("↑{a}"),
        (0, b) => format!("↓{b}"),
        (a, b) => format!("↑{a} ↓{b}"),
    }
}

/// Le diff d'un fichier en lignes (en-têtes de morceaux, contexte, ajouts, retraits), coupé s'il est long.
pub fn render_diff(d: &FileDiff) -> Vec<(u8, String)> {
    if d.is_binary {
        return vec![(DIFF_HEADER, "Fichier binaire : pas de diff à montrer.".into())];
    }
    if d.truncated {
        return vec![(DIFF_HEADER, "Diff trop volumineux : voir sur le PC.".into())];
    }
    let mut out = Vec::new();
    for h in &d.hunks {
        out.push((DIFF_HEADER, h.header.clone()));
        for l in &h.lines {
            let (kind, sign) = match l.kind {
                LineKind::Added => (DIFF_ADDED, '+'),
                LineKind::Removed => (DIFF_REMOVED, '-'),
                LineKind::Context => (DIFF_CONTEXT, ' '),
            };
            out.push((kind, format!("{sign}{}", l.text.trim_end_matches('\r'))));
            if out.len() >= DIFF_LINES {
                out.push((DIFF_HEADER, "… (coupé : la suite sur le PC)".into()));
                return out;
            }
        }
    }
    if out.is_empty() {
        out.push((DIFF_HEADER, "Aucune différence de contenu.".into()));
    }
    out
}

/// Le bandeau d'une opération en cours, et ses boutons (comme celui de DragonTree : continuer une fois les conflits
/// résolus).
fn op_block(s: &mut Screen, v: &RepoView) {
    let Some(op) = v.op.filter(|o| *o != OpState::None) else { return };
    let conflicts = v.status().map_or(0, |st| st.conflicted().len());
    let text = if conflicts > 0 {
        format!(
            "{} en cours : {conflicts} conflit(s) à résoudre (Modifications, ou dans DragonTree sur le PC), puis              continuer.",
            op_name(&op)
        )
    } else {
        format!("{} en cours : continuer ou abandonner.", op_name(&op))
    };
    s.note(text, Tone::Warn);
    let mut acts = Vec::new();
    if conflicts > 0 {
        s.row(
            Row::new("Voir les conflits").detail(conflicts.to_string()).glyph("⚠️").tone(Tone::Danger).open("modifs"),
        );
    } else {
        acts.push(Act::new("continuer", "Continuer").primary());
    }
    if matches!(op, OpState::Rebase { .. }) {
        acts.push(Act::new("passer", "Passer ce commit").confirm("Passer le commit en cours du rebase ?"));
    }
    acts.push(
        Act::new("abandonner", "Abandonner")
            .danger()
            .confirm(format!("Abandonner la {} en cours et revenir à l'état d'avant ?", op_name(&op).to_lowercase())),
    );
    s.buttons(acts);
}

/// L'accueil d'un dépôt. `job` : ce qu'a donné la dernière opération (ou celle en cours, `busy`).
pub fn overview(name: &str, path: &str, v: &RepoView, job: Option<(&str, Tone)>, busy: bool) -> Screen {
    let mut s = Screen::new(name).subtitle(path);
    s.acts.push(Act::new("actualiser", "Actualiser").glyph("🔄"));
    s.acts.push(Act::new("pc", "Ouvrir dans DragonTree sur le PC").glyph("🖥️"));
    if let Some((text, tone)) = job {
        s.note(text, tone);
    }
    let st = match &v.status {
        None => {
            s.empty("Lecture du dépôt…");
            return s;
        }
        Some(Err(e)) => {
            s.note(e, Tone::Danger);
            return s;
        }
        Some(Ok(st)) => st,
    };
    op_block(&mut s, v);
    let branch = st.branch.clone().unwrap_or_else(|| "HEAD détachée".into());
    s.row(Row::new("Branche").detail(branch).glyph("🌿").open("branches"));
    let upstream = st.upstream.clone().unwrap_or_else(|| "aucune".into());
    let tone = if st.ahead > 0 && st.behind > 0 { Tone::Warn } else { Tone::Ok };
    s.row(Row::new("Suivi").subtitle(upstream).detail(sync(st.ahead, st.behind)).glyph("🔁").tone(tone));
    if !busy {
        let mut acts = vec![Act::new("fetch", "Fetch").glyph("⬇️")];
        if st.ahead > 0 && st.behind > 0 {
            acts.push(
                Act::new("pull-fusion", "Pull (fusion)")
                    .confirm("Fusionner la branche distante dans la vôtre (un commit de fusion) ?"),
            );
            acts.push(
                Act::new("pull-rebase", "Pull (rebase)").confirm("Rejouer vos commits sur la branche distante ?"),
            );
        } else if st.upstream.is_some() {
            acts.push(Act::new("pull", "Pull"));
        }
        if st.upstream.is_some() && st.behind > 0 && st.ahead > 0 {
            acts.push(
                Act::new("push-force", "Push forcé").danger().confirm(
                    "Écraser la branche distante (avec bail : refusé si elle a bougé depuis le dernier fetch) ?",
                ),
            );
        } else if st.upstream.is_some() {
            acts.push(Act::new("push", "Push").primary().confirm("Pousser la branche vers sa branche distante ?"));
        } else if let Some(b) = &st.branch {
            acts.push(Act::new(format!("publier:{b}"), "Publier la branche").primary());
        }
        s.buttons(acts);
    } else {
        s.note("Une opération est en cours sur ce dépôt.", Tone::Info);
    }
    let staged = st.staged().len();
    let changes = st.entries.iter().filter(|e| e.worktree != FileState::Ignored).count();
    s.section("Le dépôt", "");
    let sub = if changes == 0 {
        "rien à valider".to_string()
    } else {
        format!("{changes} fichier(s), dont {staged} indexé(s)")
    };
    let mut row = Row::new("Modifications").subtitle(sub).glyph("✏️").open("modifs");
    if changes > 0 {
        row = row.badge(changes.to_string());
    }
    s.row(row);
    s.row(Row::new("Historique").subtitle("Les derniers commits").glyph("🕘").open("historique"));
    let (locals, remotes) = v.refs.as_ref().map_or((0, 0), |r| (r.locals.len(), r.remotes.len()));
    s.row(
        Row::new("Branches")
            .subtitle(format!("{locals} locale(s), {remotes} distante(s)"))
            .glyph("🌿")
            .open("branches"),
    );
    s.row(Row::new("Remises").subtitle(format!("{} remise(s) (stash)", v.stashes.len())).glyph("📦").open("remises"));
    let tags = v.refs.as_ref().map_or(0, |r| r.tags.len());
    s.row(Row::new("Étiquettes").subtitle(format!("{tags} étiquette(s)")).glyph("🏷️").open("etiquettes"));
    if v.worktrees.len() > 1 {
        s.row(
            Row::new("Worktrees").subtitle(format!("{} worktree(s)", v.worktrees.len())).glyph("🌳").open("worktrees"),
        );
    }
    s
}

/// Les gestes d'un fichier en conflit : garder une version (puis marqué résolu), ou le marquer résolu tel quel. Dans
/// un rebase, « notre » version est celle de la base, et la version qui arrive, le commit rejoué.
fn conflict_acts(v: &RepoView, e: &StatusEntry) -> Vec<Act> {
    let key = file_key(&e.path);
    let (ours, theirs) = if matches!(v.op, Some(OpState::Rebase { .. })) {
        ("Garder la version de la base", "Garder la version du commit rejoué")
    } else {
        ("Garder notre version", "Garder la version qui arrive")
    };
    let name = e.path.rsplit('/').next().unwrap_or(&e.path);
    vec![
        Act::new(format!("garder-local:{key}"), ours)
            .glyph("⬅️")
            .confirm(format!("{ours} pour {name} et le marquer résolu ?")),
        Act::new(format!("garder-entrant:{key}"), theirs)
            .glyph("➡️")
            .confirm(format!("{theirs} pour {name} et le marquer résolu ?")),
        Act::new(format!("indexer:{key}"), "Marquer résolu tel quel")
            .glyph("✅")
            .confirm(format!("{name} est-il bien résolu (plus de marques de conflit) ?")),
    ]
}

fn file_row(v: &RepoView, e: &StatusEntry, staged: bool) -> Row {
    let key = file_key(&e.path);
    let mut row = Row::new(&e.path).glyph(letter(e, staged).to_string()).open(format!("fichier/{key}"));
    if let Some(old) = &e.orig_path {
        row = row.subtitle(format!("renommé depuis {old}"));
    }
    if e.conflict.is_some() {
        let mut row = row.tone(Tone::Danger).badge("conflit");
        row.acts = conflict_acts(v, e);
        return row;
    }
    if staged {
        row.act(Act::new(format!("desindexer:{key}"), "Désindexer").glyph("➖"))
    } else {
        row.act(Act::new(format!("indexer:{key}"), "Indexer").glyph("➕")).act(
            Act::new(format!("annuler:{key}"), "Annuler les modifications")
                .glyph("↩️")
                .danger()
                .confirm(format!("Annuler les modifications de {} ? Elles seront perdues.", e.path)),
        )
    }
}

/// Pendant une opération sur le dépôt, une ligne n'offre plus que d'ouvrir son diff.
fn quiet(mut row: Row, busy: bool) -> Row {
    if busy {
        row.acts.clear();
        row.tap = None;
    }
    row
}

/// Les modifications : conflits, indexé, non indexé, et ce qu'on peut en faire.
pub fn changes(v: &RepoView, busy: bool) -> Screen {
    let mut s = Screen::new("Modifications");
    let Some(st) = v.status() else {
        s.empty("Lecture du dépôt…");
        return s;
    };
    let conflicts = st.conflicted();
    let staged = st.staged();
    let unstaged = st.unstaged();
    s.subtitle = format!("{} indexé(s), {} non indexé(s)", staged.len(), unstaged.len());
    if !busy {
        if !staged.is_empty() {
            s.acts.push(
                Act::new("valider", "Valider…")
                    .glyph("✅")
                    .primary()
                    .input("Message du commit", "Ce que fait ce commit")
                    .multiline(),
            );
        }
        if !v.last_message.is_empty() {
            s.acts.push(
                Act::new("corriger", "Corriger le dernier commit…")
                    .glyph("✏️")
                    .input("Message du commit corrigé", "")
                    .input_value(v.last_message.trim())
                    .multiline(),
            );
        }
        if !unstaged.is_empty() {
            s.acts.push(Act::new("tout-indexer", "Tout indexer").glyph("➕"));
        }
        if !staged.is_empty() {
            s.acts.push(Act::new("tout-desindexer", "Tout désindexer").glyph("➖"));
        }
        if !st.entries.is_empty() {
            s.acts.push(
                Act::new("mettre-de-cote", "Mettre de côté (stash)…").glyph("📦").input("Message (facultatif)", ""),
            );
            s.acts.push(
                Act::new("tout-annuler", "Tout annuler")
                    .glyph("↩️")
                    .danger()
                    .confirm("Annuler toutes les modifications ? Elles seront perdues, fichiers non suivis compris."),
            );
        }
    }
    if st.entries.is_empty() {
        s.empty("Rien à valider : le dossier de travail est propre.");
        return s;
    }
    if !conflicts.is_empty() {
        s.section("Conflits", conflicts.len().to_string());
        for e in conflicts {
            s.row(quiet(file_row(v, e, false), busy));
        }
    }
    if !staged.is_empty() {
        s.section("Indexé", staged.len().to_string());
        for e in staged {
            s.row(quiet(file_row(v, e, true), busy));
        }
    }
    if !unstaged.is_empty() {
        s.section("Non indexé", unstaged.len().to_string());
        for e in unstaged {
            s.row(quiet(file_row(v, e, false), busy));
        }
    }
    if !busy && !st.staged().is_empty() {
        s.buttons(vec![
            Act::new("valider", "Valider…").primary().input("Message du commit", "Ce que fait ce commit").multiline(),
        ]);
    } else if !busy && !st.unstaged().is_empty() {
        s.buttons(vec![Act::new("tout-indexer", "Tout indexer").primary()]);
    }
    s
}

/// Le diff d'un fichier du dossier de travail (indexé et non indexé).
pub fn file(v: &RepoView, key: &str, busy: bool) -> Option<Screen> {
    let e = v.entry(key)?;
    let mut s = Screen::new(e.path.rsplit('/').next().unwrap_or(&e.path)).subtitle(&e.path);
    let staged = e.conflict.is_none() && !matches!(e.index, FileState::Unmodified | FileState::Untracked);
    let unstaged = !matches!(e.worktree, FileState::Unmodified | FileState::Ignored);
    if busy {
        // Pendant une opération : le diff seulement.
    } else if e.conflict.is_some() {
        s.note("Conflit : gardez une version, ou corrigez le fichier sur le PC puis marquez-le résolu.", Tone::Danger);
        s.acts.extend(conflict_acts(v, e));
    } else if unstaged {
        s.acts.push(Act::new(format!("indexer:{key}"), "Indexer").glyph("➕"));
    }
    if staged && !busy {
        s.acts.push(Act::new(format!("desindexer:{key}"), "Désindexer").glyph("➖"));
    }
    if unstaged && e.conflict.is_none() && !busy {
        s.acts.push(
            Act::new(format!("annuler:{key}"), "Annuler les modifications")
                .glyph("↩️")
                .danger()
                .confirm(format!("Annuler les modifications de {} ? Elles seront perdues.", e.path)),
        );
    }
    match v.diffs.get(&format!("w:{key}")) {
        None => s.empty("Lecture du diff…"),
        Some(Err(e)) => s.note(e, Tone::Danger),
        Some(Ok(lines)) => s.diff(lines.clone()),
    };
    Some(s)
}

/// L'historique de la branche courante.
pub fn history(v: &RepoView, now: i64) -> Screen {
    let mut s = Screen::new("Historique").subtitle(v.current_branch().unwrap_or_else(|| "HEAD détachée".into()));
    s.acts.push(Act::new("actualiser", "Actualiser").glyph("🔄"));
    match &v.log {
        None => {
            s.empty("Lecture de l'historique…");
        }
        Some(Err(e)) => {
            s.note(e, Tone::Danger);
        }
        Some(Ok(list)) if list.is_empty() => {
            s.empty("Aucun commit.");
        }
        Some(Ok(list)) => {
            for c in list {
                let glyph = if c.parents.len() > 1 { "🔀" } else { "●" };
                s.row(
                    Row::new(c.summary.as_ref())
                        .subtitle(format!("{} · {}", c.author, ago(c.author_time, now)))
                        .detail(c.id.short())
                        .glyph(glyph)
                        .open(format!("commit/{}", c.id.to_hex())),
                );
            }
        }
    }
    s
}

/// Un commit : son message, ses fichiers, et ce qu'on peut en faire.
pub fn commit(v: &RepoView, id: &str, now: i64, busy: bool) -> Screen {
    let short: String = id.chars().take(8).collect();
    let mut s = Screen::new(format!("Commit {short}"));
    let current = v.current_branch();
    if !busy {
        s.acts.push(
            Act::new(format!("cherry-pick:{id}"), "Cherry-pick ici")
                .glyph("🍒")
                .confirm(format!("Appliquer le commit {short} sur {} ?", current.as_deref().unwrap_or("HEAD"))),
        );
        s.acts.push(
            Act::new(format!("revert:{id}"), "Annuler ce commit (revert)")
                .glyph("↩️")
                .confirm(format!("Créer un commit qui défait {short} ?")),
        );
        s.acts.push(
            Act::new(format!("branche-ici:{id}"), "Nouvelle branche ici…")
                .glyph("🌿")
                .input("Nom de la branche", "feature/…"),
        );
        s.acts.push(
            Act::new(format!("etiquette-ici:{id}"), "Nouvelle étiquette ici…")
                .glyph("🏷️")
                .input("Nom de l'étiquette", "v1.2.0"),
        );
        if let Some(b) = &current {
            s.acts.push(
                Act::new(format!("reset-soft:{id}"), format!("Ramener {b} ici (soft)"))
                    .glyph("⏪")
                    .confirm(format!("Ramener {b} sur {short} en gardant les modifications indexées ?")),
            );
            s.acts.push(
                Act::new(format!("reset-mixed:{id}"), format!("Ramener {b} ici (mixed)"))
                    .glyph("⏪")
                    .confirm(format!("Ramener {b} sur {short} en gardant les modifications, non indexées ?")),
            );
            s.acts.push(
                Act::new(format!("reset-hard:{id}"), format!("Ramener {b} ici (hard)"))
                    .glyph("⚠️")
                    .danger()
                    .confirm(format!("Ramener {b} sur {short} et PERDRE toutes les modifications non validées ?")),
            );
        }
        s.acts.push(
            Act::new(format!("detacher:{id}"), "Sortir ce commit (HEAD détachée)")
                .glyph("📍")
                .confirm(format!("Sortir {short} en HEAD détachée ?")),
        );
    }
    match v.commits.get(id) {
        None => {
            s.empty("Lecture du commit…");
        }
        Some(Err(e)) => {
            s.note(e, Tone::Danger);
        }
        Some(Ok((info, files))) => {
            s.title = info.message.lines().next().unwrap_or(&short).to_string();
            s.subtitle = format!("{short} · {} · {}", info.author, ago(info.author_time, now));
            let body = info.message.trim();
            if body.lines().count() > 1 {
                s.text(body, false);
            }
            if info.parents.len() > 1 {
                s.note(
                    format!("Commit de fusion ({} parents) : fichiers comparés au premier.", info.parents.len()),
                    Tone::Info,
                );
            }
            s.section("Fichiers", files.len().to_string());
            for f in files {
                let mut row = Row::new(&f.path)
                    .glyph(change_letter(f).to_string())
                    .open(format!("commit/{id}/{}", file_key(&f.path)));
                if let Some(old) = &f.old_path {
                    row = row.subtitle(format!("depuis {old}"));
                }
                s.row(row);
            }
        }
    }
    s
}

/// Le diff d'un fichier d'un commit.
pub fn commit_file(v: &RepoView, id: &str, key: &str) -> Option<Screen> {
    let Ok((_, files)) = v.commits.get(id)? else { return None };
    let f = files.iter().find(|f| file_key(&f.path) == key)?;
    let mut s = Screen::new(f.path.rsplit('/').next().unwrap_or(&f.path)).subtitle(&f.path);
    match v.diffs.get(&format!("c:{id}:{key}")) {
        None => s.empty("Lecture du diff…"),
        Some(Err(e)) => s.note(e, Tone::Danger),
        Some(Ok(lines)) => s.diff(lines.clone()),
    };
    Some(s)
}

/// Les branches locales et distantes.
pub fn branches(v: &RepoView, busy: bool) -> Screen {
    let mut s = Screen::new("Branches");
    let Some(refs) = &v.refs else {
        s.empty("Lecture des branches…");
        return s;
    };
    let current = refs.current_branch().map(str::to_string);
    if let Some(c) = &current {
        s.subtitle = format!("Courante : {c}");
    }
    if !busy {
        s.acts.push(
            Act::new("nouvelle-branche", "Nouvelle branche…")
                .glyph("🌿")
                .input("Nom (créée depuis HEAD et sortie)", "feature/…"),
        );
    }
    s.section("Locales", refs.locals.len().to_string());
    for b in &refs.locals {
        let head = current.as_deref() == Some(b.name.as_str());
        let mut sub = match (&b.upstream, b.upstream_gone) {
            (Some(u), true) => format!("{u} (supprimée)"),
            (Some(u), false) => format!("{u} · {}", sync(b.ahead, b.behind)),
            (None, _) => "non publiée".into(),
        };
        if head {
            sub = format!("courante · {sub}");
        }
        let mut row = Row::new(&b.name).subtitle(sub).glyph("🌿");
        if head {
            row = row.badge("courante").tone(Tone::Ok);
        }
        if !busy {
            let n = &b.name;
            if !head {
                row = row.tap(
                    Act::new(format!("basculer:{n}"), format!("Basculer sur {n}"))
                        .confirm(format!("Basculer sur {n} ?")),
                );
                row = row.act(Act::new(format!("basculer:{n}"), "Basculer").glyph("↪️"));
                if let Some(c) = &current {
                    row = row
                        .act(
                            Act::new(format!("fusionner:{n}"), format!("Fusionner dans {c}"))
                                .glyph("🔀")
                                .confirm(format!("Fusionner {n} dans {c} ?")),
                        )
                        .act(
                            Act::new(format!("rebaser:{n}"), format!("Rebaser {c} sur {n}"))
                                .glyph("⤴️")
                                .confirm(format!("Rejouer les commits de {c} sur {n} ?")),
                        );
                }
            }
            row = row.act(Act::new(format!("publier:{n}"), "Pousser").glyph("⬆️")).act(
                Act::new(format!("renommer:{n}"), "Renommer…").glyph("✏️").input("Nouveau nom", "").input_value(n),
            );
            if !head {
                row = row
                    .act(
                        Act::new(format!("supprimer-branche:{n}"), "Supprimer")
                            .glyph("🗑️")
                            .danger()
                            .confirm(format!("Supprimer la branche {n} ? (refusé si elle n'est pas fusionnée)")),
                    )
                    .act(
                        Act::new(format!("supprimer-branche-force:{n}"), "Supprimer même non fusionnée")
                            .glyph("⚠️")
                            .danger()
                            .confirm(format!("Supprimer {n} et perdre ses commits non fusionnés ?")),
                    );
            }
        }
        s.row(row);
    }
    let remotes: Vec<_> = refs.remotes.iter().filter(|r| !r.is_symbolic).collect();
    if !remotes.is_empty() {
        s.section("Distantes", remotes.len().to_string());
        for r in remotes {
            let full = format!("{}/{}", r.remote, r.name);
            let mut row = Row::new(&full).glyph("☁️");
            if !busy {
                let tracked = refs.locals.iter().any(|b| b.upstream.as_deref() == Some(full.as_str()));
                if !tracked {
                    row = row.act(
                        Act::new(format!("suivre:{full}"), "Créer une branche locale qui la suit")
                            .glyph("🌿")
                            .input("Nom de la branche locale", "")
                            .input_value(&r.name),
                    );
                }
                if let Some(c) = &current {
                    row = row.act(
                        Act::new(format!("fusionner:{full}"), format!("Fusionner dans {c}"))
                            .glyph("🔀")
                            .confirm(format!("Fusionner {full} dans {c} ?")),
                    );
                }
            }
            s.row(row);
        }
    }
    s
}

/// Les remises (stash).
pub fn stashes(v: &RepoView, now: i64, busy: bool) -> Screen {
    let mut s = Screen::new("Remises").subtitle("git stash");
    if !busy {
        s.acts.push(Act::new("mettre-de-cote", "Mettre de côté…").glyph("📦").input("Message (facultatif)", ""));
    }
    if v.stashes.is_empty() {
        s.empty("Aucune remise.");
    }
    for st in &v.stashes {
        let id = st.oid.to_hex();
        let mut row = Row::new(&st.message).subtitle(ago(st.time, now)).glyph("📦");
        if !busy {
            row = row
                .act(Act::new(format!("appliquer:{id}"), "Appliquer").glyph("📥"))
                .act(Act::new(format!("depiler:{id}"), "Appliquer et retirer").glyph("📤"))
                .act(
                    Act::new(format!("jeter:{id}"), "Jeter")
                        .glyph("🗑️")
                        .danger()
                        .confirm("Jeter cette remise ? Son contenu sera perdu."),
                );
        }
        s.row(row);
    }
    s
}

/// Les étiquettes.
pub fn tags(v: &RepoView, busy: bool) -> Screen {
    let mut s = Screen::new("Étiquettes");
    if !busy {
        s.acts.push(Act::new("nouvelle-etiquette", "Nouvelle étiquette sur HEAD…").glyph("🏷️").input("Nom", "v1.2.0"));
    }
    let list = v.refs.as_ref().map(|r| r.tags.clone()).unwrap_or_default();
    if list.is_empty() {
        s.empty("Aucune étiquette.");
    }
    for t in list.iter().rev() {
        let mut row = Row::new(&t.name).detail(t.oid.short()).glyph("🏷️").open(format!("commit/{}", t.oid.to_hex()));
        if t.annotated {
            row = row.subtitle("annotée");
        }
        if !busy {
            row = row.act(Act::new(format!("pousser-etiquette:{}", t.name), "Pousser").glyph("⬆️")).act(
                Act::new(format!("supprimer-etiquette:{}", t.name), "Supprimer (en local)")
                    .glyph("🗑️")
                    .danger()
                    .confirm(format!("Supprimer l'étiquette {} de ce dépôt ?", t.name)),
            );
        }
        s.row(row);
    }
    s
}

/// Les worktrees.
pub fn worktrees(v: &RepoView, busy: bool) -> Screen {
    let mut s = Screen::new("Worktrees");
    if !busy && v.worktrees.iter().any(|w| w.prunable) {
        s.acts.push(Act::new("elaguer", "Élaguer les disparus").glyph("🧹"));
    }
    for w in &v.worktrees {
        let name = w.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let mut row = Row::new(name)
            .subtitle(w.branch.clone().unwrap_or_else(|| "HEAD détachée".into()))
            .detail(if w.is_main { "principal" } else { "" })
            .glyph("🌳");
        if w.is_current {
            row = row.badge("celui-ci");
        }
        if w.prunable {
            row = row.tone(Tone::Warn).subtitle("dossier disparu");
        }
        if !busy && !w.is_main && !w.is_current && !w.locked {
            row = row.act(
                Act::new(format!("supprimer-worktree:{}", file_key(&w.path.to_string_lossy())), "Supprimer")
                    .glyph("🗑️")
                    .danger()
                    .confirm(format!("Supprimer le worktree {} ? (refusé s'il a des modifications)", w.path.display())),
            );
        }
        s.row(row);
    }
    s
}

fn oid(hex: &str) -> Result<Oid, String> {
    Oid::from_hex(hex).ok_or_else(|| "identifiant de commit illisible".into())
}

/// Une action d'un écran du dépôt en opération de DragonTree, avec son libellé. `input` : la saisie demandée.
pub fn parse(v: &RepoView, action: &str, input: Option<&str>) -> Result<(RemoteOp, String), String> {
    let text = || input.map(str::trim).filter(|t| !t.is_empty()).map(str::to_string);
    let needed = || text().ok_or_else(|| "Saisie vide.".to_string());
    let (verb, arg) = action.split_once(':').unwrap_or((action, ""));
    let arg = arg.to_string();
    let st = v.status();
    let file = |key: &str| v.entry(key).ok_or_else(|| "Ce fichier n'est plus dans les modifications.".to_string());
    let stash = |hex: &str| {
        let s = v.stashes.iter().find(|s| s.oid.to_hex() == hex).ok_or("Cette remise n'existe plus.")?;
        Ok::<_, String>((s.oid, s.message.clone()))
    };
    let op = match verb {
        "fetch" => (RemoteOp::Fetch, "Fetch".into()),
        "pull" => (RemoteOp::Pull { rebase: None }, "Pull".into()),
        "pull-fusion" => (RemoteOp::Pull { rebase: Some(false) }, "Pull (fusion)".into()),
        "pull-rebase" => (RemoteOp::Pull { rebase: Some(true) }, "Pull (rebase)".into()),
        "push" => (RemoteOp::Push { force: false }, "Push".into()),
        "push-force" => (RemoteOp::Push { force: true }, "Push forcé".into()),
        "indexer" => {
            let e = file(&arg)?;
            (RemoteOp::Stage(vec![e.path.clone()]), format!("Indexer {}", e.path))
        }
        "desindexer" => {
            let e = file(&arg)?;
            (RemoteOp::Unstage(vec![e.path.clone()]), format!("Désindexer {}", e.path))
        }
        "garder-local" | "garder-entrant" => {
            let e = file(&arg)?;
            if e.conflict.is_none() {
                return Err("Ce fichier n'est plus en conflit.".into());
            }
            let op = RemoteOp::Resolve { paths: vec![e.path.clone()], ours: verb == "garder-local" };
            (op, format!("Résoudre {}", e.path))
        }
        "annuler" => {
            let e = file(&arg)?;
            let untracked = e.worktree == FileState::Untracked;
            let (tracked, untracked) =
                if untracked { (vec![], vec![e.path.clone()]) } else { (vec![e.path.clone()], vec![]) };
            (RemoteOp::Discard { tracked, untracked }, format!("Annuler {}", e.path))
        }
        "tout-indexer" => {
            let paths = st.map(|s| s.unstaged().iter().map(|e| e.path.clone()).collect()).unwrap_or_default();
            (RemoteOp::Stage(paths), "Tout indexer".into())
        }
        "tout-desindexer" => {
            let paths = st.map(|s| s.staged().iter().map(|e| e.path.clone()).collect()).unwrap_or_default();
            (RemoteOp::Unstage(paths), "Tout désindexer".into())
        }
        "tout-annuler" => {
            let (mut tracked, mut untracked) = (Vec::new(), Vec::new());
            for e in st.map(|s| s.entries.iter().collect::<Vec<_>>()).unwrap_or_default() {
                match e.worktree {
                    FileState::Untracked => untracked.push(e.path.clone()),
                    FileState::Ignored => {}
                    _ => tracked.push(e.path.clone()),
                }
            }
            (RemoteOp::Discard { tracked, untracked }, "Tout annuler".into())
        }
        "valider" => {
            if st.is_none_or(|s| s.staged().is_empty()) {
                return Err("Rien d'indexé : indexez d'abord des modifications.".into());
            }
            (RemoteOp::Commit { message: needed()?, amend: false }, "Valider".into())
        }
        "corriger" => (RemoteOp::Commit { message: needed()?, amend: true }, "Corriger le dernier commit".into()),
        "mettre-de-cote" => (RemoteOp::StashSave { message: text().unwrap_or_default() }, "Mettre de côté".into()),
        "appliquer" => {
            let (oid, label) = stash(&arg)?;
            (RemoteOp::StashApply { oid, label }, "Appliquer la remise".into())
        }
        "depiler" => {
            let (oid, label) = stash(&arg)?;
            (RemoteOp::StashPop { oid, label }, "Appliquer et retirer la remise".into())
        }
        "jeter" => {
            let (oid, label) = stash(&arg)?;
            (RemoteOp::StashDrop { oid, label }, "Jeter la remise".into())
        }
        "basculer" => (RemoteOp::Checkout(arg.clone()), format!("Basculer sur {arg}")),
        "detacher" => (RemoteOp::Checkout(oid(&arg)?.to_hex()), "Sortir le commit".into()),
        "suivre" => {
            let local = needed()?;
            let op = RemoteOp::Track { remote_branch: arg.clone(), local: local.clone() };
            (op, format!("Suivre {arg} dans {local}"))
        }
        "nouvelle-branche" => {
            let name = needed()?;
            (RemoteOp::CreateBranch { name: name.clone(), start: "HEAD".into() }, format!("Nouvelle branche {name}"))
        }
        "branche-ici" => {
            let name = needed()?;
            (RemoteOp::CreateBranch { name: name.clone(), start: arg }, format!("Nouvelle branche {name}"))
        }
        "renommer" => {
            let new = needed()?;
            (RemoteOp::RenameBranch { old: arg.clone(), new: new.clone() }, format!("Renommer {arg} en {new}"))
        }
        "supprimer-branche" => (RemoteOp::DeleteBranch { name: arg.clone(), force: false }, format!("Supprimer {arg}")),
        "supprimer-branche-force" => {
            (RemoteOp::DeleteBranch { name: arg.clone(), force: true }, format!("Supprimer {arg}"))
        }
        "publier" => {
            let remote = v.default_remote().ok_or("Aucun remote : ajoutez-en un sur le PC.")?;
            (RemoteOp::PushBranch { name: arg.clone(), remote }, format!("Pousser {arg}"))
        }
        "fusionner" => (RemoteOp::Merge(arg.clone()), format!("Fusionner {arg}")),
        "rebaser" => (RemoteOp::Rebase(arg.clone()), format!("Rebaser sur {arg}")),
        "cherry-pick" => (RemoteOp::CherryPick(oid(&arg)?), "Cherry-pick".into()),
        "revert" => (RemoteOp::Revert(oid(&arg)?), "Revert".into()),
        "reset-soft" => (RemoteOp::Reset { rev: arg, mode: ResetMode::Soft }, "Reset (soft)".into()),
        "reset-mixed" => (RemoteOp::Reset { rev: arg, mode: ResetMode::Mixed }, "Reset (mixed)".into()),
        "reset-hard" => (RemoteOp::Reset { rev: arg, mode: ResetMode::Hard }, "Reset (hard)".into()),
        "continuer" => (RemoteOp::Continue, "Continuer".into()),
        "abandonner" => (RemoteOp::Abort, "Abandonner".into()),
        "passer" => (RemoteOp::Skip, "Passer le commit".into()),
        "nouvelle-etiquette" => {
            let name = needed()?;
            (RemoteOp::CreateTag { name: name.clone(), rev: "HEAD".into() }, format!("Étiquette {name}"))
        }
        "etiquette-ici" => {
            let name = needed()?;
            (RemoteOp::CreateTag { name: name.clone(), rev: arg }, format!("Étiquette {name}"))
        }
        "supprimer-etiquette" => (RemoteOp::DeleteTag(arg.clone()), format!("Supprimer l'étiquette {arg}")),
        "pousser-etiquette" => {
            let remote = v.default_remote().ok_or("Aucun remote : ajoutez-en un sur le PC.")?;
            (RemoteOp::PushTag { name: arg.clone(), remote }, format!("Pousser l'étiquette {arg}"))
        }
        "supprimer-worktree" => {
            let w = v
                .worktrees
                .iter()
                .find(|w| file_key(&w.path.to_string_lossy()) == arg)
                .ok_or("Ce worktree n'existe plus.")?;
            (RemoteOp::RemoveWorktree(w.path.clone()), format!("Supprimer le worktree {}", w.path.display()))
        }
        "elaguer" => (RemoteOp::PruneWorktrees, "Élaguer les worktrees".into()),
        _ => return Err("action inconnue".into()),
    };
    if let RemoteOp::Stage(p) | RemoteOp::Unstage(p) = &op.0
        && p.is_empty()
    {
        return Err("Aucun fichier concerné.".into());
    }
    Ok(op)
}

#[cfg(test)]
mod tests {
    use dragontree_git::{ConflictKind, HeadState, LocalBranch, Oid, RemoteBranch};
    use suite_sdk::mobile::ui::Block;

    use super::*;

    fn entry(path: &str, index: FileState, worktree: FileState) -> StatusEntry {
        StatusEntry { path: path.into(), orig_path: None, index, worktree, conflict: None, is_submodule: false }
    }

    fn view() -> RepoView {
        let status = StatusSnapshot {
            branch: Some("main".into()),
            head_oid: None,
            upstream: Some("origin/main".into()),
            ahead: 1,
            behind: 0,
            entries: vec![
                entry("src/a.rs", FileState::Modified, FileState::Unmodified),
                entry("src/b.rs", FileState::Unmodified, FileState::Modified),
                entry("neuf.txt", FileState::Untracked, FileState::Untracked),
                StatusEntry {
                    conflict: Some(ConflictKind::BothModified),
                    ..entry("conflit.rs", FileState::Modified, FileState::Modified)
                },
            ],
        };
        let oid = |c: char| Oid::from_hex(&c.to_string().repeat(40)).unwrap();
        let refs = RefsSnapshot {
            head: HeadState::Branch("main".into()),
            head_oid: Some(oid('a')),
            locals: vec![
                LocalBranch {
                    name: "main".into(),
                    oid: oid('a'),
                    upstream: Some("origin/main".into()),
                    ahead: 1,
                    behind: 0,
                    upstream_gone: false,
                    is_head: true,
                },
                LocalBranch {
                    name: "feature/x".into(),
                    oid: oid('b'),
                    upstream: None,
                    ahead: 0,
                    behind: 0,
                    upstream_gone: false,
                    is_head: false,
                },
            ],
            remotes: vec![RemoteBranch {
                remote: "origin".into(),
                name: "autre".into(),
                full: "refs/remotes/origin/autre".into(),
                oid: oid('c'),
                is_symbolic: false,
            }],
            tags: Vec::new(),
            labels: Default::default(),
        };
        RepoView {
            status: Some(Ok(status)),
            refs: Some(refs),
            remotes: vec!["origin".into()],
            op: Some(OpState::None),
            last_message: "Premier jet".into(),
            ..Default::default()
        }
    }

    #[test]
    fn changes_are_split_and_each_file_has_its_gestures() {
        let v = view();
        let s = changes(&v, false);
        let sections: Vec<&str> = s
            .blocks
            .iter()
            .filter_map(|b| match b {
                Block::Section { title, .. } => Some(title.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(sections, ["Conflits", "Indexé", "Non indexé"]);
        let ids = s.action_ids();
        let a = file_key("src/a.rs");
        let b = file_key("src/b.rs");
        assert!(ids.contains(&format!("desindexer:{a}")) && ids.contains(&format!("indexer:{b}")));
        assert!(ids.contains(&format!("annuler:{b}")) && ids.contains("valider") && ids.contains("corriger"));
        assert!(changes(&v, true).action_ids().is_empty(), "rien pendant une opération");
    }

    #[test]
    fn actions_become_git_operations() {
        let v = view();
        let b = file_key("src/b.rs");
        let n = file_key("neuf.txt");
        assert_eq!(parse(&v, &format!("indexer:{b}"), None).unwrap().0, RemoteOp::Stage(vec!["src/b.rs".into()]));
        assert_eq!(
            parse(&v, &format!("annuler:{n}"), None).unwrap().0,
            RemoteOp::Discard { tracked: vec![], untracked: vec!["neuf.txt".into()] },
            "un fichier non suivi est supprimé, pas restauré"
        );
        assert_eq!(
            parse(&v, "valider", Some("  Corrige l'export  ")).unwrap().0,
            RemoteOp::Commit { message: "Corrige l'export".into(), amend: false }
        );
        assert!(parse(&v, "valider", Some("   ")).is_err(), "message vide");
        assert_eq!(
            parse(&v, "publier:feature/x", None).unwrap().0,
            RemoteOp::PushBranch { name: "feature/x".into(), remote: "origin".into() }
        );
        assert_eq!(
            parse(&v, "suivre:origin/autre", Some("autre")).unwrap().0,
            RemoteOp::Track { remote_branch: "origin/autre".into(), local: "autre".into() }
        );
        assert!(parse(&v, "indexer:inconnu", None).is_err());
        assert!(parse(&v, "rm -rf", None).is_err());
        let mut clean = view();
        if let Some(Ok(s)) = &mut clean.status {
            s.entries.retain(|e| e.index == FileState::Unmodified);
        }
        assert!(parse(&clean, "valider", Some("x")).is_err(), "rien d'indexé");
    }

    #[test]
    fn branches_offer_switch_merge_rebase_and_tracking() {
        let v = view();
        let s = branches(&v, false);
        let ids = s.action_ids();
        for id in ["basculer:feature/x", "fusionner:feature/x", "rebaser:feature/x", "supprimer-branche:feature/x"] {
            assert!(ids.contains(id), "{id}");
        }
        assert!(!ids.contains("basculer:main") && !ids.contains("supprimer-branche:main"), "pas la courante");
        assert!(ids.contains("suivre:origin/autre") && ids.contains("nouvelle-branche"));
    }

    #[test]
    fn an_operation_in_progress_can_be_continued_or_aborted() {
        let mut v = view();
        v.op = Some(OpState::Rebase { interactive: false });
        let s = overview("ozmo", r"C:\dev\ozmo", &v, None, false);
        let ids = s.action_ids();
        // Un conflit reste : comme dans DragonTree, pas de « Continuer » avant sa résolution.
        assert!(!ids.contains("continuer") && ids.contains("abandonner") && ids.contains("passer"));
        assert!(s.opens().contains("modifs"), "les conflits à voir");
        assert!(s.blocks.iter().any(|b| matches!(b, Block::Note { text, .. } if text.contains("1 conflit"))));
        assert!(ids.contains("push"), "en avance seulement : push simple");
        if let Some(Ok(st)) = &mut v.status {
            st.entries.retain(|e| e.conflict.is_none());
        }
        assert!(overview("ozmo", "", &v, None, false).action_ids().contains("continuer"), "résolu : continuer");
    }

    #[test]
    fn a_conflict_is_resolved_by_keeping_one_side() {
        let mut v = view();
        v.op = Some(OpState::Merge);
        let c = file_key("conflit.rs");
        let ids = changes(&v, false).action_ids();
        assert!(ids.contains(&format!("garder-local:{c}")) && ids.contains(&format!("garder-entrant:{c}")));
        assert_eq!(
            parse(&v, &format!("garder-entrant:{c}"), None).unwrap().0,
            RemoteOp::Resolve { paths: vec!["conflit.rs".into()], ours: false }
        );
        assert!(parse(&v, &format!("garder-local:{}", file_key("src/b.rs")), None).is_err(), "pas en conflit");
        let s = file(&v, &c, false).unwrap();
        assert!(s.action_ids().contains(&format!("garder-local:{c}")));
        // Dans un rebase, « notre » version est celle de la base.
        v.op = Some(OpState::Rebase { interactive: false });
        let s = changes(&v, false);
        assert!(s.all_acts().iter().any(|a| a.label == "Garder la version de la base"));
    }

    #[test]
    fn a_diff_is_rendered_as_text() {
        use dragontree_git::{DiffLine, Hunk};
        let line = |kind, text: &str| DiffLine {
            kind,
            old_no: None,
            new_no: None,
            text: text.into(),
            no_newline: false,
            raw: None,
        };
        let d = FileDiff {
            hunks: vec![Hunk {
                old_start: 1,
                old_count: 2,
                new_start: 1,
                new_count: 2,
                header: "@@ -1,2 +1,2 @@".into(),
                lines: vec![
                    line(LineKind::Context, "fn main() {"),
                    line(LineKind::Removed, "  a();"),
                    line(LineKind::Added, "  b();"),
                ],
            }],
            ..Default::default()
        };
        assert_eq!(
            render_diff(&d),
            vec![
                (DIFF_HEADER, "@@ -1,2 +1,2 @@".to_string()),
                (DIFF_CONTEXT, " fn main() {".into()),
                (DIFF_REMOVED, "-  a();".into()),
                (DIFF_ADDED, "+  b();".into()),
            ]
        );
        assert!(render_diff(&FileDiff { is_binary: true, ..Default::default() })[0].1.contains("binaire"));
    }
}
