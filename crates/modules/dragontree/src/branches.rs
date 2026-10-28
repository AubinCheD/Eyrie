//! Les branches d'un dépôt sur le téléphone, et le détail d'une branche.
//!
//! - `branches` : la courante, puis les locales par date de leur dernier commit (au-delà de 12, un bandeau par
//!   dossier, `feature/…`), puis les distantes, un bandeau plié par remote (par remote et dossier pour un gros
//!   remote) ; 100 lignes au plus par bandeau (25 pour les distantes, 400 en tout), puis « N autres… » qui ouvre le
//!   groupe entier, par pages (`branches/g/<g>`). Toucher une branche ouvre son détail : jamais de bascule d'un
//!   toucher.
//! - `branche/<b>` : sa pointe, son suivi, les commits à pousser et à tirer, son historique avec son graphe, et les
//!   gestes (basculer, fusionner dans la courante, rebaser, pousser ou publier, renommer, supprimer ; suivre en local
//!   pour une distante), tous menés par DragonTree comme ses boutons.
//!
//! Une branche est désignée par une empreinte de son nom complet (`refs/heads/x`, `refs/remotes/origin/x`) : ses `/`
//! n'entrent pas dans un chemin d'écran. Les dates des pointes sont lues à part (`commits_of`, voir `mobile.rs`) et
//! relues quand DragonTree relit les branches ; tant qu'elles manquent, l'ordre est celui des noms.

use std::collections::HashMap;

use dragontree_git::{Commit, HeadState, LocalBranch, Oid, RefsSnapshot, RemoteBranch};
use suite_sdk::mobile::ui::{Act, Band, Row, Screen, Tone};

use crate::depot::{self, PAGE, RepoView};
use crate::historique;
use crate::liste::short_hash;

/// Au-delà, les branches locales sont rangées par dossier.
const LOCALS_FLAT: usize = 12;
/// Au-delà, les branches d'un remote sont rangées par dossier.
const REMOTE_SPLIT: usize = 30;
/// Lignes d'un bandeau de branches locales, puis « N autres… ».
const BAND_LOCALS: usize = 100;
/// … d'un bandeau de branches distantes.
const BAND_REMOTES: usize = 25;
/// Branches distantes montrées en tout dans l'onglet : au-delà, leurs bandeaux ne mènent qu'à leur groupe.
const REMOTES_SHOWN: usize = 400;
/// Commits à pousser ou à tirer lus et montrés.
pub const PENDING_SHOWN: usize = 30;

/// Ce qu'une branche a à pousser vers sa branche suivie et à en tirer, ou pourquoi on ne le sait pas.
pub type Pending = Result<(Vec<Commit>, Vec<Commit>), String>;

/// L'empreinte d'une branche, de son nom complet (`refs/heads/x`) : ce qui la désigne dans un chemin d'écran.
pub fn branch_key(full: &str) -> String {
    short_hash(full)
}

/// Le dossier d'une branche : ses deux premiers niveaux (`feature/achedotal/64522` → `feature/achedotal`), vide à la
/// racine.
pub fn folder(name: &str) -> &str {
    let Some((dir, _)) = name.rsplit_once('/') else { return "" };
    match dir.match_indices('/').nth(1) {
        Some((i, _)) => &dir[..i],
        None => dir,
    }
}

/// Une branche, locale ou distante.
#[derive(Clone, Copy, Debug)]
pub enum Branch<'a> {
    Local(&'a LocalBranch),
    Remote(&'a RemoteBranch),
}

impl Branch<'_> {
    /// Son nom complet : `refs/heads/x`, `refs/remotes/origin/x`.
    pub fn full(&self) -> String {
        match self {
            Branch::Local(b) => format!("refs/heads/{}", b.name),
            Branch::Remote(r) => format!("refs/remotes/{}", r.full),
        }
    }

    /// Son nom court : `x`, `origin/x`.
    pub fn name(&self) -> &str {
        match self {
            Branch::Local(b) => &b.name,
            Branch::Remote(r) => &r.full,
        }
    }

    /// Son nom dans son remote (`x` pour `origin/x`).
    fn short(&self) -> &str {
        match self {
            Branch::Local(b) => &b.name,
            Branch::Remote(r) => &r.name,
        }
    }

    pub fn oid(&self) -> Oid {
        match self {
            Branch::Local(b) => b.oid,
            Branch::Remote(r) => r.oid,
        }
    }

    pub fn key(&self) -> String {
        branch_key(&self.full())
    }
}

