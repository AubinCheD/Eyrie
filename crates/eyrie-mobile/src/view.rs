//! Les écrans du PC en lignes Slint : un bloc par ligne de liste, les lignes consécutives réunies en une carte
//! (premier et dernier arrondis).
//!
//! Chaque ligne a une clé (son identité dans l'écran : `Row.key`, sinon son titre et son glyphe) et une empreinte (ce
//! qui la dessine). La liste est mise à jour sur place ([`patch`]) : seules les lignes changées sont redessinées, et
//! une ligne touchée ou un appui long en cours survit aux mises à jour du PC (jusqu'à toutes les 300 ms).

use std::collections::{HashMap, HashSet};
use std::hash::{DefaultHasher, Hash, Hasher};

use slint::{Color, Model, ModelRc, SharedString, VecModel};
use suite_mobile::ui::{Act, ActStyle, Block, Row, Screen, Tone};

use crate::{UiAct, UiRow};

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
        confirm: a.confirm.clone().unwrap_or_default().into(),
        has_input: a.input.is_some(),
        input_label: input.label.into(),
        input_placeholder: input.placeholder.into(),
        input_value: input.value.into(),
        multiline: input.multiline,
        url: a.url.clone().unwrap_or_default().into(),
        busy: local.busy(a),
    }
}

fn acts(list: &[Act], local: &Local) -> ModelRc<UiAct> {
    ModelRc::new(VecModel::from(list.iter().map(|a| act_in(a, local)).collect::<Vec<_>>()))
}

/// Une ligne de la liste avant d'être dessinée : sa clé, son empreinte, et ce qui la dessine.
pub struct Item<'a> {
    pub key: String,
    pub sig: u64,
    what: What<'a>,
}

enum What<'a> {
    Section(&'a Block),
    Row {
        row: &'a Row,
        first: bool,
        last: bool,
    },
    /// Note, jauge, texte, liste vide, boutons.
    Block(&'a Block),
    DiffLine {
        kind: u8,
        text: &'a str,
        first: bool,
        last: bool,
    },
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

/// Les lignes de l'écran, dans l'ordre, avec leur clé et leur empreinte.
pub fn items<'a>(s: &'a Screen, local: &Local) -> Vec<Item<'a>> {
    let is_row = |i: usize| matches!(s.blocks.get(i), Some(Block::Row(_)));
    let mut keys = Keys::default();
    let mut out = Vec::with_capacity(s.blocks.len());
    for (i, b) in s.blocks.iter().enumerate() {
        let mut h = DefaultHasher::new();
        hash_block(&mut h, b, local);
        let (base, what) = match b {
            Block::Section { title, key, .. } => {
                (format!("s:{}", if key.is_empty() { title } else { key }), What::Section(b))
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
            Block::Diff { lines, .. } => {
                let diff = keys.unique("d".into());
                for (n, (kind, text)) in lines.iter().enumerate() {
                    let (first, last) = (n == 0, n + 1 == lines.len());
                    let mut h = DefaultHasher::new();
                    (kind, text, first, last).hash(&mut h);
                    out.push(Item {
                        key: format!("{diff}:{n}"),
                        sig: h.finish(),
                        what: What::DiffLine { kind: *kind, text, first, last },
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

fn row(r: &Row, first: bool, last: bool, local: &Local) -> UiRow {
    UiRow {
        kind: 1,
        title: r.title.as_str().into(),
        subtitle: r.subtitle.as_str().into(),
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
        acts: acts(&r.acts, local),
        value: 0.0,
        mono: false,
        first,
        last,
        busy: r.tap.iter().chain(&r.acts).any(|a| local.busy(a)),
    }
}

/// La ligne Slint d'une ligne de la liste.
pub fn ui_row(item: &Item, local: &Local) -> UiRow {
    match &item.what {
        What::Section(Block::Section { title, aside, .. }) => {
            UiRow { detail: aside.as_str().into(), ..empty(0, &title.to_uppercase()) }
        }
        What::Row { row: r, first, last } => row(r, *first, *last, local),
        What::Block(Block::Note { text, tone: t, .. }) => UiRow { tone: tone(Some(*t)), ..empty(2, text) },
        What::Block(Block::Meter { label, value, text, tone: t }) => {
            UiRow { detail: text.as_str().into(), value: *value, tone: tone(Some(*t)), ..empty(3, label) }
        }
        What::Block(Block::Text { text, mono }) => UiRow { mono: *mono, ..empty(4, text) },
        What::Block(Block::Empty { text }) => empty(5, text),
        What::Block(Block::Buttons { acts: list }) => UiRow { acts: acts(list, local), ..empty(6, "") },
        What::DiffLine { kind, text, first, last } => {
            UiRow { tone: *kind as i32, first: *first, last: *last, ..empty(7, text) }
        }
        What::Section(_) | What::Block(_) => empty(5, ""),
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

    use super::*;

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
        assert_eq!(r.iter().map(|r| (r.kind, r.tone)).collect::<Vec<_>>(), [(7, 3), (7, 2), (7, 1)]);
        assert!(r[0].first && r[2].last);
    }

    #[test]
    fn actions_keep_what_the_phone_needs() {
        let a = act(&Act::new("tout", "Tout arrêter").danger().confirm("Sûr ?").input("Pourquoi", "…"));
        assert_eq!((a.style, a.confirm.as_str(), a.has_input), (2, "Sûr ?", true));
        assert_eq!(act(&Act::link("Ouvrir", "https://x.org")).url, "https://x.org");
        assert_eq!(ago(100, 100), "à l'instant");
        assert_eq!(ago(0, 7300), "2 h");
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
        assert_eq!(keys, ["s:Ozmo", "r:pwsh\u{1f}›_", "r:pwsh\u{1f}›_#2", "r:t/3", "d:0", "d:1"]);
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

    #[test]
    fn sync_replaces_only_the_items_that_differ() {
        let model = VecModel::from(vec![1, 2, 3]);
        sync(&model, vec![1, 5, 3, 4]);
        assert_eq!(model.iter().collect::<Vec<_>>(), [1, 5, 3, 4]);
        sync(&model, vec![1]);
        assert_eq!(model.iter().collect::<Vec<_>>(), [1]);
    }
}
