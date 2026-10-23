//! Les réglages de la suite : général (langue, thème, zoom, données des modules), modules, comptes,
//! Tailscale, raccourcis (et leurs conflits), dispositions, et une page par module.

use dragontree_forges::ForgeKind;
use eframe::egui::{self, RichText};
use suite_sdk::i18n::Lang;
use suite_sdk::shortcut::{Owner, Shortcut};
use suite_sdk::theme::{self, ThemeChoice};
use suite_sdk::Origin;

use crate::accounts::FORGE_MODULES;
use crate::app::{Action, DEFAULT_LAYOUTS, Suite, suite_shortcuts};
use crate::core::SlotState;
use crate::i18n::{mark, tr};
use crate::persist::ModuleData;

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
        }
    }
}

/// Les noms des thèmes, définis dans le SDK (`ThemeChoice::label`) et traduits ici.
#[allow(dead_code)]
const THEME_NAMES: [&str; 4] = [mark("Système"), mark("Graphite"), mark("Fluent"), mark("Nuit colorée")];

fn section(ui: &mut egui::Ui, title: &str) {
    ui.add_space(10.0);
    ui.label(RichText::new(title).strong().size(14.5));
    ui.add_space(2.0);
}

fn hint(ui: &mut egui::Ui, p: &theme::Palette, text: &str) {
    ui.add(egui::Label::new(RichText::new(text).color(p.text_dim)).wrap());
}

pub fn show(suite: &mut Suite, ctx: &egui::Context) {
    if !suite.shell.settings.open {
        return;
    }
    let mut open = true;
    let resp = egui::Window::new(tr("Réglages de la suite"))
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
        && ctx.input(|i| i.pointer.any_pressed() && i.pointer.press_origin().is_some_and(|o| r.response.rect.contains(o)))
    {
        // Les champs des réglages reçoivent le clavier, pas le module de la dernière case.
        suite.core.focus.module = None;
    }
    if !open {
        suite.shell.settings.open = false;
    }
}

fn nav(suite: &mut Suite, ui: &mut egui::Ui) {
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
        let page = Page::Module(s.info.id.to_string());
        let text = RichText::new(format!("{}  {}", s.info.icon, s.info.name)).color(theme::readable(p, s.info.color));
        if ui.selectable_label(suite.shell.settings.page == page, text).clicked() {
            suite.shell.settings.page = page;
        }
    }
}

fn page(suite: &mut Suite, ui: &mut egui::Ui) {
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

fn general(suite: &mut Suite, ui: &mut egui::Ui) {
    let p = suite.core.palette();
    ui.heading(tr("Général"));
    hint(ui, p, tr("Dans la suite, thème, langue, comptes et Tailscale sont communs à tous les modules. Lancé seul, chaque module garde ses propres réglages : ils ne sont ni lus ni écrasés ici."));

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
    hint(ui, p, tr("Les trois thèmes de Fawkes (contrastes WCAG vérifiés). Système suit Windows : Graphite en sombre, Fluent en clair. Les modules embarqués suivent la palette de la suite là où ils lisent le style d'egui (DragonTree, Owlery) ; Fawkes suit le thème choisi ; les autres gardent leurs couleurs propres dans leurs vues."));

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
    hint(ui, p, tr("La suite est traduite ; parmi les modules, seul DragonTree l'est aujourd'hui (les cinq autres sont en français, leur traduction est un chantier à part)."));

    section(ui, tr("Données des modules"));
    let mut data = suite.core.st.module_data;
    ui.radio_value(&mut data, ModuleData::Shared, tr("Celles des exécutables autonomes"));
    hint(ui, p, tr("Mêmes groupes, terminaux, tâches et filtres des deux côtés. La suite tient le verrou de chaque exe : un module ne tourne jamais deux fois sur le même dossier."));
    ui.radio_value(&mut data, ModuleData::Trial, tr("Un dossier d'essai propre à la suite"));
    hint(ui, p, &format!("{} {}", tr("Pour essayer sans toucher aux données de tous les jours :"), suite.core.dir.join("essai").display()));
    if data != suite.core.st.module_data {
        suite.core.st.module_data = data;
        suite.core.restart_needed = true;
    }

    section(ui, tr("Contexte partagé"));
    ui.checkbox(&mut suite.core.st.follow_context, tr("Les cases suivent le contexte (dépôt courant)"));
    hint(ui, p, tr("Choisir un dépôt (barre du haut, ou depuis un module) le publie à tous les modules, comme la synchronisation avec le document actif de Visual Studio. Les modules natifs s'y accordent déjà ; les modules embarqués le feront quand ils parleront à la suite."));
    restart_hint(suite, ui);
}

fn restart_hint(suite: &mut Suite, ui: &mut egui::Ui) {
    if suite.core.restart_needed {
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new(tr("Prendra effet au prochain lancement.")).color(suite.core.palette().warn));
            if ui.button(tr("Relancer la suite")).clicked() {
                suite.core.act(Action::Restart);
            }
        });
    }
}

