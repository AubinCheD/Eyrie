//! Carnet : des notes par dépôt, et le journal du bus de la suite.
//!
//! C'est aussi le **gabarit de module** (voir `docs/ecrire-un-module.md`) : un module natif minimal
//! qui montre tout ce que l'interface permet — deux vues, une pastille, des commandes, une page de
//! réglages, les événements du bus, le contexte partagé, les demandes d'un autre module, un état
//! enregistré dans son dossier de la suite, ses écrans sur le téléphone.

use std::collections::{BTreeMap, VecDeque};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::PathBuf;
use std::time::SystemTime;

use eframe::egui::{self, Color32, RichText};
use serde::{Deserialize, Serialize};
use suite_sdk::host::{Event, Request, SharedContext};
use suite_sdk::mobile::ui::{Act, Row, Screen};
use suite_sdk::shortcut::Shortcut;
use suite_sdk::{
    Badge, BuildCx, CommandInfo, Cx, Host, Module, ModuleFactory, ModuleInfo, ModuleStatus, Origin, Tone, Unavailable,
    ViewInfo,
};

pub const ID: &str = "carnet";

pub fn info() -> ModuleInfo {
    ModuleInfo {
        id: ID,
        name: "Carnet",
        icon: "✎",
        logo: None,
        color: Color32::from_rgb(0xd9, 0xb4, 0x3c),
        summary: "Notes par dépôt et journal du bus d'Eyrie (module natif, gabarit)",
        origin: Origin::Native,
    }
}

pub fn factory() -> ModuleFactory {
    ModuleFactory { info: info(), build, fonts: None }
}

fn build(bcx: &BuildCx) -> Result<Box<dyn Module>, Unavailable> {
    Ok(Box::new(Carnet::open(bcx.host.clone())))
}

/// Ce qui est enregistré (`<suite>\modules\carnet\notes.json`).
#[derive(Serialize, Deserialize)]
#[serde(default)]
struct Saved {
    /// Dossier du dépôt (ou « » hors contexte) → texte.
    notes: BTreeMap<String, String>,
    follow_context: bool,
    #[serde(flatten)]
    extra: BTreeMap<String, serde_json::Value>,
}

impl Default for Saved {
    fn default() -> Self {
        // Suit le contexte partagé par défaut : les notes du dépôt ouvert dans DragonTree.
        Saved { notes: BTreeMap::new(), follow_context: true, extra: BTreeMap::new() }
    }
}

const VERSION: u32 = 1;

pub struct Carnet {
    info: ModuleInfo,
    host: Host,
    path: PathBuf,
    saved: Saved,
    dirty: bool,
    /// Le dépôt dont on lit les notes.
    current: String,
    events: VecDeque<(SystemTime, Event)>,
    unseen: u32,
    problems: Vec<String>,
}

impl Carnet {
    pub fn open(host: Host) -> Self {
        let path = host.module_dir(ID).join("notes.json");
        let loaded = suite_sdk::state::load_json::<Saved>(&path, VERSION);
        let current = host.context().map(|c| c.dir.display().to_string()).unwrap_or_default();
        Carnet {
            info: info(),
            host,
            path,
            saved: loaded.value,
            dirty: false,
            current,
            events: VecDeque::new(),
            unseen: 0,
            problems: loaded.problems,
        }
    }

    fn save(&mut self) {
        if let Err(e) = suite_sdk::state::save_json(&self.path, &self.saved, VERSION, env!("CARGO_PKG_VERSION")) {
            self.problems.push(format!("notes.json : {e}"));
        }
        self.dirty = false;
    }

    fn label(&self) -> String {
        repo_label(&self.current)
    }

    /// Ajoute une ligne aux notes du dépôt `dir` (demande `noter`, téléphone).
    fn append(&mut self, dir: &str, text: &str) {
        let note = self.saved.notes.entry(dir.to_string()).or_default();
        if !note.is_empty() && !note.ends_with('\n') {
            note.push('\n');
        }
        note.push_str(text);
        self.dirty = true;
    }

    /// Le dépôt d'un écran du téléphone : le courant pour l'accueil, celui de la clé pour `depot/<clé>`.
    fn mobile_dir(&self, path: &str) -> Option<String> {
        match path.strip_prefix("depot/") {
            Some(key) => self.saved.notes.keys().find(|d| repo_key(d) == key).cloned(),
            None if path.is_empty() => Some(self.current.clone()),
            None => None,
        }
    }

