//! Le terminal suivi, côté téléphone : les lignes gardées, mises à jour par les images du PC (décalage et lignes
//! changées), et leurs couleurs résolues dans le thème du téléphone (clair ou sombre).

use suite_mobile::term::{self, Color, Line, QuickReply, TermFrame};

/// Ce qu'une image a changé, pour ne retoucher que ces lignes à l'écran.
#[derive(Debug, Default, PartialEq)]
pub struct Change {
    /// Tout est à redessiner (première image, autre terminal).
    pub full: bool,
    /// Lignes retirées en haut.
    pub shifted: usize,
    /// Nombre de lignes après l'image.
    pub len: usize,
    /// Lignes réécrites (indices après l'image).
    pub changed: Vec<usize>,
}

#[derive(Default)]
pub struct TermBuf {
    pub module: String,
    pub id: String,
    pub lines: Vec<Line>,
    pub cols: u16,
    pub cursor: Option<(u32, u16)>,
    pub title: String,
    pub waiting: bool,
    pub replies: Vec<QuickReply>,
    pub ended: bool,
}

impl TermBuf {
    pub fn is(&self, module: &str, id: &str) -> bool {
        self.module == module && self.id == id
    }

    pub fn reset(&mut self, module: &str, id: &str) {
        *self = TermBuf { module: module.into(), id: id.into(), ..Default::default() };
    }

    /// Les lignes à montrer : jusqu'à la dernière écrite ou au curseur. Les lignes vides du bas de l'écran du PC ne
    /// servent à rien sur le téléphone : la sortie se lit alors collée en bas, comme une conversation.
    pub fn visible_len(&self) -> usize {
        let written = self.lines.iter().rposition(|l| !l.text().trim_end().is_empty()).map_or(0, |i| i + 1);
        let cursor = self.cursor.map_or(0, |(row, _)| row as usize + 1);
        written.max(cursor).max(1).min(self.lines.len())
    }

    /// Le texte à copier : les lignes montrées (l'historique gardé compris), une par ligne, et l'octet où chacune
    /// commence (pour sélectionner celle qu'un appui long a désignée).
    pub fn copy_text(&self) -> (String, Vec<usize>) {
        let mut text = String::new();
        let mut starts = Vec::new();
        for (i, l) in self.lines[..self.visible_len()].iter().enumerate() {
            if i > 0 {
                text.push('\n');
            }
            starts.push(text.len());
            text.push_str(&line_text(l));
        }
        (text, starts)
    }

    /// Le programme demande un secret : la ligne du curseur, jusqu'à lui, finit par « Password: », « Enter passphrase
    /// for key '…': », « Mot de passe : »… La saisie du téléphone se masque alors (ni affichée, ni apprise par le
    /// clavier).
    pub fn asks_secret(&self) -> bool {
        let Some((row, col)) = self.cursor else { return false };
        let Some(line) = self.lines.get(row as usize) else { return false };
        let before: String = line_text(line).chars().take(col as usize).collect();
        let before = before.trim_end().to_lowercase();
        before.ends_with(':') && ["password", "passphrase", "mot de passe"].iter().any(|k| before.contains(k))
    }

    pub fn apply(&mut self, f: &TermFrame) -> Change {
        let full = !self.is(&f.module, &f.id);
        if full {
            self.reset(&f.module, &f.id);
        }
        let shifted = (f.shift as usize).min(self.lines.len());
        f.apply(&mut self.lines);
        self.cols = f.cols;
        self.cursor = f.cursor;
        self.title = f.title.clone();
        self.waiting = f.waiting;
        self.replies = f.replies.clone();
        self.ended = f.ended;
        Change { full, shifted, len: self.lines.len(), changed: f.changed.iter().map(|(i, _)| *i as usize).collect() }
    }
}

/// Le texte d'une ligne, colonnes gardées (les blancs entre deux suites de même style reviennent) ; sans les blancs de
/// la fin.
pub fn line_text(line: &Line) -> String {
    let mut s = String::new();
    let mut col = 0;
    for r in &line.runs {
        while col < r.col as usize {
            s.push(' ');
            col += 1;
        }
        s.push_str(&r.text);
        col += r.text.chars().count();
    }
    s.truncate(s.trim_end().len());
    s
}

/// Les couleurs du terminal dans un thème.
pub struct TermPalette {
    pub fg: u32,
    pub bg: u32,
    pub ansi: [u32; 16],
}

