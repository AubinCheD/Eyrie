//! Les services que l'hôte rend aux modules.
//!
//! [`Host`] se clone et passe d'un fil à l'autre : un module peut notifier ou publier un événement
//! depuis un fil de fond. Les demandes (notifications, événements, « montre-moi »…) sont mises en
//! file et traitées par l'hôte à l'image suivante, jamais pendant l'appel d'un autre module.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use eframe::egui;
use serde::{Deserialize, Serialize};

use crate::i18n::{self, Lang};
use crate::theme::{self, Palette, ThemeChoice};

/// Ton d'une pastille, d'une notification.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Tone {
    #[default]
    Info,
    Ok,
    Warn,
    Danger,
}

impl Tone {
    pub fn color(self, p: &Palette) -> egui::Color32 {
        match self {
            Tone::Info => p.accent,
            Tone::Ok => p.ok,
            Tone::Warn => p.warn,
            Tone::Danger => p.danger,
        }
    }
}

/// Une notification, rangée dans le centre de notifications de la suite.
#[derive(Clone, Debug, PartialEq)]
pub struct Notification {
    /// Module émetteur (`owlery`), ou `suite`.
    pub module: String,
    pub title: String,
    pub body: String,
    pub tone: Tone,
    pub at: SystemTime,
}

impl Notification {
    pub fn new(module: impl Into<String>, title: impl Into<String>, body: impl Into<String>, tone: Tone) -> Self {
        Notification { module: module.into(), title: title.into(), body: body.into(), tone, at: SystemTime::now() }
    }
}

/// Un événement du bus interne (`service_arrete`, `ci_echec`, `pr_ouverte`, `contexte`…), le même
/// vocabulaire que les contrats locaux des modules.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Event {
    pub source: String,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub data: serde_json::Value,
}

/// Une demande d'un module à un autre : le contrat local, en appel Rust dans le même processus.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Request {
    #[serde(rename = "de")]
    pub from: String,
    #[serde(rename = "pour")]
    pub to: String,
    pub demande: String,
    #[serde(default)]
    pub data: serde_json::Value,
}

/// Le contexte partagé : le dépôt (ou dossier) sur lequel l'utilisateur travaille. Les cases qui
/// « suivent le contexte » s'y accordent.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SharedContext {
    pub dir: PathBuf,
    pub label: String,
    /// Qui l'a choisi (`dragontree`, `suite`…).
    pub source: String,
}

/// Ce que l'hôte doit traiter à l'image suivante.
#[derive(Default)]
pub struct Outbox {
    pub notifications: Vec<Notification>,
    pub events: Vec<Event>,
    pub requests: Vec<Request>,
    /// Modules qui demandent à être montrés (case sélectionnée, fenêtre au premier plan).
    pub show: Vec<String>,
    /// Modules qui demandent l'attention (clignotement de la barre des tâches).
    pub attention: Vec<String>,
    /// Page des réglages d'Eyrie à ouvrir (`comptes`…), demandée par un module.
    pub settings: Option<String>,
}

struct Shared {
    /// Les cases suivent le contexte partagé (réglage de la suite).
    follow_context: bool,
    /// Modules construits (pour un module qui veut savoir si un autre est là).
    running: Vec<String>,
    theme: ThemeChoice,
    palette: &'static Palette,
    context: Option<SharedContext>,
    /// Dossier des comptes de toute la suite, donné par l'hôte (voir `accounts_dir`).
    accounts_dir: Option<PathBuf>,
    /// Service du coffre de ces comptes quand il n'est pas celui de la suite (voir `accounts_vault`).
    accounts_vault: Option<String>,
    outbox: Outbox,
}

struct Inner {
    ctx: egui::Context,
    suite_dir: PathBuf,
    shared: Mutex<Shared>,
}

/// Les services de l'hôte.
#[derive(Clone)]
pub struct Host {
    inner: Arc<Inner>,
}

