//! Owlery sur le téléphone : la boîte de réception groupée comme sur le PC (par dépôt par défaut, ou par sorte, selon
//! son réglage), le détail d'une entrée, et les gestes de la liste du PC : marquer lue ou non lue, traitée, reportée
//! (3 h, demain, lundi), tout un groupe marqué lu, ignorer le dépôt, annuler le dernier geste, corriger avec Claude (une
//! session dans Fawkes, que le téléphone ouvre ensuite), ouvrir sur le téléphone ou sur le PC, actualiser.
//!
//! Tout se lit dans l'état vivant d'Owlery (`App::state`, groupé par `Inbox::view` comme la liste du PC) et passe par
//! ses propres méthodes (`mark`, `mute`, `undo`, `work_with_claude`) : le PC montre aussitôt le même changement.
//! « Actualiser » passe par le canal des demandes d'Owlery (`contract::Incoming::Refresh`).

use std::sync::mpsc::Sender;
use std::time::{SystemTime, UNIX_EPOCH};

use owlery::app::App;
use owlery::contract::{Incoming, kind_id};
use owlery::inbox::{Filter, Group, GroupBy, Inbox, Rules, Snooze};
use owlery::model::{Ci, Item, Kind, Status};
use suite_sdk::mobile::msg::MODULE_PREFIX;
use suite_sdk::mobile::ui::{Act, Band, Row, Screen, Tone};

/// Entrées montrées par groupe dans l'accueil ; toutes sont sur l'écran du groupe (`groupe/<clé>`).
const PER_GROUP: usize = 15;

/// Gestes qu'Owlery garde à annuler (son `UNDO_DEPTH`, privé) : au-delà, les plus anciens ne s'annulent plus.
const UNDO_KEPT: usize = 50;

/// Les reports proposés, comme « Me le rappeler » sur le PC : (action, report, libellé).
const SNOOZES: [(&str, Snooze, &str); 3] = [
    ("reporter-3h", Snooze::Later, "Reporter de 3 h"),
    ("reporter", Snooze::Tomorrow, "Reporter à demain matin"),
    ("reporter-lundi", Snooze::Monday, "Reporter à lundi matin"),
];

fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// Le glyphe d'une sorte d'entrée.
fn kind_glyph(k: Kind) -> &'static str {
    match k {
        Kind::ReviewRequested => "👀",
        Kind::CiFailed => "❌",
        Kind::Mention => "💬",
        Kind::Assigned => "📌",
        Kind::MyPr => "🔀",
    }
}

pub fn ago(t: i64, now: i64) -> String {
    let s = (now - t).max(0);
    match s {
        0..60 => "à l'instant".into(),
        60..3600 => format!("il y a {} min", s / 60),
        3600..86_400 => format!("il y a {} h", s / 3600),
        _ => format!("il y a {} j", s / 86_400),
    }
}

fn reference(i: &Item) -> String {
    match i.number {
        Some(n) => format!("{}#{n}", i.repo),
        None => i.repo.clone(),
    }
}

fn ci_tone(ci: Option<Ci>) -> Option<Tone> {
    match ci? {
        Ci::Failure => Some(Tone::Danger),
        Ci::Pending => Some(Tone::Warn),
        Ci::Success => Some(Tone::Ok),
    }
}

fn unread_text(n: usize) -> String {
    if n == 1 { "1 non lue".into() } else { format!("{n} non lues") }
}

fn entries_text(n: usize) -> String {
    if n == 1 { "1 entrée".into() } else { format!("{n} entrées") }
}

/// Les groupes de la liste, comme sur le PC (non lues et lues ; ni terminées, ni reportées, ni dépôts ignorés).
fn groups<'a>(inbox: &'a Inbox, rules: &Rules, by: GroupBy, now: i64) -> Vec<Group<'a>> {
    inbox.view(&Filter::default(), rules, by, now)
}

