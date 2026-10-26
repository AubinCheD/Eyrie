//! Les vues de Gringotts : conversations (avec les limites en cours en tête), historique des limites, réglages.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Instant;

use eframe::egui::{self, Align, Color32, Layout, RichText, Sense, Stroke, vec2};
use grimoire::profiles::Profile;
use suite_sdk::theme::Palette;

use crate::analysis::{AccountInfo, Analysis, ConvStats, WindowStat};
use crate::readings::{self, Kind};
use crate::transcripts::Conversation;

/// Ce que les vues lisent.
pub struct Data<'a> {
    pub conversations: &'a [Conversation],
    pub analysis: &'a Analysis,
    pub profiles: &'a [Profile],
    pub scanned: Option<Instant>,
    pub scanning: bool,
}

pub enum Action {
    Rescan,
    /// Installer le relais pour ce profil (indice dans `profiles`).
    Install(usize),
    OpenSettings,
}

pub enum SettingsAction {
    Install(usize),
    Uninstall(usize),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Period {
    Day,
    Week,
    #[default]
    Month,
    Quarter,
    All,
}

impl Period {
    const ALL: [Period; 5] = [Period::Day, Period::Week, Period::Month, Period::Quarter, Period::All];

    fn label(self) -> &'static str {
        match self {
            Period::Day => "24 h",
            Period::Week => "7 jours",
            Period::Month => "30 jours",
            Period::Quarter => "3 mois",
            Period::All => "Tout",
        }
    }

    fn since(self, now: i64) -> i64 {
        let days = match self {
            Period::Day => 1,
            Period::Week => 7,
            Period::Month => 30,
            Period::Quarter => 91,
            Period::All => return i64::MIN,
        };
        now - days * 86_400
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Column {
    #[default]
    Date,
    Repo,
    Title,
    Tokens,
    Cost,
    FiveHour,
    Week,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Span {
    #[default]
    FourWeeks,
    ThreeMonths,
    SixMonths,
    All,
}

impl Span {
    const ALL: [Span; 4] = [Span::FourWeeks, Span::ThreeMonths, Span::SixMonths, Span::All];

    fn label(self) -> &'static str {
        match self {
            Span::FourWeeks => "4 semaines",
            Span::ThreeMonths => "3 mois",
            Span::SixMonths => "6 mois",
            Span::All => "Tout",
        }
    }

    fn since(self, now: i64) -> i64 {
        match self {
            Span::FourWeeks => now - 28 * 86_400,
            Span::ThreeMonths => now - 91 * 86_400,
            Span::SixMonths => now - 182 * 86_400,
            Span::All => i64::MIN,
        }
    }
}

/// Filtres et tri, gardés pendant la session.
#[derive(Default)]
pub struct ViewState {
    pub period: Period,
    pub repo: Option<String>,
    /// Clé de compte ; `Some("")` : sans compte (API, profil sans relais).
    pub account: Option<String>,
    pub model: Option<String>,
    pub text: String,
    pub sort: Column,
    pub ascending: bool,
    pub span: Span,
}

pub fn ago(t: i64, now: i64) -> String {
    let s = (now - t).max(0);
    match s {
        0..60 => "à l'instant".into(),
        60..3600 => format!("il y a {} min", s / 60),
        3600..86_400 => format!("il y a {} h", s / 3600),
        _ => format!("il y a {} j", s / 86_400),
    }
}

/// « 12,34 $ ».
pub fn money(d: f64) -> String {
    if d > 0.0 && d < 0.005 {
        return "< 0,01 $".into();
    }
    // Pas de « -0,00 » (une somme vide de flottants vaut -0).
    let d = if d.abs() < 0.005 { 0.0 } else { d };
    let s = format!("{d:.2}");
    let (int, dec) = s.split_once('.').unwrap_or((&s, "00"));
    let mut grouped = String::new();
    for (i, c) in int.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            grouped.push('\u{202f}');
        }
        grouped.push(c);
    }
    format!("{},{dec} $", grouped.chars().rev().collect::<String>())
}

/// « 12,3 M », « 450 k ».
pub fn tokens(n: u64) -> String {
    match n {
        0..1_000 => n.to_string(),
        1_000..1_000_000 => format!("{} k", n / 1_000),
        _ => format!("{:.1} M", n as f64 / 1e6).replace('.', ","),
    }
}

fn tone_color(p: &Palette, pct: f64) -> Color32 {
    if pct >= 100.0 {
        p.danger
    } else if pct >= 80.0 {
        p.warn
    } else {
        p.ok
    }
}