impl Host {
    pub fn new(ctx: egui::Context, suite_dir: PathBuf, theme_choice: ThemeChoice) -> Self {
        let palette = theme::palette(theme::resolve(theme_choice, &ctx));
        Host {
            inner: Arc::new(Inner {
                ctx,
                suite_dir,
                shared: Mutex::new(Shared {
                    follow_context: true,
                    running: Vec::new(),
                    theme: theme_choice,
                    palette,
                    context: None,
                    accounts_dir: None,
                    accounts_vault: None,
                    outbox: Outbox::default(),
                }),
            }),
        }
    }

    fn shared<R>(&self, f: impl FnOnce(&mut Shared) -> R) -> R {
        // Un module qui panique en tenant le verrou ne doit pas priver les autres des services.
        let mut guard = self.inner.shared.lock().unwrap_or_else(|e| e.into_inner());
        f(&mut guard)
    }

    pub fn ctx(&self) -> &egui::Context {
        &self.inner.ctx
    }

    /// Réveille la suite (toutes ses fenêtres : les fenêtres détachées sont dessinées avec la
    /// fenêtre principale).
    pub fn repaint(&self) {
        self.inner.ctx.request_repaint();
    }

    /// Le module `id` tourne-t-il dans la suite ?
    pub fn is_running(&self, id: &str) -> bool {
        self.shared(|s| s.running.iter().any(|m| m == id))
    }

    /// Réservé à l'hôte.
    pub fn set_running(&self, ids: Vec<String>) {
        self.shared(|s| s.running = ids);
    }

    // --- Thème et langue ---

