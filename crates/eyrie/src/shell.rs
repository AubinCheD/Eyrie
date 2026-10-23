//! L'enveloppe de la suite : barre de menus, barre d'activité, barre d'état, cases de la fenêtre
//! principale, et ce qui passe par-dessus (palette, réglages, bulles, dialogues).

use std::time::Duration;

use eframe::egui::{self, Color32, RichText, Sense};
use suite_sdk::i18n::Lang;
use suite_sdk::theme::ThemeChoice;

use crate::app::{Action, DEFAULT_LAYOUTS, Suite};
use crate::core::SlotState;
use crate::i18n::tr;
use crate::layout::{Pane, Preset, SuiteView};
use crate::persist::ModuleData;
use crate::sandbox::WindowWish;
use crate::tiles::{self, SuiteTiles};
use crate::{palette, settings};

#[derive(Default)]
pub struct ShellState {
    pub palette: palette::PaletteState,
    pub settings: settings::SettingsState,
    /// Premier lancement : présentation et choix des données.
    pub welcome: bool,
    pub about: bool,
    /// Saisie du nom d'une disposition à enregistrer.
    pub save_layout: Option<String>,
    /// Commandes de module demandées depuis la palette ou un menu : (module, commande).
    pub pending_commands: Vec<(String, String)>,
}

pub fn show(suite: &mut Suite, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
    let p = suite.core.palette();
    egui::Panel::top("suite-menus")
        .exact_size(30.0)
        .frame(egui::Frame::new().fill(p.chrome).inner_margin(egui::Margin::symmetric(6, 3)))
        .show(ui, |ui| menu_bar(suite, ui));
    egui::Panel::bottom("suite-etat")
        .exact_size(24.0)
        .frame(egui::Frame::new().fill(p.chrome).inner_margin(egui::Margin::symmetric(8, 2)))
        .show(ui, |ui| status_bar(suite, ui));
    egui::Panel::left("suite-activite")
        .exact_size(48.0)
        .resizable(false)
        .frame(egui::Frame::new().fill(p.sidebar).inner_margin(egui::Margin::symmetric(4, 6)))
        .show(ui, |ui| activity_bar(suite, ui));
    egui::CentralPanel::default().frame(egui::Frame::new().fill(p.sidebar).inner_margin(egui::Margin::same(3))).show(
        ui,
        |ui| {
            let wish = WindowWish { title: crate::app::TITLE.into(), decorations: true };
            if let Some(pane) = suite.core.concentrate.clone() {
                let mut t = SuiteTiles { core: &mut suite.core, frame, window: 0, wish };
                tiles::concentrated(&mut t, ui, &pane);
            } else if suite.main.is_empty() {
                empty_main(suite, ui);
            } else {
                let mut t = SuiteTiles { core: &mut suite.core, frame, window: 0, wish };
                suite.main.ui(&mut t, ui);
            }
        },
    );
}

fn empty_main(suite: &mut Suite, ui: &mut egui::Ui) {
    let p = suite.core.palette();
    crate::views::card(
        ui,
        "▦",
        tr("Aucune case ouverte"),
        tr("Ouvrez un module depuis la barre d'activité, ou choisissez une disposition."),
        p.text_dim,
    );
    ui.vertical_centered(|ui| {
        ui.horizontal(|ui| {
            for kind in Preset::ALL {
                if ui.button(format!("{} {}", kind.icon(), tr(kind.label()))).clicked() {
                    suite.core.act(Action::Preset(kind));
                }
            }
        });
    });
}

