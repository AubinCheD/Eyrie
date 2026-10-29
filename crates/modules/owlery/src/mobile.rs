//! Owlery sur le téléphone : la boîte de réception en onglets, un par sorte d'entrée (revues, pull requests, CI,
//! tickets, mentions), que l'on fait glisser ; dans un onglet, les entrées groupées par dépôt quand c'est le réglage
//! du PC ; le détail d'une entrée, et les gestes de la liste du PC : marquer lue ou non lue, traitée, reportée
//! (3 h, demain, lundi), tout un groupe marqué lu, ignorer le dépôt, annuler le dernier geste, corriger avec Claude (une
//! session dans Fawkes, que le téléphone ouvre ensuite), ouvrir sur le téléphone ou sur le PC, actualiser.
//!
//! Tout se lit dans l'état vivant d'Owlery (`App::state`, groupé par `Inbox::view` comme la liste du PC) et passe par
//! ses propres méthodes (`mark`, `mark_read_many`, `mute`, `undo`, `work_with_claude`) : le PC montre aussitôt le même
//! changement.
//! « Actualiser » passe par le canal des demandes d'Owlery (`contract::Incoming::Refresh`).

use std::sync::mpsc::Sender;
use std::time::{SystemTime, UNIX_EPOCH};

use owlery::app::App;
use owlery::contract::{Incoming, kind_id};
use owlery::inbox::{Filter, Group, GroupBy, Inbox, Rules, Snooze};
use owlery::model::{Ci, Item, Kind, Status, Target};
use suite_sdk::mobile::msg::MODULE_PREFIX;
use suite_sdk::mobile::ui::{Act, Band, Row, Screen, Tab, Tone};

/// Les sortes dans l'ordre des onglets de l'accueil, toujours le même (un onglet ne change pas de place quand on lit
/// ses entrées) : ce qui attend un geste de l'utilisateur d'abord.
const KINDS: [Kind; 5] = [Kind::ReviewRequested, Kind::MyPr, Kind::CiFailed, Kind::Assigned, Kind::Mention];

/// Entrées montrées dans un onglet par dépôt qui a des non lues ; toutes sont sur l'écran du dépôt (`groupe/<clé>`).
const PER_GROUP: usize = 15;
/// … par groupe tout lu (plié au départ).
const PER_READ_GROUP: usize = 5;
/// Entrées montrées dans l'accueil en tout (chacune porte son menu, l'écran est renvoyé à chaque geste) : au-delà, un
/// groupe n'a plus que la ligne qui mène à son écran.
const HOME_ROWS: usize = 100;

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

/// Le titre d'un bandeau (et de son écran) quand la liste est groupée par sorte : au pluriel, puisqu'il en réunit
/// plusieurs ; les PR de l'utilisateur sont « Pull Requests ».
fn kind_title(k: Kind) -> &'static str {
    match k {
        Kind::ReviewRequested => "Revues demandées",
        Kind::CiFailed => "CI en échec",
        Kind::Mention => "Mentions",
        Kind::Assigned => "Tickets assignés",
        Kind::MyPr => "Pull Requests",
    }
}

/// Le libellé d'une sorte en onglet, court.
fn kind_tab(k: Kind) -> &'static str {
    match k {
        Kind::ReviewRequested => "Revues",
        Kind::CiFailed => "CI",
        Kind::Mention => "Mentions",
        Kind::Assigned => "Tickets",
        Kind::MyPr => "Pull Requests",
    }
}

/// La sorte d'un identifiant d'onglet (`sorte/<id>`).
fn kind_of(id: &str) -> Option<Kind> {
    KINDS.into_iter().find(|k| kind_id(*k) == id)
}

