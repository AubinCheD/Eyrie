//! Fawkes sur le téléphone : un bandeau par groupe (sa couleur, son icône, ses sessions Claude, ce qui y attend) et
//! ses terminaux, un terminal en direct, et ce qu'on y fait (saisie, touches, réponses rapides à Claude, nouveau
//! terminal, redémarrer, veille).
//!
//! Tout passe par l'API publique de Fawkes : `App::st` (groupes, terminaux), `App::live` (la grille de chaque
//! terminal, `TermRuntime`), `ui::panes::status_color` (le libellé d'état), `ui::panes::replay_keyboard` (la saisie,
//! encodée comme au clavier du PC, mode win32-input de ConPTY compris). Les écrans se construisent à partir de l'état
//! de Fawkes (`AppState`) et de ce qui est relevé des terminaux en marche ([`Seen`]).

use std::collections::HashMap;
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
use suite_sdk::mobile::ui::{Act, Band, Row, Screen, Tone};

use fawkes::app::{App, NewClaude, NewTerm};
use fawkes::model::{AppState, Group, Rgb, TermId, TermSpec};
use fawkes::ui::panes::ClaudeStatus;

/// Au-delà de ce nombre de terminaux, l'accueil a un champ de recherche.
const SEARCH_FROM: usize = 8;

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

/// L'état d'un terminal, tel que le téléphone le distingue.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum State {
    /// Claude attend une réponse.
    Waiting,
    /// Le lancement a échoué.
    Failed,
    /// Le programme du terminal s'est terminé.
    Ended,
    /// Claude travaille.
    Working,
    /// Claude est prêt.
    Ready,
    /// Un shell, ou Claude qui démarre.
    Running,
    /// Démarrage prévu (Fawkes espace les lancements).
    Starting,
    /// En veille, ou pas encore démarré (groupe jamais affiché sur le PC).
    #[default]
    Asleep,
}

/// Ce que Fawkes sait d'un terminal et que son état enregistré n'a pas : relevé dans `App` à chaque écran.
#[derive(Clone, Debug, Default)]
struct Seen {
    state: State,
    /// Le libellé d'état de Fawkes, sa première ligne (« Claude travaille », « En veille »…).
    label: String,
    /// Le terminal tourne (sinon : en veille, en échec, ou son démarrage attend).
    live: bool,
    /// Une session Claude y tourne…
    claude: bool,
    /// … avec Remote Control.
    remote: bool,
    /// La cloche a sonné.
    bell: bool,
    /// Le programme au premier plan.
    foreground: Option<String>,
}

/// Un terminal dont rien n'a été relevé : comme en veille.
static UNSEEN: Seen = Seen {
    state: State::Asleep,
    label: String::new(),
    live: false,
    claude: false,
    remote: false,
    bell: false,
    foreground: None,
};

fn seen_of(app: &App, id: TermId) -> Seen {
    let (_, label) = fawkes::ui::panes::status_color(app, id);
    let label = label.lines().next().unwrap_or_default().to_string();
    let Some(l) = app.live.get(&id) else {
        let state = if app.failed.contains_key(&id) {
            State::Failed
        } else if app.pending_start(id) {
            State::Starting
        } else {
            State::Asleep
        };
        return Seen { state, label, ..Default::default() };
    };
    let state = if app.ui.waiting.contains(&id) {
        State::Waiting
    } else if app.failed.contains_key(&id) {
        State::Failed
    } else if l.rt.is_exited() {
        State::Ended
    } else {
        match fawkes::ui::panes::claude_status(l) {
            Some(ClaudeStatus::Busy) => State::Working,
            Some(ClaudeStatus::Idle) => State::Ready,
            _ => State::Running,
        }
    };
    Seen {
        state,
        label,
        live: true,
        claude: l.proc.claude_pid.is_some(),
        remote: l.proc.claude.as_ref().is_some_and(|c| c.remote_control()),
        bell: l.input.bell,
        foreground: l.proc.foreground.as_ref().map(|f| f.name.clone()),
    }
}

/// Ce qui est relevé de chaque terminal.
fn seen(app: &App) -> HashMap<TermId, Seen> {
    app.st.terminals.iter().map(|t| (t.id, seen_of(app, t.id))).collect()
}

