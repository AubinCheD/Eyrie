//! Le terminal à la largeur du téléphone : chaque ligne du PC se replie en lignes d'écran de `width` colonnes, à une
//! taille de police lisible, sans défilement de côté.
//!
//! Une ligne trop large perd d'abord le remplissage qui ne tient pas : ses longues suites de traits (le bord d'un
//! cadre, un séparateur) et, si la ligne tient alors sur une ligne d'écran, de blancs (l'espace devant un texte calé à
//! droite, celui qui mène au bord droit d'un cadre), raccourcies ensemble, la plus longue d'abord, jusqu'à une case
//! chacune ; le bord droit d'un cadre retrouve ainsi celui de l'écran. Ce qui dépasse encore se replie, après un blanc
//! si possible (sinon après « / », « ( »… : un chemin se coupe entre ses dossiers), la suite en retrait sous le texte
//! de la ligne (après son retrait et sa puce : « ⏺ », « - », « 1. ») ; ses blancs (l'indentation d'un code, les
//! colonnes d'un tableau) restent. Une ligne de cadre qui se replie garde ses deux bords sur chaque ligne d'écran. Une
//! ligne qui tient reste telle quelle, colonnes comprises.

use std::cmp::Reverse;

use suite_mobile::term::{self, Color, Line, Run};

use crate::termbuf::{wide, zero_width};

/// Le remplissage qui se raccourcit : une suite d'au moins `FILLER_MIN` cases de l'un de ces caractères.
const FILLER: [char; 10] = [' ', '─', '━', '═', '╌', '┄', '┈', '-', '=', '_'];
const FILLER_MIN: usize = 3;
/// Les puces après lesquelles la suite d'une ligne repliée se met en retrait (celles de Claude Code comprises).
const BULLETS: [&str; 16] = ["⏺", "●", "•", "◦", "-", "*", "⎿", "│", ">", "❯", "·", "✢", "✳", "✶", "✻", "✽"];
/// Où couper un mot plus long que la place, faute de blanc : après l'un de ces caractères.
const BREAK_AFTER: [char; 12] = ['/', '\\', '-', '_', '.', ',', ';', ':', '(', '[', '{', '='];
/// Les bords verticaux d'un cadre.
const EDGES: [char; 3] = ['│', '┃', '║'];
/// Moins de colonnes n'a pas de sens (une fenêtre pas encore mise en page).
pub const MIN_WIDTH: u16 = 8;

/// Une case : son texte (un caractère et ses marques sans largeur), sa largeur (2 pour un caractère large), son style
/// (l'indice de sa suite dans la ligne ; `None` : un blanc entre deux suites), et si elle se voit sans texte (fond,
/// inversion, soulignement).
#[derive(Clone, Debug)]
struct Cell {
    text: String,
    width: u16,
    style: Option<usize>,
    marked: bool,
}

impl Cell {
    fn is(&self, c: char) -> bool {
        self.text.len() == c.len_utf8() && self.text.starts_with(c)
    }

    /// Un blanc qui ne se voit pas : il peut finir une ligne d'écran, ou disparaître au début de la suivante.
    fn blank(&self) -> bool {
        self.is(' ') && !self.marked
    }
}

fn marked(r: &Run) -> bool {
    r.bg != Color::Default || r.flags & (term::INVERSE | term::UNDERLINE | term::STRIKE) != 0
}

/// Les cases d'une ligne du PC, dans l'ordre de ses colonnes.
fn cells(line: &Line) -> Vec<Cell> {
    let mut out: Vec<Cell> = Vec::new();
    let mut col: u16 = 0;
    for (k, r) in line.runs.iter().enumerate() {
        while col < r.col {
            out.push(Cell { text: " ".into(), width: 1, style: None, marked: false });
            col += 1;
        }
        for c in r.text.chars() {
            if zero_width(c)
                && let Some(last) = out.last_mut()
            {
                last.text.push(c);
                continue;
            }
            let width = if wide(c) { 2 } else { 1 };
            out.push(Cell { text: c.to_string(), width, style: Some(k), marked: marked(r) });
            col += width;
        }
    }
    out
}