pub const DARK: TermPalette = TermPalette {
    fg: 0xd6d9df,
    bg: 0x121316,
    ansi: [
        0x2a2d35, 0xe5677a, 0x4cb872, 0xd9b43c, 0x5b8def, 0xb07ce0, 0x3aa8d8, 0xc8ccd4, 0x6f7582, 0xff8a9a, 0x6fd492,
        0xf2cc5c, 0x82a8ff, 0xc99cf0, 0x6cc6ea, 0xf2f3f5,
    ],
};

pub const LIGHT: TermPalette = TermPalette {
    fg: 0x1b1d22,
    bg: 0xfbfcfd,
    ansi: [
        0x1b1d22, 0xc42b1c, 0x1d7a41, 0x8a5a00, 0x0067c0, 0x8e3fb8, 0x00788f, 0x5f6672, 0x4b5563, 0xd6453a, 0x2a9454,
        0xa86d00, 0x1f7fd6, 0xa554cf, 0x0d93ad, 0x30343c,
    ],
};

impl TermPalette {
    /// Une des 256 couleurs : les 16 du thème, le cube 6 × 6 × 6, les gris.
    pub fn indexed(&self, i: u8) -> u32 {
        match i {
            0..=15 => self.ansi[i as usize],
            16..=231 => {
                let i = i as u32 - 16;
                let v = |x: u32| if x == 0 { 0 } else { 55 + x * 40 };
                (v(i / 36) << 16) | (v((i / 6) % 6) << 8) | v(i % 6)
            }
            _ => {
                let g = 8 + (i as u32 - 232) * 10;
                (g << 16) | (g << 8) | g
            }
        }
    }

    fn resolve(&self, c: Color, default: u32) -> u32 {
        match c {
            Color::Default => default,
            Color::Indexed(i) => self.indexed(i),
            Color::Rgb(r, g, b) => ((r as u32) << 16) | ((g as u32) << 8) | b as u32,
        }
    }
}

/// Un segment prêt à dessiner.
#[derive(Clone, Debug, PartialEq)]
pub struct Seg {
    pub col: u16,
    pub len: u16,
    pub text: String,
    /// 0xAARRGGBB.
    pub fg: u32,
    pub bg: Option<u32>,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
}

/// Hack, la police du terminal, dessine ces caractères sur une colonne : latin, grec, cyrillique, cadres et blocs.
fn in_font(c: char) -> bool {
    matches!(c, '\0'..='\u{4ff}' | '\u{2500}'..='\u{259f}')
}

/// Une marque sans largeur (accent combiné, sélecteur de variante, liant) : elle suit son caractère.
pub(crate) fn zero_width(c: char) -> bool {
    matches!(c, '\u{300}'..='\u{36f}' | '\u{200b}'..='\u{200f}' | '\u{2060}'..='\u{2064}' | '\u{20d0}'..='\u{20ff}')
        || matches!(c, '\u{fe00}'..='\u{fe0f}' | '\u{fe20}'..='\u{fe2f}')
}

/// Deux colonnes sur le PC (ou peut-être : idéogrammes, emoji) ; la largeur de ce qui suit n'est plus sûre.
pub(crate) fn wide(c: char) -> bool {
    matches!(
        c as u32,
        0x1100..=0x115f
            | 0x231a..=0x231b
            | 0x2329..=0x232a
            | 0x23e9..=0x23ec
            | 0x23f0
            | 0x23f3
            | 0x25fd..=0x25fe
            | 0x2614..=0x2615
            | 0x2648..=0x2653
            | 0x267f
            | 0x2693
            | 0x26a1
            | 0x26aa..=0x26ab
            | 0x26bd..=0x26be
            | 0x26c4..=0x26c5
            | 0x26ce
            | 0x26d4
            | 0x26ea
            | 0x26f2..=0x26f3
            | 0x26f5
            | 0x26fa
            | 0x26fd
            | 0x2705
            | 0x270a..=0x270b
            | 0x2728
            | 0x274c
            | 0x274e
            | 0x2753..=0x2755
            | 0x2757
            | 0x2795..=0x2797
            | 0x27b0
            | 0x27bf
            | 0x2b1b..=0x2b1c
            | 0x2b50
            | 0x2b55
            | 0x2e80..=0xa4cf
            | 0xac00..=0xd7a3
            | 0xf900..=0xfaff
            | 0xfe10..=0xfe19
            | 0xfe30..=0xfe6f
            | 0xff00..=0xff60
            | 0xffe0..=0xffe6
            | 0x1f000..
    )
}