fn menu_bar(suite: &mut Suite, ui: &mut egui::Ui) {
    let p = suite.core.palette();
    egui::MenuBar::new().ui(ui, |ui| {
        ui.label(RichText::new("▦").color(p.accent).size(15.0));
        ui.menu_button(tr("Eyrie"), |ui| {
            if ui.button(tr("Réglages…")).clicked() {
                suite.core.act(Action::OpenSettings(settings::Page::General));
                ui.close();
            }
            if ui.button(format!("{}    F1", tr("Palette de commandes"))).clicked() {
                suite.core.act(Action::TogglePalette);
                ui.close();
            }
            ui.separator();
            if ui.button(tr("Relancer Eyrie")).clicked() {
                suite.core.act(Action::Restart);
                ui.close();
            }
            if ui.button(tr("Quitter")).clicked() {
                suite.core.act(Action::Quit);
                ui.close();
            }
        });
        ui.menu_button(tr("Affichage"), |ui| view_menu(suite, ui));
        ui.menu_button(tr("Modules"), |ui| modules_menu(suite, ui));
        ui.menu_button(tr("Aide"), |ui| {
            if ui.button(tr("Raccourcis…")).clicked() {
                suite.core.act(Action::OpenSettings(settings::Page::Shortcuts));
                ui.close();
            }
            if ui.button(tr("Écrire un module…")).clicked() {
                let doc = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/ecrire-un-module.md");
                ui.ctx().open_url(egui::OpenUrl::new_tab(format!(
                    "file:///{}",
                    doc.display().to_string().replace('\\', "/")
                )));
                ui.close();
            }
            if ui.button(tr("À propos")).clicked() {
                suite.shell.about = true;
                ui.close();
            }
        });

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let unread = suite.core.notes.unread();
            let bell = if unread > 0 { format!("🔔 {unread}") } else { "🔔".to_string() };
            let color = if unread > 0 { p.accent } else { p.text_dim };
            if ui
                .add(egui::Button::new(RichText::new(bell).color(color)).frame(false))
                .on_hover_text(tr("Notifications"))
                .clicked()
            {
                suite.core.act(Action::Reveal(Pane::Suite { view: SuiteView::Notifications }));
            }
            if ui
                .add(
                    egui::Button::new(RichText::new(format!("⌕ {}  F1", tr("Commandes"))).color(p.text_dim))
                        .frame(false),
                )
                .on_hover_text(tr("Palette de commandes : modules, cases, dispositions, réglages"))
                .clicked()
            {
                suite.core.act(Action::TogglePalette);
            }
            context_chip(suite, ui);
        });
    });
}

/// Le contexte partagé (dépôt courant) : les cases qui le suivent s'y accordent.
fn context_chip(suite: &mut Suite, ui: &mut egui::Ui) {
    let p = suite.core.palette();
    let ctx = suite.core.host.context();
    let label = match &ctx {
        Some(c) => format!("📁 {}", c.label),
        None => format!("📁 {}", tr("aucun contexte")),
    };
    ui.menu_button(RichText::new(label).color(if ctx.is_some() { p.text } else { p.text_dim }), |ui| {
        ui.label(RichText::new(tr("Contexte partagé : le dépôt sur lequel vous travaillez.")).color(p.text_dim));
        if let Some(c) = &ctx {
            ui.label(RichText::new(c.dir.display().to_string()).monospace().small());
            ui.label(RichText::new(format!("{} {}", tr("choisi par"), c.source)).color(p.text_dim).small());
        }
        ui.separator();
        if ui.button(tr("Choisir un dossier…")).clicked() {
            if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                let label = dir
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| dir.display().to_string());
                suite.core.host.set_context(Some(suite_sdk::host::SharedContext {
                    dir,
                    label,
                    source: "suite".into(),
                }));
            }
            ui.close();
        }
        if ctx.is_some() && ui.button(tr("Effacer le contexte")).clicked() {
            suite.core.host.set_context(None);
            ui.close();
        }
        ui.separator();
        let mut follow = suite.core.st.follow_context;
        if ui.checkbox(&mut follow, tr("Les cases suivent le contexte")).changed() {
            suite.core.st.follow_context = follow;
        }
    });
}

