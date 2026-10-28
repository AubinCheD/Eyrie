//! Owlery sur le téléphone : la boîte de réception par sorte (à relire, CI en échec, mentions, tickets, mes PR), le
//! détail d'une entrée, et les gestes de la liste du PC : marquer lue ou non lue, traitée, reportée à demain, ignorer
//! le dépôt, corriger avec Claude (une session dans Fawkes, que le téléphone ouvre ensuite), ouvrir sur le téléphone
//! ou sur le PC, actualiser.
//!
//! Tout se lit dans l'état vivant d'Owlery (`App::state`) et passe par ses propres méthodes (`mark`, `mute`,
//! `work_with_claude`) : le PC montre aussitôt le même changement. « Actualiser » passe par le canal des demandes
//! d'Owlery (`contract::Incoming::Refresh`).

use std::sync::mpsc::Sender;
use std::time::{SystemTime, UNIX_EPOCH};

use owlery::app::App;
use owlery::contract::Incoming;
use owlery::inbox::{Inbox, Rules, Snooze};
use owlery::model::{Ci, Item, Kind, Status};
use suite_sdk::mobile::msg::MODULE_PREFIX;
use suite_sdk::mobile::ui::{Act, Row, Screen, Tone};

/// Entrées montrées par sorte dans l'accueil.
const PER_KIND: usize = 15;

fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// Le titre d'une section du téléphone.
fn kind_title(k: Kind) -> &'static str {
    match k {
        Kind::ReviewRequested => "À relire",
        Kind::CiFailed => "CI en échec",
        Kind::Mention => "Mentions",
        Kind::Assigned => "Tickets assignés",
        Kind::MyPr => "Mes PR",
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

/// Les entrées à montrer : ni masquées par les règles, ni reportées, ni terminées.
fn visible<'a>(inbox: &'a Inbox, rules: &Rules, now: i64) -> Vec<(&'a Item, Status)> {
    let mut v: Vec<(&Item, Status)> = inbox
        .items
        .values()
        .filter(|i| rules.shows(i))
        .map(|i| (i, inbox.status(i, now)))
        .filter(|(_, s)| matches!(s, Status::Unread | Status::Read))
        .collect();
    v.sort_by(|(a, sa), (b, sb)| {
        (*sb == Status::Unread).cmp(&(*sa == Status::Unread)).then(b.updated_at.cmp(&a.updated_at))
    });
    v
}

