//! L'adaptateur : Fawkes comme module de la suite.
//!
//! On refait ce que fait `Fawkes/src/main.rs`, sans `run_native` :
//! - même verrou d'instance que `fawkes.exe` ;
//! - **thème** : Fawkes lit sa palette à chaque dessin (`theme::p()`) ; pendant son affichage, son
//!   réglage de thème vaut celui de la suite, puis reprend sa valeur (son état enregistré garde le
//!   thème choisi dans Fawkes seul) ;
//! - sa barre de titre dessinée reste dans sa case : ses boutons réduire, agrandir et fermer sont
//!   contenus par la suite (le bac à sable de l'hôte) ;
//! - les sessions Claude qui attendent une réponse deviennent sa pastille ;
//! - il reçoit `ouvrir` des autres modules (Owlery, Patronus, Grimoire), en appel Rust : c'est le
//!   contrat local, sans tube.

use eframe::egui::Color32;
use suite_sdk::embed::{Embedded, KeyCommand};
use suite_sdk::instance::NamedMutex;
use suite_sdk::module::ModuleStatus;
use suite_sdk::theme::ThemeId;
use suite_sdk::{Badge, BuildCx, DataMode, Module, ModuleFactory, ModuleInfo, Notification, Origin, Request, Tone, Unavailable};

use crate::app::{App, NewClaude, NewTerm};
use crate::model::ThemeId as FawkesTheme;

pub const ID: &str = "fawkes";

pub fn info() -> ModuleInfo {
    ModuleInfo {
        id: ID,
        name: "Fawkes",
        icon: "🔥",
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
    let id = suite_sdk::theme::ThemeId::ALL.into_iter().find(|t| std::ptr::eq(suite_sdk::theme::palette(*t), p)).unwrap_or(ThemeId::Graphite);
    fawkes_theme(id)
}

/// Les sessions Claude qui attendent une réponse.
fn status(app: &App, base: &ModuleStatus) -> ModuleStatus {
    let mut s = base.clone();
    let waiting = app.ui.waiting.len() as u32;
    s.badge = (waiting > 0).then(|| Badge::count(waiting, Tone::Warn, format!("{waiting} session(s) Claude en attente de réponse")));
    s.waiting = app
        .st
        .terminals
        .iter()
        .filter(|t| app.ui.waiting.contains(&t.id))
        .map(|t| format!("Claude attend : {}", t.display_title()))
        .collect();
    s
}

/// `ouvrir{dir, titre, groupe, claude, resume}` : le contrat local de Fawkes, en appel Rust.
fn on_request(app: &mut App, r: &Request, cx: &mut suite_sdk::Cx) -> Result<serde_json::Value, String> {
    if r.demande != "ouvrir" {
        return Err(format!("demande inconnue : {}", r.demande));
    }
    let d = &r.data;
    let s = |k: &str| d.get(k).and_then(|v| v.as_str()).map(str::to_string);
    let dir = s("dir").ok_or("dir manquant")?;
    let group = s("groupe").and_then(|name| app.st.groups.iter().find(|g| g.name == name).map(|g| g.id));
    let claude = d.get("claude").and_then(|v| v.as_bool()).unwrap_or(false).then(|| NewClaude {
        resume: s("resume"),
        remote_control: false,
        extra_args: String::new(),
    });
    let id = app.create_terminal(NewTerm { cwd: dir, group, title: s("titre"), claude, ..Default::default() });
    app.select_term(id);
    cx.host.show(ID);
    Ok(serde_json::json!({ "terminal": id }))
}

fn build(bcx: &BuildCx) -> Result<Box<dyn Module>, Unavailable> {
    // `FAWKES_DATA_DIR` est posé par la suite en mode essai (avant tout fil).
    let custom = std::env::var_os("FAWKES_DATA_DIR");
    let guard = NamedMutex::try_acquire(&instance_name(custom.as_deref())).ok_or(Unavailable::AlreadyRunning { exe: "fawkes.exe" })?;
    let mut problems: Vec<String> = match bcx.data {
        DataMode::Shared => crate::persist::migrate_old_data_dir().into_iter().collect(),
        DataMode::Trial(_) => Vec::new(),
    };
    let crate::persist::Loaded { state, problems: load_problems } = crate::persist::load();
    problems.extend(load_problems);
    let app = App::new(bcx.cc, state, problems);

    // Pendant l'affichage : le thème de la suite, et la géométrie de la fenêtre où Fawkes est
    // dessiné (il la relève à chaque image pour son propre lancement : sans cela, `fawkes.exe`
    // rouvrirait à la taille de la suite). Les deux reprennent ensuite la valeur de Fawkes.
    let saved = std::rc::Rc::new(std::cell::RefCell::new(None::<(FawkesTheme, FawkesTheme, crate::model::WindowGeom)>));
    let (before, after) = (saved.clone(), saved);
    // Fawkes pose ses notifications sur sa propre fenêtre, qu'il ne trouve pas dans la suite : les
    // sessions Claude qui se mettent à attendre vont au centre de notifications de la suite.
    let mut waiting_before: std::collections::HashSet<crate::model::TermId> = Default::default();
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
        .status(status)
        .on_request(on_request)
        .before_ui(move |app, cx| {
            let suite = suite_theme_of(cx.host);
            *before.borrow_mut() = Some((app.st.settings.theme, suite, app.st.window.clone()));
            app.st.settings.theme = suite;
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
            if let Some((own, suite, window)) = after.borrow_mut().take() {
                // Inchangé pendant l'image : on rend à Fawkes son propre choix. Changé (menu de
                // Fawkes) : c'est le nouveau choix de Fawkes, gardé.
                if app.st.settings.theme == suite {
                    app.st.settings.theme = own;
                }
                app.st.window = window;
            }
        })
        .keep(guard);
    Ok(Box::new(module))
}

#[cfg(test)]
mod tests {
    #[test]
    fn same_lock_as_the_exe() {
        assert_eq!(super::instance_name(None), r"Local\Fawkes-single-instance");
        assert_eq!(
            super::instance_name(Some(std::ffi::OsStr::new(r"C:\Temp\Fawkes-Test"))),
            r"Local\Fawkes-single-instance:c:/temp/fawkes-test"
        );
    }
}
