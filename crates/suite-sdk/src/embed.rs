//! Faire d'une application eframe existante un module, sans toucher à son code.
//!
//! Les six modules dessinent déjà leurs panneaux dans le `Ui` reçu (en egui 0.37, `Panel::show(ui)`
//! est rattaché au `Ui` parent) : leur `eframe::App` se pose tel quel dans une case. Ce qui déborde
//! de la case (style global, polices, clavier lu sur toute la fenêtre, commandes de fenêtre, focus)
//! est contenu par l'hôte autour de chaque appel (`superapp/src/sandbox.rs`) ; cet adaptateur ne fait
//! que relier l'application à l'interface [`Module`] :
//! - `logic` et `ui` d'eframe ;
//! - le titre que l'application donne à sa fenêtre devient le titre et la pastille du module ;
//! - ses raccourcis connus deviennent des commandes de la palette de la suite : les exécuter, c'est
//!   lui envoyer la frappe ;
//! - ce qui doit vivre autant que le module (verrou d'instance, serveur de tube) est gardé ici.

use std::any::Any;

use eframe::egui;

use crate::host::{Host, Request};
use crate::module::{Badge, CloseCheck, CommandInfo, Cx, Module, ModuleInfo, ModuleStatus, ViewInfo};
use crate::shortcut::Shortcut;

/// Une commande d'un module embarqué, exécutée en lui envoyant son raccourci.
#[derive(Clone, Debug)]
pub struct KeyCommand {
    pub id: &'static str,
    pub title: &'static str,
    pub shortcut: Shortcut,
}

impl KeyCommand {
    pub fn new(id: &'static str, title: &'static str, shortcut: &str) -> Self {
        let shortcut = Shortcut::parse(shortcut).unwrap_or_else(|| panic!("raccourci illisible : {shortcut}"));
        KeyCommand { id, title, shortcut }
    }
}

type UiHook<A> = Box<dyn FnMut(&mut A, &mut Cx)>;
type StatusHook<A> = Box<dyn Fn(&A, &ModuleStatus) -> ModuleStatus>;
type CloseHook<A> = Box<dyn FnMut(&mut A) -> CloseCheck>;
type RequestHook<A> = Box<dyn FnMut(&mut A, &Request, &mut Cx) -> Result<serde_json::Value, String>>;
type TitleParser = fn(&str) -> Option<Badge>;

/// Une application eframe vue comme un module.
pub struct Embedded<A: eframe::App> {
    pub app: A,
    info: ModuleInfo,
    host: Host,
    keys: Vec<KeyCommand>,
    pending: Vec<egui::Event>,
    /// Frappes déjà données à la fenêtre principale pendant `logic`, à redonner à `ui` si le module
    /// est dessiné dans une fenêtre détachée (elle a sa propre entrée).
    injected: Vec<egui::Event>,
    status: ModuleStatus,
    hidden_ui: bool,
    title_badge: Option<TitleParser>,
    before_ui: Option<UiHook<A>>,
    after_ui: Option<UiHook<A>>,
    after_logic: Option<UiHook<A>>,
    status_hook: Option<StatusHook<A>>,
    close_hook: Option<CloseHook<A>>,
    request_hook: Option<RequestHook<A>>,
    keep: Vec<Box<dyn Any>>,
}

impl<A: eframe::App> Embedded<A> {
    pub fn new(info: ModuleInfo, host: Host, app: A) -> Self {
        Embedded {
            app,
            info,
            host,
            keys: Vec::new(),
            pending: Vec::new(),
            injected: Vec::new(),
            status: ModuleStatus::default(),
            hidden_ui: false,
            title_badge: None,
            before_ui: None,
            after_ui: None,
            after_logic: None,
            status_hook: None,
            close_hook: None,
            request_hook: None,
            keep: Vec::new(),
        }
    }

    /// Le travail de fond de l'application est resté dans `ui` : l'hôte l'appelle même masquée.
    pub fn hidden_ui(mut self, yes: bool) -> Self {
        self.hidden_ui = yes;
        self
    }

    pub fn key_commands(mut self, keys: Vec<KeyCommand>) -> Self {
        self.keys = keys;
        self
    }

    /// Lit la pastille dans le titre que l'application donne à sa fenêtre (« Owlery (3) »).
    pub fn title_badge(mut self, parse: TitleParser) -> Self {
        self.title_badge = Some(parse);
        self
    }

    pub fn before_ui(mut self, f: impl FnMut(&mut A, &mut Cx) + 'static) -> Self {
        self.before_ui = Some(Box::new(f));
        self
    }

    pub fn after_ui(mut self, f: impl FnMut(&mut A, &mut Cx) + 'static) -> Self {
        self.after_ui = Some(Box::new(f));
        self
    }

    /// Après sa logique, à chaque image, même masquée : relever ce qu'elle expose publiquement
    /// (Fawkes : les sessions Claude qui se mettent à attendre).
    pub fn after_logic(mut self, f: impl FnMut(&mut A, &mut Cx) + 'static) -> Self {
        self.after_logic = Some(Box::new(f));
        self
    }

