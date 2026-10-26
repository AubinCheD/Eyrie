//! SDK de la suite : ce que partagent l'hôte et les modules.
//!
//! - [`module`] : l'interface qu'un module implémente (identité, vues, logique, affichage, badge,
//!   commandes, demandes des autres modules, fermeture) et sa fabrique ;
//! - [`host`] : les services que l'hôte rend aux modules (thème, langue, notifications, bus
//!   d'événements, contexte partagé, « montre-moi », comptes) ;
//! - [`embed`] : l'adaptateur qui fait d'une application eframe existante un module, sans toucher
//!   à son code ;
//! - [`theme`], [`fonts`], [`i18n`], [`shortcut`] : le design commun ;
//! - [`state`], [`instance`] : fichiers d'état robustes et instance unique.

pub mod embed;
pub mod fonts;
pub mod host;
pub mod i18n;
pub mod instance;
pub mod module;
pub mod shortcut;
pub mod state;
pub mod theme;

pub use eframe;
pub use egui;

pub use host::{Event, Host, Notification, Request, Tone};
pub use module::{
    Badge, BuildCx, CloseCheck, CommandInfo, Cx, DataMode, Module, ModuleFactory, ModuleInfo, ModuleStatus, Origin,
    StatusText, Unavailable, ViewInfo,
};