/// Toutes les branches (sans `origin/HEAD`).
fn all(refs: &RefsSnapshot) -> impl Iterator<Item = Branch<'_>> {
    let remotes = refs.remotes.iter().filter(|r| !r.is_symbolic).map(Branch::Remote);
    refs.locals.iter().map(Branch::Local).chain(remotes)
}

/// La branche d'empreinte `key`.
pub fn find<'a>(refs: &'a RefsSnapshot, key: &str) -> Option<Branch<'a>> {
    all(refs).find(|b| b.key() == key)
}

/// Les commits pointés par les branches et les étiquettes : ceux dont la date est à lire.
pub fn tip_oids(refs: &RefsSnapshot) -> Vec<Oid> {
    let mut oids: Vec<Oid> = all(refs).map(|b| b.oid()).chain(refs.tags.iter().map(|t| t.oid)).collect();
    oids.sort_unstable_by_key(|o| o.to_hex());
    oids.dedup();
    oids
}

/// Le nom complet de la branche suivie par `local` (`origin/x` → `refs/remotes/origin/x`, une locale → `refs/heads/…`).
pub fn upstream_ref(refs: &RefsSnapshot, local: &LocalBranch) -> Option<String> {
    let u = local.upstream.as_deref().filter(|_| !local.upstream_gone)?;
    if refs.remotes.iter().any(|r| r.full == u) {
        Some(format!("refs/remotes/{u}"))
    } else {
        refs.local(u).map(|b| format!("refs/heads/{}", b.name))
    }
}

fn time(tips: &HashMap<Oid, Commit>, oid: Oid) -> i64 {
    tips.get(&oid).map_or(i64::MIN, |c| c.committer_time)
}

/// Un bandeau de branches : son identifiant (`l:<dossier>`, `r:<remote>:<dossier>`), son titre, ses branches, les
/// plus récentes d'abord.
pub struct Group<'a> {
    pub id: String,
    pub title: String,
    pub remote: bool,
    pub branches: Vec<Branch<'a>>,
}

/// Range `list` (les plus récentes d'abord) par dossier ; le groupe de la branche la plus récente d'abord.
fn by_folder<'a>(
    list: Vec<Branch<'a>>,
    remote: bool,
    id: impl Fn(&str) -> String,
    title: impl Fn(&str) -> String,
) -> Vec<Group<'a>> {
    let mut groups: Vec<Group> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    for b in list {
        let f = folder(b.short()).to_string();
        let i = *index.entry(f.clone()).or_insert_with(|| {
            groups.push(Group { id: id(&f), title: title(&f), remote, branches: Vec::new() });
            groups.len() - 1
        });
        groups[i].branches.push(b);
    }
    groups
}

/// Les bandeaux de l'onglet Branches, après la courante : les locales (par dossier au-delà de 12), puis les distantes
/// par remote (et par dossier pour un gros remote).
pub fn groups<'a>(refs: &'a RefsSnapshot, tips: &HashMap<Oid, Commit>) -> Vec<Group<'a>> {
    let recent_first =
        |a: &Branch, b: &Branch| time(tips, b.oid()).cmp(&time(tips, a.oid())).then(a.name().cmp(b.name()));
    let current = refs.current_branch();
    let mut locals: Vec<Branch> =
        refs.locals.iter().filter(|b| Some(b.name.as_str()) != current).map(Branch::Local).collect();
    locals.sort_by(recent_first);
    let mut out = Vec::new();
    if locals.len() > LOCALS_FLAT {
        let title = |f: &str| if f.is_empty() { "Locales, sans dossier".to_string() } else { f.to_string() };
        out.extend(by_folder(locals, false, |f| format!("l:{f}"), title));
    } else if !locals.is_empty() {
        out.push(Group { id: "l".into(), title: "Locales".into(), remote: false, branches: locals });
    }
    for remote in refs.remote_names() {
        let mut list: Vec<Branch> =
            refs.remotes.iter().filter(|r| r.remote == remote && !r.is_symbolic).map(Branch::Remote).collect();
        list.sort_by(recent_first);
        if list.len() > REMOTE_SPLIT {
            let title =
                |f: &str| if f.is_empty() { format!("{remote}, sans dossier") } else { format!("{remote} · {f}") };
            out.extend(by_folder(list, true, |f| format!("r:{remote}:{f}"), title));
        } else if !list.is_empty() {
            out.push(Group { id: format!("r:{remote}"), title: remote.clone(), remote: true, branches: list });
        }
    }
    out
}