/// Le texte d'un segment en morceaux placés chacun à sa colonne : un caractère que Hack n'a pas (« ❯ », « ✻ » de
/// Claude Code) vient d'une autre police, plus large ou plus étroite qu'une colonne ; seul, il ne décale pas ce qui
/// le suit (le bord droit d'un cadre sortait de son segment et disparaissait). Après un caractère large, la suite
/// reste d'un bloc : sa colonne n'est plus sûre.
fn pieces(text: &str, col: u16) -> Vec<(u16, String)> {
    let mut out = Vec::new();
    let (mut start, mut at, mut cur) = (col, col, String::new());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if wide(c) {
            cur.push(c);
            cur.extend(chars);
            break;
        }
        if zero_width(c) || in_font(c) {
            cur.push(c);
            at += u16::from(!zero_width(c));
            continue;
        }
        if !cur.is_empty() {
            out.push((start, std::mem::take(&mut cur)));
        }
        let mut one = c.to_string();
        while let Some(z) = chars.next_if(|&z| zero_width(z)) {
            one.push(z);
        }
        out.push((at, one));
        at += 1;
        start = at;
    }
    if !cur.is_empty() {
        out.push((start, cur));
    }
    out
}

/// Les segments d'une ligne, couleurs résolues : inversion, atténuation, fond seulement s'il diffère du fond du
/// terminal ; un caractère d'une autre police que Hack à part, à sa colonne (voir [`pieces`]).
pub fn segments(line: &Line, p: &TermPalette) -> Vec<Seg> {
    line.runs
        .iter()
        .flat_map(|r| {
            let mut fg = p.resolve(r.fg, p.fg);
            let mut bg = p.resolve(r.bg, p.bg);
            if r.flags & term::INVERSE != 0 {
                std::mem::swap(&mut fg, &mut bg);
            }
            let alpha = if r.flags & term::DIM != 0 { 0x99 } else { 0xff };
            pieces(&r.text, r.col).into_iter().map(move |(col, text)| Seg {
                col,
                len: text.chars().count() as u16,
                text,
                fg: (alpha << 24) | fg,
                bg: (bg != p.bg).then_some(0xff00_0000 | bg),
                bold: r.flags & term::BOLD != 0,
                italic: r.flags & term::ITALIC != 0,
                underline: r.flags & (term::UNDERLINE | term::STRIKE) != 0,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use suite_mobile::term::{Run, TermView};

    fn view(lines: &[&str]) -> TermView {
        TermView { cols: 40, rows: 2, lines: lines.iter().map(|l| Line::plain(*l)).collect(), ..Default::default() }
    }

    #[test]
    fn frames_keep_the_buffer_in_step_with_the_pc() {
        let mut b = TermBuf::default();
        let v1 = view(&["a", "b", "c"]);
        let c = b.apply(&TermFrame::full("fawkes", "1", &v1));
        assert!(c.full);
        let v2 = view(&["c", "d", "e"]);
        let c = b.apply(&TermFrame::diff("fawkes", "1", &v1, &v2));
        assert_eq!((c.full, c.shifted, c.len), (false, 2, 3));
        assert_eq!(b.lines, v2.lines);
        assert_eq!(b.visible_len(), 3);
        // Une image d'un autre terminal repart de zéro.
        assert!(b.apply(&TermFrame::full("fawkes", "2", &v1)).full);
    }

    #[test]
    fn blank_lines_under_the_cursor_are_not_shown() {
        let mut b = TermBuf::default();
        let mut v = view(&["PS C:\\> ", "", "", ""]);
        v.cursor = Some((0, 6));
        b.apply(&TermFrame::full("fawkes", "1", &v));
        assert_eq!(b.visible_len(), 1);
        v.cursor = Some((2, 0));
        b.apply(&TermFrame::full("fawkes", "1", &v));
        assert_eq!(b.visible_len(), 3, "le curseur plus bas : ses lignes se montrent");
    }

    #[test]
    fn the_text_to_copy_keeps_columns_and_the_kept_history() {
        let mut b = TermBuf::default();
        let mut v = view(&["$ cargo test", "", "", ""]);
        v.lines.insert(
            1,
            Line {
                runs: vec![
                    Run { col: 2, text: "ok".into(), fg: Color::Indexed(2), bg: Color::Default, flags: 0 },
                    Run { col: 6, text: "3 tests  ".into(), fg: Color::Default, bg: Color::Default, flags: 0 },
                ],
            },
        );
        v.cursor = Some((2, 0));
        b.apply(&TermFrame::full("fawkes", "1", &v));
        let (text, starts) = b.copy_text();
        assert_eq!(text, "$ cargo test\n  ok  3 tests\n", "le curseur sur la troisième : elle se copie, vide");
        assert_eq!(starts, vec![0, 13, 27]);
        assert_eq!(&text[starts[1]..starts[2] - 1], "  ok  3 tests");
        assert_eq!(TermBuf::default().copy_text(), (String::new(), Vec::new()));
    }

    #[test]
    fn a_password_prompt_at_the_cursor_asks_for_a_secret() {
        let at = |line: &str, col: u16| {
            let mut b = TermBuf::default();
            let mut v = view(&["$ git push", line, ""]);
            v.cursor = Some((1, col));
            b.apply(&TermFrame::full("fawkes", "1", &v));
            b.asks_secret()
        };
        for prompt in [
            "Password for 'https://aubin@github.com': ",
            "aubin@build's password: ",
            "Enter passphrase for key 'C:\\Users\\aubin/.ssh/id_ed25519': ",
            "[sudo] password for aubin: ",
            "Mot de passe : ",
            "Enter PASSWORD:",
        ] {
            assert!(at(prompt, prompt.chars().count() as u16), "{prompt}");
        }
        // Le curseur ailleurs que juste après, ou une ligne qui n'en demande pas.
        assert!(!at("Password: ", 0), "le curseur au début de la ligne");
        assert!(!at("PS C:\\Users\\aubin> ", 19));
        assert!(!at("Username for 'https://github.com': ", 35));
        assert!(!at("password reset done", 19));
        let mut b = TermBuf::default();
        b.apply(&TermFrame::full("fawkes", "1", &view(&["Password: "])));
        assert!(!b.asks_secret(), "sans curseur (caché) : rien");
    }

    #[test]
    fn a_glyph_from_another_font_keeps_its_column() {
        let pieces = |text: &str, col: u16| {
            let line =
                Line { runs: vec![Run { col, text: text.into(), fg: Color::Default, bg: Color::Default, flags: 0 }] };
            segments(&line, &DARK).into_iter().map(|s| (s.col, s.text)).collect::<Vec<_>>()
        };
        let s = |c: u16, t: &str| (c, t.to_string());
        // Le choix d'un dialogue de Claude Code : le cadre de droite reste à sa colonne.
        assert_eq!(pieces("│ ❯ 1. Yes   │", 0), [s(0, "│ "), s(2, "❯"), s(3, " 1. Yes   │")]);
        assert_eq!(pieces("✻ Welcome", 4), [s(4, "✻"), s(5, " Welcome")]);
        // Latin (accents compris), cadres et blocs : Hack les dessine, rien ne se coupe.
        assert_eq!(pieces("╭─ café █▌ ─╮", 0), [s(0, "╭─ café █▌ ─╮")]);
        // Une marque sans largeur suit son caractère, sans compter de colonne.
        assert_eq!(pieces("e\u{301}✔\u{fe0f} ok", 0), [s(0, "e\u{301}"), s(1, "✔\u{fe0f}"), s(2, " ok")]);
        // Un caractère large (deux colonnes sur le PC) : la suite reste d'un bloc, comme avant.
        assert_eq!(pieces("❯ 日本 ❯ x", 0), [s(0, "❯"), s(1, " 日本 ❯ x")]);
        assert_eq!(pieces("ok ✅ ❯", 0), [s(0, "ok ✅ ❯")]);
    }

    #[test]
    fn colors_follow_the_phone_theme() {
        let line = Line {
            runs: vec![
                Run { col: 0, text: "rouge".into(), fg: Color::Indexed(1), bg: Color::Default, flags: 0 },
                Run { col: 6, text: "inv".into(), fg: Color::Default, bg: Color::Default, flags: term::INVERSE },
            ],
        };
        let dark = segments(&line, &DARK);
        assert_eq!(dark[0].fg, 0xffe5677a);
        assert_eq!(dark[0].bg, None, "fond du terminal : rien à dessiner");
        assert_eq!(dark[1].bg, Some(0xff000000 | DARK.fg), "inversé : le texte devient le fond");
        assert_eq!(segments(&line, &LIGHT)[0].fg, 0xffc42b1c);
        assert_eq!(DARK.indexed(196), 0xff0000);
        assert_eq!(DARK.indexed(244), 0x808080);
    }
}
