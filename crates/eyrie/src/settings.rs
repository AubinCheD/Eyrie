//! Les réglages d'Eyrie : général (thème, zoom, langue, données des modules), modules, comptes de toute la
//! suite, Tailscale, raccourcis (et leurs conflits), dispositions, et une page par module.
//!
//! Des libellés qui se suffisent ; une phrase d'aide au plus par page ou par section, le détail en infobulle.

use dragontree_forges::ForgeKind;
use eframe::egui::{self, RichText};
use suite_sdk::Origin;
use suite_sdk::i18n::Lang;
use suite_sdk::shortcut::{Owner, Shortcut};
use suite_sdk::theme::{self, ThemeChoice};

use crate::accounts::title;
use crate::app::{Action, Core, DEFAULT_LAYOUTS, suite_shortcuts};
use crate::core::SlotState;
use crate::i18n::{mark, tr};
use crate::layout::Pane;
use crate::persist::ModuleData;
use crate::shell::ShellState;

#[derive(Clone, Debug, Default, PartialEq)]
pub enum Page {
    #[default]
    General,
    Modules,
    Accounts,
    Tailscale,
    Shortcuts,
    Layouts,
    Module(String),
}

impl SettingsState {
    /// Le compte saisi a été ajouté : le formulaire oublie le jeton (et le reste).
    pub fn clear_new_account(&mut self) {
        self.new_token.clear();
        self.new_host.clear();
        self.new_orgs.clear();
    }
}

impl Page {
    /// Page demandée par un module (`Host::open_settings`) ; inconnue : Général.
    pub fn from_key(key: &str) -> Page {
        match key {
            "comptes" => Page::Accounts,
            "modules" => Page::Modules,
            "raccourcis" => Page::Shortcuts,
            "tailscale" => Page::Tailscale,
            "dispositions" => Page::Layouts,
            k if k.starts_with("module:") => Page::Module(k["module:".len()..].to_string()),
            _ => Page::General,
        }
    }
}

pub struct SettingsState {
    pub open: bool,
    pub page: Page,
    new_kind: ForgeKind,
    new_host: String,
    new_token: String,
    new_orgs: String,
    shortcut_edit: std::collections::BTreeMap<String, String>,
    /// Nom du nœud Tailscale en cours de saisie (appliqué à la validation : chaque changement
    /// relance le compagnon).
    tailnet_host: Option<String>,
    /// Compte dont le retrait attend confirmation (il vaut pour toute la suite).
    confirm_remove: Option<String>,
    /// Fenêtre détachée où les réglages s'affichent (limités à Eyrie et à ses modules) ; `None` : la principale.
    pub window: Option<u64>,
}

impl Default for SettingsState {
    fn default() -> Self {
        SettingsState {
            open: false,
            page: Page::General,
            new_kind: ForgeKind::GitHub,
            new_host: String::new(),
            new_token: String::new(),
            new_orgs: String::new(),
            shortcut_edit: Default::default(),
            tailnet_host: None,
            confirm_remove: None,
            window: None,
        }
    }
}

/// Les noms des thèmes, définis dans le SDK (`ThemeChoice::label`) et traduits ici.
#[allow(dead_code)]
const THEME_NAMES: [&str; 4] = [mark("Système"), mark("Graphite"), mark("Fluent"), mark("Nuit colorée")];

/// Ce que la fenêtre de réglages lit et change. Elle se dessine dans la fenêtre principale (tous les modules) ou dans
/// une fenêtre détachée (Eyrie et les seuls modules de cette fenêtre).
pub struct SettingsCx<'a> {
    pub core: &'a mut Core,
    pub shell: &'a mut ShellState,
    /// Modules montrés : `None` : tous.
    pub modules: Option<Vec<String>>,
}

impl SettingsCx<'_> {
    fn shows(&self, module: &str) -> bool {
        self.modules.as_ref().is_none_or(|list| list.iter().any(|m| m == module))
    }
}

/// Les modules d'une fenêtre (ses cases), dans l'ordre, sans doublon.
pub fn window_modules(tree: &egui_tiles::Tree<Pane>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for (_, pane) in crate::layout::panes(tree) {
        if let Some(m) = pane.module_id()
            && !out.iter().any(|o| o == m)
        {
            out.push(m.to_string());
        }
    }
    out
}