    fn notes_ui(&mut self, ui: &mut egui::Ui) {
        let p = self.host.palette();
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("✎ {}", self.label())).strong().color(p.text));
            ui.label(RichText::new(&self.current).color(p.text_dim).small());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                egui::ComboBox::from_id_salt("carnet-depots").selected_text("Autres notes").show_ui(ui, |ui| {
                    let keys: Vec<String> = self.saved.notes.keys().cloned().collect();
                    for k in keys {
                        let name = if k.is_empty() { "Hors dépôt".to_string() } else { k.clone() };
                        if ui.selectable_label(k == self.current, name).clicked() {
                            self.current = k;
                        }
                    }
                });
            });
        });
        if !self.saved.follow_context {
            ui.label(RichText::new("Ne suit pas le contexte partagé (réglages du Carnet).").color(p.text_dim).small());
        }
        let text = self.saved.notes.entry(self.current.clone()).or_default();
        let edit = egui::TextEdit::multiline(text)
            .hint_text("Notes pour ce dépôt : à faire, idées, commandes utiles…")
            .desired_width(f32::INFINITY)
            .desired_rows(16)
            .font(egui::TextStyle::Monospace);
        if ui.add_sized(ui.available_size(), edit).changed() {
            self.dirty = true;
        }
    }

    fn events_ui(&mut self, ui: &mut egui::Ui) {
        let p = self.host.palette();
        self.unseen = 0;
        ui.horizontal(|ui| {
            ui.label(RichText::new("Bus d'Eyrie").strong());
            ui.label(RichText::new("événements publiés par les modules et par Eyrie").color(p.text_dim).small());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("Effacer").clicked() {
                    self.events.clear();
                }
                if ui.small_button("Publier un essai").clicked() {
                    self.host.publish(Event {
                        source: ID.into(),
                        kind: "essai".into(),
                        data: serde_json::json!({"texte": "bonjour du Carnet"}),
                    });
                }
            });
        });
        ui.separator();
        egui::ScrollArea::vertical().auto_shrink(false).stick_to_bottom(true).show(ui, |ui| {
            if self.events.is_empty() {
                ui.label(
                    RichText::new(
                        "Rien pour l'instant. Choisissez un contexte dans la barre du haut, ou publiez un essai.",
                    )
                    .color(p.text_dim),
                );
            }
            for (at, e) in &self.events {
                let secs = SystemTime::now().duration_since(*at).map(|d| d.as_secs()).unwrap_or(0);
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(format!("{secs:>4} s")).monospace().color(p.text_dim).small());
                    ui.label(RichText::new(&e.source).color(p.accent));
                    ui.label(RichText::new(&e.kind).strong());
                    ui.label(RichText::new(e.data.to_string()).monospace().color(p.text_muted).small());
                });
            }
        });
    }
}

impl Module for Carnet {
    fn info(&self) -> &ModuleInfo {
        &self.info
    }

    fn views(&self) -> Vec<ViewInfo> {
        vec![ViewInfo::new("notes", "Notes"), ViewInfo::new("bus", "Bus")]
    }

