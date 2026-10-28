//! L'historique d'un dépôt sur le téléphone, avec le graphe des commits et les étiquettes de leurs références.
//!
//! - `historique` : celui de la branche courante (HEAD), lu à part dans un fil (`git log HEAD`), avec son graphe ;
//! - `historique/tout` : le graphe de DragonTree, tel qu'il l'affiche (sa portée, ses fusions repliées, ses commits
//!   perdus), dont le sous-titre dit la vraie portée ;
//! - l'historique d'une branche, dans son détail (`branches.rs`).
//!
//! 100 commits par page, « plus » jusqu'à 2 000 ; la recherche du téléphone filtre les commits chargés. Le graphe
//! est celui de DragonTree (`GraphBuilder`, `GraphLayout`), découpé ligne par ligne comme il le dessine
//! (`graph_paint.rs`) : un lien d'un enfant vers son parent part du bas de la ligne de l'enfant, traverse les lignes
//! entre les deux, et arrive par le haut de la ligne du parent ; un parent qui n'est pas dans la page continue
//! jusqu'en bas.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use dragontree_git::graph::UNKNOWN_ROW;
use dragontree_git::{Commit, GraphBuilder, GraphLayout, HeadState, Oid, RefKind, RefLabel, RefsSnapshot};
use suite_sdk::mobile::ui::{
    Act, Chip, GRAPH_BOTTOM, GRAPH_THROUGH, GRAPH_TOP, GraphCell, LANE_COLORS, NODE_COMMIT, NODE_HEAD, NODE_LOST,
    NODE_MERGE, NODE_STASH, Row, Screen, Tone,
};

use crate::depot::{self, PAGE, PAGE_MAX, RepoView};

/// Voies dessinées au plus : un téléphone n'a pas la largeur d'un écran de PC ; une voie au-delà est ramenée sur la
/// dernière.
pub const MAX_LANES: u8 = 8;
/// Étiquettes montrées par commit (puis « +N »).
const CHIPS_MAX: usize = 5;

/// Des commits dans l'ordre d'affichage et leur graphe (mêmes indices).
#[derive(Clone, Debug, Default)]
pub struct LogPage {
    pub commits: Arc<Vec<Commit>>,
    pub graph: Arc<GraphLayout>,
}

impl LogPage {
    /// Le graphe de `commits`, comme DragonTree le construit.
    pub fn of(commits: Vec<Commit>) -> LogPage {
        let mut b = GraphBuilder::new();
        for c in &commits {
            b.push(c.id, &c.parents);
        }
        LogPage { commits: Arc::new(commits), graph: Arc::new(b.finish()) }
    }
}

/// Une ligne d'historique : le commit, sa part du graphe, ses étiquettes.
#[derive(Clone, Debug)]
pub struct LogRow {
    pub commit: Commit,
    pub cell: GraphCell,
    pub chips: Vec<Chip>,
    /// Une fusion repliée par DragonTree : les commits qu'elle cache.
    pub folded: usize,
}

/// Ce que montre un écran d'historique.
#[derive(Clone, Debug, Default)]
pub struct LogView {
    pub rows: Vec<LogRow>,
    /// La liste a d'autres commits après ceux-ci (ou peut en avoir).
    pub more: bool,
    /// Ce que parcourt cet historique (sous-titre).
    pub scope: String,
}

