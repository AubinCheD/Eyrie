//! Le terminal suit sa sortie (le bas reste en vue, ce qui arrive y reste collé) tant que l'utilisateur ne l'a pas
//! remonté. Seuls ses gestes en décident : remonter (glisser, lancer, molette), même d'une ligne, arrête aussitôt de
//! suivre, mais pas glisser surtout de côté (zoomé, pour lire une longue ligne) ; ramener la vue en bas (ou « Revenir
//! en bas ») reprend. Ce que la liste fait d'elle-même (la sortie qui grandit ou rapetisse, les vieilles lignes qui
//! tombent de l'historique par le haut, le clavier qui s'ouvre, un autre zoom) n'en décide jamais : la sortie qui
//! arrive pendant qu'on relit ne bouge pas la vue.
//!
//! `TermView` (`ui/term.slint`) demande ici ([`Follower`]), à chaque mouvement de sa liste, si elle suit encore le bas.

use std::time::Duration;

/// Un mouvement de la liste du terminal (pixels logiques).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Move {
    /// Le doigt l'a fait (glissé, molette, barre de défilement). Sinon, la liste a bougé d'elle-même : l'élan d'un
    /// lancer, des lignes tombées de l'historique, une sortie plus courte, une autre hauteur.
    pub user: bool,
    /// De combien la vue est montée, vers l'historique ; négatif : descendue, vers le bas.
    pub up: f32,
    /// De combien elle est allée de côté (zoomé, les lignes dépassent l'écran), dans un sens ou dans l'autre.
    pub side: f32,
    /// Ce qui reste de la sortie sous la vue (0 : le bas est en vue).
    pub gap: f32,
    /// La hauteur d'une ligne.
    pub row: f32,
    /// Toute la sortie tient dans la vue : rien à remonter.
    pub fits: bool,
}

/// La vue est en bas à une ligne près : une ligne arrivée pendant le geste ne l'empêche pas d'y revenir.
const NEAR_ROWS: f32 = 1.0;

/// Un geste du doigt finit quand il ne fait plus bouger la liste pendant ce temps : le mouvement suivant en commence
/// un autre (le doigt levé puis reposé, ou arrêté un moment).
pub const PAUSE: Duration = Duration::from_millis(150);

/// Le geste du doigt en cours sur la liste, d'un mouvement à l'autre : seul le geste entier dit s'il va vers le haut ou
/// de côté. Zoomé, les lignes dépassent l'écran : on les lit en glissant de côté, et la liste suit les deux sens à la
/// fois ; le doigt descend toujours un peu en chemin, et au bout des lignes, la liste ne bouge plus que de cette dérive.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Follower {
    /// Le moment du dernier mouvement du geste (aucun : pas encore de geste).
    last: Option<Duration>,
    /// De combien le geste a fait monter la vue, en tout (ce qu'il l'a fait descendre s'en retire).
    up: f32,
    /// De combien il l'a fait aller de côté, en tout, dans un sens ou dans l'autre.
    side: f32,
}

