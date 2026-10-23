//! Fawkes, module de la suite.
//!
//! Les modules de Fawkes sont compilés à la racine de cette crate (voir `build.rs`) ; l'adaptateur
//! vit dans `suite`.

include!(concat!(env!("OUT_DIR"), "/fawkes_mods.rs"));

/// Repris de `Fawkes/src/main.rs` : lu par la barre de titre et par `winhook`.
pub const APP_TITLE: &str = "Fawkes";

pub mod suite;
