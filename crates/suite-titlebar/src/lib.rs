//! Barre de titre dessinée par l'application, comme celles de VS Code et de Fawkes : icône, titre et menus à gauche,
//! ce que l'application veut à droite, puis réduire / agrandir / fermer. Le reste de la barre appartient à Windows,
//! qui y garde déplacement, double-clic, accrochage aux bords de l'écran et menu système (clic droit) ; les bords de
//! la fenêtre la redimensionnent.
//!
//! Reprise de Fawkes (`src/winhook.rs`, `src/ui/menu.rs`) pour Eyrie et DragonTree lancé seul. La fenêtre est créée
//! sans décorations (`ViewportBuilder::with_decorations(false)`) ; [`install`] chaîne sa procédure de fenêtre pour
//! répondre à `WM_NCHITTEST` d'après la [`Caption`] que [`show`] pose à chaque image.

use std::cell::RefCell;

use egui::{Color32, Rect, RichText, pos2, vec2};

/// Hauteur de la barre (celle de Windows 11).
pub const HEIGHT: f32 = 32.0;
/// Largeur des boutons réduire / agrandir / fermer (ceux de Windows).
pub const BUTTON_W: f32 = 46.0;

/// Les couleurs de la barre, prises dans le thème de l'application.
#[derive(Clone, Copy, Debug)]
pub struct Colors {
    pub fill: Color32,
    pub text: Color32,
    /// Titre d'une fenêtre qui n'a pas le focus.
    pub text_dim: Color32,
    /// Fond d'un bouton de la fenêtre survolé.
    pub hover: Color32,
}

/// Les infobulles des boutons de la fenêtre, dans la langue de l'application.
#[derive(Clone, Copy, Debug)]
pub struct Labels<'a> {
    pub minimize: &'a str,
    pub maximize: &'a str,
    pub restore: &'a str,
    pub close: &'a str,
}

impl Default for Labels<'_> {
    fn default() -> Self {
        Labels { minimize: "Réduire", maximize: "Agrandir", restore: "Niveau inférieur", close: "Fermer" }
    }
}

/// Ce que la barre montre d'elle-même.
pub struct Bar<'a> {
    pub icon: Option<egui::TextureId>,
    pub title: &'a str,
    pub colors: Colors,
    pub labels: Labels<'a>,
}

/// Les deux côtés de la barre que remplit l'application.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    /// Les menus, après l'icône et le titre, de gauche à droite.
    Left,
    /// Ce qui précède les boutons de la fenêtre, de droite à gauche.
    Right,
}