/// Barre horizontale de `width` points, remplie à `pct` %.
fn bar(ui: &mut egui::Ui, p: &Palette, pct: f64, width: f32, height: f32) {
    let (rect, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
    let r = egui::CornerRadius::same((height / 2.0).min(4.0) as u8);
    ui.painter().rect_filled(rect, r, p.widget);
    let mut filled = rect;
    filled.set_width(rect.width() * (pct.clamp(0.0, 100.0) as f32 / 100.0));
    if filled.width() > 0.5 {
        ui.painter().rect_filled(filled, r, tone_color(p, pct));
    }
}

/// Une carte par compte : les deux fenêtres en cours.
fn live_cards(ui: &mut egui::Ui, p: &Palette, d: &Data, action: &mut Option<Action>) {
    let now = crate::now();
    if d.analysis.accounts.is_empty() {
        egui::Frame::new()
            .fill(p.surface)
            .stroke(Stroke::new(1.0, p.border))
            .corner_radius(egui::CornerRadius::same(p.radius))
            .inner_margin(egui::Margin::same(12))
            .show(ui, |ui| {
                ui.label(RichText::new("Limites des abonnements : aucun relevé pour l'instant").strong().color(p.text));
                ui.label(
                    RichText::new(
                        "Claude Code ne donne ces taux qu'à sa ligne d'état. Installez le relais de Gringotts pour un \
                         profil : il relève les limites à chaque mise à jour et montre, dans chaque terminal, la part \
                         de la conversation.",
                    )
                    .color(p.text_dim),
                );
                ui.horizontal_wrapped(|ui| {
                    for (i, pr) in d.profiles.iter().enumerate() {
                        if ui.button(format!("Installer pour « {} »", pr.name)).clicked() {
                            *action = Some(Action::Install(i));
                        }
                    }
                    if ui.button("Réglages…").clicked() {
                        *action = Some(Action::OpenSettings);
                    }
                });
            });
        return;
    }
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(10.0, 10.0);
        for a in &d.analysis.accounts {
            account_card(ui, p, a, now);
        }
    });
}

fn account_card(ui: &mut egui::Ui, p: &Palette, a: &AccountInfo, now: i64) {
    egui::Frame::new()
        .fill(p.surface)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(egui::CornerRadius::same(p.radius))
        .inner_margin(egui::Margin::same(12))
        .show(ui, |ui| {
            // Dans une rangée de cartes : le contenu de la carte, lui, va de haut en bas.
            ui.vertical(|ui| {
                ui.set_width(330.0);
                ui.label(RichText::new(&a.label).strong().color(p.text));
                for kind in Kind::ALL {
                    let Some(w) = a.last.window(kind) else { continue };
                    let pct = crate::current(w, now);
                    ui.horizontal(|ui| {
                        ui.add_sized([62.0, 18.0], egui::Label::new(RichText::new(kind.label()).color(p.text_muted)));
                        bar(ui, p, pct, 180.0, 10.0);
                        ui.label(RichText::new(readings::pct_text(pct)).strong().color(tone_color(p, pct)));
                    });
                    let fmt = if kind == Kind::FiveHour { "%H:%M" } else { "%d/%m à %H:%M" };
                    let reset = if w.resets_at <= now {
                        "remise à zéro depuis le relevé".to_string()
                    } else {
                        format!("remise à zéro {} ({})", readings::local(w.resets_at, fmt), until(w.resets_at, now))
                    };
                    ui.label(RichText::new(reset).small().color(p.text_dim));
                }
                ui.label(RichText::new(format!("relevé {}", ago(a.last.t, now))).small().color(p.text_dim));
            });
        });
}

/// « dans 1 h 05 », « dans 3 j 2 h ».
fn until(t: i64, now: i64) -> String {
    let s = (t - now).max(0);
    let (d, h, m) = (s / 86_400, (s % 86_400) / 3600, (s % 3600) / 60);
    if d > 0 {
        format!("dans {d} j {h} h")
    } else if h > 0 {
        format!("dans {h} h {m:02}")
    } else {
        format!("dans {m} min")
    }
}

