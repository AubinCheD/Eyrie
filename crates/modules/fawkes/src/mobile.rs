//! Fawkes sur le téléphone : les groupes et leurs terminaux (état, sessions Claude qui attendent), un terminal en
//! direct, et ce qu'on y fait (saisie, touches, réponses rapides à Claude, nouveau terminal, redémarrer, veille).
//!
//! Tout passe par l'API publique de Fawkes : `App::st` (groupes, terminaux), `App::live` (la grille de chaque
//! terminal, `TermRuntime`), `ui::panes::status_color` (le libellé d'état), `ui::panes::replay_keyboard` (la saisie,
//! encodée comme au clavier du PC, mode win32-input de ConPTY compris).

use std::hash::{DefaultHasher, Hash, Hasher};

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line as GridLine};
use alacritty_terminal::term::TermMode;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::vte::ansi::{Color as AnsiColor, NamedColor};
use eframe::egui;
use suite_sdk::Cx;
use suite_sdk::mobile::msg::OPEN_PREFIX;
use suite_sdk::mobile::term::{self, Color, Key, Line, Run, TermInput, TermView};
use suite_sdk::mobile::ui::{Act, Row, Screen, Tone};

use fawkes::app::{App, NewClaude, NewTerm};
use fawkes::model::{Rgb, TermId};

fn rgb(c: Rgb) -> u32 {
    ((c.0 as u32) << 16) | ((c.1 as u32) << 8) | c.2 as u32
}

/// Les deux derniers éléments d'un chemin (`…\Eyrie\crates`).
fn short_path(p: &str) -> String {
    let parts: Vec<&str> = p.split(['\\', '/']).filter(|s| !s.is_empty()).collect();
    match parts.len() {
        0 => p.to_string(),
        1 => parts[0].to_string(),
        n => format!("…\\{}\\{}", parts[n - 2], parts[n - 1]),
    }
}

/// Empreinte courte d'un dossier : l'identifiant d'action « nouveau terminal ici », stable même si la liste des
/// dossiers récents change d'ordre entre l'écran et le toucher.
fn dir_key(dir: &str) -> String {
    let mut h = DefaultHasher::new();
    dir.to_lowercase().hash(&mut h);
    format!("{:x}", h.finish() & 0xffff_ffff)
}

/// Le libellé d'état d'un terminal (sa première ligne) et son ton.
fn state(app: &App, id: TermId) -> (String, Option<Tone>) {
    let (_, label) = fawkes::ui::panes::status_color(app, id);
    let label = label.lines().next().unwrap_or_default().to_string();
    let tone = if app.ui.waiting.contains(&id) {
        Some(Tone::Warn)
    } else if app.failed.contains_key(&id) {
        Some(Tone::Danger)
    } else if app.live.get(&id).is_some_and(|l| fawkes::ui::panes::claude_status(l).is_some()) {
        Some(Tone::Ok)
    } else {
        None
    };
    (label, tone)
}

fn term_acts(app: &App, id: TermId) -> Vec<Act> {
    let asleep = !app.live.contains_key(&id);
    let mut acts = vec![Act::new(format!("redemarrer:{id}"), "Redémarrer").glyph("🔄")];
    acts.push(if asleep {
        Act::new(format!("reveiller:{id}"), "Réveiller").glyph("☀️")
    } else {
        Act::new(format!("veille:{id}"), "Mettre en veille").glyph("🌙")
    });
    acts.push(Act::new(format!("montrer:{id}"), "Montrer sur le PC").glyph("🖥️"));
    acts
}

/// Les dossiers proposés pour un nouveau terminal : les récents, puis les dépôts des groupes.
fn known_dirs(app: &App) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let candidates = app.st.recent_dirs.iter().cloned().chain(app.st.groups.iter().filter_map(|g| g.repo.clone()));
    for d in candidates {
        if !out.iter().any(|o| o.eq_ignore_ascii_case(&d)) {
            out.push(d);
        }
        if out.len() == 12 {
            break;
        }
    }
    out
}

pub fn screen(app: &mut App, path: &str, _cx: &mut Cx) -> Option<Screen> {
    if path.is_empty() {
        return Some(home(app));
    }
    if path == "nouveau" {
        return Some(new_terminal(app));
    }
    let id: TermId = path.strip_prefix("t/")?.parse().ok()?;
    let t = app.st.term(id)?;
    let (label, _) = state(app, id);
    let mut s = Screen::new(t.display_title()).subtitle(label).terminal(id.to_string());
    s.acts = term_acts(app, id);
    Some(s)
}

