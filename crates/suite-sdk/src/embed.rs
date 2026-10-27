//! Faire d'une application eframe existante un module, sans toucher à son code.
//!
//! Les six modules dessinent déjà leurs panneaux dans le `Ui` reçu (en egui 0.37, `Panel::show(ui)`
//! est rattaché au `Ui` parent) : leur `eframe::App` se pose tel quel dans une case. Ce qui déborde
//! de la case (style global, polices, clavier lu sur toute la fenêtre, commandes de fenêtre, focus)
//! est contenu par l'hôte autour de chaque appel (`eyrie/src/sandbox.rs`) ; cet adaptateur ne fait
//! que relier l'application à l'interface [`Module`] :
//! - `logic` et `ui` d'eframe ;
//! - le titre que l'application donne à sa fenêtre devient le titre et la pastille du module ;
//! - ses raccourcis connus deviennent des commandes de la palette de la suite : les exécuter, c'est
//!   lui envoyer la frappe ;
//! - ce qui doit vivre autant que le module (verrou d'instance, serveur de tube) est gardé ici.

use std::any::Any;

use eframe::egui;

use crate::host::{Host, Request};
use crate::mobile;
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
type EventHook<A> = Box<dyn FnMut(&mut A, &crate::host::Event, &mut Cx)>;
type PanelHook<A> = Box<dyn FnMut(&mut A, &mut egui::Ui)>;
type LeftHook<A> = Box<dyn FnMut(&mut A)>;
type ScreenHook<A> = Box<dyn FnMut(&mut A, &str, &mut Cx) -> Option<mobile::ui::Screen>>;
type ActionHook<A> = Box<dyn FnMut(&mut A, &str, &str, Option<&str>, &mut Cx) -> Result<Option<String>, String>>;
type WaitingHook<A> = Box<dyn Fn(&A) -> Vec<(String, Option<String>)>>;
type TermHook<A> = Box<dyn FnMut(&mut A, &str, usize) -> Option<mobile::term::TermView>>;
type TermInputHook<A> = Box<dyn FnMut(&mut A, &str, &mobile::term::TermInput, &mut Cx) -> Result<(), String>>;

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
    before_logic: Option<UiHook<A>>,
    event_hook: Option<EventHook<A>>,
    status_hook: Option<StatusHook<A>>,
    close_hook: Option<CloseHook<A>>,
    request_hook: Option<RequestHook<A>>,
    settings_panel: Option<PanelHook<A>>,
    settings_left: Option<LeftHook<A>>,
    mobile_screen: Option<ScreenHook<A>>,
    mobile_waiting: Option<WaitingHook<A>>,
    mobile_action: Option<ActionHook<A>>,
    mobile_term: Option<TermHook<A>>,
    mobile_term_input: Option<TermInputHook<A>>,
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
            before_logic: None,
            event_hook: None,
            status_hook: None,
            close_hook: None,
            request_hook: None,
            settings_panel: None,
            settings_left: None,
            mobile_screen: None,
            mobile_waiting: None,
            mobile_action: None,
            mobile_term: None,
            mobile_term_input: None,
            keep: Vec::new(),
        }
    }

    /// Le travail de fond de l'application est resté dans `ui` : l'hôte l'appelle même masquée.
    pub fn hidden_ui(mut self, yes: bool) -> Self {
        self.hidden_ui = yes;
        self
    }

    /// Ses réglages, dessinés dans la page que l'hôte lui donne dans ses propres réglages (`Module::settings_ui`).
    pub fn settings_panel(mut self, f: impl FnMut(&mut A, &mut egui::Ui) + 'static) -> Self {
        self.settings_panel = Some(Box::new(f));
        self
    }

    /// Sa page dans les réglages de l'hôte a été quittée (`Module::settings_left`).
    pub fn on_settings_left(mut self, f: impl FnMut(&mut A) + 'static) -> Self {
        self.settings_left = Some(Box::new(f));
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

    /// Avant sa logique, à chaque image : lui passer ce que la suite impose (DragonTree : thème et
    /// langue).
    pub fn before_logic(mut self, f: impl FnMut(&mut A, &mut Cx) + 'static) -> Self {
        self.before_logic = Some(Box::new(f));
        self
    }

    /// Après sa logique, à chaque image, même masquée : relever ce qu'elle expose publiquement
    /// (Fawkes : les sessions Claude qui se mettent à attendre).
    pub fn after_logic(mut self, f: impl FnMut(&mut A, &mut Cx) + 'static) -> Self {
        self.after_logic = Some(Box::new(f));
        self
    }

    /// Les événements du bus de la suite (le contexte partagé, `contexte`…), traduits en appels de
    /// ce que l'application expose publiquement.
    pub fn on_event(mut self, f: impl FnMut(&mut A, &crate::host::Event, &mut Cx) + 'static) -> Self {
        self.event_hook = Some(Box::new(f));
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

    /// Ses écrans sur le téléphone (`Module::mobile_screen`).
    pub fn mobile_screens(
        mut self,
        f: impl FnMut(&mut A, &str, &mut Cx) -> Option<mobile::ui::Screen> + 'static,
    ) -> Self {
        self.mobile_screen = Some(Box::new(f));
        self
    }

    /// Ce qui attend, avec l'écran mobile où y répondre (`Module::mobile_waiting`).
    pub fn mobile_waitings(mut self, f: impl Fn(&A) -> Vec<(String, Option<String>)> + 'static) -> Self {
        self.mobile_waiting = Some(Box::new(f));
        self
    }

    /// Les actions de ses écrans sur le téléphone (`Module::mobile_action`).
    pub fn mobile_actions(
        mut self,
        f: impl FnMut(&mut A, &str, &str, Option<&str>, &mut Cx) -> Result<Option<String>, String> + 'static,
    ) -> Self {
        self.mobile_action = Some(Box::new(f));
        self
    }

    /// Un terminal suivi depuis le téléphone (`Module::mobile_term`).
    pub fn mobile_terminals(
        mut self,
        f: impl FnMut(&mut A, &str, usize) -> Option<mobile::term::TermView> + 'static,
    ) -> Self {
        self.mobile_term = Some(Box::new(f));
        self
    }

    /// Une saisie du téléphone dans un terminal (`Module::mobile_term_input`).
    pub fn mobile_terminal_input(
        mut self,
        f: impl FnMut(&mut A, &str, &mobile::term::TermInput, &mut Cx) -> Result<(), String> + 'static,
    ) -> Self {
        self.mobile_term_input = Some(Box::new(f));
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
        if let Some(h) = &mut self.before_logic {
            h(&mut self.app, cx);
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

    fn settings_ui(&mut self, ui: &mut egui::Ui, _host: &Host) -> bool {
        match &mut self.settings_panel {
            Some(draw) => {
                draw(&mut self.app, ui);
                true
            }
            None => false,
        }
    }

    fn settings_left(&mut self) {
        if let Some(f) = &mut self.settings_left {
            f(&mut self.app);
        }
    }

    fn run_command(&mut self, id: &str, _cx: &mut Cx) {
        if let Some(k) = self.keys.iter().find(|k| k.id == id) {
            self.pending.extend(k.shortcut.to_events());
            self.host.repaint();
        }
    }

    fn on_event(&mut self, event: &crate::host::Event, cx: &mut Cx) {
        if let Some(h) = &mut self.event_hook {
            h(&mut self.app, event, cx);
        }
    }

    fn on_request(&mut self, request: &Request, cx: &mut Cx) -> Result<serde_json::Value, String> {
        match &mut self.request_hook {
            Some(h) => h(&mut self.app, request, cx),
            None => Err(format!("{} ne reçoit pas encore de demande dans Eyrie", self.info.name)),
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

    fn mobile_screen(&mut self, path: &str, cx: &mut Cx) -> Option<mobile::ui::Screen> {
        self.mobile_screen.as_mut().and_then(|h| h(&mut self.app, path, cx))
    }

    fn mobile_waiting(&self) -> Vec<(String, Option<String>)> {
        self.mobile_waiting.as_ref().map(|h| h(&self.app)).unwrap_or_default()
    }

    fn mobile_action(
        &mut self,
        path: &str,
        action: &str,
        input: Option<&str>,
        cx: &mut Cx,
    ) -> Result<Option<String>, String> {
        match &mut self.mobile_action {
            Some(h) => h(&mut self.app, path, action, input, cx),
            None => Err(format!("{} n'a pas d'action sur le téléphone", self.info.name)),
        }
    }

    fn mobile_term(&mut self, id: &str, history: usize) -> Option<mobile::term::TermView> {
        self.mobile_term.as_mut().and_then(|h| h(&mut self.app, id, history))
    }

    fn mobile_term_input(&mut self, id: &str, input: &mobile::term::TermInput, cx: &mut Cx) -> Result<(), String> {
        match &mut self.mobile_term_input {
            Some(h) => h(&mut self.app, id, input, cx),
            None => Err(format!("{} n'a pas de terminal", self.info.name)),
        }
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

    struct Dummy {
        drawn: u32,
    }

    impl eframe::App for Dummy {
        fn ui(&mut self, _ui: &mut egui::Ui, _frame: &mut eframe::Frame) {}
    }

    fn info() -> ModuleInfo {
        ModuleInfo {
            id: "factice",
            name: "Factice",
            icon: "?",
            logo: None,
            color: egui::Color32::GRAY,
            summary: "essai",
            origin: crate::module::Origin::Native,
        }
    }

    #[test]
    fn embedded_settings_panel_is_drawn() {
        let host = Host::new(egui::Context::default(), std::env::temp_dir(), crate::theme::ThemeChoice::Graphite);
        let mut plain = Embedded::new(info(), host.clone(), Dummy { drawn: 0 });
        let mut tuned = Embedded::new(info(), host.clone(), Dummy { drawn: 0 }).settings_panel(|app, ui| {
            app.drawn += 1;
            ui.label("réglages du module");
        });
        egui::__run_test_ui(|ui| {
            assert!(!plain.settings_ui(ui, &host), "sans panneau : aucune page");
            assert!(tuned.settings_ui(ui, &host));
        });
        assert_eq!(tuned.app.drawn, 1);
    }

    #[test]
    fn embedded_settings_left_reaches_the_app() {
        let host = Host::new(egui::Context::default(), std::env::temp_dir(), crate::theme::ThemeChoice::Graphite);
        let mut plain = Embedded::new(info(), host.clone(), Dummy { drawn: 0 });
        let mut tuned = Embedded::new(info(), host, Dummy { drawn: 3 }).on_settings_left(|app| app.drawn = 0);
        plain.settings_left();
        tuned.settings_left();
        assert_eq!(plain.app.drawn, 0);
        assert_eq!(tuned.app.drawn, 0, "le module a oublié ce que sa page avait en cours");
    }

    #[test]
    fn mobile_hooks_reach_the_app() {
        let host = Host::new(egui::Context::default(), std::env::temp_dir(), crate::theme::ThemeChoice::Graphite);
        let mut plain = Embedded::new(info(), host.clone(), Dummy { drawn: 0 });
        let mut tuned = Embedded::new(info(), host.clone(), Dummy { drawn: 2 })
            .mobile_screens(|app, path, _| Some(mobile::ui::Screen::new(format!("{path}:{}", app.drawn))))
            .mobile_actions(|app, _, action, _, _| {
                app.drawn += 1;
                Ok(Some(action.to_string()))
            });
        let mut frame = eframe::Frame::_new_kittest();
        let mut cx = Cx { host: &host, frame: &mut frame, active: false, visible: false };
        assert_eq!(Module::mobile_screen(&mut plain, "", &mut cx), None);
        assert!(Module::mobile_action(&mut plain, "", "x", None, &mut cx).is_err());
        assert_eq!(Module::mobile_screen(&mut tuned, "t/1", &mut cx).map(|s| s.title), Some("t/1:2".into()));
        assert_eq!(Module::mobile_action(&mut tuned, "", "lancer", None, &mut cx), Ok(Some("lancer".into())));
        assert_eq!(tuned.app.drawn, 3);
    }

    #[test]
    fn numbers_in_titles() {
        assert_eq!(count_in_parens("Owlery (12)"), Some(12));
        assert_eq!(count_in_parens("Owlery"), None);
        assert_eq!(first_number("Menagerie — 2 à voir"), Some(2));
        assert_eq!(first_number("Menagerie"), None);
    }
}
