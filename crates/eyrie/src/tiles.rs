//! Les cases : comment `egui_tiles` dessine onglets et contenus, pour une fenêtre donnée.

use eframe::egui::{self, Color32, RichText, Stroke, text::LayoutJob};
use egui_tiles::{Behavior, SimplificationOptions, TabState, TileId, Tiles, UiResponse};
use suite_sdk::Unavailable;

use crate::app::{Action, Core};
use crate::core::{self, CallKind, SlotState};
use crate::i18n::tr;
use crate::layout::Pane;
use crate::sandbox::{Policy, WindowWish};
use crate::views;

/// Côté de l'image d'un module dans son onglet.
const TAB_LOGO: f32 = 15.0;

pub struct SuiteTiles<'a> {
    pub core: &'a mut Core,
    pub frame: &'a mut eframe::Frame,
    /// 0 : fenêtre principale.
    pub window: u64,
    pub wish: WindowWish,
}

impl SuiteTiles<'_> {
    fn badge_of(&self, pane: &Pane) -> Option<(String, Color32)> {
        let p = self.core.palette();
        match pane {
            Pane::Module { module, .. } => {
                let s = self.core.slot(module)?;
                match &s.state {
                    SlotState::Running(m) => {
                        let b = m.status().badge?;
                        Some((b.count.map(|c| c.to_string()).unwrap_or_else(|| "●".into()), b.tone.color(p)))
                    }
                    SlotState::Crashed(_) => Some(("!".into(), p.danger)),
                    SlotState::Unavailable(_) => Some(("—".into(), p.text_dim)),
                    SlotState::Disabled => None,
                }
            }
            Pane::Suite { view: crate::layout::SuiteView::Notifications } => {
                let n = self.core.notes.unread();
                (n > 0).then(|| (n.to_string(), p.accent))
            }
            Pane::Suite { .. } => None,
        }
    }

    fn module_pane(&mut self, ui: &mut egui::Ui, module: &str, view: &str) {
        let rect = ui.max_rect();
        let ctx = ui.ctx().clone();
        if pressed_on(ui, rect) {
            self.core.focus.module = Some(module.to_string());
        }
        self.core.shown.insert(module.to_string());
        if !self.core.drawn.insert((module.to_string(), view.to_string())) {
            // Déjà dessinée ailleurs pendant cette image (garde-fou : un module n'est jamais appelé
            // deux fois pour la même vue).
            ui.label(
                RichText::new(tr("Cette vue est affichée dans une autre case.")).color(self.core.palette().text_dim),
            );
            return;
        }
        let palette = self.core.palette();
        let active = self.core.focus.module.as_deref() == Some(module);
        let Core { slots, host, journal, quit, .. } = &mut *self.core;
        let Some(slot) = slots.iter_mut().find(|s| s.info.id == module) else {
            views::card(ui, "?", &format!("{} « {module} »", tr("Module inconnu")), "", palette.text_dim);
            return;
        };
        match &slot.state {
            SlotState::Running(_) => {
                let mut child = ui.new_child(
                    egui::UiBuilder::new()
                        .id_salt(("module", module, view))
                        .max_rect(rect)
                        .layout(egui::Layout::top_down(egui::Align::Min)),
                );
                child.set_clip_rect(rect.intersect(ui.clip_rect()));
                let policy = Policy { keyboard: active, visible: true, sees_close: quit.sees(module) };
                if let Some(o) =
                    core::call(slot, host, self.frame, &ctx, CallKind::Ui, policy, &self.wish, journal, |m, cx| {
                        m.ui(&mut child, view, cx)
                    })
                {
                    quit.observe(module, &o);
                }
                if active {
                    ui.painter().rect_stroke(
                        rect.shrink(0.5),
                        0.0,
                        Stroke::new(1.0, palette.accent.gamma_multiply(0.7)),
                        egui::StrokeKind::Inside,
                    );
                }
            }
            SlotState::Disabled => {
                let info = slot.info.clone();
                views::module_card(
                    ui,
                    &info,
                    &format!("{} {}", info.name, tr("est désactivé")),
                    tr("Un module désactivé n'est pas construit et ne coûte rien."),
                    palette.text_dim,
                );
                let id = info.id.to_string();
                ui.vertical_centered(|ui| {
                    ui.horizontal(|ui| {
                        if ui.button(tr("Activer maintenant")).clicked() {
                            self.core.act(Action::StartModule(id));
                        }
                        if ui.button(tr("Réglages des modules…")).clicked() {
                            self.core.act(Action::OpenSettings(crate::settings::Page::Modules));
                        }
                    });
                });
            }
            SlotState::Unavailable(u) => {
                let (info, u) = (slot.info.clone(), u.clone());
                views::module_card(
                    ui,
                    &info,
                    &format!("{} {}", info.name, tr("n'est pas disponible")),
                    &crate::app::unavailable_text(&u),
                    palette.warn,
                );
                ui.vertical_centered(|ui| {
                    ui.horizontal(|ui| {
                        if let Unavailable::AlreadyRunning { exe } = u
                            && ui.button(format!("{} {exe}", tr("Fermer"))).on_hover_text(tr("Demande poliment à l'exe autonome de se fermer (il garde la main pour enregistrer).")).clicked()
                        {
                            self.core.act(Action::CloseExe(exe));
                        }
                        if ui.button(tr("Réessayer")).on_hover_text(tr("Une fois l'exe autonome fermé")).clicked() {
                            self.core.act(Action::StartModule(info.id.to_string()));
                        }
                    });
                    ui.label(RichText::new(tr("Lancé seul, un module garde ses propres réglages ; dans Eyrie, il suit ceux d'Eyrie. Un module ne tourne jamais deux fois sur le même dossier d'état.")).small().color(palette.text_dim));
                });
            }
            SlotState::Crashed(msg) => {
                let (info, msg) = (slot.info.clone(), msg.clone());
                views::module_card(ui, &info, &format!("{} {}", info.name, tr("s'est arrêté")), &msg, palette.danger);
                ui.vertical_centered(|ui| {
                    ui.label(RichText::new(tr("Les autres modules continuent.")).color(palette.text_dim));
                    ui.horizontal(|ui| {
                        if ui.button(tr("Relancer le module")).clicked() {
                            self.core.act(Action::StartModule(info.id.to_string()));
                        }
                        if ui.button(tr("Journal")).clicked() {
                            self.core.act(Action::Reveal(Pane::Suite { view: crate::layout::SuiteView::Journal }));
                        }
                    });
                });
            }
        }
    }
}