/// La part du graphe de chaque ligne `0..rows` de `layout` : nœud (voie, couleur, commit ou fusion) et traits. Les
/// voies au-delà de [`MAX_LANES`] sont ramenées sur la dernière ; une voie n'est traversée qu'une fois par ligne.
pub fn cells(layout: &GraphLayout, rows: usize) -> Vec<GraphCell> {
    let rows = rows.min(layout.rows.len());
    if rows == 0 {
        return Vec::new();
    }
    let lane = |l: u16| l.min(u16::from(MAX_LANES) - 1) as u8;
    let color = |c: u8| c % LANE_COLORS;
    // Par ligne : traits du bas et du haut (peu nombreux), et la couleur de chaque voie traversée.
    let mut ends: Vec<Vec<(u8, u8, u8, u8)>> = vec![Vec::new(); rows];
    let mut through: Vec<[Option<u8>; MAX_LANES as usize]> = vec![[None; MAX_LANES as usize]; rows];
    let mut widest = 0;
    for e in layout.edges_in(0, (rows - 1) as u32) {
        let (a, b, c, col) = (lane(e.child_lane), lane(e.lane), lane(e.parent_lane), color(e.color));
        let child = e.child_row as usize;
        ends[child].push((GRAPH_BOTTOM, a, b, col));
        let end = if e.parent_row == UNKNOWN_ROW { rows } else { (e.parent_row as usize).min(rows) };
        for t in &mut through[child + 1..end.max(child + 1)] {
            t[b as usize].get_or_insert(col);
        }
        if e.parent_row != UNKNOWN_ROW && (e.parent_row as usize) < rows {
            ends[e.parent_row as usize].push((GRAPH_TOP, b, c, col));
        }
        widest = widest.max(a).max(b).max(c);
    }
    let mut out = Vec::with_capacity(rows);
    for (r, (mut lines, across)) in ends.into_iter().zip(through).enumerate() {
        let row = layout.rows[r];
        let node = lane(row.lane);
        widest = widest.max(node);
        lines.sort_unstable();
        lines.dedup();
        lines.extend(across.iter().enumerate().filter_map(|(l, c)| c.map(|c| (GRAPH_THROUGH, l as u8, l as u8, c))));
        let kind = if row.is_merge { NODE_MERGE } else { NODE_COMMIT };
        out.push(GraphCell { lanes: 0, node, color: color(row.color), kind, lines });
    }
    for c in &mut out {
        c.lanes = widest + 1;
    }
    out
}

/// Les étiquettes d'un commit : HEAD détachée et la branche courante mises en avant, puis les branches locales, les
/// distantes et les étiquettes git ; « +N » au-delà de cinq.
pub fn chips(labels: Option<&Vec<RefLabel>>) -> Vec<Chip> {
    let Some(labels) = labels else { return Vec::new() };
    let rank = |l: &RefLabel| match l.kind {
        RefKind::Head => 0,
        RefKind::Local if l.is_head => 1,
        RefKind::Local => 2,
        RefKind::Remote => 3,
        RefKind::Tag => 4,
    };
    let mut sorted: Vec<&RefLabel> = labels.iter().collect();
    sorted.sort_by_key(|l| rank(l));
    let mut out: Vec<Chip> = sorted
        .iter()
        .take(CHIPS_MAX)
        .map(|l| match l.kind {
            RefKind::Head => Chip::new("HEAD").glyph("📍").tone(Tone::Warn).strong(),
            RefKind::Local => {
                let c = Chip::new(&l.name).glyph("🌿").tone(Tone::Ok);
                if l.is_head { c.strong() } else { c }
            }
            RefKind::Remote => Chip::new(&l.name).glyph("☁️").tone(Tone::Info),
            RefKind::Tag => Chip::new(&l.name).glyph("🏷️"),
        })
        .collect();
    if labels.len() > CHIPS_MAX {
        out.push(Chip::new(format!("+{}", labels.len() - CHIPS_MAX)));
    }
    out
}

/// Ce qui marque un commit dans le graphe, en plus de la fusion : HEAD, perdu, remise.
#[derive(Default)]
pub struct Marks<'a> {
    pub head: Option<Oid>,
    pub lost: Option<&'a HashSet<Oid>>,
    pub stashes: HashSet<Oid>,
    pub folded: Option<&'a HashMap<Oid, usize>>,
}

/// Les `cells.len()` premières lignes de `page`, avec leur graphe (`cells`, voir [`cells`]) et leurs étiquettes.
pub fn rows(page: &LogPage, cells: &[GraphCell], refs: Option<&RefsSnapshot>, marks: &Marks) -> Vec<LogRow> {
    let head = marks.head.or_else(|| refs.and_then(|r| r.head_oid));
    page.commits
        .iter()
        .zip(cells)
        .map(|(c, cell)| {
            let mut cell = cell.clone();
            if Some(c.id) == head {
                cell.kind = NODE_HEAD;
            } else if marks.stashes.contains(&c.id) {
                cell.kind = NODE_STASH;
            } else if marks.lost.is_some_and(|l| l.contains(&c.id)) {
                cell.kind = NODE_LOST;
            }
            LogRow {
                commit: c.clone(),
                cell,
                chips: chips(refs.and_then(|r| r.labels.get(&c.id))),
                folded: marks.folded.and_then(|f| f.get(&c.id)).copied().unwrap_or(0),
            }
        })
        .collect()
}

