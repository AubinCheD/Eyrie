//! La palette de commandes de la suite (F1) : aller à un module, ouvrir une vue, exécuter une
//! commande d'un module (pour un module embarqué : lui envoyer son raccourci), changer de
//! disposition, de thème, de langue, ouvrir les réglages.

use eframe::egui::{self, Color32, RichText};
use suite_sdk::i18n::Lang;
use suite_sdk::theme::ThemeChoice;

use crate::app::{Action, DEFAULT_LAYOUTS, Suite};
use crate::i18n::{mark, tr};
use crate::layout::{Pane, Preset, SuiteView};
use crate::settings::Page;

#[derive(Default)]
pub struct PaletteState {
    pub open: bool,
    query: String,
    selected: usize,
    focus_next: bool,
}

impl PaletteState {
    pub fn toggle(&mut self) {
        self.open = !self.open;
        self.query.clear();
        self.selected = 0;
        self.focus_next = self.open;
    }
}

pub struct Item {
    pub icon: String,
    pub color: Color32,
    pub title: String,
    pub detail: String,
    pub action: Action,
}

fn fold(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            'à' | 'â' | 'ä' => 'a',
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'î' | 'ï' => 'i',
            'ô' | 'ö' => 'o',
            'ù' | 'û' | 'ü' => 'u',
            'ç' => 'c',
            c => c,
        })
        .flat_map(char::to_lowercase)
        .collect()
}

/// Les lettres de la requête apparaissent-elles dans l'ordre ? Score plus haut pour des lettres
/// consécutives et un début de mot.
pub fn score(query: &str, text: &str) -> Option<i32> {
    let (q, t) = (fold(query), fold(text));
    if q.is_empty() {
        return Some(0);
    }
    if let Some(pos) = t.find(&q) {
        return Some(1000 - pos as i32);
    }
    let mut score = 0;
    let mut chars = t.char_indices().peekable();
    let mut last: Option<usize> = None;
    for qc in q.chars().filter(|c| !c.is_whitespace()) {
        let found = chars.by_ref().find(|(_, c)| *c == qc)?;
        score += match last {
            Some(l) if found.0 == l + 1 => 5,
            _ => 1,
        };
        last = Some(found.0);
    }
    Some(score)
}

pub fn items(suite: &Suite) -> Vec<Item> {
    let core = &suite.core;
    let p = core.palette();
    let mut v = Vec::new();
    for s in &core.slots {
        if matches!(s.state, crate::core::SlotState::Disabled) {
            continue;
        }
        let views = s.module().map(|m| m.views()).unwrap_or_else(|| vec![suite_sdk::ViewInfo::main(s.info.name)]);
        for view in &views {
            let pane = Pane::module(s.info.id, &view.id);
            let title = if views.len() > 1 {
                format!("{} : {}", s.info.name, view.title)
            } else {
                format!("{} {}", tr("Aller à"), s.info.name)
            };
            v.push(Item {
                icon: s.info.icon.into(),
                color: suite_sdk::theme::readable(p, s.info.color),
                title,
                detail: s.info.summary.into(),
                action: Action::Reveal(pane.clone()),
            });
            v.push(Item {
                icon: "⧉".into(),
                color: suite_sdk::theme::readable(p, s.info.color),
                title: format!("{} {} {}", tr("Ouvrir"), view.title, tr("dans une nouvelle fenêtre")),
                detail: String::new(),
                action: Action::OpenInNewWindow(pane),
            });
        }
        if let Some(m) = s.module() {
            for c in m.commands() {
                v.push(Item {
                    icon: s.info.icon.into(),
                    color: suite_sdk::theme::readable(p, s.info.color),
                    title: format!("{} : {}", s.info.name, c.title),
                    detail: c.shortcut.map(|sc| sc.to_string()).unwrap_or_default(),
                    action: Action::RunCommand { module: s.info.id.into(), command: c.id.clone() },
                });
            }
        }
    }
    for view in SuiteView::ALL {
        v.push(Item {
            icon: view.icon().into(),
            color: p.accent,
            title: tr(view.title()).into(),
            detail: tr("vue de la suite").into(),
            action: Action::Reveal(Pane::Suite { view }),
        });
    }
    for kind in Preset::ALL {
        v.push(Item {
            icon: kind.icon().into(),
            color: p.text_muted,
            title: format!("{} : {}", tr("Disposition"), tr(kind.label())),
            detail: String::new(),
            action: Action::Preset(kind),
        });
    }
    let mut names: Vec<String> = core.st.layouts.keys().cloned().collect();
    for (n, _) in DEFAULT_LAYOUTS {
        if !names.iter().any(|x| x == n) {
            names.push(n.into());
        }
    }
    for name in names {
        v.push(Item {
            icon: "▦".into(),
            color: p.text_muted,
            title: format!("{} « {name} »", tr("Disposition")),
            detail: tr("enregistrée").into(),
            action: Action::LoadLayout(name),
        });
    }
    if let Some(pane) = suite.active_pane() {
        v.push(Item {
            icon: "⛶".into(),
            color: p.text_muted,
            title: tr("Agrandir ou restaurer la case active").into(),
            detail: "Alt+Maj+Entrée".into(),
            action: Action::Concentrate(if core.concentrate.is_some() { None } else { Some(pane.clone()) }),
        });
        v.push(Item {
            icon: "⧉".into(),
            color: p.text_muted,
            title: tr("Détacher la case active").into(),
            detail: "Alt+Maj+D".into(),
            action: Action::Detach(pane),
        });
    }
    if !suite.windows.is_empty() {
        v.push(Item {
            icon: "⇲".into(),
            color: p.text_muted,
            title: tr("Remettre toutes les fenêtres dans la principale").into(),
            detail: String::new(),
            action: Action::AttachAll,
        });
    }
    for t in ThemeChoice::ALL {
        v.push(Item {
            icon: "◐".into(),
            color: p.text_muted,
            title: format!("{} : {}", tr("Thème"), tr(t.label())),
            detail: String::new(),
            action: Action::SetTheme(t),
        });
    }
    for l in Lang::ALL {
        v.push(Item {
            icon: "🌐".into(),
            color: p.text_muted,
            title: format!("{} : {}", tr("Langue"), l.label()),
            detail: String::new(),
            action: Action::SetLang(Some(l)),
        });
    }
    for (page, title) in [
        (Page::General, mark("Réglages de la suite")),
        (Page::Modules, mark("Réglages : modules")),
        (Page::Accounts, mark("Réglages : comptes")),
        (Page::Tailscale, mark("Réglages : Tailscale")),
        (Page::Shortcuts, mark("Réglages : raccourcis")),
        (Page::Layouts, mark("Réglages : dispositions")),
    ] {
        v.push(Item {
            icon: "⚙".into(),
            color: p.text_muted,
            title: tr(title).into(),
            detail: String::new(),
            action: Action::OpenSettings(page),
        });
    }
    v.push(Item {
        icon: "⟳".into(),
        color: p.text_muted,
        title: tr("Relancer la suite").into(),
        detail: String::new(),
        action: Action::Restart,
    });
    v
}