fn modules(suite: &mut Suite, ui: &mut egui::Ui) {
    let p = suite.core.palette();
    ui.heading(tr("Modules"));
    hint(ui, p, tr("Tout ou partie des modules : un module désactivé n'est pas construit et ne coûte rien. Les modules sont compilés dans la suite ; en ajouter un ne demande pas de retoucher les autres. Activer ou arrêter un module prend effet tout de suite."));
    ui.add_space(6.0);
    let mut actions = Vec::new();
    for s in &suite.core.slots {
        let color = theme::readable(p, s.info.color);
        egui::Frame::new().fill(p.surface).corner_radius(egui::CornerRadius::same(p.radius)).inner_margin(egui::Margin::same(8)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let mut enabled = !matches!(s.state, SlotState::Disabled);
                if ui.checkbox(&mut enabled, "").on_hover_text(tr("Activer ou arrêter le module")).changed() {
                    actions.push(if enabled { Action::StartModule(s.info.id.to_string()) } else { Action::StopModule(s.info.id.to_string()) });
                }
                ui.label(RichText::new(s.info.icon).size(18.0).color(color));
                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(s.info.name).color(color).strong());
                        let (state, c) = match &s.state {
                            SlotState::Running(_) => (tr("actif").to_string(), p.ok),
                            SlotState::Disabled => (tr("désactivé").to_string(), p.text_dim),
                            SlotState::Unavailable(u) => (crate::app::unavailable_text(u), p.warn),
                            SlotState::Crashed(m) => (format!("{} : {m}", tr("arrêté")), p.danger),
                        };
                        ui.label(RichText::new(state).color(c));
                    });
                    ui.label(RichText::new(s.info.summary).color(p.text_dim));
                    let origin = match &s.info.origin {
                        Origin::Native => tr("Module natif, écrit pour la suite.").to_string(),
                        Origin::Embedded { exe, repo } => format!("{} {exe} · {repo}", tr("Embarqué sans changer son code :")),
                    };
                    ui.label(RichText::new(origin).color(p.text_dim).small());
                    ui.horizontal(|ui| {
                        if let SlotState::Unavailable(suite_sdk::Unavailable::AlreadyRunning { exe }) = &s.state
                            && ui.small_button(format!("{} {exe}", tr("Fermer"))).clicked()
                        {
                            actions.push(Action::CloseExe(exe));
                        }
                        if matches!(s.state, SlotState::Unavailable(_) | SlotState::Crashed(_)) && ui.small_button(tr("Relancer")).clicked() {
                            actions.push(Action::StartModule(s.info.id.to_string()));
                        }
                    });
                });
            });
        });
        ui.add_space(4.0);
    }
    for a in actions {
        suite.core.act(a);
    }
    restart_hint(suite, ui);
}

