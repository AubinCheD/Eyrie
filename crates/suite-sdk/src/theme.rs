//! Les thèmes de la suite.
//!
//! Les trois thèmes de Fawkes (Graphite, Fluent, Nuit colorée), les plus aboutis des six modules
//! (contrastes WCAG testés), plus *Système* qui suit Windows (Graphite en sombre, Fluent en clair).
//! La palette est sémantique : les modules la complètent par leurs couleurs métier (terminal et
//! couleurs ANSI de Fawkes, branches de DragonTree) sans toucher au style global.

use eframe::egui::{self, Color32, CornerRadius, Stroke, Visuals};
use serde::{Deserialize, Serialize};

/// Le thème choisi dans les réglages de la suite.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeChoice {
    #[default]
    Systeme,
    Graphite,
    Fluent,
    Nuit,
}

impl ThemeChoice {
    pub const ALL: [ThemeChoice; 4] =
        [ThemeChoice::Systeme, ThemeChoice::Graphite, ThemeChoice::Fluent, ThemeChoice::Nuit];

    /// Nom affiché (en français : la clé du catalogue de traduction).
    pub fn label(self) -> &'static str {
        match self {
            ThemeChoice::Systeme => "Système",
            ThemeChoice::Graphite => "Graphite",
            ThemeChoice::Fluent => "Fluent",
            ThemeChoice::Nuit => "Nuit colorée",
        }
    }
}

/// Un thème effectif.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThemeId {
    Graphite,
    Fluent,
    Nuit,
}

impl ThemeId {
    pub const ALL: [ThemeId; 3] = [ThemeId::Graphite, ThemeId::Fluent, ThemeId::Nuit];
}

/// *Système* suit le thème de Windows, tel qu'egui le relève.
pub fn resolve(choice: ThemeChoice, ctx: &egui::Context) -> ThemeId {
    resolve_with(choice, ctx.system_theme())
}

pub fn resolve_with(choice: ThemeChoice, system: Option<egui::Theme>) -> ThemeId {
    match choice {
        ThemeChoice::Graphite => ThemeId::Graphite,
        ThemeChoice::Fluent => ThemeId::Fluent,
        ThemeChoice::Nuit => ThemeId::Nuit,
        ThemeChoice::Systeme => match system {
            Some(egui::Theme::Light) => ThemeId::Fluent,
            _ => ThemeId::Graphite,
        },
    }
}

/// Palette sémantique, reprise de `Fawkes/src/theme.rs` (partie interface).
#[derive(Debug)]
pub struct Palette {
    pub dark: bool,
    /// Barre de titre et menus.
    pub chrome: Color32,
    /// Barre d'activité, barres latérales.
    pub sidebar: Color32,
    /// Fond des cases.
    pub panel: Color32,
    /// Onglets, surfaces légèrement relevées.
    pub surface: Color32,
    pub surface_hover: Color32,
    pub text: Color32,
    /// Titres inactifs.
    pub text_muted: Color32,
    /// Libellés secondaires.
    pub text_dim: Color32,
    pub accent: Color32,
    pub button: Color32,
    pub on_button: Color32,
    pub danger: Color32,
    pub danger_button: Color32,
    pub ok: Color32,
    pub warn: Color32,
    /// Ce qui attend une réponse (cadre, pastille et son texte).
    pub alert: Color32,
    pub alert_badge: Color32,
    pub on_alert_badge: Color32,
    pub border: Color32,
    /// Case vide.
    pub slot: Color32,
    pub window: Color32,
    pub extreme: Color32,
    pub widget: Color32,
    pub widget_hover: Color32,
    pub widget_active: Color32,
    pub widget_stroke: Color32,
    pub selection: Color32,
    pub selection_stroke: Color32,
    pub radius: u8,
}