fn term_acts(t: &TermSpec, s: &Seen) -> Vec<Act> {
    let id = t.id;
    let mut acts = Vec::new();
    if s.claude && !s.remote {
        acts.push(Act::new(format!("remote:{id}"), "Activer Remote Control").glyph("📡"));
    }
    acts.push(Act::new(format!("redemarrer:{id}"), "Redémarrer").glyph("🔄"));
    acts.push(if s.live {
        Act::new(format!("veille:{id}"), "Mettre en veille").glyph("🌙")
    } else {
        Act::new(format!("reveiller:{id}"), "Réveiller").glyph("☀️")
    });
    acts.push(Act::new(format!("montrer:{id}"), "Montrer sur le PC").glyph("🖥️"));
    acts.push(Act::new(format!("fermer:{id}"), "Fermer").glyph("✖️").danger().confirm(format!(
        "Fermer « {} » ? Ce qui y tourne s'arrête ; il reste rouvrable sur le PC.",
        t.display_title()
    )));
    acts
}

/// La ligne d'un terminal : son état se lit au glyphe, au ton et au texte de droite (◐ Claude travaille, ✳ Claude,
/// 🌙 en veille) ; un bouton visible quand un geste s'impose (réveiller, relancer).
fn term_row(t: &TermSpec, g: &Group, s: &Seen) -> Row {
    let claude = t.claude.is_some() || s.claude;
    let (glyph, detail) = match s.state {
        State::Working => ("◐", "travaille".to_string()),
        State::Asleep => ("🌙", "en veille".to_string()),
        _ => (if claude { "✳" } else { "›_" }, s.foreground.clone().unwrap_or_default()),
    };
    let id = t.id;
    let mut row = Row::new(t.display_title())
        .key(id.to_string())
        .subtitle(format!("{} · {}", s.label, short_path(&t.cwd)))
        .color(rgb(t.color.unwrap_or(g.color)))
        .glyph(glyph)
        .detail(detail)
        .open(format!("t/{id}"));
    row = match s.state {
        State::Waiting => row.tone(Tone::Warn).badge("attend"),
        State::Failed => {
            row.tone(Tone::Danger).badge("échec").trailing(Act::new(format!("redemarrer:{id}"), "Relancer").glyph("🔄"))
        }
        State::Ended => row.trailing(Act::new(format!("redemarrer:{id}"), "Redémarrer").glyph("🔄")),
        State::Asleep => row.trailing(Act::new(format!("reveiller:{id}"), "Réveiller").glyph("☀️")),
        State::Working => row.tone(Tone::Info),
        State::Ready => row.tone(Tone::Ok),
        State::Running | State::Starting => row,
    };
    row.acts = term_acts(t, s);
    row
}

/// Les comptes d'un groupe, comme sa ligne dans la barre latérale du PC.
#[derive(Debug, Default, PartialEq)]
struct Tally {
    terms: usize,
    /// Sessions Claude qui tournent (« ✻2 »)…
    claude: usize,
    /// … dont celles sans Remote Control.
    without_remote: usize,
    waiting: usize,
    failed: usize,
    live: usize,
    asleep: usize,
    bell: bool,
}

fn tally(terms: &[&TermSpec], seen: &HashMap<TermId, Seen>) -> Tally {
    let mut n = Tally { terms: terms.len(), ..Default::default() };
    for t in terms {
        let s = seen.get(&t.id).unwrap_or(&UNSEEN);
        n.claude += s.claude as usize;
        n.without_remote += (s.claude && !s.remote) as usize;
        n.waiting += (s.state == State::Waiting) as usize;
        n.failed += (s.state == State::Failed) as usize;
        n.live += s.live as usize;
        n.asleep += (s.state == State::Asleep) as usize;
        n.bell |= s.bell;
    }
    n
}

/// Ouvert au départ ? Toujours s'il attend, est en échec ou sonne, si c'est le groupe affiché sur le PC ou s'il n'y a
/// que deux groupes ; sinon fermé s'il est vide ou tout en veille. Le téléphone retient ensuite le choix de
/// l'utilisateur.
fn opened(n: &Tally, selected: bool, groups: usize) -> bool {
    if n.waiting > 0 || n.failed > 0 || n.bell || selected || groups <= 2 {
        return true;
    }
    n.asleep < n.terms
}

