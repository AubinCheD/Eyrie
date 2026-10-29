//! Les écrans du PC en lignes Slint : un bloc par ligne de liste, les lignes consécutives réunies en une carte
//! (premier et dernier arrondis).
//!
//! Chaque ligne a une clé (son identité dans l'écran : `Row.key`, sinon son titre et son glyphe) et une empreinte (ce
//! qui la dessine). La liste est mise à jour sur place ([`patch`]) : seules les lignes changées sont redessinées, et
//! une ligne touchée ou un appui long en cours survit aux mises à jour du PC (jusqu'à toutes les 300 ms).

use std::collections::{HashMap, HashSet};
use std::hash::{DefaultHasher, Hash, Hasher};

use slint::{Color, Model, ModelRc, SharedString, VecModel};
use suite_mobile::ui::{
    Act, ActStyle, Block, Chip, DIFF_ADDED, DIFF_REMOVED, Fold, GRAPH_BOTTOM, GRAPH_TOP, GraphCell, NODE_NONE,
    PROGRESS_UNKNOWN, Row, Screen, Tone,
};

use crate::{GraphSeg, UiAct, UiChip, UiRow};

pub fn tone(t: Option<Tone>) -> i32 {
    match t {
        None => 0,
        Some(Tone::Info) => 1,
        Some(Tone::Ok) => 2,
        Some(Tone::Warn) => 3,
        Some(Tone::Danger) => 4,
    }
}

pub fn rgb(c: u32) -> Color {
    Color::from_rgb_u8((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

/// Ce que le téléphone ajoute à l'écran du PC, sans rien lui envoyer.
#[derive(Clone, Debug, Default)]
pub struct Local {
    /// Les actions envoyées depuis cet écran dont le PC n'a pas encore donné le résultat (leurs identifiants).
    pub pending: HashSet<String>,
    /// Les bandeaux que l'utilisateur a pliés (`false`) ou dépliés (`true`), par clé ; les autres gardent l'état de
    /// départ donné par le PC.
    pub open: HashMap<String, bool>,
    /// Le texte cherché dans l'écran (vide : pas de recherche).
    pub search: String,
    /// Les lignes des diffs passent à la ligne (sinon elles sont coupées, à chasse fixe et de hauteur égale).
    pub wrap: bool,
    /// L'écran (module, chemin), que ses actions portent.
    pub screen: (String, String),
}

impl Local {
    /// L'action est en cours : envoyée et sans réponse, ou déjà faite par le module (`Act.busy`).
    fn busy(&self, a: &Act) -> bool {
        a.busy || (!a.id.is_empty() && self.pending.contains(&a.id))
    }
}

pub fn act(a: &Act) -> UiAct {
    act_in(a, &Local::default())
}

pub fn act_in(a: &Act, local: &Local) -> UiAct {
    let input = a.input.clone().unwrap_or_default();
    UiAct {
        id: a.id.as_str().into(),
        label: a.label.as_str().into(),
        glyph: a.glyph.as_str().into(),
        style: match a.style {
            ActStyle::Normal => 0,
            ActStyle::Primary => 1,
            ActStyle::Danger => 2,
        },
        confirm: dialog(a.confirm.as_deref().unwrap_or_default()).into(),
        has_input: a.input.is_some(),
        input_label: dialog(&input.label).into(),
        input_placeholder: input.placeholder.into(),
        input_value: input.value.into(),
        multiline: input.multiline,
        url: a.url.clone().unwrap_or_default().into(),
        open: a.open.clone().unwrap_or_default().into(),
        busy: local.busy(a),
        folder: input.folder.is_some(),
        module: local.screen.0.as_str().into(),
        path: local.screen.1.as_str().into(),
    }
}

fn acts(list: &[Act], local: &Local) -> ModelRc<UiAct> {
    ModelRc::new(VecModel::from(list.iter().map(|a| act_in(a, local)).collect::<Vec<_>>()))
}

/// Les actions d'un menu, les dangereuses à la fin (la feuille les sépare des autres).
pub fn menu<'a>(list: impl IntoIterator<Item = &'a Act>) -> Vec<&'a Act> {
    let (danger, safe): (Vec<&Act>, Vec<&Act>) = list.into_iter().partition(|a| a.style == ActStyle::Danger);
    safe.into_iter().chain(danger).collect()
}

fn menu_acts(list: &[Act], local: &Local) -> ModelRc<UiAct> {
    ModelRc::new(VecModel::from(menu(list).into_iter().map(|a| act_in(a, local)).collect::<Vec<_>>()))
}

/// Les actions de la barre du haut : une seule, en clair ; sinon jusqu'à deux en icônes (celles qui ont un glyphe), les
/// autres dans le menu ⋯ avec celles du téléphone (`local`, « Tout replier »…). Rend (barre, menu).
pub fn bar<'a>(acts: &'a [Act], local: &'a [Act]) -> (Vec<&'a Act>, Vec<&'a Act>) {
    if acts.len() == 1 && local.is_empty() {
        return (acts.iter().collect(), Vec::new());
    }
    let icons: Vec<&Act> = acts.iter().filter(|a| !a.glyph.is_empty()).take(2).collect();
    let rest = acts.iter().filter(|a| !icons.iter().any(|i| std::ptr::eq(*i, *a)));
    (icons.clone(), menu(rest.chain(local)))
}

/// Une ligne de la liste avant d'être dessinée : sa clé, son empreinte, et ce qui la dessine.
pub struct Item<'a> {
    pub key: String,
    pub sig: u64,
    what: What<'a>,
}

enum What<'a> {
    /// `open` : `None` pour un titre qui ne se plie pas.
    Section {
        block: &'a Block,
        open: Option<bool>,
    },
    Row {
        row: &'a Row,
        first: bool,
        last: bool,
    },
    /// Note, jauge, texte, liste vide, boutons.
    Block(&'a Block),
    /// L'en-tête d'un diff : ses ajouts et retraits, le choix du retour à la ligne.
    DiffBar {
        added: usize,
        removed: usize,
        empty: bool,
    },
    /// `numbers` : ancien et nouveau numéro (0 : sans) ; `gutter` : chiffres de la gouttière (0 : pas de numéros).
    DiffLine {
        kind: u8,
        text: &'a str,
        numbers: (u32, u32),
        gutter: usize,
        last: bool,
    },
    /// Rien ne correspond à la recherche.
    NoMatch(String),
}

/// Des clés uniques dans l'écran : la deuxième ligne « PowerShell » devient `r:PowerShell…#2`.
#[derive(Default)]
struct Keys(HashMap<String, usize>);

impl Keys {
    fn unique(&mut self, base: String) -> String {
        let n = self.0.entry(base.clone()).or_insert(0);
        *n += 1;
        if *n == 1 { base } else { format!("{base}#{n}") }
    }
}

fn hash_acts<'a>(h: &mut DefaultHasher, list: impl IntoIterator<Item = &'a Act>, local: &Local) {
    for a in list {
        (a, local.busy(a)).hash(h);
    }
}

