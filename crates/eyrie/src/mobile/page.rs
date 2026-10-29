//! Ce que le pont montre sur le PC : la page Téléphones des réglages (interrupteur, démarrage depuis le téléphone, QR
//! code d'appairage, appareils et leurs droits, connexions, journal), la confirmation d'un appairage, et l'indicateur
//! de la barre d'état.

use std::path::Path;
use std::time::{Duration, Instant, UNIX_EPOCH};

use eframe::egui::{self, RichText};
use suite_mobile::msg::Rights;

use crate::app::{Action, Suite};
use crate::i18n::tr;
use crate::settings::{Page, SettingsCx};
use crate::veille;

/// Ce que la page garde d'une image à l'autre.
#[derive(Default)]
pub struct PageState {
    /// Le QR code du ticket montré (son lien, sa texture).
    qr: Option<(String, egui::TextureHandle)>,
    /// Appareil dont le retrait attend confirmation.
    confirm_revoke: Option<String>,
    journal_open: bool,
    /// Message d'un ticket qui n'a pas pu être créé.
    ticket_error: Option<String>,
    /// `veille-echec.json`, relu toutes les deux secondes au plus.
    standby_failure: Option<(Instant, Option<veille::Failure>)>,
    /// La valeur Run désactivée dans le Gestionnaire des tâches, relu toutes les deux secondes au plus.
    run_disabled: Option<(Instant, bool)>,
}

fn ago(secs: u64) -> String {
    crate::core::ago(UNIX_EPOCH + Duration::from_secs(secs))
}

/// Le QR code d'un lien, en image (modules noirs sur blanc, marge de quatre modules).
fn qr_image(text: &str) -> Option<egui::ColorImage> {
    let code = qrcode::QrCode::with_error_correction_level(text.as_bytes(), qrcode::EcLevel::M).ok()?;
    let width = code.width();
    let colors = code.to_colors();
    const SCALE: usize = 6;
    const QUIET: usize = 4;
    let side = (width + 2 * QUIET) * SCALE;
    let mut pixels = vec![egui::Color32::WHITE; side * side];
    for (i, c) in colors.iter().enumerate() {
        if *c != qrcode::Color::Dark {
            continue;
        }
        let (x, y) = (i % width + QUIET, i / width + QUIET);
        for dy in 0..SCALE {
            let row = (y * SCALE + dy) * side;
            for dx in 0..SCALE {
                pixels[row + x * SCALE + dx] = egui::Color32::BLACK;
            }
        }
    }
    Some(egui::ColorImage::new([side, side], pixels))
}

