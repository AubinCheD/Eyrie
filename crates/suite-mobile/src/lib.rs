//! Le protocole entre Eyrie (le PC) et son application mobile.
//!
//! Le PC est la seule source de vérité : le téléphone montre ce qu'il reçoit et demande des actions. Tout passe par
//! un canal Noise (`XX_25519_ChaChaPoly_BLAKE2s`) entre deux clés appairées, au-dessus d'une connexion TCP que le
//! compagnon Tailscale d'Eyrie relaie. Voir `docs/mobile.md`.
//!
//! - [`ui`] : l'écran générique qu'un module décrit et que le téléphone dessine ;
//! - [`term`] : un terminal suivi en direct (vue, différences, saisie) ;
//! - [`msg`] : les messages dans les deux sens ;
//! - [`pairing`] : ticket du QR code, code à saisir, preuves mutuelles, code de vérification ;
//! - [`channel`] : poignée de main et trames chiffrées ;
//! - [`client`] : la connexion côté téléphone (application, essais d'Eyrie) ;
//! - [`veille`] : demander au veilleur du PC de démarrer Eyrie quand elle est fermée.

pub mod channel;
pub mod client;
pub mod msg;
pub mod pairing;
pub mod term;
pub mod ui;
pub mod veille;

pub use msg::{PROTOCOL, ToPc, ToPhone};

/// Port d'Eyrie sur le tailnet.
pub const PORT: u16 = 4747;