/// Ce qui dessine un bloc (les jauges ont un nombre à virgule : `Block` ne dérive pas `Hash`).
fn hash_block(h: &mut DefaultHasher, b: &Block, local: &Local) {
    std::mem::discriminant(b).hash(h);
    match b {
        Block::Section { title, aside, key, fold, color, glyph, badge, tone, acts } => {
            (title, aside, key, fold, color, glyph, badge, tone, acts.len()).hash(h);
            hash_acts(h, acts, local);
        }
        Block::Row(r) => {
            r.hash(h);
            hash_acts(h, r.tap.iter().chain(&r.trailing).chain(&r.acts), local);
        }
        Block::Note { text, tone, progress, acts } => {
            (text, tone, progress, acts.len()).hash(h);
            hash_acts(h, acts, local);
        }
        Block::Meter { label, value, text, tone } => (label, value.to_bits(), text, tone).hash(h),
        Block::Text { text, mono } => (text, mono).hash(h),
        Block::Empty { text } => text.hash(h),
        Block::Buttons { acts } => {
            acts.len().hash(h);
            hash_acts(h, acts, local);
        }
        Block::Diff { lines, numbers } => (lines, numbers).hash(h),
    }
}

/// Un bandeau est-il ouvert ? Le choix de l'utilisateur (ouvert, et le ton du bandeau quand il l'a fait), sinon l'état
/// de départ du PC. « L'alerte gagne » : un bandeau plié qui passe à Attention ou Danger, plus grave qu'au pliage, se
/// rouvre, une fois. Plié pendant une alerte, il reste plié tant qu'elle dure ; s'il se calme, le ton retenu baisse
/// avec lui, et la prochaine alerte le rouvre. Rend l'état, et le choix à retenir désormais quand il change.
pub fn band_open(chosen: Option<(bool, i32)>, default_open: bool, tone_now: i32) -> (bool, Option<(bool, i32)>) {
    match chosen {
        None => (default_open, None),
        Some((false, then)) if tone_now >= 3 && tone_now > then => (true, Some((true, tone_now))),
        Some((false, then)) if tone_now < then => (false, Some((false, tone_now))),
        Some((open, _)) => (open, None),
    }
}

/// Le pliage d'un titre de section : `None` s'il ne se plie pas.
fn fold_of(b: &Block, local: &Local) -> Option<bool> {
    match b {
        Block::Section { key, fold, .. } if !key.is_empty() && *fold != Fold::No => {
            Some(local.open.get(key).copied().unwrap_or(*fold == Fold::Open))
        }
        _ => None,
    }
}

/// Pour chercher sans tenir compte de la casse ni des accents : « Œuvre élue » → « oeuvre elue ».
pub fn plain(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars().flat_map(char::to_lowercase) {
        match c {
            'à' | 'â' | 'ä' | 'á' | 'ã' | 'å' => out.push('a'),
            'ç' => out.push('c'),
            'é' | 'è' | 'ê' | 'ë' => out.push('e'),
            'î' | 'ï' | 'í' | 'ì' => out.push('i'),
            'ô' | 'ö' | 'ó' | 'ò' | 'õ' => out.push('o'),
            'ù' | 'û' | 'ü' | 'ú' => out.push('u'),
            'ÿ' | 'ý' => out.push('y'),
            'ñ' => out.push('n'),
            'œ' => out.push_str("oe"),
            'æ' => out.push_str("ae"),
            c => out.push(c),
        }
    }
    out
}

/// La ligne correspond à la recherche (`needle` passé par [`plain`]) : titre, sous-titre, texte à droite, pastille,
/// étiquettes.
fn hit(r: &Row, needle: &str) -> bool {
    let texts = [&r.title, &r.subtitle, &r.detail, &r.badge].into_iter().chain(r.chips.iter().map(|c| &c.text));
    texts.into_iter().any(|t| plain(t).contains(needle))
}

/// Les blocs montrés. Hors recherche : tout, sauf ce qui suit un bandeau plié jusqu'à la section suivante. Pendant une
/// recherche : les lignes qui correspondent, et les sections qui en ont, dépliées.
fn visible<'a>(s: &'a Screen, local: &Local, needle: &str) -> Vec<(&'a Block, Option<bool>)> {
    let mut out: Vec<(&Block, Option<bool>)> = Vec::with_capacity(s.blocks.len());
    if needle.is_empty() {
        let mut folded = false;
        for b in &s.blocks {
            if matches!(b, Block::Section { .. }) {
                let open = fold_of(b, local);
                folded = open == Some(false);
                out.push((b, open));
            } else if !folded {
                out.push((b, None));
            }
        }
        return out;
    }
    let mut section = None;
    for b in &s.blocks {
        match b {
            Block::Section { .. } => section = Some((b, fold_of(b, local).map(|_| true))),
            Block::Row(r) if hit(r, needle) => {
                out.extend(section.take());
                out.push((b, None));
            }
            _ => {}
        }
    }
    out
}

/// Les lignes de l'écran, dans l'ordre, avec leur clé et leur empreinte : un bandeau plié cache les blocs qui le
/// suivent jusqu'à la section suivante, une recherche ne garde que les lignes qui y répondent, et les cartes (premier
/// et dernier arrondis) se forment sur ce qui reste.
pub fn items<'a>(s: &'a Screen, local: &Local) -> Vec<Item<'a>> {
    let needle = plain(local.search.trim());
    let visible = visible(s, local, &needle);
    if visible.is_empty() && !needle.is_empty() {
        let text = format!("Aucun résultat pour « {} ».", local.search.trim());
        let mut h = DefaultHasher::new();
        text.hash(&mut h);
        return vec![Item { key: "@vide".into(), sig: h.finish(), what: What::NoMatch(text) }];
    }
    let is_row = |i: usize| matches!(visible.get(i), Some((Block::Row(_), _)));
    let mut keys = Keys::default();
    let mut out = Vec::with_capacity(visible.len());
    for (i, &(b, open)) in visible.iter().enumerate() {
        let mut h = DefaultHasher::new();
        hash_block(&mut h, b, local);
        let (base, what) = match b {
            Block::Section { title, key, .. } => {
                (open, needle.is_empty()).hash(&mut h);
                (format!("s:{}", if key.is_empty() { title } else { key }), What::Section { block: b, open })
            }
            Block::Row(r) => {
                let (first, last) = (i == 0 || !is_row(i - 1), !is_row(i + 1));
                (first, last).hash(&mut h);
                let base =
                    if r.key.is_empty() { format!("r:{}\u{1f}{}", r.title, r.glyph) } else { format!("r:{}", r.key) };
                (base, What::Row { row: r, first, last })
            }
            Block::Note { .. } => ("n".into(), What::Block(b)),
            Block::Meter { label, .. } => (format!("m:{label}"), What::Block(b)),
            Block::Text { .. } => ("t".into(), What::Block(b)),
            Block::Empty { .. } => ("e".into(), What::Block(b)),
            Block::Buttons { .. } => ("b".into(), What::Block(b)),
            Block::Diff { lines, numbers } => {
                let diff = keys.unique("d".into());
                let count = |k: u8| lines.iter().filter(|(kind, _)| *kind == k).count();
                let (added, removed, empty) = (count(DIFF_ADDED), count(DIFF_REMOVED), lines.is_empty());
                let mut h = DefaultHasher::new();
                (added, removed, empty, local.wrap).hash(&mut h);
                out.push(Item { key: diff.clone(), sig: h.finish(), what: What::DiffBar { added, removed, empty } });
                let widest = numbers.iter().map(|&(a, b)| a.max(b)).max().unwrap_or(0);
                let gutter = if numbers.len() == lines.len() && widest > 0 { widest.to_string().len() } else { 0 };
                for (n, (kind, text)) in lines.iter().enumerate() {
                    let last = n + 1 == lines.len();
                    let numbers = if gutter > 0 { numbers[n] } else { (0, 0) };
                    let mut h = DefaultHasher::new();
                    (kind, text, numbers, gutter, last, local.wrap).hash(&mut h);
                    out.push(Item {
                        key: format!("{diff}:{n}"),
                        sig: h.finish(),
                        what: What::DiffLine { kind: *kind, text, numbers, gutter, last },
                    });
                }
                continue;
            }
        };
        out.push(Item { key: keys.unique(base), sig: h.finish(), what });
    }
    out
}