fn section(ui: &mut egui::Ui, title: &str) {
    ui.add_space(10.0);
    ui.label(RichText::new(title).strong().size(14.5));
    ui.add_space(2.0);
}

fn hint(ui: &mut egui::Ui, p: &theme::Palette, text: &str) {
    ui.add(egui::Label::new(RichText::new(text).color(p.text_dim)).wrap());
}

pub fn show(suite: &mut SettingsCx<'_>, ctx: &egui::Context) {
    if !suite.shell.settings.open {
        return;
    }
    let mut open = true;
    let resp = egui::Window::new(tr("Réglages d'Eyrie"))
        .id(egui::Id::unique("suite-reglages"))
        .open(&mut open)
        .default_size([820.0, 560.0])
        .default_pos(ctx.content_rect().center() - egui::vec2(410.0, 290.0))
        .min_size([620.0, 400.0])
        .collapsible(false)
        .show(ctx, |ui| {
            egui::Panel::left("reglages-pages").exact_size(190.0).resizable(false).show(ui, |ui| nav(suite, ui));
            egui::CentralPanel::default().show(ui, |ui| {
                egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| page(suite, ui));
            });
        });
    if let Some(r) = resp
        && ctx
            .input(|i| i.pointer.any_pressed() && i.pointer.press_origin().is_some_and(|o| r.response.rect.contains(o)))
    {
        // Les champs des réglages reçoivent le clavier, pas le module de la dernière case.
        suite.core.focus.module = None;
    }
    if !open {
        suite.shell.settings.open = false;
    }
}

fn nav(suite: &mut SettingsCx<'_>, ui: &mut egui::Ui) {
    let p = suite.core.palette();
    let st = &mut suite.shell.settings;
    for (page, label) in [
        (Page::General, mark("Général")),
        (Page::Modules, mark("Modules")),
        (Page::Accounts, mark("Comptes")),
        (Page::Tailscale, mark("Tailscale")),
        (Page::Shortcuts, mark("Raccourcis")),
        (Page::Layouts, mark("Dispositions")),
    ] {
        if ui.selectable_label(st.page == page, tr(label)).clicked() {
            st.page = page;
        }
    }
    ui.separator();
    ui.label(RichText::new(tr("Modules")).color(p.text_dim).small());
    for s in &suite.core.slots {
        if !suite.shows(s.info.id) {
            continue;
        }
        let page = Page::Module(s.info.id.to_string());
        let text = RichText::new(format!("{}  {}", s.info.icon, s.info.name)).color(theme::readable(p, s.info.color));
        if ui.selectable_label(suite.shell.settings.page == page, text).clicked() {
            suite.shell.settings.page = page;
        }
    }
}

fn page(suite: &mut SettingsCx<'_>, ui: &mut egui::Ui) {
    // Dans une fenêtre détachée, la page d'un module qui n'y est pas cède la place à Général.
    if let Page::Module(id) = &suite.shell.settings.page
        && !suite.shows(id)
    {
        suite.shell.settings.page = Page::General;
    }
    match suite.shell.settings.page.clone() {
        Page::General => general(suite, ui),
        Page::Modules => modules(suite, ui),
        Page::Accounts => accounts(suite, ui),
        Page::Tailscale => tailscale(suite, ui),
        Page::Shortcuts => shortcuts(suite, ui),
        Page::Layouts => layouts(suite, ui),
        Page::Module(id) => module_page(suite, ui, &id),
    }
}