/// La clé d'un groupe (son bandeau, son écran) : le dépôt, ou la sorte.
fn group_key(g: &Group, by: GroupBy) -> String {
    match by {
        GroupBy::Repo => g.title.clone(),
        GroupBy::Kind => g.entries.first().map(|(i, _)| kind_id(i.kind).to_string()).unwrap_or_default(),
    }
}

/// Une entrée dans la liste ; le groupe dit déjà le dépôt (ou la sorte), la ligne dit l'autre.
fn item_row(item: &Item, status: Status, by: GroupBy, now: i64) -> Row {
    let place = match (by, item.number) {
        (GroupBy::Repo, Some(n)) => format!("{} · #{n}", item.kind.label()),
        (GroupBy::Repo, None) => item.kind.label().to_string(),
        (GroupBy::Kind, _) => reference(item),
    };
    let sub = if item.author.is_empty() { place } else { format!("{place} · {}", item.author) };
    let id = &item.id;
    let mut row = Row::new(&item.title)
        .key(id)
        .subtitle(sub)
        .detail(ago(item.updated_at, now))
        .glyph(kind_glyph(item.kind))
        .open(format!("item/{id}"));
    if let Some(t) = ci_tone(item.ci) {
        row = row.tone(t);
    }
    if owlery::actions::is_safe_url(&item.url) {
        row = row.act(Act::link("Ouvrir", &item.url));
    }
    if status == Status::Unread {
        row = row.badge("non lu").act(Act::new(format!("lu:{id}"), "Marquer lue").glyph("✅"));
    } else {
        row = row.act(Act::new(format!("non-lu:{id}"), "Marquer non lue").glyph("⚪"));
    }
    row = row.act(Act::new(format!("fait:{id}"), "Traitée").glyph("☑️"));
    for (code, _, label) in SNOOZES {
        row = row.act(Act::new(format!("{code}:{id}"), label).glyph("⏰"));
    }
    row
}

/// « Ignorer ce dépôt » (groupement par dépôt seulement).
fn mute_act(repo: &str) -> Act {
    Act::new(format!("ignorer-depot:{repo}"), "Ignorer ce dépôt")
        .glyph("🔕")
        .danger()
        .confirm(format!("Ne plus rien montrer de {repo} ? (rétabli dans les réglages d'Owlery)"))
}

/// « Tout marquer lu » sur un groupe de `unread` non lues : chacune s'annule à part (Owlery n'offre pas encore de quoi
/// les marquer d'un coup), ce que la confirmation dit avant.
fn mark_group_act(key: &str, unread: usize) -> Act {
    let act = Act::new(format!("tout-lu:{key}"), "Tout marquer lu").glyph("✅");
    match unread {
        0 | 1 => act,
        n if n > UNDO_KEPT => act.confirm(format!(
            "Marquer lues les {n} non lues ? « Annuler » les reprend une à une, les {UNDO_KEPT} dernières seulement, \
             et les gestes d'avant ne s'annuleront plus."
        )),
        n => act.confirm(format!("Marquer lues les {n} non lues ? « Annuler » les reprend une à une.")),
    }
}

fn undo_act() -> Act {
    Act::new("annuler", "Annuler le dernier geste").glyph("↩️")
}

/// Le bandeau d'un groupe : ouvert s'il a des non lues, en alerte si une CI y échoue ; son menu reprend celui de
/// l'en-tête du PC.
fn group_band(g: &Group, key: &str, by: GroupBy) -> Band {
    let glyph = match by {
        GroupBy::Repo => "📦",
        GroupBy::Kind => g.entries.first().map_or("✉", |(i, _)| kind_glyph(i.kind)),
    };
    let mut band = Band::new(key, &g.title).aside(g.entries.len().to_string()).glyph(glyph).open(g.unread > 0);
    if g.unread > 0 {
        band = band.badge(unread_text(g.unread)).act(mark_group_act(key, g.unread));
    }
    if g.entries.iter().any(|(i, _)| i.ci == Some(Ci::Failure)) {
        band = band.tone(Tone::Danger);
    }
    if by == GroupBy::Repo {
        band = band.act(mute_act(&g.title));
    }
    band
}