fn view_menu(suite: &mut Suite, ui: &mut egui::Ui) {
    ui.label(RichText::new(tr("Disposer les cases")).small());
    for kind in Preset::ALL {
        if ui.button(format!("{}  {}", kind.icon(), tr(kind.label()))).clicked() {
            suite.core.act(Action::Preset(kind));
            ui.close();
        }
    }
    ui.separator();
    ui.menu_button(tr("Dispositions enregistrées"), |ui| {
        let mut names: Vec<String> = suite.core.st.layouts.keys().cloned().collect();
        for (n, _) in DEFAULT_LAYOUTS {
            if !names.iter().any(|x| x == n) {
                names.push(n.to_string());
            }
        }
        for name in names {
            if ui.button(&name).clicked() {
                suite.core.act(Action::LoadLayout(name));
                ui.close();
            }
        }
    });
    if ui.button(tr("Enregistrer la disposition…")).clicked() {
        suite.shell.save_layout = Some(String::new());
        suite.core.focus.module = None;
        ui.close();
    }
    if ui.button(tr("Gérer les dispositions…")).clicked() {
        suite.core.act(Action::OpenSettings(settings::Page::Layouts));
        ui.close();
    }
    ui.separator();
    let concentrated = suite.core.concentrate.is_some();
    if ui
        .button(format!(
            "{}    Alt+Maj+Entrée",
            if concentrated { tr("Restaurer la disposition") } else { tr("Agrandir la case active") }
        ))
        .clicked()
    {
        let a = if concentrated { Action::Concentrate(None) } else { Action::Concentrate(suite.active_pane()) };
        suite.core.act(a);
        ui.close();
    }
    if !suite.windows.is_empty() && ui.button(tr("Remettre toutes les fenêtres dans la principale")).clicked() {
        suite.core.act(Action::AttachAll);
        ui.close();
    }
    ui.separator();
    ui.menu_button(tr("Vues d'Eyrie"), |ui| {
        for v in SuiteView::ALL {
            if ui.button(format!("{}  {}", v.icon(), tr(v.title()))).clicked() {
                suite.core.act(Action::Reveal(Pane::Suite { view: v }));
                ui.close();
            }
        }
    });
    ui.menu_button(tr("Thème"), |ui| {
        for t in ThemeChoice::ALL {
            if ui.radio(suite.core.st.theme == t, tr(t.label())).clicked() {
                suite.core.act(Action::SetTheme(t));
                ui.close();
            }
        }
    });
    ui.menu_button(tr("Langue"), |ui| {
        if ui.radio(suite.core.st.lang.is_none(), tr("Celle de Windows")).clicked() {
            suite.core.act(Action::SetLang(None));
            ui.close();
        }
        for l in Lang::ALL {
            if ui.radio(suite.core.st.lang == Some(l), l.label()).clicked() {
                suite.core.act(Action::SetLang(Some(l)));
                ui.close();
            }
        }
    });
}

fn modules_menu(suite: &mut Suite, ui: &mut egui::Ui) {
    let mut actions = Vec::new();
    for s in &suite.core.slots {
        let enabled = !matches!(s.state, SlotState::Disabled);
        let text = RichText::new(format!("{}  {}", s.info.icon, s.info.name)).color(if enabled {
            s.info.color
        } else {
            suite.core.palette().text_dim
        });
        ui.add_enabled_ui(enabled, |ui| {
            ui.menu_button(text, |ui| {
                let views =
                    s.module().map(|m| m.views()).unwrap_or_else(|| vec![suite_sdk::ViewInfo::main(s.info.name)]);
                for v in &views {
                    let pane = Pane::module(s.info.id, &v.id);
                    let label = if views.len() > 1 { v.title.clone() } else { tr("Ouvrir").to_string() };
                    if ui.button(label).clicked() {
                        actions.push(Action::Reveal(pane.clone()));
                        ui.close();
                    }
                    if ui.button(format!("⧉  {}", tr("Dans une nouvelle fenêtre"))).clicked() {
                        actions.push(Action::OpenInNewWindow(pane));
                        ui.close();
                    }
                }
                let commands = s.module().map(|m| m.commands()).unwrap_or_default();
                if !commands.is_empty() {
                    ui.separator();
                    for c in commands {
                        let text = match c.shortcut {
                            Some(sc) => format!("{}    {sc}", c.title),
                            None => c.title.clone(),
                        };
                        if ui.button(text).clicked() {
                            actions.push(Action::RunCommand { module: s.info.id.to_string(), command: c.id.clone() });
                            ui.close();
                        }
                    }
                }
            });
        });
    }
    ui.separator();
    if ui.button(tr("Gérer les modules…")).clicked() {
        actions.push(Action::OpenSettings(settings::Page::Modules));
        ui.close();
    }
    for a in actions {
        suite.core.act(a);
    }
}

