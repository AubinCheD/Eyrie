//! Icône de la suite, dessinée : quatre cases de couleurs, comme les modules qu'elle réunit. Et l'image des modules
//! qui en ont une (le phénix de Fawkes), à la place de leur glyphe.

use eframe::egui::{self, Color32, IntoAtoms as _};
use suite_sdk::ModuleInfo;

const COLORS: [[u8; 3]; 4] = [[0x5b, 0x8d, 0xef], [0xe0, 0x8a, 0x5c], [0x4c, 0xb8, 0x72], [0xb0, 0x5c, 0xc9]];

pub fn render(size: usize) -> Vec<u8> {
    let mut rgba = vec![0u8; size * size * 4];
    let s = size as f32;
    let gap = (s * 0.06).max(1.0);
    let half = (s - gap) / 2.0;
    let radius = s * 0.12;
    for y in 0..size {
        for x in 0..size {
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            let col = if fx < half {
                0
            } else if fx > half + gap {
                1
            } else {
                continue;
            };
            let row = if fy < half {
                0
            } else if fy > half + gap {
                1
            } else {
                continue;
            };
            // Coin arrondi de chaque case.
            let (x0, y0) = (col as f32 * (half + gap), row as f32 * (half + gap));
            let (lx, ly) = (fx - x0, fy - y0);
            let dx = (radius - lx).max(lx - (half - radius)).max(0.0);
            let dy = (radius - ly).max(ly - (half - radius)).max(0.0);
            let alpha = (radius + 0.5 - (dx * dx + dy * dy).sqrt()).clamp(0.0, 1.0);
            if alpha <= 0.0 {
                continue;
            }
            let c = COLORS[row * 2 + col];
            let i = (y * size + x) * 4;
            rgba[i..i + 4].copy_from_slice(&[c[0], c[1], c[2], (alpha * 255.0) as u8]);
        }
    }
    rgba
}

pub fn window_icon() -> egui::IconData {
    let size = 64;
    egui::IconData { rgba: render(size), width: size as u32, height: size as u32 }
}

/// L'image d'un module qui en a une (`ModuleInfo::logo`), chargée une fois pour toutes les fenêtres.
pub fn logo(ctx: &egui::Context, info: &ModuleInfo) -> Option<egui::TextureHandle> {
    let render = info.logo?;
    let id = egui::Id::unique(("suite-logo", info.id));
    if let Some(t) = ctx.data(|d| d.get_temp::<egui::TextureHandle>(id)) {
        return Some(t);
    }
    // Rendue en grand, réduite à l'affichage : les mipmaps gardent les petites tailles nettes.
    const SIZE: usize = 64;
    let image = egui::ColorImage::from_rgba_unmultiplied([SIZE, SIZE], &render(SIZE as u32));
    let options = egui::TextureOptions::LINEAR.with_mipmap_mode(Some(egui::TextureFilter::Linear));
    let t = ctx.load_texture(format!("logo-{}", info.id), image, options);
    ctx.data_mut(|d| d.insert_temp(id, t.clone()));
    Some(t)
}

/// L'image du module à `size` points, éteinte (`on` faux) pour un module arrêté.
fn logo_image(t: &egui::TextureHandle, size: f32, on: bool) -> egui::Image<'static> {
    let tint = if on { Color32::WHITE } else { Color32::WHITE.gamma_multiply(0.4) };
    egui::Image::new((t.id(), egui::vec2(size, size))).tint(tint)
}

/// L'icône d'un module dans une ligne : son image, sinon son glyphe dans sa couleur.
pub fn module_icon(ui: &mut egui::Ui, info: &ModuleInfo, size: f32, color: Color32) -> egui::Response {
    match logo(ui.ctx(), info) {
        Some(t) => ui.add(logo_image(&t, size + 1.0, true)),
        None => ui.label(egui::RichText::new(info.icon).size(size).color(color)),
    }
}

/// Le module devant le texte d'un bouton ou d'un menu, dans `color` : son image (éteinte si `on` est faux), sinon son
/// glyphe dans le texte.
pub fn with_module(
    ctx: &egui::Context,
    info: &ModuleInfo,
    text: &str,
    color: Color32,
    on: bool,
) -> egui::Atoms<'static> {
    match logo(ctx, info) {
        Some(t) => (logo_image(&t, 15.0, on), egui::RichText::new(text).color(color)).into_atoms(),
        None => egui::RichText::new(format!("{}  {text}", info.icon)).color(color).into_atoms(),
    }
}

/// Dessine l'image d'un module dans `rect` (barre d'activité, onglets).
pub fn paint_logo(painter: &egui::Painter, t: &egui::TextureHandle, rect: egui::Rect, on: bool) {
    let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
    let tint = if on { Color32::WHITE } else { Color32::WHITE.gamma_multiply(0.4) };
    painter.image(t.id(), rect, uv, tint);
}

#[cfg(test)]
mod tests {
    #[test]
    fn icon_has_four_opaque_tiles_and_a_transparent_gap() {
        let size = 64;
        let px = super::render(size);
        let alpha = |x: usize, y: usize| px[(y * size + x) * 4 + 3];
        assert_eq!(alpha(16, 16), 255);
        assert_eq!(alpha(48, 48), 255);
        assert_eq!(alpha(32, 16), 0, "l'interstice entre les cases reste transparent");
    }
}
