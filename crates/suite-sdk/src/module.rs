//! L'interface d'un module de la suite.
//!
//! Un module est une bibliothèque compilée dans la suite et activée à l'exécution (pas de DLL : Rust
//! n'a pas d'ABI stable, et tous les modules partagent la même version d'egui). L'hôte le construit
//! au démarrage par sa [`ModuleFactory`], appelle [`Module::logic`] à chaque image même s'il n'est
//! affiché nulle part, et [`Module::ui`] pour chacune de ses vues posées dans une case.

use std::path::PathBuf;

use eframe::egui;

use crate::host::{Event, Host, Request, Tone};
use crate::mobile;
use crate::shortcut::Shortcut;

/// D'où vient le module.
#[derive(Clone, Debug, PartialEq)]
pub enum Origin {
    /// Écrit pour la suite (interface [`Module`] implémentée directement).
    Native,
    /// Application autonome existante, embarquée sans changer son code (voir [`crate::embed`]).
    Embedded {
        /// Nom de son exécutable autonome (`fawkes.exe`).
        exe: &'static str,
        /// Dépôt voisin dont viennent ses sources (`../Fawkes`).
        repo: &'static str,
    },
}

/// Identité d'un module, connue avant de le construire.
#[derive(Clone, Debug)]
pub struct ModuleInfo {
    /// Identifiant stable (`fawkes`, `dragontree`…) : clés de disposition, de réglages, de raccourcis.
    pub id: &'static str,
    /// Nom affiché.
    pub name: &'static str,
    /// Glyphe de la barre d'activité (couvert par les polices de la suite).
    pub icon: &'static str,
    /// Image du module, à la place du glyphe (barre d'activité, onglets, menus) : pixels RGBA d'un carré de la taille
    /// demandée, comme l'icône de son exécutable.
    pub logo: Option<fn(u32) -> Vec<u8>>,
    /// Couleur du module (barre d'activité, onglets).
    pub color: egui::Color32,
    /// Une ligne : ce que fait le module.
    pub summary: &'static str,
    pub origin: Origin,
}

/// Une vue d'un module : ce qu'on pose dans une case. Un module a au moins la vue principale.
#[derive(Clone, Debug, PartialEq)]
pub struct ViewInfo {
    pub id: String,
    pub title: String,
}

impl ViewInfo {
    pub const MAIN: &'static str = "principal";

    pub fn new(id: impl Into<String>, title: impl Into<String>) -> Self {
        ViewInfo { id: id.into(), title: title.into() }
    }

    pub fn main(title: impl Into<String>) -> Self {
        ViewInfo::new(Self::MAIN, title)
    }
}

/// Pastille de la barre d'activité et des onglets.
#[derive(Clone, Debug, PartialEq)]
pub struct Badge {
    /// Compteur ; `None` pour un simple point.
    pub count: Option<u32>,
    pub tone: Tone,
    /// Ce que signifie la pastille (« 3 PR à relire »).
    pub tip: String,
}

impl Badge {
    pub fn count(count: u32, tone: Tone, tip: impl Into<String>) -> Self {
        Badge { count: Some(count), tone, tip: tip.into() }
    }
}

/// Ce que le module affiche de lui-même hors de sa case.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ModuleStatus {
    /// Titre courant (« DragonTree — Ozmo »), à la place de `ViewportCommand::Title`.
    pub title: Option<String>,
    pub badge: Option<Badge>,
    /// Ce qui attend l'utilisateur, pour la vue d'ensemble.
    pub waiting: Vec<String>,
    /// Texte court affiché en permanence dans la barre d'état de l'hôte (en bas de la fenêtre).
    pub status_line: Option<StatusText>,
}

/// Un texte de la barre d'état de l'hôte : quelques mots, leur ton (couleur) et le détail en infobulle.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StatusText {
    pub text: String,
    pub tone: Tone,
    pub tip: String,
}

/// Une commande du module, pour la palette et le registre des raccourcis.
#[derive(Clone, Debug)]
pub struct CommandInfo {
    /// Identifiant stable dans le module.
    pub id: String,
    pub title: String,
    /// Raccourci par défaut, actif quand le module a le clavier.
    pub shortcut: Option<Shortcut>,
}

/// Réponse à « peut-on fermer ? ».
#[derive(Clone, Debug, PartialEq)]
pub enum CloseCheck {
    Ready,
    /// Travail en cours ou modifications non enregistrées : la raison, montrée avant de fermer.
    Busy(String),
}

/// Ce que l'hôte passe à chaque appel d'un module.
pub struct Cx<'a> {
    pub host: &'a Host,
    /// Le cadre d'eframe de la fenêtre principale (les fenêtres détachées sont des viewports
    /// immédiats, dessinés pendant l'image de la fenêtre principale).
    pub frame: &'a mut eframe::Frame,
    /// Le module a le clavier (sa case a reçu le dernier clic).
    pub active: bool,
    /// Au moins une vue du module est affichée.
    pub visible: bool,
}

/// L'interface d'un module.
pub trait Module {
    fn info(&self) -> &ModuleInfo;

    /// Les vues que le module propose ; la première est la vue principale.
    fn views(&self) -> Vec<ViewInfo> {
        vec![ViewInfo::main(self.info().name)]
    }