/// Une icône de la barre d'activité, avec sa pastille.
fn activity_button(
    ui: &mut egui::Ui,
    icon: &str,
    color: Color32,
    badge: Option<(String, Color32)>,
    shown: bool,
    focused: bool,
    p: &suite_sdk::theme::Palette,
) -> egui::Response {
    let size = egui::vec2(40.0, 38.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let painter = ui.painter_at(rect.expand(2.0));
    if resp.hovered() || focused {
        painter.rect_filled(rect, egui::CornerRadius::same(p.radius), p.surface_hover);
    }
    if shown {
        let bar = egui::Rect::from_min_size(
            egui::pos2(rect.left() - 3.0, rect.top() + 8.0),
            egui::vec2(3.0, rect.height() - 16.0),
        );
        painter.rect_filled(bar, egui::CornerRadius::same(2), if focused { p.accent } else { p.text_dim });
    }
    painter.text(rect.center(), egui::Align2::CENTER_CENTER, icon, egui::FontId::proportional(19.0), color);
    if let Some((text, c)) = badge {
        let center = egui::pos2(rect.right() - 8.0, rect.top() + 8.0);
        let galley = painter.layout_no_wrap(text, egui::FontId::proportional(10.0), p.on_alert_badge);
        let w = (galley.size().x + 6.0).max(14.0);
        let r = egui::Rect::from_center_size(center, egui::vec2(w, 14.0));
        painter.rect_filled(r, egui::CornerRadius::same(7), c);
        painter.galley(r.center() - galley.size() / 2.0, galley, p.on_alert_badge);
    }
    resp
}

fn activity_bar(suite: &mut Suite, ui: &mut egui::Ui) {
    let p = suite.core.palette();
    let mut actions = Vec::new();
    ui.vertical_centered(|ui| {
        ui.spacing_mut().item_spacing.y = 4.0;
        let mut i = 0;
        for s in &suite.core.slots {
            if matches!(s.state, SlotState::Disabled) {
                continue;
            }
            // Alt+n compte les modules présents dans la barre, comme `Action::FocusModule`.
            i += 1;
            let badge = match &s.state {
                SlotState::Running(m) => m
                    .status()
                    .badge
                    .map(|b| (b.count.map(|c| c.to_string()).unwrap_or_else(|| "●".into()), b.tone.color(p))),
                SlotState::Crashed(_) => Some(("!".into(), p.danger)),
                SlotState::Unavailable(_) => Some(("—".into(), p.text_dim)),
                SlotState::Disabled => None,
            };
            let color = if s.is_running() { suite_sdk::theme::readable(p, s.info.color) } else { p.text_dim };
            let shown = suite.core.shown_prev.contains(s.info.id);
            let focused = suite.core.focus.module.as_deref() == Some(s.info.id);
            let resp = activity_button(ui, s.info.icon, color, badge, shown, focused, p);
            let state = match &s.state {
                SlotState::Running(m) => m.status().badge.map(|b| b.tip).unwrap_or_default(),
                SlotState::Unavailable(u) => crate::app::unavailable_text(u),
                SlotState::Crashed(m) => format!("{} : {m}", tr("arrêté")),
                SlotState::Disabled => String::new(),
            };
            let shortcut = if i <= 9 { format!("  (Alt+{i})") } else { String::new() };
            let resp = resp.on_hover_text(
                format!("{}{shortcut}\n{}\n{state}", s.info.name, s.info.summary).trim_end().to_string(),
            );
            if resp.clicked() {
                actions.push(Action::Reveal(Pane::module(s.info.id, suite_sdk::ViewInfo::MAIN)));
            }
            let id = s.info.id;
            let views = s.module().map(|m| m.views()).unwrap_or_default();
            resp.context_menu(|ui| {
                for v in &views {
                    if ui.button(format!("{}  {}", tr("Ouvrir"), v.title)).clicked() {
                        actions.push(Action::Reveal(Pane::module(id, &v.id)));
                        ui.close();
                    }
                }
                if ui.button(format!("⧉  {}", tr("Dans une nouvelle fenêtre"))).clicked() {
                    actions.push(Action::OpenInNewWindow(Pane::module(id, suite_sdk::ViewInfo::MAIN)));
                    ui.close();
                }
                if ui.button(format!("⚙  {}", tr("Réglages du module"))).clicked() {
                    actions.push(Action::OpenSettings(settings::Page::Module(id.to_string())));
                    ui.close();
                }
            });
        }
        ui.with_layout(egui::Layout::bottom_up(egui::Align::Center), |ui| {
            if activity_button(ui, "⚙", p.text_muted, None, false, false, p)
                .on_hover_text(tr("Réglages d'Eyrie"))
                .clicked()
            {
                actions.push(Action::OpenSettings(settings::Page::General));
            }
            let unread = suite.core.notes.unread();
            let badge = (unread > 0).then(|| (unread.to_string(), p.accent));
            let shown = suite.locate(&Pane::Suite { view: SuiteView::Notifications }).is_some();
            if activity_button(ui, "🔔", p.text_muted, badge, shown, false, p)
                .on_hover_text(tr("Notifications"))
                .clicked()
            {
                actions.push(Action::Reveal(Pane::Suite { view: SuiteView::Notifications }));
            }
            let shown = suite.locate(&Pane::Suite { view: SuiteView::Overview }).is_some();
            if activity_button(ui, "⌂", p.text_muted, None, shown, false, p)
                .on_hover_text(tr("Vue d'ensemble (Alt+Maj+O)"))
                .clicked()
            {
                actions.push(Action::Reveal(Pane::Suite { view: SuiteView::Overview }));
            }
        });
    });
    for a in actions {
        suite.core.act(a);
    }
}

fn status_bar(suite: &mut Suite, ui: &mut egui::Ui) {
    let p = suite.core.palette();
    let small = |t: String, c: Color32| RichText::new(t).color(c).size(11.5);
    ui.horizontal_centered(|ui| {
        let keyboard = match &suite.core.focus.module {
            Some(m) => suite.core.slot(m).map(|s| s.info.name.to_string()).unwrap_or_else(|| m.clone()),
            None => tr("Eyrie").to_string(),
        };
        ui.label(small(format!("⌨ {keyboard}"), p.text_muted)).on_hover_text(tr(
            "Le module qui a le clavier : celui de la case cliquée en dernier. Les autres ne voient aucune frappe.",
        ));
        ui.separator();
        // Le mode de cette session (le réglage changé depuis s'applique au prochain lancement).
        let data = match suite.core.launch_data {
            ModuleData::Shared => tr("données des exe"),
            ModuleData::Trial => tr("données d'essai"),
        };
        let color = if suite.core.launch_data == ModuleData::Trial { p.warn } else { p.text_dim };
        ui.label(small(format!("{} : {data}", tr("Modules")), color))
            .on_hover_text(tr("Où les modules embarqués rangent leur état (Réglages > Général)."));
        let running = suite.core.slots.iter().filter(|s| s.is_running()).count();
        ui.label(small(format!("· {running}/{} {}", suite.core.slots.len(), tr("actifs")), p.text_dim));
        if suite.core.restart_needed {
            ui.separator();
            if ui
                .add(egui::Button::new(small(format!("⟳ {}", tr("Relancer pour appliquer")), p.warn)).frame(false))
                .clicked()
            {
                suite.core.act(Action::Restart);
            }
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let (text, color) = suite.core.tailnet.status_line(p);
            if ui
                .add(egui::Button::new(small(text, color)).frame(false))
                .on_hover_text(tr("Tailscale d'Eyrie (synchro mobile)"))
                .clicked()
            {
                suite.core.act(Action::OpenSettings(settings::Page::Tailscale));
            }
            ui.separator();
            let n = suite.core.accounts.list().len();
            if ui.add(egui::Button::new(small(format!("👤 {n} {}", tr("comptes")), p.text_dim)).frame(false)).clicked()
            {
                suite.core.act(Action::OpenSettings(settings::Page::Accounts));
            }
            if !suite.windows.is_empty() {
                ui.separator();
                let n = suite.windows.len();
                ui.label(small(
                    format!("⧉ {n} {}", if n == 1 { tr("fenêtre détachée") } else { tr("fenêtres détachées") }),
                    p.text_dim,
                ));
            }
        });
    });
}