fn general(suite: &mut SettingsCx<'_>, ui: &mut egui::Ui) {
    let p = suite.core.palette();
    ui.heading(tr("Général"));

    section(ui, tr("Thème"));
    ui.horizontal_wrapped(|ui| {
        for t in ThemeChoice::ALL {
            let pal = theme::palette(theme::resolve(t, ui.ctx()));
            let selected = suite.core.st.theme == t;
            let frame = egui::Frame::new()
                .fill(pal.panel)
                .stroke(egui::Stroke::new(if selected { 2.0 } else { 1.0 }, if selected { p.accent } else { p.border }))
                .corner_radius(egui::CornerRadius::same(pal.radius))
                .inner_margin(egui::Margin::same(8));
            let r = frame
                .show(ui, |ui| {
                    ui.set_width(130.0);
                    ui.label(RichText::new(tr(t.label())).color(pal.text).strong());
                    ui.horizontal(|ui| {
                        for c in [pal.sidebar, pal.surface, pal.accent, pal.ok, pal.warn, pal.danger] {
                            let (r, _) = ui.allocate_exact_size(egui::vec2(14.0, 14.0), egui::Sense::hover());
                            ui.painter().rect_filled(r, egui::CornerRadius::same(3), c);
                        }
                    });
                })
                .response
                .interact(egui::Sense::click());
            if r.clicked() {
                suite.core.act(Action::SetTheme(t));
            }
        }
    });

    section(ui, tr("Zoom"));
    let mut zoom = suite.core.st.zoom;
    if ui.add(egui::Slider::new(&mut zoom, 0.8..=1.6).step_by(0.05).text(tr("taille de l'interface"))).changed() {
        suite.core.st.zoom = zoom;
        theme::apply(ui.ctx(), suite.core.host.palette(), zoom);
    }

    section(ui, tr("Langue"));
    ui.horizontal(|ui| {
        if ui.radio(suite.core.st.lang.is_none(), tr("Celle de Windows")).clicked() {
            suite.core.act(Action::SetLang(None));
        }
        for l in Lang::ALL {
            if ui.radio(suite.core.st.lang == Some(l), l.label()).clicked() {
                suite.core.act(Action::SetLang(Some(l)));
            }
        }
    });

    section(ui, tr("Données des modules"));
    let mut data = suite.core.st.module_data;
    ui.radio_value(&mut data, ModuleData::Shared, tr("Les mêmes que les applications lancées seules"))
        .on_hover_text(tr("Mêmes groupes, terminaux, tâches et filtres, dans Eyrie ou en dehors."));
    ui.radio_value(&mut data, ModuleData::Trial, tr("Un dossier d'essai"))
        .on_hover_text(suite.core.dir.join("essai").display().to_string());
    if data != suite.core.st.module_data {
        suite.core.st.module_data = data;
        suite.core.restart_needed = true;
    }

    section(ui, tr("Dépôt courant"));
    ui.checkbox(&mut suite.core.st.follow_context, tr("Les modules suivent le dépôt choisi"))
        .on_hover_text(tr("Un dépôt choisi dans la barre du haut ou dans un module s'affiche partout."));
    restart_hint(suite, ui);
}

fn restart_hint(suite: &mut SettingsCx<'_>, ui: &mut egui::Ui) {
    if suite.core.restart_needed {
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new(tr("Prendra effet au prochain lancement.")).color(suite.core.palette().warn));
            if ui.button(tr("Relancer Eyrie")).clicked() {
                suite.core.act(Action::Restart);
            }
        });
    }
}

fn modules(suite: &mut SettingsCx<'_>, ui: &mut egui::Ui) {
    let p = suite.core.palette();
    ui.heading(tr("Modules"));
    hint(ui, p, tr("Un module désactivé ne démarre pas et ne coûte rien."));
    ui.add_space(6.0);
    let mut actions = Vec::new();
    for s in &suite.core.slots {
        egui::Frame::new()
            .fill(p.surface)
            .corner_radius(egui::CornerRadius::same(p.radius))
            .inner_margin(egui::Margin::same(8))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                slot_row(s, p, ui, &mut actions);
            });
        ui.add_space(4.0);
    }
    for a in actions {
        suite.core.act(a);
    }
    restart_hint(suite, ui);
}

/// Un module : interrupteur, nom, état, résumé, et ce qu'on peut y faire.
fn slot_row(s: &crate::core::Slot, p: &theme::Palette, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
    let color = theme::readable(p, s.info.color);
    ui.horizontal(|ui| {
        slot_toggle(s, ui, actions);
        ui.label(RichText::new(s.info.icon).size(18.0).color(color));
        ui.vertical(|ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(s.info.name).color(color).strong());
                slot_state(s, p, ui, actions);
            });
            ui.label(RichText::new(s.info.summary).color(p.text_dim));
        });
    });
}

