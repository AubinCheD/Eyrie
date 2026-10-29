//! Le terminal suit sa sortie (le bas reste en vue, ce qui arrive y reste collé) tant que l'utilisateur ne l'a pas
//! remonté. Seuls ses gestes en décident : remonter (glisser, lancer, molette), même d'une ligne, arrête aussitôt de
//! suivre ; ramener la vue en bas (ou « Revenir en bas ») reprend. Ce que la liste fait d'elle-même (la sortie qui
//! grandit ou rapetisse, les vieilles lignes qui tombent de l'historique par le haut, le clavier qui s'ouvre, un autre
//! zoom) n'en décide jamais : la sortie qui arrive pendant qu'on relit ne bouge pas la vue.
//!
//! Les lignes du terminal se replient à la largeur de l'écran (`termfit.rs`) : la liste ne va jamais de côté.
//!
//! `TermView` (`ui/term.slint`) demande ici ([`follows`]), à chaque mouvement de sa liste, si elle suit encore le bas.

/// Un mouvement de la liste du terminal (pixels logiques).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Move {
    /// Le doigt l'a fait (glissé, molette, barre de défilement). Sinon, la liste a bougé d'elle-même : l'élan d'un
    /// lancer, des lignes tombées de l'historique, une sortie plus courte, une autre hauteur.
    pub user: bool,
    /// De combien la vue est montée, vers l'historique ; négatif : descendue, vers le bas.
    pub up: f32,
    /// Ce qui reste de la sortie sous la vue (0 : le bas est en vue).
    pub gap: f32,
    /// La hauteur d'une ligne.
    pub row: f32,
    /// Toute la sortie tient dans la vue : rien à remonter.
    pub fits: bool,
}

/// La vue est en bas à une ligne près : une ligne arrivée pendant le geste ne l'empêche pas d'y revenir.
const NEAR_ROWS: f32 = 1.0;

/// La liste suit-elle le bas après ce mouvement (`follow` : elle le suivait) ?
pub fn follows(follow: bool, m: Move) -> bool {
    if m.fits {
        return true;
    }
    // Le doigt remonte : on relit, aussitôt (à quelques pixels du bas, la sortie suivante l'y ramènerait).
    if m.user && m.up > 0.0 {
        return false;
    }
    // Descendue jusqu'en bas : par le doigt, ou l'élan de son lancer. La liste ne descend jamais d'elle-même : ses
    // décalages (lignes tombées de l'historique, sortie plus courte, clavier refermé) la font monter.
    if m.up < 0.0 && m.gap <= m.row * NEAR_ROWS {
        return true;
    }
    follow
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROW: f32 = 20.0;

    fn user(up: f32, gap: f32) -> Move {
        Move { user: true, up, gap, row: ROW, fits: false }
    }

    fn by_itself(up: f32, gap: f32) -> Move {
        Move { user: false, up, gap, row: ROW, fits: false }
    }

    /// Remonter, même de quelques pixels à partir du bas, arrête de suivre : la sortie qui arrive ne ramène plus la vue.
    #[test]
    fn going_up_stops_following_at_once_even_a_few_pixels_from_the_bottom() {
        assert!(!follows(true, user(4.0, 4.0)));
        assert!(!follows(true, user(30.0, 2.0 * ROW)), "plus loin aussi");
        assert!(!follows(false, user(30.0, 40.0 * ROW)), "déjà remontée : elle le reste");
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