const fn rgb(hex: u32) -> Color32 {
    Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

pub static GRAPHITE: Palette = Palette {
    dark: true,
    chrome: rgb(0x121316),
    sidebar: rgb(0x16171b),
    panel: rgb(0x1b1d22),
    surface: rgb(0x23252b),
    surface_hover: rgb(0x2c2f37),
    text: rgb(0xe3e5ea),
    text_muted: rgb(0xc0c4cc),
    text_dim: rgb(0x9aa0ac),
    accent: rgb(0x5b8def),
    button: rgb(0x3570d4),
    on_button: rgb(0xffffff),
    danger: rgb(0xe5677a),
    danger_button: rgb(0xc0394d),
    ok: rgb(0x4cb872),
    warn: rgb(0xd9b43c),
    alert: rgb(0xf2a93b),
    alert_badge: rgb(0xf2a93b),
    on_alert_badge: rgb(0x121316),
    border: rgb(0x3a3e48),
    slot: rgb(0x17181d),
    window: rgb(0x202229),
    extreme: rgb(0x121317),
    widget: rgb(0x2a2d35),
    widget_hover: rgb(0x343842),
    widget_active: rgb(0x3b404c),
    widget_stroke: rgb(0x4a4f5b),
    selection: rgb(0x2c4a73),
    selection_stroke: rgb(0x9cc0ff),
    radius: 6,
};

pub static FLUENT: Palette = Palette {
    dark: false,
    chrome: rgb(0xeef1f6),
    sidebar: rgb(0xeef1f6),
    panel: rgb(0xf9fafc),
    surface: rgb(0xe6eaf0),
    surface_hover: rgb(0xdce2ea),
    text: rgb(0x1b1d22),
    text_muted: rgb(0x30343c),
    text_dim: rgb(0x5c6370),
    accent: rgb(0x0067c0),
    button: rgb(0x0067c0),
    on_button: rgb(0xffffff),
    danger: rgb(0xc42b1c),
    danger_button: rgb(0xc42b1c),
    ok: rgb(0x1d7a41),
    warn: rgb(0x8a5a00),
    alert: rgb(0xb56900),
    alert_badge: rgb(0x8a5200),
    on_alert_badge: rgb(0xffffff),
    border: rgb(0xccd3dd),
    slot: rgb(0xf1f4f8),
    window: rgb(0xffffff),
    extreme: rgb(0xf7f9fc),
    widget: rgb(0xeef1f5),
    widget_hover: rgb(0xe2e7ee),
    widget_active: rgb(0xd6dde7),
    widget_stroke: rgb(0xa9b2bf),
    selection: rgb(0xcce0f5),
    selection_stroke: rgb(0x0067c0),
    radius: 8,
};

pub static NUIT: Palette = Palette {
    dark: true,
    chrome: rgb(0x0b1020),
    sidebar: rgb(0x0f1527),
    panel: rgb(0x121a2e),
    surface: rgb(0x1a2338),
    surface_hover: rgb(0x233050),
    text: rgb(0xe6ebf5),
    text_muted: rgb(0xc3cbdc),
    text_dim: rgb(0x9aa6c0),
    accent: rgb(0x35c2e6),
    button: rgb(0x2563c9),
    on_button: rgb(0xffffff),
    danger: rgb(0xff6b85),
    danger_button: rgb(0xc8344f),
    ok: rgb(0x3ddc97),
    warn: rgb(0xffd166),
    alert: rgb(0xffb020),
    alert_badge: rgb(0xffb020),
    on_alert_badge: rgb(0x0b1020),
    border: rgb(0x26314d),
    slot: rgb(0x0f1527),
    window: rgb(0x151d33),
    extreme: rgb(0x0b1020),
    widget: rgb(0x1d2740),
    widget_hover: rgb(0x26314d),
    widget_active: rgb(0x2e3a5a),
    widget_stroke: rgb(0x46567c),
    selection: rgb(0x24406e),
    selection_stroke: rgb(0x7fdcf2),
    radius: 9,
};

pub fn palette(id: ThemeId) -> &'static Palette {
    match id {
        ThemeId::Graphite => &GRAPHITE,
        ThemeId::Fluent => &FLUENT,
        ThemeId::Nuit => &NUIT,
    }
}