pub fn page(suite: &mut SettingsCx<'_>, ui: &mut egui::Ui) {
    let p = suite.core.palette();
    ui.heading(tr("Téléphones"));
    ui.add(
        egui::Label::new(
            RichText::new(tr("Pilotez Eyrie depuis l'application mobile, par Tailscale. Tout ce que fait le téléphone se fait ici, sur le PC."))
                .color(p.text_dim),
        )
        .wrap(),
    );
    ui.add_space(8.0);

    let was = suite.core.st.mobile.enabled;
    ui.checkbox(&mut suite.core.st.mobile.enabled, tr("Accès depuis le téléphone"))
        .on_hover_text(tr("Coupé : plus aucune connexion, plus d'appairage ; le compagnon ne relaie plus rien."));
    if was && !suite.core.st.mobile.enabled {
        suite.core.mobile.stop("Accès depuis le téléphone coupé sur le PC.");
    }
    let mobile = &suite.core.mobile;
    if let Some(e) = &mobile.error {
        ui.label(RichText::new(e).color(p.danger));
        if ui.button(tr("Réessayer")).clicked() {
            suite.core.mobile.retry();
        }
    } else if suite.core.st.mobile.enabled {
        let tailnet = suite.core.tailnet.state();
        let line = match (&tailnet, mobile.local()) {
            (_, true) => (
                format!(
                    "{} {}",
                    tr("Écoute directe (essais) :"),
                    mobile.listening_on().map(|a| a.to_string()).unwrap_or_default()
                ),
                p.warn,
            ),
            (Some(suite_fawkes::tailnet::TailnetState::Running { name, .. }), _) if suite.core.tailnet.relays() => {
                (format!("{} {name}:{}", tr("Joignable sur le tailnet :"), suite_mobile::PORT), p.ok)
            }
            (Some(suite_fawkes::tailnet::TailnetState::Running { .. }), _)
                if suite.core.tailnet.own_companion() == Some(false) =>
            {
                (tr("Le compagnon en marche est celui de Fawkes : il ne relaie pas le téléphone. Compilez eyrie-tailnet.exe (tools\\compagnon.ps1).").to_string(), p.warn)
            }
            (Some(suite_fawkes::tailnet::TailnetState::Running { .. }), _) => {
                (tr("Le relais du téléphone démarre…").to_string(), p.text_dim)
            }
            _ => (tr("Tailscale n'est pas connecté : activez-le dans la page Tailscale.").to_string(), p.warn),
        };
        ui.label(RichText::new(line.0).color(line.1));
    }
    if let Some(problem) = &suite.core.mobile.store.problem {
        ui.label(RichText::new(problem).color(p.warn));
    }
    ui.add_space(6.0);
    standby(suite, ui);

    // --- Appairer ---
    ui.add_space(10.0);
    ui.label(RichText::new(tr("Appairer un téléphone")).strong().size(14.5));
    match &suite.core.mobile.ticket {
        None => {
            ui.add(
                egui::Label::new(
                    RichText::new(tr("Installez l'application Eyrie sur le téléphone et connectez-la à Tailscale avec le même compte que ce PC (elle le fait elle-même), puis scannez le code."))
                        .color(p.text_dim),
                )
                .wrap(),
            );
            // Le code montré a servi à un appairage abandonné trop près de sa fin pour qu'un autre le remplace.
            if suite.core.mobile.ticket_expired {
                ui.label(RichText::new(tr("Code expiré : demandez-en un nouveau.")).color(p.warn));
            }
            if ui.button(format!("📱 {}", tr("Afficher le code d'appairage"))).clicked() {
                suite.core.st.mobile.enabled = true;
                suite.core.mobile.ensure(true);
                let node = match suite.core.tailnet.state() {
                    Some(suite_fawkes::tailnet::TailnetState::Running { name, ip }) => Some((name, ip)),
                    _ => None,
                };
                suite.core.mobile.page.ticket_error = suite.core.mobile.new_ticket(node).err();
            }
            if let Some(e) = &suite.core.mobile.page.ticket_error {
                ui.label(RichText::new(e).color(p.warn));
            }
        }
        Some(t) => {
            let (uri, code, host, left) = (t.uri.clone(), t.code.clone(), t.host.clone(), t.remaining());
            // Le code a changé sous les yeux de l'utilisateur : la page dit pourquoi.
            if t.renewed {
                ui.label(
                    RichText::new(tr("Nouveau code : le précédent a servi à un appairage abandonné.")).color(p.warn),
                );
            }
            let state = &mut suite.core.mobile.page;
            if state.qr.as_ref().is_none_or(|(u, _)| *u != uri)
                && let Some(img) = qr_image(&uri)
            {
                let tex = ui.ctx().load_texture("mobile-qr", img, egui::TextureOptions::NEAREST);
                state.qr = Some((uri.clone(), tex));
            }
            let qr = state.qr.as_ref().map(|(_, t)| t.clone());
            let mut cancel = false;
            ui.horizontal(|ui| {
                if let Some(tex) = &qr {
                    ui.add(egui::Image::new(tex).fit_to_exact_size(egui::vec2(220.0, 220.0)));
                }
                ui.vertical(|ui| {
                    ui.label(tr("Dans l'application : « Scanner le QR code ».")).on_hover_text(&uri);
                    ui.add_space(6.0);
                    ui.label(RichText::new(tr("Ou saisissez :")).color(p.text_dim));
                    ui.label(RichText::new(format!("{} {host}", tr("PC :"))).monospace().size(15.0));
                    ui.label(RichText::new(format!("{} {code}", tr("Code :"))).monospace().size(18.0).strong());
                    ui.add_space(6.0);
                    let secs = left.as_secs();
                    ui.label(
                        RichText::new(format!("{} {}:{:02}", tr("Valable encore"), secs / 60, secs % 60))
                            .color(if secs < 60 { p.warn } else { p.text_dim }),
                    );
                    ui.ctx().request_repaint_after(Duration::from_secs(1));
                    if ui.button(tr("Annuler")).clicked() {
                        cancel = true;
                    }
                });
            });
            if cancel {
                suite.core.mobile.cancel_ticket();
            }
        }
    }

    // --- Appareils ---
    ui.add_space(10.0);
    ui.label(RichText::new(tr("Appareils appairés")).strong().size(14.5));
    if suite.core.mobile.store.devices.is_empty() {
        ui.label(RichText::new(tr("Aucun pour l'instant.")).color(p.text_dim));
    }
    let mut revoke = None;
    let mut rights_change: Vec<(String, Rights)> = Vec::new();
    let connected: Vec<String> = suite.core.mobile.sessions.iter().map(|s| s.device.clone()).collect();
    let confirm = suite.core.mobile.page.confirm_revoke.clone();
    egui::Grid::new("mobile-appareils").num_columns(4).spacing([14.0, 6.0]).striped(true).show(ui, |ui| {
        for d in &suite.core.mobile.store.devices {
            let online = connected.contains(&d.id);
            ui.horizontal(|ui| {
                ui.label(RichText::new("●").color(if online { p.ok } else { p.text_dim })).on_hover_text(if online {
                    tr("Connecté")
                } else {
                    tr("Hors ligne")
                });
                ui.label(RichText::new(&d.name).strong()).on_hover_text(format!(
                    "{}\n{} {}\n{} {}",
                    d.model,
                    tr("Tailscale :"),
                    if d.login.is_empty() { "—" } else { &d.login },
                    tr("Empreinte :"),
                    d.key_bytes().map(|k| suite_mobile::pairing::fingerprint(&k)).unwrap_or_default()
                ));
            });
            ui.label(
                RichText::new(format!(
                    "{} {} · {} {}",
                    tr("appairé"),
                    ago(d.paired_at),
                    tr("vu"),
                    if online { tr("maintenant").to_string() } else { ago(d.last_seen) }
                ))
                .color(p.text_dim),
            );
            let mut r = d.rights;
            ui.horizontal(|ui| {
                ui.checkbox(&mut r.act, tr("Agir")).on_hover_text(tr("Actions des modules et commandes"));
                ui.checkbox(&mut r.terminals, tr("Terminaux"))
                    .on_hover_text(tr("Écrire dans les terminaux, en ouvrir"));
                // Parcourir les dossiers ne sert qu'à une action : sans « Agir », la case est grisée.
                let files = tr("Parcourir les dossiers du PC pour une action qui en demande un (cloner, ajouter un dépôt, ouvrir un terminal) ; avec « Agir »");
                ui.add_enabled(r.act, egui::Checkbox::new(&mut r.files, tr("Dossiers du PC")))
                    .on_hover_text(files)
                    .on_disabled_hover_text(files);
            });
            if r != d.rights {
                rights_change.push((d.id.clone(), r));
            }
            if confirm.as_deref() == Some(d.id.as_str()) {
                ui.horizontal(|ui| {
                    if ui.button(RichText::new(tr("Retirer")).color(p.danger)).clicked() {
                        revoke = Some(d.id.clone());
                    }
                    if ui.button(tr("Garder")).clicked() {
                        revoke = Some(String::new());
                    }
                });
            } else if ui.button(tr("Retirer…")).clicked() {
                suite.core.mobile.page.confirm_revoke = Some(d.id.clone());
            }
            ui.end_row();
        }
    });
    for (id, r) in rights_change {
        suite.core.mobile.set_rights(&id, r);
    }
    match revoke {
        Some(id) if id.is_empty() => suite.core.mobile.page.confirm_revoke = None,
        Some(id) => {
            suite.core.mobile.revoke(&id);
            suite.core.mobile.page.confirm_revoke = None;
        }
        None => {}
    }

    // --- Connexions et journal ---
    if !suite.core.mobile.sessions.is_empty() {
        ui.add_space(10.0);
        ui.label(RichText::new(tr("Connectés maintenant")).strong().size(14.5));
        for s in &suite.core.mobile.sessions {
            let what = match s.watching() {
                Some((m, "")) => m.to_string(),
                Some((m, path)) => format!("{m} › {path}"),
                None => "—".into(),
            };
            let since = s.since.elapsed().as_secs() / 60;
            ui.label(format!(
                "📱 {} · {} {} · {} {} · {} {since} min",
                s.name,
                tr("depuis"),
                if s.identity.node.is_empty() { &s.identity.addr } else { &s.identity.node },
                tr("regarde"),
                what,
                tr("connecté depuis")
            ))
            .on_hover_text(&s.model);
        }
    }
    ui.add_space(10.0);
    ui.checkbox(&mut suite.core.st.mobile.show_actions, tr("Montrer ici ce que fait le téléphone (bulles)"));
    ui.add_space(6.0);
    let open = &mut suite.core.mobile.page.journal_open;
    egui::CollapsingHeader::new(tr("Journal")).open(Some(*open)).show(ui, |ui| {
        let lines = suite.core.mobile.store.read_journal(150);
        if lines.is_empty() {
            ui.label(RichText::new(tr("Vide.")).color(p.text_dim));
        }
        egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
            for l in lines {
                let color = if l.kind == "refus" { p.warn } else { p.text };
                ui.label(
                    RichText::new(format!("{} · {} · {} · {}", ago(l.at), l.device, l.kind, l.text))
                        .color(color)
                        .small(),
                );
            }
        });
    });
    if ui.small_button(if *open { tr("Masquer le journal") } else { tr("Voir le journal") }).clicked() {
        suite.core.mobile.page.journal_open = !suite.core.mobile.page.journal_open;
    }
}