/// L'accueil : un bandeau par groupe, dans l'ordre du PC (ceux qui ont des non lues d'abord) ; au-delà de
/// `PER_GROUP` entrées, une ligne mène à l'écran du groupe.
pub fn home(inbox: &Inbox, rules: &Rules, by: GroupBy, now: i64) -> Screen {
    let view = groups(inbox, rules, by, now);
    let unread: usize = view.iter().map(|g| g.unread).sum();
    let subtitle = match unread {
        0 => "Rien en attente".to_string(),
        1 => "1 en attente".to_string(),
        n => format!("{n} en attente"),
    };
    let mut s =
        Screen::new("Owlery").subtitle(subtitle).act(Act::new("actualiser", "Actualiser").glyph("🔄")).act(undo_act());
    if view.is_empty() {
        s.empty("Boîte de réception vide.");
    }
    for g in &view {
        let key = group_key(g, by);
        s.band(group_band(g, &key, by));
        for (item, status) in g.entries.iter().take(PER_GROUP) {
            s.row(item_row(item, *status, by, now));
        }
        let more = g.entries.len().saturating_sub(PER_GROUP);
        if more > 0 {
            let title = if more == 1 { "1 autre…".to_string() } else { format!("{more} autres…") };
            s.row(Row::new(title).key(format!("{key}/autres")).glyph("…").open(format!("groupe/{key}")));
        }
    }
    s
}

/// Un groupe en entier (`groupe/<clé>`), avec une recherche ; vide quand il n'a plus rien à montrer.
pub fn group(inbox: &Inbox, rules: &Rules, by: GroupBy, key: &str, now: i64) -> Screen {
    let view = groups(inbox, rules, by, now);
    let Some(g) = view.iter().find(|g| group_key(g, by) == key) else {
        let mut s = Screen::new(key).act(undo_act());
        s.empty("Plus rien dans ce groupe.");
        return s;
    };
    let subtitle = match g.unread {
        0 => entries_text(g.entries.len()),
        n => format!("{} · {}", unread_text(n), entries_text(g.entries.len())),
    };
    let mut s = Screen::new(&g.title).subtitle(subtitle).search("Chercher dans le groupe");
    if g.unread > 0 {
        s = s.act(mark_group_act(key, g.unread));
    }
    s = s.act(undo_act());
    if by == GroupBy::Repo {
        s = s.act(mute_act(&g.title));
    }
    for (item, status) in &g.entries {
        s.row(item_row(item, *status, by, now));
    }
    s
}

