//! L'icône d'Eyrie, « la fleur » (proposition D24) : le nid vu d'en haut, anneau tressé, sept œufs aux
//! couleurs des modules en couronne autour d'un cœur doré. Le nid remplit l'icône, bord à bord, sans tuile
//! autour : sur une barre des tâches sombre, la tuile sombre ne se voyait pas et le nid paraissait petit à côté
//! des autres icônes. Dessinée sur une grille de 64 unités, sans dépendance : `build.rs` s'en sert aussi pour
//! l'icône de l'exécutable.

use std::f32::consts::TAU;

/// Centre de la grille.
const C: f32 = 32.0;

/// Agrandissement autour du centre : le brin extérieur de l'anneau (rayon 25,9 sur la grille) touche le bord.
const ZOOM: f32 = 31.75 / 25.9;

/// Couleurs des œufs, du haut puis dans le sens des aiguilles d'une montre : Fawkes, DragonTree, Owlery,
/// Patronus, Menagerie, Grimoire, et le bleu de la suite.
const EGGS: [u32; 7] = [0xf26d3b, 0xe08a5c, 0x3aa8d8, 0x9fc5ff, 0x4cb872, 0xb07ce0, 0x5b8def];

/// Une forme pleine et opaque.
enum Shape {
    /// Disque centré sur la grille.
    Disc { r: f32 },
    /// Cercle tracé, centré sur la grille ; pointillés `(trait, blanc)` mesurés sur le cercle, depuis
    /// 3 heures dans le sens des aiguilles d'une montre, comme `stroke-dasharray` en SVG.
    Ring { r: f32, width: f32, dash: Option<(f32, f32)> },
    /// Ellipse : centre, demi-axes, rotation en degrés dans le sens des aiguilles d'une montre.
    Egg { cx: f32, cy: f32, rx: f32, ry: f32, rot: f32 },
}

impl Shape {
    fn covers(&self, x: f32, y: f32) -> bool {
        match *self {
            Shape::Disc { r } => (x - C).hypot(y - C) <= r,
            Shape::Ring { r, width, dash } => {
                let (dx, dy) = (x - C, y - C);
                if (dx.hypot(dy) - r).abs() > width / 2.0 {
                    return false;
                }
                dash.is_none_or(|(on, off)| (dy.atan2(dx).rem_euclid(TAU) * r).rem_euclid(on + off) < on)
            }
            Shape::Egg { cx, cy, rx, ry, rot } => {
                let (s, c) = rot.to_radians().sin_cos();
                let (dx, dy) = (x - cx, y - cy);
                // Le point dans le repère de l'œuf (rotation inverse).
                let (u, v) = (dx * c + dy * s, dy * c - dx * s);
                (u / rx).powi(2) + (v / ry).powi(2) <= 1.0
            }
        }
    }
}

fn rgb(v: u32) -> [u8; 3] {
    [(v >> 16) as u8, (v >> 8) as u8, v as u8]
}

/// Les formes, du fond vers le dessus.
fn layers() -> Vec<(Shape, [u8; 3])> {
    let mut v = vec![
        // L'anneau tressé : fond, couronne, trois brins en pointillés.
        (Shape::Disc { r: 20.5 }, rgb(0x4a3220)),
        (Shape::Ring { r: 22.0, width: 7.0, dash: None }, rgb(0x8a5a32)),
        (Shape::Ring { r: 25.0, width: 1.8, dash: Some((9.0, 4.0)) }, rgb(0xa8713f)),
        (Shape::Ring { r: 22.0, width: 2.0, dash: Some((7.0, 5.0)) }, rgb(0xc4884f)),
        (Shape::Ring { r: 19.0, width: 1.6, dash: Some((5.0, 6.0)) }, rgb(0xe0ad73)),
    ];
    // Les œufs, la pointe vers le cœur.
    for (i, &color) in EGGS.iter().enumerate() {
        let a = -90.0 + 360.0 * i as f32 / EGGS.len() as f32;
        let (s, c) = a.to_radians().sin_cos();
        v.push((Shape::Egg { cx: C + 9.0 * c, cy: C + 9.0 * s, rx: 3.6, ry: 5.2, rot: a + 90.0 }, rgb(color)));
    }
    v.push((Shape::Disc { r: 3.4 }, rgb(0xe8b84a)));
    v
}

/// L'icône en RGBA (non prémultiplié), `size` × `size` pixels.
pub fn render(size: usize) -> Vec<u8> {
    // Échantillons par pixel sur chaque axe : de quoi lisser les bords.
    const N: usize = 6;
    let layers = layers();
    let scale = 64.0 / size as f32;
    let mut rgba = vec![0u8; size * size * 4];
    for py in 0..size {
        for px in 0..size {
            let (mut sum, mut hits) = ([0u32; 3], 0u32);
            for sy in 0..N {
                for sx in 0..N {
                    let x = C + ((px as f32 + (sx as f32 + 0.5) / N as f32) * scale - C) / ZOOM;
                    let y = C + ((py as f32 + (sy as f32 + 0.5) / N as f32) * scale - C) / ZOOM;
                    // Toutes les formes sont opaques : celle du dessus l'emporte.
                    if let Some((_, c)) = layers.iter().rev().find(|(s, _)| s.covers(x, y)) {
                        for k in 0..3 {
                            sum[k] += u32::from(c[k]);
                        }
                        hits += 1;
                    }
                }
            }
            // Aucune forme touchée : le pixel reste transparent.
            let i = (py * size + px) * 4;
            for k in 0..3 {
                rgba[i + k] = sum[k].checked_div(hits).unwrap_or(0) as u8;
            }
            rgba[i + 3] = (hits * 255 / (N * N) as u32) as u8;
        }
    }
    rgba
}