fn slot_toggle(s: &crate::core::Slot, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
    let mut enabled = !matches!(s.state, SlotState::Disabled);
    if ui.checkbox(&mut enabled, "").on_hover_text(tr("Activer ou arrêter le module")).changed() {
        let id = s.info.id.to_string();
        actions.push(if enabled { Action::StartModule(id) } else { Action::StopModule(id) });
    }
}

/// L'état d'un module, et « Fermer <exe> » ou « Relancer » quand c'est utile.
fn slot_state(s: &crate::core::Slot, p: &theme::Palette, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
    let (state, c) = match &s.state {
        SlotState::Running(_) => (tr("actif").to_string(), p.ok),
        SlotState::Disabled => (tr("désactivé").to_string(), p.text_dim),
        SlotState::Unavailable(u) => (crate::app::unavailable_text(u), p.warn),
        SlotState::Crashed(m) => (format!("{} : {m}", tr("arrêté")), p.danger),
    };
    ui.label(RichText::new(state).color(c));
    if let SlotState::Unavailable(suite_sdk::Unavailable::AlreadyRunning { exe }) = &s.state
        && ui.small_button(format!("{} {exe}", tr("Fermer"))).clicked()
    {
        actions.push(Action::CloseExe(exe));
    }
    if matches!(s.state, SlotState::Unavailable(_) | SlotState::Crashed(_)) && ui.small_button(tr("Relancer")).clicked()
    {
        actions.push(Action::StartModule(s.info.id.to_string()));
    }
}

fn accounts(suite: &mut SettingsCx<'_>, ui: &mut egui::Ui) {
    let p = suite.core.palette();
    ui.heading(tr("Comptes"));
    hint(ui, p, tr("Valables dans toute la suite : Eyrie, DragonTree et Owlery, seuls ou intégrés."));
    ui.add_space(6.0);
    let list = suite.core.accounts.list().to_vec();
    if list.is_empty() {
        ui.label(RichText::new(tr("Aucun compte.")).color(p.text_dim));
    }
    let (mut remove, mut recheck) = (None, None);
    let mut confirm = suite.shell.settings.confirm_remove.clone();
    for a in &list {
        egui::Frame::new()
            .fill(p.surface)
            .corner_radius(egui::CornerRadius::same(p.radius))
            .inner_margin(egui::Margin::same(8))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(RichText::new(title(a)).strong());
                    if !a.orgs.is_empty() {
                        ui.label(RichText::new(a.orgs.join(", ")).color(p.text_dim));
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if confirm.as_deref() == Some(a.id.as_str()) {
                            if ui.small_button(tr("Annuler")).clicked() {
                                confirm = None;
                            }
                            if ui.small_button(RichText::new(tr("Oui, retirer")).color(p.danger)).clicked() {
                                remove = Some(a.id.clone());
                                confirm = None;
                            }
                            ui.label(RichText::new(tr("Retirer de toute la suite ?")).color(p.warn));
                            return;
                        }
                        if ui.small_button(tr("Retirer")).clicked() {
                            confirm = Some(a.id.clone());
                        }
                        if ForgeKind::from_suite_key(&a.forge).is_some() && ui.small_button(tr("Vérifier")).clicked() {
                            recheck = Some(a.id.clone());
                        }
                        let accounts = &suite.core.accounts;
                        if accounts.is_checking(&a.id) {
                            ui.spinner();
                        } else {
                            match accounts.status.get(&a.id) {
                                Some(Ok(())) => {
                                    ui.label(RichText::new(tr("connecté")).color(p.ok));
                                }
                                Some(Err(e)) => {
                                    ui.label(RichText::new(tr("refusé")).color(p.danger)).on_hover_text(e);
                                }
                                None => {}
                            }
                        }
                    });
                });
            });
        ui.add_space(4.0);
    }
    suite.shell.settings.confirm_remove = confirm;
    if let Some(id) = remove {
        suite.core.accounts.remove(&id);
    }
    if let Some(id) = recheck {
        suite.core.accounts.recheck(&id);
    }

    section(ui, tr("Ajouter un compte"));
    let st = &mut suite.shell.settings;
    ui.horizontal(|ui| {
        for k in ForgeKind::ALL {
            ui.radio_value(&mut st.new_kind, k, k.label());
        }
    });
    let kind = st.new_kind;
    egui::Grid::new("nouveau-compte").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
        ui.label(tr("Jeton (PAT)"));
        ui.add(egui::TextEdit::singleline(&mut st.new_token).password(true).desired_width(320.0));
        ui.end_row();
        if kind == ForgeKind::AzureDevOps {
            ui.label(tr("Organisations"));
            ui.add(
                egui::TextEdit::singleline(&mut st.new_orgs).hint_text(tr("contoso, fabrikam")).desired_width(320.0),
            )
            .on_hover_text(tr("Indispensable si le jeton est limité à une organisation."));
            ui.end_row();
        }
    });
    egui::CollapsingHeader::new(tr("Serveur")).id_salt("nouveau-compte-serveur").show(ui, |ui| {
        ui.add(egui::TextEdit::singleline(&mut st.new_host).hint_text(kind.default_host()).desired_width(320.0))
            .on_hover_text(tr(
                "GitHub Enterprise, GitLab auto-hébergé ou Azure DevOps Server ; vide : le service en ligne.",
            ));
    });
    ui.horizontal(|ui| {
        let org = st.new_orgs.split(',').map(str::trim).find(|o| !o.is_empty());
        if ui.link(tr("Créer un jeton")).clicked() {
            let url = dragontree_forges::token_creation_url(kind, st.new_host.trim(), org);
            ui.ctx().open_url(egui::OpenUrl::new_tab(url));
        }
        ui.label(
            RichText::new(format!("{} {}", tr("Droits :"), dragontree_forges::recommended_scopes(kind)))
                .color(p.text_dim)
                .small(),
        );
    });
    let busy = suite.core.accounts.is_busy();
    ui.horizontal(|ui| {
        let can = !busy && !suite.shell.settings.new_token.trim().is_empty();
        if ui.add_enabled(can, egui::Button::new(tr("Vérifier et ajouter"))).clicked() {
            let st = &mut suite.shell.settings;
            let orgs = st.new_orgs.split(',').map(|o| o.trim().to_string()).filter(|o| !o.is_empty()).collect();
            // Le jeton reste dans le champ tant qu'il n'est pas accepté (voir `clear_new_account`).
            suite.core.accounts.add(st.new_kind, &st.new_host, &st.new_token, orgs);
        }
        if busy {
            ui.spinner();
        }
    });
    if let Some((ok, msg)) = &suite.core.accounts.message {
        ui.add(egui::Label::new(RichText::new(msg).color(if *ok { p.ok } else { p.danger })).wrap());
    }
}