/// Le détail d'une entrée ; `claude` : son dossier local est connu (*Corriger avec Claude* possible).
pub fn item(i: &Item, status: Status, now: i64, claude: bool) -> Screen {
    let mut s = Screen::new(&i.title).subtitle(format!("{} · {}", reference(i), i.kind.label()));
    s.acts.push(match status {
        Status::Unread => Act::new("lu", "Marquer lue").glyph("✅"),
        _ => Act::new("non-lu", "Marquer non lue").glyph("⚪"),
    });
    s.acts.push(Act::new("fait", "Traitée").glyph("☑️"));
    for (code, _, label) in SNOOZES {
        s.acts.push(Act::new(code, label).glyph("⏰"));
    }
    s.acts.push(
        Act::new("ignorer", format!("Ignorer {}", i.repo))
            .glyph("🔕")
            .danger()
            .confirm(format!("Ne plus rien montrer de {} ? (rétabli dans les réglages d'Owlery)", i.repo)),
    );
    let mut info: Vec<(&str, String)> = vec![("Forge", i.forge.label().to_string())];
    if !i.author.is_empty() {
        info.push(("Auteur", i.author.clone()));
    }
    if let Some(b) = &i.branch {
        info.push(("Branche", b.clone()));
    }
    info.push(("Activité", ago(i.updated_at, now)));
    if i.comments > 0 {
        info.push(("Commentaires", i.comments.to_string()));
    }
    for (k, v) in info {
        s.row(Row::new(k).detail(v));
    }
    if let Some(ci) = i.ci {
        let text = match ci {
            Ci::Success => "réussie",
            Ci::Pending => "en cours",
            Ci::Failure => "en échec",
        };
        let mut row = Row::new("CI").detail(text);
        if let Some(t) = ci_tone(Some(ci)) {
            row = row.tone(t);
        }
        s.row(row);
    }
    for c in i.checks.iter().take(5) {
        let mut row = Row::new(&c.name).subtitle("vérification en échec").glyph("❌").tone(Tone::Danger);
        if owlery::actions::is_safe_url(&c.url) {
            row = row.tap(Act::link("Ouvrir", &c.url));
        }
        s.row(row);
    }
    if !i.summary.trim().is_empty() {
        s.section("Description", "");
        s.text(i.summary.trim(), false);
    }
    let mut acts = Vec::new();
    if claude {
        let label = if i.ci == Some(Ci::Failure) { "Corriger avec Claude" } else { "Ouvrir avec Claude" };
        acts.push(Act::new("claude", label).glyph("✳️").primary());
    }
    if owlery::actions::is_safe_url(&i.url) {
        let link = Act::link("Ouvrir", &i.url);
        acts.push(if claude { link } else { link.primary() });
        acts.push(Act::new("pc", "Ouvrir sur le PC").glyph("🖥️"));
    }
    if !acts.is_empty() {
        s.buttons(acts);
    }
    s
}

/// « Tout marquer lu » sur un groupe : ses non lues, par `App::mark` comme le reste (chacune s'annule à part : Owlery
/// n'offre pas encore de quoi les marquer d'un coup, la réponse le rappelle).
fn mark_group_read(app: &mut App, key: &str) -> Result<Option<String>, String> {
    let ids: Vec<String> = {
        let st = app.state();
        let by = st.settings.group_by;
        let view = groups(&st.inbox, &st.settings.rules, by, now());
        let g = view.iter().find(|g| group_key(g, by) == key).ok_or("Ce groupe n'existe plus.")?;
        g.entries.iter().filter(|(_, s)| *s == Status::Unread).map(|(i, _)| i.id.clone()).collect()
    };
    for id in &ids {
        app.mark(id, Status::Read);
    }
    Ok(Some(match ids.len() {
        0 => "Rien à marquer".into(),
        1 => "1 entrée marquée lue".into(),
        n => format!("{n} entrées marquées lues (« Annuler » les reprend une à une)"),
    }))
}

/// Défait le dernier geste, comme Ctrl+Z sur le PC (qui le dit d'une bulle) ; dit combien d'entrées il a rétablies
/// (une seule après un « Tout marquer lu » du téléphone, qui s'annule entrée par entrée).
fn undo(app: &mut App) -> String {
    let before = app.state().inbox.marks.clone();
    app.undo();
    let after = &app.state().inbox.marks;
    let changed = before.keys().chain(after.keys().filter(|k| !before.contains_key(*k)));
    match changed.filter(|k| before.get(*k) != after.get(*k)).count() {
        0 => "Rien à annuler".into(),
        1 => "Dernier geste annulé (1 entrée)".into(),
        n => format!("Dernier geste annulé ({n} entrées)"),
    }
}

/// Ce que le téléphone garde : le canal des demandes d'Owlery (« Actualiser »).
pub struct Phone {
    requests: Sender<Incoming>,
}

impl Phone {
    pub fn new(requests: Sender<Incoming>) -> Phone {
        Phone { requests }
    }