/// Conversations retenues par les filtres, avec leurs chiffres.
fn filtered<'a>(d: &'a Data, v: &ViewState, now: i64) -> Vec<(&'a Conversation, &'a ConvStats)> {
    let since = v.period.since(now);
    let text = v.text.trim().to_lowercase();
    let mut rows: Vec<(&Conversation, &ConvStats)> = d
        .conversations
        .iter()
        .zip(&d.analysis.conversations)
        .filter(|(c, _)| c.ended >= since)
        .filter(|(c, _)| v.repo.as_ref().is_none_or(|r| &c.repo() == r))
        .filter(|(_, s)| match &v.account {
            None => true,
            Some(a) if a.is_empty() => s.account.is_none(),
            Some(a) => s.account.as_ref() == Some(a),
        })
        .filter(|(c, _)| v.model.as_ref().is_none_or(|m| c.models().contains(m)))
        .filter(|(c, _)| {
            text.is_empty() || c.title.to_lowercase().contains(&text) || c.cwd.to_lowercase().contains(&text)
        })
        .collect();
    let key = |(c, s): &(&Conversation, &ConvStats)| -> f64 {
        match v.sort {
            Column::Date => c.ended as f64,
            Column::Tokens => s.tokens as f64,
            Column::Cost => s.cost,
            Column::FiveHour => s.share[0],
            Column::Week => s.share[1],
            Column::Repo | Column::Title => 0.0,
        }
    };
    match v.sort {
        Column::Repo => rows.sort_by_key(|(c, _)| c.repo().to_lowercase()),
        Column::Title => rows.sort_by_key(|(c, _)| c.title.to_lowercase()),
        _ => rows.sort_by(|a, b| key(a).total_cmp(&key(b))),
    }
    if !v.ascending {
        rows.reverse();
    }
    rows
}

pub fn conversations(ui: &mut egui::Ui, p: &Palette, d: &Data, v: &mut ViewState) -> Option<Action> {
    let mut action = None;
    let now = crate::now();
    ui.horizontal(|ui| {
        ui.label(RichText::new("Consommation de Claude").size(18.0).strong().color(p.text));
        let status = match (d.scanning, d.scanned) {
            (true, _) => "lecture des conversations…".to_string(),
            (false, Some(t)) => format!("{} conversations lues, {}", d.conversations.len(), ago_instant(t)),
            (false, None) => String::new(),
        };
        ui.label(RichText::new(status).color(p.text_dim));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if ui.button("⟳ Relire").on_hover_text("Relire les transcriptions qui ont changé").clicked() {
                action = Some(Action::Rescan);
            }
        });
    });
    ui.add_space(6.0);
    live_cards(ui, p, d, &mut action);
    ui.add_space(8.0);

    // Filtres.
    let repos: Vec<String> = {
        let mut by: BTreeMap<String, f64> = BTreeMap::new();
        for (c, s) in d.conversations.iter().zip(&d.analysis.conversations) {
            *by.entry(c.repo()).or_default() += s.cost;
        }
        let mut list: Vec<(String, f64)> = by.into_iter().collect();
        list.sort_by(|a, b| b.1.total_cmp(&a.1));
        list.into_iter().map(|(r, _)| r).collect()
    };
    let models: Vec<String> = {
        let mut set = std::collections::BTreeSet::new();
        for c in d.conversations {
            set.extend(c.models());
        }
        set.into_iter().collect()
    };
    ui.horizontal_wrapped(|ui| {
        for period in Period::ALL {
            if ui.selectable_label(v.period == period, period.label()).clicked() {
                v.period = period;
            }
        }
        ui.separator();
        let repo = egui::ComboBox::from_id_salt("gringotts-depot")
            .selected_text(v.repo.clone().unwrap_or_else(|| "Tous les dépôts".into()))
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut v.repo, None, "Tous les dépôts");
                for r in &repos {
                    ui.selectable_value(&mut v.repo, Some(r.clone()), r.as_str());
                }
            });
        repo.response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::ComboBox, true, "Dépôt"));
        let account_label = |key: &Option<String>| match key {
            None => "Tous les comptes".to_string(),
            Some(k) if k.is_empty() => "Sans compte (API)".to_string(),
            Some(k) => d.analysis.accounts.iter().find(|a| &a.key == k).map_or_else(|| k.clone(), |a| a.label.clone()),
        };
        let account = egui::ComboBox::from_id_salt("gringotts-compte")
            .selected_text(account_label(&v.account))
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut v.account, None, "Tous les comptes");
                for a in &d.analysis.accounts {
                    ui.selectable_value(&mut v.account, Some(a.key.clone()), a.label.as_str());
                }
                ui.selectable_value(&mut v.account, Some(String::new()), "Sans compte (API)");
            });
        account.response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::ComboBox, true, "Compte"));
        let model = egui::ComboBox::from_id_salt("gringotts-modele")
            .selected_text(v.model.clone().unwrap_or_else(|| "Tous les modèles".into()))
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut v.model, None, "Tous les modèles");
                for m in &models {
                    ui.selectable_value(&mut v.model, Some(m.clone()), m.as_str());
                }
            });
        model.response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::ComboBox, true, "Modèle"));
        ui.add(egui::TextEdit::singleline(&mut v.text).hint_text("Chercher (titre, dossier)").desired_width(180.0));
    });

    let rows = filtered(d, v, now);
    let total_cost: f64 = rows.iter().map(|(_, s)| s.cost).sum();
    let total_tokens: u64 = rows.iter().map(|(_, s)| s.tokens).sum();
    let overflow: f64 = rows.iter().map(|(_, s)| s.overflow).sum();
    let five: f64 = rows.iter().map(|(_, s)| s.share[0]).sum();
    let week: f64 = rows.iter().map(|(_, s)| s.share[1]).sum();
    ui.add_space(4.0);
    ui.horizontal_wrapped(|ui| {
        let chip = |ui: &mut egui::Ui, value: String, label: &str, color: Color32| {
            egui::Frame::new()
                .fill(p.surface)
                .corner_radius(egui::CornerRadius::same(p.radius))
                .inner_margin(egui::Margin::symmetric(10, 6))
                .show(ui, |ui| {
                    ui.vertical(|ui| {
                        ui.label(RichText::new(value).size(16.0).strong().color(color));
                        ui.label(RichText::new(label).small().color(p.text_dim));
                    });
                });
        };
        chip(ui, rows.len().to_string(), "conversations", p.text);
        chip(ui, tokens(total_tokens), "jetons", p.text);
        chip(ui, money(total_cost), "équivalent API", p.accent);
        // Parts cumulées : en fenêtres entières (100 % = une fenêtre de 5 h, ou une semaine, toute entière).
        if five > 0.0 || week > 0.0 {
            let windows = |x: f64| format!("{:.1}", x / 100.0).replace('.', ",");
            chip(ui, windows(five), "fenêtres de 5 h (équivalent)", p.text);
            chip(ui, windows(week), "semaines (équivalent)", p.text);
        }
        if overflow > 0.0 {
            chip(ui, money(overflow), "limite atteinte (dépassement)", p.danger);
        }
    });
    ui.add_space(6.0);

    let wide = ui.available_width() > 1100.0;
    if wide {
        let repo_w = 280.0;
        ui.horizontal_top(|ui| {
            let table_w = ui.available_width() - repo_w - 12.0;
            ui.allocate_ui(vec2(table_w, ui.available_height()), |ui| table(ui, p, &rows, v, now));
            ui.allocate_ui(vec2(repo_w, ui.available_height()), |ui| by_repo(ui, p, &rows, v));
        });
    } else {
        table(ui, p, &rows, v, now);
    }
    action
}