fn tailscale(suite: &mut SettingsCx<'_>, ui: &mut egui::Ui) {
    let p = suite.core.palette();
    ui.heading("Tailscale");
    hint(ui, p, tr("Un seul nœud pour toute la suite, pour la synchro avec le mobile."));
    ui.add_space(6.0);
    ui.checkbox(&mut suite.core.st.tailnet.enabled, tr("Rejoindre mon tailnet"));
    ui.horizontal(|ui| {
        ui.label(tr("Nom du nœud"));
        let draft = suite.shell.settings.tailnet_host.get_or_insert_with(|| suite.core.st.tailnet.hostname.clone());
        let r = ui.add(egui::TextEdit::singleline(draft).hint_text(crate::tailnet::hostname(&Default::default())));
        if r.lost_focus() {
            suite.core.st.tailnet.hostname = draft.trim().to_string();
            suite.shell.settings.tailnet_host = None;
        }
    });
    let companion = crate::tailnet::find_companion();
    match &companion {
        Some(c) => hint(ui, p, &format!("{} {}", tr("Compagnon :"), c.display())),
        None => {
            ui.label(RichText::new(tr("Compagnon fawkes-tailnet.exe introuvable.")).color(p.warn)).on_hover_text(tr(
                "Compilez-le dans Fawkes (tools\\compagnon.ps1) ou indiquez-le dans FAWKES_TAILNET_EXE.",
            ));
        }
    }
    let (line, color) = suite.core.tailnet.status_line(p);
    ui.label(RichText::new(line).color(color));
    match suite.core.tailnet.state() {
        Some(suite_fawkes::tailnet::TailnetState::NeedsLogin(url)) => {
            if ui.button(tr("Se connecter à Tailscale")).clicked() {
                ui.ctx().open_url(egui::OpenUrl::new_tab(url));
            }
        }
        Some(suite_fawkes::tailnet::TailnetState::Running { name, ip }) => {
            ui.label(RichText::new(format!("http://{name}  ·  {ip}")).monospace());
        }
        Some(suite_fawkes::tailnet::TailnetState::Failed(e)) => {
            ui.label(RichText::new(e).color(p.danger));
        }
        _ => {}
    }
}