pub fn overlays(suite: &mut Suite, ctx: &egui::Context, _frame: &mut eframe::Frame) {
    palette::show(suite, ctx);
    // Les réglages demandés dans une fenêtre détachée disparue reviennent dans la principale.
    if let Some(id) = suite.shell.settings.window
        && !suite.windows.iter().any(|w| w.id == id)
    {
        suite.shell.settings.window = None;
    }
    if suite.shell.settings.window.is_none() {
        settings::show(
            &mut settings::SettingsCx { core: &mut suite.core, shell: &mut suite.shell, modules: None },
            ctx,
        );
    }
    toasts(suite, ctx);
    close_dialog(suite, ctx);
    save_layout_dialog(suite, ctx);
    if suite.shell.welcome {
        welcome(suite, ctx);
    }
    if suite.shell.about {
        about(suite, ctx);
    }
}

fn toasts(suite: &mut Suite, ctx: &egui::Context) {
    let p = suite.core.palette();
    suite.core.notes.toasts.retain(|(_, at)| at.elapsed() < Duration::from_secs(6));
    if suite.core.notes.toasts.is_empty() {
        return;
    }
    egui::Area::new(egui::Id::unique("suite-bulles"))
        .anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-16.0, -34.0))
        .order(egui::Order::Foreground)
        .interactable(false)
        .show(ctx, |ui| {
            for (n, _) in suite.core.notes.toasts.iter().rev().take(3) {
                let (icon, color) =
                    suite.core.slot(&n.module).map(|s| (s.info.icon, s.info.color)).unwrap_or(("◆", p.accent));
                egui::Frame::popup(ui.style()).fill(p.window).stroke(egui::Stroke::new(1.0, n.tone.color(p))).show(
                    ui,
                    |ui| {
                        ui.set_max_width(340.0);
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(icon).color(color).size(16.0));
                            ui.vertical(|ui| {
                                ui.label(RichText::new(&n.title).strong().color(p.text));
                                if !n.body.is_empty() {
                                    ui.label(RichText::new(&n.body).color(p.text_dim));
                                }
                            });
                        });
                    },
                );
                ui.add_space(6.0);
            }
        });
}