fn ago_instant(t: Instant) -> String {
    let s = t.elapsed().as_secs() as i64;
    ago(0, s)
}

/// Le coût par dépôt, en barres ; un clic filtre sur le dépôt.
fn by_repo(ui: &mut egui::Ui, p: &Palette, rows: &[(&Conversation, &ConvStats)], v: &mut ViewState) {
    let mut by: BTreeMap<String, f64> = BTreeMap::new();
    for (c, s) in rows {
        *by.entry(c.repo()).or_default() += s.cost;
    }
    let mut list: Vec<(String, f64)> = by.into_iter().collect();
    list.sort_by(|a, b| b.1.total_cmp(&a.1));
    let max = list.first().map_or(0.0, |x| x.1);
    ui.label(RichText::new("Par dépôt").strong().color(p.text));
    for (repo, cost) in list.iter().take(14) {
        let resp = ui
            .horizontal(|ui| {
                ui.add_sized([110.0, 18.0], egui::Label::new(RichText::new(repo).color(p.text_muted)).truncate());
                let w = if max > 0.0 { (cost / max) as f32 * 90.0 } else { 0.0 };
                let (rect, _) = ui.allocate_exact_size(vec2(90.0, 10.0), Sense::hover());
                let mut f = rect;
                f.set_width(w.max(1.0));
                ui.painter().rect_filled(f, 3.0, p.accent);
                ui.label(RichText::new(money(*cost)).small().color(p.text_dim));
            })
            .response
            .interact(Sense::click())
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text("Ne montrer que ce dépôt");
        let name = format!("Dépôt {repo}");
        resp.widget_info(move || egui::WidgetInfo::labeled(egui::Role::Button, true, name.clone()));
        if resp.clicked() {
            v.repo = Some(repo.clone());
        }
    }
}