/// Le bandeau d'un groupe : clé tirée de son identifiant (elle survit à un renommage), « ✻2 · 5 » (sessions Claude,
/// terminaux) comme la barre du PC, et ce qui l'y fait remarquer : « 2 attendent », « 1 en échec », 🔔.
fn group_band(g: &Group, n: &Tally, selected: bool, groups: usize) -> Band {
    let aside = if n.claude > 0 { format!("✻{} · {}", n.claude, n.terms) } else { n.terms.to_string() };
    let band = Band::new(format!("g{}", g.id), &g.name)
        .glyph(&g.icon)
        .color(rgb(g.color))
        .aside(aside)
        .open(opened(n, selected, groups));
    if n.waiting > 0 {
        let text = if n.waiting == 1 { "1 attend".to_string() } else { format!("{} attendent", n.waiting) };
        band.badge(text).tone(Tone::Warn)
    } else if n.failed > 0 {
        band.badge(format!("{} en échec", n.failed)).tone(Tone::Danger)
    } else if n.bell {
        band.badge("🔔")
    } else {
        band
    }
}

/// Les sessions Claude qui attendent, avec leur terminal : la vue d'ensemble du téléphone y mène directement.
pub fn waiting(app: &App) -> Vec<(String, Option<String>)> {
    app.st
        .terminals
        .iter()
        .filter(|t| app.ui.waiting.contains(&t.id))
        .map(|t| (format!("Claude attend : {}", t.display_title()), Some(format!("t/{}", t.id))))
        .collect()
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
        let seen = seen(app);
        return Some(home(&app.st, &seen));
    }
    if path == "nouveau" {
        return Some(new_terminal(app));
    }
    let id: TermId = path.strip_prefix("t/")?.parse().ok()?;
    let t = app.st.term(id)?;
    let s = seen_of(app, id);
    let mut screen = Screen::new(t.display_title()).subtitle(&s.label).terminal(id.to_string());
    screen.acts = term_acts(t, &s);
    Some(screen)
}

