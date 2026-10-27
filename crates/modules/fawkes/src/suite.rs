//! L'adaptateur : Fawkes comme module de la suite.
//!
//! On refait ce que fait `Fawkes/src/main.rs`, sans `run_native` :
//! - même verrou d'instance que `fawkes.exe` ;
//! - **thème** : Fawkes lit sa palette à chaque dessin (`theme::p()`) ; pendant son affichage, son
//!   réglage de thème vaut celui de la suite, puis reprend sa valeur (son état enregistré garde le
//!   thème choisi dans Fawkes seul) ;
//! - **pas de barre de titre dans sa case** : pendant son affichage, il se croit sous celle de Windows (la barre
//!   d'Eyrie) et ne montre que ses menus et sa recherche, sans réduire, agrandir et fermer ; son réglage enregistré
//!   ne change pas ;
//! - **réglages** : les siens sont dans ceux d'Eyrie (page Fawkes), et « Réglages… » y mène ;
//! - les sessions Claude qui attendent une réponse deviennent sa pastille ;
//! - il reçoit `ouvrir` des autres modules (Owlery, Patronus, Grimoire), en appel Rust : c'est le
//!   contrat local, sans tube.

use eframe::egui::Color32;
use suite_sdk::embed::{Embedded, KeyCommand};
use suite_sdk::instance::NamedMutex;
use suite_sdk::module::ModuleStatus;
use suite_sdk::theme::ThemeId;
use suite_sdk::{
    Badge, BuildCx, DataMode, Module, ModuleFactory, ModuleInfo, Notification, Origin, Request, Tone, Unavailable,
};

use fawkes::app::{App, NewClaude, NewTerm};
use fawkes::model::ThemeId as FawkesTheme;

pub const ID: &str = "fawkes";

pub fn info() -> ModuleInfo {
    ModuleInfo {
        id: ID,
        name: "Fawkes",
        icon: "🔥",
        // Le phénix de son exécutable.
        logo: Some(fawkes::phoenix::render),
        color: Color32::from_rgb(0xf2, 0x6d, 0x3b),
        summary: "Terminaux par groupes, restauration exacte des sessions Claude",
        origin: Origin::Embedded { exe: "fawkes.exe", repo: "../Fawkes" },
    }
}

pub fn factory() -> ModuleFactory {
    ModuleFactory { info: info(), build, fonts: None }
}

/// Repris de `Fawkes/src/main.rs` (`instance_name`).
pub fn instance_name(data_dir: Option<&std::ffi::OsStr>) -> String {
    let base = "Local\\Fawkes-single-instance";
    match data_dir {
        None => base.to_string(),
        Some(d) => format!("{base}:{}", d.to_string_lossy().replace('\\', "/").to_lowercase()),
    }
}

fn commands() -> Vec<KeyCommand> {
    vec![
        KeyCommand::new("terminal", "Nouveau terminal", "Ctrl+Maj+T"),
        KeyCommand::new("rouvrir", "Rouvrir le terminal fermé", "Ctrl+Maj+R"),
        KeyCommand::new("groupe", "Nouveau groupe", "Ctrl+Maj+N"),
        KeyCommand::new("fermer", "Fermer le terminal", "Ctrl+Maj+W"),
        KeyCommand::new("agrandir", "Agrandir le terminal", "Ctrl+Maj+M"),
        KeyCommand::new("attente", "Session Claude suivante en attente", "Ctrl+Maj+A"),
        KeyCommand::new("chercher", "Rechercher dans le terminal", "Ctrl+Maj+F"),
        KeyCommand::new("chercher_tout", "Rechercher dans tous les terminaux", "Ctrl+Maj+G"),
        KeyCommand::new("palette", "Palette de Fawkes", "Ctrl+Maj+P"),
        KeyCommand::new("reglages", "Paramètres de Fawkes", "Ctrl+,"),
    ]
}

fn fawkes_theme(id: ThemeId) -> FawkesTheme {
    match id {
        ThemeId::Graphite => FawkesTheme::Graphite,
        ThemeId::Fluent => FawkesTheme::Fluent,
        ThemeId::Nuit => FawkesTheme::Nuit,
    }
}