/// Dessine la barre en haut de `ui` et la donne à Windows (voir [`install`]).
///
/// `contents` remplit chaque côté (la droite d'abord) et rend la zone de ce qu'il y a mis : elle reste à
/// l'application, le reste de la barre déplace la fenêtre. Une barre de menus egui prend toute la largeur : rendre la
/// zone de ses boutons, pas la sienne.
pub fn show(ui: &mut egui::Ui, bar: &Bar, mut contents: impl FnMut(Side, &mut egui::Ui) -> Rect) {
    let ctx = ui.ctx().clone();
    let (maximized, focused) =
        ctx.input(|i| (i.viewport().maximized.unwrap_or(false), i.viewport().focused.unwrap_or(true)));
    let mut holes: Vec<Rect> = Vec::new();
    egui::Panel::top("suite-barre-de-titre")
        .exact_size(HEIGHT)
        .resizable(false)
        .show_separator_line(false)
        .frame(egui::Frame::new().fill(bar.colors.fill))
        .show(ui, |ui| {
            let full = ui.max_rect();
            let close = Rect::from_min_size(pos2(full.right() - BUTTON_W, full.top()), vec2(BUTTON_W, full.height()));
            let max = close.translate(vec2(-BUTTON_W, 0.0));
            let min = max.translate(vec2(-BUTTON_W, 0.0));
            let c = bar.colors;
            if window_button(ui, min, Glyph::Minimize, bar.labels.minimize, c).clicked() {
                ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
            }
            let (glyph, tip) =
                if maximized { (Glyph::Restore, bar.labels.restore) } else { (Glyph::Maximize, bar.labels.maximize) };
            if window_button(ui, max, glyph, tip, c).clicked() {
                ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
            }
            if window_button(ui, close, Glyph::Close, bar.labels.close, c).clicked() {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            holes.push(min.union(close));

            let right_room = Rect::from_min_max(full.min, pos2(min.left() - 8.0, full.bottom()));
            let used = ui
                .scope_builder(
                    egui::UiBuilder::new()
                        .max_rect(right_room)
                        .layout(egui::Layout::right_to_left(egui::Align::Center)),
                    |ui| contents(Side::Right, ui),
                )
                .inner;
            if used.is_positive() {
                holes.push(used);
            }
            let end = if used.is_positive() { used.left() } else { right_room.right() };

            let left_room = Rect::from_min_max(full.min, pos2(end - 8.0, full.bottom()));
            ui.scope_builder(
                egui::UiBuilder::new().max_rect(left_room).layout(egui::Layout::left_to_right(egui::Align::Center)),
                |ui| {
                    ui.add_space(10.0);
                    if let Some(icon) = bar.icon {
                        ui.add(egui::Image::new((icon, vec2(16.0, 16.0))));
                        ui.add_space(6.0);
                    }
                    let color = if focused { c.text } else { c.text_dim };
                    ui.label(RichText::new(bar.title).size(12.5).color(color));
                    ui.add_space(12.0);
                    holes.push(contents(Side::Left, ui));
                },
            );
        });
    let ppp = ctx.pixels_per_point();
    let px = |v: f32| (v * ppp).round() as i32;
    let holes = holes.iter().map(|r| [px(r.left()), px(r.top()), px(r.right()), px(r.bottom())]).collect();
    set_caption(Some(Caption { height: px(HEIGHT), border: px(5.0), holes }));
}

#[derive(Clone, Copy)]
enum Glyph {
    Minimize,
    Maximize,
    Restore,
    Close,
}

/// Bouton de la fenêtre, dessiné comme ceux de Windows : fond au survol, rouge pour Fermer.
fn window_button(ui: &mut egui::Ui, rect: Rect, glyph: Glyph, tip: &str, c: Colors) -> egui::Response {
    let id = ui.scope_id().with(("bouton-fenetre", glyph as u8));
    let resp = ui.interact(rect, id, egui::Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, tip));
    let resp = resp.on_hover_text(tip);
    let close = matches!(glyph, Glyph::Close);
    let mut color = c.text;
    if resp.hovered() {
        let bg = if close { Color32::from_rgb(196, 43, 28) } else { c.hover };
        ui.painter().rect_filled(rect, 0.0, bg);
        if close {
            color = Color32::WHITE;
        }
    }
    let m = rect.center();
    let s = egui::Stroke::new(1.0, color);
    let p = ui.painter();
    let sq = |center: egui::Pos2| Rect::from_center_size(center, vec2(9.0, 9.0));
    match glyph {
        Glyph::Minimize => {
            p.line_segment([pos2(m.x - 5.0, m.y), pos2(m.x + 5.0, m.y)], s);
        }
        Glyph::Maximize => {
            p.rect_stroke(sq(m), 1.0, s, egui::StrokeKind::Inside);
        }
        Glyph::Restore => {
            p.rect_stroke(sq(m + vec2(-1.5, 1.5)), 1.0, s, egui::StrokeKind::Inside);
            let back = sq(m + vec2(1.5, -1.5));
            p.line_segment([pos2(back.left() + 2.0, back.top()), back.right_top()], s);
            p.line_segment([back.right_top(), pos2(back.right(), back.bottom() - 2.0)], s);
        }
        Glyph::Close => {
            p.line_segment([m + vec2(-5.0, -5.0), m + vec2(5.0, 5.0)], s);
            p.line_segment([m + vec2(5.0, -5.0), m + vec2(-5.0, 5.0)], s);
        }
    }
    resp
}

/// La barre telle que Windows la voit, en pixels de la zone client (toute la fenêtre).
#[derive(Clone, Debug, PartialEq)]
pub struct Caption {
    pub height: i32,
    /// Épaisseur des bords de redimensionnement.
    pub border: i32,
    /// Zones qui restent à l'application (menus, boutons) : gauche, haut, droite, bas (exclus).
    pub holes: Vec<[i32; 4]>,
}

/// Réponses à `WM_NCHITTEST` (valeurs de Windows, `HT*`).
pub mod hit {
    pub const CLIENT: u32 = 1;
    pub const CAPTION: u32 = 2;
    pub const LEFT: u32 = 10;
    pub const RIGHT: u32 = 11;
    pub const TOP: u32 = 12;
    pub const TOPLEFT: u32 = 13;
    pub const TOPRIGHT: u32 = 14;
    pub const BOTTOM: u32 = 15;
    pub const BOTTOMLEFT: u32 = 16;
    pub const BOTTOMRIGHT: u32 = 17;
}