/// Un clic vient d'être fait dans la case, et pas sur une fenêtre posée par-dessus (fenêtre d'un
/// module, palette, dialogue de la suite) : il donne le clavier au module de la case.
fn pressed_on(ui: &egui::Ui, rect: egui::Rect) -> bool {
    let Some(p) = ui.input(|i| if i.pointer.any_pressed() { i.pointer.press_origin() } else { None }) else {
        return false;
    };
    rect.contains(p) && ui.ctx().layer_id_at(p).is_none_or(|l| l == ui.layer_id())
}

/// Ce qu'un onglet propose au clic droit (et le bouton ⋯ de la barre d'onglets).
pub fn pane_menu(core: &mut Core, ui: &mut egui::Ui, pane: &Pane, window: u64) {
    if window == 0 {
        if ui.button(format!("⧉  {}", tr("Détacher dans une fenêtre"))).clicked() {
            core.act(Action::Detach(pane.clone()));
            ui.close();
        }
    } else if ui.button(format!("⇲  {}", tr("Remettre dans la fenêtre principale"))).clicked() {
        core.act(Action::MoveToMain(pane.clone()));
        ui.close();
    }
    let concentrated = core.concentrate.as_ref() == Some(pane);
    let label = if concentrated { tr("Restaurer la disposition") } else { tr("Agrandir (concentration)") };
    if ui.button(format!("⛶  {label}")).clicked() {
        core.act(Action::Concentrate((!concentrated).then(|| pane.clone())));
        ui.close();
    }
    if let Some(m) = pane.module_id().map(str::to_string) {
        let views = core.slot(&m).and_then(|s| s.module()).map(|m| m.views()).unwrap_or_default();
        if views.len() > 1 {
            ui.separator();
            for v in views {
                let p = Pane::module(&m, &v.id);
                if &p != pane && ui.button(format!("+  {}", v.title)).clicked() {
                    core.act(Action::Reveal(p));
                    ui.close();
                }
            }
        }
        // Les commandes du module sont dans Modules › module et dans la palette : pas ici.
    }
    ui.separator();
    if ui
        .button(format!("✕  {}", tr("Fermer la case")))
        .on_hover_text(tr("Le module continue de tourner ; rouvrez-le depuis la barre d'activité."))
        .clicked()
    {
        core.act(Action::ClosePane(pane.clone()));
        ui.close();
    }
}

