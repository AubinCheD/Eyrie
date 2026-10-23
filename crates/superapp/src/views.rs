//! Les vues propres à la suite : vue d'ensemble, notifications, ressources, journal.

use eframe::egui::{self, Color32, RichText};
use suite_sdk::Tone;

use crate::app::{Action, Core};
use crate::core::{SlotState, ago};
use crate::i18n::{mark, tr};
use crate::layout::{Pane, SuiteView};

pub fn show(core: &mut Core, ui: &mut egui::Ui, view: SuiteView) {
    match view {
        SuiteView::Overview => overview(core, ui),
        SuiteView::Notifications => notifications(core, ui),
        SuiteView::Resources => resources(core, ui),
        SuiteView::Journal => journal(core, ui),
    }
}

/// Carte centrée : icône, titre, explication.
pub fn card(ui: &mut egui::Ui, icon: &str, title: &str, body: &str, color: Color32) {
    ui.add_space((ui.available_height() * 0.25).min(120.0));
    ui.vertical_centered(|ui| {
        ui.label(RichText::new(icon).size(34.0).color(color));
        ui.add_space(4.0);
        ui.label(RichText::new(title).size(17.0).strong());
        if !body.is_empty() {
            ui.add_space(2.0);
            ui.add(egui::Label::new(RichText::new(body).color(ui.visuals().weak_text_color())).wrap());
        }
        ui.add_space(8.0);
    });
}

fn heading(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text).size(16.0).strong());
    ui.add_space(2.0);
}

fn state_label(core: &Core, s: &SlotState) -> (String, Color32) {
    let p = core.palette();
    match s {
        SlotState::Running(_) => (tr("actif").into(), p.ok),
        SlotState::Disabled => (tr("désactivé").into(), p.text_dim),
        SlotState::Unavailable(_) => (tr("indisponible").into(), p.warn),
        SlotState::Crashed(_) => (tr("arrêté").into(), p.danger),
    }
}

fn overview(core: &mut Core, ui: &mut egui::Ui) {
    let p = core.palette();
    heading(ui, tr("Vue d'ensemble"));
    ui.label(RichText::new(tr("Ce qui vous attend dans tous les modules.")).color(p.text_dim));
    ui.add_space(8.0);
    egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
        let width = ui.available_width();
        let gap = 10.0;
        let cols = ((width + gap) / (250.0 + gap)).floor().max(1.0);
        let card_w = ((width + gap) / cols - gap).floor();
        let mut actions = Vec::new();
        let per_row = cols as usize;
        for row in core.slots.chunks(per_row) {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing = egui::vec2(gap, gap);
                for s in row {
                    let (state, state_color) = state_label(core, &s.state);
                    let status = s.module().map(|m| m.status()).unwrap_or_default();
                    let resp = egui::Frame::new()
                        .fill(p.surface)
                        .stroke(egui::Stroke::new(
                            1.0,
                            if status.badge.is_some() { s.info.color.gamma_multiply(0.6) } else { p.border },
                        ))
                        .corner_radius(egui::CornerRadius::same(p.radius))
                        .inner_margin(egui::Margin::same(10))
                        .show(ui, |ui| {
                            ui.set_width(card_w - 22.0);
                            ui.set_height(118.0);
                            ui.vertical(|ui| {
                                ui.horizontal(|ui| {
                                    ui.label(
                                        RichText::new(s.info.icon)
                                            .size(18.0)
                                            .color(suite_sdk::theme::readable(p, s.info.color)),
                                    );
                                    ui.label(RichText::new(s.info.name).strong().color(p.text));
                                    ui.label(RichText::new(state).color(state_color).small());
                                    if let Some(b) = &status.badge {
                                        let text = b.count.map(|c| c.to_string()).unwrap_or_else(|| "●".into());
                                        ui.label(
                                            RichText::new(format!(" {text} "))
                                                .background_color(b.tone.color(p))
                                                .color(p.on_alert_badge)
                                                .small(),
                                        );
                                    }
                                });
                                ui.add(
                                    egui::Label::new(RichText::new(s.info.summary).color(p.text_dim).small())
                                        .truncate(),
                                );
                                ui.add_space(2.0);
                                if let Some(b) = &status.badge {
                                    ui.label(RichText::new(&b.tip).color(b.tone.color(p)));
                                }
                                for w in status.waiting.iter().take(2) {
                                    ui.add(
                                        egui::Label::new(RichText::new(format!("• {w}")).color(p.text_muted))
                                            .truncate(),
                                    );
                                }
                                if let SlotState::Crashed(m) = &s.state {
                                    ui.add(egui::Label::new(RichText::new(m).color(p.danger).small()).truncate());
                                }
                                if let SlotState::Unavailable(u) = &s.state {
                                    ui.add(
                                        egui::Label::new(
                                            RichText::new(crate::app::unavailable_text(u)).color(p.warn).small(),
                                        )
                                        .truncate(),
                                    );
                                }
                                if status.badge.is_none() && status.waiting.is_empty() && s.is_running() {
                                    let quiet =
                                        status.title.clone().unwrap_or_else(|| tr("Rien en attente.").to_string());
                                    ui.add(egui::Label::new(RichText::new(quiet).color(p.text_dim).small()).truncate());
                                }
                            });
                        })
                        .response
                        .interact(egui::Sense::click())
                        .on_hover_cursor(egui::CursorIcon::PointingHand);
                    if resp.clicked() && !matches!(s.state, SlotState::Disabled) {
                        actions.push(Action::Reveal(Pane::module(s.info.id, suite_sdk::ViewInfo::MAIN)));
                    }
                }
            });
            ui.add_space(gap);
        }
        for a in actions {
            core.act(a);
        }
        ui.add_space(14.0);
        heading(ui, tr("Dernières notifications"));
        if core.notes.items.is_empty() {
            ui.label(RichText::new(tr("Rien pour l'instant.")).color(p.text_dim));
        }
        for (n, read) in core.notes.items.iter().take(6) {
            note_row(core, ui, n, *read);
        }
    });
}