impl Caption {
    /// Ce que Windows doit faire au point (x, y) d'une zone client de w × h.
    pub fn hit(&self, x: i32, y: i32, w: i32, h: i32, maximized: bool) -> u32 {
        if !maximized {
            let b = self.border;
            match (x < b, x >= w - b, y < b, y >= h - b) {
                (true, _, true, _) => return hit::TOPLEFT,
                (_, true, true, _) => return hit::TOPRIGHT,
                (true, _, _, true) => return hit::BOTTOMLEFT,
                (_, true, _, true) => return hit::BOTTOMRIGHT,
                (true, ..) => return hit::LEFT,
                (_, true, ..) => return hit::RIGHT,
                (_, _, true, _) => return hit::TOP,
                (_, _, _, true) => return hit::BOTTOM,
                _ => {}
            }
        }
        let in_hole = self.holes.iter().any(|[l, t, r, b]| (*l..*r).contains(&x) && (*t..*b).contains(&y));
        if y < self.height && !in_hole { hit::CAPTION } else { hit::CLIENT }
    }
}

thread_local! {
    /// Posée à chaque image par le fil de l'interface, qui est aussi celui qui reçoit les messages de la fenêtre.
    static CAPTION: RefCell<Option<Caption>> = const { RefCell::new(None) };
}

/// La barre dessinée par l'application, ou `None` pour celle de Windows.
pub fn set_caption(caption: Option<Caption>) {
    CAPTION.with(|c| *c.borrow_mut() = caption);
}

/// La barre posée à la dernière image (essais).
pub fn caption() -> Option<Caption> {
    CAPTION.with(|c| c.borrow().clone())
}

/// Chaîne la procédure de la fenêtre `window` (la fenêtre principale, `eframe::Frame`), une seule fois par
/// processus : à appeler depuis le fil de l'interface tant que ça n'a pas abouti (la fenêtre n'existe qu'après la
/// première image). Ailleurs que sous Windows, ne fait rien.
pub fn install(window: &impl raw_window_handle::HasWindowHandle) -> bool {
    #[cfg(windows)]
    {
        win::install(window)
    }
    #[cfg(not(windows))]
    {
        let _ = window;
        true
    }
}