/// Pourquoi le veilleur ne tourne pas, s'il l'a écrit (`veille-echec.json`) ; le fichier est relu toutes les deux
/// secondes au plus.
fn standby_failure(state: &mut PageState, node: Option<&Path>) -> Option<veille::Failure> {
    if state.standby_failure.as_ref().is_none_or(|(at, _)| at.elapsed() >= Duration::from_secs(2)) {
        state.standby_failure = Some((Instant::now(), node.and_then(veille::read_failure)));
    }
    state.standby_failure.as_ref().and_then(|(_, f)| f.clone())
}

/// « Démarrer Eyrie depuis le téléphone » : la case, l'état du veilleur, l'ouverture de session, son dernier échec.
fn standby(suite: &mut SettingsCx<'_>, ui: &mut egui::Ui) {
    let p = suite.core.palette();
    ui.checkbox(&mut suite.core.st.mobile.wake, tr("Démarrer Eyrie depuis le téléphone")).on_hover_text(tr(
        "Le veilleur du PC garde Eyrie joignable sur le tailnet quand elle est fermée : un téléphone appairé qui a « Agir » peut alors la démarrer (elle s'ouvre réduite). Le PC doit être allumé, éveillé, et la session Windows ouverte (verrouillée, ça va).",
    ));
    let line = |ui: &mut egui::Ui, text: &str, color| {
        ui.add(egui::Label::new(RichText::new(text).color(color)).wrap());
    };
    // Eyrie ne donne la clé de démarrage qu'aux téléphones qui peuvent agir, et le veilleur refuse les autres.
    line(ui, tr("Seuls les téléphones qui ont « Agir » peuvent démarrer Eyrie."), p.text_dim);
    if !suite.core.st.mobile.wake {
        return;
    }
    if !suite.core.st.tailnet.enabled {
        line(ui, tr("Tailscale est coupé, le veilleur aussi : activez-le dans la page Tailscale."), p.warn);
        return;
    }
    let tailnet = &suite.core.tailnet;
    let attached = tailnet.standby_attached();
    let mut retry = false;
    if let Some(problem) = &tailnet.veille.problem {
        match &tailnet.veille.kept {
            // Une Eyrie rangée ailleurs que le veilleur (celle de target\debug) : celui de l'autre reste inscrit.
            Some(exe) => {
                line(ui, &format!("{} {problem}", tr("Cette Eyrie ne peut pas lancer le veilleur :")), p.warn);
                line(
                    ui,
                    &format!(
                        "{} {}",
                        tr("Celui qu'une autre Eyrie a inscrit à l'ouverture de session reste en place :"),
                        exe.display()
                    ),
                    p.text_dim,
                );
            }
            None => line(ui, &format!("{} {problem}", tr("Impossible :")), p.danger),
        }
        retry = ui.button(tr("Réessayer")).clicked();
    } else if attached {
        line(ui, tr("Le veilleur tient le nœud : Eyrie fermée, le téléphone pourra la démarrer."), p.ok);
        if !suite.core.st.mobile.enabled {
            line(ui, tr("Accès depuis le téléphone coupé : le veilleur refuse de démarrer Eyrie."), p.warn);
        }
    } else if let Some(why) = tailnet.standby_fallback() {
        line(ui, &format!("{} {why}", tr("Le veilleur ne tourne pas :")), p.warn);
        retry = ui.button(tr("Réessayer")).clicked();
    } else {
        line(ui, tr("Le veilleur démarre…"), p.text_dim);
    }
    if let Some(e) = &tailnet.veille.error {
        line(ui, e, p.warn);
    } else if tailnet.veille.problem.is_none() && tailnet.veille.run.is_some() {
        let page = &mut suite.core.mobile.page;
        if page.run_disabled.is_none_or(|(at, _)| at.elapsed() >= Duration::from_secs(2)) {
            page.run_disabled = Some((Instant::now(), tailnet.os.run_disabled()));
        }
        if page.run_disabled.is_some_and(|(_, off)| off) {
            line(
                ui,
                tr(
                    "Désactivé dans le Gestionnaire des tâches (onglet Démarrage) : le veilleur ne démarrera pas à l'ouverture de session. Réactivez-le là-bas.",
                ),
                p.warn,
            );
        } else {
            line(ui, tr("Lancé à chaque ouverture de session Windows."), p.text_dim);
        }
    }
    let node = tailnet.node_path().map(Path::to_path_buf);
    if !attached && let Some(f) = standby_failure(&mut suite.core.mobile.page, node.as_deref()) {
        line(ui, &format!("{} ({}) : {}", tr("Dernier échec du veilleur"), ago(f.at), f.message), p.warn);
    }
    if retry {
        suite.core.tailnet.retry_standby();
    }
}