fn suite_theme_of(host: &suite_sdk::Host) -> FawkesTheme {
    let p = host.palette();
    let id = suite_sdk::theme::ThemeId::ALL
        .into_iter()
        .find(|t| std::ptr::eq(suite_sdk::theme::palette(*t), p))
        .unwrap_or(ThemeId::Graphite);
    fawkes_theme(id)
}

/// `draw` dans le thème d'Eyrie : le réglage de Fawkes (`theme_of`) et sa palette active (`theme::p()`, que lisent ses
/// widgets) le valent le temps de l'appel, puis sont rendus.
fn in_suite_theme<T>(
    target: &mut T,
    theme_of: fn(&mut T) -> &mut FawkesTheme,
    suite: FawkesTheme,
    draw: impl FnOnce(&mut T),
) {
    let own = std::mem::replace(theme_of(target), suite);
    let active = fawkes::theme::current();
    fawkes::theme::set(suite);
    draw(target);
    *theme_of(target) = own;
    fawkes::theme::set(active);
}

/// Les sessions Claude qui attendent une réponse.
fn status(app: &App, base: &ModuleStatus) -> ModuleStatus {
    let mut s = base.clone();
    let waiting = app.ui.waiting.len() as u32;
    s.badge = (waiting > 0)
        .then(|| Badge::count(waiting, Tone::Warn, format!("{waiting} session(s) Claude en attente de réponse")));
    s.waiting = app
        .st
        .terminals
        .iter()
        .filter(|t| app.ui.waiting.contains(&t.id))
        .map(|t| format!("Claude attend : {}", t.display_title()))
        .collect();
    s
}

