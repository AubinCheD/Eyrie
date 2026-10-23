//! Icône de la suite, dessinée : quatre cases de couleurs, comme les modules qu'elle réunit.

use eframe::egui;

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
            let col = if fx < half { 0 } else if fx > half + gap { 1 } else { continue };
            let row = if fy < half { 0 } else if fy > half + gap { 1 } else { continue };
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
