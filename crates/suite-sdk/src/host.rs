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

/// Un compte connecté de la suite, sans son jeton.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AccountInfo {
    pub id: String,
    /// `github`, `gitlab`, `azure`, `claude`.
    pub forge: String,
    pub host: String,
    pub username: String,
    /// Modules qui ont le droit de lire ce compte (partage compte par compte).
    #[serde(default)]
    pub shared_with: Vec<String>,
}

type TokenReader = dyn Fn(&str) -> Option<String> + Send + Sync;

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
}

struct Shared {
    /// Les cases suivent le contexte partagé (réglage de la suite).
    follow_context: bool,
    /// Modules construits (pour un module qui veut savoir si un autre est là).
    running: Vec<String>,
    theme: ThemeChoice,
    palette: &'static Palette,
    context: Option<SharedContext>,
    accounts: Vec<AccountInfo>,
    outbox: Outbox,
}

struct Inner {
    ctx: egui::Context,
    suite_dir: PathBuf,
    shared: Mutex<Shared>,
    token_reader: Mutex<Option<Arc<TokenReader>>>,
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
                    accounts: Vec::new(),
                    outbox: Outbox::default(),
                }),
                token_reader: Mutex::new(None),
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

    /// Dossier de la suite (`%APPDATA%\SuperApp`, ou `SUPERAPP_DATA_DIR`).
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

    /// Fait clignoter la suite dans la barre des tâches, pour ce module.
    pub fn attention(&self, module: &str) {
        self.shared(|s| s.outbox.attention.push(module.to_string()));
        self.repaint();
    }

    /// Réservé à l'hôte.
    pub fn take_outbox(&self) -> Outbox {
        self.shared(|s| std::mem::take(&mut s.outbox))
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

    // --- Comptes ---

    /// Les comptes que ce module a le droit de lire.
    pub fn accounts_for(&self, module: &str) -> Vec<AccountInfo> {
        self.shared(|s| s.accounts.iter().filter(|a| a.shared_with.iter().any(|m| m == module)).cloned().collect())
    }

    /// Le jeton d'un compte, si ce module y a droit.
    pub fn token(&self, module: &str, account: &str) -> Option<String> {
        let allowed = self.accounts_for(module).iter().any(|a| a.id == account);
        let reader = self.inner.token_reader.lock().unwrap_or_else(|e| e.into_inner()).clone();
        if allowed { reader.and_then(|r| r(account)) } else { None }
    }

    /// Réservé à l'hôte : la liste des comptes et la lecture du coffre.
    pub fn set_accounts(&self, accounts: Vec<AccountInfo>, reader: Arc<TokenReader>) {
        self.shared(|s| s.accounts = accounts);
        *self.inner.token_reader.lock().unwrap_or_else(|e| e.into_inner()) = Some(reader);
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
    fn tokens_are_shared_account_by_account() {
        let h = host();
        let acc = AccountInfo {
            id: "a1".into(),
            forge: "github".into(),
            host: "github.com".into(),
            username: "moi".into(),
            shared_with: vec!["owlery".into()],
        };
        h.set_accounts(vec![acc], Arc::new(|id: &str| Some(format!("jeton-{id}"))));
        assert_eq!(h.token("owlery", "a1").as_deref(), Some("jeton-a1"));
        assert_eq!(h.token("patronus", "a1"), None);
    }
}