/// Le style egui d'une palette, tel que Fawkes l'applique (`Fawkes/src/theme.rs`, `apply_style`),
/// avec le zoom de la suite.
pub fn style_for(p: &Palette, base: &egui::Style) -> egui::Style {
    let mut v = if p.dark { Visuals::dark() } else { Visuals::light() };
    v.panel_fill = p.panel;
    v.window_fill = p.window;
    v.extreme_bg_color = p.extreme;
    v.faint_bg_color = p.surface;
    v.hyperlink_color = p.accent;
    v.selection.bg_fill = p.selection;
    v.selection.stroke = Stroke::new(1.0, p.selection_stroke);
    v.window_corner_radius = CornerRadius::same(p.radius + 2);
    v.menu_corner_radius = CornerRadius::same(p.radius);
    v.window_stroke = Stroke::new(1.0, p.border);
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, p.border);
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, p.text);
    v.widgets.inactive.weak_bg_fill = p.widget;
    v.widgets.inactive.bg_fill = p.widget;
    v.widgets.inactive.bg_stroke = Stroke::new(1.0, p.widget_stroke);
    v.widgets.inactive.fg_stroke = Stroke::new(1.0, p.text_muted);
    v.widgets.hovered.weak_bg_fill = p.widget_hover;
    v.widgets.hovered.bg_fill = p.widget_hover;
    v.widgets.hovered.fg_stroke = Stroke::new(1.5, p.text);
    v.widgets.active.weak_bg_fill = p.widget_active;
    v.widgets.active.bg_fill = p.widget_active;
    v.widgets.active.fg_stroke = Stroke::new(1.5, p.text);
    for w in [&mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active, &mut v.widgets.open] {
        w.corner_radius = CornerRadius::same(p.radius.saturating_sub(1));
    }
    let mut s = base.clone();
    s.visuals = v;
    s.spacing.item_spacing = egui::vec2(6.0, 5.0);
    s.spacing.button_padding = egui::vec2(8.0, 4.0);
    s.spacing.menu_margin = egui::Margin::same(6);
    s.interaction.tooltip_delay = 0.35;
    s.animation_time = 0.08;
    s
}

/// Applique le thème à tout le contexte : clair et sombre reçoivent la même palette, pour que le
/// thème choisi tienne quel que soit celui de Windows.
pub fn apply(ctx: &egui::Context, p: &Palette, zoom: f32) {
    let theme = if p.dark { egui::Theme::Dark } else { egui::Theme::Light };
    ctx.options_mut(|o| {
        let style = std::sync::Arc::new(style_for(p, &o.dark_style));
        o.dark_style = style.clone();
        o.light_style = style;
        o.theme_preference = theme.into();
        o.zoom_factor = zoom;
        // Le zoom au clavier (Ctrl+molette, Ctrl+=) appartiendrait au module qui a le clavier.
        o.zoom_with_keyboard = false;
    });
}

/// La couleur d'un module, lisible sur les fonds de la palette : assombrie dans un thème clair
/// (les couleurs des modules sont pensées pour un fond sombre).
pub fn readable(p: &Palette, c: Color32) -> Color32 {
    let mut c = c;
    for _ in 0..6 {
        if contrast(c, p.surface) >= 3.0 {
            break;
        }
        c = if p.dark { c.lerp_to_gamma(Color32::WHITE, 0.2) } else { c.lerp_to_gamma(Color32::BLACK, 0.2) };
    }
    c
}

/// Mêle une couleur au fond des surfaces (fonds de pastilles, d'onglets colorés).
pub fn tint(p: &Palette, c: Color32, amount: f32) -> Color32 {
    p.surface.lerp_to_gamma(c, amount)
}