    /// La palette du thème de la suite.
    pub fn palette(&self) -> &'static Palette {
        self.shared(|s| s.palette)
    }

    pub fn theme_choice(&self) -> ThemeChoice {
        self.shared(|s| s.theme)
    }

    /// Réservé à l'hôte : le thème choisi dans les réglages de la suite (ou celui de Windows).
    pub fn set_theme(&self, choice: ThemeChoice) {
        let palette = theme::palette(theme::resolve(choice, &self.inner.ctx));
        self.shared(|s| {
            s.theme = choice;
            s.palette = palette;
        });
    }

    pub fn lang(&self) -> Lang {
        i18n::lang()
    }

    // --- Dossiers ---

    /// Dossier d'Eyrie (`%APPDATA%\EyrieSuite\Eyrie`, ou `EYRIE_DATA_DIR`).
    pub fn suite_dir(&self) -> &Path {
        &self.inner.suite_dir
    }

    /// Dossier de données d'un module natif.
    pub fn module_dir(&self, module: &str) -> PathBuf {
        self.inner.suite_dir.join("modules").join(module)
    }

    // --- Ce que l'hôte traite à l'image suivante ---

    pub fn notify(&self, n: Notification) {
        self.shared(|s| s.outbox.notifications.push(n));
        self.repaint();
    }

    pub fn publish(&self, event: Event) {
        self.shared(|s| s.outbox.events.push(event));
        self.repaint();
    }

    pub fn send(&self, request: Request) {
        self.shared(|s| s.outbox.requests.push(request));
        self.repaint();
    }

    /// « Montre-moi » : sélectionne la case du module et ramène sa fenêtre au premier plan.
    pub fn show(&self, module: &str) {
        self.shared(|s| s.outbox.show.push(module.to_string()));
        self.repaint();
    }

    /// Ouvre une page des réglages d'Eyrie (`comptes`) : un module intégré y renvoie au lieu de montrer ses
    /// propres réglages de ce qu'Eyrie gère pour toute la suite.
    pub fn open_settings(&self, page: &str) {
        self.shared(|s| s.outbox.settings = Some(page.to_string()));
        self.repaint();
    }

    /// Fait clignoter la suite dans la barre des tâches, pour ce module.
    pub fn attention(&self, module: &str) {
        self.shared(|s| s.outbox.attention.push(module.to_string()));
        self.repaint();
    }

    /// Réservé à l'hôte.
    pub fn take_outbox(&self) -> Outbox {
        self.shared(|s| std::mem::take(&mut s.outbox))
    }

    // --- Comptes ---

    /// Dossier des comptes de toute la suite (`suite-accounts`) : les modules qui lisent les comptes le reçoivent
    /// ainsi, et non par une variable d'environnement que leurs processus enfants hériteraient.
    pub fn accounts_dir(&self) -> Option<PathBuf> {
        self.shared(|s| s.accounts_dir.clone())
    }

    /// Réservé à l'hôte.
    pub fn set_accounts_dir(&self, dir: PathBuf) {
        self.shared(|s| s.accounts_dir = Some(dir));
    }

    /// Service du coffre de Windows où sont les jetons de ces comptes, quand ce n'est pas celui de la suite
    /// (« EyrieSuite ») : un hôte lancé sur un profil isolé (essais) a son propre coffre, et ses modules ne doivent
    /// jamais lire ni effacer les vrais jetons. `None` : le coffre de la suite.
    pub fn accounts_vault(&self) -> Option<String> {
        self.shared(|s| s.accounts_vault.clone())
    }

    /// Réservé à l'hôte.
    pub fn set_accounts_vault(&self, service: String) {
        self.shared(|s| s.accounts_vault = Some(service));
    }

    // --- Contexte partagé ---

    /// Les modules doivent-ils suivre le contexte partagé (réglage de la suite) ?
    pub fn follows_context(&self) -> bool {
        self.shared(|s| s.follow_context)
    }

    /// Réservé à l'hôte.
    pub fn set_follows_context(&self, yes: bool) {
        self.shared(|s| s.follow_context = yes);
    }

    pub fn context(&self) -> Option<SharedContext> {
        self.shared(|s| s.context.clone())
    }

    /// Change le contexte partagé et le publie (`contexte`) à tous les modules.
    pub fn set_context(&self, context: Option<SharedContext>) {
        let changed = self.shared(|s| {
            let changed = s.context != context;
            s.context = context.clone();
            changed
        });
        if changed {
            let source = context.as_ref().map_or("suite".to_string(), |c| c.source.clone());
            let data = serde_json::to_value(&context).unwrap_or_default();
            self.publish(Event { source, kind: "contexte".into(), data });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host() -> Host {
        Host::new(egui::Context::default(), std::env::temp_dir(), ThemeChoice::Graphite)
    }

    #[test]
    fn outbox_is_emptied_once() {
        let h = host();
        h.notify(Notification::new("owlery", "PR", "à relire", Tone::Info));
        h.show("owlery");
        let out = h.take_outbox();
        assert_eq!(out.notifications.len(), 1);
        assert_eq!(out.show, vec!["owlery".to_string()]);
        assert!(h.take_outbox().notifications.is_empty());
    }

    #[test]
    fn context_change_is_published_once() {
        let h = host();
        let c = SharedContext { dir: "C:/dev/ozmo".into(), label: "ozmo".into(), source: "suite".into() };
        h.set_context(Some(c.clone()));
        h.set_context(Some(c));
        let events = h.take_outbox().events;
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, "contexte");
    }

    #[test]
    fn the_host_gives_the_accounts_folder_to_its_modules() {
        let h = host();
        assert_eq!(h.accounts_dir(), None, "pas d'hôte qui le donne : chaque module choisit");
        h.set_accounts_dir(PathBuf::from(r"C:\Temp\suite"));
        assert_eq!(h.accounts_dir().as_deref(), Some(Path::new(r"C:\Temp\suite")));
        assert_eq!(h.accounts_vault(), None, "le coffre de la suite");
        h.set_accounts_vault("EyrieSuite:0123".into());
        assert_eq!(h.accounts_vault().as_deref(), Some("EyrieSuite:0123"));
    }

    #[test]
    fn open_settings_reaches_the_outbox() {
        let h = host();
        h.open_settings("comptes");
        assert_eq!(h.take_outbox().settings.as_deref(), Some("comptes"));
        assert_eq!(h.take_outbox().settings, None, "une seule fois");
    }
}