/// La ligne d'un commit : titre, auteur et date, identifiant court, graphe et étiquettes ; elle ouvre le commit.
pub fn commit_row(base: &str, r: &LogRow, now: i64) -> Row {
    let c = &r.commit;
    let hex = c.id.to_hex();
    let mut row = Row::new(c.summary.as_ref())
        .subtitle(format!("{} · {}", c.author, depot::ago(c.author_time, now)))
        .detail(c.id.short())
        .key(&hex)
        .graph(r.cell.clone())
        .open(format!("{base}/commit/{hex}"));
    row.chips = r.chips.clone();
    if r.folded > 0 {
        row = row.badge(format!("+{}", r.folded));
    }
    if r.cell.kind == NODE_LOST {
        row = row.subtitle(format!("perdu (seul le reflog l'atteint) · {}", c.author));
    }
    row
}

/// La ligne qui allonge un historique : `shown` commits montrés, `more` : il y en a d'autres.
pub fn more_row(shown: usize, more: bool) -> Option<Row> {
    if !more {
        return None;
    }
    if shown >= PAGE_MAX {
        return Some(Row::new("La suite de l'historique est sur le PC").glyph("🖥️").key("plus"));
    }
    Some(
        Row::new(format!("{} commits de plus", PAGE.min(PAGE_MAX - shown)))
            .subtitle(format!("{shown} montrés"))
            .glyph("⬇️")
            .key("plus")
            .tap(Act::new("plus", "Afficher la suite")),
    )
}

/// Les commits d'un historique, ou ce qui manque pour les montrer.
pub fn fill(s: &mut Screen, base: &str, v: &RepoView, now: i64) {
    match &v.log {
        None => depot::reading(s, "Lecture de l'historique…", v),
        Some(Err(e)) => {
            s.note(e, Tone::Danger);
        }
        Some(Ok(l)) if l.rows.is_empty() => {
            s.empty("Aucun commit.");
        }
        Some(Ok(l)) => {
            for r in &l.rows {
                s.row(commit_row(base, r, now));
            }
            if let Some(row) = more_row(l.rows.len(), l.more) {
                s.row(row);
            }
        }
    }
}

/// L'onglet Historique : celui de la branche courante, avec son graphe ; « Toutes les branches » mène au graphe de
/// DragonTree.
pub fn current(base: &str, v: &RepoView, now: i64) -> Screen {
    // HEAD d'après les branches lues par DragonTree, sinon son état ; rien tant qu'il n'a lu ni l'un ni l'autre.
    let subtitle = match (v.refs.as_deref().map(|r| &r.head), v.current_branch().as_ref()) {
        (Some(HeadState::Branch(b) | HeadState::Unborn(b)), _) | (None, Some(b)) => format!("Branche courante : {b}"),
        (Some(HeadState::Detached(o)), _) => format!("HEAD détachée ({})", o.short()),
        (None, None) => String::new(),
    };
    let s = Screen::new("Historique").subtitle(subtitle).search("Chercher un commit (titre, auteur, identifiant)");
    let mut s = depot::with_tabs(s, base, &format!("{base}/historique"), v);
    s.acts.push(Act::new("actualiser", "Actualiser").glyph("🔄"));
    s.row(
        Row::new("Toutes les branches")
            .subtitle("Le graphe de DragonTree, avec les autres branches")
            .glyph("🌳")
            .key("tout")
            .open(format!("{base}/historique/tout")),
    );
    fill(&mut s, base, v, now);
    s
}

/// Le graphe de DragonTree, toutes branches : sa portée est dite en sous-titre.
pub fn all(base: &str, v: &RepoView, now: i64) -> Screen {
    let scope = v.log.as_ref().and_then(|l| l.as_ref().ok()).map(|l| l.scope.clone()).unwrap_or_default();
    let s =
        Screen::new("Toutes les branches").subtitle(scope).search("Chercher un commit (titre, auteur, identifiant)");
    let mut s = depot::with_tabs(s, base, &format!("{base}/historique/tout"), v);
    s.acts.push(Act::new("actualiser", "Actualiser").glyph("🔄"));
    fill(&mut s, base, v, now);
    s
}

#[cfg(test)]
mod tests {
    use dragontree_git::Oid;

    use super::*;

    fn oid(n: u8) -> Oid {
        Oid::from_hex(&format!("{n:02x}").repeat(20)).unwrap()
    }

    fn commit(n: u8, parents: &[u8]) -> Commit {
        Commit {
            id: oid(n),
            parents: parents.iter().map(|p| oid(*p)).collect(),
            author: "Essai".into(),
            author_email: "essai@exemple.org".into(),
            author_time: 0,
            committer: "Essai".into(),
            committer_time: 0,
            summary: format!("commit {n}").into(),
        }
    }