impl Behavior<Pane> for SuiteTiles<'_> {
    fn pane_ui(&mut self, ui: &mut egui::Ui, tile_id: TileId, pane: &mut Pane) -> UiResponse {
        let rect = ui.max_rect();
        if pressed_on(ui, rect) {
            self.core.last_tile.insert(self.window, tile_id);
            if matches!(pane, Pane::Suite { .. }) {
                self.core.focus.module = None;
            }
        }
        match pane.clone() {
            Pane::Module { module, view } => self.module_pane(ui, &module, &view),
            Pane::Suite { view } => {
                egui::Frame::new().inner_margin(egui::Margin::same(10)).show(ui, |ui| {
                    ui.set_min_size(ui.available_size());
                    views::show(self.core, ui, view);
                });
            }
        }
        UiResponse::None
    }

    fn tab_title_for_pane(&mut self, pane: &Pane) -> egui::WidgetText {
        let (title, icon, color) = self.core.pane_title(pane);
        let p = self.core.palette();
        let font = egui::FontId::proportional(13.0);
        let mut job = LayoutJob::default();
        // L'image d'un module se peint par-dessus la place laissée devant le titre (`on_tab_button`).
        let logo = self.core.pane_logo(self.core.host.ctx(), pane).is_some();
        if !logo {
            job.append(icon, 0.0, egui::TextFormat { font_id: font.clone(), color, ..Default::default() });
        }
        let active = pane.module_id().is_some_and(|m| self.core.focus.module.as_deref() == Some(m));
        job.append(
            &title,
            if logo { TAB_LOGO + 6.0 } else { 6.0 },
            egui::TextFormat {
                font_id: font.clone(),
                color: if active { p.text } else { p.text_muted },
                ..Default::default()
            },
        );
        if let Some((badge, c)) = self.badge_of(pane) {
            job.append(
                &format!(" {badge} "),
                6.0,
                egui::TextFormat {
                    font_id: egui::FontId::proportional(11.0),
                    color: p.on_alert_badge,
                    background: c,
                    ..Default::default()
                },
            );
        }
        job.into()
    }

    fn is_tab_closable(&self, _tiles: &Tiles<Pane>, _tile_id: TileId) -> bool {
        true
    }

    fn on_tab_button(&mut self, tiles: &mut Tiles<Pane>, tile_id: TileId, response: egui::Response) -> egui::Response {
        let Some(pane) = tiles.get_pane(&tile_id).cloned() else { return response };
        if let Some(t) = self.core.pane_logo(&response.ctx, &pane)
            && !response.dragged()
        {
            // À la marge du titre (`tab_title_spacing` d'egui_tiles), centrée en hauteur.
            let at = egui::pos2(response.rect.left() + 8.0, response.rect.center().y - TAB_LOGO / 2.0);
            let painter = response.ctx.layer_painter(response.layer_id).with_clip_rect(response.rect);
            crate::icon::paint_logo(&painter, &t, egui::Rect::from_min_size(at, egui::vec2(TAB_LOGO, TAB_LOGO)), true);
        }
        if response.clicked() {
            self.core.last_tile.insert(self.window, tile_id);
            self.core.focus.module = pane.module_id().map(str::to_string);
        }
        if response.double_clicked() {
            let concentrated = self.core.concentrate.as_ref() == Some(&pane);
            self.core.act(Action::Concentrate((!concentrated).then(|| pane.clone())));
        }
        let window = self.window;
        let core = &mut *self.core;
        response.context_menu(|ui| pane_menu(core, ui, &pane, window));
        if let Some(Pane::Module { module, .. }) = Some(&pane)
            && let Some(s) = self.core.slot(module)
        {
            let tip = match s.module().map(|m| m.status()) {
                Some(st) => {
                    let mut t = s.info.summary.to_string();
                    if let Some(b) = st.badge {
                        t = format!("{t}\n{}", b.tip);
                    }
                    if let Some(title) = st.title {
                        t = format!("{t}\n« {title} »");
                    }
                    t
                }
                None => s.info.summary.to_string(),
            };
            return response.on_hover_text(tip);
        }
        response
    }

    fn top_bar_right_ui(
        &mut self,
        tiles: &Tiles<Pane>,
        ui: &mut egui::Ui,
        _tile_id: TileId,
        tabs: &egui_tiles::Tabs,
        _scroll: &mut f32,
    ) {
        let Some(active) = tabs.active.and_then(|a| tiles.get_pane(&a)).cloned() else { return };
        let p = self.core.palette();
        let small = |t: &str| RichText::new(t).color(p.text_dim).size(13.0);
        ui.add_space(4.0);
        let window = self.window;
        let core = &mut *self.core;
        ui.menu_button(small("⋯"), |ui| pane_menu(core, ui, &active, window)).response.on_hover_text(tr("Case"));
        let concentrated = self.core.concentrate.as_ref() == Some(&active);
        if ui
            .add(egui::Button::new(small("⛶")).frame(false))
            .on_hover_text(tr("Agrandir ou restaurer (Alt+Maj+Entrée)"))
            .clicked()
        {
            self.core.act(Action::Concentrate((!concentrated).then(|| active.clone())));
        }
        if self.window == 0 {
            if ui
                .add(egui::Button::new(small("⧉")).frame(false))
                .on_hover_text(tr("Détacher dans une fenêtre (Alt+Maj+D)"))
                .clicked()
            {
                self.core.act(Action::Detach(active.clone()));
            }
        } else if ui
            .add(egui::Button::new(small("⇲")).frame(false))
            .on_hover_text(tr("Remettre dans la fenêtre principale"))
            .clicked()
        {
            self.core.act(Action::MoveToMain(active.clone()));
        }
    }

    fn simplification_options(&self) -> SimplificationOptions {
        SimplificationOptions { all_panes_must_have_tabs: true, ..Default::default() }
    }

    fn tab_bar_height(&self, _style: &egui::Style) -> f32 {
        28.0
    }

    fn gap_width(&self, _style: &egui::Style) -> f32 {
        3.0
    }

    fn min_size(&self) -> f32 {
        120.0
    }

    fn tab_bar_color(&self, _visuals: &egui::Visuals) -> Color32 {
        self.core.palette().sidebar
    }

    fn tab_bg_color(
        &self,
        _visuals: &egui::Visuals,
        _tiles: &Tiles<Pane>,
        _tile_id: TileId,
        state: &TabState,
    ) -> Color32 {
        let p = self.core.palette();
        if state.active { p.panel } else { p.sidebar }
    }

    fn tab_outline_stroke(
        &self,
        _visuals: &egui::Visuals,
        _tiles: &Tiles<Pane>,
        _tile_id: TileId,
        state: &TabState,
    ) -> Stroke {
        let p = self.core.palette();
        if state.active { Stroke::new(1.0, p.border) } else { Stroke::NONE }
    }

    fn tab_bar_hline_stroke(&self, _visuals: &egui::Visuals) -> Stroke {
        Stroke::new(1.0, self.core.palette().border)
    }

    fn resize_stroke(&self, _style: &egui::Style, state: egui_tiles::ResizeState) -> Stroke {
        let p = self.core.palette();
        match state {
            egui_tiles::ResizeState::Idle => Stroke::new(1.0, p.border),
            egui_tiles::ResizeState::Hovering => Stroke::new(2.0, p.accent),
            egui_tiles::ResizeState::Dragging => Stroke::new(2.0, p.selection_stroke),
        }
    }
}