impl Follower {
    /// La liste suit-elle le bas après ce mouvement, fait à `now` (`follow` : elle le suivait) ?
    pub fn follows(&mut self, follow: bool, m: Move, now: Duration) -> bool {
        if m.user {
            if self.last.is_none_or(|last| now.saturating_sub(last) > PAUSE) {
                *self = Follower::default();
            }
            self.last = Some(now);
            self.up += m.up;
            self.side += m.side.abs();
        }
        if m.fits {
            return true;
        }
        // Le doigt remonte : on relit, aussitôt (à quelques pixels du bas, la sortie suivante l'y ramènerait). Sauf
        // quand son geste va surtout de côté : on lit une longue ligne.
        if m.user && m.up > 0.0 && self.up >= self.side {
            return false;
        }
        // Descendue jusqu'en bas : par le doigt, ou l'élan de son lancer. La liste ne descend jamais d'elle-même : ses
        // décalages (lignes tombées de l'historique, sortie plus courte, clavier refermé) la font monter.
        if m.up < 0.0 && m.gap <= m.row * NEAR_ROWS {
            return true;
        }
        follow
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROW: f32 = 20.0;
    /// Entre deux mouvements du doigt pendant un glissé.
    const STEP: Duration = Duration::from_millis(16);

    /// Un mouvement seul de son geste.
    fn follows(follow: bool, m: Move) -> bool {
        Follower::default().follows(follow, m, Duration::ZERO)
    }

    fn user(up: f32, gap: f32) -> Move {
        Move { user: true, up, side: 0.0, gap, row: ROW, fits: false }
    }

    fn by_itself(up: f32, gap: f32) -> Move {
        Move { user: false, up, side: 0.0, gap, row: ROW, fits: false }
    }

    /// Remonter, même de quelques pixels à partir du bas, arrête de suivre : la sortie qui arrive ne ramène plus la vue.
    #[test]
    fn going_up_stops_following_at_once_even_a_few_pixels_from_the_bottom() {
        assert!(!follows(true, user(4.0, 4.0)));
        assert!(!follows(true, user(30.0, 2.0 * ROW)), "plus loin aussi");
        assert!(!follows(false, user(30.0, 40.0 * ROW)), "déjà remontée : elle le reste");
    }

    /// Zoomé, les lignes dépassent l'écran : on les lit en glissant de côté. Le doigt descend un peu en chemin (la
    /// liste suit les deux sens à la fois) : elle suit toujours le bas. Un glissé surtout vers le haut, lui, l'arrête.
    #[test]
    fn a_sideways_pan_drifting_up_a_little_keeps_following() {
        assert!(follows(true, Move { side: -200.0, ..user(6.0, 6.0) }), "de côté, 6 px de dérive");
        assert!(follows(true, Move { side: 9.0, ..user(3.0, 3.0) }), "le premier pas, après la zone morte");
        assert!(!follows(true, Move { side: -20.0, ..user(30.0, 30.0) }), "surtout vers le haut");
        assert!(!follows(true, Move { side: 12.0, ..user(12.0, 12.0) }), "en biais : on relit");
    }

    /// Le glissé de côté arrive au bout des lignes : la liste ne va plus de côté, elle ne bouge plus que de la dérive
    /// du doigt vers le bas. C'est toujours le même geste, de côté : elle suit le bas. Le doigt levé, un glissé vers le
    /// haut est un autre geste : il arrête de suivre aussitôt.
    #[test]
    fn a_sideways_pan_past_the_end_of_the_lines_keeps_following() {
        let mut f = Follower::default();
        let mut now = Duration::ZERO;
        for k in 0..40 {
            now += STEP;
            let side = if k < 10 { -20.0 } else { 0.0 };
            assert!(f.follows(true, Move { side, ..user(0.6, 0.6) }, now), "mouvement {k}");
            // Entre deux mouvements, la sortie qui arrive : la liste se recolle en bas d'elle-même.
            assert!(f.follows(true, by_itself(-0.6, 0.0), now + STEP / 2));
        }
        now += PAUSE + STEP;
        assert!(!f.follows(true, user(3.0, 3.0), now), "un autre geste, vers le haut");
    }

    /// Le geste qui tourne vers le haut sans que le doigt se lève arrête de suivre dès qu'il est allé plus haut que de
    /// côté.
    #[test]
    fn a_pan_turning_up_stops_following_once_more_up_than_sideways() {
        let mut f = Follower::default();
        let mut now = Duration::ZERO;
        for _ in 0..5 {
            now += STEP;
            assert!(f.follows(true, Move { side: 20.0, ..user(0.0, 0.0) }, now), "100 px de côté");
        }
        for up in [30, 60, 90] {
            now += STEP;
            assert!(f.follows(true, user(30.0, 30.0), now), "{up} px vers le haut : pas encore");
        }
        now += STEP;
        assert!(!f.follows(true, user(30.0, 30.0), now), "120 px vers le haut");
    }

    /// La liste qui bouge d'elle-même n'en décide pas : des lignes qui tombent de l'historique la décalent vers le
    /// haut, une sortie plus courte la ramène à son nouveau bas ; elle suit, ou non, comme avant.
    #[test]
    fn the_list_moving_by_itself_decides_nothing() {
        assert!(follows(true, by_itself(3.0 * ROW, 3.0 * ROW)), "lignes tombées de l'historique, en suivant");
        assert!(!follows(false, by_itself(3.0 * ROW, 12.0 * ROW)), "les mêmes, pendant qu'on relit");
        assert!(!follows(false, by_itself(5.0 * ROW, 0.0)), "une sortie plus courte : la vue posée sur son bas");
        assert!(!follows(false, by_itself(0.0, 0.0)), "une autre hauteur, la vue au bas");
        assert!(follows(true, by_itself(0.0, 4.0 * ROW)), "la sortie qui grandit, en suivant");
        assert!(!follows(false, by_itself(0.0, 4.0 * ROW)), "la même, pendant qu'on relit");
    }

    /// Ramener la vue en bas (glisser, molette, ou l'élan du lancer qui y arrive) reprend le suivi ; s'arrêter avant,
    /// non.
    #[test]
    fn bringing_the_view_back_to_the_bottom_follows_again() {
        assert!(follows(false, user(-60.0, 0.0)), "glissée jusqu'en bas");
        assert!(follows(false, user(-60.0, ROW * 0.5)), "à une ligne près");
        assert!(follows(false, by_itself(-35.0, 0.0)), "l'élan du lancer arrive en bas");
        assert!(!follows(false, user(-60.0, 3.0 * ROW)), "trois lignes avant le bas");
        assert!(!follows(false, by_itself(-35.0, 6.0 * ROW)), "l'élan s'arrête avant");
    }

    /// Toute la sortie tient dans la vue : il n'y a rien à relire, la liste suit.
    #[test]
    fn a_short_output_is_followed() {
        let fits = Move { fits: true, ..by_itself(4.0 * ROW, 0.0) };
        assert!(follows(false, fits));
        assert!(follows(true, Move { fits: true, ..user(0.0, 0.0) }));
    }
}