/// Le suivi d'une branche locale : « origin/x · ↑1 », « non publiée »…
fn tracking(l: &LocalBranch) -> String {
    match (&l.upstream, l.upstream_gone) {
        (Some(u), true) => format!("{u} (supprimée sur le serveur)"),
        (Some(u), false) => format!("{u} · {}", depot::sync(l.ahead, l.behind)),
        (None, _) => "non publiée".into(),
    }
}

/// Les branches locales qui suivent la distante `r`.
fn tracked_by<'a>(refs: &'a RefsSnapshot, r: &RemoteBranch) -> Vec<&'a str> {
    refs.locals.iter().filter(|l| l.upstream.as_deref() == Some(r.full.as_str())).map(|l| l.name.as_str()).collect()
}

/// Les gestes sur une branche locale `n` (`head` : la courante), sauf pousser.
fn local_acts(n: &str, head: bool, current: Option<&str>) -> Vec<Act> {
    let mut acts = Vec::new();
    if !head {
        acts.push(Act::new(format!("basculer:{n}"), "Basculer").glyph("↪️").confirm(format!("Basculer sur {n} ?")));
        if let Some(c) = current {
            acts.push(
                Act::new(format!("fusionner:{n}"), format!("Fusionner dans {c}"))
                    .glyph("🔀")
                    .confirm(format!("Fusionner {n} dans {c} ?")),
            );
            acts.push(
                Act::new(format!("rebaser:{n}"), format!("Rebaser {c} sur {n}"))
                    .glyph("⤴️")
                    .confirm(format!("Rejouer les commits de {c} sur {n} ?")),
            );
        }
    }
    acts.push(Act::new(format!("renommer:{n}"), "Renommer…").glyph("✏️").input("Nouveau nom", "").input_value(n));
    if !head {
        acts.push(
            Act::new(format!("supprimer-branche:{n}"), "Supprimer")
                .glyph("🗑️")
                .danger()
                .confirm(format!("Supprimer la branche {n} ? (refusé si elle n'est pas fusionnée)")),
        );
        acts.push(
            Act::new(format!("supprimer-branche-force:{n}"), "Supprimer même non fusionnée")
                .glyph("⚠️")
                .danger()
                .confirm(format!("Supprimer {n} et perdre ses commits non fusionnés ?")),
        );
    }
    acts
}

/// Où « publier » pousse la branche locale `l` (`PushBranch` : vers une branche du même nom, qui devient sa branche
/// suivie) : sur le remote de sa branche suivie si celle-ci porte son nom (même supprimée sur le serveur), sinon,
/// sans suivi, sur `remote` (le remote par défaut). Erreur : sa branche suivie a un autre nom, ou est locale ;
/// `PushBranch` la ferait suivre une autre branche.
pub fn publish_remote(refs: &RefsSnapshot, l: &LocalBranch, remote: Option<String>) -> Result<String, String> {
    let Some(u) = &l.upstream else {
        return remote.ok_or_else(|| "Aucun remote : ajoutez-en un sur le PC.".into());
    };
    let target = match refs.remotes.iter().find(|r| r.full == *u) {
        Some(r) => Some((r.remote.clone(), r.name.clone())),
        // Supprimée sur le serveur : son remote d'après son nom.
        None if l.upstream_gone => refs.upstream_parts(u),
        None => None,
    };
    match target {
        Some((remote, name)) if name == l.name => Ok(remote),
        _ => Err(format!("{} suit {u}, d'un autre nom : elle se pousse depuis DragonTree sur le PC.", l.name)),
    }
}

/// Pousser une branche locale, en disant vers où : la courante vers sa branche suivie (le push de DragonTree) ; une
/// autre vers sa branche suivie si celle-ci porte son nom ; une branche sans suivi est publiée sur `remote` (le remote
/// par défaut). `None` : elle ne se pousse pas d'ici (voir [`publish_remote`]).
fn push_act(refs: &RefsSnapshot, l: &LocalBranch, head: bool, remote: Option<&str>) -> Option<Act> {
    let n = &l.name;
    let act = |id: String, label: &str, question: String| Some(Act::new(id, label).glyph("⬆️").confirm(question));
    match &l.upstream {
        Some(u) if head => act("push".into(), "Pousser", format!("Pousser {n} vers {u} ?")),
        Some(u) => {
            publish_remote(refs, l, None).ok()?;
            act(format!("publier:{n}"), "Pousser", format!("Pousser {n} vers {u} ?"))
        }
        None => {
            let to = publish_remote(refs, l, remote.map(str::to_string)).ok()?;
            act(format!("publier:{n}"), "Publier", format!("Publier {n} sur {to} ? Elle suivra {to}/{n}."))
        }
    }
}