    pub fn refresh(&self) -> Result<Option<String>, String> {
        self.requests.send(Incoming::Refresh).map_err(|_| "Owlery ne répond plus.".to_string())?;
        Ok(Some("Lecture des forges lancée".into()))
    }

    pub fn screen(&self, app: &App, path: &str) -> Option<Screen> {
        let st = app.state();
        let now = now();
        let by = st.settings.group_by;
        if path.is_empty() {
            return Some(home(&st.inbox, &st.settings.rules, by, now));
        }
        if let Some(key) = path.strip_prefix("groupe/") {
            return Some(group(&st.inbox, &st.settings.rules, by, key, now));
        }
        let id = path.strip_prefix("item/")?;
        let i = st.inbox.items.get(id)?;
        Some(item(i, st.inbox.status(i, now), now, app.knows_local_dir(id)))
    }

    /// Un geste : sur l'entrée de l'écran (`item/<id>`), sur une ligne (`lu:<id>`…), sur un groupe (`tout-lu:<clé>`,
    /// `ignorer-depot:<dépôt>`) ou sur toute la boîte (`annuler`, `actualiser`).
    pub fn action(
        &self,
        app: &mut App,
        ctx: &suite_sdk::egui::Context,
        path: &str,
        action: &str,
    ) -> Result<Option<String>, String> {
        match action {
            "actualiser" => return self.refresh(),
            "annuler" => return Ok(Some(undo(app))),
            _ => {}
        }
        let (verb, id) = match action.split_once(':') {
            Some((verb, id)) => (verb, id.to_string()),
            None => (action, path.strip_prefix("item/").ok_or("action inconnue")?.to_string()),
        };
        match verb {
            "tout-lu" => return mark_group_read(app, &id),
            "ignorer-depot" => {
                if !app.state().inbox.items.values().any(|i| i.repo == id) {
                    return Err("Ce dépôt n'a plus d'entrée.".into());
                }
                app.mute(&id);
                return Ok(Some(format!("Dépôt ignoré : {id}")));
            }
            _ => {}
        }
        let Some(i) = app.state().inbox.items.get(&id).cloned() else {
            return Err("Cette entrée n'existe plus.".into());
        };
        if let Some((_, snooze, label)) = SNOOZES.iter().find(|(code, ..)| *code == verb) {
            app.mark(&id, Status::Snoozed { until: snooze.until(now()) });
            return Ok(Some(label.replace("Reporter", "Reportée")));
        }
        let done = match verb {
            "lu" => {
                app.mark(&id, Status::Read);
                "Marquée lue"
            }
            "non-lu" => {
                app.mark(&id, Status::Unread);
                "Marquée non lue"
            }
            "fait" => {
                app.mark(&id, Status::Done);
                "Traitée"
            }
            "ignorer" => {
                app.mute(&i.repo);
                "Dépôt ignoré"
            }
            "claude" => {
                app.work_with_claude(ctx, &id);
                // La session s'ouvre dans Fawkes : le téléphone y va.
                return Ok(Some(format!("{MODULE_PREFIX}fawkes")));
            }
            "pc" => {
                owlery::actions::open_url(&i.url)?;
                "Ouverte sur le PC"
            }
            _ => return Err("action inconnue".into()),
        };
        Ok(Some(done.into()))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, mpsc};

    use owlery::app::StartOptions;
    use owlery::model::{Check, Mark};
    use owlery::state::{AppState, Loaded, Settings};
    use suite_sdk::mobile::ui::{Block, Fold};

    use super::*;

    fn entry(id: &str, kind: Kind, repo: &str, n: u64, at: i64, ci: Option<Ci>) -> Item {
        Item {
            id: id.into(),
            kind,
            repo: repo.into(),
            number: Some(n),
            title: format!("PR {n}"),
            url: format!("https://github.com/{repo}/pull/{n}"),
            author: "ada".into(),
            updated_at: at,
            ci,
            ..Default::default()
        }
    }