/// Ce qu'une demande `ouvrir` donne au shell, contrôlé avant d'atteindre Fawkes : la demande vient d'un autre module,
/// qui peut la tirer de données qu'il ne maîtrise pas (une forge…). `dir` : un dossier existant sur un lecteur local ou
/// mappé (`X:\…`), jamais un chemin réseau (`\\serveur\…`, que le simple fait de le sonder ouvrirait en SMB) ; `resume` :
/// un identifiant de session Claude, qui part tel quel dans la ligne de commande.
pub(crate) fn check_open(dir: &str, resume: Option<&str>) -> Result<(), String> {
    let b = dir.as_bytes();
    let on_a_drive = b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && matches!(b[2], b'\\' | b'/');
    if !on_a_drive || dir.chars().any(char::is_control) || !std::path::Path::new(dir).is_dir() {
        return Err(format!("dossier refusé (dossier local existant attendu) : {dir}"));
    }
    if let Some(id) = resume
        && !(!id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'))
    {
        return Err(format!("identifiant de session refusé : {id}"));
    }
    Ok(())
}

/// `ouvrir{dir, titre, groupe, claude, resume}` : le contrat local de Fawkes, en appel Rust.
fn on_request(app: &mut App, r: &Request, cx: &mut suite_sdk::Cx) -> Result<serde_json::Value, String> {
    if r.demande != "ouvrir" {
        return Err(format!("demande inconnue : {}", r.demande));
    }
    let d = &r.data;
    let s = |k: &str| d.get(k).and_then(|v| v.as_str()).map(str::to_string);
    let dir = s("dir").ok_or("dir manquant")?;
    check_open(&dir, s("resume").as_deref())?;
    let group = s("groupe").and_then(|name| app.st.groups.iter().find(|g| g.name == name).map(|g| g.id));
    let claude = d.get("claude").and_then(|v| v.as_bool()).unwrap_or(false).then(|| NewClaude {
        resume: s("resume"),
        remote_control: false,
        extra_args: String::new(),
    });
    let with_claude = claude.is_some();
    let id = app.create_terminal(NewTerm { cwd: dir, group, title: s("titre"), claude, ..Default::default() });
    // Une consigne (Owlery : « Corriger avec Claude ») : collée dans la session dès que Claude est prêt, sans être
    // envoyée, comme Fawkes le fait pour une sélection envoyée à une nouvelle session.
    if let Some(consigne) = s("consigne").filter(|c| with_claude && !c.trim().is_empty()) {
        app.ui.pending_paste.insert(id, (consigne, std::time::Instant::now()));
    }
    app.select_term(id);
    cx.host.show(ID);
    Ok(serde_json::json!({ "terminal": id }))
}

fn build(bcx: &BuildCx) -> Result<Box<dyn Module>, Unavailable> {
    // `FAWKES_DATA_DIR` est posé par la suite en mode essai (avant tout fil).
    let custom = std::env::var_os("FAWKES_DATA_DIR");
    let guard = NamedMutex::try_acquire(&instance_name(custom.as_deref()))
        .ok_or(Unavailable::AlreadyRunning { exe: "fawkes.exe" })?;
    let mut problems: Vec<String> = match bcx.data {
        DataMode::Shared => fawkes::persist::migrate_old_data_dir().into_iter().collect(),
        DataMode::Trial(_) => Vec::new(),
    };
    let fawkes::persist::Loaded { state, problems: load_problems } = fawkes::persist::load();
    problems.extend(load_problems);
    let mut app = App::new(bcx.cc, state, problems);
    // Intégré : ses réglages sont dans ceux d'Eyrie (« Réglages… » y mène), sans ce qu'Eyrie règle pour toute la
    // suite (thème, barre de titre, téléphone, démarrage de Windows).
    app.set_hosted(true);
    let link = bcx.host.clone();
    app.set_settings_link(Box::new(move || link.open_settings(&format!("module:{ID}"))));
    let panel_host = bcx.host.clone();

    // Pendant l'affichage : le thème de la suite, la barre de titre de Windows (celle d'Eyrie) et la géométrie de la
    // fenêtre où Fawkes est dessiné (il la relève à chaque image pour son propre lancement : sans cela, `fawkes.exe`
    // rouvrirait à la taille de la suite). Tous reprennent ensuite la valeur de Fawkes.
    type Saved = (FawkesTheme, FawkesTheme, bool, fawkes::model::WindowGeom);
    let saved = std::rc::Rc::new(std::cell::RefCell::new(None::<Saved>));
    let (before, after) = (saved.clone(), saved);
    // Fawkes pose ses notifications sur sa propre fenêtre, qu'il ne trouve pas dans la suite : les
    // sessions Claude qui se mettent à attendre vont au centre de notifications de la suite.
    let mut waiting_before: std::collections::HashSet<fawkes::model::TermId> = Default::default();
    let module = Embedded::new(info(), bcx.host.clone(), app)
        .after_logic(move |app, cx| {
            let now = &app.ui.waiting;
            let new: Vec<String> = app
                .st
                .terminals
                .iter()
                .filter(|t| now.contains(&t.id) && !waiting_before.contains(&t.id))
                .map(|t| t.display_title())
                .collect();
            if !new.is_empty() && app.st.settings.notify_windows {
                cx.host.notify(Notification::new(ID, "Claude attend une réponse", new.join(", "), Tone::Warn));
            }
            waiting_before = now.clone();
        })
        .key_commands(commands())
        // Sa page dans les réglages d'Eyrie, aux couleurs d'Eyrie comme son affichage.
        .settings_panel(move |app, ui| {
            in_suite_theme(app, |a| &mut a.st.settings.theme, suite_theme_of(&panel_host), |a| a.settings_ui(ui));
        })
        .on_settings_left(|app| app.settings_left())
        .status(status)
        .on_request(on_request)
        // Le téléphone : groupes et terminaux, un terminal en direct, la saisie (`mobile.rs`).
        .mobile_screens(crate::mobile::screen)
        .mobile_waitings(crate::mobile::waiting)
        .mobile_actions(crate::mobile::action)
        .mobile_terminals(crate::mobile::term_view)
        .mobile_terminal_input(crate::mobile::term_input)
        // Contexte partagé : le groupe lié à ce dépôt devient le groupe affiché.
        .on_event(|app, event, cx| {
            if event.kind != "contexte" || !cx.host.follows_context() || event.source == ID {
                return;
            }
            let dir = event.data.get("dir").and_then(|d| d.as_str());
            if let Some(g) = dir.and_then(|d| app.st.group_for_dir(d)) {
                app.select_group(g);
            }
        })
        .before_ui(move |app, cx| {
            let suite = suite_theme_of(cx.host);
            let s = &mut app.st.settings;
            *before.borrow_mut() = Some((s.theme, suite, s.native_title_bar, app.st.window.clone()));
            s.theme = suite;
            s.native_title_bar = true;
            // Ce que `track_window` va relever : rien ne change, rien n'est marqué à enregistrer.
            let (outer, inner, maximized) =
                cx.host.ctx().input(|i| (i.viewport().outer_rect, i.viewport().inner_rect, i.viewport().maximized));
            let w = &mut app.st.window;
            w.maximized = maximized.unwrap_or(false);
            if !w.maximized {
                if let Some(o) = outer {
                    w.pos = Some([o.min.x, o.min.y]);
                }
                if let Some(i) = inner {
                    w.size = [i.width(), i.height()];
                }
            }
        })
        .after_ui(move |app, _cx| {
            if let Some((own, suite, title_bar, window)) = after.borrow_mut().take() {
                // Inchangé pendant l'image : on rend à Fawkes son propre choix. Changé (menu de
                // Fawkes) : c'est le nouveau choix de Fawkes, gardé.
                if app.st.settings.theme == suite {
                    app.st.settings.theme = own;
                }
                app.st.settings.native_title_bar = title_bar;
                app.st.window = window;
            }
        })
        .keep(guard);
    Ok(Box::new(module))
}

#[cfg(test)]
mod tests {
    use super::FawkesTheme;

    /// Sa page se dessine avec la palette active (`theme::p()`), pas seulement son réglage : les deux valent le thème
    /// d'Eyrie le temps de la page, même si sa case n'a pas encore été dessinée, puis sont rendus.
    #[test]
    fn the_settings_page_is_drawn_in_the_suite_theme() {
        use fawkes::theme;
        let mut setting = FawkesTheme::Fluent;
        theme::set(FawkesTheme::Fluent);
        let mut seen = None;
        super::in_suite_theme(&mut setting, |t| t, FawkesTheme::Graphite, |t| seen = Some((*t, theme::current())));
        assert_eq!(seen, Some((FawkesTheme::Graphite, FawkesTheme::Graphite)), "réglage et palette d'Eyrie");
        assert_eq!((setting, theme::current()), (FawkesTheme::Fluent, FawkesTheme::Fluent), "rendus ensuite");
    }

    /// Une demande `ouvrir` ne fait entrer dans le shell ni chemin réseau, ni dossier inexistant, ni commande cachée
    /// dans l'identifiant de session.
    #[test]
    fn an_open_request_is_checked_before_reaching_the_shell() {
        let here = env!("CARGO_MANIFEST_DIR");
        assert_eq!(super::check_open(here, None), Ok(()));
        assert_eq!(super::check_open(here, Some("7f3c2a9e-1b4d-4e8f-9a6b-0c5d2e1f3a4b")), Ok(()));
        assert!(super::check_open(here, Some("x; Start-Process calc")).is_err());
        assert!(super::check_open(here, Some("$(calc)")).is_err());
        assert!(super::check_open(here, Some("")).is_err());
        for dir in [r"\\hote.invalid\partage", "//hote.invalid/partage", "relatif", r"C:\n'existe\pas", ""] {
            assert!(super::check_open(dir, None).is_err(), "{dir}");
        }
    }

    #[test]
    fn same_lock_as_the_exe() {
        assert_eq!(super::instance_name(None), r"Local\Fawkes-single-instance");
        assert_eq!(
            super::instance_name(Some(std::ffi::OsStr::new(r"C:\Temp\Fawkes-Test"))),
            r"Local\Fawkes-single-instance:c:/temp/fawkes-test"
        );
    }
}