fn empty(kind: i32, title: &str) -> UiRow {
    UiRow { kind, title: SharedString::from(title), acts: acts(&[], &Local::default()), ..Default::default() }
}

/// Largeur d'une voie du graphe des commits.
pub const LANE_W: f32 = 14.0;
/// Hauteur d'une ligne de commit : le graphe en occupe toute la hauteur, les traits des lignes voisines se raccordent.
pub const COMMIT_H: f32 = 56.0;
/// Marge de part et d'autre des voies.
const GRAPH_PAD: f32 = 6.0;
/// Voies dessinées au plus (au-delà, les traits sont coupés au bord de la colonne).
pub const MAX_LANES: u8 = 8;

/// Abscisse du milieu d'une voie, dans la colonne du graphe.
fn lane_x(lane: u8) -> f32 {
    GRAPH_PAD + LANE_W / 2.0 + f32::from(lane) * LANE_W
}

/// Largeur de la colonne du graphe.
fn graph_w(lanes: u8) -> f32 {
    2.0 * GRAPH_PAD + f32::from(lanes.clamp(1, MAX_LANES)) * LANE_W
}

/// Un trait du graphe, en commandes de chemin SVG (pixels de la cellule) : droit entre deux points d'une même voie,
/// sinon la courbe de DragonTree (`graph_paint.rs`, points de contrôle à 0,6 de la hauteur). `GRAPH_TOP` : du haut de
/// la ligne au nœud ; `GRAPH_BOTTOM` : du nœud au bas ; `GRAPH_THROUGH` : toute la hauteur.
pub fn graph_path(part: u8, from: u8, to: u8) -> String {
    let (h, mid) = (COMMIT_H, COMMIT_H / 2.0);
    let (x1, y1, x2, y2) = match part {
        GRAPH_TOP => (lane_x(from), 0.0, lane_x(to), mid),
        GRAPH_BOTTOM => (lane_x(from), mid, lane_x(to), h),
        _ => (lane_x(from), 0.0, lane_x(to), h),
    };
    // Au dixième de pixel : des commandes courtes et lisibles.
    let px = |v: f32| (v * 10.0).round() / 10.0;
    let (x1, y1, x2, y2) = (px(x1), px(y1), px(x2), px(y2));
    if (x1 - x2).abs() < 0.5 {
        format!("M {x1} {y1} L {x2} {y2}")
    } else {
        let dy = y2 - y1;
        format!("M {x1} {y1} C {x1} {} {x2} {} {x2} {y2}", px(y1 + dy * 0.6), px(y2 - dy * 0.6))
    }
}

/// La part du graphe d'une ligne de commit. Le nœud : -1 sans nœud, sinon sa sorte (`NODE_COMMIT`…).
fn graph(ui: &mut UiRow, g: &GraphCell) {
    ui.graph = true;
    ui.graph_w = graph_w(g.lanes);
    ui.node = if g.node == NODE_NONE { -1 } else { i32::from(g.kind) };
    ui.node_x = lane_x(g.node.min(MAX_LANES - 1));
    ui.node_lane = i32::from(g.color);
    let segs = g
        .lines
        .iter()
        .map(|&(part, from, to, color)| GraphSeg { d: graph_path(part, from, to).into(), lane: i32::from(color) });
    ui.segs = ModelRc::new(VecModel::from(segs.collect::<Vec<_>>()));
}

/// Étiquettes montrées au plus ; les autres sont comptées (« +2 »).
const CHIPS_SHOWN: usize = 3;
/// Sur une ligne de commit, les étiquettes partagent la ligne du sous-titre : deux au plus.
const COMMIT_CHIPS_SHOWN: usize = 2;

/// Les étiquettes d'une ligne, `shown` au plus, puis le compte des autres. Le PC borne parfois déjà les siennes : son
/// « +N » final (sans glyphe) compte celles qu'il n'a pas envoyées, et s'ajoute au compte.
fn chips(list: &[Chip], shown: usize) -> ModelRc<UiChip> {
    let counted = |c: &Chip| c.glyph.is_empty().then(|| c.text.strip_prefix('+')?.parse::<usize>().ok()).flatten();
    let (named, already) = match list.split_last() {
        Some((last, rest)) => counted(last).map_or((list, 0), |n| (rest, n)),
        None => (list, 0),
    };
    let mut out: Vec<UiChip> = named
        .iter()
        .take(shown)
        .map(|c| UiChip {
            text: c.text.as_str().into(),
            glyph: c.glyph.as_str().into(),
            color: c.color.map(rgb).unwrap_or_default(),
            has_color: c.color.is_some(),
            tone: tone(c.tone),
            strong: c.strong,
        })
        .collect();
    let more = named.len().saturating_sub(shown) + already;
    if more > 0 {
        out.push(UiChip { text: format!("+{more}").into(), ..Default::default() });
    }
    ModelRc::new(VecModel::from(out))
}

/// Une progression en pour cent ([`PROGRESS_UNKNOWN`] : indéterminée) dans une ligne : la barre, et sa valeur.
fn progress(ui: &mut UiRow, p: Option<u8>) {
    if let Some(p) = p {
        ui.has_progress = true;
        ui.indeterminate = p > 100 || p == PROGRESS_UNKNOWN;
        ui.value = f32::from(p.min(100)) / 100.0;
    }
}

fn row(r: &Row, first: bool, last: bool, local: &Local) -> UiRow {
    let mut ui = UiRow {
        kind: 1,
        title: r.title.as_str().into(),
        // Le sous-titre passe à la ligne : « il y a 1 h » ne s'y coupe pas.
        subtitle: tied(&r.subtitle).into(),
        detail: r.detail.as_str().into(),
        glyph: if r.glyph.is_empty() { "•".into() } else { r.glyph.as_str().into() },
        color: r.color.map(rgb).unwrap_or_default(),
        has_color: r.color.is_some(),
        tone: tone(r.tone),
        badge: r.badge.as_str().into(),
        path: r.open.clone().unwrap_or_default().into(),
        opens: r.open.is_some(),
        tap: r.tap.as_ref().map(|a| act_in(a, local)).unwrap_or_default(),
        has_tap: r.tap.is_some(),
        trailing: r.trailing.as_ref().map(|a| act_in(a, local)).unwrap_or_default(),
        has_trailing: r.trailing.is_some(),
        acts: menu_acts(&r.acts, local),
        value: 0.0,
        mono: false,
        first,
        last,
        busy: r.tap.iter().chain(&r.acts).any(|a| local.busy(a)),
        chips: chips(&r.chips, if r.graph.is_some() { COMMIT_CHIPS_SHOWN } else { CHIPS_SHOWN }),
        ..Default::default()
    };
    if let Some(g) = &r.graph {
        graph(&mut ui, g);
    }
    progress(&mut ui, r.progress);
    ui
}

