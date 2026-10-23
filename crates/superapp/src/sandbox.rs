//! Le bac à sable : ce que l'hôte fait autour de **chaque** appel d'un module.
//!
//! Chaque module se croit seul dans sa fenêtre (état des lieux §3). Sans toucher à leur code, l'hôte
//! contient ce qui déborde de leur case :
//! - **style global** (`set_visuals`, `set_theme`, `global_style_mut`, `set_zoom_factor`,
//!   `override_text_color`) : relevé avant l'appel, rétabli après. Les panneaux d'un module héritent
//!   du style du `Ui` de sa case, pas de celui du contexte ;
//! - **clavier** : un module qui n'a pas le clavier ne voit ni touches, ni texte, ni copier-coller.
//!   Il ne peut pas non plus prendre le focus (Fawkes le reprend dès que rien ne l'a) ;
//! - **fenêtre** : il voit la fenêtre « sans focus » s'il n'a pas le clavier, « masquée » s'il n'est
//!   affiché nulle part, et ne voit pas la demande de fermeture de la suite, sauf quand la suite se
//!   ferme vraiment (il peut alors l'annuler pour poser sa question) ;
//! - **commandes de fenêtre** (`Title`, `Minimized`, `Maximized`, `Decorations`, `Visible(false)`,
//!   `Fullscreen`, `Close`) : relevées, rendues au module (titre et pastille), et contrées pour la
//!   fenêtre de la suite ;
//! - **plantage** : une panique est rattrapée ; le module est arrêté, les autres continuent.

use std::any::Any;
use std::cell::RefCell;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui::{self, Event, ViewportCommand};

/// Ce que le module a le droit de voir pendant l'appel.
#[derive(Clone, Copy, Debug)]
pub struct Policy {
    /// Le module a le clavier.
    pub keyboard: bool,
    /// Au moins une vue du module est affichée.
    pub visible: bool,
    /// La suite se ferme vraiment : le module voit la demande de fermeture.
    pub sees_close: bool,
}

/// Ce que l'hôte veut pour la fenêtre où le module est dessiné.
#[derive(Clone, Debug)]
pub struct WindowWish {
    pub title: String,
    pub decorations: bool,
}

/// Ce qui s'est passé pendant l'appel.
#[derive(Default, Debug)]
pub struct Observed {
    /// Commandes de fenêtre émises par le module (déjà contrées s'il le fallait).
    pub commands: Vec<ViewportCommand>,
    /// Le module a demandé à fermer la fenêtre (croix de sa barre de titre, menu Quitter…).
    pub wants_close: bool,
    /// Le module a annulé une fermeture en cours.
    pub cancels_close: bool,
    pub panic: Option<String>,
    pub elapsed: Duration,
    /// Le module a remplacé le style global (rétabli).
    pub touched_style: bool,
}

fn is_keyboard(e: &Event) -> bool {
    matches!(e, Event::Key { .. } | Event::Text(_) | Event::Copy | Event::Cut | Event::Paste(_) | Event::Ime(_))
}

thread_local! {
    /// Message et position de la dernière panique, relevés par le crochet installé par [`install_panic_hook`].
    static LAST_PANIC: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// Garde le message et la position de chaque panique, pour la case « le module s'est arrêté ».
pub fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let msg = match info.payload().downcast_ref::<&str>() {
            Some(s) => s.to_string(),
            None => info.payload().downcast_ref::<String>().cloned().unwrap_or_else(|| "panique".into()),
        };
        let at = info.location().map(|l| format!(" ({}:{})", l.file(), l.line())).unwrap_or_default();
        LAST_PANIC.with(|p| *p.borrow_mut() = Some(format!("{msg}{at}")));
        previous(info);
    }));
}

fn panic_message(payload: Box<dyn Any + Send>) -> String {
    LAST_PANIC.with(|p| p.borrow_mut().take()).unwrap_or_else(|| {
        payload
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| payload.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "panique".into())
    })
}

type StyleSnapshot = (Arc<egui::Style>, Arc<egui::Style>, egui::ThemePreference, f32, bool);

fn style_snapshot(ctx: &egui::Context) -> StyleSnapshot {
    ctx.options(|o| (o.dark_style.clone(), o.light_style.clone(), o.theme_preference, o.zoom_factor, o.zoom_with_keyboard))
}