/// Le titre d'une entrée sur le téléphone, et la sorte de ticket qu'Owlery met devant celui d'un ticket Azure DevOps
/// (« Product Backlog Item : Corrige l'export ») : elle prend de la place pour rien en tête de ligne, le détail la
/// montre à part.
fn split_title(i: &Item) -> (&str, Option<&str>) {
    if matches!(i.target, Target::Azure { pr: false, .. })
        && let Some((kind, title)) = i.title.split_once(" : ")
        && !kind.is_empty()
    {
        return (title, Some(kind));
    }
    (&i.title, None)
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

fn ci_tone(ci: Option<Ci>) -> Option<Tone> {
    match ci? {
        Ci::Failure => Some(Tone::Danger),
        // Une CI qui tourne n'est pas une alerte : pas de couleur.
        Ci::Pending => None,
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

/// Une entrée dans la liste : son titre en entier et son auteur, en gras tant qu'elle n'est pas lue ; sa sorte en
/// glyphe quand la liste en mêle plusieurs (`glyph`), sinon l'onglet la dit. Le reste (dépôt, numéro, activité,
/// branche) est sur l'écran de l'entrée.
fn item_row(item: &Item, status: Status, glyph: bool) -> Row {
    let id = &item.id;
    let mut row = Row::new(split_title(item).0)
        .full()
        .key(id)
        .subtitle(&item.author)
        .unread(status == Status::Unread)
        .open(format!("item/{id}"));
    if glyph {
        row = row.glyph(kind_glyph(item.kind));
    }
    if let Some(t) = ci_tone(item.ci) {
        row = row.tone(t);
    }
    if owlery::actions::is_safe_url(&item.url) {
        row = row.act(Act::link("Ouvrir", &item.url));
    }
    if status == Status::Unread {
        row = row.act(Act::new(format!("lu:{id}"), "Marquer lue").glyph("✅"));
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

/// « Tout marquer lu » sur un groupe : sans confirmation, comme dans l'en-tête du PC (« Annuler » le défait d'un coup).
fn mark_group_act(key: &str) -> Act {
    Act::new(format!("tout-lu:{key}"), "Tout marquer lu").glyph("✅")
}

/// « Tout marquer lu » sur l'onglet d'une sorte, quel que soit le groupement du PC.
fn mark_kind_act(k: Kind) -> Act {
    Act::new(format!("tout-lu-sorte:{}", kind_id(k)), "Tout marquer lu").glyph("✅")
}

/// Les entrées d'une seule sorte, groupées comme sur le PC (par dépôt, ou toutes ensemble par sorte).
fn kind_groups<'a>(inbox: &'a Inbox, rules: &Rules, by: GroupBy, k: Kind, now: i64) -> Vec<Group<'a>> {
    let filter = Filter { kinds: [k].into(), ..Default::default() };
    inbox.view(&filter, rules, by, now)
}

/// « Annuler le dernier geste » en barre du haut, seulement s'il y en a un (`App::can_undo`).
fn with_undo(s: Screen, can_undo: bool) -> Screen {
    if can_undo { s.act(Act::new("annuler", "Annuler le dernier geste").glyph("↩️")) } else { s }
}

/// Le titre d'un groupe : son dépôt, ou sa sorte au pluriel.
fn group_title<'a>(g: &'a Group, by: GroupBy) -> &'a str {
    match (by, g.entries.first()) {
        (GroupBy::Kind, Some((i, _))) => kind_title(i.kind),
        _ => &g.title,
    }
}

/// Le titre d'un dépôt dans un onglet (groupement par dépôt) : un simple intitulé, son compte, et son menu (ignorer
/// le dépôt) ; il ne se plie pas : l'onglet est déjà une page à part.
fn repo_band(g: &Group) -> Band {
    let mut band = Band::new("", &g.title).aside(g.entries.len().to_string()).act(mute_act(&g.title));
    if g.entries.iter().any(|(i, _)| i.ci == Some(Ci::Failure)) {
        band = band.tone(Tone::Danger);
    }
    band
}

/// L'accueil et ses onglets : une sorte par onglet (celles qui ont des entrées, dans l'ordre de `KINDS`), le premier
/// à l'accueil (`""`), les autres en `sorte/<id>` ; ses non lues en pastille d'onglet. `tab` : l'onglet montré (aucun :
/// le premier). Dans l'onglet, les entrées par dépôt quand le PC groupe par dépôt (au-delà de `PER_GROUP` entrées —
/// `PER_READ_GROUP` pour un dépôt tout lu — ou de `HOME_ROWS` dans l'onglet, une ligne mène à l'écran du dépôt),
/// sinon toutes ensemble. `can_undo` : Owlery a un geste à annuler.
pub fn home(inbox: &Inbox, rules: &Rules, by: GroupBy, now: i64, can_undo: bool, tab: Option<Kind>) -> Screen {
    let unread: usize = groups(inbox, rules, by, now).iter().map(|g| g.unread).sum();
    let subtitle = match unread {
        0 => "Rien en attente".to_string(),
        1 => "1 en attente".to_string(),
        n => format!("{n} en attente"),
    };
    // Chaque sorte qui a des entrées : (sorte, non lues) ; l'onglet demandé reste, même vidé.
    let kinds: Vec<(Kind, usize)> = KINDS
        .into_iter()
        .filter_map(|k| {
            let view = kind_groups(inbox, rules, GroupBy::Kind, k, now);
            let shown = view.iter().any(|g| !g.entries.is_empty()) || tab == Some(k);
            shown.then(|| (k, view.iter().map(|g| g.unread).sum()))
        })
        .collect();
    let first = kinds.first().map(|(k, _)| *k);
    let mut s = Screen::new("Owlery").subtitle(subtitle);
    let Some(current) = tab.or(first) else {
        s = with_undo(s.act(Act::new("actualiser", "Actualiser").glyph("🔄")), can_undo);
        s.empty("Boîte de réception vide.");
        return s;
    };
    for &(k, n) in &kinds {
        let path = if Some(k) == first { String::new() } else { format!("sorte/{}", kind_id(k)) };
        let badge = if n > 0 { n.to_string() } else { String::new() };
        s.tabs.push(Tab { label: kind_tab(k).into(), path, badge });
    }
    if kinds.iter().any(|&(k, n)| k == current && n > 0) {
        s = s.act(mark_kind_act(current));
    }
    s = with_undo(s.act(Act::new("actualiser", "Actualiser").glyph("🔄")), can_undo);
    let view = kind_groups(inbox, rules, by, current, now);
    if view.iter().all(|g| g.entries.is_empty()) {
        s.empty("Plus rien ici.");
    }
    let mut room = HOME_ROWS;
    for g in &view {
        let key = group_key(g, by);
        let per = match by {
            GroupBy::Kind => HOME_ROWS,
            GroupBy::Repo => {
                s.band(repo_band(g));
                if g.unread > 0 { PER_GROUP } else { PER_READ_GROUP }
            }
        };
        let shown = g.entries.len().min(per).min(room);
        room -= shown;
        for (item, status) in g.entries.iter().take(shown) {
            s.row(item_row(item, *status, false));
        }
        let title = match (shown, g.entries.len() - shown) {
            (_, 0) => continue,
            (0, n) => entries_text(n) + "…",
            (_, 1) => "1 autre…".to_string(),
            (_, n) => format!("{n} autres…"),
        };
        s.row(Row::new(title).key(format!("{key}/autres")).open(format!("groupe/{key}")));
    }
    s
}

/// Un groupe en entier (`groupe/<clé>`), avec une recherche ; vide quand il n'a plus rien à montrer.
pub fn group(inbox: &Inbox, rules: &Rules, by: GroupBy, key: &str, now: i64, can_undo: bool) -> Screen {
    let view = groups(inbox, rules, by, now);
    let Some(g) = view.iter().find(|g| group_key(g, by) == key) else {
        let mut s = with_undo(Screen::new(key), can_undo);
        s.empty("Plus rien dans ce groupe.");
        return s;
    };
    let subtitle = match g.unread {
        0 => entries_text(g.entries.len()),
        n => format!("{} · {}", unread_text(n), entries_text(g.entries.len())),
    };
    let mut s = Screen::new(group_title(g, by)).subtitle(subtitle).search("Chercher dans le groupe");
    if g.unread > 0 {
        s = s.act(mark_group_act(key));
    }
    s = with_undo(s, can_undo);
    if by == GroupBy::Repo {
        s = s.act(mute_act(&g.title));
    }
    for (item, status) in &g.entries {
        s.row(item_row(item, *status, by == GroupBy::Repo));
    }
    s
}

/// Le détail d'une entrée : la barre du haut dit son numéro (un dépôt Azure DevOps, « org/projet/dépôt », y serait
/// coupé) et sa sorte, puis viennent son titre en entier, et ce que la liste ne montre pas, le dépôt et la branche en
/// entier ; `claude` : son dossier local est connu (*Corriger avec Claude* possible).
pub fn item(i: &Item, status: Status, now: i64, claude: bool) -> Screen {
    let (title, ticket) = split_title(i);
    let number = i.number.map_or_else(|| i.repo.clone(), |n| format!("#{n}"));
    let mut s = Screen::new(number).subtitle(i.kind.label());
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
    s.row(Row::new(title).full().glyph(kind_glyph(i.kind)));
    s.section("Détails", "");
    // Les textes longs (sorte de ticket, dépôt, branche) dessous, en entier ; les courts à droite.
    if let Some(t) = ticket {
        s.row(Row::new("Type").subtitle(t).full());
    }
    s.row(Row::new("Dépôt").subtitle(&i.repo).full());
    s.row(Row::new("Forge").detail(i.forge.label()));
    if !i.author.is_empty() {
        s.row(Row::new("Auteur").subtitle(&i.author).full());
    }
    if let Some(b) = &i.branch {
        s.row(Row::new("Branche").subtitle(b).full());
    }
    s.row(Row::new("Activité").detail(ago(i.updated_at, now)));
    if i.comments > 0 {
        s.row(Row::new("Commentaires").detail(i.comments.to_string()));
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
        // La session s'ouvre dans un terminal de Fawkes : seulement avec le droit « Terminaux ».
        acts.push(Act::new("claude", label).glyph("✳️").primary().terminal());
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

/// « Tout marquer lu » sur un groupe, comme l'en-tête du PC (`App::mark_read_many`) : ses non lues, en un seul geste
/// qu'« Annuler » défait d'un coup.
fn mark_group_read(app: &mut App, key: &str) -> Result<Option<String>, String> {
    let ids: Vec<String> = {
        let st = app.state();
        let by = st.settings.group_by;
        let view = groups(&st.inbox, &st.settings.rules, by, now());
        let g = view.iter().find(|g| group_key(g, by) == key).ok_or("Ce groupe n'existe plus.")?;
        g.entries.iter().map(|(i, _)| i.id.clone()).collect()
    };
    Ok(Some(match app.mark_read_many(ids) {
        0 => "Rien à marquer".into(),
        1 => "1 entrée marquée lue".into(),
        n => format!("{n} entrées marquées lues"),
    }))
}

/// « Tout marquer lu » sur l'onglet d'une sorte : ses non lues, en un seul geste qu'« Annuler » défait d'un coup.
fn mark_kind_read(app: &mut App, id: &str) -> Result<Option<String>, String> {
    let k = kind_of(id).ok_or("Cette sorte n'existe pas.")?;
    let ids: Vec<String> = {
        let st = app.state();
        let view = kind_groups(&st.inbox, &st.settings.rules, GroupBy::Kind, k, now());
        view.iter().flat_map(|g| g.entries.iter().map(|(i, _)| i.id.clone())).collect()
    };
    Ok(Some(match app.mark_read_many(ids) {
        0 => "Rien à marquer".into(),
        1 => "1 entrée marquée lue".into(),
        n => format!("{n} entrées marquées lues"),
    }))
}

/// Défait le dernier geste, comme Ctrl+Z sur le PC (qui le dit d'une bulle) ; dit combien d'entrées il a rétablies
/// (toutes celles d'un « Tout marquer lu »).
fn undo(app: &mut App) -> String {
    if !app.can_undo() {
        return "Rien à annuler".into();
    }
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
            return Some(home(&st.inbox, &st.settings.rules, by, now, app.can_undo(), None));
        }
        if let Some(id) = path.strip_prefix("sorte/") {
            return Some(home(&st.inbox, &st.settings.rules, by, now, app.can_undo(), Some(kind_of(id)?)));
        }
        if let Some(key) = path.strip_prefix("groupe/") {
            return Some(group(&st.inbox, &st.settings.rules, by, key, now, app.can_undo()));
        }
        let id = path.strip_prefix("item/")?;
        let i = st.inbox.items.get(id)?;
        Some(item(i, st.inbox.status(i, now), now, app.knows_local_dir(id)))
    }

    /// Un geste : sur l'entrée de l'écran (`item/<id>`), sur une ligne (`lu:<id>`…), sur un groupe (`tout-lu:<clé>`,
    /// `ignorer-depot:<dépôt>`), sur l'onglet d'une sorte (`tout-lu-sorte:<id>`) ou sur toute la boîte (`annuler`,
    /// `actualiser`).
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
            "tout-lu-sorte" => return mark_kind_read(app, &id),
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

    /// Les onglets d'un écran : (libellé, chemin, pastille).
    fn tabs(s: &Screen) -> Vec<(&str, &str, &str)> {
        s.tabs.iter().map(|t| (t.label.as_str(), t.path.as_str(), t.badge.as_str())).collect()
    }

    /// Un onglet par sorte qui a des entrées, toujours dans le même ordre, le premier à l'accueil ; ses non lues en
    /// pastille. Groupé par dépôt comme sur le PC, l'onglet montre un intitulé par dépôt (qui ne se plie pas) ; une
    /// entrée non lue est en gras, sans pastille ni glyphe (l'onglet dit sa sorte).
    #[test]
    fn the_home_has_a_tab_per_kind_and_groups_it_by_repo_like_the_pc() {
        let now = 1_800_000_000;
        let (inbox, rules) = sample(now);
        let s = home(&inbox, &rules, GroupBy::Repo, now, true, None);
        assert_eq!(s.subtitle, "3 en attente");
        assert_eq!(tabs(&s), [("Revues", "", "2"), ("CI", "sorte/ci", "1"), ("Mentions", "sorte/mention", "")]);
        let s_bands = bands(&s);
        let summary: Vec<(&str, Fold, Option<Tone>)> =
            s_bands.iter().map(|(_, t, f, _, tone)| (t.as_str(), *f, *tone)).collect();
        assert_eq!(summary, [("contoso/web", Fold::No, None)]);
        // La plus récente d'abord ; le titre en entier et l'auteur ; le reste est sur l'écran de l'entrée.
        let r = rows(&s);
        assert_eq!(r.iter().map(|r| r.key.as_str()).collect::<Vec<_>>(), ["b", "a"]);
        let first = r[0];
        assert_eq!((first.title.as_str(), first.subtitle.as_str(), first.detail.as_str()), ("PR 13", "ada", ""));
        assert!(first.full && first.unread);
        assert_eq!((first.badge.as_str(), first.glyph.as_str()), ("", ""));
        let ids = s.action_ids();
        for id in [
            "actualiser",
            "annuler",
            "lu:b",
            "reporter-3h:b",
            "reporter:b",
            "reporter-lundi:b",
            "tout-lu-sorte:revue",
            "ignorer-depot:contoso/web",
        ] {
            assert!(ids.contains(id), "{id} absent : {ids:?}");
        }
        // Comme l'en-tête du PC : sans confirmation, « Annuler » le défait d'un coup.
        let confirm = |id: &str| s.all_acts().into_iter().find(|a| a.id == id).and_then(|a| a.confirm.clone());
        assert_eq!(confirm("tout-lu-sorte:revue"), None);
        let links: Vec<&str> = first.acts.iter().filter_map(|a| a.url.as_deref()).collect();
        assert_eq!(links, ["https://github.com/contoso/web/pull/13"], "« Ouvrir » sur le téléphone");

        // L'onglet des mentions : tout lu, rien à marquer ; la CI en échec dit son alerte au dépôt.
        let mentions = home(&inbox, &rules, GroupBy::Repo, now, true, Some(Kind::Mention));
        assert_eq!(tabs(&mentions), tabs(&s), "mêmes onglets d'un onglet à l'autre");
        assert!(!mentions.action_ids().iter().any(|a| a.starts_with("tout-lu")));
        assert!(!rows(&mentions)[0].unread);
        let ci = home(&inbox, &rules, GroupBy::Repo, now, true, Some(Kind::CiFailed));
        assert_eq!(bands(&ci)[0].4, Some(Tone::Danger));
        // Rien à annuler : pas de bouton « Annuler ».
        let calm = home(&inbox, &rules, GroupBy::Repo, now, false, None);
        assert!(!calm.action_ids().contains("annuler"));
        assert!(!group(&inbox, &rules, GroupBy::Repo, "contoso/web", now, false).action_ids().contains("annuler"));
        assert!(group(&inbox, &rules, GroupBy::Repo, "contoso/web", now, true).action_ids().contains("annuler"));
        // L'écran d'un dépôt mêle les sortes : elles s'y lisent au glyphe.
        assert_eq!(rows(&group(&inbox, &rules, GroupBy::Repo, "contoso/web", now, false))[0].glyph, "👀");
    }

    /// Groupé par sorte, l'onglet est la sorte : ses entrées ensemble, sans intitulé ni « Ignorer ce dépôt ».
    #[test]
    fn grouped_by_kind_a_tab_is_the_kind_alone() {
        let now = 1_800_000_000;
        let (inbox, rules) = sample(now);
        let s = home(&inbox, &rules, GroupBy::Kind, now, false, None);
        assert_eq!(tabs(&s).len(), 3);
        assert!(bands(&s).is_empty());
        assert_eq!(rows(&s).iter().map(|r| r.key.as_str()).collect::<Vec<_>>(), ["b", "a"]);
        assert!(!s.action_ids().iter().any(|a| a.starts_with("ignorer-depot:")));
        assert!(s.action_ids().contains("tout-lu-sorte:revue"));
        let ci = home(&inbox, &rules, GroupBy::Kind, now, false, Some(Kind::CiFailed));
        let c = rows(&ci)[0];
        assert_eq!((c.key.as_str(), c.tone), ("c", Some(Tone::Danger)));
        assert_eq!(group(&inbox, &rules, GroupBy::Kind, "revue", now, false).title, "Revues demandées");
        // Un onglet vidé reste affiché tant qu'on y est.
        let empty = home(&Inbox::default(), &rules, GroupBy::Kind, now, false, Some(Kind::Assigned));
        assert_eq!(tabs(&empty), [("Tickets", "", "")]);
        assert!(empty.blocks.iter().any(|b| matches!(b, Block::Empty { .. })));
        let none = home(&Inbox::default(), &rules, GroupBy::Kind, now, false, None);
        assert!(none.tabs.is_empty() && none.blocks.iter().any(|b| matches!(b, Block::Empty { .. })));
    }

    /// Les onglets sont courts ; les PR de l'utilisateur, « Pull Requests », les tickets, « Tickets ».
    #[test]
    fn kind_tabs_are_short_and_my_prs_are_pull_requests() {
        let now = 1_800_000_000;
        let mut inbox = Inbox::default();
        for i in [
            entry("p", Kind::MyPr, "contoso/web", 1, now - 60, None),
            entry("t", Kind::Assigned, "contoso/web", 2, now - 120, None),
        ] {
            inbox.items.insert(i.id.clone(), i);
        }
        let s = home(&inbox, &Rules::default(), GroupBy::Kind, now, false, None);
        assert_eq!(tabs(&s), [("Pull Requests", "", "1"), ("Tickets", "sorte/ticket", "1")]);
    }

    /// Un ticket Azure DevOps : sa sorte (« Product Backlog Item ») ne précède plus son titre ; l'écran du ticket la
    /// montre à part. Le titre d'une PR, même avec « : », reste entier.
    #[test]
    fn a_work_item_title_loses_its_kind_which_its_screen_shows_apart() {
        let now = 1_800_000_000;
        let mut t = entry("t", Kind::Assigned, "xelya/Ozmo", 4321, now - 60, None);
        t.forge = owlery::model::Forge::AzureDevOps;
        t.title = "Product Backlog Item : Export : les remises disparaissent".into();
        t.target =
            Target::Azure { org: "xelya".into(), project: "Ozmo".into(), repo_id: String::new(), id: 4321, pr: false };
        assert_eq!(item_row(&t, Status::Unread, false).title, "Export : les remises disparaissent");
        let s = item(&t, Status::Unread, now, false);
        let r = rows(&s);
        assert_eq!(r[0].title, "Export : les remises disparaissent");
        assert_eq!((r[1].title.as_str(), r[1].subtitle.as_str()), ("Type", "Product Backlog Item"));

        let mut pr = t.clone();
        pr.target = Target::Azure { org: "xelya".into(), project: "Ozmo".into(), repo_id: "r".into(), id: 7, pr: true };
        pr.title = "Export : corrige les remises".into();
        assert_eq!(item_row(&pr, Status::Unread, false).title, "Export : corrige les remises");
        assert!(rows(&item(&pr, Status::Unread, now, false)).iter().all(|r| r.title != "Type"));
    }

    /// Avec `SUITE_MOBILE_DUMP=<dossier>`, écrit des écrans d'exemple (l'accueil groupé par sorte, une PR, un ticket)
    /// pour les voir dans l'appli de bureau : `EYRIE_MOBILE_DEMO=ecran:owlery:<dossier>/owlery-accueil.json`.
    #[test]
    fn example_screens_can_be_dumped_for_a_preview() {
        let Some(out) = std::env::var_os("SUITE_MOBILE_DUMP") else { return };
        let out = std::path::Path::new(&out);
        let now = 1_800_000_000;
        let mut pr = entry("pr", Kind::ReviewRequested, "xelya/Ozmo/Ozmo", 18342, now - 600, Some(Ci::Pending));
        pr.forge = owlery::model::Forge::AzureDevOps;
        pr.title =
            "Facturation électronique : envoi des factures rectificatives au PDP avec leurs pièces jointes".into();
        pr.author = "Camille Martin".into();
        pr.branch = Some("feature/18342-facturation-electronique-factures-rectificatives-pieces-jointes".into());
        let mut ticket = entry("ticket", Kind::Assigned, "xelya/Ozmo", 52817, now - 7200, None);
        ticket.forge = owlery::model::Forge::AzureDevOps;
        ticket.title = "Product Backlog Item : Export comptable : les remises disparaissent des écritures".into();
        ticket.author = "Jean Dupont".into();
        ticket.target =
            Target::Azure { org: "xelya".into(), project: "Ozmo".into(), repo_id: String::new(), id: 52817, pr: false };
        let mine = entry("mine", Kind::MyPr, "AubinCheD/Eyrie", 42, now - 60, Some(Ci::Failure));
        let mut inbox = Inbox::default();
        for i in [pr.clone(), ticket.clone(), mine] {
            inbox.items.insert(i.id.clone(), i);
        }
        let rules = Rules::default();
        for (name, screen) in [
            ("owlery-accueil", home(&inbox, &rules, GroupBy::Kind, now, true, None)),
            ("owlery-accueil-revues", home(&inbox, &rules, GroupBy::Kind, now, true, Some(Kind::ReviewRequested))),
            ("owlery-accueil-depots", home(&inbox, &rules, GroupBy::Repo, now, true, None)),
            ("owlery-pr", item(&pr, Status::Unread, now, true)),
            ("owlery-ticket", item(&ticket, Status::Read, now, false)),
        ] {
            std::fs::write(out.join(format!("{name}.json")), serde_json::to_vec(&screen).unwrap()).unwrap();
        }
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
        let s = home(&inbox, &rules, GroupBy::Repo, now, false, None);
        let r = rows(&s);
        assert_eq!(r.len(), PER_GROUP + 1);
        let more = r.last().unwrap();
        assert_eq!((more.title.as_str(), more.open.as_deref()), ("5 autres…", Some("groupe/contoso/web")));

        let g = group(&inbox, &rules, GroupBy::Repo, "contoso/web", now, false);
        assert_eq!((g.title.as_str(), g.subtitle.as_str()), ("contoso/web", "20 non lues · 20 entrées"));
        assert_eq!(rows(&g).len(), 20);
        assert!(!g.search.is_empty());
        assert!(g.action_ids().contains("tout-lu:contoso/web"));
        let (one, _) = sample(now);
        let docs = group(&one, &Rules::default(), GroupBy::Repo, "contoso/docs", now, false);
        assert_eq!(docs.subtitle, "1 entrée");
        let gone = group(&inbox, &rules, GroupBy::Repo, "contoso/parti", now, false);
        assert!(gone.blocks.iter().any(|b| matches!(b, Block::Empty { .. })));
    }

    /// Beaucoup de dépôts : un dépôt tout lu montre moins d'entrées, et l'onglet entier en garde au plus `HOME_ROWS` ;
    /// le reste est sur l'écran de chaque dépôt.
    #[test]
    fn the_home_stays_small_with_many_repos() {
        let now = 1_800_000_000;
        let mut inbox = Inbox::default();
        for r in 0..8 {
            for n in 0..15 {
                let id = format!("r{r}-{n}");
                let i = entry(&id, Kind::Mention, &format!("contoso/d{r}"), n, now - (r * 100 + n) as i64, None);
                inbox.items.insert(id, i);
            }
        }
        for n in 0..8 {
            let i = entry(&format!("lue{n}"), Kind::Mention, "contoso/lu", n, now - 9000, None);
            inbox.marks.insert(i.id.clone(), Mark::new(Status::Read, &i, None));
            inbox.items.insert(i.id.clone(), i);
        }
        let s = home(&inbox, &Rules::default(), GroupBy::Repo, now, false, None);
        let r = rows(&s);
        let more = |r: &&&Row| r.key.ends_with("/autres");
        let entries = r.iter().filter(|r| !more(r)).count();
        assert_eq!(entries, HOME_ROWS);
        let more: Vec<(&str, &str)> =
            r.iter().filter(more).map(|r| (r.title.as_str(), r.open.as_deref().unwrap_or_default())).collect();
        assert_eq!(
            more,
            [
                ("5 autres…", "groupe/contoso/d6"),
                ("15 entrées…", "groupe/contoso/d7"),
                ("8 entrées…", "groupe/contoso/lu")
            ]
        );
        // Seul, le groupe tout lu montre ses cinq plus récentes.
        inbox.items.retain(|id, _| id.starts_with("lue"));
        let s = home(&inbox, &Rules::default(), GroupBy::Repo, now, false, None);
        let titles: Vec<&str> = rows(&s).iter().map(|r| r.title.as_str()).collect();
        assert_eq!(titles, ["PR 0", "PR 1", "PR 2", "PR 3", "PR 4", "3 autres…"]);
    }

    #[test]
    fn an_item_opens_on_the_phone_or_on_the_pc() {
        let now = 1_800_000_000;
        let mut i = entry("c", Kind::CiFailed, "contoso/api", 7, now - 7200, Some(Ci::Failure));
        i.checks = vec![Check { name: "build".into(), url: "https://ci.example/1".into() }];
        i.branch = Some("correctif/7-export-des-factures-avec-remises-sur-plusieurs-lignes".into());
        let s = item(&i, Status::Unread, now, false);
        assert_eq!((s.title.as_str(), s.subtitle.as_str()), ("#7", "CI en échec"));
        // Le titre en tête, la branche dessous : en entier tous deux.
        let r = rows(&s);
        assert!(r[0].title == "PR 7" && r[0].full);
        let branch = r.iter().find(|r| r.title == "Branche").expect("branche");
        assert!(branch.full && branch.subtitle == "correctif/7-export-des-factures-avec-remises-sur-plusieurs-lignes");
        assert_eq!(
            s.action_ids().into_iter().collect::<Vec<_>>(),
            ["fait", "ignorer", "lu", "pc", "reporter", "reporter-3h", "reporter-lundi"]
        );
        let with_claude = item(&i, Status::Unread, now, true);
        let claude = with_claude.all_acts().into_iter().find(|a| a.id == "claude").cloned();
        assert!(claude.is_some_and(|a| a.terminal), "dossier local connu ; ouvre un terminal de Fawkes");
        assert!(s.all_acts().iter().all(|a| !a.terminal), "rien d'autre n'ouvre de terminal");
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

    /// Une vraie instance d'Owlery sur l'exemple, groupée par `by`, dans un dossier d'essai (rien n'est écrit).
    fn app(now: i64, by: GroupBy, dir: &std::path::Path) -> App {
        let (inbox, rules) = sample(now);
        let _ = std::fs::remove_dir_all(dir);
        let settings = Settings { rules, group_by: by, ..Default::default() };
        let state = AppState { inbox, settings, ..Default::default() };
        let opts = StartOptions::for_tests(
            dir.to_path_buf(),
            Arc::new(owlery::secrets::MemoryStore::default()),
            Arc::new(owlery::demo::DemoFetcher),
        );
        let cc = eframe::CreationContext::_new_kittest(suite_sdk::egui::Context::default());
        App::new(&cc, opts, Loaded { state, problems: Vec::new(), writable: false })
    }

    fn status(app: &App, id: &str, now: i64) -> Status {
        let st = app.state();
        st.inbox.status(&st.inbox.items[id], now)
    }

    /// Les gestes de groupe passent par Owlery : tout un dépôt marqué lu, un report, puis « Annuler » sur le dernier,
    /// puis sur tout le dépôt d'un coup, comme sur le PC ; « Annuler » n'est proposé que s'il reste un geste.
    #[test]
    fn group_gestures_and_undo_go_through_owlery() {
        let now = now();
        let dir = std::env::temp_dir().join(format!("suite-owlery-telephone-{}", std::process::id()));
        let mut app = app(now, GroupBy::Repo, &dir);
        let ctx = suite_sdk::egui::Context::default();
        let (tx, _rx) = mpsc::channel();
        let phone = Phone::new(tx);
        let status = |app: &App, id: &str| status(app, id, now);
        let undo_offered = |app: &App| phone.screen(app, "").unwrap().action_ids().contains("annuler");
        assert!(!undo_offered(&app), "rien à annuler au départ");

        assert_eq!(phone.action(&mut app, &ctx, "", "tout-lu:contoso/web"), Ok(Some("2 entrées marquées lues".into())));
        assert_eq!((status(&app, "a"), status(&app, "b")), (Status::Read, Status::Read));
        assert!(undo_offered(&app));
        assert_eq!(phone.action(&mut app, &ctx, "", "tout-lu:contoso/web"), Ok(Some("Rien à marquer".into())));
        assert!(phone.action(&mut app, &ctx, "", "tout-lu:contoso/parti").is_err());

        assert_eq!(phone.action(&mut app, &ctx, "", "reporter-3h:c"), Ok(Some("Reportée de 3 h".into())));
        assert!(matches!(status(&app, "c"), Status::Snoozed { until } if until > now));
        assert_eq!(phone.action(&mut app, &ctx, "", "annuler"), Ok(Some("Dernier geste annulé (1 entrée)".into())));
        assert_eq!(status(&app, "c"), Status::Unread);
        assert_eq!(phone.action(&mut app, &ctx, "", "annuler"), Ok(Some("Dernier geste annulé (2 entrées)".into())));
        assert_eq!((status(&app, "a"), status(&app, "b")), (Status::Unread, Status::Unread));
        assert!(!undo_offered(&app), "plus rien à annuler");
        assert_eq!(phone.action(&mut app, &ctx, "", "annuler"), Ok(Some("Rien à annuler".into())));

        assert!(phone.action(&mut app, &ctx, "", "ignorer-depot:inconnu/depot").is_err());
        assert_eq!(
            phone.action(&mut app, &ctx, "", "ignorer-depot:contoso/api"),
            Ok(Some("Dépôt ignoré : contoso/api".into()))
        );
        assert!(app.state().settings.rules.muted_repos.contains("contoso/api"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// « Tout marquer lu » d'un onglet vise sa sorte (`revue`), groupée par dépôt ou non ; sur l'écran d'une entrée,
    /// un report vise l'entrée de l'écran.
    #[test]
    fn grouped_by_kind_a_whole_kind_is_read_and_an_item_snoozes_from_its_screen() {
        let now = now();
        let dir = std::env::temp_dir().join(format!("suite-owlery-telephone-sorte-{}", std::process::id()));
        let mut app = app(now, GroupBy::Kind, &dir);
        let ctx = suite_sdk::egui::Context::default();
        let (tx, _rx) = mpsc::channel();
        let phone = Phone::new(tx);
        assert!(phone.screen(&app, "").unwrap().action_ids().contains("tout-lu-sorte:revue"));
        assert_eq!(phone.screen(&app, "sorte/ci").unwrap().tabs.len(), 3);
        assert!(phone.screen(&app, "sorte/inconnue").is_none());
        assert_eq!(phone.action(&mut app, &ctx, "", "tout-lu-sorte:revue"), Ok(Some("2 entrées marquées lues".into())));
        assert_eq!((status(&app, "a", now), status(&app, "b", now)), (Status::Read, Status::Read));
        assert_eq!(status(&app, "c", now), Status::Unread, "une autre sorte");

        assert!(phone.screen(&app, "item/c").unwrap().action_ids().contains("reporter-lundi"));
        assert_eq!(phone.action(&mut app, &ctx, "item/c", "reporter-lundi"), Ok(Some("Reportée à lundi matin".into())));
        let monday = Snooze::Monday.until(now);
        assert!(matches!(status(&app, "c", now), Status::Snoozed { until } if until >= monday));
        assert!(phone.action(&mut app, &ctx, "item/parti", "reporter-lundi").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