/// L'accueil : un bandeau par groupe, dans l'ordre du PC, groupes vides compris.
fn home(st: &AppState, seen: &HashMap<TermId, Seen>) -> Screen {
    let mut s = Screen::new("Fawkes");
    if st.terminals.len() > SEARCH_FROM {
        s = s.search("Chercher un terminal");
    }
    let waiting = st.terminals.iter().filter(|t| seen.get(&t.id).is_some_and(|s| s.state == State::Waiting)).count();
    if waiting > 0 {
        let text = if waiting == 1 {
            "Une session Claude attend une réponse.".to_string()
        } else {
            format!("{waiting} sessions Claude attendent une réponse.")
        };
        s.note(text, Tone::Warn);
    }
    s.row(Row::new("Nouveau terminal").glyph("➕").subtitle("Shell ou Claude, dans un dossier récent").open("nouveau"));
    for g in &st.groups {
        let terms: Vec<&TermSpec> = g.terminals.iter().filter_map(|id| st.term(*id)).collect();
        let n = tally(&terms, seen);
        s.band(group_band(g, &n, g.id == st.selected_group, st.groups.len()));
        for t in terms {
            s.row(term_row(t, g, seen.get(&t.id).unwrap_or(&UNSEEN)));
        }
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
        let name = d.split(['\\', '/']).rfind(|p| !p.is_empty()).unwrap_or(&d).to_string();
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
        "fermer" => {
            app.close_terminal(id);
            // Le terminal n'existe plus : retour à la liste.
            Ok(Some(OPEN_PREFIX.to_string()))
        }
        "remote" => {
            app.remote_control(id)?;
            Ok(Some("Remote Control activé : la session continue dans l'application Claude".into()))
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
    use fawkes::model::GroupId;
    use suite_sdk::mobile::ui::{Block, Fold};

    use super::*;

    fn group(id: GroupId, name: &str, terms: &[TermId]) -> Group {
        Group {
            id,
            name: name.into(),
            icon: "🦉".into(),
            color: Rgb(0x12, 0x34, 0x56),
            terminals: terms.to_vec(),
            ..Default::default()
        }
    }

    fn term(id: TermId, group: GroupId) -> TermSpec {
        TermSpec { id, group, title: Some(format!("t{id}")), cwd: r"C:\DEV\Eyrie".into(), ..Default::default() }
    }

    fn running(state: State) -> Seen {
        let live = !matches!(state, State::Asleep | State::Failed | State::Starting);
        Seen { state, live, label: format!("{state:?}"), ..Default::default() }
    }

    fn claude(state: State) -> Seen {
        Seen { claude: true, ..running(state) }
    }

    /// Trois groupes dans l'ordre du PC : Général (vide), Eyrie (affiché sur le PC : Claude qui travaille, un shell),
    /// Ozmo (en veille).
    fn three_groups() -> (AppState, HashMap<TermId, Seen>) {
        let st = AppState {
            groups: vec![group(0, "Général", &[]), group(5, "Eyrie", &[1, 2]), group(7, "Ozmo", &[3])],
            terminals: vec![term(1, 5), term(2, 5), term(3, 7)],
            selected_group: 5,
            ..Default::default()
        };
        let seen =
            HashMap::from([(1, claude(State::Working)), (2, running(State::Running)), (3, running(State::Asleep))]);
        (st, seen)
    }

    /// Les bandeaux d'un écran : (clé, titre, à droite, pliage, pastille, ton).
    fn bands(s: &Screen) -> Vec<(String, String, String, Fold, String, Option<Tone>)> {
        s.blocks
            .iter()
            .filter_map(|b| match b {
                Block::Section { key, title, aside, fold, badge, tone, .. } => {
                    Some((key.clone(), title.clone(), aside.clone(), *fold, badge.clone(), *tone))
                }
                _ => None,
            })
            .collect()
    }

    /// Les lignes sous le bandeau `key`, jusqu'au suivant.
    fn rows_under<'a>(s: &'a Screen, key: &str) -> Vec<&'a Row> {
        let start = s.blocks.iter().position(|b| matches!(b, Block::Section { key: k, .. } if k == key));
        s.blocks[start.expect(key) + 1..]
            .iter()
            .take_while(|b| !matches!(b, Block::Section { .. }))
            .filter_map(|b| match b {
                Block::Row(r) => Some(r),
                _ => None,
            })
            .collect()
    }

    /// Un bandeau par groupe, dans l'ordre du PC, groupes vides compris : la clé est l'identifiant du groupe (elle
    /// survit à un renommage), « ✻1 · 2 » comme la barre du PC ; chaque ligne a pour clé son terminal.
    #[test]
    fn every_group_is_a_band_in_the_pc_order_empty_ones_included() {
        let (st, seen) = three_groups();
        let s = home(&st, &seen);
        let b: Vec<_> = bands(&s).into_iter().filter(|b| b.0.starts_with('g')).collect();
        assert_eq!(
            b.iter().map(|b| (b.0.as_str(), b.1.as_str(), b.2.as_str())).collect::<Vec<_>>(),
            [("g0", "Général", "0"), ("g5", "Eyrie", "✻1 · 2"), ("g7", "Ozmo", "1")]
        );
        assert_eq!(b.iter().map(|b| b.3).collect::<Vec<_>>(), [Fold::Closed, Fold::Open, Fold::Closed]);
        let eyrie = s.blocks.iter().find_map(|b| match b {
            Block::Section { key, color, glyph, .. } if key == "g5" => Some((*color, glyph.clone())),
            _ => None,
        });
        assert_eq!(eyrie, Some((Some(0x123456), "🦉".to_string())), "couleur et icône du groupe");
        let keys: Vec<&str> = rows_under(&s, "g5").iter().map(|r| r.key.as_str()).collect();
        assert_eq!(keys, ["1", "2"]);
        assert!(rows_under(&s, "g0").is_empty());
    }

    /// Ouvert s'il attend, est en échec ou sonne, s'il est affiché sur le PC ou s'il n'y a que deux groupes ; sinon
    /// fermé s'il est vide ou tout en veille.
    #[test]
    fn the_default_fold_opens_what_needs_looking_at() {
        let empty = Tally::default();
        assert!(!opened(&empty, false, 3), "vide");
        assert!(opened(&empty, false, 2), "deux groupes au plus : tout ouvert");
        assert!(opened(&empty, true, 3), "le groupe affiché sur le PC");
        let asleep = Tally { terms: 2, asleep: 2, ..Default::default() };
        assert!(!opened(&asleep, false, 3), "tout en veille");
        assert!(opened(&Tally { bell: true, ..asleep }, false, 3), "la cloche a sonné");
        let alive = Tally { terms: 2, asleep: 1, live: 1, ..Default::default() };
        assert!(opened(&alive, false, 3));
        assert!(opened(&Tally { waiting: 1, ..Default::default() }, false, 9));
        assert!(opened(&Tally { failed: 1, ..Default::default() }, false, 9));
    }

    /// La pastille dit ce qui attend, sinon ce qui a échoué, sinon la cloche ; elle reste visible bandeau plié.
    #[test]
    fn a_band_shows_who_waits_then_who_failed_then_the_bell() {
        let g = group(5, "Eyrie", &[]);
        let badge = |n: Tally| {
            let b = group_band(&g, &n, false, 3);
            (b.badge, b.tone)
        };
        let two = Tally { terms: 3, waiting: 2, failed: 1, bell: true, ..Default::default() };
        assert_eq!(badge(two), ("2 attendent".into(), Some(Tone::Warn)));
        assert_eq!(badge(Tally { terms: 1, waiting: 1, ..Default::default() }), ("1 attend".into(), Some(Tone::Warn)));
        let failed = Tally { terms: 2, failed: 1, bell: true, ..Default::default() };
        assert_eq!(badge(failed), ("1 en échec".into(), Some(Tone::Danger)));
        assert_eq!(badge(Tally { terms: 1, live: 1, bell: true, ..Default::default() }), ("🔔".into(), None));
        assert_eq!(badge(Tally { terms: 1, live: 1, ..Default::default() }), (String::new(), None));
        let renamed = group(5, "Eyrie (renommé)", &[]);
        assert_eq!(group_band(&renamed, &Tally::default(), false, 3).key, "g5");
    }

    /// Claude qui travaille, Claude prêt, en veille, en échec, qui attend : chaque état a son glyphe ou son ton, et un
    /// bouton visible quand un geste s'impose.
    #[test]
    fn terminal_states_are_told_apart() {
        let g = group(5, "Eyrie", &[]);
        let t = term(3, 5);
        let row = |s: Seen| term_row(&t, &g, &s);
        let trailing = |r: &Row| r.trailing.as_ref().map(|a| a.id.clone());
        let working = row(claude(State::Working));
        assert_eq!(
            (working.glyph.as_str(), working.detail.as_str(), working.tone),
            ("◐", "travaille", Some(Tone::Info))
        );
        let ready = row(claude(State::Ready));
        assert_eq!((ready.glyph.as_str(), ready.tone), ("✳", Some(Tone::Ok)));
        let asleep = row(running(State::Asleep));
        assert_eq!((asleep.glyph.as_str(), asleep.detail.as_str()), ("🌙", "en veille"));
        assert_eq!(trailing(&asleep).as_deref(), Some("reveiller:3"));
        let failed = row(running(State::Failed));
        assert_eq!((failed.tone, trailing(&failed).as_deref()), (Some(Tone::Danger), Some("redemarrer:3")));
        let waiting = row(claude(State::Waiting));
        assert_eq!((waiting.tone, waiting.badge.as_str()), (Some(Tone::Warn), "attend"));
        let shell = row(running(State::Running));
        assert_eq!((shell.glyph.as_str(), shell.tone, trailing(&shell)), ("›_", None, None));
        assert_eq!((shell.key.as_str(), shell.open.as_deref()), ("3", Some("t/3")));
        // Le geste visible est aussi dans le menu : la liste blanche du pont l'accepte de l'un ou de l'autre.
        let menu: Vec<&str> = asleep.acts.iter().map(|a| a.id.as_str()).collect();
        assert!(menu.contains(&"reveiller:3") && !menu.contains(&"veille:3"));
    }

    #[test]
    fn a_search_field_appears_beyond_eight_terminals() {
        let ids: Vec<TermId> = (1..=9).collect();
        let mut st = AppState { groups: vec![group(0, "Général", &ids[..8])], ..Default::default() };
        st.terminals = ids[..8].iter().map(|id| term(*id, 0)).collect();
        assert!(home(&st, &HashMap::new()).search.is_empty());
        st.groups[0].terminals.push(9);
        st.terminals.push(term(9, 0));
        assert!(!home(&st, &HashMap::new()).search.is_empty());
    }

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