/// Raccourcit le remplissage d'une ligne trop large pour `width` colonnes (voir le module). Rend, pour chaque case
/// d'avant (et la fin de ligne), son indice après ; une case retirée prend celui de la case qui la suivait.
fn shrink(cells: &mut Vec<Cell>, width: usize) -> Vec<usize> {
    let total: usize = cells.iter().map(|c| c.width as usize).sum();
    if total <= width {
        return (0..=cells.len()).collect();
    }
    let mut fillers: Vec<(usize, usize, bool)> = Vec::new();
    let mut i = 0;
    while i < cells.len() {
        let c = &cells[i];
        if c.width != 1 || !FILLER.iter().any(|&f| c.is(f)) {
            i += 1;
            continue;
        }
        let mut j = i + 1;
        while j < cells.len() && cells[j].text == c.text {
            j += 1;
        }
        // Le retrait du début reste, sauf s'il cale le texte loin à droite.
        let indent = i == 0 && c.is(' ');
        if j - i >= FILLER_MIN && (!indent || j - i > width / 3) {
            fillers.push((i, j - i, c.is(' ')));
        }
        i = j;
    }
    // Les blancs ne cèdent que si la ligne tient alors sur une ligne d'écran : sinon, elle se replie de toute façon.
    let spare: usize = fillers.iter().map(|f| f.1 - 1).sum();
    if total - spare > width {
        fillers.retain(|f| !f.2);
    }
    let mut kept: Vec<usize> = fillers.iter().map(|f| f.1).collect();
    let mut over = total - width;
    while over > 0 {
        // La plus longue d'abord ; à égalité, la première.
        let Some(k) = (0..kept.len()).filter(|&k| kept[k] > 1).max_by_key(|&k| (kept[k], Reverse(k))) else { break };
        kept[k] -= 1;
        over -= 1;
    }
    let mut gone = vec![false; cells.len()];
    for (&(start, len, _), &keep) in fillers.iter().zip(&kept) {
        gone[start + keep..start + len].fill(true);
    }
    let mut map = Vec::with_capacity(cells.len() + 1);
    let mut next = 0;
    for &g in &gone {
        map.push(next);
        next += usize::from(!g);
    }
    map.push(next);
    let mut k = 0;
    cells.retain(|_| {
        k += 1;
        !gone[k - 1]
    });
    map
}

/// Le retrait de la suite d'une ligne repliée : celui de la ligne, plus sa puce et le blanc qui la suit (« ⏺ »,
/// « - », « 1. ») ; un tiers de la largeur au plus.
fn hanging(cells: &[Cell], width: usize) -> usize {
    let lead = cells.iter().take_while(|c| c.blank()).count();
    let rest = &cells[lead..];
    let digits = rest.iter().take_while(|c| c.text.len() == 1 && c.text.as_bytes()[0].is_ascii_digit()).count();
    let bullet = if (1..=3).contains(&digits) && rest.get(digits).is_some_and(|c| c.is('.') || c.is(')')) {
        digits + 1
    } else if rest.first().is_some_and(|c| BULLETS.contains(&c.text.as_str())) {
        rest[0].width as usize
    } else {
        0
    };
    let indent = if bullet > 0 && rest.get(bullet).is_some_and(Cell::blank) { lead + bullet + 1 } else { lead };
    indent.min(width / 3)
}

/// Une ligne du PC repliée à la largeur du téléphone.
#[derive(Debug, Default, PartialEq)]
pub struct Fitted {
    /// Ses lignes d'écran (une au moins), chacune avec ses colonnes à l'écran.
    pub rows: Vec<Line>,
    /// Le curseur, s'il est sur cette ligne : (ligne d'écran, colonne).
    pub cursor: Option<(usize, u16)>,
}