fn home(app: &App) -> Screen {
    let mut s = Screen::new("Fawkes");
    let waiting = app.ui.waiting.len();
    if waiting > 0 {
        let text = if waiting == 1 {
            "Une session Claude attend une réponse.".to_string()
        } else {
            format!("{waiting} sessions Claude attendent une réponse.")
        };
        s.note(text, Tone::Warn);
    }
    s.row(Row::new("Nouveau terminal").glyph("➕").subtitle("Shell ou Claude, dans un dossier récent").open("nouveau"));
    for g in &app.st.groups {
        let terms: Vec<_> = g.terminals.iter().filter_map(|id| app.st.term(*id)).collect();
        if terms.is_empty() {
            continue;
        }
        s.section(&g.name, terms.len().to_string());
        for t in terms {
            let (label, tone) = state(app, t.id);
            let fg = app.live.get(&t.id).and_then(|l| l.proc.foreground.as_ref()).map(|f| f.name.clone());
            let mut row = Row::new(t.display_title())
                .subtitle(format!("{label} · {}", short_path(&t.cwd)))
                .color(rgb(t.color.unwrap_or(g.color)))
                .glyph(if t.claude.is_some() { "✳" } else { "›_" })
                .detail(fg.unwrap_or_default())
                .open(format!("t/{}", t.id));
            if let Some(tone) = tone {
                row = row.tone(tone);
            }
            if app.ui.waiting.contains(&t.id) {
                row = row.badge("attend");
            }
            row.acts = term_acts(app, t.id);
            s.row(row);
        }
    }
    if app.st.terminals.is_empty() {
        s.empty("Aucun terminal : ouvrez-en un.");
    }
    s
}

fn new_terminal(app: &App) -> Screen {
    let mut s = Screen::new("Nouveau terminal").subtitle("Il s'ouvre dans Fawkes, sur le PC");
    let dirs = known_dirs(app);
    if !dirs.is_empty() {
        s.section("Dossiers récents", "");
    }
    for d in dirs {
        let key = dir_key(&d);
        let name = d.split(['\\', '/']).filter(|p| !p.is_empty()).next_back().unwrap_or(&d).to_string();
        s.row(
            Row::new(name)
                .subtitle(d.clone())
                .glyph("📁")
                .tap(Act::new(format!("shell:{key}"), "Ouvrir un shell").glyph("›_"))
                .act(Act::new(format!("shell:{key}"), "Ouvrir un shell").glyph("›_"))
                .act(Act::new(format!("claude:{key}"), "Ouvrir Claude ici").glyph("✳️")),
        );
    }
    s.section("Ailleurs", "");
    s.row(
        Row::new("Autre dossier…")
            .glyph("⌨️")
            .tap(Act::new("shell:saisi", "Ouvrir").input("Dossier sur le PC", r"C:\Users\…\projet")),
    );
    s
}

/// Le dossier visé par `shell:<clé>` ou `claude:<clé>`, contrôlé comme une demande `ouvrir` d'un autre module.
fn target_dir(app: &App, key: &str, input: Option<&str>) -> Result<String, String> {
    let dir = if key == "saisi" {
        input.map(str::trim).filter(|d| !d.is_empty()).ok_or("Indiquez un dossier.")?.to_string()
    } else {
        known_dirs(app).into_iter().find(|d| dir_key(d) == key).ok_or("Ce dossier n'est plus proposé : réessayez.")?
    };
    crate::suite::check_open(&dir, None)?;
    Ok(dir)
}

pub fn action(
    app: &mut App,
    _path: &str,
    action: &str,
    input: Option<&str>,
    cx: &mut Cx,
) -> Result<Option<String>, String> {
    let (verb, arg) = action.split_once(':').ok_or("action inconnue")?;
    if verb == "shell" || verb == "claude" {
        let dir = target_dir(app, arg, input)?;
        let group = app.st.group_for_dir(&dir);
        let claude = (verb == "claude").then(NewClaude::default);
        let id = app.create_terminal(NewTerm { cwd: dir, group, claude, ..Default::default() });
        return Ok(Some(format!("{OPEN_PREFIX}t/{id}")));
    }
    let id: TermId = arg.parse().map_err(|_| "terminal inconnu")?;
    if app.st.term(id).is_none() {
        return Err("Ce terminal n'existe plus.".into());
    }
    match verb {
        "redemarrer" => {
            app.restart(id);
            Ok(Some("Redémarré".into()))
        }
        "veille" => {
            app.sleep(id);
            Ok(Some("En veille".into()))
        }
        "reveiller" => {
            app.wake(id);
            Ok(Some("Réveillé".into()))
        }
        "montrer" => {
            app.select_term(id);
            cx.host.show(crate::suite::ID);
            Ok(Some("Montré sur le PC".into()))
        }
        _ => Err("action inconnue".into()),
    }
}