#[cfg(windows)]
mod win {
    use std::sync::atomic::{AtomicIsize, Ordering};

    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
    use windows_sys::Win32::Graphics::Dwm::{DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND, DwmSetWindowAttribute};
    use windows_sys::Win32::Graphics::Gdi::ScreenToClient;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CallWindowProcW, GWLP_WNDPROC, GetClientRect, IsZoomed, SetWindowLongPtrW, WM_NCHITTEST, WNDPROC,
    };

    static PREV_PROC: AtomicIsize = AtomicIsize::new(0);

    unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        if msg == WM_NCHITTEST
            && let Some(code) = super::CAPTION.with(|c| {
                c.borrow().as_ref().map(|c| {
                    // Coordonnées d'écran signées dans `lparam`.
                    let mut p =
                        POINT { x: (lparam & 0xFFFF) as i16 as i32, y: ((lparam >> 16) & 0xFFFF) as i16 as i32 };
                    let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
                    unsafe {
                        ScreenToClient(hwnd, &mut p);
                        GetClientRect(hwnd, &mut r);
                        c.hit(p.x, p.y, r.right, r.bottom, IsZoomed(hwnd) != 0)
                    }
                })
            })
        {
            return code as LRESULT;
        }
        unsafe {
            let prev: WNDPROC = std::mem::transmute(PREV_PROC.load(Ordering::Relaxed));
            CallWindowProcW(prev, hwnd, msg, wparam, lparam)
        }
    }

    pub fn install(window: &impl HasWindowHandle) -> bool {
        if PREV_PROC.load(Ordering::Relaxed) != 0 {
            return true;
        }
        let Ok(handle) = window.window_handle() else { return false };
        let RawWindowHandle::Win32(h) = handle.as_raw() else { return false };
        let hwnd = h.hwnd.get() as HWND;
        unsafe {
            let prev = SetWindowLongPtrW(hwnd, GWLP_WNDPROC, wndproc as *const () as isize);
            PREV_PROC.store(prev, Ordering::Relaxed);
            // Coins arrondis de Windows 11, que la fenêtre perd sans la barre de titre de Windows (sans effet sur
            // Windows 10).
            let round = DWMWCP_ROUND;
            DwmSetWindowAttribute(
                hwnd,
                DWMWA_WINDOW_CORNER_PREFERENCE as u32,
                (&round as *const i32).cast(),
                size_of::<i32>() as u32,
            );
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fenêtre de 800 × 600 px, bords de 5 px, barre de 32 px avec deux zones actives (menus, boutons de la fenêtre).
    fn frame() -> Caption {
        Caption { height: 32, border: 5, holes: vec![[40, 0, 200, 32], [662, 0, 800, 32]] }
    }

    #[test]
    fn edges_resize_the_window() {
        let c = frame();
        assert_eq!(c.hit(0, 300, 800, 600, false), hit::LEFT);
        assert_eq!(c.hit(799, 300, 800, 600, false), hit::RIGHT);
        assert_eq!(c.hit(400, 599, 800, 600, false), hit::BOTTOM);
        assert_eq!(c.hit(400, 2, 800, 600, false), hit::TOP);
        assert_eq!(c.hit(2, 2, 800, 600, false), hit::TOPLEFT);
        assert_eq!(c.hit(798, 598, 800, 600, false), hit::BOTTOMRIGHT);
        assert_eq!(c.hit(799, 1, 800, 600, false), hit::TOPRIGHT, "le coin l'emporte sur le bouton Fermer");
    }

    /// La barre déplace la fenêtre, sauf sur les menus et les boutons ; le reste est le contenu de l'application.
    #[test]
    fn the_caption_moves_the_window_except_its_controls() {
        let c = frame();
        assert_eq!(c.hit(300, 16, 800, 600, false), hit::CAPTION);
        assert_eq!(c.hit(20, 16, 800, 600, false), hit::CAPTION, "icône et titre");
        assert_eq!(c.hit(100, 16, 800, 600, false), hit::CLIENT, "menus");
        assert_eq!(c.hit(700, 16, 800, 600, false), hit::CLIENT, "boutons de la fenêtre");
        assert_eq!(c.hit(300, 100, 800, 600, false), hit::CLIENT);
    }

    /// Agrandie, la fenêtre n'a pas de bords : le haut de l'écran est la barre de titre.
    #[test]
    fn a_maximized_window_has_no_edges() {
        let c = frame();
        assert_eq!(c.hit(300, 0, 800, 600, true), hit::CAPTION);
        assert_eq!(c.hit(0, 300, 800, 600, true), hit::CLIENT);
        assert_eq!(c.hit(799, 0, 800, 600, true), hit::CLIENT, "Fermer jusqu'au coin de l'écran");
    }

    /// Les valeurs de `hit` sont celles de Windows.
    #[cfg(windows)]
    #[test]
    fn hit_codes_are_the_windows_ones() {
        use windows_sys::Win32::UI::WindowsAndMessaging as w;
        assert_eq!(
            [hit::CLIENT, hit::CAPTION, hit::LEFT, hit::RIGHT, hit::TOP, hit::TOPLEFT, hit::TOPRIGHT],
            [w::HTCLIENT, w::HTCAPTION, w::HTLEFT, w::HTRIGHT, w::HTTOP, w::HTTOPLEFT, w::HTTOPRIGHT]
        );
        assert_eq!([hit::BOTTOM, hit::BOTTOMLEFT, hit::BOTTOMRIGHT], [w::HTBOTTOM, w::HTBOTTOMLEFT, w::HTBOTTOMRIGHT]);
    }

    /// La barre dessinée garde à l'application ses menus, ce qu'elle met à droite et les boutons de la fenêtre.
    #[test]
    fn show_leaves_the_controls_to_the_application() {
        let ctx = egui::Context::default();
        let bar = Bar {
            icon: None,
            title: "Essai",
            colors: Colors {
                fill: Color32::BLACK,
                text: Color32::WHITE,
                text_dim: Color32::GRAY,
                hover: Color32::DARK_GRAY,
            },
            labels: Labels::default(),
        };
        let mut menus = Rect::NOTHING;
        let mut bell = Rect::NOTHING;
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1000.0, 600.0))),
            ..Default::default()
        };
        let mut out = ctx.run_ui(input, |ui| {
            show(ui, &bar, |side, ui| match side {
                Side::Left => {
                    menus = ui.button("Fichier").rect;
                    menus
                }
                Side::Right => {
                    bell = ui.button("Cloche").rect;
                    ui.min_rect()
                }
            });
        });
        // Pas de rendu ici : l'atlas des polices n'est envoyé nulle part.
        out.textures_delta.clear();
        let c = caption().expect("barre posée");
        assert_eq!(c.height, 32);
        let at = |r: Rect| c.hit(r.center().x as i32, r.center().y as i32, 1000, 600, false);
        assert_eq!(at(menus), hit::CLIENT, "menus");
        assert_eq!(at(bell), hit::CLIENT, "à droite");
        assert!(bell.right() <= 1000.0 - 3.0 * BUTTON_W, "avant les boutons de la fenêtre");
        assert_eq!(c.hit(600, 16, 1000, 600, false), hit::CAPTION, "le milieu déplace la fenêtre");
        assert_eq!(c.hit(980, 16, 1000, 600, false), hit::CLIENT, "Fermer");
    }
}