const ROW: f32 = 22.0;

/// Les colonnes de droite (chiffres) s'alignent à droite.
const RIGHT: [bool; 8] = [false, false, false, false, true, true, true, true];

/// Une case du tableau, de largeur exacte : ce qui dépasse est coupé (« … »).
fn cell<R>(ui: &mut egui::Ui, width: f32, right: bool, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let (_, rect) = ui.allocate_space(vec2(width, ROW));
    let layout = if right { Layout::right_to_left(Align::Center) } else { Layout::left_to_right(Align::Center) };
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect).layout(layout));
    child.set_clip_rect(rect.intersect(ui.clip_rect()));
    add(&mut child)
}

/// Le tableau des conversations ; un clic sur un titre de colonne trie.
fn table(ui: &mut egui::Ui, p: &Palette, rows: &[(&Conversation, &ConvStats)], v: &mut ViewState, now: i64) {
    let widths = |avail: f32| {
        let fixed = [96.0, 110.0, 0.0, 110.0, 64.0, 76.0, 64.0, 70.0];
        let flex = (avail - fixed.iter().sum::<f32>() - 8.0 * 6.0).max(140.0);
        let mut w = fixed;
        w[2] = flex;
        w
    };
    let w = widths(ui.available_width());
    ui.horizontal(|ui| {
        for (i, (label, col)) in [
            ("Activité", Some(Column::Date)),
            ("Dépôt", Some(Column::Repo)),
            ("Conversation", Some(Column::Title)),
            ("Modèles", None),
            ("Jetons", Some(Column::Tokens)),
            ("Coût", Some(Column::Cost)),
            ("5 h", Some(Column::FiveHour)),
            ("Semaine", Some(Column::Week)),
        ]
        .into_iter()
        .enumerate()
        {
            let arrow = match col {
                Some(c) if c == v.sort => {
                    if v.ascending {
                        " ▲"
                    } else {
                        " ▼"
                    }
                }
                _ => "",
            };
            let text = RichText::new(format!("{label}{arrow}")).strong().color(p.text_muted);
            let resp = cell(ui, w[i], RIGHT[i], |ui| ui.add(egui::Button::new(text).frame(false)));
            if let Some(c) = col
                && resp.clicked()
            {
                if v.sort == c {
                    v.ascending = !v.ascending;
                } else {
                    v.sort = c;
                    v.ascending = matches!(c, Column::Repo | Column::Title);
                }
            }
        }
    });
    ui.separator();
    if rows.is_empty() {
        ui.label(RichText::new("Aucune conversation pour ces filtres.").color(p.text_dim));
        return;
    }
    egui::ScrollArea::vertical().id_salt("gringotts-conversations").auto_shrink(false).show_rows(
        ui,
        ROW,
        rows.len(),
        |ui, range| {
            for (n, (c, s)) in rows[range.clone()].iter().enumerate() {
                // Le fond d'une ligne sur deux, sous le texte (sa taille n'est connue qu'après).
                let stripe = ui.painter().add(egui::Shape::Noop);
                let row = ui.horizontal(|ui| {
                    let text_cell = |ui: &mut egui::Ui, i: usize, text: RichText| {
                        cell(ui, w[i], RIGHT[i], |ui| ui.add(egui::Label::new(text).truncate()));
                    };
                    let dim = if c.gone { p.text_dim } else { p.text };
                    text_cell(ui, 0, RichText::new(readings::local(c.ended, "%d/%m %H:%M")).color(p.text_dim));
                    text_cell(ui, 1, RichText::new(c.repo()).color(p.text_muted));
                    let title = if c.gone { format!("{} (transcription effacée)", c.title) } else { c.title.clone() };
                    text_cell(ui, 2, RichText::new(title).color(dim));
                    text_cell(ui, 3, RichText::new(c.models().join(", ")).small().color(p.text_dim));
                    text_cell(ui, 4, RichText::new(tokens(s.tokens)).color(p.text_muted));
                    let cost_color = if s.overflow > 0.0 { p.danger } else { p.text };
                    text_cell(ui, 5, RichText::new(money(s.cost)).color(cost_color));
                    for (k, i) in [(0usize, 6usize), (1, 7)] {
                        let text = match (&s.account, s.share[k]) {
                            (None, _) => "—".to_string(),
                            (Some(_), x) if s.estimated => format!("≈ {}", readings::pct_text(x)),
                            (Some(_), x) => readings::pct_text(x),
                        };
                        text_cell(ui, i, RichText::new(text).color(p.text_muted));
                    }
                });
                if (range.start + n) % 2 == 1 {
                    let fill = p.surface.gamma_multiply(0.35);
                    ui.painter().set(stripe, egui::Shape::rect_filled(row.response.rect, 0.0, fill));
                }
                row.response.on_hover_ui(|ui| details(ui, p, c, s, now));
            }
        },
    );
}