    fn sample(now: i64) -> (Inbox, Rules) {
        let mut inbox = Inbox::default();
        for i in [
            entry("a", Kind::ReviewRequested, "contoso/web", 12, now - 600, Some(Ci::Pending)),
            entry("b", Kind::ReviewRequested, "contoso/web", 13, now - 60, None),
            entry("c", Kind::CiFailed, "contoso/api", 7, now - 7200, Some(Ci::Failure)),
            entry("lue", Kind::Mention, "contoso/docs", 8, now - 3600, None),
            entry("faite", Kind::Assigned, "contoso/api", 9, now - 60, None),
            entry("muette", Kind::Mention, "contoso/bruit", 1, now - 60, None),
        ] {
            inbox.items.insert(i.id.clone(), i);
        }
        let mark = |inbox: &Inbox, id: &str, status| Mark::new(status, &inbox.items[id], None);
        let read = mark(&inbox, "lue", Status::Read);
        let done = mark(&inbox, "faite", Status::Done);
        inbox.marks.insert("lue".into(), read);
        inbox.marks.insert("faite".into(), done);
        let rules = Rules { muted_repos: ["contoso/bruit".to_string()].into(), hide_bots: false };
        (inbox, rules)
    }

    /// Les bandeaux d'un écran : (clé, titre, pliage, pastille, ton).
    fn bands(s: &Screen) -> Vec<(String, String, Fold, String, Option<Tone>)> {
        s.blocks
            .iter()
            .filter_map(|b| match b {
                Block::Section { key, title, fold, badge, tone, .. } => {
                    Some((key.clone(), title.clone(), *fold, badge.clone(), *tone))
                }
                _ => None,
            })
            .collect()
    }

    fn rows(s: &Screen) -> Vec<&Row> {
        s.blocks
            .iter()
            .filter_map(|b| match b {
                Block::Row(r) => Some(r),
                _ => None,
            })
            .collect()
    }

    /// Comme le PC : un bandeau par dépôt, ceux qui ont des non lues d'abord, ouverts ; les autres pliés.
    #[test]
    fn the_home_groups_by_repo_like_the_pc() {
        let now = 1_800_000_000;
        let (inbox, rules) = sample(now);
        let s = home(&inbox, &rules, GroupBy::Repo, now);
        assert_eq!(s.subtitle, "3 en attente");
        let s_bands = bands(&s);
        let summary: Vec<(&str, Fold, &str, Option<Tone>)> =
            s_bands.iter().map(|(k, _, f, b, t)| (k.as_str(), *f, b.as_str(), *t)).collect();
        assert_eq!(
            summary,
            [
                ("contoso/web", Fold::Open, "2 non lues", None),
                ("contoso/api", Fold::Open, "1 non lue", Some(Tone::Danger)),
                ("contoso/docs", Fold::Closed, "", None),
            ]
        );
        // La plus récente d'abord ; ni terminée, ni dépôt ignoré.
        assert_eq!(s.opens().len(), 4);
        let first = rows(&s)[0];
        assert_eq!((first.title.as_str(), first.subtitle.as_str()), ("PR 13", "Revue demandée · #13 · ada"));
        assert_eq!((first.key.as_str(), first.badge.as_str(), first.glyph.as_str()), ("b", "non lu", "👀"));
        let ids = s.action_ids();
        for id in [
            "actualiser",
            "annuler",
            "lu:b",
            "reporter-3h:b",
            "reporter:b",
            "reporter-lundi:b",
            "tout-lu:contoso/web",
            "ignorer-depot:contoso/web",
            "ignorer-depot:contoso/docs",
        ] {
            assert!(ids.contains(id), "{id} absent : {ids:?}");
        }
        assert!(!ids.contains("tout-lu:contoso/docs"), "rien à marquer dans un groupe déjà lu");
        // Plusieurs non lues : la confirmation dit qu'elles s'annuleront une à une ; une seule : rien à confirmer.
        let confirm = |id: &str| s.all_acts().into_iter().find(|a| a.id == id).and_then(|a| a.confirm.clone());
        assert_eq!(
            confirm("tout-lu:contoso/web").as_deref(),
            Some("Marquer lues les 2 non lues ? « Annuler » les reprend une à une.")
        );
        assert_eq!(confirm("tout-lu:contoso/api"), None);
        assert!(mark_group_act("k", UNDO_KEPT + 1).confirm.is_some_and(|q| q.contains("ne s'annuleront plus")));
        let links: Vec<&str> = first.acts.iter().filter_map(|a| a.url.as_deref()).collect();
        assert_eq!(links, ["https://github.com/contoso/web/pull/13"], "« Ouvrir » sur le téléphone");
    }