fn shortcuts(suite: &mut SettingsCx<'_>, ui: &mut egui::Ui) {
    let p = suite.core.palette();
    ui.heading(tr("Raccourcis"));
    hint(ui, p, tr("Ceux d'Eyrie passent avant ceux des modules ; ceux d'un module valent dans sa case."));
    section(ui, tr("Eyrie"));
    let current = suite_shortcuts(&suite.core.st);
    let mut changed = Vec::new();
    egui::Grid::new("raccourcis-suite").num_columns(3).striped(true).spacing([12.0, 4.0]).show(ui, |ui| {
        for (id, title, sc) in &current {
            if id.starts_with("module_") && *id != "module_1" {
                continue;
            }
            let label = if *id == "module_1" { tr("Aller au module 1 à 9 (Alt+1…9)") } else { tr(title) };
            ui.label(label);
            let edit = suite.shell.settings.shortcut_edit.entry(id.to_string()).or_insert_with(|| sc.to_string());
            let r = ui.add_sized([170.0, 22.0], egui::TextEdit::singleline(edit));
            let parsed = Shortcut::parse(edit);
            if r.lost_focus()
                && let Some(sc) = parsed
            {
                changed.push((id.to_string(), sc.to_string()));
            }
            ui.label(if parsed.is_none() { RichText::new(tr("illisible")).color(p.danger) } else { RichText::new("") });
            ui.end_row();
        }
    });
    for (id, sc) in changed {
        suite.core.st.shortcuts.insert(id, sc);
    }

    section(ui, tr("Les modules"));
    let mut rows: Vec<(String, String, String)> = Vec::new();
    for b in &suite.core.registry.bindings {
        if let Owner::Module(m) = &b.owner {
            let name = suite.core.slot(m).map(|s| s.info.name.to_string()).unwrap_or_else(|| m.clone());
            rows.push((name, b.title.clone(), b.shortcut.to_string()));
        }
    }
    egui::Grid::new("raccourcis-modules").num_columns(3).striped(true).spacing([12.0, 3.0]).show(ui, |ui| {
        for (m, t, sc) in rows {
            ui.label(RichText::new(m).color(p.text_dim));
            ui.label(t);
            ui.label(RichText::new(sc).monospace());
            ui.end_row();
        }
    });

    section(ui, tr("Conflits"));
    let conflicts = suite.core.registry.conflicts();
    if conflicts.is_empty() {
        ui.label(RichText::new(tr("Aucun.")).color(p.ok));
    }
    for c in conflicts {
        let owners: Vec<String> = c
            .owners
            .iter()
            .map(|(o, t)| match o {
                Owner::Suite => format!("{} ({t})", tr("Eyrie")),
                Owner::Module(m) => format!("{} ({t})", suite.core.slot(m).map(|s| s.info.name).unwrap_or(m)),
            })
            .collect();
        let color = if c.suite_wins { p.warn } else { p.text_muted };
        ui.label(RichText::new(format!("{}  —  {}", c.shortcut, owners.join(", "))).color(color));
    }
    hint(ui, p, tr("Entre deux modules, c'est celui qui a le clavier qui l'emporte."));
}

