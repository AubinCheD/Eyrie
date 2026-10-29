//! Fawkes sur le téléphone : un bandeau par groupe (sa couleur, son icône, ses sessions Claude, ce qui y attend) et
//! ses terminaux, un terminal en direct, et ce qu'on y fait (saisie, touches, réponses rapides à Claude, nouveau
//! terminal, redémarrer, veille).
//!
//! Tout passe par l'API publique de Fawkes : `App::st` (groupes, terminaux), `App::live` (la grille de chaque
//! terminal, `TermRuntime`), `ui::panes::status_color` (le libellé d'état), `ui::panes::replay_keyboard` (la saisie,
//! encodée comme au clavier du PC, mode win32-input de ConPTY compris). Les écrans se construisent à partir de l'état
//! de Fawkes (`AppState`) et de ce qui est relevé des terminaux en marche (`Seen`).

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
use suite_sdk::mobile::ui::{Act, Band, FOLDER_ANY, FolderInput, Place, Row, Screen, Tone};

use fawkes::app::{App, NewClaude, NewTerm};
use fawkes::model::{AppState, Group, GroupId, Rgb, TermId, TermSpec};
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

/// Ce que Fawkes sait d'un terminal, en un état. Arrêté : en échec, en attente de son tour, sinon en veille. En
/// marche : Claude qui attend passe avant tout (c'est ce qu'il faut voir), puis l'échec, la fin du programme, et ce
/// que fait Claude ; Claude qui démarre, non identifié ou absent : un terminal qui tourne.
fn state_of(
    live: bool,
    waiting: bool,
    failed: bool,
    pending: bool,
    exited: bool,
    claude: Option<ClaudeStatus>,
) -> State {
    if !live {
        return if failed {
            State::Failed
        } else if pending {
            State::Starting
        } else {
            State::Asleep
        };
    }
    if waiting || claude == Some(ClaudeStatus::Waiting) {
        State::Waiting
    } else if failed {
        State::Failed
    } else if exited {
        State::Ended
    } else {
        match claude {
            Some(ClaudeStatus::Busy) => State::Working,
            Some(ClaudeStatus::Idle) => State::Ready,
            _ => State::Running,
        }
    }
}