/// Les gestes sur une branche distante : la suivre en local, la fusionner dans la courante, rebaser la courante.
fn remote_acts(r: &RemoteBranch, tracked: bool, current: Option<&str>) -> Vec<Act> {
    let full = &r.full;
    let mut acts = Vec::new();
    if !tracked {
        acts.push(
            Act::new(format!("suivre:{full}"), "Suivre en local…")
                .glyph("🌿")
                .input("Nom de la branche locale (créée et sortie)", "")
                .input_value(&r.name),
        );
    }
    if let Some(c) = current {
        acts.push(
            Act::new(format!("fusionner:{full}"), format!("Fusionner dans {c}"))
                .glyph("🔀")
                .confirm(format!("Fusionner {full} dans {c} ?")),
        );
        acts.push(
            Act::new(format!("rebaser:{full}"), format!("Rebaser {c} sur {full}"))
                .glyph("⤴️")
                .confirm(format!("Rejouer les commits de {c} sur {full} ?")),
        );
    }
    acts
}

/// La ligne d'une branche : elle ouvre son détail ; une locale porte ses gestes dans son menu (appui long). `remote` :
/// le remote par défaut, où publier une branche sans suivi.
fn branch_row(
    base: &str,
    b: Branch,
    refs: &RefsSnapshot,
    tips: &HashMap<Oid, Commit>,
    remote: Option<&str>,
    now: i64,
    busy: bool,
) -> Row {
    let key = b.key();
    let current = refs.current_branch();
    let mut sub = Vec::new();
    let mut row = Row::new(b.short()).key(&key).open(format!("{base}/branche/{key}"));
    match b {
        Branch::Local(l) => {
            let head = current == Some(l.name.as_str());
            sub.push(tracking(l));
            row = row.glyph("🌿");
            if head {
                row = row.badge("courante").tone(Tone::Ok);
            } else if l.ahead > 0 && l.behind > 0 {
                row = row.tone(Tone::Warn);
            }
            if !busy {
                row.acts = local_acts(&l.name, head, current);
                if let Some(push) = push_act(refs, l, head, remote) {
                    row.acts.insert(0, push);
                }
            }
        }
        Branch::Remote(r) => {
            row = row.glyph("☁️");
            let by = tracked_by(refs, r);
            if !by.is_empty() {
                sub.push(format!("suivie par {}", by.join(", ")));
            }
        }
    }
    if let Some(c) = tips.get(&b.oid()) {
        sub.push(depot::ago(c.committer_time, now));
    }
    row.subtitle(sub.join(" · "))
}

/// L'onglet Branches.
pub fn list(base: &str, v: &RepoView, now: i64, busy: bool) -> Screen {
    let s = Screen::new("Branches").search("Chercher une branche");
    let mut s = depot::with_tabs(s, base, &format!("{base}/historique"), v);
    let Some(refs) = &v.refs else {
        depot::reading(&mut s, "Lecture des branches…", v);
        return s;
    };
    let remotes = refs.remotes.iter().filter(|r| !r.is_symbolic).count();
    let counts = format!("{} locale(s), {remotes} distante(s)", refs.locals.len());
    s.subtitle = match refs.current_branch() {
        Some(c) => format!("Courante : {c} · {counts}"),
        None => format!("HEAD détachée · {counts}"),
    };
    if !busy {
        s.acts.push(
            Act::new("nouvelle-branche", "Nouvelle branche…")
                .glyph("🌿")
                .input("Nom (créée depuis HEAD et sortie)", "feature/…"),
        );
    }
    let remote = v.default_remote();
    let remote = remote.as_deref();
    s.band(Band::new("courante", "Courante").glyph("📍"));
    match (&refs.head, refs.current_branch().and_then(|c| refs.local(c))) {
        (HeadState::Detached(oid), _) => {
            s.row(
                Row::new("HEAD détachée")
                    .detail(oid.short())
                    .glyph("📍")
                    .key("detachee")
                    .open(format!("{base}/commit/{}", oid.to_hex())),
            );
        }
        (_, Some(l)) => {
            s.row(branch_row(base, Branch::Local(l), refs, &v.tips, remote, now, busy));
        }
        (_, None) => {
            s.empty("Branche sans commit encore.");
        }
    }
    let groups = groups(refs, &v.tips);
    let mut budget = REMOTES_SHOWN;
    let mut first_local = true;
    for g in &groups {
        let n = g.branches.len();
        let hash = short_hash(&g.id);
        let cap = if g.remote { BAND_REMOTES.min(budget) } else { BAND_LOCALS };
        // Les locales les plus récentes ouvertes ; les distantes pliées.
        let open = !g.remote && std::mem::take(&mut first_local);
        let glyph = if g.remote { "☁️" } else { "🌿" };
        s.band(Band::new(format!("g:{hash}"), &g.title).glyph(glyph).aside(n.to_string()).open(open));
        for b in g.branches.iter().take(cap) {
            s.row(branch_row(base, *b, refs, &v.tips, remote, now, busy));
        }
        if n > cap {
            let more = if cap == 0 { format!("Voir les {n} branches") } else { format!("{} autres…", n - cap) };
            s.row(Row::new(more).glyph("📋").key(format!("autres:{hash}")).open(format!("{base}/branches/g/{hash}")));
        }
        if g.remote {
            budget = budget.saturating_sub(cap.min(n));
        }
    }
    s
}