    #[test]
    fn grouped_by_kind_the_bands_are_kinds_and_nothing_mutes_a_kind() {
        let now = 1_800_000_000;
        let (inbox, rules) = sample(now);
        let s = home(&inbox, &rules, GroupBy::Kind, now);
        let keys: Vec<(String, String)> = bands(&s).into_iter().map(|(k, t, ..)| (k, t)).collect();
        assert_eq!(
            keys,
            [
                ("revue".to_string(), "Revue demandée".to_string()),
                ("ci".to_string(), "CI en échec".to_string()),
                ("mention".to_string(), "Mention".to_string()),
            ]
        );
        assert_eq!(rows(&s)[0].subtitle, "contoso/web#13 · ada");
        assert!(!s.action_ids().iter().any(|a| a.starts_with("ignorer-depot:")));
    }

    /// Plus rien n'est coupé sans le dire : au-delà de 15, une ligne mène au groupe entier, qui se cherche.
    #[test]
    fn a_long_group_ends_with_a_row_to_the_whole_group() {
        let now = 1_800_000_000;
        let mut inbox = Inbox::default();
        for n in 0..20 {
            let i = entry(&format!("e{n}"), Kind::Mention, "contoso/web", n, now - n as i64 * 60, None);
            inbox.items.insert(i.id.clone(), i);
        }
        let rules = Rules::default();
        let s = home(&inbox, &rules, GroupBy::Repo, now);
        let r = rows(&s);
        assert_eq!(r.len(), PER_GROUP + 1);
        let more = r.last().unwrap();
        assert_eq!((more.title.as_str(), more.open.as_deref()), ("5 autres…", Some("groupe/contoso/web")));

        let g = group(&inbox, &rules, GroupBy::Repo, "contoso/web", now);
        assert_eq!((g.title.as_str(), g.subtitle.as_str()), ("contoso/web", "20 non lues · 20 entrées"));
        assert_eq!(rows(&g).len(), 20);
        assert!(!g.search.is_empty());
        assert!(g.action_ids().contains("tout-lu:contoso/web"));
        let (one, _) = sample(now);
        let docs = group(&one, &Rules::default(), GroupBy::Repo, "contoso/docs", now);
        assert_eq!(docs.subtitle, "1 entrée");
        let gone = group(&inbox, &rules, GroupBy::Repo, "contoso/parti", now);
        assert!(gone.blocks.iter().any(|b| matches!(b, Block::Empty { .. })));
    }

