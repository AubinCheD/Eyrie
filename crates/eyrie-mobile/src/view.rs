//! Les écrans du PC en lignes Slint : un bloc par ligne de liste, les lignes consécutives réunies en une carte
//! (premier et dernier arrondis).

use slint::{Color, ModelRc, SharedString, VecModel};
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

pub fn act(a: &Act) -> UiAct {
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
    }
}

pub fn acts(list: &[Act]) -> ModelRc<UiAct> {
    ModelRc::new(VecModel::from(list.iter().map(act).collect::<Vec<_>>()))
}

fn empty(kind: i32, title: &str) -> UiRow {
    UiRow { kind, title: SharedString::from(title), acts: acts(&[]), ..Default::default() }
}

fn row(r: &Row, first: bool, last: bool) -> UiRow {
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
        tap: r.tap.as_ref().map(act).unwrap_or_default(),
        has_tap: r.tap.is_some(),
        acts: acts(&r.acts),
        value: 0.0,
        mono: false,
        first,
        last,
    }
}

pub fn rows(s: &Screen) -> Vec<UiRow> {
    let is_row = |i: usize| matches!(s.blocks.get(i), Some(Block::Row(_)));
    s.blocks
        .iter()
        .enumerate()
        .map(|(i, b)| match b {
            Block::Section { title, aside } => {
                UiRow { detail: aside.as_str().into(), ..empty(0, &title.to_uppercase()) }
            }
            Block::Row(r) => row(r, i == 0 || !is_row(i - 1), !is_row(i + 1)),
            Block::Note { text, tone: t } => UiRow { tone: tone(Some(*t)), ..empty(2, text) },
            Block::Meter { label, value, text, tone: t } => {
                UiRow { detail: text.as_str().into(), value: *value, tone: tone(Some(*t)), ..empty(3, label) }
            }
            Block::Text { text, mono } => UiRow { mono: *mono, ..empty(4, text) },
            Block::Empty { text } => empty(5, text),
            Block::Buttons { acts: list } => UiRow { acts: acts(list), ..empty(6, "") },
        })
        .collect()
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
    use super::*;
    use slint::Model;

    #[test]
    fn consecutive_rows_share_a_card() {
        let mut s = Screen::new("x");
        s.section("Tâches", "2")
            .row(Row::new("a").open("t/a"))
            .row(Row::new("b").tap(Act::new("lancer", "Lancer")).act(Act::new("x", "X")))
            .note("attention", Tone::Warn)
            .row(Row::new("c"));
        let r = rows(&s);
        assert_eq!(r.len(), 5);
        assert_eq!((r[0].kind, r[0].title.as_str(), r[0].detail.as_str()), (0, "TÂCHES", "2"));
        assert!(r[1].first && !r[1].last && r[1].opens && r[1].path == "t/a");
        assert!(!r[2].first && r[2].last && r[2].has_tap && r[2].acts.row_count() == 1);
        assert_eq!((r[3].kind, r[3].tone), (2, 3));
        assert!(r[4].first && r[4].last, "seule dans sa carte");
    }

    #[test]
    fn actions_keep_what_the_phone_needs() {
        let a = act(&Act::new("tout", "Tout arrêter").danger().confirm("Sûr ?").input("Pourquoi", "…"));
        assert_eq!((a.style, a.confirm.as_str(), a.has_input), (2, "Sûr ?", true));
        assert_eq!(act(&Act::link("Ouvrir", "https://x.org")).url, "https://x.org");
        assert_eq!(ago(100, 100), "à l'instant");
        assert_eq!(ago(0, 7300), "2 h");
    }
}