/// Un groupe de branches entier (`branches/g/<g>`), par pages ; `None` : il n'existe plus.
pub fn group(base: &str, v: &RepoView, hash: &str, now: i64, busy: bool) -> Option<Screen> {
    let Some(refs) = &v.refs else {
        let mut s = Screen::new("Branches");
        depot::reading(&mut s, "Lecture des branches…", v);
        return Some(s);
    };
    let g = groups(refs, &v.tips).into_iter().find(|g| short_hash(&g.id) == hash)?;
    let n = g.branches.len();
    let mut s = Screen::new(&g.title)
        .subtitle(format!("{n} branche(s), les plus récentes d'abord"))
        .search("Chercher une branche");
    let shown = v.count.max(PAGE).min(n);
    let remote = v.default_remote();
    for b in &g.branches[..shown] {
        s.row(branch_row(base, *b, refs, &v.tips, remote.as_deref(), now, busy));
    }
    if let Some(row) = depot::more_row(shown, n, "branches") {
        s.row(row);
    }
    Some(s)
}

/// Une ligne de commit sans graphe (à pousser, à tirer) ; `prefix` distingue sa clé de celle de l'historique.
fn plain_commit_row(base: &str, c: &Commit, prefix: &str, now: i64) -> Row {
    let hex = c.id.to_hex();
    Row::new(c.summary.as_ref())
        .subtitle(format!("{} · {}", c.author, depot::ago(c.author_time, now)))
        .detail(c.id.short())
        .glyph("●")
        .key(format!("{prefix}:{hex}"))
        .open(format!("{base}/commit/{hex}"))
}