fn seen_of(app: &App, id: TermId) -> Seen {
    let (_, label) = fawkes::ui::panes::status_color(app, id);
    let label = label.lines().next().unwrap_or_default().to_string();
    let (failed, pending) = (app.failed.contains_key(&id), app.pending_start(id));
    let Some(l) = app.live.get(&id) else {
        return Seen { state: state_of(false, false, failed, pending, false, None), label, ..Default::default() };
    };
    let waiting = app.ui.waiting.contains(&id);
    let claude = fawkes::ui::panes::claude_status(l);
    Seen {
        state: state_of(true, waiting, failed, pending, l.rt.is_exited(), claude),
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

/// Une session Claude sans Remote Control qui peut le prendre : Fawkes le refuse tant que Claude travaille ou attend
/// une réponse (`App::remote_control`).
fn remote_possible(s: &Seen) -> bool {
    s.claude && !s.remote && !matches!(s.state, State::Working | State::Waiting)
}

fn term_acts(t: &TermSpec, s: &Seen) -> Vec<Act> {
    let id = t.id;
    let mut acts = Vec::new();
    // Remote Control mène la session depuis l'application Claude : comme y écrire, avec le droit « Terminaux ».
    if remote_possible(s) {
        acts.push(Act::new(format!("remote:{id}"), "Activer Remote Control").glyph("📡").terminal());
    }
    // Redémarrer arrête ce qui tourne (une session Claude au travail…), et un terminal en veille repart sans l'écran
    // gardé (`App::restart`) : demandé d'abord, comme « Tout redémarrer ». Fini, en échec ou pas encore lancé, rien n'y
    // tourne.
    let title = t.display_title();
    let restart = Act::new(format!("redemarrer:{id}"), "Redémarrer").glyph("🔄");
    acts.push(match s.state {
        State::Asleep => {
            restart.confirm(format!("Relancer « {title} » sans son ancien écran ? « Réveiller » le garde."))
        }
        State::Ended | State::Failed | State::Starting => restart,
        _ => restart.confirm(format!("Redémarrer « {title} » ? Ce qui y tourne s'arrête, puis repart.")),
    });
    acts.push(if s.live {
        Act::new(format!("veille:{id}"), "Mettre en veille").glyph("🌙")
    } else {
        Act::new(format!("reveiller:{id}"), "Réveiller").glyph("☀️")
    });
    acts.push(Act::new(format!("montrer:{id}"), "Montrer sur le PC").glyph("🖥️"));
    acts.push(
        Act::new(format!("fermer:{id}"), "Fermer")
            .glyph("✖️")
            .danger()
            .confirm(format!("Fermer « {title} » ? Ce qui y tourne s'arrête ; il reste rouvrable sur le PC.")),
    );
    acts
}

/// La ligne d'un terminal : son état se lit au glyphe, au ton et au texte de droite (◐ Claude travaille, ✳ Claude,
/// 🌙 en veille) ; un bouton visible quand un geste s'impose (réveiller, relancer).
fn term_row(t: &TermSpec, g: &Group, s: &Seen) -> Row {
    // Fawkes garde la session d'un terminal où Claude s'est arrêté (pour la reprendre) : seul Claude qui tourne, ou
    // qui reprendra au réveil, fait un ✳.
    let claude = t.claude.as_ref().is_some_and(|c| c.running) || s.claude;
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
    /// … dont celles sans Remote Control qui peuvent le prendre (voir [`remote_possible`]).
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
        n.without_remote += remote_possible(s) as usize;
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

/// Le nom d'un dossier : son dernier élément.
fn dir_name(d: &str) -> String {
    d.split(['\\', '/']).rfind(|p| !p.is_empty()).unwrap_or(d).to_string()
}

/// Le menu d'un bandeau : celui du groupe sur le PC (`sidebar::group_menu`), avec ce qui a un effet. Modifier et
/// supprimer le groupe restent sur le PC.
fn group_acts(g: &Group, n: &Tally) -> Vec<Act> {
    let gid = g.id;
    let mut acts = Vec::new();
    if let Some(repo) = &g.repo {
        let name = dir_name(repo);
        acts.push(Act::new(format!("shell:g{gid}"), format!("Shell dans {name}")).glyph("›_").terminal());
        acts.push(Act::new(format!("claude:g{gid}"), format!("Claude dans {name}")).glyph("✳️").terminal());
    }
    acts.push(Act::screen("Nouveau terminal dans ce groupe…", format!("nouveau/g/{gid}")).glyph("➕"));
    if n.without_remote > 0 {
        acts.push(Act::new(format!("remote:g{gid}"), "Remote Control sur les sessions Claude").glyph("📡").terminal());
    }
    if n.live < n.terms {
        acts.push(Act::new(format!("reveiller:g{gid}"), "Tout réveiller").glyph("☀️"));
    }
    if n.live > 0 {
        acts.push(Act::new(format!("veille:g{gid}"), "Tout mettre en veille").glyph("🌙").confirm(format!(
            "Mettre en veille les terminaux de « {} » ? Ce qui y tourne s'arrête jusqu'au réveil.",
            g.name
        )));
    }
    if n.terms > 0 {
        acts.push(
            Act::new(format!("redemarrer:g{gid}"), "Tout redémarrer")
                .glyph("🔄")
                .danger()
                .confirm(restart_question(g, n)),
        );
    }
    acts.push(Act::new(format!("montrer:g{gid}"), "Montrer sur le PC").glyph("🖥️"));
    acts
}

/// La question de « Tout redémarrer ». Fawkes (`App::restart_group`) relance sans l'écran gardé à la mise en veille,
/// que « Tout réveiller » rejouerait : elle le dit dès qu'un terminal du groupe dort, et ne parle de ce qui tourne que
/// s'il tourne quelque chose.
fn restart_question(g: &Group, n: &Tally) -> String {
    let name = &g.name;
    let one = n.terms == 1;
    if n.asleep == n.terms {
        let (what, their) = if one {
            ("le terminal en veille".to_string(), "son")
        } else {
            (format!("les {} terminaux en veille", n.terms), "leur")
        };
        return format!("Relancer {what} de « {name} » sans {their} ancien écran ? « Tout réveiller » le garde.");
    }
    let what = if one { "le terminal".to_string() } else { format!("les {} terminaux", n.terms) };
    let mut q = if n.live > 0 {
        format!("Redémarrer {what} de « {name} » ? Ce qui y tourne s'arrête, puis repart.")
    } else {
        format!("Relancer {what} de « {name} » ?")
    };
    match n.asleep {
        0 => {}
        1 => q += " Celui en veille repart sans son ancien écran (« Tout réveiller » le garde).",
        k => q += &format!(" Les {k} en veille repartent sans leur ancien écran (« Tout réveiller » le garde)."),
    }
    q
}

/// Le bandeau d'un groupe : clé tirée de son identifiant (elle survit à un renommage), « ✻2 · 5 » (sessions Claude,
/// terminaux) comme la barre du PC, ce qui l'y fait remarquer (« 2 attendent », « 1 en échec », 🔔) et son menu.
fn group_band(g: &Group, n: &Tally, selected: bool, groups: usize) -> Band {
    let aside = if n.claude > 0 { format!("✻{} · {}", n.claude, n.terms) } else { n.terms.to_string() };
    let mut band = Band::new(format!("g{}", g.id), &g.name)
        .glyph(&g.icon)
        .color(rgb(g.color))
        .aside(aside)
        .open(opened(n, selected, groups));
    band.acts = group_acts(g, n);
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

/// Les dossiers proposés pour un nouveau terminal, 12 au plus : ceux du groupe visé d'abord (son dépôt, puis ceux de
/// ses terminaux, 6 au plus ; marqués `true`), les récents, puis les dépôts des groupes. Un seul par [`dir_key`] :
/// c'est la clé de sa ligne et de ses actions.
fn known_dirs(st: &AppState, group: Option<&Group>) -> Vec<(String, bool)> {
    let own = group.into_iter().flat_map(|g| {
        let cwds = g.terminals.iter().filter_map(|id| st.term(*id)).map(|t| t.cwd.clone());
        g.repo.clone().into_iter().chain(cwds).map(|d| (d, true))
    });
    let others = st.recent_dirs.iter().cloned().chain(st.groups.iter().filter_map(|g| g.repo.clone()));
    let mut out: Vec<(String, bool)> = Vec::new();
    for (d, mine) in own.chain(others.map(|d| (d, false))) {
        let full = mine && out.len() >= 6;
        if full || d.trim().is_empty() || out.iter().any(|(o, _)| dir_key(o) == dir_key(&d)) {
            continue;
        }
        out.push((d, mine));
        if out.len() == 12 {
            break;
        }
    }
    out
}

/// Le groupe d'un écran `nouveau/g/<groupe>` (menu d'un bandeau).
fn path_group(path: &str) -> Option<GroupId> {
    path.strip_prefix("nouveau/g/")?.parse().ok()
}

pub fn screen(app: &mut App, path: &str, _cx: &mut Cx) -> Option<Screen> {
    if path.is_empty() {
        let seen = seen(app);
        return Some(home(&app.st, &seen));
    }
    if path == "nouveau" {
        return Some(new_terminal(&app.st, None));
    }
    if let Some(gid) = path_group(path) {
        return Some(new_terminal(&app.st, Some(app.st.group(gid)?)));
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
    let mut s = Screen::new("Fawkes").act(Act::screen("Nouveau terminal", "nouveau").glyph("➕"));
    if st.terminals.len() > SEARCH_FROM {
        s = s.search("Chercher un terminal");
    }
    // En tête, les sessions Claude qui attendent, chacune à un toucher de son terminal ; elles restent aussi dans leur
    // groupe (une autre clé : une ligne par clé dans l'écran).
    let waiting: Vec<(&Group, &TermSpec)> = st
        .groups
        .iter()
        .flat_map(|g| g.terminals.iter().filter_map(|id| st.term(*id)).map(move |t| (g, t)))
        .filter(|(_, t)| seen.get(&t.id).is_some_and(|s| s.state == State::Waiting))
        .collect();
    if !waiting.is_empty() {
        s.band(
            Band::new("attend", "Claude attend").fixed().glyph("✳").tone(Tone::Warn).aside(waiting.len().to_string()),
        );
    }
    for (g, t) in waiting {
        let mut row = Row::new(t.display_title())
            .key(format!("attend:{}", t.id))
            .subtitle(format!("{} · {}", g.name, short_path(&t.cwd)))
            .color(rgb(t.color.unwrap_or(g.color)))
            .glyph("✳")
            .tone(Tone::Warn)
            .open(format!("t/{}", t.id));
        row.acts = term_acts(t, seen.get(&t.id).unwrap_or(&UNSEEN));
        s.row(row);
    }
    for g in &st.groups {
        let terms: Vec<&TermSpec> = g.terminals.iter().filter_map(|id| st.term(*id)).collect();
        let n = tally(&terms, seen);
        s.band(group_band(g, &n, g.id == st.selected_group, st.groups.len()));
        if terms.is_empty() {
            s.row(
                Row::new("Nouveau terminal dans ce groupe")
                    .key(format!("nouveau:g{}", g.id))
                    .glyph("➕")
                    .subtitle("Aucun terminal pour l'instant")
                    .open(format!("nouveau/g/{}", g.id)),
            );
        }
        for t in terms {
            s.row(term_row(t, g, seen.get(&t.id).unwrap_or(&UNSEEN)));
        }
    }
    s
}

/// `nouveau` : un shell ou Claude dans un dossier connu, ou dans un autre choisi dans les dossiers du PC ;
/// `nouveau/g/<groupe>` (menu d'un bandeau) : la même chose dans ce groupe, ses dossiers d'abord.
fn new_terminal(st: &AppState, group: Option<&Group>) -> Screen {
    let subtitle = match group {
        Some(g) => format!("Dans le groupe « {} », sur le PC", g.name),
        None => "Il s'ouvre dans Fawkes, sur le PC".to_string(),
    };
    let mut s = Screen::new("Nouveau terminal").subtitle(subtitle);
    let dirs = known_dirs(st, group);
    let mut section = None;
    for (d, mine) in &dirs {
        if section != Some(*mine) {
            s.section(if *mine { "Dossiers du groupe" } else { "Dossiers récents" }, "");
            section = Some(*mine);
        }
        let key = dir_key(d);
        s.row(
            Row::new(dir_name(d))
                .key(key.clone())
                .subtitle(d.clone())
                .glyph("📁")
                .tap(Act::new(format!("shell:{key}"), "Ouvrir un shell").glyph("›_").terminal())
                .act(Act::new(format!("shell:{key}"), "Ouvrir un shell").glyph("›_").terminal())
                .act(Act::new(format!("claude:{key}"), "Ouvrir Claude ici").glyph("✳️").terminal()),
        );
    }
    // Le sélecteur de dossiers du PC, ouvert dans le dépôt du groupe s'il y en a un, avec les dossiers que Fawkes
    // connaît en raccourcis ; une application plus ancienne montre un champ de texte. Le chemin choisi est contrôlé
    // comme une demande `ouvrir` (`target_dir`).
    let spec = FolderInput {
        want: FOLDER_ANY,
        start: group.and_then(|g| g.repo.clone()).unwrap_or_default(),
        places: dirs.iter().map(|(d, _)| Place { label: dir_name(d), path: d.clone(), glyph: "📁".into() }).collect(),
        ..Default::default()
    };
    let pick = |id: &str, label: &str| {
        Act::new(id, label)
            .input("Dossier sur le PC", r"C:\Users\…\projet")
            .folder("Dossier sur le PC", spec.clone())
            .terminal()
    };
    s.section("Ailleurs", "");
    s.row(
        Row::new("Autre dossier…")
            .key("autre")
            .subtitle("Choisir un dossier du PC")
            .glyph("📂")
            .tap(pick("shell:saisi", "Ouvrir un shell").glyph("›_"))
            .act(pick("shell:saisi", "Ouvrir un shell").glyph("›_"))
            .act(pick("claude:saisi", "Ouvrir Claude ici").glyph("✳️")),
    );
    s
}

/// Le dossier et le groupe visés par `shell:<clé>` ou `claude:<clé>` : un dossier que l'écran `path` propose, le dépôt
/// d'un groupe (`g<groupe>`, menu de son bandeau) ou un dossier saisi, contrôlé comme une demande `ouvrir` d'un autre
/// module. Le groupe : celui de l'écran (`nouveau/g/<groupe>`) ou du bandeau, sinon celui lié au dépôt du dossier.
fn target_dir(st: &AppState, path: &str, key: &str, input: Option<&str>) -> Result<(String, Option<GroupId>), String> {
    let mut group = match path_group(path) {
        Some(gid) => Some(st.group(gid).ok_or("Ce groupe n'existe plus.")?),
        None => None,
    };
    let dir = if key == "saisi" {
        input.map(str::trim).filter(|d| !d.is_empty()).ok_or("Indiquez un dossier.")?.to_string()
    } else if let Some(gid) = key.strip_prefix('g').and_then(|g| g.parse::<GroupId>().ok()) {
        // Les clés de dossier sont en hexadécimal : jamais de « g ».
        let g = st.group(gid).ok_or("Ce groupe n'existe plus.")?;
        group = Some(g);
        g.repo.clone().ok_or("Ce groupe n'a plus de dépôt.")?
    } else {
        known_dirs(st, group)
            .into_iter()
            .map(|(d, _)| d)
            .find(|d| dir_key(d) == key)
            .ok_or("Ce dossier n'est plus proposé : réessayez.")?
    };
    crate::suite::check_open(&dir, None)?;
    let group = group.map(|g| g.id).or_else(|| st.group_for_dir(&dir));
    Ok((dir, group))
}

/// Un geste sur un terminal ou sur tout un groupe.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Verb {
    Restart,
    Sleep,
    Wake,
    Show,
    Close,
    Remote,
}

/// Une action du téléphone, lue dans son identifiant.
#[derive(Debug, PartialEq, Eq)]
enum Cmd<'a> {
    /// `shell:<clé>`, `claude:<clé>` : un nouveau terminal (voir [`target_dir`]).
    New { claude: bool, key: &'a str },
    /// `veille:12` : le terminal 12.
    Term(Verb, TermId),
    /// `veille:g3` : tout le groupe 3 (menu de son bandeau).
    Group(Verb, GroupId),
}

fn parse(action: &str) -> Result<Cmd<'_>, String> {
    let (verb, arg) = action.split_once(':').ok_or("action inconnue")?;
    if verb == "shell" || verb == "claude" {
        return Ok(Cmd::New { claude: verb == "claude", key: arg });
    }
    let verb = match verb {
        "redemarrer" => Verb::Restart,
        "veille" => Verb::Sleep,
        "reveiller" => Verb::Wake,
        "montrer" => Verb::Show,
        "fermer" => Verb::Close,
        "remote" => Verb::Remote,
        _ => return Err("action inconnue".into()),
    };
    match arg.strip_prefix('g') {
        // Pas de quoi fermer tout un groupe depuis le téléphone.
        Some(_) if verb == Verb::Close => Err("action inconnue".into()),
        Some(gid) => Ok(Cmd::Group(verb, gid.parse().map_err(|_| "groupe inconnu")?)),
        None => Ok(Cmd::Term(verb, arg.parse().map_err(|_| "terminal inconnu")?)),
    }
}

pub fn action(
    app: &mut App,
    path: &str,
    action: &str,
    input: Option<&str>,
    cx: &mut Cx,
) -> Result<Option<String>, String> {
    match parse(action)? {
        Cmd::New { claude, key } => {
            let (dir, group) = target_dir(&app.st, path, key, input)?;
            let claude = claude.then(NewClaude::default);
            let id = app.create_terminal(NewTerm { cwd: dir, group, claude, ..Default::default() });
            Ok(Some(format!("{OPEN_PREFIX}t/{id}")))
        }
        Cmd::Term(verb, id) => term_action(app, verb, id, cx),
        Cmd::Group(verb, gid) => group_action(app, verb, gid, cx),
    }
}

fn term_action(app: &mut App, verb: Verb, id: TermId, cx: &mut Cx) -> Result<Option<String>, String> {
    if app.st.term(id).is_none() {
        return Err("Ce terminal n'existe plus.".into());
    }
    match verb {
        Verb::Restart => {
            app.restart(id);
            Ok(Some("Redémarré".into()))
        }
        Verb::Sleep => {
            app.sleep(id);
            Ok(Some("En veille".into()))
        }
        Verb::Wake => {
            app.wake(id);
            Ok(Some("Réveillé".into()))
        }
        Verb::Show => {
            app.select_term(id);
            cx.host.show(crate::suite::ID);
            Ok(Some("Montré sur le PC".into()))
        }
        Verb::Close => {
            app.close_terminal(id);
            // Le terminal n'existe plus : retour à la liste.
            Ok(Some(OPEN_PREFIX.to_string()))
        }
        Verb::Remote => {
            app.remote_control(id)?;
            Ok(Some("Remote Control activé : la session continue dans l'application Claude".into()))
        }
    }
}

/// Le menu d'un bandeau, comme celui du groupe sur le PC (`Action::SleepGroup`, `WakeGroup`, `RemoteControlGroup`) :
/// terminal par terminal, par les méthodes publiques de Fawkes ; tout redémarrer, d'un coup (`App::restart_group` :
/// terminal actif d'abord, lancements de Claude espacés).
fn group_action(app: &mut App, verb: Verb, gid: GroupId, cx: &mut Cx) -> Result<Option<String>, String> {
    if app.st.group(gid).is_none() {
        return Err("Ce groupe n'existe plus.".into());
    }
    let ids = app.terms_of(gid);
    match verb {
        Verb::Restart => {
            if ids.is_empty() {
                return Err("Ce groupe n'a aucun terminal.".into());
            }
            app.restart_group(gid);
            Ok(Some(count(ids.len(), "terminal redémarré", "terminaux redémarrés")))
        }
        Verb::Sleep => {
            let live: Vec<TermId> = ids.into_iter().filter(|id| app.live.contains_key(id)).collect();
            for &id in &live {
                app.sleep(id);
            }
            Ok(Some(count(live.len(), "terminal mis en veille", "terminaux mis en veille")))
        }
        Verb::Wake => {
            let asleep: Vec<TermId> = ids.into_iter().filter(|id| !app.live.contains_key(id)).collect();
            for &id in &asleep {
                app.wake(id);
            }
            Ok(Some(count(asleep.len(), "terminal réveillé", "terminaux réveillés")))
        }
        Verb::Remote => {
            let sessions: Vec<TermId> =
                ids.into_iter().filter(|id| app.live.get(id).is_some_and(|l| l.proc.claude_pid.is_some())).collect();
            let (mut done, mut skipped) = (0, Vec::new());
            for id in sessions {
                match app.remote_control(id) {
                    Ok(()) => done += 1,
                    Err(why) => skipped.push(format!(
                        "« {} » : {why}",
                        app.st.term(id).map(|t| t.display_title()).unwrap_or_default()
                    )),
                }
            }
            remote_report(done, &skipped).map(Some)
        }
        Verb::Show => {
            app.select_group(gid);
            cx.host.show(crate::suite::ID);
            Ok(Some("Montré sur le PC".into()))
        }
        Verb::Close => Err("action inconnue".into()),
    }
}

/// « 1 terminal réveillé », « 2 terminaux réveillés », « Aucun terminal réveillé ».
fn count(n: usize, one: &str, many: &str) -> String {
    match n {
        0 => format!("Aucun {one}"),
        1 => format!("1 {one}"),
        n => format!("{n} {many}"),
    }
}

/// Ce que Remote Control a donné sur les sessions d'un groupe, dit comme sur le PC ; un échec si aucune ne l'a.
fn remote_report(done: usize, skipped: &[String]) -> Result<String, String> {
    let why = skipped.join(" · ");
    match (done, skipped.len()) {
        (0, 0) => Err("Aucune session Claude ne tourne dans ce groupe.".into()),
        (0, _) => Err(format!("Remote Control non activé : {why}")),
        (d, 0) => Ok(format!("Remote Control activé sur {}", count(d, "session Claude", "sessions Claude"))),
        (d, k) => Ok(format!(
            "Remote Control activé sur {} ; non activé sur {k} : {why}",
            count(d, "session Claude", "sessions Claude")
        )),
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

/// Un collage du téléphone, sans ses caractères de contrôle (Échap, Ctrl+O…, Effacer) : hors collage encadré, Fawkes
/// l'écrit tel quel, et le shell les lirait comme des touches (`Échap [13;28;13;1;0;1_` est Entrée en mode win32). Il
/// garde ses tabulations et retours à la ligne. Le téléphone demande déjà avant de coller un tel texte.
fn paste_text(t: &str) -> String {
    t.chars().filter(|&c| !c.is_control() || matches!(c, '\t' | '\r' | '\n')).collect()
}

/// Une saisie du téléphone, sans retour à la ligne ni caractère de contrôle, tabulation à part. Seule exception :
/// Alt+touche, qui arrive en Échap suivi d'un caractère (Alt+Effacer : Échap et Effacer), d'un seul envoi.
fn typed_text(t: &str) -> String {
    let mut chars = t.chars();
    if let (Some('\u{1b}'), Some(c), None) = (chars.next(), chars.next(), chars.next())
        && (!c.is_control() || c == '\u{7f}')
    {
        return t.to_string();
    }
    t.chars().filter(|&c| !c.is_control() || c == '\t').collect()
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
        TermInput::Text(t) => egui::Event::Text(typed_text(t)),
        TermInput::Paste(t) => {
            let mode = *live.rt.term.lock().mode();
            live.rt.write(fawkes::term::input::paste_bytes(&paste_text(t), mode));
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
    use fawkes::model::ClaudeState;
    use suite_sdk::mobile::ui::{ActStyle, Block, Fold};

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
        let empty: Vec<_> = rows_under(&s, "g0").iter().map(|r| r.open.clone()).collect();
        assert_eq!(empty, [Some("nouveau/g/0".to_string())], "un groupe vide propose d'y ouvrir un terminal");
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

    /// Ce que Fawkes sait d'un terminal donne un état, dans cet ordre : arrêté (échec, démarrage prévu, veille), puis
    /// en marche (Claude qui attend, échec, programme terminé, Claude au travail ou prêt, le reste).
    #[test]
    fn what_fawkes_knows_gives_one_state() {
        use ClaudeStatus::{Busy, Idle, Starting, Unidentified, Waiting};
        let off = |failed, pending| state_of(false, false, failed, pending, false, None);
        assert_eq!(off(true, true), State::Failed, "arrêté en échec, même si un démarrage est prévu");
        assert_eq!(off(false, true), State::Starting, "son tour de démarrer : pas en veille");
        assert_eq!(off(false, false), State::Asleep);
        assert_eq!(state_of(false, true, false, false, false, Some(Waiting)), State::Asleep, "arrêté : rien n'attend");
        let on = |waiting, failed, exited, claude| state_of(true, waiting, failed, false, exited, claude);
        assert_eq!(on(true, true, true, Some(Idle)), State::Waiting, "Claude qui attend passe avant l'échec");
        assert_eq!(on(false, false, false, Some(Waiting)), State::Waiting, "relevé entre deux images de Fawkes");
        assert_eq!(on(false, true, true, Some(Busy)), State::Failed, "l'échec passe avant la fin");
        assert_eq!(on(false, false, true, Some(Busy)), State::Ended, "la fin passe avant ce que faisait Claude");
        assert_eq!(on(false, false, false, Some(Busy)), State::Working);
        assert_eq!(on(false, false, false, Some(Idle)), State::Ready);
        for claude in [Some(Starting), Some(Unidentified), None] {
            assert_eq!(on(false, false, false, claude), State::Running, "{claude:?}");
        }
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
        // Fawkes garde la session d'un Claude arrêté : le shell qui reste n'est pas une session Claude.
        let ran = |running| TermSpec { claude: Some(ClaudeState { running, ..Default::default() }), ..term(3, 5) };
        assert_eq!(term_row(&ran(false), &g, &running(State::Running)).glyph, "›_", "Claude s'est arrêté");
        assert_eq!(term_row(&ran(true), &g, &running(State::Running)).glyph, "✳", "Claude tourne, ou reprendra");
        // Le geste visible est aussi dans le menu : la liste blanche du pont l'accepte de l'un ou de l'autre.
        let menu: Vec<&str> = asleep.acts.iter().map(|a| a.id.as_str()).collect();
        assert!(menu.contains(&"reveiller:3") && !menu.contains(&"veille:3"));
    }

    /// Les sessions qui attendent sont en tête, dans un bandeau qui ne se plie pas, chacune à un toucher de son
    /// terminal ; elles restent aussi dans leur groupe. « Nouveau terminal » est dans la barre du haut.
    #[test]
    fn waiting_sessions_come_first_and_stay_in_their_group() {
        let (mut st, mut seen) = three_groups();
        st.groups[2].terminals.push(4);
        st.terminals.push(term(4, 7));
        seen.insert(4, claude(State::Waiting));
        seen.insert(1, claude(State::Waiting));
        let s = home(&st, &seen);
        let first = &bands(&s)[0];
        assert_eq!((first.0.as_str(), first.3, first.5), ("attend", Fold::No, Some(Tone::Warn)));
        assert_eq!(first.2, "2");
        let top: Vec<(&str, Option<&str>)> =
            rows_under(&s, "attend").iter().map(|r| (r.key.as_str(), r.open.as_deref())).collect();
        assert_eq!(top, [("attend:1", Some("t/1")), ("attend:4", Some("t/4"))], "dans l'ordre du PC");
        assert!(rows_under(&s, "g7").iter().any(|r| r.key == "4"), "toujours dans son groupe");
        let ozmo = bands(&s).into_iter().find(|b| b.0 == "g7").unwrap();
        assert_eq!((ozmo.3, ozmo.4.as_str()), (Fold::Open, "1 attend"), "un groupe qui attend s'ouvre");
        let keys: Vec<&str> =
            s.blocks.iter().filter_map(|b| if let Block::Row(r) = b { Some(r.key.as_str()) } else { None }).collect();
        let unique: std::collections::BTreeSet<&str> = keys.iter().copied().collect();
        assert_eq!(unique.len(), keys.len(), "une ligne par clé");
        let bar: Vec<_> = s.acts.iter().map(|a| (a.label.as_str(), a.open.as_deref(), a.reaches_pc())).collect();
        assert_eq!(bar, [("Nouveau terminal", Some("nouveau"), false)]);
        assert!(s.blocks.iter().all(|b| !matches!(b, Block::Row(r) if r.open.as_deref() == Some("nouveau"))));
        let (calm, calm_seen) = three_groups();
        assert!(bands(&home(&calm, &calm_seen)).iter().all(|b| b.0 != "attend"), "rien n'attend : pas de bandeau");
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

    /// Le menu d'un bandeau reprend celui du groupe sur le PC, avec seulement ce qui a un effet. Tout passe par la liste
    /// blanche du pont, sauf « Nouveau terminal dans ce groupe… », un écran ouvert par le téléphone. « Tout redémarrer »
    /// demande confirmation ; ni fermer ni supprimer un groupe depuis le téléphone.
    #[test]
    fn band_menus_offer_what_applies_and_are_whitelisted() {
        let (mut st, mut seen) = three_groups();
        st.groups[1].repo = Some(r"C:\DEV\Eyrie".into());
        seen.insert(1, claude(State::Ready));
        let s = home(&st, &seen);
        let menu = |s: &Screen, key: &str| -> Vec<String> {
            let acts = s.blocks.iter().find_map(|b| match b {
                Block::Section { key: k, acts, .. } if k == key => Some(acts.clone()),
                _ => None,
            });
            acts.unwrap().into_iter().map(|a| a.open.unwrap_or(a.id)).collect()
        };
        assert_eq!(
            menu(&s, "g5"),
            ["shell:g5", "claude:g5", "nouveau/g/5", "remote:g5", "veille:g5", "redemarrer:g5", "montrer:g5"],
            "dépôt, Claude sans Remote Control, tout éveillé"
        );
        assert_eq!(
            menu(&s, "g7"),
            ["nouveau/g/7", "reveiller:g7", "redemarrer:g7", "montrer:g7"],
            "sans dépôt, tout en veille"
        );
        assert_eq!(menu(&s, "g0"), ["nouveau/g/0", "montrer:g0"], "vide : rien à redémarrer");
        let ids = s.action_ids();
        for id in [
            "shell:g5",
            "claude:g5",
            "remote:g5",
            "veille:g5",
            "redemarrer:g5",
            "montrer:g5",
            "reveiller:g7",
            "redemarrer:g7",
            "montrer:g0",
        ] {
            assert!(ids.contains(id), "{id} dans la liste blanche");
        }
        assert!(s.opens().contains("nouveau/g/5"));
        assert!(!ids.iter().any(|id| id.starts_with("fermer:g")));
        let restart = s.all_acts().into_iter().find(|a| a.id == "redemarrer:g5").unwrap();
        assert_eq!(
            (restart.label.as_str(), restart.style, restart.confirm.as_deref()),
            (
                "Tout redémarrer",
                ActStyle::Danger,
                Some("Redémarrer les 2 terminaux de « Eyrie » ? Ce qui y tourne s'arrête, puis repart.")
            )
        );
        let one = s.all_acts().into_iter().find(|a| a.id == "redemarrer:g7").unwrap();
        assert_eq!(
            one.confirm.as_deref(),
            Some("Relancer le terminal en veille de « Ozmo » sans son ancien écran ? « Tout réveiller » le garde."),
            "tout en veille : rien ne tourne, et le redémarrage efface l'écran gardé"
        );
        let labels: Vec<String> = s
            .blocks
            .iter()
            .find_map(|b| match b {
                Block::Section { key, acts, .. } if key == "g5" => Some(acts.iter().map(|a| a.label.clone()).collect()),
                _ => None,
            })
            .unwrap();
        assert_eq!(labels[..2], ["Shell dans Eyrie", "Claude dans Eyrie"]);
        let sleep = s.all_acts().into_iter().find(|a| a.id == "veille:g5").unwrap();
        assert!(sleep.confirm.is_some(), "tout un groupe en veille : confirmé");
        // Fawkes refuse Remote Control à Claude qui travaille ou attend : ni dans le bandeau, ni sur la ligne.
        for busy in [State::Working, State::Waiting] {
            seen.insert(1, claude(busy));
            let ids = home(&st, &seen).action_ids();
            assert!(!ids.contains("remote:g5") && !ids.contains("remote:1"), "{busy:?}");
        }
        seen.insert(1, Seen { remote: true, ..claude(State::Ready) });
        let ids = home(&st, &seen).action_ids();
        assert!(!ids.contains("remote:g5") && !ids.contains("remote:1"), "déjà sous Remote Control");
    }

    /// « Tout redémarrer » efface l'écran que Fawkes a gardé à la mise en veille (« Tout réveiller » le rejoue) : la
    /// question le dit dès qu'un terminal du groupe dort, et ne parle de ce qui tourne que s'il tourne quelque chose.
    #[test]
    fn restarting_a_group_warns_that_sleeping_terminals_lose_their_screen() {
        let g = group(7, "Ozmo", &[1, 2, 3]);
        let ask = |n: Tally| restart_question(&g, &n);
        assert_eq!(
            ask(Tally { terms: 3, live: 3, ..Default::default() }),
            "Redémarrer les 3 terminaux de « Ozmo » ? Ce qui y tourne s'arrête, puis repart."
        );
        assert_eq!(
            ask(Tally { terms: 3, live: 2, asleep: 1, ..Default::default() }),
            "Redémarrer les 3 terminaux de « Ozmo » ? Ce qui y tourne s'arrête, puis repart. Celui en veille repart \
             sans son ancien écran (« Tout réveiller » le garde)."
        );
        assert_eq!(
            ask(Tally { terms: 3, live: 1, asleep: 2, ..Default::default() }),
            "Redémarrer les 3 terminaux de « Ozmo » ? Ce qui y tourne s'arrête, puis repart. Les 2 en veille \
             repartent sans leur ancien écran (« Tout réveiller » le garde)."
        );
        assert_eq!(
            ask(Tally { terms: 3, asleep: 3, ..Default::default() }),
            "Relancer les 3 terminaux en veille de « Ozmo » sans leur ancien écran ? « Tout réveiller » le garde."
        );
        assert_eq!(
            ask(Tally { terms: 3, failed: 2, asleep: 1, ..Default::default() }),
            "Relancer les 3 terminaux de « Ozmo » ? Celui en veille repart sans son ancien écran (« Tout réveiller » \
             le garde).",
            "rien ne tourne : en échec, ou en veille"
        );
        assert_eq!(ask(Tally { terms: 1, failed: 1, ..Default::default() }), "Relancer le terminal de « Ozmo » ?");
    }

    /// Redémarrer un terminal arrête ce qui y tourne (une session Claude au travail…) : confirmé, comme pour tout un
    /// groupe ; en veille, il repart sans son ancien écran, ce que dit la question. Fini ou en échec, rien n'y tourne :
    /// « Relancer » part d'un toucher.
    #[test]
    fn restarting_a_terminal_asks_first_when_something_runs() {
        let t = term(1, 5);
        let restart = |s: Seen| term_acts(&t, &s).into_iter().find(|a| a.id == "redemarrer:1").map(|a| a.confirm);
        let running_question = Some(Some("Redémarrer « t1 » ? Ce qui y tourne s'arrête, puis repart.".to_string()));
        for s in [claude(State::Working), claude(State::Waiting), claude(State::Ready), running(State::Running)] {
            assert_eq!(restart(s.clone()), running_question, "{s:?}");
        }
        let asleep = Some(Some("Relancer « t1 » sans son ancien écran ? « Réveiller » le garde.".to_string()));
        assert_eq!(restart(running(State::Asleep)), asleep);
        for s in [running(State::Ended), running(State::Failed), running(State::Starting)] {
            assert_eq!(restart(s.clone()), Some(None), "{s:?}");
        }
        let failed = term_row(&t, &group(5, "Eyrie", &[1]), &running(State::Failed));
        assert!(failed.trailing.iter().all(|a| a.confirm.is_none()), "{failed:?}");
    }

    /// Chaque action que proposent l'accueil et les écrans « nouveau terminal » est comprise, tout redémarrer un groupe
    /// compris ; fermer tout un groupe ne l'est pas.
    #[test]
    fn every_offered_action_is_understood() {
        let (mut st, mut seen) = three_groups();
        st.groups[1].repo = Some(r"C:\DEV\Eyrie".into());
        st.recent_dirs = vec![r"C:\DEV\Ozmo".into()];
        seen.insert(2, running(State::Failed));
        let screens = [home(&st, &seen), new_terminal(&st, None), new_terminal(&st, st.group(5))];
        for s in &screens {
            for id in s.action_ids() {
                assert!(parse(&id).is_ok(), "{id}");
            }
        }
        assert_eq!(parse("veille:g5"), Ok(Cmd::Group(Verb::Sleep, 5)));
        assert_eq!(parse("veille:12"), Ok(Cmd::Term(Verb::Sleep, 12)));
        assert_eq!(parse("claude:g5"), Ok(Cmd::New { claude: true, key: "g5" }));
        assert_eq!(parse("redemarrer:g5"), Ok(Cmd::Group(Verb::Restart, 5)));
        for refused in ["fermer:g5", "redemarrer:gx", "veille:gx", "veille:", "supprimer:g5", "veille"] {
            assert!(parse(refused).is_err(), "{refused}");
        }
    }

    /// `nouveau/g/<groupe>` (menu d'un bandeau) : les dossiers du groupe d'abord, et le terminal s'ouvre dans ce
    /// groupe ; « Shell dans <dépôt> » vise le dépôt du groupe. Un groupe disparu ou sans dépôt est refusé.
    #[test]
    fn a_new_terminal_from_a_band_opens_in_its_group() {
        let here = env!("CARGO_MANIFEST_DIR");
        let src = format!(r"{here}\src");
        let (mut st, _) = three_groups();
        st.groups[1].repo = Some(here.into());
        st.terminals[0].cwd = src.clone();
        st.recent_dirs = vec![r"C:\DEV\Ozmo".into(), here.to_uppercase()];
        let s = new_terminal(&st, st.group(5));
        assert!(s.subtitle.contains("« Eyrie »"), "{}", s.subtitle);
        let sections: Vec<&str> = s
            .blocks
            .iter()
            .filter_map(|b| match b {
                Block::Section { title, .. } => Some(title.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(sections, ["Dossiers du groupe", "Dossiers récents", "Ailleurs"]);
        let dirs: Vec<&str> = s
            .blocks
            .iter()
            .filter_map(|b| match b {
                Block::Row(r) if r.glyph == "📁" => Some(r.subtitle.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(dirs, [here, &src, r"C:\DEV\Eyrie", r"C:\DEV\Ozmo"], "dépôt, dossiers des terminaux, récents");
        assert_eq!(target_dir(&st, "nouveau/g/5", &dir_key(&src), None), Ok((src.clone(), Some(5))));
        assert_eq!(target_dir(&st, "", "g5", None), Ok((here.to_string(), Some(5))), "« Shell dans Eyrie »");
        assert_eq!(target_dir(&st, "nouveau/g/7", "saisi", Some(here)), Ok((here.to_string(), Some(7))));
        assert!(target_dir(&st, "", "g7", None).is_err(), "Ozmo n'a pas de dépôt");
        assert!(target_dir(&st, "nouveau/g/9", "saisi", Some(here)).is_err(), "groupe disparu");
        assert!(target_dir(&st, "nouveau/g/5", "nope", None).is_err(), "dossier qui n'est plus proposé");
        assert_eq!(
            (path_group("nouveau/g/12"), path_group("nouveau/g/x"), path_group("nouveau")),
            (Some(12), None, None)
        );
        let plain = new_terminal(&st, None);
        assert_eq!(plain.subtitle, "Il s'ouvre dans Fawkes, sur le PC");
        assert!(
            plain.blocks.iter().all(|b| !matches!(b, Block::Section { title, .. } if title == "Dossiers du groupe"))
        );
    }

    /// Ouvrir un shell ou Claude (`shell:…`, `claude:…` : dans un dossier, dans le dépôt d'un groupe, dans un autre
    /// dossier) ouvre un terminal, et Remote Control (`remote:…`) mène une session Claude d'ailleurs : le pont les
    /// refuse à un téléphone sans le droit « Terminaux ». Rien d'autre ne l'est.
    #[test]
    fn opening_a_shell_or_claude_is_marked_as_opening_a_terminal() {
        let (mut st, mut seen) = three_groups();
        seen.insert(1, claude(State::Ready));
        st.groups[1].repo = Some(r"C:\DEV\Eyrie".into());
        st.recent_dirs = vec![r"C:\DEV\Ozmo".into()];
        let mut opening = std::collections::BTreeSet::new();
        for s in [home(&st, &seen), new_terminal(&st, None), new_terminal(&st, st.group(5))] {
            for a in s.all_acts().into_iter().filter(|a| a.reaches_pc()) {
                let new = ["shell:", "claude:", "remote:"].iter().any(|p| a.id.starts_with(p));
                assert_eq!(a.terminal, new, "{}", a.id);
                if new {
                    opening.insert(a.id.clone());
                }
            }
        }
        let ozmo = dir_key(r"C:\DEV\Ozmo");
        let ids = ["shell:g5".to_string(), "claude:g5".into(), "remote:g5".into(), "remote:1".into()];
        for id in ids.into_iter().chain([format!("shell:{ozmo}"), format!("claude:{ozmo}")]) {
            assert!(opening.contains(&id), "{id} : {opening:?}");
        }
        assert!(opening.contains("shell:saisi") && opening.contains("claude:saisi"), "{opening:?}");
    }

    /// « Autre dossier… » ouvre le sélecteur de dossiers du PC (n'importe quel dossier), dans le dépôt du groupe s'il y
    /// en a un, avec les dossiers que Fawkes connaît en raccourcis ; une application plus ancienne montre un champ de
    /// texte. Le chemin choisi reste contrôlé comme une demande `ouvrir`.
    #[test]
    fn another_folder_is_picked_among_the_pc_folders() {
        let here = env!("CARGO_MANIFEST_DIR");
        let (mut st, _) = three_groups();
        st.groups[1].repo = Some(here.into());
        st.recent_dirs = vec![r"C:\DEV\Ozmo".into()];
        let other = |s: &Screen| {
            s.blocks
                .iter()
                .find_map(|b| match b {
                    Block::Row(r) if r.key == "autre" => Some(r.clone()),
                    _ => None,
                })
                .unwrap()
        };
        let row = other(&new_terminal(&st, st.group(5)));
        let acts: Vec<Act> = row.tap.into_iter().chain(row.acts).collect();
        assert_eq!(
            acts.iter().map(|a| a.id.as_str()).collect::<Vec<_>>(),
            ["shell:saisi", "shell:saisi", "claude:saisi"]
        );
        for a in &acts {
            let input = a.input.as_ref().unwrap();
            let folder = input.folder.as_ref().expect("sélecteur de dossiers du PC");
            assert_eq!((folder.want, folder.start.as_str()), (FOLDER_ANY, here));
            let places: Vec<&str> = folder.places.iter().map(|p| p.path.as_str()).collect();
            assert_eq!(places, [here, r"C:\DEV\Eyrie", r"C:\DEV\Ozmo"], "les dossiers proposés au-dessus");
            assert!(!input.placeholder.is_empty(), "le champ de texte d'une application plus ancienne");
        }
        let plain = other(&new_terminal(&st, None));
        assert_eq!(plain.tap.unwrap().input.unwrap().folder.unwrap().start, "", "hors groupe : la racine du sélecteur");
        assert!(target_dir(&st, "nouveau", "saisi", Some(r"\\hote.invalid\partage")).is_err(), "chemin réseau");
        assert!(target_dir(&st, "nouveau", "saisi", Some("  ")).is_err(), "rien de choisi");
        assert_eq!(target_dir(&st, "nouveau", "saisi", Some(here)).map(|(d, _)| d), Ok(here.to_string()));
    }

    #[test]
    fn remote_control_on_a_group_says_what_it_did() {
        assert!(remote_report(0, &[]).is_err(), "aucune session");
        let busy = vec!["« t1 » : Claude travaille, réessayer quand il aura fini".to_string()];
        assert_eq!(remote_report(0, &busy), Err(format!("Remote Control non activé : {}", busy[0])));
        assert_eq!(remote_report(2, &[]).as_deref(), Ok("Remote Control activé sur 2 sessions Claude"));
        assert_eq!(
            remote_report(1, &busy),
            Ok(format!("Remote Control activé sur 1 session Claude ; non activé sur 1 : {}", busy[0]))
        );
        assert_eq!(count(0, "terminal réveillé", "terminaux réveillés"), "Aucun terminal réveillé");
    }

    /// Un dossier écrit de plusieurs façons (casse des lettres accentuées comprise) n'est proposé qu'une fois : sa clé
    /// est celle de sa ligne et de ses actions.
    #[test]
    fn a_folder_is_offered_once_whatever_its_case() {
        let (mut st, _) = three_groups();
        st.recent_dirs = vec![
            r"C:\Users\Élodie\proj".into(),
            r"C:\Users\élodie\proj".into(),
            r"C:\USERS\ÉLODIE\PROJ".into(),
            r"C:\DEV\Ozmo".into(),
        ];
        let dirs: Vec<String> = known_dirs(&st, None).into_iter().map(|(d, _)| d).collect();
        assert_eq!(dirs, [r"C:\Users\Élodie\proj", r"C:\DEV\Ozmo"], "le premier venu");
        let s = new_terminal(&st, None);
        let keys: Vec<&str> =
            s.blocks.iter().filter_map(|b| if let Block::Row(r) = b { Some(r.key.as_str()) } else { None }).collect();
        let unique: std::collections::BTreeSet<&str> = keys.iter().copied().collect();
        assert_eq!((keys.len(), unique.len()), (3, 3), "deux dossiers et « Autre dossier… », une ligne par clé");
    }

    #[test]
    fn paths_are_shortened_and_keys_are_stable() {
        assert_eq!(short_path(r"C:\Users\aubin\DEV\Eyrie"), r"…\DEV\Eyrie");
        assert_eq!(short_path("C:"), "C:");
        assert_eq!(dir_key(r"C:\Dev\X"), dir_key(r"c:\dev\x"), "casse ignorée, comme sous Windows");
        assert_ne!(dir_key(r"C:\Dev\X"), dir_key(r"C:\Dev\Y"));
    }

    #[test]
    fn a_phone_paste_keeps_no_control_character_but_tabs_and_line_breaks() {
        // Échap et la suite que ConPTY lit comme Entrée (le mode win32 de Fawkes), Ctrl+O, Effacer, un CSI en C1.
        assert_eq!(paste_text("ls\u{1b}[13;28;13;1;0;1_whoami"), "ls[13;28;13;1;0;1_whoami");
        assert_eq!(paste_text("a\u{f}b\u{7f}c\u{9b}d\u{0}"), "abcd");
        assert_eq!(paste_text("cargo\ttest\r\ngit status\n"), "cargo\ttest\r\ngit status\n");
    }

    #[test]
    fn a_phone_text_keeps_escape_only_as_alt() {
        // Alt+lettre et Alt+Effacer : Échap puis un caractère, d'un seul envoi.
        assert_eq!(typed_text("\u{1b}b"), "\u{1b}b");
        assert_eq!(typed_text("\u{1b}."), "\u{1b}.");
        assert_eq!(typed_text("\u{1b}\u{7f}"), "\u{1b}\u{7f}");
        // Ailleurs, Échap et les autres contrôles ne passent pas ; les retours non plus, comme avant.
        assert_eq!(typed_text("a\u{1b}[13;28;13;1;0;1_b"), "a[13;28;13;1;0;1_b");
        assert_eq!(typed_text("\u{1b}\r"), "");
        assert_eq!(typed_text("\u{1b}\u{1b}"), "");
        assert_eq!(typed_text("\u{1b}"), "");
        assert_eq!(typed_text("l\u{f}s\r\n"), "ls");
        assert_eq!(typed_text("a\tb"), "a\tb", "la tabulation reste : elle complète dans un shell");
        assert_eq!(typed_text("3"), "3");
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