    fn logic(&mut self, _ctx: &egui::Context, _cx: &mut Cx) {
        if self.dirty {
            self.save();
        }
        for p in std::mem::take(&mut self.problems) {
            self.host.notify(suite_sdk::Notification::new(ID, "Carnet", p, Tone::Warn));
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, view: &str, _cx: &mut Cx) {
        egui::Frame::new().inner_margin(egui::Margin::same(8)).show(ui, |ui| match view {
            "bus" => self.events_ui(ui),
            _ => self.notes_ui(ui),
        });
    }

    fn status(&self) -> ModuleStatus {
        let note = self
            .saved
            .notes
            .get(&self.current)
            .map(|t| t.lines().filter(|l| !l.trim().is_empty()).count())
            .unwrap_or(0);
        ModuleStatus {
            title: Some(format!("Carnet — {}", self.label())),
            badge: (self.unseen > 0)
                .then(|| Badge::count(self.unseen, Tone::Info, format!("{} événement(s) du bus non lus", self.unseen))),
            waiting: if note > 0 {
                vec![format!("{note} ligne(s) de notes pour {}", self.label())]
            } else {
                Vec::new()
            },
            ..Default::default()
        }
    }

    fn commands(&self) -> Vec<CommandInfo> {
        vec![
            CommandInfo { id: "hors_depot".into(), title: "Notes hors dépôt".into(), shortcut: None },
            CommandInfo {
                id: "effacer_bus".into(),
                title: "Effacer le journal du bus".into(),
                shortcut: Shortcut::parse("Ctrl+Maj+K"),
            },
        ]
    }

    fn run_command(&mut self, id: &str, _cx: &mut Cx) {
        match id {
            "hors_depot" => self.current.clear(),
            "effacer_bus" => {
                self.events.clear();
                self.unseen = 0;
            }
            _ => {}
        }
    }

    fn settings_ui(&mut self, ui: &mut egui::Ui, host: &Host) -> bool {
        if ui.checkbox(&mut self.saved.follow_context, "Suivre le contexte partagé (dépôt courant)").changed() {
            self.dirty = true;
        }
        ui.label(
            RichText::new(format!("Notes enregistrées dans {}", host.module_dir(ID).display()))
                .color(host.palette().text_dim)
                .small(),
        );
        true
    }

    fn on_event(&mut self, event: &Event, _cx: &mut Cx) {
        if event.kind == "contexte" && self.saved.follow_context {
            let ctx: Option<SharedContext> = serde_json::from_value(event.data.clone()).ok().flatten();
            self.current = ctx.map(|c| c.dir.display().to_string()).unwrap_or_default();
        }
        if self.events.len() == 200 {
            self.events.pop_front();
        }
        self.events.push_back((SystemTime::now(), event.clone()));
        self.unseen += 1;
    }

    /// `noter{dir, texte}` : un autre module ajoute une ligne aux notes d'un dépôt.
    fn on_request(&mut self, request: &Request, _cx: &mut Cx) -> Result<serde_json::Value, String> {
        match request.demande.as_str() {
            "noter" => {
                let dir = request.data.get("dir").and_then(|v| v.as_str()).unwrap_or_default().to_string();
                let text = request.data.get("texte").and_then(|v| v.as_str()).ok_or("texte manquant")?;
                self.append(&dir, text);
                Ok(serde_json::json!({"ok": true}))
            }
            other => Err(format!("demande inconnue : {other}")),
        }
    }

    fn on_exit(&mut self) {
        if self.dirty {
            self.save();
        }
    }

    /// Accueil : les notes du dépôt courant et les autres dépôts qui en ont ; `depot/<clé>` : les notes d'un dépôt.
    fn mobile_screen(&mut self, path: &str, _cx: &mut Cx) -> Option<Screen> {
        let dir = self.mobile_dir(path)?;
        Some(notes_screen(&dir, &self.saved.notes, path.is_empty()))
    }

    fn mobile_action(
        &mut self,
        path: &str,
        action: &str,
        input: Option<&str>,
        _cx: &mut Cx,
    ) -> Result<Option<String>, String> {
        if action != "ajouter" {
            return Err("action inconnue".into());
        }
        let dir = self.mobile_dir(path).ok_or("Ce dépôt n'a plus de notes.")?;
        let text =
            input.map(str::trim_end).filter(|t| !t.trim().is_empty()).ok_or("Ligne vide : rien n'est ajouté.")?;
        self.append(&dir, text);
        Ok(Some("Ligne ajoutée".into()))
    }
}

/// Nom court d'un dépôt : son dossier, ou « Hors dépôt ».
fn repo_label(dir: &str) -> String {
    if dir.is_empty() {
        "Hors dépôt".into()
    } else {
        std::path::Path::new(dir).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| dir.into())
    }
}

/// Empreinte courte d'un dépôt, pour le chemin de son écran sur le téléphone (`depot/<clé>`) : pas de chemin Windows
/// dans un chemin d'écran, et la même clé quand la liste des dépôts change.
fn repo_key(dir: &str) -> String {
    let mut h = DefaultHasher::new();
    dir.hash(&mut h);
    format!("{:x}", h.finish() & 0xffff_ffff)
}

fn line_count(text: &str) -> usize {
    text.lines().filter(|l| !l.trim().is_empty()).count()
}