fn close_dialog(suite: &mut Suite, ctx: &egui::Context) {
    if suite.core.quit.busy.is_empty() {
        return;
    }
    let p = suite.core.palette();
    let mut close = None;
    egui::Modal::new(egui::Id::unique("suite-fermeture")).show(ctx, |ui| {
        ui.set_max_width(440.0);
        ui.heading(tr("Fermer Eyrie ?"));
        ui.label(tr("Du travail est en cours :"));
        for (m, why) in &suite.core.quit.busy {
            ui.label(RichText::new(format!("• {m} : {why}")).color(p.warn));
        }
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if ui.button(tr("Fermer quand même")).clicked() {
                close = Some(true);
            }
            if ui.button(tr("Annuler")).clicked() {
                close = Some(false);
            }
        });
    });
    match close {
        Some(true) => {
            suite.core.quit.busy.clear();
            suite.core.quit.forced = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        Some(false) => {
            suite.core.quit.busy.clear();
            suite.core.quit.forced = false;
            suite.core.quit.restart = false;
        }
        None => {}
    }
}

fn save_layout_dialog(suite: &mut Suite, ctx: &egui::Context) {
    let Some(name) = &mut suite.shell.save_layout else { return };
    let mut done = None;
    egui::Modal::new(egui::Id::unique("suite-enregistrer-disposition")).show(ctx, |ui| {
        ui.heading(tr("Enregistrer la disposition"));
        ui.label(tr("Les cases de la fenêtre principale et les fenêtres détachées, sous un nom."));
        let r = ui.text_edit_singleline(name);
        r.request_focus();
        ui.horizontal(|ui| {
            let ok = !name.trim().is_empty();
            if ui.add_enabled(ok, egui::Button::new(tr("Enregistrer"))).clicked()
                || (ok && ui.input(|i| i.key_pressed(egui::Key::Enter)))
            {
                done = Some(Some(name.trim().to_string()));
            }
            if ui.button(tr("Annuler")).clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                done = Some(None);
            }
        });
    });
    if let Some(d) = done {
        suite.shell.save_layout = None;
        if let Some(name) = d {
            suite.core.act(Action::SaveLayout(name));
        }
    }
}