pub fn show(suite: &mut Suite, ctx: &egui::Context) {
    if !suite.shell.palette.open {
        return;
    }
    let p = suite.core.palette();
    let all = items(suite);
    let state = &mut suite.shell.palette;
    let mut ranked: Vec<(i32, usize)> =
        all.iter().enumerate().filter_map(|(i, it)| score(&state.query, &it.title).map(|s| (s, i))).collect();
    ranked.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    ranked.truncate(14);
    let (up, down, enter, escape) = ctx.input_mut(|i| {
        (
            i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp),
            i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown),
            i.consume_key(egui::Modifiers::NONE, egui::Key::Enter),
            i.consume_key(egui::Modifiers::NONE, egui::Key::Escape),
        )
    });
    if down {
        state.selected = (state.selected + 1).min(ranked.len().saturating_sub(1));
    }
    if up {
        state.selected = state.selected.saturating_sub(1);
    }
    state.selected = state.selected.min(ranked.len().saturating_sub(1));
    let mut chosen = enter.then(|| ranked.get(state.selected).map(|(_, i)| *i)).flatten();
    let screen = ctx.content_rect();
    let width = (screen.width() * 0.5).clamp(420.0, 680.0);
    let resp = egui::Area::new(egui::Id::unique("suite-palette"))
        .order(egui::Order::Foreground)
        .fixed_pos(egui::pos2(screen.center().x - width / 2.0, screen.top() + 60.0))
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style())
                .fill(p.window)
                .stroke(egui::Stroke::new(1.0, p.border))
                .inner_margin(egui::Margin::same(8))
                .show(ui, |ui| {
                    ui.set_width(width);
                    let edit = egui::TextEdit::singleline(&mut state.query)
                        .hint_text(tr("Module, case, disposition, commande…"))
                        .desired_width(f32::INFINITY);
                    let r = ui.add(edit);
                    if state.focus_next || !r.has_focus() {
                        r.request_focus();
                        state.focus_next = false;
                    }
                    if r.changed() {
                        state.selected = 0;
                    }
                    ui.add_space(4.0);
                    for (row, (_, i)) in ranked.iter().enumerate() {
                        let it = &all[*i];
                        let selected = row == state.selected;
                        let frame = egui::Frame::new()
                            .fill(if selected { p.selection } else { Color32::TRANSPARENT })
                            .corner_radius(egui::CornerRadius::same(p.radius))
                            .inner_margin(egui::Margin::symmetric(6, 3));
                        let resp = frame
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.horizontal(|ui| {
                                    ui.label(RichText::new(&it.icon).color(it.color));
                                    ui.label(RichText::new(&it.title).color(p.text));
                                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                        ui.label(RichText::new(&it.detail).color(p.text_dim).small());
                                    });
                                });
                            })
                            .response
                            .interact(egui::Sense::click());
                        if resp.clicked() {
                            chosen = Some(*i);
                        }
                        if resp.hovered() {
                            state.selected = row;
                        }
                    }
                    if ranked.is_empty() {
                        ui.label(RichText::new(tr("Rien ne correspond.")).color(p.text_dim));
                    }
                });
        });
    let clicked_outside = ctx.input(|i| {
        i.pointer.any_pressed() && i.pointer.press_origin().is_some_and(|o| !resp.response.rect.contains(o))
    });
    if escape || clicked_outside {
        suite.shell.palette.open = false;
    }
    if let Some(i) = chosen {
        suite.shell.palette.open = false;
        suite.core.act(all[i].action.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::score;

    #[test]
    fn fuzzy_matching() {
        assert!(score("faw", "Aller à Fawkes").is_some());
        assert!(score("reg", "Réglages de la suite").is_some(), "sans accents");
        assert!(score("zzz", "Fawkes").is_none());
        assert!(
            score("dra", "DragonTree").unwrap() > score("dra", "Disposition : grille avec Dragon").unwrap_or(0) - 1000
        );
        assert!(score("dt", "DragonTree").is_some(), "lettres dans l'ordre");
    }
}