fn layouts(suite: &mut SettingsCx<'_>, ui: &mut egui::Ui) {
    let p = suite.core.palette();
    ui.heading(tr("Dispositions"));
    hint(ui, p, tr("Plusieurs dispositions nommées, et une par configuration d'écrans, appliquée au branchement."));
    section(ui, tr("Enregistrées"));
    let mut names: Vec<String> = suite.core.st.layouts.keys().cloned().collect();
    for (n, _) in DEFAULT_LAYOUTS {
        if !names.iter().any(|x| x == n) {
            names.push(n.to_string());
        }
    }
    let mut actions = Vec::new();
    for name in &names {
        ui.horizontal(|ui| {
            ui.label(RichText::new(name).strong());
            let custom = suite.core.st.layouts.contains_key(name);
            if !custom {
                ui.label(RichText::new(tr("(proposée)")).color(p.text_dim).small());
            }
            if ui.small_button(tr("Appliquer")).clicked() {
                actions.push(Action::LoadLayout(name.clone()));
            }
            if ui.small_button(tr("Remplacer par l'actuelle")).clicked() {
                actions.push(Action::SaveLayout(name.clone()));
            }
            if custom && ui.small_button(tr("Supprimer")).clicked() {
                actions.push(Action::DeleteLayout(name.clone()));
            }
        });
    }
    if ui.button(tr("Enregistrer l'actuelle sous…")).clicked() {
        suite.shell.save_layout = Some(String::new());
        suite.core.focus.module = None;
    }

    section(ui, tr("Par configuration d'écrans"));
    let current = suite.core.screens.current();
    ui.label(format!("{} : {current}", tr("Actuelle")));
    let assigned = suite.core.st.screen_layouts.get(&current).cloned();
    egui::ComboBox::from_id_salt("disposition-ecrans")
        .selected_text(assigned.clone().unwrap_or_else(|| tr("aucune").to_string()))
        .show_ui(ui, |ui| {
            if ui.selectable_label(assigned.is_none(), tr("aucune")).clicked() {
                suite.core.st.screen_layouts.remove(&current);
            }
            for name in &names {
                if ui.selectable_label(assigned.as_ref() == Some(name), name).clicked() {
                    suite.core.st.screen_layouts.insert(current.clone(), name.clone());
                }
            }
        });
    for (sig, name) in &suite.core.st.screen_layouts {
        ui.label(RichText::new(format!("{sig}  →  {name}")).color(p.text_dim).small());
    }
    for a in actions {
        suite.core.act(a);
    }
}

/// Libellé du bouton qui ouvre les réglages propres d'un module.
pub fn module_settings_label(name: &str) -> String {
    format!("⚙ {} {name}", tr("Réglages de"))
}

fn module_page(suite: &mut SettingsCx<'_>, ui: &mut egui::Ui, id: &str) {
    let p = suite.core.palette();
    let mut actions = Vec::new();
    let Some(s) = suite.core.slots.iter_mut().find(|s| s.info.id == id) else { return };
    ui.heading(RichText::new(format!("{}  {}", s.info.icon, s.info.name)).color(theme::readable(p, s.info.color)));
    hint(ui, p, s.info.summary);
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        slot_toggle(s, ui, &mut actions);
        slot_state(s, p, ui, &mut actions);
    });
    // Ses réglages, ici même, dans le bac à sable comme ses autres appels.
    if s.is_running() {
        section(ui, tr("Réglages"));
        let host = suite.core.host.clone();
        let shown = crate::core::settings(s, &host, ui, &suite.core.wish, &mut suite.core.journal);
        let has_command = s.module().is_some_and(|m| m.commands().iter().any(|c| c.id == "reglages"));
        if !shown && has_command {
            // Un module sans page de réglages ici : les siens s'ouvrent dans sa case.
            if ui.button(module_settings_label(s.info.name)).clicked() {
                actions.push(Action::RunCommand { module: id.to_string(), command: "reglages".into() });
            }
        } else if !shown {
            ui.label(RichText::new(tr("Aucun réglage propre.")).color(p.text_dim));
        }
    }
    if let Origin::Embedded { exe, repo } = &s.info.origin {
        ui.add_space(10.0);
        egui::CollapsingHeader::new(tr("Diagnostic")).id_salt(("diagnostic", id)).show(ui, |ui| {
            ui.label(RichText::new(format!("{exe} · {repo}")).color(p.text_dim).small());
            ui.label(
                RichText::new(format!(
                    "{} : {} · {} : {}",
                    tr("Style remis en place"),
                    s.stats.style_restored,
                    tr("Commandes de fenêtre ignorées"),
                    s.stats.commands_countered
                ))
                .color(p.text_dim)
                .small(),
            );
        });
    }
    let leave = actions.iter().any(|a| matches!(a, Action::RunCommand { .. }));
    for a in actions {
        suite.core.act(a);
    }
    // Les réglages du module s'ouvrent dans sa case : ceux d'Eyrie s'effacent.
    if leave {
        suite.shell.settings.open = false;
    }
}