/// Un bandeau (`kind` 8 : pliable, ou orné d'une couleur, d'un glyphe, d'une pastille, d'un menu), sinon un simple
/// titre de section (`kind` 0). Pendant une recherche, un bandeau pliable est ouvert et ne se plie pas (`fold` 3) :
/// son pliage, caché, changerait sans que rien ne bouge à l'écran.
fn section(b: &Block, open: Option<bool>, local: &Local) -> UiRow {
    let Block::Section { title, aside, key, color, glyph, badge, tone: t, acts: list, .. } = b else {
        return empty(5, "");
    };
    let plain = open.is_none() && color.is_none() && glyph.is_empty() && badge.is_empty() && list.is_empty();
    if plain {
        return UiRow { detail: aside.as_str().into(), ..empty(0, &title.to_uppercase()) };
    }
    UiRow {
        kind: 8,
        key: key.as_str().into(),
        title: title.as_str().into(),
        detail: aside.as_str().into(),
        glyph: glyph.as_str().into(),
        color: color.map(rgb).unwrap_or_default(),
        has_color: color.is_some(),
        badge: badge.as_str().into(),
        tone: tone(*t),
        acts: menu_acts(list, local),
        fold: match open {
            None => 0,
            Some(_) if !local.search.trim().is_empty() => 3,
            Some(true) => 1,
            Some(false) => 2,
        },
        busy: list.iter().any(|a| local.busy(a)),
        ..Default::default()
    }
}

/// La ligne Slint d'une ligne de la liste.
pub fn ui_row(item: &Item, local: &Local) -> UiRow {
    match &item.what {
        What::Section { block, open } => section(block, *open, local),
        What::Row { row: r, first, last } => row(r, *first, *last, local),
        What::Block(Block::Note { text, tone: t, progress: p, acts: list }) => {
            let mut ui = UiRow { tone: tone(Some(*t)), acts: acts(list, local), ..empty(2, text) };
            progress(&mut ui, *p);
            ui
        }
        What::Block(Block::Meter { label, value, text, tone: t }) => {
            UiRow { detail: text.as_str().into(), value: *value, tone: tone(Some(*t)), ..empty(3, label) }
        }
        What::Block(Block::Text { text, mono }) => UiRow { mono: *mono, ..empty(4, text) },
        What::Block(Block::Empty { text }) => empty(5, text),
        What::Block(Block::Buttons { acts: list }) => UiRow { acts: acts(list, local), ..empty(6, "") },
        What::DiffBar { added, removed, empty: no_lines } => UiRow {
            title: format!("+{added}  −{removed}").into(),
            first: true,
            last: *no_lines,
            wrap: local.wrap,
            ..empty(9, "")
        },
        What::DiffLine { kind, text, numbers: (old, new), gutter, last } => {
            let number = |n: &u32| if *n == 0 { SharedString::new() } else { n.to_string().into() };
            UiRow {
                tone: i32::from(*kind),
                subtitle: number(old),
                detail: number(new),
                gutter: *gutter as i32,
                last: *last,
                wrap: local.wrap,
                ..empty(7, text)
            }
        }
        What::NoMatch(text) => empty(5, text),
        What::Block(_) => empty(5, ""),
    }
}

/// Toutes les lignes Slint de l'écran.
pub fn rows(s: &Screen, local: &Local) -> Vec<UiRow> {
    items(s, local).iter().map(|i| ui_row(i, local)).collect()
}

/// Au-delà, un écran qui change de fond en comble est remplacé d'un coup plutôt que ligne à ligne.
const PATCH_MAX: usize = 400;

/// Met `model` à jour pour qu'il montre `new`, sans toucher aux lignes inchangées : les lignes de même clé au début et
/// à la fin sont gardées (et redessinées si leur empreinte a changé), le milieu est remplacé. `shown` : les clés et
/// empreintes que montre `model`, remises à jour. Rend le nombre de lignes dessinées.
pub fn patch(
    model: &VecModel<UiRow>,
    shown: &mut Vec<(String, u64)>,
    new: &[Item],
    build: impl Fn(&Item) -> UiRow,
) -> usize {
    let (n_old, n_new) = (shown.len(), new.len());
    let lim = n_old.min(n_new);
    let pre = (0..lim).take_while(|&i| shown[i].0 == new[i].key).count();
    let suf = (0..lim - pre).take_while(|&j| shown[n_old - 1 - j].0 == new[n_new - 1 - j].key).count();
    let (old_mid, new_mid) = (n_old - pre - suf, n_new - pre - suf);
    let mut built = 0;
    if model.row_count() != n_old || old_mid.min(new_mid) > 0 && old_mid + new_mid > PATCH_MAX {
        model.set_vec(new.iter().map(&build).collect::<Vec<_>>());
        built = n_new;
    } else {
        for i in 0..pre {
            if shown[i].1 != new[i].sig {
                model.set_row_data(i, build(&new[i]));
                built += 1;
            }
        }
        for _ in 0..old_mid {
            model.remove(pre);
        }
        for (k, it) in new[pre..pre + new_mid].iter().enumerate() {
            model.insert(pre + k, build(it));
            built += 1;
        }
        for j in 0..suf {
            let (o, n) = (n_old - suf + j, n_new - suf + j);
            if shown[o].1 != new[n].sig {
                model.set_row_data(n, build(&new[n]));
                built += 1;
            }
        }
    }
    *shown = new.iter().map(|i| (i.key.clone(), i.sig)).collect();
    built
}

/// Met `model` à jour pour qu'il montre `new`, en ne remplaçant que les éléments qui diffèrent (barre du bas, cartes
/// des modules, ce qui attend, notifications) : les éléments affichés ne sont pas recréés.
pub fn sync<T: Clone + PartialEq + 'static>(model: &VecModel<T>, new: Vec<T>) {
    let old = model.row_count();
    for (i, v) in new.iter().enumerate().take(old) {
        if model.row_data(i).as_ref() != Some(v) {
            model.set_row_data(i, v.clone());
        }
    }
    if new.len() > old {
        model.extend(new.into_iter().skip(old));
    } else {
        for i in (new.len()..old).rev() {
            model.remove(i);
        }
    }
}

/// De quand datent les données montrées hors ligne : « Données reçues à l'instant », « Données d'il y a 3 min ».
pub fn data_age(at: u64, now: u64) -> String {
    match ago(at, now) {
        a if now.saturating_sub(at) < 60 => format!("Données reçues {a}"),
        a => format!("Données d'il y a {a}"),
    }
}

/// La clé d'une notification du PC (pour la déplier sur le téléphone).
pub fn note_key(at: u64, module: &str, title: &str) -> String {
    format!("{at}\u{1f}{module}\u{1f}{title}")
}

/// Les espaces que le français veut insécables (dans « », avant : ; ? !, entre un nombre et son unité) et celles
/// d'« il y a 3 h » : un texte qui passe à la ligne ne laisse pas un « » » ou un « : » seul en tête de ligne, ni
/// « il » seul en fin de ligne. L'espace fine insécable (U+202F), pas l'espace insécable (U+00A0) : quand celle-ci
/// tombe au bord de la ligne, la mise en page de Slint (parley 0.11) y coupe quand même.
pub fn tied(text: &str) -> String {
    let mut out = text.replace("« ", "«\u{202f}");
    for mark in [" »", " :", " ;", " ?", " !"] {
        out = out.replace(mark, &mark.replacen(' ', "\u{202f}", 1));
    }
    durations(&out)
}