fn accounts(suite: &mut Suite, ui: &mut egui::Ui) {
    let p = suite.core.palette();
    ui.heading(tr("Comptes"));
    hint(ui, p, tr("Les comptes de la suite : la liste sans jeton dans le dossier de la suite, les jetons dans le Gestionnaire d'identifiants de Windows (service « SuperApp »). Chaque compte se partage module par module."));
    section(ui, tr("Comptes connectés"));
    if suite.core.accounts.list.is_empty() {
        ui.label(RichText::new(tr("Aucun compte.")).color(p.text_dim));
    }
    let mut remove = None;
    let mut shares = Vec::new();
    for a in &suite.core.accounts.list {
        egui::Frame::new().fill(p.surface).corner_radius(egui::CornerRadius::same(p.radius)).inner_margin(egui::Margin::same(8)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(RichText::new(a.title()).strong());
                ui.label(RichText::new(format!("{} {}", tr("depuis"), a.source)).color(p.text_dim).small());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button(tr("Retirer")).clicked() {
                        remove = Some(a.info.id.clone());
                    }
                });
            });
            ui.horizontal(|ui| {
                ui.label(RichText::new(tr("Partagé avec :")).color(p.text_dim));
                for m in FORGE_MODULES {
                    let mut on = a.info.shared_with.iter().any(|x| x == m);
                    let name = suite.core.slot(m).map(|s| s.info.name).unwrap_or(m);
                    if ui.checkbox(&mut on, name).changed() {
                        shares.push((a.info.id.clone(), m, on));
                    }
                }
            });
        });
        ui.add_space(4.0);
    }
    if let Some(id) = remove {
        suite.core.accounts.remove(&id, &suite.core.host);
    }
    for (id, m, on) in shares {
        suite.core.accounts.set_share(&id, m, on, &suite.core.host);
    }

    section(ui, tr("Ajouter un compte"));
    let st = &mut suite.shell.settings;
    ui.horizontal(|ui| {
        for k in ForgeKind::ALL {
            ui.radio_value(&mut st.new_kind, k, k.label());
        }
    });
    egui::Grid::new("nouveau-compte").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
        ui.label(tr("Hôte"));
        ui.add(egui::TextEdit::singleline(&mut st.new_host).hint_text(st.new_kind.default_host()));
        ui.end_row();
        ui.label(tr("Jeton (PAT)"));
        ui.add(egui::TextEdit::singleline(&mut st.new_token).password(true));
        ui.end_row();
        if st.new_kind == ForgeKind::AzureDevOps {
            ui.label(tr("Organisations"));
            ui.add(egui::TextEdit::singleline(&mut st.new_orgs).hint_text(tr("séparées par des virgules")));
            ui.end_row();
        }
    });
    let busy = suite.core.accounts.is_busy();
    ui.horizontal(|ui| {
        let can = !busy && !suite.shell.settings.new_token.trim().is_empty();
        if ui.add_enabled(can, egui::Button::new(tr("Vérifier et ajouter"))).clicked() {
            let st = &mut suite.shell.settings;
            let orgs = st.new_orgs.split(',').map(|o| o.trim().to_string()).filter(|o| !o.is_empty()).collect();
            suite.core.accounts.add(st.new_kind, &st.new_host, &st.new_token, orgs);
            st.new_token.clear();
        }
        if busy {
            ui.spinner();
            ui.label(RichText::new(tr("vérification auprès de la forge…")).color(p.text_dim));
        }
    });

    section(ui, tr("Importer"));
    hint(ui, p, tr("Copie la liste et les jetons ; DragonTree et Owlery lancés seuls gardent les leurs."));
    ui.horizontal(|ui| {
        if ui.button(tr("Depuis DragonTree")).clicked() {
            suite.core.accounts.import_dragontree(&suite.core.host);
        }
        if ui.button(tr("Depuis Owlery")).clicked() {
            suite.core.accounts.import_owlery(&suite.core.host);
        }
    });
    if let Some((ok, msg)) = &suite.core.accounts.message {
        ui.label(RichText::new(msg).color(if *ok { p.ok } else { p.danger }));
    }
    hint(ui, p, tr("Owlery lit déjà les comptes partagés avec lui (son coffre est injectable). DragonTree et Patronus les liront quand ils parleront à la suite ; les profils Claude de Grimoire sont un autre type de compte, prévu."));
}