fn note_row(core: &Core, ui: &mut egui::Ui, n: &suite_sdk::Notification, read: bool) {
    let p = core.palette();
    let (icon, color) = match core.slot(&n.module) {
        Some(s) => (s.info.icon, suite_sdk::theme::readable(p, s.info.color)),
        None => ("◆", p.accent),
    };
    ui.horizontal(|ui| {
        ui.label(RichText::new(icon).color(color));
        let title = RichText::new(&n.title).color(if read { p.text_muted } else { p.text });
        ui.label(if read { title } else { title.strong() });
        ui.label(RichText::new(&n.body).color(p.text_dim));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(RichText::new(ago(n.at)).color(p.text_dim).small());
            ui.label(RichText::new("●").color(n.tone.color(p)).small());
        });
    });
}

fn notifications(core: &mut Core, ui: &mut egui::Ui) {
    let p = core.palette();
    ui.horizontal(|ui| {
        heading(ui, tr("Notifications"));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button(tr("Tout effacer")).clicked() {
                core.notes.items.clear();
            }
            if ui.button(tr("Tout marquer comme lu")).clicked() {
                core.notes.mark_all_read();
            }
        });
    });
    ui.label(
        RichText::new(tr("Un seul centre pour tous les modules (Owlery, Patronus et la suite y écrivent)."))
            .color(p.text_dim),
    );
    ui.separator();
    egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
        if core.notes.items.is_empty() {
            card(ui, "🔔", tr("Aucune notification"), "", p.text_dim);
        }
        for (n, read) in core.notes.items.iter() {
            note_row(core, ui, n, *read);
        }
    });
}

fn resources(core: &mut Core, ui: &mut egui::Ui) {
    let p = core.palette();
    heading(ui, tr("Ressources"));
    ui.label(
        RichText::new(tr("Tous les modules partagent un processus : temps passé par chacun, à chaque image."))
            .color(p.text_dim),
    );
    ui.add_space(6.0);
    egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
        egui::Grid::new("ressources").striped(true).num_columns(8).spacing([14.0, 6.0]).show(ui, |ui| {
            for h in [
                mark("Module"),
                mark("État"),
                mark("Logique (ms)"),
                mark("Affichage (ms)"),
                mark("Pic (ms)"),
                mark("Appels"),
                mark("Style rétabli"),
                mark("Commandes contenues"),
            ] {
                ui.label(RichText::new(tr(h)).strong());
            }
            ui.end_row();
            for s in &core.slots {
                let (state, color) = state_label(core, &s.state);
                ui.label(RichText::new(format!("{} {}", s.info.icon, s.info.name)).color(s.info.color));
                ui.label(RichText::new(state).color(color));
                ui.label(format!("{:.2}", s.stats.logic_avg));
                ui.label(format!("{:.2}", s.stats.ui_avg));
                ui.label(format!("{:.1}", s.stats.peak.as_secs_f32() * 1000.0));
                ui.label(s.stats.calls.to_string());
                ui.label(s.stats.style_restored.to_string());
                ui.label(s.stats.commands_countered.to_string());
                ui.end_row();
            }
        });
        ui.add_space(10.0);
        let shown: Vec<&str> = core.shown_prev.iter().map(String::as_str).collect();
        ui.label(
            RichText::new(format!(
                "{} : {}",
                tr("Affichés"),
                if shown.is_empty() { "—".to_string() } else { shown.join(", ") }
            ))
            .color(p.text_dim),
        );
        ui.label(
            RichText::new(tr("La mémoire n'est pas mesurable par module dans un seul processus ; le temps l'est."))
                .color(p.text_dim)
                .small(),
        );
    });
}

fn journal(core: &mut Core, ui: &mut egui::Ui) {
    let p = core.palette();
    ui.horizontal(|ui| {
        heading(ui, tr("Journal de la suite"));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button(tr("Effacer")).clicked() {
                core.journal.lines.clear();
            }
        });
    });
    ui.separator();
    egui::ScrollArea::vertical().auto_shrink(false).stick_to_bottom(true).show(ui, |ui| {
        for l in &core.journal.lines {
            let color = match l.tone {
                Tone::Info => p.text_muted,
                Tone::Ok => p.ok,
                Tone::Warn => p.warn,
                Tone::Danger => p.danger,
            };
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(ago(l.at)).color(p.text_dim).monospace().small());
                ui.label(RichText::new(format!("[{}]", l.module)).color(p.text_dim).monospace().small());
                ui.label(RichText::new(&l.text).color(color));
            });
        }
    });
}
