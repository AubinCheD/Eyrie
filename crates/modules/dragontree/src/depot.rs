//! Un dépôt sur le téléphone : un client git presque complet, mené par DragonTree sur le PC.
//!
//! Les écrans, tous sous `depot/<clé>` (chemins absolus dans le module : le téléphone demande au PC le chemin tel
//! quel, `base` est `depot/<clé>`). Quatre onglets, entre lesquels on passe sans empiler :
//! - **État** (`depot/<clé>`) : opération en cours (fusion, rebase…) à poursuivre ou abandonner, branche et suivi,
//!   fetch, pull, push, et les modifications (conflits, indexé, non indexé ; les 30 premières, puis « Tout voir ») ;
//! - **Historique** (`historique`, `historique/tout` : `historique.rs`) : celui de la branche courante, ou le graphe
//!   de DragonTree, avec le graphe des commits ;
//! - **Branches** (`branches`, et le détail d'une branche `branche/<b>` : `branches.rs`) : la courante, les locales
//!   par date et par dossier, les distantes par remote ;
//! - **Plus** (`plus`) : remises (appliquer, appliquer et retirer, jeter), étiquettes (les plus récentes ; toutes dans
//!   `etiquettes`, par pages), worktrees (supprimer, élaguer), remotes et leurs URL, afficher sur le PC.
//!
//! Les détails s'empilent : `modifs` (toutes les modifications ; indexer, désindexer, annuler, valider ou corriger le
//! dernier commit, mettre de côté), `fichier/<f>` (le diff d'un fichier), `commit/<id>` (message, fichiers,
//! cherry-pick, revert, branche ou étiquette ici, reset, sortir ce commit), `commit/<id>/<f>` (le diff d'un fichier du
//! commit).
//!
//! Ce module ne fait qu'écrire les écrans à partir de ce que DragonTree sait du dépôt ([`RepoView`]) et traduire une
//! action en opération de DragonTree ([`RemoteOp`]) ; `mobile.rs` la lui confie. Un fichier est désigné par une
//! empreinte de son chemin, un commit par son identifiant, une branche ou une étiquette par son nom (git interdit `:`
//! dans un nom de référence). Les listes longues sont coupées : un écran reste petit, même sur un dépôt de 2 000
//! étiquettes.

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::Arc;

use dragontree_app::repo::RemoteOp;
use dragontree_git::{
    ChangeKind, Commit, CommitInfo, FileChange, FileDiff, FileState, LineKind, Oid, OpState, RefsSnapshot, Remote,
    ResetMode, StashEntry, StatusEntry, StatusSnapshot, TagRef, Worktree,
};
use suite_sdk::mobile::ui::{Act, Band, DIFF_ADDED, DIFF_CONTEXT, DIFF_HEADER, DIFF_REMOVED, Row, Screen, Tab, Tone};

use crate::branches::{Pending, branch_key, publish_remote};
use crate::historique::LogView;

/// Une liste par pages (étiquettes, historique, branches) : sa première page, et ce que « plus » ajoute.
pub const PAGE: usize = 100;
/// … et jusqu'où « plus » va.
pub const PAGE_MAX: usize = 2000;
/// Modifications montrées dans l'onglet État (toutes : `modifs`).
const CHANGES_INLINE: usize = 30;
/// Fichiers montrés par section (modifications, fichiers d'un commit) : au-delà, sur le PC.
const FILES_MAX: usize = 300;
/// Étiquettes montrées dans l'onglet Plus (toutes : `etiquettes`).
const TAGS_INLINE: usize = 10;
/// Un diff montré est coupé au-delà.
const DIFF_LINES: usize = 600;

/// Empreinte courte d'un chemin de fichier : ce qui le désigne dans un chemin d'écran ou une action.
pub fn file_key(path: &str) -> String {
    let mut h = DefaultHasher::new();
    path.hash(&mut h);
    format!("{:x}", h.finish() & 0xffff_ffff)
}

/// Une URL de remote sans ce qui identifie (`https://jeton@hôte/…` → `https://hôte/…`) : le téléphone ne voit jamais
/// d'identifiants.
pub fn safe_url(url: &str) -> String {
    if let Some((scheme, rest)) = url.split_once("://") {
        let host_end = rest.find('/').unwrap_or(rest.len());
        if let Some(at) = rest[..host_end].rfind('@') {
            return format!("{scheme}://{}", &rest[at + 1..]);
        }
    }
    url.to_string()
}