/// Replie une ligne du PC en lignes d'écran de `width` colonnes au plus ; `cursor` : la colonne du curseur, s'il est
/// sur elle (au-delà du texte, comme après une invite dont le blanc final n'est pas envoyé).
pub fn fit(line: &Line, width: u16, cursor: Option<u16>) -> Fitted {
    let width = width.max(MIN_WIDTH) as usize;
    let mut cells = cells(line);
    // Le curseur : sur une case, ou après la fin de la ligne (de tant de colonnes).
    let at = cursor.map(|col| {
        let mut x = 0usize;
        for (i, c) in cells.iter().enumerate() {
            if (col as usize) < x + c.width as usize {
                return (i, 0);
            }
            x += c.width as usize;
        }
        (cells.len(), col as usize - x)
    });
    let mut map = shrink(&mut cells, width);
    // Une ligne de cadre qui se replie encore : son bord droit (et les blancs devant) se met au bout de chaque ligne
    // d'écran, son bord gauche au début de chacune, et le texte se replie entre les deux.
    let total: usize = cells.iter().map(|c| c.width as usize).sum();
    let lead = cells.iter().take_while(|c| c.blank()).count();
    let edge = |c: Option<&Cell>| c.is_some_and(|c| c.width == 1 && EDGES.iter().any(|&e| c.is(e)));
    let boxed = total > width && cells.len() > lead + 1 && edge(cells.get(lead)) && edge(cells.last());
    let mut edges = None;
    if boxed {
        let right = cells.pop().expect("un bord droit");
        while cells.last().is_some_and(Cell::blank) {
            cells.pop();
        }
        map.iter_mut().for_each(|m| *m = (*m).min(cells.len()));
        edges = Some((cells[lead].clone(), right));
    }
    // Un cadre : un blanc et son bord droit au bout de chaque ligne d'écran.
    let limit = if boxed { width - 2 } else { width };
    let indent = hanging(&cells, width);
    // Les lignes d'écran : (colonne de départ, cases), et la place de chaque case.
    let mut rows: Vec<(usize, std::ops::Range<usize>)> = Vec::new();
    let mut place = vec![(0usize, 0usize); cells.len()];
    let mut i = 0;
    loop {
        let row = rows.len();
        let start = if row == 0 { 0 } else { indent };
        if row > 0 {
            // Les blancs où la ligne s'est repliée ne commencent pas la suivante.
            while i < cells.len() && cells[i].blank() {
                place[i] = (row, start);
                i += 1;
            }
            if i >= cells.len() {
                break;
            }
        }
        let room = limit - start;
        let (mut j, mut used) = (i, 0);
        while j < cells.len() && used + cells[j].width as usize <= room {
            used += cells[j].width as usize;
            j += 1;
        }
        // Repliée sur le blanc qui ne tient plus, sinon après un blanc, s'il y en a un dans la seconde moitié de la ligne
        // d'écran ; sinon après « / », « ( »… (un chemin, un appel) ; sinon où la place manque.
        if j < cells.len() && !cells[j].blank() {
            let late = i + room / 2..j;
            if let Some(k) = late.clone().rev().find(|&k| cells[k].blank()) {
                j = k + 1;
            } else if let Some(k) = late.rev().find(|&k| BREAK_AFTER.iter().any(|&b| cells[k].is(b))) {
                j = k + 1;
            }
            // Une case plus large que la place qui reste : seule sur sa ligne.
            j = j.max(i + 1);
        }
        let mut col = start;
        for (k, c) in cells.iter().enumerate().take(j).skip(i) {
            place[k] = (row, col);
            col += c.width as usize;
        }
        rows.push((start, i..j));
        i = j;
        if i >= cells.len() {
            break;
        }
    }
    let mut out: Vec<Line> = rows.iter().map(|(start, range)| row_line(line, &cells, range.clone(), *start)).collect();
    if let Some((left, right)) = &edges {
        for (k, row) in out.iter_mut().enumerate() {
            if k > 0 {
                row.runs.insert(0, cell_run(line, left, lead));
            }
            row.runs.push(cell_run(line, right, width - 1));
        }
    }
    let cursor = at.map(|(i, beyond)| {
        let i = map[i];
        if i < cells.len() {
            let (row, col) = place[i];
            return (row, col as u16);
        }
        // Après la fin : sur la dernière ligne d'écran, ou sur une nouvelle si elle est pleine.
        let (row, start) = (rows.len() - 1, rows[rows.len() - 1].0);
        let end = rows[row].1.clone().map(|k| cells[k].width as usize).sum::<usize>() + start + beyond;
        if end < width {
            (row, end as u16)
        } else {
            out.push(Line::default());
            (row + 1, indent as u16)
        }
    });
    Fitted { rows: out, cursor }
}

/// Une case seule, à sa colonne (le bord d'un cadre).
fn cell_run(line: &Line, c: &Cell, col: usize) -> Run {
    let (fg, bg, flags) = match c.style.and_then(|s| line.runs.get(s)) {
        Some(r) => (r.fg, r.bg, r.flags),
        None => (Color::Default, Color::Default, 0),
    };
    Run { col: col as u16, text: c.text.clone(), fg, bg, flags }
}