/// Une couleur de cellule, symbolique : le téléphone la résout dans son thème. Les couleurs « atténuées » deviennent
/// la couleur de base et le style `DIM`.
fn color(c: AnsiColor) -> (Color, bool) {
    match c {
        AnsiColor::Spec(rgb) => (Color::Rgb(rgb.r, rgb.g, rgb.b), false),
        AnsiColor::Indexed(i) => (Color::Indexed(i), false),
        AnsiColor::Named(n) => {
            let i = n as usize;
            match n {
                _ if i < 16 => (Color::Indexed(i as u8), false),
                NamedColor::DimBlack
                | NamedColor::DimRed
                | NamedColor::DimGreen
                | NamedColor::DimYellow
                | NamedColor::DimBlue
                | NamedColor::DimMagenta
                | NamedColor::DimCyan
                | NamedColor::DimWhite => (Color::Indexed((i - NamedColor::DimBlack as usize) as u8), true),
                NamedColor::DimForeground => (Color::Default, true),
                _ => (Color::Default, false),
            }
        }
    }
}

fn style(flags: Flags) -> u8 {
    let mut s = 0;
    if flags.contains(Flags::BOLD) {
        s |= term::BOLD;
    }
    if flags.contains(Flags::ITALIC) {
        s |= term::ITALIC;
    }
    if flags.intersects(Flags::ALL_UNDERLINES) {
        s |= term::UNDERLINE;
    }
    if flags.contains(Flags::INVERSE) {
        s |= term::INVERSE;
    }
    if flags.contains(Flags::DIM) {
        s |= term::DIM;
    }
    if flags.contains(Flags::STRIKEOUT) {
        s |= term::STRIKE;
    }
    s
}

/// Une ligne de la grille en segments de même style ; les blancs de fin sont retirés.
fn read_line(grid: &alacritty_terminal::grid::Grid<alacritty_terminal::term::cell::Cell>, line: GridLine) -> Line {
    let row = &grid[line];
    let cols = grid.columns();
    let mut runs: Vec<Run> = Vec::new();
    // Dernière colonne non vide (texte ou fond).
    let last = (0..cols).rev().find(|&c| {
        let cell = &row[Column(c)];
        cell.c != ' '
            || !matches!(cell.bg, AnsiColor::Named(NamedColor::Background))
            || cell.flags.contains(Flags::INVERSE)
    });
    let Some(last) = last else { return Line::default() };
    for c in 0..=last {
        let cell = &row[Column(c)];
        if cell.flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) {
            continue;
        }
        let (fg, fg_dim) = color(cell.fg);
        let (bg, _) = color(cell.bg);
        let mut flags = style(cell.flags);
        if fg_dim {
            flags |= term::DIM;
        }
        let mut text = String::new();
        text.push(if cell.flags.contains(Flags::HIDDEN) { ' ' } else { cell.c });
        if let Some(extra) = cell.zerowidth() {
            text.extend(extra.iter());
        }
        // Un blanc sans fond ni style rejoint le segment en cours, quelle que soit sa couleur de texte.
        let blank = cell.c == ' ' && bg == Color::Default && flags & (term::INVERSE | term::UNDERLINE) == 0;
        match runs.last_mut() {
            Some(r) if (r.fg == fg && r.bg == bg && r.flags == flags) || (blank && r.bg == bg) => {
                r.text.push_str(&text)
            }
            _ => runs.push(Run { col: c as u16, text, fg, bg, flags }),
        }
    }
    Line { runs }
}

pub fn term_view(app: &mut App, id: &str, history: usize) -> Option<TermView> {
    let id: TermId = id.parse().ok()?;
    let spec = app.st.term(id)?;
    let title = spec.display_title();
    let waiting = app.ui.waiting.contains(&id);
    let Some(live) = app.live.get(&id) else {
        return Some(TermView {
            cols: spec.cols,
            rows: spec.rows,
            lines: vec![Line::plain("Terminal en veille. Réveillez-le pour le reprendre.")],
            title,
            ended: true,
            ..Default::default()
        });
    };
    let term = live.rt.term.lock();
    let grid = term.grid();
    let (cols, screen, hist) = (grid.columns(), grid.screen_lines(), grid.history_size());
    let alt = term.mode().contains(TermMode::ALT_SCREEN);
    let kept = if alt { 0 } else { hist.min(history) };
    let mut lines = Vec::with_capacity(kept + screen);
    for l in -(kept as i32)..screen as i32 {
        lines.push(read_line(grid, GridLine(l)));
    }
    let cursor = term.mode().contains(TermMode::SHOW_CURSOR).then(|| {
        let p = grid.cursor.point;
        ((kept as i32 + p.line.0) as u32, p.column.0 as u16)
    });
    drop(term);
    let replies = if waiting { term::numbered_choices(&lines) } else { Vec::new() };
    Some(TermView {
        cols: cols as u16,
        rows: screen as u16,
        lines,
        cursor,
        title,
        alt_screen: alt,
        waiting,
        replies,
        ended: live.rt.is_exited(),
    })
}