/// Les notes de `dir` sur le téléphone ; `home` : l'accueil du module, avec les autres dépôts qui ont des notes.
fn notes_screen(dir: &str, notes: &BTreeMap<String, String>, home: bool) -> Screen {
    let label = repo_label(dir);
    let mut s = if home {
        Screen::new("Carnet").subtitle(label)
    } else {
        Screen::new(label).subtitle(if dir.is_empty() { "Notes prises hors de tout dépôt" } else { dir })
    };
    match notes.get(dir).filter(|t| !t.trim().is_empty()) {
        Some(text) => s.text(text.trim_end(), false),
        None => s.empty("Pas encore de notes pour ce dépôt."),
    };
    let add = Act::new("ajouter", "Ajouter une ligne").glyph("➕").primary().input("Nouvelle ligne", "À faire, idée…");
    s.buttons(vec![add]);
    if home {
        let others: Vec<(&String, usize)> = notes
            .iter()
            .filter(|(d, _)| d.as_str() != dir)
            .map(|(d, t)| (d, line_count(t)))
            .filter(|(_, n)| *n > 0)
            .collect();
        if !others.is_empty() {
            s.section("Autres dépôts", others.len().to_string());
            for (d, n) in others {
                let lines = if n == 1 { "1 ligne".to_string() } else { format!("{n} lignes") };
                s.row(
                    Row::new(repo_label(d))
                        .subtitle(d.as_str())
                        .detail(lines)
                        .glyph("✏️")
                        .open(format!("depot/{}", repo_key(d))),
                );
            }
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use suite_sdk::mobile::ui::Block;

    use super::*;

    fn host(name: &str) -> Host {
        let dir = std::env::temp_dir().join(format!("suite-carnet-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        Host::new(egui::Context::default(), dir, suite_sdk::theme::ThemeChoice::Graphite)
    }

    #[test]
    fn follows_the_shared_context() {
        let mut c = Carnet::open(host("contexte"));
        c.saved.follow_context = true;
        let ctx = SharedContext { dir: "C:/dev/ozmo".into(), label: "ozmo".into(), source: "suite".into() };
        let ev =
            Event { source: "suite".into(), kind: "contexte".into(), data: serde_json::to_value(Some(ctx)).unwrap() };
        let mut frame = eframe::Frame::_new_kittest();
        let h = c.host.clone();
        let mut cx = Cx { host: &h, frame: &mut frame, active: false, visible: false };
        c.on_event(&ev, &mut cx);
        assert_eq!(c.current, "C:/dev/ozmo");
        assert_eq!(c.label(), "ozmo");
        assert_eq!(c.unseen, 1);
    }

    #[test]
    fn another_module_can_add_a_note_and_it_is_saved() {
        let h = host("noter");
        let mut c = Carnet::open(h.clone());
        let req = Request {
            from: "owlery".into(),
            to: ID.into(),
            demande: "noter".into(),
            data: serde_json::json!({"dir": "C:/dev/ozmo", "texte": "relire la PR 12"}),
        };
        let mut frame = eframe::Frame::_new_kittest();
        let mut cx = Cx { host: &h, frame: &mut frame, active: false, visible: false };
        assert!(c.on_request(&req, &mut cx).is_ok());
        c.on_exit();
        let again = Carnet::open(h);
        assert_eq!(again.saved.notes.get("C:/dev/ozmo").map(String::as_str), Some("relire la PR 12"));
    }

    #[test]
    fn the_phone_sees_the_current_notes_and_the_other_repos() {
        let notes = BTreeMap::from([
            ("C:/dev/ozmo".to_string(), "relire la PR 12\nlancer les tests\n".to_string()),
            ("C:/dev/diapaz".to_string(), "migration".to_string()),
            ("C:/dev/vide".to_string(), "  \n".to_string()),
        ]);
        let s = notes_screen("C:/dev/ozmo", &notes, true);
        assert_eq!((s.title.as_str(), s.subtitle.as_str()), ("Carnet", "ozmo"));
        assert!(s.blocks.contains(&Block::Text { text: "relire la PR 12\nlancer les tests".into(), mono: false }));
        assert_eq!(s.action_ids().into_iter().collect::<Vec<_>>(), ["ajouter"]);
        // Les dépôts sans notes n'apparaissent pas.
        assert_eq!(s.opens().into_iter().collect::<Vec<_>>(), [format!("depot/{}", repo_key("C:/dev/diapaz"))]);

        let empty = notes_screen("", &notes, false);
        assert_eq!(empty.title, "Hors dépôt");
        assert!(empty.blocks.iter().any(|b| matches!(b, Block::Empty { .. })));
        assert!(empty.opens().is_empty(), "pas d'autres dépôts hors de l'accueil");
    }

    #[test]
    fn a_line_added_from_the_phone_goes_to_the_right_repo() {
        let h = host("telephone");
        let mut c = Carnet::open(h.clone());
        c.current = "C:/dev/ozmo".into();
        c.saved.notes.insert("C:/dev/diapaz".into(), "migration".into());
        let mut frame = eframe::Frame::_new_kittest();
        let mut cx = Cx { host: &h, frame: &mut frame, active: false, visible: false };
        assert_eq!(c.mobile_action("", "ajouter", Some("relire la PR 12"), &mut cx), Ok(Some("Ligne ajoutée".into())));
        let other = format!("depot/{}", repo_key("C:/dev/diapaz"));
        assert!(c.mobile_screen(&other, &mut cx).is_some());
        assert!(c.mobile_action(&other, "ajouter", Some("  "), &mut cx).is_err(), "ligne vide refusée");
        assert!(c.mobile_action(&other, "ajouter", Some("garder les données"), &mut cx).is_ok());
        assert!(c.mobile_screen("depot/inconnu", &mut cx).is_none());
        assert_eq!(c.saved.notes["C:/dev/ozmo"], "relire la PR 12");
        assert_eq!(c.saved.notes["C:/dev/diapaz"], "migration\ngarder les données");
    }
}