/// Contraste WCAG entre deux couleurs opaques (1 à 21).
pub fn contrast(a: Color32, b: Color32) -> f32 {
    let lum = |c: Color32| {
        let ch = |v: u8| {
            let s = v as f32 / 255.0;
            if s <= 0.03928 { s / 12.92 } else { ((s + 0.055) / 1.055).powf(2.4) }
        };
        0.2126 * ch(c.r()) + 0.7152 * ch(c.g()) + 0.0722 * ch(c.b())
    };
    let (la, lb) = (lum(a), lum(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Chaque thème reste lisible (WCAG) : 4,5 pour le texte, 3 pour les contours et icônes.
    #[test]
    fn every_theme_is_legible() {
        for id in ThemeId::ALL {
            let p = palette(id);
            let mut failures = Vec::new();
            let mut check = |what: &str, a: Color32, b: Color32, min: f32| {
                let c = contrast(a, b);
                if c < min {
                    failures.push(format!("{what} : {c:.2} < {min}"));
                }
            };
            for (bg_name, bg) in [
                ("barre", p.sidebar),
                ("case", p.panel),
                ("onglet", p.surface),
                ("fenêtre", p.window),
                ("titre", p.chrome),
            ] {
                check(&format!("texte / {bg_name}"), p.text, bg, 4.5);
                check(&format!("texte secondaire / {bg_name}"), p.text_dim, bg, 4.5);
            }
            for (bg_name, bg) in [("barre", p.sidebar), ("onglet", p.surface), ("survol", p.surface_hover)] {
                check(&format!("titre inactif / {bg_name}"), p.text_muted, bg, 4.5);
            }
            check("bouton principal", p.on_button, p.button, 4.5);
            check("bouton destructif", Color32::WHITE, p.danger_button, 4.5);
            check("pastille « attend »", p.on_alert_badge, p.alert_badge, 4.5);
            check("icône active / barre", p.accent, p.sidebar, 3.0);
            check("contour des cases à cocher", p.widget_stroke, p.window, 1.9);
            assert!(failures.is_empty(), "{id:?} :\n{}", failures.join("\n"));
        }
    }

    /// Le thème choisi tient quel que soit celui de Windows.
    #[test]
    fn chosen_theme_wins_over_windows() {
        for (id, system) in [(ThemeId::Graphite, egui::Theme::Light), (ThemeId::Fluent, egui::Theme::Dark)] {
            let ctx = egui::Context::default();
            apply(&ctx, palette(id), 1.0);
            ctx.options_mut(|o| o.fallback_theme = system);
            let style = ctx.global_style();
            assert_eq!(style.visuals.dark_mode, palette(id).dark, "{id:?}");
            assert_eq!(style.visuals.window_fill, palette(id).window, "{id:?}");
        }
    }

    #[test]
    fn module_colors_become_readable() {
        for id in ThemeId::ALL {
            let p = palette(id);
            for c in [
                Color32::from_rgb(0x9f, 0xc5, 0xff),
                Color32::from_rgb(0xd9, 0xb4, 0x3c),
                Color32::from_rgb(0x3a, 0xa8, 0xd8),
            ] {
                assert!(contrast(readable(p, c), p.surface) >= 3.0, "{id:?} {c:?}");
            }
        }
    }

    #[test]
    fn system_follows_windows() {
        assert_eq!(resolve_with(ThemeChoice::Systeme, Some(egui::Theme::Light)), ThemeId::Fluent);
        assert_eq!(resolve_with(ThemeChoice::Systeme, Some(egui::Theme::Dark)), ThemeId::Graphite);
        assert_eq!(resolve_with(ThemeChoice::Systeme, None), ThemeId::Graphite);
        assert_eq!(resolve_with(ThemeChoice::Nuit, Some(egui::Theme::Light)), ThemeId::Nuit);
    }
}
