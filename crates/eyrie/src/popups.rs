//! Les fenêtres flottantes (Réglages d'Eyrie, Cloner de DragonTree…) suivent la fenêtre qui les montre quand elle
//! change de taille (agrandie, restaurée, bord tiré) : chacune garde sa place relative, centrée si elle l'était,
//! collée à un bord si elle y était. Sans cela, egui les laisse à leur position absolue (une fenêtre centrée finit
//! en haut à gauche d'une fenêtre agrandie) et ne les rabat qu'au bord en cas de réduction.

use eframe::egui::{self, Pos2, Rect};

/// La zone de la fenêtre (un viewport) à l'image précédente.
#[derive(Default)]
pub struct Follow {
    last: Option<Rect>,
}

impl Follow {
    /// Au début de l'image de cette fenêtre, avant que la moindre fenêtre flottante soit montrée.
    pub fn update(&mut self, ctx: &egui::Context) {
        let now = ctx.content_rect();
        // Fenêtre réduite dans la barre des tâches : rien à suivre, la taille d'avant reviendra.
        if now.width() < 50.0 || now.height() < 50.0 {
            return;
        }
        let Some(before) = self.last.replace(now) else { return };
        if before == now {
            return;
        }
        let layers: Vec<egui::LayerId> = ctx
            .memory(|m| m.areas().visible_layer_ids().into_iter().filter(|l| l.order == egui::Order::Middle).collect());
        for layer in layers {
            let Some(mut state) = egui::AreaState::load(ctx, layer.id) else { continue };
            let Some(size) = state.size else { continue };
            state.set_left_top_pos(follow(Rect::from_min_size(state.left_top_pos(), size), before, now));
            ctx.memory_mut(|m| m.areas_mut().set_state(layer, state));
        }
    }
}

/// Coin haut gauche de `rect`, placé dans `before`, une fois la zone devenue `now` : sur chaque axe, la même part de
/// l'espace libre avant lui (0 collé au début, ½ centré, 1 collé à la fin).
pub fn follow(rect: Rect, before: Rect, now: Rect) -> Pos2 {
    let axis = |pos: f32, size: f32, start: f32, room: f32, new_start: f32, new_room: f32| {
        let free = room - size;
        let t = if free > 0.5 { ((pos - start) / free).clamp(0.0, 1.0) } else { 0.5 };
        new_start + t * (new_room - size).max(0.0)
    };
    egui::pos2(
        axis(rect.left(), rect.width(), before.left(), before.width(), now.left(), now.width()),
        axis(rect.top(), rect.height(), before.top(), before.height(), now.top(), now.height()),
    )
}

#[cfg(test)]
mod tests {
    use super::follow;
    use eframe::egui::{Rect, pos2, vec2};

    fn screen(w: f32, h: f32) -> Rect {
        Rect::from_min_size(pos2(0.0, 0.0), vec2(w, h))
    }

    #[test]
    fn a_centered_window_stays_centered() {
        let rect = Rect::from_center_size(pos2(500.0, 400.0), vec2(600.0, 400.0));
        let at = follow(rect, screen(1000.0, 800.0), screen(1920.0, 1040.0));
        assert_eq!(Rect::from_min_size(at, rect.size()).center(), pos2(960.0, 520.0));
    }

    #[test]
    fn a_window_against_an_edge_stays_against_it() {
        let rect = Rect::from_min_size(pos2(700.0, 0.0), vec2(300.0, 200.0));
        let at = follow(rect, screen(1000.0, 800.0), screen(1600.0, 900.0));
        assert_eq!(at, pos2(1300.0, 0.0), "collée en haut à droite");
    }

    #[test]
    fn a_window_larger_than_the_new_room_starts_at_its_corner() {
        let rect = Rect::from_min_size(pos2(200.0, 100.0), vec2(800.0, 600.0));
        assert_eq!(follow(rect, screen(1600.0, 900.0), screen(700.0, 500.0)), pos2(0.0, 0.0));
    }
}