/// Ce qu'on sait d'un dépôt : ce que DragonTree en a lu (partagé avec lui, sans copie), et ce qui a été lu à part pour
/// l'écran (diff, commit).
#[derive(Clone, Debug, Default)]
pub struct RepoView {
    pub status: Option<Result<StatusSnapshot, String>>,
    pub refs: Option<Arc<RefsSnapshot>>,
    pub stashes: Arc<Vec<StashEntry>>,
    pub worktrees: Arc<Vec<Worktree>>,
    pub remotes: Arc<Vec<Remote>>,
    pub op: Option<OpState>,
    pub last_message: String,
    /// Le dernier fetch a été refusé par le serveur (authentification) : ce qu'a dit git.
    pub auth_error: Option<String>,
    /// DragonTree n'a toujours pas lu l'état ou les branches du dépôt, sans erreur, 15 s après que le téléphone les
    /// attend (ce que le téléphone lit lui-même n'y entre pas).
    pub slow: bool,
    /// La dernière lecture de l'état par DragonTree a échoué : pourquoi (`status` garde la lecture d'avant, s'il y en
    /// a une).
    pub status_error: Option<String>,
    /// La dernière lecture des branches par DragonTree a échoué : pourquoi (`refs` garde la lecture d'avant, s'il y en
    /// a une).
    pub refs_error: Option<String>,
    /// L'historique montré (`None` : pas encore lu).
    pub log: Option<Result<LogView, String>>,
    /// Les commits pointés par les branches et les étiquettes (date, titre), lus à part.
    pub tips: Arc<HashMap<Oid, Commit>>,
    /// Ce qu'une branche a à pousser et à tirer (lu à part).
    pub pending: Option<Pending>,
    /// La branche distante (`origin/x`) dont le téléphone a demandé la suppression sur le serveur, tant qu'elle tourne.
    pub deleting: Option<String>,
    /// Lignes demandées de la liste par pages de l'écran.
    pub count: usize,
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
        let names = || self.remotes.iter().map(|r| &r.name);
        let from_upstream =
            upstream.and_then(|u| names().filter(|r| u.starts_with(&format!("{r}/"))).max_by_key(|r| r.len()).cloned());
        from_upstream.or_else(|| names().find(|r| *r == "origin").cloned()).or(names().next().cloned())
    }

    /// Les étiquettes, les plus récentes d'abord (date de leur commit, une fois lue ; sinon l'ordre inverse des noms).
    pub fn tags_by_date(&self) -> Vec<&TagRef> {
        let Some(refs) = &self.refs else { return Vec::new() };
        let mut tags: Vec<&TagRef> = refs.tags.iter().rev().collect();
        tags.sort_by_key(|t| std::cmp::Reverse(self.tips.get(&t.oid).map_or(i64::MIN, |c| c.committer_time)));
        tags
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

pub fn ago(t: i64, now: i64) -> String {
    match now.saturating_sub(t) {
        s if s < 60 => "à l'instant".into(),
        s if s < 3600 => format!("il y a {} min", s / 60),
        s if s < 86_400 => format!("il y a {} h", s / 3600),
        s => format!("il y a {} j", s / 86_400),
    }
}

pub fn sync(ahead: u32, behind: u32) -> String {
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

/// Les lectures ratées de DragonTree : `missing`, celles dont rien n'a été lu avant (l'écran les attendrait en vain) ;
/// sinon, celles dont l'écran montre la lecture d'avant.
fn failed_reads(v: &RepoView, missing: bool) -> Vec<String> {
    let (again, hint) = if missing {
        ("", "« Actualiser » relance la lecture")
    } else {
        ("re", "ce qui est montré date de sa lecture d'avant")
    };
    let mut out = Vec::new();
    if let Some(e) = v.status_error.as_ref().filter(|_| v.status.is_none() == missing) {
        out.push(format!("DragonTree n'a pas pu {again}lire l'état du dépôt ({hint}) : {e}"));
    }
    if let Some(e) = v.refs_error.as_ref().filter(|_| v.refs.is_none() == missing) {
        out.push(format!("DragonTree n'a pas pu {again}lire les branches ({hint}) : {e}"));
    }
    out
}

/// Les lectures ratées de DragonTree dont l'écran montre la lecture d'avant : en tête de chaque écran du dépôt.
pub fn stale_reads(v: &RepoView) -> Vec<String> {
    failed_reads(v, false)
}

/// Ce qui n'est pas encore lu : pourquoi, si DragonTree n'a pas pu ouvrir le dépôt ou le lire (« Actualiser » relance
/// la lecture) ; sinon `text`, et un mot de plus s'il le lit depuis longtemps sans erreur.
pub fn reading(s: &mut Screen, text: &str, v: &RepoView) {
    if let Some(Err(e)) = &v.status {
        s.note(e, Tone::Danger);
        return;
    }
    let failed = failed_reads(v, true);
    if !failed.is_empty() {
        for e in failed {
            s.note(e, Tone::Danger);
        }
        return;
    }
    s.empty(text);
    if v.slow {
        s.note(
            "DragonTree n'a pas encore lu ce dépôt : il est peut-être très gros (voir DragonTree sur le PC). \
             « Actualiser » le fait relire.",
            Tone::Warn,
        );
    }
}

/// La ligne qui allonge une liste par pages : `shown` lignes montrées sur `total` ; rien si tout est montré. Au-delà de
/// [`PAGE_MAX`], la suite est sur le PC.
pub fn more_row(shown: usize, total: usize, what: &str) -> Option<Row> {
    if shown >= total {
        return None;
    }
    let left = total - shown;
    if shown >= PAGE_MAX {
        return Some(Row::new(format!("… et {left} {what} de plus : voir sur le PC")).glyph("🖥️").key("plus"));
    }
    let next = left.min(PAGE).min(PAGE_MAX - shown);
    Some(
        Row::new(format!("{next} {what} de plus"))
            .subtitle(format!("{shown} sur {total}"))
            .glyph("⬇️")
            .key("plus")
            .tap(Act::new("plus", "Afficher la suite")),
    )
}

/// Les quatre onglets d'un dépôt ; `history` : le chemin de l'historique montré (celui de la branche courante, ou de
/// toutes). L'onglet État compte les modifications.
pub fn with_tabs(mut s: Screen, base: &str, history: &str, v: &RepoView) -> Screen {
    let changes = v.status().map_or(0, |st| st.entries.len());
    let tab = |label: &str, path: String, badge: String| Tab { label: label.into(), path, badge };
    s.tabs = vec![
        tab("État", base.into(), if changes > 0 { changes.to_string() } else { String::new() }),
        tab("Historique", history.into(), String::new()),
        tab("Branches", format!("{base}/branches"), String::new()),
        tab("Plus", format!("{base}/plus"), String::new()),
    ];
    s
}

/// Le bandeau d'une opération en cours, et ses boutons (comme celui de DragonTree : continuer une fois les conflits
/// résolus).
fn op_block(s: &mut Screen, base: &str, v: &RepoView) {
    let Some(op) = v.op.filter(|o| *o != OpState::None) else { return };
    let conflicts = v.status().map_or(0, |st| st.conflicted().len());
    let text = if conflicts > 0 {
        format!(
            "{} en cours : {conflicts} conflit(s) à résoudre (ci-dessous, ou dans DragonTree sur le PC), puis continuer.",
            op_name(&op)
        )
    } else {
        format!("{} en cours : continuer ou abandonner.", op_name(&op))
    };
    s.note(text, Tone::Warn);
    let mut acts = Vec::new();
    if conflicts > 0 {
        s.row(
            Row::new("Voir les conflits")
                .detail(conflicts.to_string())
                .glyph("⚠️")
                .tone(Tone::Danger)
                .open(format!("{base}/modifs")),
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

fn commit_act() -> Act {
    Act::new("valider", "Valider…")
        .glyph("✅")
        .primary()
        .input("Message du commit", "Ce que fait ce commit")
        .multiline()
}

/// Ce qu'on fait des modifications (barre du haut) : valider, corriger le dernier commit, tout indexer ou désindexer,
/// mettre de côté, tout annuler.
fn change_acts(v: &RepoView, st: &StatusSnapshot) -> Vec<Act> {
    let (staged, unstaged) = (st.staged(), st.unstaged());
    let mut acts = Vec::new();
    if !staged.is_empty() {
        acts.push(commit_act());
    }
    if !v.last_message.is_empty() {
        acts.push(
            Act::new("corriger", "Corriger le dernier commit…")
                .glyph("✏️")
                .input("Message du commit corrigé", "")
                .input_value(v.last_message.trim())
                .multiline(),
        );
    }
    if !unstaged.is_empty() {
        acts.push(Act::new("tout-indexer", "Tout indexer").glyph("➕"));
    }
    if !staged.is_empty() {
        acts.push(Act::new("tout-desindexer", "Tout désindexer").glyph("➖"));
    }
    if !st.entries.is_empty() {
        acts.push(Act::new("mettre-de-cote", "Mettre de côté (stash)…").glyph("📦").input("Message (facultatif)", ""));
        acts.push(
            Act::new("tout-annuler", "Tout annuler")
                .glyph("↩️")
                .danger()
                .confirm("Annuler toutes les modifications ? Elles seront perdues, fichiers non suivis compris."),
        );
    }
    acts
}

/// Le bouton du bas : valider ce qui est indexé, sinon tout indexer.
fn commit_buttons(s: &mut Screen, st: &StatusSnapshot) {
    if !st.staged().is_empty() {
        s.buttons(vec![commit_act()]);
    } else if !st.unstaged().is_empty() {
        s.buttons(vec![Act::new("tout-indexer", "Tout indexer").primary()]);
    }
}

/// Fetch, pull et push selon le suivi ; le push forcé n'est jamais un bouton (menu de l'écran, avec confirmation).
fn net_buttons(st: &StatusSnapshot) -> Vec<Act> {
    let mut acts = vec![Act::new("fetch", "Fetch").glyph("⬇️")];
    let diverged = st.ahead > 0 && st.behind > 0;
    if st.upstream.is_some() && diverged {
        acts.push(
            Act::new("pull-fusion", "Pull (fusion)")
                .confirm("Fusionner la branche distante dans la vôtre (un commit de fusion) ?"),
        );
        acts.push(Act::new("pull-rebase", "Pull (rebase)").confirm("Rejouer vos commits sur la branche distante ?"));
    } else if st.upstream.is_some() {
        acts.push(Act::new("pull", "Pull"));
        acts.push(Act::new("push", "Push").primary().confirm("Pousser la branche vers sa branche distante ?"));
    } else if let Some(b) = &st.branch {
        acts.push(Act::new(format!("publier:{b}"), "Publier la branche").primary());
    }
    acts
}

/// L'onglet État d'un dépôt.
pub fn overview(base: &str, name: &str, path: &str, v: &RepoView, busy: bool) -> Screen {
    let mut s = with_tabs(Screen::new(name).subtitle(path), base, &format!("{base}/historique"), v);
    s.acts.push(Act::new("actualiser", "Actualiser").glyph("🔄"));
    s.acts.push(Act::new("pc", "Afficher dans DragonTree sur le PC").glyph("🖥️"));
    let st = match &v.status {
        None => {
            reading(&mut s, "Lecture du dépôt…", v);
            return s;
        }
        Some(Err(e)) => {
            s.note(e, Tone::Danger);
            return s;
        }
        Some(Ok(st)) => st,
    };
    if let Some(e) = &v.auth_error {
        s.note(format!("Le dernier fetch a été refusé par le serveur : {e}"), Tone::Warn);
    }
    if !busy {
        s.acts.extend(change_acts(v, st));
        if let Some(u) = st.upstream.as_ref().filter(|_| st.ahead > 0 && st.behind > 0) {
            s.acts.push(Act::new("push-force", "Push forcé…").glyph("⚠️").danger().confirm(format!(
                "Écraser {u} par votre branche ? Ses {} commit(s) que vous n'avez pas seront perdus (refusé si elle a \
                 bougé depuis le dernier fetch).",
                st.behind
            )));
        }
    }
    op_block(&mut s, base, v);
    match &st.branch {
        Some(b) => s.row(
            Row::new("Branche")
                .detail(b)
                .glyph("🌿")
                .key("branche")
                .open(format!("{base}/branche/{}", branch_key(&format!("refs/heads/{b}")))),
        ),
        None => s.row(Row::new("Branche").detail("HEAD détachée").glyph("📍").key("branche")),
    };
    let upstream = st.upstream.clone().unwrap_or_else(|| "aucune".into());
    let tone = if st.ahead > 0 && st.behind > 0 { Tone::Warn } else { Tone::Ok };
    s.row(Row::new("Suivi").subtitle(upstream).detail(sync(st.ahead, st.behind)).glyph("🔁").tone(tone));
    if !busy {
        s.buttons(net_buttons(st));
    }
    if st.entries.is_empty() {
        s.band(Band::new("", "Modifications"));
        s.empty("Rien à valider : le dossier de travail est propre.");
        return s;
    }
    let mut left = CHANGES_INLINE;
    for (key, title, files, staged, tone) in [
        ("conflits", "Conflits", st.conflicted(), false, Some(Tone::Danger)),
        ("indexe", "Indexé", st.staged(), true, None),
        ("non-indexe", "Non indexé", st.unstaged(), false, None),
    ] {
        if files.is_empty() || left == 0 {
            continue;
        }
        let mut band = Band::new(key, title).aside(files.len().to_string());
        if let Some(t) = tone {
            band = band.tone(t);
        }
        s.band(band);
        for e in files.iter().take(left) {
            s.row(quiet(file_row(base, v, e, staged), busy));
        }
        left = left.saturating_sub(files.len());
    }
    if st.entries.len() > CHANGES_INLINE {
        s.row(
            Row::new(format!("Tout voir ({} fichiers)", st.entries.len()))
                .glyph("📋")
                .key("tout-voir")
                .open(format!("{base}/modifs")),
        );
    }
    if !busy {
        commit_buttons(&mut s, st);
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

fn file_row(base: &str, v: &RepoView, e: &StatusEntry, staged: bool) -> Row {
    let key = file_key(&e.path);
    // Un fichier en partie indexé est dans les deux listes : sa ligne a une clé par liste.
    let mut row = Row::new(&e.path)
        .key(format!("{}:{key}", if staged { "i" } else { "w" }))
        .glyph(letter(e, staged).to_string())
        .open(format!("{base}/fichier/{key}"));
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

/// Toutes les modifications : conflits, indexé, non indexé, et ce qu'on peut en faire.
pub fn changes(base: &str, v: &RepoView, busy: bool) -> Screen {
    let mut s = Screen::new("Modifications").search("Chercher un fichier");
    let Some(st) = v.status() else {
        reading(&mut s, "Lecture du dépôt…", v);
        return s;
    };
    let conflicts = st.conflicted();
    let staged = st.staged();
    let unstaged = st.unstaged();
    s.subtitle = format!("{} indexé(s), {} non indexé(s)", staged.len(), unstaged.len());
    if !busy {
        s.acts = change_acts(v, st);
    }
    if st.entries.is_empty() {
        s.empty("Rien à valider : le dossier de travail est propre.");
        return s;
    }
    for (key, title, files, is_staged) in [
        ("conflits", "Conflits", conflicts, false),
        ("indexe", "Indexé", staged, true),
        ("non-indexe", "Non indexé", unstaged, false),
    ] {
        if files.is_empty() {
            continue;
        }
        s.band(Band::new(key, title).aside(files.len().to_string()));
        for e in files.iter().take(FILES_MAX) {
            s.row(quiet(file_row(base, v, e, is_staged), busy));
        }
        if files.len() > FILES_MAX {
            s.note(format!("… et {} autres : voir sur le PC.", files.len() - FILES_MAX), Tone::Info);
        }
    }
    if !busy {
        commit_buttons(&mut s, st);
    }
    s
}

/// Le diff d'un fichier du dossier de travail (indexé et non indexé) ; `None` : il n'est plus dans les modifications.
pub fn file(v: &RepoView, key: &str, busy: bool) -> Option<Screen> {
    if v.status().is_none() {
        let mut s = Screen::new("Fichier");
        reading(&mut s, "Lecture du dépôt…", v);
        return Some(s);
    }
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

/// Un commit : son message, ses fichiers, et ce qu'on peut en faire.
pub fn commit(base: &str, v: &RepoView, id: &str, now: i64, busy: bool) -> Screen {
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
            for f in files.iter().take(FILES_MAX) {
                let key = file_key(&f.path);
                let mut row = Row::new(&f.path)
                    .glyph(change_letter(f).to_string())
                    .key(&key)
                    .open(format!("{base}/commit/{id}/{key}"));
                if let Some(old) = &f.old_path {
                    row = row.subtitle(format!("depuis {old}"));
                }
                s.row(row);
            }
            if files.len() > FILES_MAX {
                s.note(format!("… et {} autres : voir sur le PC.", files.len() - FILES_MAX), Tone::Info);
            }
        }
    }
    s
}

/// Le diff d'un fichier d'un commit ; `None` : le commit lu n'a pas ce fichier.
pub fn commit_file(v: &RepoView, id: &str, key: &str) -> Option<Screen> {
    let files = match v.commits.get(id) {
        Some(Ok((_, files))) => files,
        Some(Err(e)) => {
            let mut s = Screen::new("Fichier");
            s.note(e, Tone::Danger);
            return Some(s);
        }
        None => {
            let mut s = Screen::new("Fichier");
            s.empty("Lecture du commit…");
            return Some(s);
        }
    };
    let f = files.iter().find(|f| file_key(&f.path) == key)?;
    let mut s = Screen::new(f.path.rsplit('/').next().unwrap_or(&f.path)).subtitle(&f.path);
    match v.diffs.get(&format!("c:{id}:{key}")) {
        None => s.empty("Lecture du diff…"),
        Some(Err(e)) => s.note(e, Tone::Danger),
        Some(Ok(lines)) => s.diff(lines.clone()),
    };
    Some(s)
}

fn stash_row(st: &StashEntry, now: i64, busy: bool) -> Row {
    let id = st.oid.to_hex();
    let mut row = Row::new(&st.message).subtitle(ago(st.time, now)).glyph("📦").key(format!("s:{id}"));
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
    row
}

fn tag_row(base: &str, t: &TagRef, v: &RepoView, now: i64, busy: bool) -> Row {
    let mut row = Row::new(&t.name)
        .detail(t.oid.short())
        .glyph("🏷️")
        .key(format!("t:{}", t.name))
        .open(format!("{base}/commit/{}", t.oid.to_hex()));
    let mut sub = Vec::new();
    if t.annotated {
        sub.push("annotée".to_string());
    }
    if let Some(c) = v.tips.get(&t.oid) {
        sub.push(ago(c.committer_time, now));
    }
    row = row.subtitle(sub.join(" · "));
    if !busy {
        row = row.act(Act::new(format!("pousser-etiquette:{}", t.name), "Pousser").glyph("⬆️")).act(
            Act::new(format!("supprimer-etiquette:{}", t.name), "Supprimer (en local)")
                .glyph("🗑️")
                .danger()
                .confirm(format!("Supprimer l'étiquette {} de ce dépôt ?", t.name)),
        );
    }
    row
}

fn worktree_row(w: &Worktree, busy: bool) -> Row {
    let name = w.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let mut row = Row::new(name)
        .subtitle(w.branch.clone().unwrap_or_else(|| "HEAD détachée".into()))
        .detail(if w.is_main { "principal" } else { "" })
        .glyph("🌳")
        .key(format!("w:{}", file_key(&w.path.to_string_lossy())));
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
    row
}

/// L'onglet Plus : remises, étiquettes récentes, worktrees, remotes, et DragonTree sur le PC.
pub fn more(base: &str, v: &RepoView, now: i64, busy: bool) -> Screen {
    let mut s = with_tabs(Screen::new("Plus"), base, &format!("{base}/historique"), v);
    s.acts.push(Act::new("pc", "Afficher dans DragonTree sur le PC").glyph("🖥️"));
    if !busy {
        s.acts.push(Act::new("mettre-de-cote", "Mettre de côté…").glyph("📦").input("Message (facultatif)", ""));
        s.acts.push(Act::new("nouvelle-etiquette", "Nouvelle étiquette sur HEAD…").glyph("🏷️").input("Nom", "v1.2.0"));
        if v.worktrees.iter().any(|w| w.prunable) {
            s.acts.push(Act::new("elaguer", "Élaguer les worktrees disparus").glyph("🧹"));
        }
    }
    // Les remises, worktrees et remotes sont lus avec le dépôt : rien à en dire avant.
    if !matches!(v.status, Some(Ok(_))) {
        reading(&mut s, "Lecture du dépôt…", v);
        return s;
    }
    s.band(Band::new("remises", "Remises (stash)").glyph("📦").aside(v.stashes.len().to_string()));
    if v.stashes.is_empty() {
        s.empty("Aucune remise.");
    }
    for st in v.stashes.iter() {
        s.row(stash_row(st, now, busy));
    }
    let tags = v.tags_by_date();
    s.band(Band::new("etiquettes", "Étiquettes").glyph("🏷️").aside(tags.len().to_string()));
    if v.refs.is_none() {
        reading(&mut s, "Lecture des étiquettes…", v);
    } else if tags.is_empty() {
        s.empty("Aucune étiquette.");
    }
    for t in tags.iter().take(TAGS_INLINE) {
        s.row(tag_row(base, t, v, now, busy));
    }
    if tags.len() > TAGS_INLINE {
        s.row(
            Row::new(format!("Toutes les étiquettes ({})", tags.len()))
                .glyph("🏷️")
                .key("toutes-etiquettes")
                .open(format!("{base}/etiquettes")),
        );
    }
    if v.worktrees.len() > 1 {
        s.band(Band::new("worktrees", "Worktrees").glyph("🌳").aside(v.worktrees.len().to_string()));
        for w in v.worktrees.iter() {
            s.row(worktree_row(w, busy));
        }
    }
    s.band(Band::new("remotes", "Remotes").glyph("☁️").aside(v.remotes.len().to_string()));
    if v.remotes.is_empty() {
        s.empty("Aucun remote : ce dépôt n'est publié nulle part.");
    }
    for r in v.remotes.iter() {
        let mut row = Row::new(&r.name).subtitle(safe_url(&r.fetch_url)).glyph("☁️").key(format!("r:{}", r.name));
        if !r.push_url.is_empty() && r.push_url != r.fetch_url {
            row = row.detail("push à part").subtitle(format!(
                "{} · push : {}",
                safe_url(&r.fetch_url),
                safe_url(&r.push_url)
            ));
        }
        s.row(row);
    }
    s.band(Band::new("", "DragonTree"));
    s.buttons(vec![Act::new("pc", "Afficher dans DragonTree sur le PC").glyph("🖥️")]);
    s
}

/// Toutes les étiquettes, les plus récentes d'abord, par pages de [`PAGE`].
pub fn tags(base: &str, v: &RepoView, now: i64, busy: bool) -> Screen {
    let mut s = Screen::new("Étiquettes").search("Chercher une étiquette");
    if !busy {
        s.acts.push(Act::new("nouvelle-etiquette", "Nouvelle étiquette sur HEAD…").glyph("🏷️").input("Nom", "v1.2.0"));
    }
    if v.refs.is_none() {
        reading(&mut s, "Lecture des étiquettes…", v);
        return s;
    }
    let list = v.tags_by_date();
    s.subtitle = format!("{} étiquette(s), les plus récentes d'abord", list.len());
    if list.is_empty() {
        s.empty("Aucune étiquette.");
    }
    let shown = v.count.max(PAGE).min(list.len());
    for t in &list[..shown] {
        s.row(tag_row(base, t, v, now, busy));
    }
    if let Some(row) = more_row(shown, list.len(), "étiquettes") {
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
        "supprimer-distante" => {
            // Le remote et le nom tels que DragonTree les a lus (un nom de remote peut contenir un `/`).
            let remotes = v.refs.as_deref().map(|r| r.remotes.as_slice()).unwrap_or_default();
            let r = remotes
                .iter()
                .find(|r| r.full == arg && !r.is_symbolic)
                .ok_or_else(|| format!("{arg} : branche distante introuvable, ou pas encore relue par DragonTree."))?;
            let op = RemoteOp::DeleteRemoteBranch { remote: r.remote.clone(), branch: r.name.clone() };
            (op, format!("Supprimer {arg} sur le serveur"))
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
            // Vers sa branche suivie (sur son remote à elle) ou, sans suivi, sur le remote par défaut.
            let remote = match v.refs.as_deref().map(|r| (r, r.local(&arg))) {
                Some((refs, Some(l))) => publish_remote(refs, l, v.default_remote())?,
                // Branches pas encore lues par DragonTree : seulement la courante, sans suivi d'après son état.
                _ if st.is_some_and(|s| s.branch.as_deref() == Some(arg.as_str()) && s.upstream.is_none()) => {
                    v.default_remote().ok_or("Aucun remote : ajoutez-en un sur le PC.")?
                }
                _ => return Err(format!("{arg} : branche introuvable, ou pas encore relue par DragonTree.")),
            };
            let label = format!("Pousser {arg} vers {remote}/{arg}");
            (RemoteOp::PushBranch { name: arg.clone(), remote }, label)
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

    fn oid_of(c: char) -> Oid {
        Oid::from_hex(&c.to_string().repeat(40)).unwrap()
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
        let refs = RefsSnapshot {
            head: HeadState::Branch("main".into()),
            head_oid: Some(oid_of('a')),
            locals: vec![
                LocalBranch {
                    name: "main".into(),
                    oid: oid_of('a'),
                    upstream: Some("origin/main".into()),
                    ahead: 1,
                    behind: 0,
                    upstream_gone: false,
                    is_head: true,
                },
                LocalBranch {
                    name: "feature/x".into(),
                    oid: oid_of('b'),
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
                oid: oid_of('c'),
                is_symbolic: false,
            }],
            tags: Vec::new(),
            labels: Default::default(),
        };
        let origin = Remote {
            name: "origin".into(),
            fetch_url: "https://jeton@exemple.org/depot.git".into(),
            push_url: "https://jeton@exemple.org/depot.git".into(),
        };
        RepoView {
            status: Some(Ok(status)),
            refs: Some(Arc::new(refs)),
            remotes: Arc::new(vec![origin]),
            op: Some(OpState::None),
            last_message: "Premier jet".into(),
            ..Default::default()
        }
    }

    fn titles(s: &Screen) -> Vec<&str> {
        s.blocks
            .iter()
            .filter_map(|b| match b {
                Block::Section { title, .. } => Some(title.as_str()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn changes_are_split_and_each_file_has_its_gestures() {
        let v = view();
        let s = changes("depot/k", &v, false);
        assert_eq!(titles(&s), ["Conflits", "Indexé", "Non indexé"]);
        let ids = s.action_ids();
        let a = file_key("src/a.rs");
        let b = file_key("src/b.rs");
        assert!(ids.contains(&format!("desindexer:{a}")) && ids.contains(&format!("indexer:{b}")));
        assert!(ids.contains(&format!("annuler:{b}")) && ids.contains("valider") && ids.contains("corriger"));
        assert!(changes("depot/k", &v, true).action_ids().is_empty(), "rien pendant une opération");
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
    fn an_operation_in_progress_can_be_continued_or_aborted() {
        let mut v = view();
        v.op = Some(OpState::Rebase { interactive: false });
        let s = overview("depot/k", "ozmo", r"C:\dev\ozmo", &v, false);
        let ids = s.action_ids();
        // Un conflit reste : comme dans DragonTree, pas de « Continuer » avant sa résolution.
        assert!(!ids.contains("continuer") && ids.contains("abandonner") && ids.contains("passer"));
        assert!(s.opens().contains("depot/k/modifs"), "les conflits à voir, par un chemin absolu");
        assert!(s.blocks.iter().any(|b| matches!(b, Block::Note { text, .. } if text.contains("1 conflit"))));
        assert!(ids.contains("push"), "en avance seulement : push simple");
        if let Some(Ok(st)) = &mut v.status {
            st.entries.retain(|e| e.conflict.is_none());
        }
        assert!(overview("depot/k", "ozmo", "", &v, false).action_ids().contains("continuer"), "résolu : continuer");
    }

    /// L'onglet État : quatre onglets aux chemins absolus, les modifications sur place (30 au plus, puis « Tout
    /// voir »), et le push forcé seulement dans le menu de l'écran, avec une question qui dit ce qui sera perdu.
    #[test]
    fn the_state_tab_shows_changes_in_place_and_keeps_force_push_in_the_menu() {
        let mut v = view();
        let s = overview("depot/k", "ozmo", "", &v, false);
        let tabs: Vec<(&str, &str)> = s.tabs.iter().map(|t| (t.label.as_str(), t.path.as_str())).collect();
        assert_eq!(
            tabs,
            [
                ("État", "depot/k"),
                ("Historique", "depot/k/historique"),
                ("Branches", "depot/k/branches"),
                ("Plus", "depot/k/plus")
            ]
        );
        assert_eq!(s.tabs[0].badge, "4");
        assert_eq!(titles(&s), ["Conflits", "Indexé", "Non indexé"]);
        assert!(!s.opens().contains("depot/k/modifs"), "tout tient : pas de « Tout voir »");
        if let Some(Ok(st)) = &mut v.status {
            st.ahead = 2;
            st.behind = 3;
            for i in 0..40 {
                st.entries.push(entry(&format!("f{i}.txt"), FileState::Untracked, FileState::Untracked));
            }
        }
        let s = overview("depot/k", "ozmo", "", &v, false);
        let files = s
            .blocks
            .iter()
            .filter(|b| matches!(b, Block::Row(r) if r.open.as_deref().is_some_and(|o| o.contains("/fichier/"))))
            .count();
        assert_eq!(files, CHANGES_INLINE);
        assert!(s.opens().contains("depot/k/modifs"), "Tout voir");
        let buttons: Vec<&str> = s
            .blocks
            .iter()
            .filter_map(|b| match b {
                Block::Buttons { acts } => Some(acts.iter().map(|a| a.id.as_str())),
                _ => None,
            })
            .flatten()
            .collect();
        assert!(!buttons.contains(&"push-force"), "jamais un bouton : {buttons:?}");
        let force = s.acts.iter().find(|a| a.id == "push-force").expect("dans le menu");
        assert!(force.confirm.as_deref().is_some_and(|q| q.contains("3 commit(s)")), "{force:?}");
    }

    /// L'onglet Plus : remises, étiquettes récentes (puis toutes, par pages), remotes sans identifiants.
    #[test]
    fn the_more_tab_lists_stashes_tags_and_remotes_without_credentials() {
        let mut v = view();
        let mut refs = (**v.refs.as_ref().unwrap()).clone();
        refs.tags = (0..250).map(|i| TagRef { name: format!("v{i}"), oid: oid_of('d'), annotated: false }).collect();
        v.refs = Some(Arc::new(refs));
        let s = more("depot/k", &v, 0, false);
        assert_eq!(titles(&s), ["Remises (stash)", "Étiquettes", "Remotes", "DragonTree"]);
        assert!(s.opens().contains("depot/k/etiquettes"));
        let text = format!("{s:?}");
        assert!(text.contains("https://exemple.org/depot.git") && !text.contains("jeton"), "{text}");
        let all = tags("depot/k", &v, 0, false);
        let rows = all.blocks.iter().filter(|b| matches!(b, Block::Row(_))).count();
        assert_eq!(rows, PAGE + 1, "une page, et « plus »");
        assert!(all.action_ids().contains("plus"));
        v.count = 2 * PAGE;
        let rows = tags("depot/k", &v, 0, false).blocks.iter().filter(|b| matches!(b, Block::Row(_))).count();
        assert_eq!(rows, 2 * PAGE + 1);
        assert_eq!(safe_url("git@exemple.org:a/b.git"), "git@exemple.org:a/b.git");
        assert_eq!(safe_url("https://moi:secret@exemple.org/a"), "https://exemple.org/a");
    }

    /// Ce qui n'est pas encore lu le dit ; une longue attente de DragonTree le dit aussi (peut-être une lecture ratée),
    /// jamais avant ; un dépôt que DragonTree n'a pas pu ouvrir dit pourquoi.
    #[test]
    fn a_long_wait_for_dragontree_is_said_only_when_it_is_long() {
        let tones = |v: &RepoView| {
            let mut s = Screen::new("Historique");
            reading(&mut s, "Lecture…", v);
            let notes = s.blocks.iter().filter_map(|b| match b {
                Block::Note { tone, .. } => Some(*tone),
                _ => None,
            });
            notes.collect::<Vec<Tone>>()
        };
        let mut v = RepoView::default();
        assert!(tones(&v).is_empty());
        v.slow = true;
        assert_eq!(tones(&v), [Tone::Warn]);
        v.status = Some(Err("pas un dépôt git".into()));
        assert_eq!(tones(&v), [Tone::Danger]);
    }

    /// Une lecture ratée de DragonTree se dit : à la place de « Lecture… » si rien n'avait été lu avant, sinon en tête
    /// de l'écran, qui montre la lecture d'avant. Chaque erreur une seule fois.
    #[test]
    fn a_failed_read_of_dragontree_says_why() {
        let notes = |v: &RepoView| {
            let mut s = Screen::new("Branches");
            reading(&mut s, "Lecture des branches…", v);
            let text = format!("{s:?}");
            let notes = s.blocks.into_iter().filter_map(|b| match b {
                Block::Note { text, tone, .. } => Some((tone, text)),
                _ => None,
            });
            (notes.collect::<Vec<(Tone, String)>>(), text.contains("Lecture des branches…"))
        };
        let mut v = RepoView {
            refs_error: Some("HEAD illisible".into()),
            status_error: Some("index corrompu".into()),
            ..Default::default()
        };
        let (shown, waiting) = notes(&v);
        assert!(!waiting, "pas de « Lecture… » sans fin : {shown:?}");
        assert_eq!(shown.iter().map(|(t, _)| *t).collect::<Vec<_>>(), [Tone::Danger, Tone::Danger]);
        assert!(shown[0].1.contains("l'état du dépôt") && shown[0].1.ends_with("index corrompu"), "{shown:?}");
        assert!(shown[1].1.contains("les branches") && shown[1].1.contains("« Actualiser »"), "{shown:?}");
        assert!(stale_reads(&v).is_empty(), "rien de lu avant : rien en tête");
        // Les branches lues avant : leur erreur passe en tête, l'écran les montre.
        v.refs = view().refs;
        let (shown, _) = notes(&v);
        assert_eq!(shown.len(), 1, "{shown:?}");
        let stale = stale_reads(&v);
        assert_eq!(stale.len(), 1);
        assert!(stale[0].contains("relire les branches") && stale[0].contains("lecture d'avant"), "{stale:?}");
    }

    #[test]
    fn a_conflict_is_resolved_by_keeping_one_side() {
        let mut v = view();
        v.op = Some(OpState::Merge);
        let c = file_key("conflit.rs");
        let ids = changes("depot/k", &v, false).action_ids();
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
        let s = changes("depot/k", &v, false);
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