/// Une ligne d'écran : ses cases en suites de même style, à leurs colonnes ; sans ses blancs de fin. Un blanc sans fond
/// rejoint la suite en cours, comme sur le PC (`read_line`) ; les blancs entre deux suites du PC restent un saut de
/// colonne.
fn row_line(line: &Line, cells: &[Cell], range: std::ops::Range<usize>, start: usize) -> Line {
    let mut end = range.end;
    while end > range.start && cells[end - 1].blank() {
        end -= 1;
    }
    let mut runs: Vec<Run> = Vec::new();
    let mut col = start;
    let mut gap = false;
    for c in &cells[range.start..end] {
        if c.style.is_none() && c.blank() {
            gap = true;
            col += 1;
            continue;
        }
        let (fg, bg, flags) = match c.style.and_then(|s| line.runs.get(s)) {
            Some(r) => (r.fg, r.bg, r.flags),
            None => (Color::Default, Color::Default, 0),
        };
        match runs.last_mut() {
            Some(r) if !gap && ((r.fg, r.bg, r.flags) == (fg, bg, flags) || (c.blank() && !marked(r))) => {
                r.text.push_str(&c.text)
            }
            _ => runs.push(Run { col: col as u16, text: c.text.clone(), fg, bg, flags }),
        }
        gap = false;
        col += c.width as usize;
    }
    Line { runs }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::termbuf::line_text;

    fn texts(f: &Fitted) -> Vec<String> {
        f.rows.iter().map(line_text).collect()
    }

    fn run(col: u16, text: &str, fg: Color) -> Run {
        Run { col, text: text.into(), fg, bg: Color::Default, flags: 0 }
    }

    /// Une ligne qui tient ne bouge pas, colonnes et blancs compris.
    #[test]
    fn a_line_that_fits_stays_as_it_is() {
        let line = Line { runs: vec![run(2, "ok", Color::Indexed(2)), run(8, "3 tests", Color::Default)] };
        let f = fit(&line, 40, None);
        assert_eq!(f.rows, vec![line.clone()]);
        let same = Line { runs: vec![run(0, "a", Color::Default), run(4, "b", Color::Default)] };
        assert_eq!(fit(&same, 40, None).rows, vec![same.clone()], "deux suites pareilles restent à leurs colonnes");
        assert_eq!(fit(&Line::default(), 40, None).rows, vec![Line::default()], "une ligne vide : une ligne d'écran");
    }

    /// Un cadre de Claude Code plus large que l'écran : le remplissage se raccourcit, le bord droit retrouve le bord de
    /// l'écran, sur une seule ligne.
    #[test]
    fn a_box_wider_than_the_screen_shrinks_its_filling() {
        let top = format!("╭{}╮", "─".repeat(78));
        let choice = format!("│ ❯ 1. Yes{}│", " ".repeat(70));
        let rule = "─".repeat(120);
        for (line, want) in [
            (top.as_str(), format!("╭{}╮", "─".repeat(28))),
            (choice.as_str(), format!("│ ❯ 1. Yes{}│", " ".repeat(19))),
            (rule.as_str(), "─".repeat(30)),
        ] {
            assert_eq!(texts(&fit(&Line::plain(line), 30, None)), [want]);
        }
    }

    /// Plusieurs remplissages : le plus long cède d'abord ; ce qui reste passe à la ligne.
    #[test]
    fn the_longest_filling_gives_way_first() {
        let line = format!("ab{}cd{}ef", " ".repeat(10), " ".repeat(4));
        assert_eq!(texts(&fit(&Line::plain(line), 14, None)), [format!("ab{}cd{}ef", " ".repeat(4), " ".repeat(4))]);
        let line = format!("{}12:04", " ".repeat(60));
        assert_eq!(texts(&fit(&Line::plain(line), 20, None)), [format!("{}12:04", " ".repeat(15))], "calé à droite");
    }

    /// Du texte plus long que l'écran se replie après un blanc, la suite en retrait sous le texte, après la puce.
    #[test]
    fn prose_wraps_at_spaces_under_its_bullet() {
        let line = Line::plain("  ⏺ Le terminal du PC se lit maintenant sur le téléphone sans défiler de côté.");
        assert_eq!(
            texts(&fit(&line, 30, None)),
            ["  ⏺ Le terminal du PC se lit", "    maintenant sur le", "    téléphone sans défiler de", "    côté."]
        );
        let list = Line::plain("12. un deux trois quatre cinq six");
        assert_eq!(texts(&fit(&list, 16, None)), ["12. un deux", "    trois quatre", "    cinq six"]);
    }

    /// Un mot plus long que la place se coupe où il faut ; le retrait ne dépasse pas un tiers de l'écran.
    #[test]
    fn a_long_word_is_cut_and_the_indent_stays_small() {
        let line = Line::plain("cargo:warning=C:/Users/aubin/DEV/TestAubin/Eyrie/target/debug/build");
        let rows = texts(&fit(&line, 24, None));
        assert_eq!(rows[0], "cargo:warning=C:/Users/", "coupé entre deux dossiers");
        assert!(rows.iter().skip(1).all(|r| r.chars().count() <= 24));
        let deep = Line::plain(format!("{}- {}", " ".repeat(9), "mot ".repeat(10)));
        let rows = texts(&fit(&deep, 30, None));
        assert!(rows[1].starts_with(&" ".repeat(10)) && !rows[1].starts_with(&" ".repeat(11)), "{rows:?}");
    }

    /// Les couleurs suivent leur texte sur la ligne suivante ; les colonnes y repartent du retrait.
    #[test]
    fn colors_follow_their_text_onto_the_next_row() {
        let line =
            Line { runs: vec![run(0, "- ", Color::Default), run(2, "rouge vif et encore rouge", Color::Indexed(1))] };
        let f = fit(&line, 16, None);
        assert_eq!(texts(&f), ["- rouge vif et", "  encore rouge"]);
        assert_eq!(f.rows[1].runs, [run(2, "encore rouge", Color::Indexed(1))]);
    }

    /// Le curseur garde sa place : sur le texte, après lui (le blanc final d'une invite n'est pas envoyé), ou sur une
    /// nouvelle ligne d'écran quand la dernière est pleine.
    #[test]
    fn the_cursor_keeps_its_place() {
        let prompt = Line::plain("PS C:\\Users\\aubin>");
        assert_eq!(fit(&prompt, 40, Some(19)).cursor, Some((0, 19)));
        let f = fit(&prompt, 10, Some(19));
        assert_eq!(texts(&f), ["PS C:\\", "Users\\", "aubin>"], "coupée entre les dossiers");
        assert_eq!(f.cursor, Some((2, 7)), "après la fin, sur la suite repliée");
        let full = Line::plain("abcdefghij");
        let f = fit(&full, 10, Some(10));
        assert_eq!((f.cursor, f.rows.len()), (Some((1, 0)), 2), "la ligne pleine : une ligne d'écran de plus");
        let words = Line::plain("un deux trois quatre");
        assert_eq!(fit(&words, 12, Some(14)).cursor, Some((1, 6)), "sur « quatre », repliée");
        assert_eq!(fit(&Line::default(), 40, Some(3)).cursor, Some((0, 3)));
        // Dans un remplissage raccourci : la case qui le suit.
        let boxed = Line::plain(format!("│ a{}│", " ".repeat(40)));
        assert_eq!(fit(&boxed, 12, Some(30)).cursor, Some((0, 11)));
    }

    /// Une ligne de cadre qui se replie garde ses deux bords sur chaque ligne d'écran ; le texte se replie entre eux.
    #[test]
    fn a_box_line_that_wraps_keeps_both_edges() {
        let line = Line::plain(format!("│ /help for help, /status for your current setup{}│", " ".repeat(60)));
        assert_eq!(
            texts(&fit(&line, 36, None)),
            ["│ /help for help, /status for your │".to_string(), format!("│ current setup{}│", " ".repeat(20))]
        );
    }

    /// Une ligne qui se replie de toute façon garde ses blancs (l'indentation d'un code) ; seuls les traits cèdent.
    #[test]
    fn spaces_stay_when_the_line_wraps_anyway() {
        let code = Line::plain("    +     let camera = Camera::open_back()?;");
        assert_eq!(texts(&fit(&code, 28, None)), ["    +     let camera =", "    Camera::open_back()?;"]);
        let rule = Line::plain(format!("── Résultat {}── fin du texte qui dépasse", "─".repeat(40)));
        assert_eq!(texts(&fit(&rule, 30, None))[0], "── Résultat ─ fin du texte qui");
    }

    /// Les puces de Claude Code (« ✻ » qui réfléchit) mettent la suite en retrait, comme « ⏺ ».
    #[test]
    fn claude_code_glyphs_are_bullets() {
        let line = Line::plain("✻ Cogitating… (12s · ↑ 1.2k tokens · esc to interrupt)");
        assert_eq!(texts(&fit(&line, 36, None)), ["✻ Cogitating… (12s · ↑ 1.2k tokens ·", "  esc to interrupt)"]);
    }

    /// Un caractère large compte deux colonnes, comme sur le PC.
    #[test]
    fn a_wide_character_takes_two_columns() {
        let f = fit(&Line::plain("日本語のテキスト"), 9, None);
        assert_eq!(texts(&f), ["日本語の", "テキスト"]);
    }

    /// Un blanc qui se voit (fond, inversion : la barre d'un choix) ne disparaît pas au bout d'une ligne d'écran.
    #[test]
    fn a_visible_blank_is_kept() {
        let bar = Run { col: 0, text: "choix   ".into(), fg: Color::Default, bg: Color::Indexed(4), flags: 0 };
        assert_eq!(fit(&Line { runs: vec![bar.clone()] }, 20, None).rows[0].runs, [bar]);
    }
}