fn tailscale(suite: &mut Suite, ui: &mut egui::Ui) {
    let p = suite.core.palette();
    ui.heading("Tailscale");
    hint(ui, p, tr("Un seul nœud pour la suite (et non un par module), pour la synchro mobile. Le compagnon de Fawkes devient celui de la suite ; Fawkes lancé seul garde son propre nœud."));
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
            ui.label(RichText::new(tr("Compagnon fawkes-tailnet.exe introuvable : compilez-le dans Fawkes (tools\\compagnon.ps1) ou indiquez-le dans FAWKES_TAILNET_EXE.")).color(p.warn));
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
            hint(ui, p, tr("Un routeur par module (/fawkes, /owlery…) est prévu derrière ce nœud."));
        }
        Some(suite_fawkes::tailnet::TailnetState::Failed(e)) => {
            ui.label(RichText::new(e).color(p.danger));
        }
        _ => {}
    }
}

fn shortcuts(suite: &mut Suite, ui: &mut egui::Ui) {
    let p = suite.core.palette();
    ui.heading(tr("Raccourcis"));
    hint(ui, p, tr("Le module qui a le clavier est celui de la case cliquée en dernier : les autres ne voient aucune frappe. Les raccourcis de la suite passent toujours avant ceux des modules."));
    section(ui, tr("La suite"));
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
                Owner::Suite => format!("{} ({t})", tr("la suite")),
                Owner::Module(m) => format!("{} ({t})", suite.core.slot(m).map(|s| s.info.name).unwrap_or(m)),
            })
            .collect();
        let color = if c.suite_wins { p.warn } else { p.text_muted };
        ui.label(RichText::new(format!("{}  —  {}", c.shortcut, owners.join(", "))).color(color));
    }
    hint(ui, p, tr("Entre deux modules, la même combinaison fait l'action du module qui a le clavier. Quand la suite la revendique, elle gagne : le module ne la reçoit plus."));
}

fn layouts(suite: &mut Suite, ui: &mut egui::Ui) {
    let p = suite.core.palette();
    ui.heading(tr("Dispositions"));
    hint(ui, p, tr("Comme les dispositions de fenêtre de Visual Studio : plusieurs dispositions nommées, et une par configuration d'écrans, appliquée automatiquement au branchement."));
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
    egui::ComboBox::from_id_salt("disposition-ecrans").selected_text(assigned.clone().unwrap_or_else(|| tr("aucune").to_string())).show_ui(ui, |ui| {
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

fn module_page(suite: &mut Suite, ui: &mut egui::Ui, id: &str) {
    let p = suite.core.palette();
    let Some(s) = suite.core.slots.iter_mut().find(|s| s.info.id == id) else { return };
    ui.heading(RichText::new(format!("{}  {}", s.info.icon, s.info.name)).color(s.info.color));
    hint(ui, p, s.info.summary);
    ui.add_space(6.0);
    match &s.info.origin {
        Origin::Native => {
            hint(ui, p, tr("Module natif : écrit pour la suite."));
        }
        Origin::Embedded { exe, repo } => {
            hint(ui, p, &format!("{} {exe}, {} {repo}. {}", tr("Application autonome embarquée sans changer son code :"), tr("sources dans"), tr("Ses réglages propres restent dans son propre menu, dans sa case ; ceux de la suite (thème, langue, comptes, Tailscale) s'appliquent là où il sait les suivre.")));
            section(ui, tr("Ce que la suite contient pour lui"));
            for line in [
                mark("style global (visuels, thème, zoom) rétabli après chacun de ses appels"),
                mark("clavier seulement quand sa case a été cliquée en dernier"),
                mark("commandes de fenêtre (titre, réduire, agrandir, fermer) relevées, sans effet sur la suite"),
                mark("sa logique tourne même quand sa case est masquée"),
                mark("une panique l'arrête, sans arrêter les autres"),
            ] {
                ui.label(RichText::new(format!("• {}", tr(line))).color(p.text_muted));
            }
            ui.label(RichText::new(format!(
                "{} : {} · {} : {}",
                tr("Style rétabli"),
                s.stats.style_restored,
                tr("commandes contenues"),
                s.stats.commands_countered
            )).color(p.text_dim).small());
        }
    }
    let host = suite.core.host.clone();
    if s.module().is_some() {
        section(ui, tr("Réglages propres"));
        let shown = s.module_mut().is_some_and(|m| m.settings_ui(ui, &host));
        if !shown {
            ui.label(RichText::new(tr("Dans le module lui-même (son menu Réglages, dans sa case).")).color(p.text_dim));
        }
    }
}