    /// Tout ce qui n'est pas de l'affichage, appelé à chaque image, même vue masquée.
    fn logic(&mut self, ctx: &egui::Context, cx: &mut Cx);

    /// Dessine la vue `view` dans le `Ui` d'une case.
    fn ui(&mut self, ui: &mut egui::Ui, view: &str, cx: &mut Cx);

    /// Le module a besoin que `ui` soit appelé à chaque image même masqué (travail de fond resté
    /// dans `ui`) : l'hôte l'appelle alors dans un `Ui` invisible, sans clavier.
    fn needs_hidden_ui(&self) -> bool {
        false
    }

    fn status(&self) -> ModuleStatus {
        ModuleStatus::default()
    }

    fn commands(&self) -> Vec<CommandInfo> {
        Vec::new()
    }

    fn run_command(&mut self, _id: &str, _cx: &mut Cx) {}

    /// Page de réglages propres au module dans les réglages de la suite. `false` : aucune.
    fn settings_ui(&mut self, _ui: &mut egui::Ui, _host: &Host) -> bool {
        false
    }

    /// Sa page de réglages n'est plus montrée (fenêtre fermée, autre page) : une saisie en cours y est abandonnée, un
    /// brouillon non enregistré oublié, comme à la fermeture de sa propre fenêtre de réglages.
    fn settings_left(&mut self) {}

    /// Un événement publié par un autre module (ou par l'hôte) sur le bus.
    fn on_event(&mut self, _event: &Event, _cx: &mut Cx) {}

    /// Une demande d'un autre module (le contrat local, en appel Rust dans le même processus).
    fn on_request(&mut self, _request: &Request, _cx: &mut Cx) -> Result<serde_json::Value, String> {
        Err("demande inconnue".into())
    }

    /// Commandes de fenêtre émises par le module pendant son dernier appel (titre, attention…),
    /// relevées par l'hôte qui ne les applique pas à la fenêtre de la suite.
    fn observe_viewport_commands(&mut self, _commands: &[egui::ViewportCommand]) {}

    fn can_close(&mut self) -> CloseCheck {
        CloseCheck::Ready
    }

    fn on_exit(&mut self) {}

    // --- Le téléphone (voir `docs/mobile.md`) ---

    /// L'écran `path` du module sur le téléphone (`""` : son accueil). `None` : pas d'écran mobile, l'hôte en montre
    /// un générique (état, ce qui attend, commandes).
    fn mobile_screen(&mut self, _path: &str, _cx: &mut Cx) -> Option<mobile::ui::Screen> {
        None
    }

    /// Une action d'un écran mobile. L'hôte ne l'appelle que pour une action que le dernier écran `path` envoyé au
    /// téléphone proposait. `Ok(Some(texte))` : un message à montrer au téléphone.
    fn mobile_action(
        &mut self,
        _path: &str,
        _action: &str,
        _input: Option<&str>,
        _cx: &mut Cx,
    ) -> Result<Option<String>, String> {
        Err("action inconnue".into())
    }

    /// Le terminal `id` suivi depuis le téléphone : l'écran et au plus `history` lignes d'historique.
    fn mobile_term(&mut self, _id: &str, _history: usize) -> Option<mobile::term::TermView> {
        None
    }

    /// Une saisie du téléphone dans le terminal `id`.
    fn mobile_term_input(&mut self, _id: &str, _input: &mobile::term::TermInput, _cx: &mut Cx) -> Result<(), String> {
        Err("pas de terminal".into())
    }
}

/// Où un module embarqué range son état dans la suite.
#[derive(Clone, Debug, PartialEq)]
pub enum DataMode {
    /// Le même dossier que son exécutable autonome (décision du 22/10 : ses groupes, tâches,
    /// filtres… sont les mêmes des deux côtés). L'hôte tient alors le verrou de l'exe.
    Shared,
    /// Un dossier d'essai propre à la suite (`<suite>/essai/<module>`), pour essayer sans toucher
    /// aux données de tous les jours.
    Trial(PathBuf),
}

/// Ce que reçoit la fabrique d'un module.
pub struct BuildCx<'a, 'b> {
    pub cc: &'a eframe::CreationContext<'b>,
    pub host: Host,
    pub data: DataMode,
}

/// Pourquoi un module n'a pas pu être construit.
#[derive(Clone, Debug, PartialEq)]
pub enum Unavailable {
    /// Son exécutable autonome tourne déjà sur le même dossier d'état.
    AlreadyRunning {
        exe: &'static str,
    },
    Failed(String),
}

impl std::fmt::Display for Unavailable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Unavailable::AlreadyRunning { exe } => write!(f, "{exe} tourne déjà sur le même dossier d'état"),
            Unavailable::Failed(e) => f.write_str(e),
        }
    }
}

/// Comment construire un module.
pub struct ModuleFactory {
    pub info: ModuleInfo,
    pub build: fn(&BuildCx) -> Result<Box<dyn Module>, Unavailable>,
    /// Polices dont le module a besoin en plus de celles de la suite (icônes…).
    pub fonts: Option<fn(&mut crate::fonts::FontPlan)>,
}