fn details(ui: &mut egui::Ui, p: &Palette, c: &Conversation, s: &ConvStats, now: i64) {
    ui.set_max_width(420.0);
    ui.label(RichText::new(&c.title).strong());
    ui.label(RichText::new(&c.cwd).small().color(p.text_dim));
    if let Some(b) = &c.branch {
        ui.label(RichText::new(format!("branche {b}")).small().color(p.text_dim));
    }
    ui.label(format!(
        "du {} au {} ({}) · {} demande(s) · {} sous-agent(s)",
        readings::local(c.started, "%d/%m %H:%M"),
        readings::local(c.ended, "%d/%m %H:%M"),
        ago(c.ended, now),
        c.prompts,
        c.subagents
    ));
    let (mut input, mut output, mut write, mut read) = (0u64, 0u64, 0u64, 0u64);
    for r in &c.replies {
        input += r.input;
        output += r.output;
        write += r.write_5m + r.write_1h;
        read += r.read;
    }
    ui.label(format!(
        "jetons : {} entrée · {} sortie (réflexion comprise) · {} écriture de cache · {} lecture de cache",
        tokens(input),
        tokens(output),
        tokens(write),
        tokens(read)
    ));
    ui.label(format!("{} réponse(s) · {} équivalent API", c.replies.len(), money(s.cost)));
    match &s.account {
        None => {
            ui.label(RichText::new("Sans compte relevé : clé d'API, ou profil sans relais.").color(p.text_dim));
        }
        Some(_) => {
            ui.label(format!(
                "Part des limites : {} de fenêtres de 5 h, {} de semaines{}",
                readings::pct_text(s.share[0]),
                readings::pct_text(s.share[1]),
                if s.estimated { " (en partie estimée : hors relevés)" } else { "" }
            ));
        }
    }
    if s.overflow > 0.0 {
        ui.label(RichText::new(format!("{} dépensés limite atteinte", money(s.overflow))).color(p.danger));
    }
}

pub fn limits(ui: &mut egui::Ui, p: &Palette, d: &Data, v: &mut ViewState) -> Option<Action> {
    let mut action = None;
    let now = crate::now();
    ui.horizontal(|ui| {
        ui.label(RichText::new("Limites des abonnements").size(18.0).strong().color(p.text));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            for span in Span::ALL.iter().rev() {
                if ui.selectable_label(v.span == *span, span.label()).clicked() {
                    v.span = *span;
                }
            }
        });
    });
    ui.add_space(6.0);
    live_cards(ui, p, d, &mut action);
    let since = v.span.since(now);
    egui::ScrollArea::vertical().id_salt("gringotts-limites").auto_shrink(false).show(ui, |ui| {
        for a in &d.analysis.accounts {
            ui.add_space(12.0);
            account_history(ui, p, d, a, since);
        }
        ui.add_space(12.0);
        egui::CollapsingHeader::new("Comment c'est mesuré").show(ui, |ui| {
            ui.label(RichText::new(
                "Les taux viennent de Claude Code lui-même : sa ligne d'état reçoit, pour un abonnement, le pourcentage \
                 utilisé de la fenêtre de 5 h et de la semaine, avec leur heure de remise à zéro. Le relais de Gringotts \
                 note chaque relevé qui change quelque chose : l'heure des 100 % est donc celle du premier relevé à \
                 100 %. Le dépassement est le coût, en équivalent API, des réponses faites ensuite jusqu'à la remise à \
                 zéro. La part d'une conversation : entre deux relevés, la hausse est partagée entre les réponses de \
                 l'intervalle au prorata de leur coût (modèle, effort et cache compris) ; hors relevés, elle est \
                 estimée (≈) avec le rapport moyen du compte. Il n'y a pas d'historique avant l'installation du relais.",
            ).color(p.text_dim));
        });
    });
    action
}