/// Une seule case, en grand (mode concentration).
pub fn concentrated(t: &mut SuiteTiles, ui: &mut egui::Ui, pane: &Pane) {
    let p = t.core.palette();
    let (title, icon, color) = t.core.pane_title(pane);
    let logo = t.core.pane_logo(ui.ctx(), pane);
    egui::Panel::top("concentration")
        .exact_size(28.0)
        .frame(egui::Frame::new().fill(p.sidebar).inner_margin(egui::Margin::symmetric(8, 4)))
        .show(ui, |ui| {
            ui.horizontal_centered(|ui| {
                match &logo {
                    Some(l) => ui.add(egui::Image::new((l.id(), egui::vec2(TAB_LOGO, TAB_LOGO)))),
                    None => ui.label(RichText::new(icon).color(color)),
                };
                ui.label(RichText::new(title).color(p.text).strong());
                ui.label(RichText::new(tr("· concentration")).color(p.text_dim));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button(tr("Restaurer la disposition")).clicked() {
                        t.core.act(Action::Concentrate(None));
                    }
                });
            });
        });
    egui::CentralPanel::default().frame(egui::Frame::new().fill(p.panel)).show(ui, |ui| {
        let mut pane = pane.clone();
        let _ = t.pane_ui(ui, TileId::from_u64(u64::MAX), &mut pane);
    });
}