    #[test]
    fn an_item_opens_on_the_phone_or_on_the_pc() {
        let now = 1_800_000_000;
        let mut i = entry("c", Kind::CiFailed, "contoso/api", 7, now - 7200, Some(Ci::Failure));
        i.checks = vec![Check { name: "build".into(), url: "https://ci.example/1".into() }];
        let s = item(&i, Status::Unread, now, false);
        assert_eq!(s.subtitle, "contoso/api#7 · CI en échec");
        assert_eq!(
            s.action_ids().into_iter().collect::<Vec<_>>(),
            ["fait", "ignorer", "lu", "pc", "reporter", "reporter-3h", "reporter-lundi"]
        );
        assert!(item(&i, Status::Unread, now, true).action_ids().contains("claude"), "dossier local connu");
        let links: Vec<&str> = s.all_acts().into_iter().filter_map(|a| a.url.as_deref()).collect();
        assert_eq!(links, ["https://ci.example/1", "https://github.com/contoso/api/pull/7"]);
        i.url = "javascript:alert(1)".into();
        assert!(!item(&i, Status::Read, now, false).action_ids().contains("pc"), "pas de lien douteux");
    }

    #[test]
    fn refresh_goes_through_owlery_requests() {
        let (tx, rx) = mpsc::channel();
        let phone = Phone::new(tx);
        assert_eq!(phone.refresh(), Ok(Some("Lecture des forges lancée".into())));
        assert!(matches!(rx.try_recv(), Ok(Incoming::Refresh)));
    }

    /// Les gestes de groupe passent par Owlery : tout un dépôt marqué lu, un report, puis « Annuler » sur le dernier,
    /// et sur chaque entrée du groupe, une à une, jusqu'à ce qu'il n'y ait plus rien à annuler.
    #[test]
    fn group_gestures_and_undo_go_through_owlery() {
        let now = now();
        let (inbox, rules) = sample(now);
        let dir = std::env::temp_dir().join(format!("suite-owlery-telephone-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let state = AppState { inbox, settings: Settings { rules, ..Default::default() }, ..Default::default() };
        let opts = StartOptions::for_tests(
            dir.clone(),
            Arc::new(owlery::secrets::MemoryStore::default()),
            Arc::new(owlery::demo::DemoFetcher),
        );
        let cc = eframe::CreationContext::_new_kittest(suite_sdk::egui::Context::default());
        let mut app = App::new(&cc, opts, Loaded { state, problems: Vec::new(), writable: false });
        let ctx = suite_sdk::egui::Context::default();
        let (tx, _rx) = mpsc::channel();
        let phone = Phone::new(tx);
        let status = |app: &App, id: &str| {
            let st = app.state();
            st.inbox.status(&st.inbox.items[id], now)
        };

        assert_eq!(
            phone.action(&mut app, &ctx, "", "tout-lu:contoso/web"),
            Ok(Some("2 entrées marquées lues (« Annuler » les reprend une à une)".into()))
        );
        assert_eq!((status(&app, "a"), status(&app, "b")), (Status::Read, Status::Read));
        assert!(phone.action(&mut app, &ctx, "", "tout-lu:contoso/parti").is_err());

        assert_eq!(phone.action(&mut app, &ctx, "", "reporter-3h:c"), Ok(Some("Reportée de 3 h".into())));
        assert!(matches!(status(&app, "c"), Status::Snoozed { until } if until > now));
        assert_eq!(phone.action(&mut app, &ctx, "", "annuler"), Ok(Some("Dernier geste annulé (1 entrée)".into())));
        assert_eq!(status(&app, "c"), Status::Unread);
        for _ in 0..2 {
            assert_eq!(phone.action(&mut app, &ctx, "", "annuler"), Ok(Some("Dernier geste annulé (1 entrée)".into())));
        }
        assert_eq!((status(&app, "a"), status(&app, "b")), (Status::Unread, Status::Unread));
        assert_eq!(phone.action(&mut app, &ctx, "", "annuler"), Ok(Some("Rien à annuler".into())));

        assert!(phone.action(&mut app, &ctx, "", "ignorer-depot:inconnu/depot").is_err());
        assert_eq!(
            phone.action(&mut app, &ctx, "", "ignorer-depot:contoso/api"),
            Ok(Some("Dépôt ignoré : contoso/api".into()))
        );
        assert!(app.state().settings.rules.muted_repos.contains("contoso/api"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