fn account_history(ui: &mut egui::Ui, p: &Palette, d: &Data, a: &AccountInfo, since: i64) {
    let mine = |kind: Kind| -> Vec<&WindowStat> {
        d.analysis.windows.iter().filter(|w| w.account == a.key && w.kind == kind && w.resets_at >= since).collect()
    };
    let weeks = mine(Kind::SevenDay);
    let fives = mine(Kind::FiveHour);
    let overflow: f64 = d
        .conversations
        .iter()
        .zip(&d.analysis.conversations)
        .filter(|(c, s)| s.account.as_deref() == Some(a.key.as_str()) && c.ended >= since)
        .map(|(_, s)| s.overflow)
        .sum();
    ui.label(RichText::new(&a.label).size(15.0).strong().color(p.text));
    ui.horizontal_wrapped(|ui| {
        let reached_w = weeks.iter().filter(|w| w.reached_at.is_some()).count();
        let reached_f = fives.iter().filter(|w| w.reached_at.is_some()).count();
        let pill = |ui: &mut egui::Ui, text: String, color: Color32| {
            egui::Frame::new()
                .fill(p.surface)
                .corner_radius(egui::CornerRadius::same(p.radius))
                .inner_margin(egui::Margin::symmetric(8, 4))
                .show(ui, |ui| ui.label(RichText::new(text).color(color)));
        };
        pill(
            ui,
            format!("Semaines à 100 % : {reached_w} sur {}", weeks.len()),
            if reached_w > 0 { p.danger } else { p.text },
        );
        pill(
            ui,
            format!("Fenêtres de 5 h à 100 % : {reached_f} sur {}", fives.len()),
            if reached_f > 0 { p.warn } else { p.text },
        );
        pill(ui, format!("Dépassement : {}", money(overflow)), if overflow > 0.0 { p.danger } else { p.text_dim });
    });
    if weeks.is_empty() && fives.is_empty() {
        ui.label(RichText::new("Rien de relevé sur cette période.").color(p.text_dim));
        return;
    }
    // Chaque semaine relevée et ses fenêtres de 5 h ; les fenêtres hors semaine relevée, par semaine du calendrier.
    let mut groups: Vec<(String, Option<&WindowStat>, Vec<&WindowStat>)> = weeks
        .iter()
        .map(|w| {
            let label = format!(
                "Semaine du {} au {}",
                readings::local(w.start, "%d/%m %H:%M"),
                readings::local(w.resets_at, "%d/%m %H:%M")
            );
            let inside: Vec<&WindowStat> =
                fives.iter().copied().filter(|f| f.resets_at > w.start && f.resets_at <= w.resets_at).collect();
            (label, Some(*w), inside)
        })
        .collect();
    let mut loose: BTreeMap<i64, Vec<&WindowStat>> = BTreeMap::new();
    for f in &fives {
        if !weeks.iter().any(|w| f.resets_at > w.start && f.resets_at <= w.resets_at) {
            loose.entry(f.start.div_euclid(7 * 86_400)).or_default().push(f);
        }
    }
    for (week, list) in loose.into_iter().rev() {
        groups.push((format!("Semaine du {}", readings::local(week * 7 * 86_400, "%d/%m")), None, list));
    }
    for (label, week, mut list) in groups {
        list.sort_by_key(|w| w.resets_at);
        egui::Frame::new()
            .fill(p.panel)
            .stroke(Stroke::new(1.0, p.border))
            .corner_radius(egui::CornerRadius::same(p.radius))
            .inner_margin(egui::Margin::same(10))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(RichText::new(label).strong().color(p.text_muted));
                    if let Some(w) = week {
                        bar(ui, p, w.max_pct, 220.0, 10.0);
                        ui.label(
                            RichText::new(format!("max {}", readings::pct_text(w.max_pct)))
                                .color(tone_color(p, w.max_pct)),
                        );
                        if let Some(at) = w.reached_at {
                            ui.label(
                                RichText::new(format!("100 % le {}", readings::local(at, "%d/%m à %H:%M")))
                                    .color(p.danger),
                            );
                        }
                        if w.overflow > 0.0 {
                            ui.label(RichText::new(format!("dépassement {}", money(w.overflow))).color(p.danger));
                        }
                    }
                });
                strip(ui, p, &list);
            });
        ui.add_space(6.0);
    }
}