/// Le détail d'une branche (`branche/<b>`) : pointe, suivi, à pousser et à tirer, gestes, historique avec son graphe.
pub fn detail(base: &str, v: &RepoView, key: &str, now: i64, busy: bool) -> Screen {
    let mut s = Screen::new("Branche").search("Chercher un commit (titre, auteur, identifiant)");
    let Some(refs) = &v.refs else {
        depot::reading(&mut s, "Lecture des branches…", v);
        return s;
    };
    let Some(b) = find(refs, key) else {
        s.empty("Branche introuvable : supprimée, renommée, ou pas encore relue par DragonTree.");
        return s;
    };
    let current = refs.current_branch();
    s.title = b.name().to_string();
    let tip = b.oid();
    let tip_row = match v.tips.get(&tip) {
        Some(c) => Row::new(c.summary.as_ref())
            .subtitle(format!("Pointe · {} · {}", c.author, depot::ago(c.committer_time, now)))
            .detail(tip.short()),
        None => Row::new("Pointe").detail(tip.short()),
    };
    s.row(tip_row.glyph("📍").key("pointe").open(format!("{base}/commit/{}", tip.to_hex())));
    let mut buttons = Vec::new();
    match b {
        Branch::Local(l) => {
            let head = current == Some(l.name.as_str());
            s.subtitle = if head { "Branche locale, courante".into() } else { "Branche locale".into() };
            let tone = if l.upstream_gone || (l.ahead > 0 && l.behind > 0) { Tone::Warn } else { Tone::Ok };
            s.row(Row::new("Suivi").subtitle(tracking(l)).glyph("🔁").key("suivi").tone(tone));
            if !busy {
                let mut acts = local_acts(&l.name, head, current);
                // Basculer, fusionner et rebaser en boutons (basculer d'abord) ; renommer et supprimer au menu.
                let (shown, menu): (Vec<Act>, Vec<Act>) = acts
                    .drain(..)
                    .partition(|a| ["basculer:", "fusionner:", "rebaser:"].iter().any(|p| a.id.starts_with(p)));
                buttons.extend(shown);
                if let Some(first) = buttons.first_mut().filter(|a| a.id.starts_with("basculer:")) {
                    *first = first.clone().primary();
                }
                match push_act(refs, l, head, v.default_remote().as_deref()) {
                    Some(push) => buttons.push(push),
                    None if l.ahead > 0 => {
                        if let Err(e) = publish_remote(refs, l, None) {
                            s.note(e, Tone::Info);
                        }
                    }
                    None => {}
                }
                s.acts.extend(menu);
            }
            if l.ahead > 0 || l.behind > 0 {
                match &v.pending {
                    None => {
                        s.empty("Lecture des commits à pousser et à tirer…");
                    }
                    Some(Err(e)) => {
                        s.note(e, Tone::Danger);
                    }
                    Some(Ok((push, pull))) => {
                        for (key, title, list, total) in
                            [("a-pousser", "À pousser", push, l.ahead), ("a-tirer", "À tirer", pull, l.behind)]
                        {
                            if list.is_empty() {
                                continue;
                            }
                            s.band(Band::new(key, title).glyph("↕️").aside(total.to_string()));
                            for c in list.iter().take(PENDING_SHOWN) {
                                s.row(plain_commit_row(base, c, key, now));
                            }
                            if total as usize > PENDING_SHOWN {
                                s.note(format!("… et {} autres.", total as usize - PENDING_SHOWN), Tone::Info);
                            }
                        }
                    }
                }
            }
        }
        Branch::Remote(r) => {
            let by = tracked_by(refs, r);
            s.subtitle = if by.is_empty() {
                format!("Branche distante, sur {}", r.remote)
            } else {
                format!("Branche distante, suivie par {}", by.join(", "))
            };
            if !busy {
                buttons = remote_acts(r, !by.is_empty(), current);
                if let Some(first) = buttons.first_mut() {
                    *first = first.clone().primary();
                }
            }
        }
    }
    if !buttons.is_empty() {
        s.buttons(buttons);
    }
    s.band(Band::new("historique", "Historique").glyph("🕘"));
    historique::fill(&mut s, base, v, now);
    s
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use suite_sdk::mobile::ui::{Block, Fold};

    use super::*;

    fn oid(n: u32) -> Oid {
        Oid::from_hex(&format!("{n:08x}").repeat(5)).unwrap()
    }

    fn local(name: &str, n: u32) -> LocalBranch {
        LocalBranch {
            name: name.into(),
            oid: oid(n),
            upstream: None,
            ahead: 0,
            behind: 0,
            upstream_gone: false,
            is_head: false,
        }
    }

    fn remote(remote: &str, name: &str, n: u32) -> RemoteBranch {
        RemoteBranch {
            remote: remote.into(),
            name: name.into(),
            full: format!("{remote}/{name}"),
            oid: oid(n),
            is_symbolic: false,
        }
    }

    fn tip(n: u32, time: i64) -> (Oid, Commit) {
        let c = Commit {
            id: oid(n),
            parents: Box::new([]),
            author: "Essai".into(),
            author_email: "essai@exemple.org".into(),
            author_time: time,
            committer: "Essai".into(),
            committer_time: time,
            summary: format!("commit {n}").into(),
        };
        (c.id, c)
    }

    fn view(locals: Vec<LocalBranch>, remotes: Vec<RemoteBranch>, tips: Vec<(Oid, Commit)>) -> RepoView {
        let refs = RefsSnapshot {
            head: HeadState::Branch("main".into()),
            head_oid: Some(oid(1)),
            locals,
            remotes,
            tags: Vec::new(),
            labels: Default::default(),
        };
        RepoView {
            refs: Some(Arc::new(refs)),
            tips: Arc::new(tips.into_iter().collect()),
            remotes: Arc::new(Vec::new()),
            ..Default::default()
        }
    }

    fn bands(s: &Screen) -> Vec<(&str, Fold, &str)> {
        s.blocks
            .iter()
            .filter_map(|b| match b {
                Block::Section { title, fold, aside, .. } => Some((title.as_str(), *fold, aside.as_str())),
                _ => None,
            })
            .collect()
    }

    fn rows(s: &Screen) -> Vec<&Row> {
        s.blocks
            .iter()
            .filter_map(|b| match b {
                Block::Row(r) => Some(r),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn folders_keep_two_levels() {
        assert_eq!(folder("feature/achedotal/64522-x"), "feature/achedotal");
        assert_eq!(folder("a/b/c/d"), "a/b");
        assert_eq!(folder("fix/x"), "fix");
        assert_eq!(folder("main"), "");
    }

    /// La courante d'abord, puis les locales par date ; un toucher ouvre le détail (jamais une bascule) ; les gestes
    /// sont au menu.
    #[test]
    fn branches_come_current_first_then_by_date_and_open_their_detail() {
        let mut main = local("main", 1);
        main.is_head = true;
        let v = view(
            vec![main, local("ancienne", 2), local("recente", 3)],
            vec![remote("origin", "autre", 4)],
            vec![tip(2, 100), tip(3, 900)],
        );
        let s = list("depot/k", &v, 1000, false);
        let titles: Vec<&str> = rows(&s).iter().map(|r| r.title.as_str()).collect();
        assert_eq!(titles, ["main", "recente", "ancienne", "autre"]);
        assert_eq!(
            bands(&s),
            [("Courante", Fold::Open, ""), ("Locales", Fold::Open, "2"), ("origin", Fold::Closed, "1")]
        );
        for r in rows(&s) {
            assert!(r.tap.is_none() && r.open.as_deref().is_some_and(|o| o.starts_with("depot/k/branche/")), "{r:?}");
        }
        let ids = s.action_ids();
        for id in ["basculer:recente", "fusionner:recente", "rebaser:recente", "supprimer-branche:recente"] {
            assert!(ids.contains(id), "{id}");
        }
        assert!(!ids.contains("basculer:main") && !ids.contains("supprimer-branche:main"), "pas la courante");
        assert!(ids.contains("nouvelle-branche"));
        assert!(!ids.iter().any(|i| i.starts_with("suivre:")), "une distante se suit depuis son détail");
    }

    /// Au-delà de 12 locales, un bandeau par dossier ; un gros remote, un bandeau par dossier, plié, coupé à 25 lignes
    /// avec « N autres… » qui ouvre le groupe entier, par pages de 100.
    #[test]
    fn many_branches_are_grouped_by_folder_and_cut() {
        let locals: Vec<LocalBranch> =
            (0..15).map(|i| local(&format!("feature/moi/b{i}"), 10 + i)).chain([local("fix", 9)]).collect();
        let remotes: Vec<RemoteBranch> =
            (0..150).map(|i| remote("origin", &format!("feature/eux/r{i}"), 100 + i)).collect();
        let v = view(locals, remotes, Vec::new());
        let s = list("depot/k", &v, 0, false);
        let b = bands(&s);
        assert!(b.contains(&("feature/moi", Fold::Open, "15")), "{b:?}");
        assert!(b.contains(&("Locales, sans dossier", Fold::Closed, "1")), "{b:?}");
        assert!(b.contains(&("origin · feature/eux", Fold::Closed, "150")), "{b:?}");
        let more = rows(&s).into_iter().find(|r| r.title == "125 autres…").expect("N autres");
        let path = more.open.clone().unwrap();
        let hash = path.rsplit('/').next().unwrap();
        let g = group("depot/k", &v, hash, 0, false).unwrap();
        assert_eq!(rows(&g).len(), PAGE + 1, "une page, et « plus »");
        assert!(g.action_ids().contains("plus"));
        assert!(group("depot/k", &v, "inconnu", 0, false).is_none());
    }

    /// Le détail d'une branche locale : basculer en premier bouton, fusionner et rebaser, pousser ; renommer et
    /// supprimer au menu ; ce qu'elle a à pousser et à tirer. Une distante : la suivre en local.
    #[test]
    fn a_branch_detail_offers_its_gestures() {
        let mut main = local("main", 1);
        main.is_head = true;
        let mut x = local("feature/x", 2);
        x.upstream = Some("origin/feature/x".into());
        x.ahead = 1;
        let mut v =
            view(vec![main, x], vec![remote("origin", "feature/x", 3), remote("origin", "seule", 4)], Vec::new());
        v.pending = Some(Ok((vec![tip(2, 0).1], Vec::new())));
        let key = branch_key("refs/heads/feature/x");
        let s = detail("depot/k", &v, &key, 0, false);
        assert_eq!(s.title, "feature/x");
        let buttons: Vec<(&str, bool)> = s
            .blocks
            .iter()
            .filter_map(|b| match b {
                Block::Buttons { acts } => {
                    Some(acts.iter().map(|a| (a.id.as_str(), a.style == suite_sdk::mobile::ui::ActStyle::Primary)))
                }
                _ => None,
            })
            .flatten()
            .collect();
        assert_eq!(
            buttons,
            [
                ("basculer:feature/x", true),
                ("fusionner:feature/x", false),
                ("rebaser:feature/x", false),
                ("publier:feature/x", false)
            ]
        );
        let menu: Vec<&str> = s.acts.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(menu, ["renommer:feature/x", "supprimer-branche:feature/x", "supprimer-branche-force:feature/x"]);
        assert!(bands(&s).iter().any(|b| b.0 == "À pousser"), "{s:?}");
        let r = detail("depot/k", &v, &branch_key("refs/remotes/origin/seule"), 0, false);
        assert!(r.action_ids().contains("suivre:origin/seule"), "{r:?}");
        let tracked = detail("depot/k", &v, &branch_key("refs/remotes/origin/feature/x"), 0, false);
        assert!(!tracked.action_ids().contains("suivre:origin/feature/x"), "déjà suivie");
        assert!(format!("{:?}", detail("depot/k", &v, "inconnue", 0, false)).contains("introuvable"));
        assert!(detail("depot/k", &v, &key, 0, true).action_ids().is_empty(), "rien pendant une opération");
    }

    /// Le défaut du 28/10 : « Pousser » une branche qui n'est pas la courante la poussait sur le remote de la
    /// courante, et la faisait suivre une autre branche. Elle va vers sa branche suivie, sur son remote à elle ; une
    /// branche qui suit une branche d'un autre nom ne se pousse pas d'ici ; une branche sans suivi est publiée sur le
    /// remote par défaut. La question dit où.
    #[test]
    fn a_push_goes_to_the_remote_the_branch_tracks() {
        use dragontree_app::repo::RemoteOp;
        use dragontree_git::{Remote, StatusSnapshot};

        let mut main = local("main", 1);
        main.is_head = true;
        main.upstream = Some("upstream/main".into());
        let mut fork = local("feature/x", 2);
        fork.upstream = Some("origin/feature/x".into());
        fork.ahead = 1;
        let mut renamed = local("renommee", 3);
        renamed.upstream = Some("origin/ancien-nom".into());
        renamed.ahead = 1;
        let remotes =
            vec![remote("upstream", "main", 5), remote("origin", "feature/x", 6), remote("origin", "ancien-nom", 7)];
        let mut v = view(vec![main, fork, renamed, local("seule", 4)], remotes, Vec::new());
        let named = |name: &str| Remote { name: name.into(), fetch_url: String::new(), push_url: String::new() };
        v.remotes = Arc::new(vec![named("origin"), named("upstream")]);
        let status = StatusSnapshot {
            branch: Some("main".into()),
            upstream: Some("upstream/main".into()),
            ..Default::default()
        };
        v.status = Some(Ok(status));
        let push = |name: &str| {
            let s = detail("depot/k", &v, &branch_key(&format!("refs/heads/{name}")), 0, false);
            s.all_acts().into_iter().find(|a| a.id == "push" || a.id.starts_with("publier:")).cloned()
        };

        let fork = push("feature/x").expect("pousser feature/x");
        assert_eq!(fork.confirm.as_deref(), Some("Pousser feature/x vers origin/feature/x ?"));
        assert_eq!(
            depot::parse(&v, &fork.id, None).unwrap().0,
            RemoteOp::PushBranch { name: "feature/x".into(), remote: "origin".into() },
            "vers son remote, pas celui de la courante"
        );
        assert!(push("renommee").is_none(), "suit une branche d'un autre nom");
        assert!(depot::parse(&v, "publier:renommee", None).is_err());
        let s = detail("depot/k", &v, &branch_key("refs/heads/renommee"), 0, false);
        assert!(format!("{s:?}").contains("sur le PC"), "elle dit pourquoi : {s:?}");
        let alone = push("seule").expect("publier seule");
        assert_eq!(alone.confirm.as_deref(), Some("Publier seule sur upstream ? Elle suivra upstream/seule."));
        assert_eq!(
            depot::parse(&v, &alone.id, None).unwrap().0,
            RemoteOp::PushBranch { name: "seule".into(), remote: "upstream".into() }
        );
        let main = push("main").expect("pousser la courante");
        assert_eq!((main.id.as_str(), main.confirm.as_deref()), ("push", Some("Pousser main vers upstream/main ?")));
        assert!(depot::parse(&v, "publier:inconnue", None).is_err(), "une branche que DragonTree n'a pas lue");
    }
}