/// Appelle `f` (un appel de module) dans le bac à sable.
pub fn call(ctx: &egui::Context, policy: Policy, wish: &WindowWish, f: impl FnOnce()) -> Observed {
    let started = Instant::now();
    let style = style_snapshot(ctx);
    let focus_before = ctx.memory(|m| m.focused());

    // Entrée vue par le module.
    let (saved_keyboard, saved_keys_down, saved_info) = ctx.input_mut(|i| {
        let keyboard: Vec<Event> = if policy.keyboard {
            Vec::new()
        } else {
            let (k, rest): (Vec<Event>, Vec<Event>) = std::mem::take(&mut i.events).into_iter().partition(is_keyboard);
            i.events = rest;
            k
        };
        let keys_down = if policy.keyboard { None } else { Some(std::mem::take(&mut i.keys_down)) };
        let id = i.raw.viewport_id;
        let saved = i.raw.viewports.get(&id).map(|v| (v.focused, v.occluded, v.events.clone()));
        if let Some(v) = i.raw.viewports.get_mut(&id) {
            if !policy.keyboard {
                v.focused = Some(false);
            }
            if !policy.visible {
                v.occluded = Some(true);
            }
            if !policy.sees_close {
                v.events.retain(|e| *e != egui::ViewportEvent::Close);
            } else if id != egui::ViewportId::ROOT && !v.events.contains(&egui::ViewportEvent::Close) {
                // Le module est dans une fenêtre détachée : c'est la suite qui se ferme.
                v.events.push(egui::ViewportEvent::Close);
            }
        }
        (keyboard, keys_down, saved)
    });
    let first_command = ctx.viewport(|v| v.commands.len());

    let result = catch_unwind(AssertUnwindSafe(f));

    let mut observed = Observed { elapsed: started.elapsed(), ..Default::default() };
    if let Err(payload) = result {
        observed.panic = Some(panic_message(payload));
    }

    // Entrée rendue aux suivants.
    ctx.input_mut(|i| {
        if !saved_keyboard.is_empty() {
            i.events.extend(saved_keyboard);
        }
        if let Some(k) = saved_keys_down {
            i.keys_down = k;
        }
        let id = i.raw.viewport_id;
        if let (Some(v), Some((focused, occluded, events))) = (i.raw.viewports.get_mut(&id), saved_info) {
            v.focused = focused;
            v.occluded = occluded;
            v.events = events;
        }
    });

    // Focus : un module sans le clavier ne le prend pas. Il peut le perdre (egui le retire à un
    // champ quand on clique ailleurs, pendant l'appel de ce champ) : on le laisse partir.
    if !policy.keyboard {
        let focus_after = ctx.memory(|m| m.focused());
        if let Some(after) = focus_after
            && focus_after != focus_before
        {
            ctx.memory_mut(|m| {
                m.surrender_focus(after);
                if let Some(before) = focus_before {
                    m.request_focus(before);
                }
            });
        }
    }

    // Style global : rétabli.
    let after = style_snapshot(ctx);
    if !Arc::ptr_eq(&after.0, &style.0) || !Arc::ptr_eq(&after.1, &style.1) || after.2 != style.2 || after.3 != style.3 || after.4 != style.4 {
        observed.touched_style = true;
        ctx.options_mut(|o| {
            o.dark_style = style.0.clone();
            o.light_style = style.1.clone();
            o.theme_preference = style.2;
            o.zoom_factor = style.3;
            o.zoom_with_keyboard = style.4;
        });
    }

    // Commandes de fenêtre : relevées et contrées.
    let commands: Vec<ViewportCommand> = ctx.viewport(|v| v.commands.get(first_command..).map(<[_]>::to_vec).unwrap_or_default());
    if !commands.is_empty() {
        let (maximized, fullscreen) = ctx.input(|i| (i.viewport().maximized, i.viewport().fullscreen));
        let mut counter = Vec::new();
        for c in &commands {
            match c {
                ViewportCommand::Title(_) => counter.push(ViewportCommand::Title(wish.title.clone())),
                ViewportCommand::Decorations(_) => counter.push(ViewportCommand::Decorations(wish.decorations)),
                ViewportCommand::Minimized(true) => counter.push(ViewportCommand::Minimized(false)),
                ViewportCommand::Maximized(_) => counter.push(ViewportCommand::Maximized(maximized.unwrap_or(false))),
                ViewportCommand::Fullscreen(_) => counter.push(ViewportCommand::Fullscreen(fullscreen.unwrap_or(false))),
                ViewportCommand::Visible(false) => counter.push(ViewportCommand::Visible(true)),
                ViewportCommand::Close => observed.wants_close = true,
                ViewportCommand::CancelClose => observed.cancels_close = true,
                _ => {}
            }
        }
        // Les commandes s'appliquent dans l'ordre : les nôtres, envoyées après, l'emportent.
        for c in counter {
            ctx.send_viewport_cmd(c);
        }
        observed.commands = commands;
    }
    observed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wish() -> WindowWish {
        WindowWish { title: "SuperApp".into(), decorations: true }
    }

    fn frame(ctx: &egui::Context, input: egui::RawInput, f: impl FnMut(&mut egui::Ui)) {
        let mut out = ctx.run_ui(input, f);
        out.textures_delta.clear();
    }

    fn key_input() -> egui::RawInput {
        let [down, up] = suite_sdk::shortcut::Shortcut::ctrl(egui::Key::P).to_events();
        egui::RawInput { events: vec![down, up, Event::Text("x".into())], ..Default::default() }
    }

    #[test]
    fn a_module_without_keyboard_sees_no_keys_but_the_next_one_does() {
        let ctx = egui::Context::default();
        frame(&ctx, key_input(), |ui| {
            let off = Policy { keyboard: false, visible: true, sees_close: false };
            let on = Policy { keyboard: true, ..off };
            let mut seen = 0;
            call(ui.ctx(), off, &wish(), || seen = ui.ctx().input(|i| i.events.iter().filter(|e| is_keyboard(e)).count()));
            assert_eq!(seen, 0);
            call(ui.ctx(), on, &wish(), || seen = ui.ctx().input(|i| i.events.iter().filter(|e| is_keyboard(e)).count()));
            assert_eq!(seen, 3);
        });
    }

    #[test]
    fn global_style_is_restored() {
        let ctx = egui::Context::default();
        ctx.set_visuals(egui::Visuals::dark());
        frame(&ctx, Default::default(), |ui| {
            let before = ui.ctx().global_style();
            let o = call(ui.ctx(), Policy { keyboard: true, visible: true, sees_close: false }, &wish(), || {
                ui.ctx().set_visuals(egui::Visuals::light());
            });
            assert!(o.touched_style);
            assert_eq!(ui.ctx().global_style().visuals.dark_mode, before.visuals.dark_mode);
        });
    }

    #[test]
    fn window_commands_are_countered() {
        let ctx = egui::Context::default();
        frame(&ctx, Default::default(), |ui| {
            let o = call(ui.ctx(), Policy { keyboard: true, visible: true, sees_close: false }, &wish(), || {
                ui.ctx().send_viewport_cmd(ViewportCommand::Title("Owlery (3)".into()));
                ui.ctx().send_viewport_cmd(ViewportCommand::Close);
            });
            assert!(o.wants_close);
            assert_eq!(o.commands.len(), 2);
            let last = ui.ctx().viewport(|v| v.commands.last().cloned());
            assert_eq!(last, Some(ViewportCommand::Title("SuperApp".into())));
        });
    }

    #[test]
    fn a_panic_is_caught_and_the_input_restored() {
        install_panic_hook();
        let ctx = egui::Context::default();
        frame(&ctx, key_input(), |ui| {
            let o = call(ui.ctx(), Policy { keyboard: false, visible: true, sees_close: false }, &wish(), || {
                panic!("boum");
            });
            assert!(o.panic.as_deref().is_some_and(|m| m.contains("boum")), "{:?}", o.panic);
            assert_eq!(ui.ctx().input(|i| i.events.len()), 3, "les frappes sont rendues aux autres");
        });
    }

    #[test]
    fn close_request_is_hidden_unless_the_suite_closes() {
        let ctx = egui::Context::default();
        let mut input = egui::RawInput::default();
        input.viewports.entry(egui::ViewportId::ROOT).or_default().events.push(egui::ViewportEvent::Close);
        frame(&ctx, input, |ui| {
            let mut seen = true;
            call(ui.ctx(), Policy { keyboard: true, visible: true, sees_close: false }, &wish(), || {
                seen = ui.ctx().input(|i| i.viewport().close_requested())
            });
            assert!(!seen);
            call(ui.ctx(), Policy { keyboard: true, visible: true, sees_close: true }, &wish(), || {
                seen = ui.ctx().input(|i| i.viewport().close_requested())
            });
            assert!(seen);
        });
    }
}