/// Les unités qu'une durée met après son nombre (« 3 min », « 2 h », « 4 j »).
const UNITS: [&str; 3] = ["min", "h", "j"];

/// Une durée d'un seul tenant : un nombre et son unité, « il y a » et le nombre qui le suit.
fn durations(text: &str) -> String {
    let w: Vec<&str> = text.split(' ').collect();
    let unit = |s: &str| UNITS.iter().any(|u| s.strip_prefix(u).is_some_and(|r| !r.starts_with(char::is_alphanumeric)));
    // « il y a 3 » commence au mot `j`.
    let ago = |j: usize| {
        w.get(j..j + 4).is_some_and(|s| {
            s[0].eq_ignore_ascii_case("il")
                && s[1] == "y"
                && s[2] == "a"
                && s[3].starts_with(|c: char| c.is_ascii_digit())
        })
    };
    let mut out = String::with_capacity(text.len());
    for (i, word) in w.iter().enumerate() {
        if i > 0 {
            let tie =
                (w[i - 1].ends_with(|c: char| c.is_ascii_digit()) && unit(word)) || (i.saturating_sub(3)..i).any(ago);
            out.push(if tie { '\u{202f}' } else { ' ' });
        }
        out.push_str(word);
    }
    out
}

/// Un mot plus court tient sur une ligne d'un dialogue, même à 360 px (un nom de branche, de dépôt).
const DIALOG_WORD: usize = 24;

/// Le texte d'un dialogue venu du PC (une question, le titre d'une saisie) : [`tied`], et un mot court à trait
/// d'union (« mobile-ergonomie ») d'un seul tenant, par le trait d'union insécable (U+2011) : Slint coupait après le
/// trait d'union alors que le mot entier tenait sur la ligne suivante. Un mot plus long garde ses traits d'union, où
/// il passe à la ligne.
pub fn dialog(text: &str) -> String {
    let text = tied(text);
    let mut out = String::with_capacity(text.len());
    for chunk in text.split_inclusive(char::is_whitespace) {
        let word = chunk.trim_end();
        if word.contains('-') && word.chars().count() <= DIALOG_WORD {
            out.push_str(&chunk.replace('-', "\u{2011}"));
        } else {
            out.push_str(chunk);
        }
    }
    out
}