    /// Une fusion : 1 (fusion de 2 et 3), 2 puis 3 sur deux voies, 4 leur base. Les traits se raccordent d'une ligne
    /// à l'autre comme DragonTree les dessine.
    #[test]
    fn graph_cells_join_from_one_row_to_the_next() {
        let page = LogPage::of(vec![commit(1, &[2, 3]), commit(2, &[4]), commit(3, &[4]), commit(4, &[])]);
        let c = cells(&page.graph, 4);
        assert_eq!(c.len(), 4);
        assert!(c.iter().all(|c| c.lanes == 2), "{c:?}");
        assert_eq!((c[0].node, c[0].kind), (0, NODE_MERGE));
        let parts = |i: usize, part: u8| -> Vec<(u8, u8)> {
            c[i].lines.iter().filter(|l| l.0 == part).map(|l| (l.1, l.2)).collect()
        };
        // Ligne 0 : la fusion part vers 2 (voie 0) et vers 3 (voie 1).
        assert_eq!(parts(0, GRAPH_BOTTOM), [(0, 0), (0, 1)]);
        // Ligne 1 : 2 arrive par le haut, la voie 1 passe à côté.
        assert_eq!(parts(1, GRAPH_TOP), [(0, 0)]);
        assert_eq!(parts(1, GRAPH_THROUGH), [(1, 1)]);
        // Ligne 3 : la base reçoit ses deux enfants.
        assert_eq!(parts(3, GRAPH_TOP).len(), 2);
        // Une page plus courte que le graphe : le lien vers un parent hors page descend jusqu'en bas.
        let first = cells(&page.graph, 2);
        assert!(first[1].lines.iter().any(|l| l.0 == GRAPH_THROUGH && l.1 == 1), "{first:?}");
    }

    #[test]
    fn many_lanes_are_folded_onto_the_last_one() {
        // Huit branches qui partent du même commit, et une neuvième.
        let tips: Vec<Commit> = (1..=10).map(|n| commit(n, &[50])).collect();
        let mut all = tips;
        all.push(commit(50, &[]));
        let page = LogPage::of(all);
        let c = cells(&page.graph, page.commits.len());
        assert!(c.iter().all(|c| c.lanes == MAX_LANES && c.node < MAX_LANES));
        assert!(c.iter().all(|c| c.lines.iter().all(|l| l.1 < MAX_LANES && l.2 < MAX_LANES)));
        let through = c[9].lines.iter().filter(|l| l.0 == GRAPH_THROUGH).count();
        assert!(through <= MAX_LANES as usize, "une voie traversée une fois : {:?}", c[9]);
    }

    #[test]
    fn rows_carry_head_chips_and_folded_merges() {
        let page = LogPage::of(vec![commit(1, &[2]), commit(2, &[])]);
        let c = cells(&page.graph, 2);
        let mut labels = HashMap::new();
        labels.insert(
            oid(1),
            vec![
                RefLabel { kind: RefKind::Tag, name: "v1".into(), is_head: false },
                RefLabel { kind: RefKind::Local, name: "main".into(), is_head: true },
                RefLabel { kind: RefKind::Remote, name: "origin/main".into(), is_head: false },
            ],
        );
        let refs = RefsSnapshot {
            head: dragontree_git::HeadState::Branch("main".into()),
            head_oid: Some(oid(1)),
            locals: Vec::new(),
            remotes: Vec::new(),
            tags: Vec::new(),
            labels,
        };
        let folded = HashMap::from([(oid(2), 7)]);
        let r = rows(&page, &c, Some(&refs), &Marks { folded: Some(&folded), ..Default::default() });
        assert_eq!(r[0].cell.kind, NODE_HEAD);
        let chips: Vec<(&str, bool)> = r[0].chips.iter().map(|c| (c.text.as_str(), c.strong)).collect();
        assert_eq!(chips, [("main", true), ("origin/main", false), ("v1", false)]);
        let row = commit_row("depot/k", &r[1], 0);
        assert_eq!((row.badge.as_str(), row.key.as_str()), ("+7", oid(2).to_hex().as_str()));
        assert_eq!(row.open.as_deref(), Some(format!("depot/k/commit/{}", oid(2).to_hex()).as_str()));
        assert!(more_row(100, true).is_some_and(|r| r.tap.is_some()) && more_row(100, false).is_none());
        assert!(more_row(PAGE_MAX, true).is_some_and(|r| r.tap.is_none()), "au-delà : sur le PC");
    }
}