    /// Complète l'état relevé (titre, pastille) par ce que l'application expose publiquement.
    pub fn status(mut self, f: impl Fn(&A, &ModuleStatus) -> ModuleStatus + 'static) -> Self {
        self.status_hook = Some(Box::new(f));
        self
    }

    pub fn can_close(mut self, f: impl FnMut(&mut A) -> CloseCheck + 'static) -> Self {
        self.close_hook = Some(Box::new(f));
        self
    }

    pub fn on_request(
        mut self,
        f: impl FnMut(&mut A, &Request, &mut Cx) -> Result<serde_json::Value, String> + 'static,
    ) -> Self {
        self.request_hook = Some(Box::new(f));
        self
    }

    /// Garde une valeur autant que le module (verrou d'instance, serveur de tube…).
    pub fn keep(mut self, value: impl Any) -> Self {
        self.keep.push(Box::new(value));
        self
    }
}

impl<A: eframe::App> Module for Embedded<A> {
    fn info(&self) -> &ModuleInfo {
        &self.info
    }

    fn views(&self) -> Vec<ViewInfo> {
        vec![ViewInfo::main(self.info.name)]
    }

    fn logic(&mut self, ctx: &egui::Context, cx: &mut Cx) {
        // Les frappes d'une commande de la palette : dès la logique (certaines applications lisent
        // leurs raccourcis là), elles restent dans l'entrée pour `ui`.
        self.injected.clear();
        if cx.active && !self.pending.is_empty() {
            let keys = std::mem::take(&mut self.pending);
            ctx.input_mut(|i| i.events.extend(keys.iter().cloned()));
            self.injected = keys;
        }
        self.app.logic(ctx, cx.frame);
        if let Some(h) = &mut self.after_logic {
            h(&mut self.app, cx);
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _view: &str, cx: &mut Cx) {
        if ui.ctx().viewport_id() != egui::ViewportId::ROOT && !self.injected.is_empty() {
            let keys = std::mem::take(&mut self.injected);
            ui.ctx().input_mut(|i| i.events.extend(keys));
        }
        if let Some(h) = &mut self.before_ui {
            h(&mut self.app, cx);
        }
        self.app.ui(ui, cx.frame);
        if let Some(h) = &mut self.after_ui {
            h(&mut self.app, cx);
        }
    }

    fn needs_hidden_ui(&self) -> bool {
        self.hidden_ui
    }

    fn status(&self) -> ModuleStatus {
        match &self.status_hook {
            Some(h) => h(&self.app, &self.status),
            None => self.status.clone(),
        }
    }

    fn commands(&self) -> Vec<CommandInfo> {
        self.keys
            .iter()
            .map(|k| CommandInfo { id: k.id.to_string(), title: k.title.to_string(), shortcut: Some(k.shortcut) })
            .collect()
    }

    fn run_command(&mut self, id: &str, _cx: &mut Cx) {
        if let Some(k) = self.keys.iter().find(|k| k.id == id) {
            self.pending.extend(k.shortcut.to_events());
            self.host.repaint();
        }
    }

    fn on_request(&mut self, request: &Request, cx: &mut Cx) -> Result<serde_json::Value, String> {
        match &mut self.request_hook {
            Some(h) => h(&mut self.app, request, cx),
            None => Err(format!("{} ne reçoit pas encore de demande dans la suite", self.info.name)),
        }
    }

    fn observe_viewport_commands(&mut self, commands: &[egui::ViewportCommand]) {
        for c in commands {
            match c {
                egui::ViewportCommand::Title(t) => {
                    self.status.title = Some(t.clone());
                    if let Some(parse) = self.title_badge {
                        self.status.badge = parse(t);
                    }
                }
                // « Montre-toi » (une autre instance a demandé `afficher`, clic sur une notification) :
                // la case du module est sélectionnée et la suite passe au premier plan.
                egui::ViewportCommand::Focus => self.host.show(self.info.id),
                egui::ViewportCommand::RequestUserAttention(a) if *a != egui::UserAttentionType::Reset => {
                    self.host.attention(self.info.id)
                }
                _ => {}
            }
        }
    }

    fn can_close(&mut self) -> CloseCheck {
        match &mut self.close_hook {
            Some(h) => h(&mut self.app),
            None => CloseCheck::Ready,
        }
    }

    fn on_exit(&mut self) {
        self.app.on_exit(None);
        self.keep.clear();
    }
}

/// « Nom (3) » → 3 ; pour [`Embedded::title_badge`].
pub fn count_in_parens(title: &str) -> Option<u32> {
    let open = title.rfind('(')?;
    let close = title[open..].find(')')? + open;
    title[open + 1..close].trim().parse().ok()
}

/// Premier nombre d'un texte (« Menagerie — 2 à voir » → 2).
pub fn first_number(text: &str) -> Option<u32> {
    let start = text.find(|c: char| c.is_ascii_digit())?;
    let digits: String = text[start..].chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_in_titles() {
        assert_eq!(count_in_parens("Owlery (12)"), Some(12));
        assert_eq!(count_in_parens("Owlery"), None);
        assert_eq!(first_number("Menagerie — 2 à voir"), Some(2));
        assert_eq!(first_number("Menagerie"), None);
    }
}