/// Les fenêtres de 5 h, une barre chacune (hauteur : maximum atteint).
fn strip(ui: &mut egui::Ui, p: &Palette, list: &[&WindowStat]) {
    if list.is_empty() {
        return;
    }
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 3.0;
        ui.label(RichText::new("5 h :").small().color(p.text_dim));
        for w in list {
            let (rect, resp) = ui.allocate_exact_size(vec2(12.0, 36.0), Sense::hover());
            ui.painter().rect_filled(rect, 2.0, p.widget);
            let h = rect.height() * (w.max_pct.clamp(0.0, 100.0) as f32 / 100.0);
            let filled = egui::Rect::from_min_max(egui::pos2(rect.left(), rect.bottom() - h.max(1.0)), rect.max);
            ui.painter().rect_filled(filled, 2.0, tone_color(p, w.max_pct));
            let mut tip = format!(
                "Fenêtre de 5 h, du {} au {}\nmaximum {}",
                readings::local(w.start, "%d/%m %H:%M"),
                readings::local(w.resets_at, "%H:%M"),
                readings::pct_text(w.max_pct)
            );
            if let Some(at) = w.reached_at {
                tip.push_str(&format!("\n100 % atteint à {}", readings::local(at, "%H:%M")));
            }
            if w.cost > 0.0 {
                tip.push_str(&format!("\n{} consommés (équivalent API)", money(w.cost)));
            }
            if w.overflow > 0.0 {
                tip.push_str(&format!(", dont {} limite atteinte", money(w.overflow)));
            }
            resp.on_hover_text(tip);
        }
    });
}

pub fn settings(
    ui: &mut egui::Ui,
    p: &Palette,
    profiles: &[Profile],
    dir: &Path,
    message: Option<&(String, bool)>,
) -> Option<SettingsAction> {
    let mut action = None;
    ui.label(RichText::new("Relais des limites").size(14.5).strong());
    ui.label(
        RichText::new(
            "Claude Code ne donne le taux des limites qu'à sa ligne d'état. Le relais de Gringotts devient la ligne \
             d'état du profil : il relève les limites à chaque mise à jour, montre dans le terminal la part de la \
             conversation, et rejoue la ligne d'état d'avant (remise en place au retrait).",
        )
        .color(p.text_dim),
    );
    ui.add_space(4.0);
    for (i, pr) in profiles.iter().enumerate() {
        let paths = pr.paths();
        let installed = crate::relay::installed(&paths);
        ui.horizontal(|ui| {
            ui.label(RichText::new(&pr.name).strong());
            ui.label(RichText::new(paths.config_dir.display().to_string()).small().color(p.text_dim));
            if installed {
                ui.label(RichText::new("relais installé").color(p.ok));
                if ui.button("Retirer").clicked() {
                    action = Some(SettingsAction::Uninstall(i));
                }
            } else if ui.add_enabled(paths.config_dir.is_dir(), egui::Button::new("Installer le relais")).clicked() {
                action = Some(SettingsAction::Install(i));
            }
        });
    }
    if let Some((text, ok)) = message {
        ui.add_space(4.0);
        ui.label(RichText::new(text).color(if *ok { p.ok } else { p.danger }));
    }
    ui.add_space(10.0);
    ui.label(RichText::new("Données").size(14.5).strong());
    ui.label(
        RichText::new(format!(
            "Relevés, résumé des conversations (gardé quand Claude Code efface une transcription) et parts pour le \
             relais : {}",
            dir.display()
        ))
        .color(p.text_dim),
    );
    ui.add_space(10.0);
    egui::CollapsingHeader::new("Prix utilisés (équivalent API, $ par million de jetons)").show(ui, |ui| {
        egui::Grid::new("gringotts-prix").striped(true).show(ui, |ui| {
            for h in ["Modèle", "Entrée", "Cache 5 min", "Cache 1 h", "Lecture cache", "Sortie"] {
                ui.label(RichText::new(h).strong());
            }
            ui.end_row();
            for m in [
                "claude-fable-5-1",
                "claude-opus-5-5",
                "claude-opus-4-8",
                "claude-opus-4-1",
                "claude-sonnet-5-5",
                "claude-sonnet-4-6",
                "claude-haiku-5-5",
                "claude-haiku-4-5",
            ] {
                let Some(pr) = crate::prices::price(m, 0) else { continue };
                ui.label(crate::prices::short_name(m));
                for x in [pr.input, pr.write_5m, pr.write_1h, pr.read, pr.output] {
                    ui.label(format!("{x}"));
                }
                ui.end_row();
            }
        });
    });
    action
}