/// La confirmation d'un appairage, par-dessus tout : le nom, le compte Tailscale, et le code à comparer.
pub fn confirm(suite: &mut Suite, ctx: &egui::Context) {
    let Some(r) = suite.core.mobile.requests.first() else { return };
    let p = suite.core.palette();
    let mut answer = None;
    egui::Modal::new(egui::Id::unique("mobile-appairage")).show(ctx, |ui| {
        ui.set_max_width(420.0);
        ui.heading(tr("Appairer ce téléphone ?"));
        ui.add_space(4.0);
        ui.label(RichText::new(&r.name).strong().size(16.0));
        if !r.model.is_empty() {
            ui.label(RichText::new(&r.model).color(p.text_dim));
        }
        if !r.identity.login.is_empty() {
            ui.label(format!("{} {} ({})", tr("Tailscale :"), r.identity.login, r.identity.node));
        }
        ui.add_space(8.0);
        ui.label(tr("Le téléphone doit afficher le même code :"));
        ui.label(RichText::new(&r.sas).monospace().size(30.0).strong().color(p.accent));
        ui.add_space(4.0);
        ui.add(
            egui::Label::new(
                RichText::new(tr("Appairé, il pourra voir les modules, agir et écrire dans les terminaux (droits réglables ensuite)."))
                    .color(p.text_dim),
            )
            .wrap(),
        );
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if ui.button(RichText::new(tr("Appairer")).strong()).clicked() {
                answer = Some(true);
            }
            if ui.button(tr("Refuser")).clicked() {
                answer = Some(false);
            }
        });
    });
    if let Some(yes) = answer {
        suite.core.mobile.decide(0, yes);
    }
}

/// L'indicateur de la barre d'état : les téléphones connectés ; un clic mène à la page Téléphones.
pub fn status_item(suite: &mut Suite, ui: &mut egui::Ui) {
    let names = suite.core.mobile.connected().join(", ");
    if names.is_empty() {
        return;
    }
    let p = suite.core.palette();
    let n = suite.core.mobile.sessions.len();
    let text = RichText::new(format!("📱 {n}")).small().color(p.ok);
    if ui.add(egui::Button::new(text).frame(false)).on_hover_text(format!("{} {names}", tr("Connectés :"))).clicked() {
        suite.core.act(Action::OpenSettings(Page::Phones));
    }
    ui.separator();
}
