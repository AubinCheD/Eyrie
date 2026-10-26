//! Fenêtres détachées : une fenêtre de Windows par viewport egui **immédiat** (les applications
//! embarquées ne sont ni `Send` ni `Sync`), chacune avec son propre arbre de cases. Elles sont
//! dessinées pendant l'image de la fenêtre principale : un réveil de la suite les redessine toutes.

use eframe::egui::{self, RichText};
use egui_tiles::Tree;

use crate::app::{Action, Suite};
use crate::i18n::tr;
use crate::layout::{self, Pane};
use crate::persist::SavedWindow;
use crate::sandbox::WindowWish;
use crate::settings;
use crate::tiles::SuiteTiles;

pub struct Detached {
    pub id: u64,
    pub tree: Tree<Pane>,
    /// Position et taille données à la création (le constructeur de viewport reste constant ;
    /// sinon chaque relevé de position renverrait la fenêtre où elle était).
    initial_pos: Option<egui::Pos2>,
    initial_size: egui::Vec2,
    /// Position et taille actuelles, relevées pour l'enregistrement.
    pos: Option<egui::Pos2>,
    size: egui::Vec2,
    /// Ses fenêtres flottantes suivent sa taille.
    follow: crate::popups::Follow,
}

pub fn viewport_id(id: u64) -> egui::ViewportId {
    egui::ViewportId::from_hash_of(("suite-fenetre", id))
}

impl Detached {
    pub fn new(id: u64, pane: Pane) -> Self {
        let size = egui::vec2(960.0, 680.0);
        Detached {
            id,
            tree: Tree::new_tabs(layout::tree_id(id), vec![pane]),
            initial_pos: None,
            initial_size: size,
            pos: None,
            size,
            follow: Default::default(),
        }
    }

    pub fn from_saved(s: SavedWindow) -> Self {
        // Un écran débranché depuis : la fenêtre revient sur l'écran principal.
        let pos = s.pos.filter(|p| crate::screens::on_some_monitor(*p)).map(|p| egui::pos2(p[0], p[1]));
        let size = egui::vec2(s.size[0], s.size[1]);
        Detached { id: s.id, tree: s.tree, initial_pos: pos, initial_size: size, pos, size, follow: Default::default() }
    }

    pub fn to_saved(&self) -> SavedWindow {
        SavedWindow {
            id: self.id,
            tree: self.tree.clone(),
            pos: self.pos.map(|p| [p.x, p.y]),
            size: [self.size.x, self.size.y],
        }
    }
}

fn title(suite_core: &crate::app::Core, tree: &Tree<Pane>) -> String {
    let names: Vec<String> = layout::panes(tree).iter().map(|(_, p)| suite_core.pane_title(p).0).collect();
    format!("{} — {}", names.join(", "), crate::app::TITLE)
}

pub fn show_all(suite: &mut Suite, ctx: &egui::Context, frame: &mut eframe::Frame) {
    let Suite { core, windows, shell, .. } = suite;
    let mut closed = Vec::new();
    for w in windows.iter_mut() {
        let title = title(core, &w.tree);
        let mut builder = egui::ViewportBuilder::default()
            .with_title(&title)
            .with_inner_size(w.initial_size)
            .with_min_inner_size([360.0, 240.0])
            .with_icon(std::sync::Arc::new(crate::icon::window_icon()));
        if let Some(p) = w.initial_pos {
            builder = builder.with_position(p);
        }
        ctx.show_viewport_immediate(viewport_id(w.id), builder, |ui, _class| {
            let vctx = ui.ctx().clone();
            // Des polices posées par un module pendant l'image de la fenêtre principale s'appliquent
            // au début de celle-ci : le plan de la suite est rétabli avant toute mise en page.
            suite_sdk::fonts::guard(&vctx, &core.fonts);
            w.follow.update(&vctx);
            let (outer, inner, close) =
                vctx.input(|i| (i.viewport().outer_rect, i.viewport().inner_rect, i.viewport().close_requested()));
            if let Some(r) = outer {
                w.pos = Some(r.min);
            }
            if let Some(r) = inner {
                w.size = r.size();
            }
            if close {
                closed.push(w.id);
            }
            let p = core.palette();
            egui::Panel::top("fenetre-barre")
                .exact_size(26.0)
                .frame(egui::Frame::new().fill(p.chrome).inner_margin(egui::Margin::symmetric(8, 3)))
                .show(ui, |ui| {
                    ui.horizontal_centered(|ui| {
                        ui.label(RichText::new(format!("{} {}", tr("Fenêtre"), w.id)).color(p.text_dim).small());
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui
                                .small_button(format!("⇲ {}", tr("Tout remettre dans la fenêtre principale")))
                                .clicked()
                            {
                                core.act(Action::AttachWindow(w.id));
                            }
                            // Les réglages d'Eyrie et ceux des seuls modules de cette fenêtre.
                            if ui.small_button(format!("⚙ {}", tr("Réglages"))).clicked() {
                                let page = settings::window_modules(&w.tree)
                                    .first()
                                    .map_or(settings::Page::General, |m| settings::Page::Module(m.clone()));
                                core.act(Action::OpenSettingsIn(page, Some(w.id)));
                            }
                        });
                    });
                });
            egui::CentralPanel::default()
                .frame(egui::Frame::new().fill(p.sidebar).inner_margin(egui::Margin::same(3)))
                .show(ui, |ui| {
                    let mut tiles = SuiteTiles {
                        core: &mut *core,
                        frame: &mut *frame,
                        window: w.id,
                        wish: WindowWish { title: title.clone(), decorations: true },
                    };
                    w.tree.ui(&mut tiles, ui);
                });
            if shell.settings.open && shell.settings.window == Some(w.id) {
                let modules = Some(settings::window_modules(&w.tree));
                let wish = WindowWish { title: title.clone(), decorations: true };
                settings::show(
                    &mut settings::SettingsCx { core: &mut *core, shell: &mut *shell, modules, wish },
                    &vctx,
                );
            }
        });
        // Dernier onglet fermé : la fenêtre n'a plus lieu d'être.
        if w.tree.is_empty() {
            closed.push(w.id);
        }
    }
    for id in closed {
        core.act(Action::AttachWindow(id));
    }
}