fn key_event(key: egui::Key, modifiers: egui::Modifiers) -> egui::Event {
    egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers }
}

/// Une saisie du téléphone, encodée par Fawkes comme une frappe sur le PC.
pub fn term_input(app: &mut App, id: &str, input: &TermInput, cx: &mut Cx) -> Result<(), String> {
    let id: TermId = id.parse().map_err(|_| "terminal inconnu")?;
    let Some(live) = app.live.get(&id) else {
        return Err("Terminal en veille : réveillez-le d'abord.".into());
    };
    if live.rt.is_exited() {
        return Err("Le programme du terminal est terminé : redémarrez-le.".into());
    }
    let none = egui::Modifiers::NONE;
    let event = match input {
        TermInput::Text(t) => egui::Event::Text(t.replace(['\r', '\n'], "")),
        TermInput::Paste(t) => {
            let mode = *live.rt.term.lock().mode();
            live.rt.write(fawkes::term::input::paste_bytes(t, mode));
            return Ok(());
        }
        TermInput::Key(k) => match k {
            Key::Enter => key_event(egui::Key::Enter, none),
            Key::ShiftEnter => key_event(egui::Key::Enter, egui::Modifiers::SHIFT),
            Key::Escape => key_event(egui::Key::Escape, none),
            Key::Tab => key_event(egui::Key::Tab, none),
            Key::BackTab => key_event(egui::Key::Tab, egui::Modifiers::SHIFT),
            Key::Backspace => key_event(egui::Key::Backspace, none),
            Key::Delete => key_event(egui::Key::Delete, none),
            Key::Up => key_event(egui::Key::ArrowUp, none),
            Key::Down => key_event(egui::Key::ArrowDown, none),
            Key::Left => key_event(egui::Key::ArrowLeft, none),
            Key::Right => key_event(egui::Key::ArrowRight, none),
            Key::Home => key_event(egui::Key::Home, none),
            Key::End => key_event(egui::Key::End, none),
            Key::PageUp => key_event(egui::Key::PageUp, none),
            Key::PageDown => key_event(egui::Key::PageDown, none),
            Key::Ctrl(c) => {
                let key = egui::Key::from_name(&c.to_ascii_uppercase().to_string()).ok_or("touche inconnue")?;
                key_event(key, egui::Modifiers::CTRL)
            }
        },
    };
    fawkes::ui::panes::replay_keyboard(cx.host.ctx(), live, &[event]);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_are_shortened_and_keys_are_stable() {
        assert_eq!(short_path(r"C:\Users\aubin\DEV\Eyrie"), r"…\DEV\Eyrie");
        assert_eq!(short_path("C:"), "C:");
        assert_eq!(dir_key(r"C:\Dev\X"), dir_key(r"c:\dev\x"), "casse ignorée, comme sous Windows");
        assert_ne!(dir_key(r"C:\Dev\X"), dir_key(r"C:\Dev\Y"));
    }

    #[test]
    fn colors_stay_symbolic() {
        assert_eq!(color(AnsiColor::Named(NamedColor::Red)), (Color::Indexed(1), false));
        assert_eq!(color(AnsiColor::Named(NamedColor::BrightBlue)), (Color::Indexed(12), false));
        assert_eq!(color(AnsiColor::Named(NamedColor::DimGreen)), (Color::Indexed(2), true));
        assert_eq!(color(AnsiColor::Named(NamedColor::Foreground)), (Color::Default, false));
        assert_eq!(color(AnsiColor::Indexed(208)), (Color::Indexed(208), false));
    }

    #[test]
    fn a_grid_line_becomes_styled_runs() {
        use alacritty_terminal::event::VoidListener;
        use alacritty_terminal::term::{Config, Term, test::TermSize};
        use alacritty_terminal::vte::ansi::Processor;
        let mut term = Term::new(Config::default(), &TermSize::new(20, 3), VoidListener);
        let mut parser: Processor = Processor::new();
        parser.advance(&mut term, b"ok \x1b[31mrouge\x1b[0m   ");
        let line = read_line(term.grid(), GridLine(0));
        assert_eq!(line.text(), "ok rouge");
        assert_eq!(line.runs.len(), 2);
        assert_eq!((line.runs[1].col, line.runs[1].fg), (3, Color::Indexed(1)));
        assert_eq!(read_line(term.grid(), GridLine(2)), Line::default(), "ligne vide");
    }
}