fn welcome(suite: &mut Suite, ctx: &egui::Context) {
    let p = suite.core.palette();
    let mut close = false;
    egui::Modal::new(egui::Id::unique("suite-accueil")).show(ctx, |ui| {
        ui.set_max_width(560.0);
        ui.heading(format!("{} {}", tr("Bienvenue dans"), crate::app::TITLE));
        ui.label(tr("Les modules réunis dans une seule application : posez-les dans des cases, en onglets ou en grille, détachez-les dans leur propre fenêtre pour un deuxième écran. Chaque module garde son exécutable : lancé seul, il utilise ses propres réglages."));
        ui.add_space(8.0);
        ui.label(RichText::new(tr("Données des modules")).strong());
        let mut data = suite.core.st.module_data;
        ui.radio_value(&mut data, ModuleData::Shared, tr("Celles des exécutables (mêmes groupes, tâches et filtres des deux côtés)"));
        ui.radio_value(&mut data, ModuleData::Trial, tr("Un dossier d'essai propre à Eyrie (pour essayer sans rien toucher)"));
        suite.core.st.module_data = data;
        ui.label(RichText::new(tr("Aucun module n'a encore démarré : Eyrie se relance pour les démarrer sur les données choisies.")).color(p.text_dim));
        ui.add_space(8.0);
        ui.label(RichText::new(tr("Comptes")).strong());
        ui.label(RichText::new(tr("Communs à toute la suite : ceux de DragonTree et d'Owlery sont repris d'eux-mêmes (Réglages › Comptes).")).color(p.text_dim));
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            if ui.button(RichText::new(tr("Commencer")).strong()).clicked() {
                close = true;
            }
        });
    });
    if close {
        suite.shell.welcome = false;
        suite.core.st.welcomed = true;
        // Les modules démarrent au lancement suivant, sur les données choisies (le mode essai pose
        // ses variables d'environnement avant le moindre fil).
        suite.core.act(Action::Restart);
    }
}

fn about(suite: &mut Suite, ctx: &egui::Context) {
    let p = suite.core.palette();
    let mut open = true;
    egui::Window::new(format!("{} {}", tr("À propos de"), crate::app::TITLE))
        .id(egui::Id::unique("suite-a-propos"))
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .show(ctx, |ui| {
            ui.label(format!("{} {}", crate::app::TITLE, env!("CARGO_PKG_VERSION")));
            ui.label(
                RichText::new(tr("Premier jet : l'hôte et les modules réunis, sans changer le code des modules."))
                    .color(p.text_dim),
            );
            ui.separator();
            for s in &suite.core.slots {
                let origin = match &s.info.origin {
                    suite_sdk::Origin::Native => tr("natif").to_string(),
                    suite_sdk::Origin::Embedded { exe, repo } => format!("{} {exe} ({repo})", tr("embarqué :")),
                };
                ui.label(RichText::new(format!("{} {} — {origin}", s.info.icon, s.info.name)).color(s.info.color));
            }
        });
    if !open {
        suite.shell.about = false;
    }
}