/// L'accueil : une section par sorte, non lues d'abord.
pub fn home(inbox: &Inbox, rules: &Rules, now: i64) -> Screen {
    let all = visible(inbox, rules, now);
    let unread = all.iter().filter(|(_, s)| *s == Status::Unread).count();
    let subtitle = match unread {
        0 => "Rien en attente".to_string(),
        1 => "1 en attente".to_string(),
        n => format!("{n} en attente"),
    };
    let mut s = Screen::new("Owlery").subtitle(subtitle).act(Act::new("actualiser", "Actualiser").glyph("🔄"));
    if all.is_empty() {
        s.empty("Boîte de réception vide.");
    }
    for kind in Kind::ALL {
        let items: Vec<&(&Item, Status)> = all.iter().filter(|(i, _)| i.kind == kind).collect();
        if items.is_empty() {
            continue;
        }
        let waiting = items.iter().filter(|(_, s)| *s == Status::Unread).count();
        let aside = if waiting > 0 { format!("{waiting} / {}", items.len()) } else { items.len().to_string() };
        s.section(kind_title(kind), aside);
        for (item, status) in items.into_iter().take(PER_KIND) {
            let mut sub = reference(item);
            if !item.author.is_empty() {
                sub = format!("{sub} · {}", item.author);
            }
            let mut row =
                Row::new(&item.title).subtitle(sub).detail(ago(item.updated_at, now)).open(format!("item/{}", item.id));
            if let Some(t) = ci_tone(item.ci) {
                row = row.tone(t);
            }
            if *status == Status::Unread {
                row = row.badge("non lu").act(Act::new(format!("lu:{}", item.id), "Marquer lue").glyph("✅"));
            } else {
                row = row.act(Act::new(format!("non-lu:{}", item.id), "Marquer non lue").glyph("⚪"));
            }
            row = row
                .act(Act::new(format!("fait:{}", item.id), "Traitée").glyph("☑️"))
                .act(Act::new(format!("reporter:{}", item.id), "Reporter à demain matin").glyph("⏰"));
            s.row(row);
        }
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
    s.acts.push(Act::new("reporter", "Reporter à demain matin").glyph("⏰"));
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
        if path.is_empty() {
            return Some(home(&st.inbox, &st.settings.rules, now));
        }
        let id = path.strip_prefix("item/")?;
        let i = st.inbox.items.get(id)?;
        Some(item(i, st.inbox.status(i, now), now, app.knows_local_dir(id)))
    }

    /// Un geste : sur l'entrée de l'écran (`item/<id>`), ou sur une ligne de l'accueil (`lu:<id>`…).
    pub fn action(
        &self,
        app: &mut App,
        ctx: &suite_sdk::egui::Context,
        path: &str,
        action: &str,
    ) -> Result<Option<String>, String> {
        if action == "actualiser" {
            return self.refresh();
        }
        let (verb, id) = match action.split_once(':') {
            Some((verb, id)) => (verb, id.to_string()),
            None => (action, path.strip_prefix("item/").ok_or("action inconnue")?.to_string()),
        };
        let Some(i) = app.state().inbox.items.get(&id).cloned() else {
            return Err("Cette entrée n'existe plus.".into());
        };
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
            "reporter" => {
                app.mark(&id, Status::Snoozed { until: Snooze::Tomorrow.until(now()) });
                "Reportée à demain matin"
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
    use std::sync::mpsc;

    use owlery::model::{Check, Mark};
    use suite_sdk::mobile::ui::Block;

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
            entry("lue", Kind::Mention, "contoso/api", 8, now - 3600, None),
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

    #[test]
    fn the_home_groups_by_kind_with_unread_first() {
        let now = 1_800_000_000;
        let (inbox, rules) = sample(now);
        let s = home(&inbox, &rules, now);
        assert_eq!(s.subtitle, "3 en attente");
        let sections: Vec<(&str, &str)> = s
            .blocks
            .iter()
            .filter_map(|b| match b {
                Block::Section { title, aside, .. } => Some((title.as_str(), aside.as_str())),
                _ => None,
            })
            .collect();
        assert_eq!(sections, [("À relire", "2 / 2"), ("CI en échec", "1 / 1"), ("Mentions", "1")]);
        // La plus récente d'abord ; ni terminée, ni dépôt ignoré.
        assert_eq!(s.opens().len(), 4);
        let first = s.blocks.iter().find_map(|b| match b {
            Block::Row(r) => Some(r),
            _ => None,
        });
        let first = first.unwrap();
        assert_eq!((first.title.as_str(), first.subtitle.as_str()), ("PR 13", "contoso/web#13 · ada"));
        assert_eq!(first.badge, "non lu");
        let ids = s.action_ids();
        assert!(ids.contains("actualiser") && ids.contains("lu:b") && ids.contains("reporter:b"), "{ids:?}");
    }

    #[test]
    fn an_item_opens_on_the_phone_or_on_the_pc() {
        let now = 1_800_000_000;
        let mut i = entry("c", Kind::CiFailed, "contoso/api", 7, now - 7200, Some(Ci::Failure));
        i.checks = vec![Check { name: "build".into(), url: "https://ci.example/1".into() }];
        let s = item(&i, Status::Unread, now, false);
        assert_eq!(s.subtitle, "contoso/api#7 · CI en échec");
        assert_eq!(s.action_ids().into_iter().collect::<Vec<_>>(), ["fait", "ignorer", "lu", "pc", "reporter"]);
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
}