/// « il y a » : à l'instant, 5 min, 2 h, 3 j.
pub fn ago(at: u64, now: u64) -> String {
    match now.saturating_sub(at) {
        0..=59 => "à l'instant".into(),
        s @ 60..=3599 => format!("{} min", s / 60),
        s @ 3600..=86_399 => format!("{} h", s / 3600),
        s => format!("{} j", s / 86_400),
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use suite_mobile::ui::{Band, DIFF_CONTEXT, DIFF_HEADER, GRAPH_THROUGH, NODE_HEAD};

    use super::*;

    #[test]
    fn french_punctuation_stays_with_its_word() {
        let t = tied("Démarrage refusé : ce téléphone n'a pas le droit « Agir » sur le PC ; vraiment ? oui !");
        assert_eq!(
            t,
            "Démarrage refusé\u{202f}: ce téléphone n'a pas le droit «\u{202f}Agir\u{202f}» sur le PC\u{202f}; \
             vraiment\u{202f}? oui\u{202f}!"
        );
        assert_eq!(tied(r"C:\Users\aubin › Réglages"), r"C:\Users\aubin › Réglages", "rien d'autre ne change");
    }

    /// « il y a 1 h » ne se coupe pas : le sous-titre d'une branche laissait « il » seul en fin de ligne. Une durée
    /// reste avec son unité, et « il y a » avec elle.
    #[test]
    fn a_duration_stays_in_one_piece() {
        assert_eq!(tied("Vu il y a 12 min."), "Vu il\u{202f}y\u{202f}a\u{202f}12\u{202f}min.");
        assert_eq!(
            tied("Chaque jour à 8 h · Il y a 3 j"),
            "Chaque jour à 8\u{202f}h · Il\u{202f}y\u{202f}a\u{202f}3\u{202f}j"
        );
        let same = "s'il y a lieu, 2 tests, en 3 heures, 5 hôtes";
        assert_eq!(tied(same), same, "rien d'autre ne change");
        let mut s = Screen::new("Branches");
        s.row(Row::new("mobile-ergonomie").subtitle("origin/mobile-ergonomie · ↑2 ↓1 · il y a 1 h"));
        let r = rows(&s, &Local::default());
        assert_eq!(r[0].subtitle, "origin/mobile-ergonomie · ↑2 ↓1 · il\u{202f}y\u{202f}a\u{202f}1\u{202f}h");
    }

    /// Une question du PC passe à la ligne sans laisser « ? » seul en tête de ligne, ni couper un nom de branche après
    /// son trait d'union (« mobile- », puis « ergonomie ? ») quand il tient entier sur la ligne suivante.
    #[test]
    fn a_dialog_keeps_short_names_and_their_punctuation_whole() {
        let merge = Act::new("fusionner:main", "Fusionner dans mobile-ergonomie")
            .confirm("Fusionner main dans mobile-ergonomie ?")
            .input("Nouveau nom de « mobile-ergonomie » :", "");
        let a = act(&merge);
        assert_eq!(a.confirm, "Fusionner main dans mobile\u{2011}ergonomie\u{202f}?");
        assert_eq!(a.input_label, "Nouveau nom de «\u{202f}mobile\u{2011}ergonomie\u{202f}»\u{202f}:");
        assert_eq!(a.label, "Fusionner dans mobile-ergonomie", "un bouton ne passe pas à la ligne");
        assert_eq!(
            dialog("Supprimer feature/achedotal/64522-correctif-veille ?"),
            "Supprimer feature/achedotal/64522-correctif-veille\u{202f}?",
            "trop long pour une ligne : il se coupe à ses traits d'union"
        );
    }

    #[test]
    fn consecutive_rows_share_a_card() {
        let mut s = Screen::new("x");
        s.section("Tâches", "2")
            .row(Row::new("a").open("t/a"))
            .row(Row::new("b").tap(Act::new("lancer", "Lancer")).act(Act::new("x", "X")))
            .note("attention", Tone::Warn)
            .row(Row::new("c"));
        let r = rows(&s, &Local::default());
        assert_eq!(r.len(), 5);
        assert_eq!((r[0].kind, r[0].title.as_str(), r[0].detail.as_str()), (0, "TÂCHES", "2"));
        assert!(r[1].first && !r[1].last && r[1].opens && r[1].path == "t/a");
        assert!(!r[2].first && r[2].last && r[2].has_tap && r[2].acts.row_count() == 1);
        assert_eq!((r[3].kind, r[3].tone), (2, 3));
        assert!(r[4].first && r[4].last, "seule dans sa carte");
    }

    #[test]
    fn a_diff_becomes_one_row_per_line() {
        let mut s = Screen::new("x");
        s.diff(vec![(3, "@@ -1 +1 @@".into()), (2, "-a".into()), (1, "+b".into())]);
        let r = rows(&s, &Local::default());
        let kinds = r.iter().map(|r| (r.kind, r.tone)).collect::<Vec<_>>();
        assert_eq!(kinds, [(9, 0), (7, 3), (7, 2), (7, 1)], "l'en-tête du diff, puis ses lignes");
        assert_eq!(r[0].title, "+1  −1");
        assert!(r[0].first && !r[0].last && r[3].last);
        assert_eq!(r[1].gutter, 0, "pas de numéros");
    }

    /// Les numéros de ligne prennent la largeur du plus grand ; 0 : pas de numéro ; le retour à la ligne, choisi sur
    /// le téléphone, redessine les lignes du diff.
    #[test]
    fn diff_lines_carry_their_numbers_and_the_wrap_choice() {
        let mut s = Screen::new("x");
        let lines =
            vec![(DIFF_HEADER, "@@ -98,2 +98,3 @@".into()), (DIFF_CONTEXT, " a".into()), (DIFF_ADDED, "+b".into())];
        s.diff_numbered(lines, vec![(0, 0), (99, 99), (0, 100)]);
        let flat = Local::default();
        let r = rows(&s, &flat);
        assert_eq!(r[2].gutter, 3);
        assert_eq!((r[2].subtitle.as_str(), r[2].detail.as_str()), ("99", "99"));
        assert_eq!((r[3].subtitle.as_str(), r[3].detail.as_str()), ("", "100"));
        assert!(!r[3].wrap);
        let wrapped = Local { wrap: true, ..Default::default() };
        let sigs = |l: &Local| items(&s, l).into_iter().map(|i| i.sig).collect::<Vec<_>>();
        assert!(sigs(&flat).iter().zip(sigs(&wrapped)).all(|(a, b)| *a != b), "tout le diff est redessiné");
    }

    #[test]
    fn actions_keep_what_the_phone_needs() {
        let a = act(&Act::new("tout", "Tout arrêter").danger().confirm("Sûr ?").input("Pourquoi", "…"));
        assert_eq!((a.style, a.confirm.as_str(), a.has_input), (2, "Sûr\u{202f}?", true));
        assert_eq!(act(&Act::link("Ouvrir", "https://x.org")).url, "https://x.org");
        assert_eq!(ago(100, 100), "à l'instant");
        assert_eq!(ago(0, 7300), "2 h");
        assert_eq!(data_age(100, 130), "Données reçues à l'instant");
        assert_eq!(data_age(100, 100 + 180), "Données d'il y a 3 min");
    }

    /// Les clés : `Row.key`, sinon titre et glyphe ; deux lignes pareilles restent distinctes.
    #[test]
    fn every_line_has_a_stable_unique_key() {
        let mut s = Screen::new("x");
        s.section("Ozmo", "2")
            .row(Row::new("pwsh").glyph("›_"))
            .row(Row::new("pwsh").glyph("›_"))
            .row(Row::new("Claude").key("t/3"))
            .diff(vec![(0, "a".into()), (1, "+b".into())]);
        let keys: Vec<String> = items(&s, &Local::default()).into_iter().map(|i| i.key).collect();
        assert_eq!(keys, ["s:Ozmo", "r:pwsh\u{1f}›_", "r:pwsh\u{1f}›_#2", "r:t/3", "d", "d:0", "d:1"]);
    }

    fn screen(progress: u8, extra: bool) -> Screen {
        let mut s = Screen::new("DragonTree");
        s.note(format!("Fetch… {progress} %"), Tone::Info).section("Branches", "3");
        s.row(Row::new("main").key("b/main")).row(Row::new("dev").key("b/dev"));
        if extra {
            s.row(Row::new("feature/x").key("b/x"));
        }
        s.row(Row::new("release").key("b/release"));
        s
    }

    fn titles(m: &VecModel<UiRow>) -> Vec<String> {
        m.iter().map(|r| r.title.to_string()).collect()
    }

    /// Une progression qui avance ne redessine que sa note ; une ligne ajoutée ou retirée au milieu ne touche pas aux
    /// autres (une ligne touchée reste la même instance dans la liste).
    #[test]
    fn an_update_redraws_only_what_changed() {
        let model = VecModel::<UiRow>::default();
        let mut shown = Vec::new();
        let count = Cell::new(0);
        let local = Local::default();
        let build = |i: &Item| {
            count.set(count.get() + 1);
            ui_row(i, &local)
        };
        let first = screen(10, false);
        assert_eq!(patch(&model, &mut shown, &items(&first, &local), build), 5);
        assert_eq!(titles(&model), ["Fetch… 10 %", "BRANCHES", "main", "dev", "release"]);

        count.set(0);
        let next = screen(42, false);
        assert_eq!(patch(&model, &mut shown, &items(&next, &local), build), 1, "la note seule");
        assert_eq!(model.row_data(0).unwrap().title, "Fetch… 42 %");

        let more = screen(42, true);
        assert_eq!(patch(&model, &mut shown, &items(&more, &local), build), 1, "la ligne ajoutée seule");
        assert_eq!(titles(&model), ["Fetch… 42 %", "BRANCHES", "main", "dev", "feature/x", "release"]);

        assert_eq!(patch(&model, &mut shown, &items(&next, &local), build), 0, "retirée : rien à redessiner");
        assert_eq!(titles(&model), ["Fetch… 42 %", "BRANCHES", "main", "dev", "release"]);
        assert_eq!(patch(&model, &mut shown, &items(&next, &local), build), 0, "le même écran : rien");

        let mut fewer = Screen::new("DragonTree");
        fewer.note("Fetch… 42 %", Tone::Info).section("Branches", "3");
        fewer.row(Row::new("main").key("b/main")).row(Row::new("dev").key("b/dev"));
        assert_eq!(
            patch(&model, &mut shown, &items(&fewer, &local), build),
            1,
            "`dev` devient la dernière de sa carte"
        );
        assert!(model.row_data(3).unwrap().last);
    }

    /// Une action envoyée et sans réponse, ou que le module fait déjà, est grisée avec un indicateur, où qu'elle soit ;
    /// seules les lignes qui la portent sont redessinées quand elle part ou revient.
    #[test]
    fn a_pending_action_is_busy_wherever_it_is() {
        let mut s = Screen::new("Dépôt");
        s.buttons(vec![Act::new("fetch", "Fetch"), Act::new("push", "Push").busy(true)]);
        s.row(Row::new("main").key("b/main").tap(Act::new("basculer:main", "Basculer")));
        s.row(Row::new("dev").key("b/dev").act(Act::new("supprimer:dev", "Supprimer")));
        let idle = Local::default();
        let busy = |l: &Local| {
            let r = rows(&s, l);
            let buttons: Vec<bool> = r[0].acts.iter().map(|a| a.busy).collect();
            (buttons, r[1].busy, r[1].tap.busy, r[2].busy)
        };
        assert_eq!(busy(&idle), (vec![false, true], false, false, false), "Push : le module le fait déjà");
        let mut sent = Local::default();
        sent.pending.extend(["fetch".to_string(), "supprimer:dev".to_string()]);
        assert_eq!(busy(&sent), (vec![true, true], false, false, true));

        let model = VecModel::<UiRow>::default();
        let mut shown = Vec::new();
        patch(&model, &mut shown, &items(&s, &idle), |i| ui_row(i, &idle));
        let built = patch(&model, &mut shown, &items(&s, &sent), |i| ui_row(i, &sent));
        assert_eq!(built, 2, "les boutons et la ligne `dev`");
    }

    fn bands() -> Screen {
        let mut s = Screen::new("Fawkes");
        s.row(Row::new("Nouveau terminal").open("nouveau"));
        s.band(Band::new("g1", "Eyrie").badge("1 attend").tone(Tone::Warn).act(Act::new("veille:g1", "Veille")));
        s.row(Row::new("Claude").key("t/3")).row(Row::new("pwsh").key("t/4"));
        s.band(Band::new("g2", "Ozmo").open(false)).row(Row::new("Git Bash").key("t/7"));
        s.note("Une note du groupe Ozmo", Tone::Info);
        s.section("Récents", "").row(Row::new("Eyrie").key("r/1"));
        s
    }

    fn shown(s: &Screen, local: &Local) -> Vec<String> {
        items(s, local).into_iter().map(|i| i.key).collect()
    }

    /// Un bandeau plié cache ce qui le suit jusqu'à la section suivante ; le choix fait sur le téléphone l'emporte sur
    /// l'état de départ du PC ; un simple titre ne se plie pas.
    #[test]
    fn a_folded_band_hides_what_follows_it_up_to_the_next_section() {
        let s = bands();
        let mut local = Local::default();
        assert_eq!(
            shown(&s, &local),
            ["r:Nouveau terminal\u{1f}", "s:g1", "r:t/3", "r:t/4", "s:g2", "s:Récents", "r:r/1"]
        );
        let r = rows(&s, &local);
        assert_eq!((r[1].kind, r[1].fold, r[1].title.as_str(), r[1].tone), (8, 1, "Eyrie", 3));
        assert_eq!((r[4].fold, r[4].key.as_str()), (2, "g2"));
        assert_eq!((r[5].kind, r[5].title.as_str()), (0, "RÉCENTS"), "un simple titre");

        local.open.insert("g1".into(), false);
        local.open.insert("g2".into(), true);
        assert_eq!(shown(&s, &local), ["r:Nouveau terminal\u{1f}", "s:g1", "s:g2", "r:t/7", "n", "s:Récents", "r:r/1"]);
    }

    /// Les cartes se forment sur ce qui est montré : la ligne avant un bandeau plié reste la dernière de sa carte même
    /// si des lignes cachées la suivent.
    #[test]
    fn cards_are_rounded_after_folding() {
        let mut s = Screen::new("x");
        s.row(Row::new("a")).band(Band::new("k", "Plié").open(false)).row(Row::new("b"));
        s.band(Band::new("k2", "Ouvert").fixed()).row(Row::new("c")).row(Row::new("d"));
        let r = rows(&s, &Local::default());
        assert_eq!(r.iter().map(|r| r.title.to_string()).collect::<Vec<_>>(), ["a", "Plié", "OUVERT", "c", "d"]);
        assert!(r[0].first && r[0].last);
        assert!(r[3].first && !r[3].last && r[4].last);
        assert_eq!(r[2].kind, 0, "qui ne se plie pas, sans ornement : un simple titre");
    }

    /// L'alerte gagne : plié en Info, un bandeau qui passe à Attention se rouvre (une fois) ; plié en Attention, il
    /// ne se rouvre que s'il passe à Danger.
    #[test]
    fn a_folded_band_reopens_once_when_it_becomes_alarming() {
        assert_eq!(band_open(None, true, 4), (true, None), "l'état de départ du PC");
        assert_eq!(band_open(None, false, 4), (false, None));
        assert_eq!(band_open(Some((false, 1)), true, 1), (false, None));
        assert_eq!(band_open(Some((false, 1)), true, 3), (true, Some((true, 3))));
        assert_eq!(band_open(Some((false, 3)), true, 3), (false, None), "replié par l'utilisateur en connaissance");
        assert_eq!(band_open(Some((false, 3)), true, 4), (true, Some((true, 4))));
        assert_eq!(band_open(Some((true, 0)), false, 0), (true, None));
    }

    /// Ce que le téléphone retient d'un bandeau, écran après écran reçu du PC : `chosen` suit le choix retenu.
    fn shown_open(chosen: &mut Option<(bool, i32)>, tone_now: i32) -> bool {
        let (open, keep) = band_open(*chosen, true, tone_now);
        if keep.is_some() {
            *chosen = keep;
        }
        open
    }

    /// Plié pendant qu'une session attend (Fawkes), un groupe reste plié tant qu'elle attend ; il se calme, puis une
    /// autre session attend : il se rouvre. Replié pendant cette alerte, il se rouvrira à la suivante.
    #[test]
    fn a_band_folded_during_an_alert_reopens_at_the_next_one() {
        let mut chosen = Some((false, 3));
        assert!(!shown_open(&mut chosen, 3), "replié en connaissance de cause");
        assert!(!shown_open(&mut chosen, 0), "calme : reste plié");
        assert_eq!(chosen, Some((false, 0)), "le ton retenu baisse avec lui");
        assert!(shown_open(&mut chosen, 3), "une nouvelle attente : rouvert");
        assert_eq!(chosen, Some((true, 3)));
        chosen = Some((false, 3));
        assert!(!shown_open(&mut chosen, 3));
        assert!(!shown_open(&mut chosen, 1));
        assert!(shown_open(&mut chosen, 4), "plus grave encore");
    }

    /// La recherche, sans casse ni accents, garde les lignes qui y répondent (titre, sous-titre, texte à droite,
    /// pastille, étiquettes) et les sections qui en ont, dépliées ; le reste est caché ; rien : un message.
    #[test]
    fn the_search_keeps_matching_rows_and_their_sections_unfolded() {
        let mut s = bands();
        s.row(Row::new("Dépôt").chip(Chip::new("release/été")));
        let mut local = Local { search: "  GIT ".into(), ..Default::default() };
        assert_eq!(shown(&s, &local), ["s:g2", "r:t/7"], "Ozmo, plié au départ, est déplié");
        assert_eq!(rows(&s, &local)[0].fold, 3, "ouvert le temps de la recherche : il ne se plie pas");
        let sig = |l: &Local| items(&s, l).into_iter().find(|i| i.key == "s:g2").map(|i| i.sig);
        let open = Local { open: [("g2".to_string(), true)].into(), ..Default::default() };
        let open_searching = Local { search: "git".into(), ..open.clone() };
        assert_ne!(sig(&open), sig(&open_searching), "le bandeau est redessiné quand la recherche commence");
        local.search = "ete".into();
        assert_eq!(shown(&s, &local), ["s:Récents", "r:Dépôt\u{1f}"], "dans une étiquette, sans accent");
        local.search = "claude".into();
        assert_eq!(shown(&s, &local), ["s:g1", "r:t/3"]);
        local.search = "zzz".into();
        let r = rows(&s, &local);
        assert_eq!((r.len(), r[0].kind, r[0].title.as_str()), (1, 5, "Aucun résultat pour « zzz »."));
        assert_eq!(plain("Œuvre ÉLUE à Noël"), "oeuvre elue a noel");
    }

    /// Les traits du graphe se raccordent d'une ligne à l'autre : un trait vertical va du haut au bas de la ligne, une
    /// courbe part du nœud (au milieu) avec les points de contrôle de DragonTree.
    #[test]
    fn graph_lines_join_from_row_to_row() {
        assert_eq!(graph_path(GRAPH_THROUGH, 1, 1), "M 27 0 L 27 56");
        assert_eq!(graph_path(GRAPH_BOTTOM, 0, 1), "M 13 28 C 13 44.8 27 39.2 27 56");
        assert_eq!(graph_path(GRAPH_TOP, 1, 0), "M 27 0 C 27 16.8 13 11.2 13 28");
        assert_eq!(graph_path(GRAPH_TOP, 2, 2), "M 41 0 L 41 28");
    }

    fn chip_texts(r: &UiRow) -> Vec<String> {
        r.chips.iter().map(|c| c.text.to_string()).collect()
    }

    /// Une ligne de commit : colonne à la largeur de ses voies (bornée), nœud et traits avec leurs couleurs ; deux
    /// étiquettes au plus (elles partagent la ligne du sous-titre), les autres comptées ; sa pastille (« +3 » fusions
    /// repliées) lui reste.
    #[test]
    fn a_commit_row_carries_its_graph_and_chips() {
        let cell = GraphCell {
            lanes: 2,
            node: 1,
            color: 3,
            kind: NODE_HEAD,
            lines: vec![(GRAPH_TOP, 1, 1, 3), (GRAPH_BOTTOM, 1, 0, 5), (GRAPH_THROUGH, 0, 0, 2)],
        };
        let mut s = Screen::new("Historique");
        s.row(
            Row::new("Corrige le push")
                .key("abc123")
                .graph(cell)
                .badge("+3")
                .chip(Chip::new("main").strong())
                .chip(Chip::new("origin/main"))
                .chip(Chip::new("v1.2"))
                .chip(Chip::new("v1.2.1"))
                .chip(Chip::new("v1.2.2")),
        );
        s.row(Row::new("Sans graphe").graph(GraphCell { lanes: 40, node: NODE_NONE, ..Default::default() }));
        let r = rows(&s, &Local::default());
        assert!(r[0].graph && r[0].node == i32::from(NODE_HEAD) && r[0].node_lane == 3);
        assert_eq!((r[0].graph_w, r[0].node_x), (40.0, 27.0));
        let lanes: Vec<i32> = r[0].segs.iter().map(|g| g.lane).collect();
        assert_eq!(lanes, [3, 5, 2]);
        assert_eq!(chip_texts(&r[0]), ["main", "origin/main", "+3"]);
        assert!(r[0].chips.row_data(0).unwrap().strong);
        assert_eq!(r[0].badge, "+3");
        assert_eq!((r[1].node, r[1].graph_w), (-1, 2.0 * GRAPH_PAD + 8.0 * LANE_W), "huit voies au plus");
    }

    /// Le « +N » que le PC met après les étiquettes qu'il a bornées s'ajoute au compte du téléphone : rien de caché
    /// n'est oublié, et il n'est pas compté comme une étiquette.
    #[test]
    fn the_pc_count_of_hidden_chips_adds_to_the_phone_count() {
        let row = |chips: &[&str]| {
            let mut r = Row::new("x");
            for c in chips {
                r = r.chip(Chip::new(*c));
            }
            r
        };
        let mut s = Screen::new("x");
        s.row(row(&["a", "b", "c", "d", "e", "+3"]));
        s.row(row(&["a", "b", "c", "+4"]));
        s.row(row(&["a", "+2"]));
        s.row(row(&["a", "b", "c"]));
        s.row(row(&["+1 jour"]).graph(GraphCell::default()));
        let r = rows(&s, &Local::default());
        assert_eq!(chip_texts(&r[0]), ["a", "b", "c", "+5"], "deux cachées ici, trois sur le PC");
        assert_eq!(chip_texts(&r[1]), ["a", "b", "c", "+4"]);
        assert_eq!(chip_texts(&r[2]), ["a", "+2"]);
        assert_eq!(chip_texts(&r[3]), ["a", "b", "c"]);
        assert_eq!(chip_texts(&r[4]), ["+1 jour"], "pas un compte");
    }

    /// Une progression : une barre sous la ligne, ou dans la note d'une opération en cours avec son « Annuler » ;
    /// indéterminée hors de 0 à 100.
    #[test]
    fn progress_shows_under_a_row_and_in_a_note_with_its_cancel() {
        let mut s = Screen::new("Dépôt");
        s.progress("Fetch… 42 %", Some(42), Some(Act::new("annuler", "Annuler")));
        s.row(Row::new("Clonage").progress(None)).row(Row::new("Rien"));
        let r = rows(&s, &Local::default());
        assert_eq!((r[0].kind, r[0].has_progress, r[0].indeterminate, r[0].value), (2, true, false, 0.42));
        assert_eq!(r[0].acts.row_data(0).map(|a| a.label.to_string()), Some("Annuler".into()));
        assert!(r[1].has_progress && r[1].indeterminate);
        assert!(!r[2].has_progress);
    }

    /// Le bouton visible d'une ligne suit l'état en cours de son action ; une action qui ouvre un écran du téléphone
    /// le dit (rien n'est envoyé au PC).
    #[test]
    fn a_trailing_button_and_a_local_screen_act_reach_the_phone() {
        let mut s = Screen::new("Patronus");
        s.row(
            Row::new("build")
                .trailing(Act::new("lancer:build", "Lancer").primary())
                .act(Act::screen("Historique", "tache/build")),
        );
        let mut local = Local::default();
        local.pending.insert("lancer:build".into());
        let r = rows(&s, &local);
        assert!(r[0].has_trailing && r[0].trailing.busy && r[0].trailing.style == 1);
        assert!(!r[0].busy, "le bouton visible porte son indicateur ; la ligne reste touchable");
        assert_eq!(r[0].acts.row_data(0).map(|a| a.open.to_string()), Some("tache/build".into()));
    }

    /// La barre du haut : une action seule en clair ; sinon deux icônes au plus, le reste au menu ⋯, les actions
    /// dangereuses à la fin du menu.
    #[test]
    fn the_top_bar_keeps_two_icons_and_the_menu_the_rest() {
        let labels = |l: &[&Act]| l.iter().map(|a| a.label.clone()).collect::<Vec<_>>();
        let one = [Act::new("lu", "Tout marquer lu")];
        assert_eq!(labels(&bar(&one, &[]).0), ["Tout marquer lu"]);
        let acts = [
            Act::new("tout", "Tout arrêter").danger().glyph("⏹"),
            Act::new("pause", "Pause"),
            Act::new("actualiser", "Actualiser").glyph("↻"),
            Act::new("nouveau", "Nouveau").glyph("+"),
        ];
        let local = [Act::new("@replier", "Tout replier")];
        let (icons, more) = bar(&acts, &local);
        assert_eq!(labels(&icons), ["Tout arrêter", "Actualiser"]);
        assert_eq!(labels(&more), ["Pause", "Nouveau", "Tout replier"]);
        let sheet = [Act::new("x", "Supprimer").danger(), Act::new("y", "Basculer")];
        assert_eq!(labels(&menu(&sheet)), ["Basculer", "Supprimer"]);
    }

    #[test]
    fn sync_replaces_only_the_items_that_differ() {
        let model = VecModel::from(vec![1, 2, 3]);
        sync(&model, vec![1, 5, 3, 4]);
        assert_eq!(model.iter().collect::<Vec<_>>(), [1, 5, 3, 4]);
        sync(&model, vec![1]);
        assert_eq!(model.iter().collect::<Vec<_>>(), [1]);
    }
}
