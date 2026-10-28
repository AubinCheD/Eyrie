//! Gringotts sur le téléphone : les limites en cours (5 h et semaine, un bandeau par compte, en alerte au-delà de
//! 80 %), le coût du jour, les conversations les plus coûteuses de la semaine, et le détail de chacune.

use suite_sdk::mobile::ui::{Act, Band, Row, Screen, Tone};

use crate::analysis::{AccountInfo, ConvStats};
use crate::readings::{self, Kind};
use crate::transcripts::Conversation;
use crate::ui::{ago, money, tokens};

/// Conversations montrées dans l'accueil.
const TOP: usize = 10;

/// Ce que l'accueil tire des conversations : des milliers de réponses, relues quand les conversations ou le jour
/// changent, pas à chaque demande du téléphone.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Summary {
    /// Début du jour local (secondes Unix).
    pub day: i64,
    pub today_cost: f64,
    /// Conversations qui ont eu au moins une réponse aujourd'hui.
    pub today_convs: usize,
    /// Les plus coûteuses des 7 derniers jours (indices dans les conversations), la plus coûteuse d'abord.
    pub top: Vec<usize>,
}

/// Minuit, heure locale, du jour de `now`.
pub fn day_start(now: i64) -> i64 {
    use chrono::{Local, TimeZone};
    chrono::DateTime::from_timestamp(now, 0)
        .map(|d| d.with_timezone(&Local).date_naive())
        .and_then(|d| d.and_hms_opt(0, 0, 0))
        .and_then(|d| Local.from_local_datetime(&d).earliest())
        .map_or(now - now.rem_euclid(86_400), |d| d.timestamp())
}

pub fn summarize(convs: &[Conversation], stats: &[ConvStats], day: i64, now: i64) -> Summary {
    let mut today_cost = 0.0;
    let mut today_convs = 0;
    // `ended` est l'heure de la dernière réponse : les conversations d'avant aujourd'hui sont sautées sans lire leurs
    // réponses.
    for c in convs.iter().filter(|c| c.ended >= day) {
        let today: Vec<f64> = c.replies.iter().filter(|r| r.at >= day).map(|r| r.cost()).collect();
        if !today.is_empty() {
            today_convs += 1;
            today_cost += today.iter().sum::<f64>();
        }
    }
    let since = now - 7 * 86_400;
    let cost = |i: usize| stats.get(i).map_or(0.0, |s| s.cost);
    let mut top: Vec<usize> = (0..convs.len()).filter(|i| convs[*i].ended >= since && cost(*i) > 0.0).collect();
    top.sort_by(|a, b| cost(*b).total_cmp(&cost(*a)));
    top.truncate(TOP);
    Summary { day, today_cost, today_convs, top }
}

fn tone(pct: f64) -> Tone {
    if pct >= 100.0 {
        Tone::Danger
    } else if pct >= 80.0 {
        Tone::Warn
    } else {
        Tone::Ok
    }
}

fn title(c: &Conversation) -> String {
    if c.title.trim().is_empty() { "(sans titre)".into() } else { c.title.clone() }
}

/// « 5 h 4 % · semaine 1 % » : la part des limites d'une conversation, en points ; aucune sans compte connu.
fn share_text(s: &ConvStats) -> Option<String> {
    let mark = if s.estimated { " (estimée)" } else { "" };
    s.account
        .as_ref()
        .map(|_| format!("5 h {} · semaine {}{mark}", readings::pct_text(s.share[0]), readings::pct_text(s.share[1])))
}

/// L'accueil ; `status` : où en est la lecture des conversations.
pub fn home(
    accounts: &[AccountInfo],
    convs: &[Conversation],
    stats: &[ConvStats],
    sum: &Summary,
    status: &str,
    now: i64,
) -> Screen {
    let mut s = Screen::new("Gringotts").subtitle(status).act(Act::new("relire", "Relire").glyph("🔄"));
    if accounts.is_empty() {
        s.note(
            "Aucun relevé des limites : installez le relais de Gringotts depuis ses réglages, sur le PC.",
            Tone::Info,
        );
    }
    for a in accounts {
        let current = |kind| a.window(kind).filter(|w| w.resets_at > now);
        let worst = Kind::ALL.into_iter().filter_map(|k| current(k).map(|w| w.pct)).fold(0.0, f64::max);
        let mut band = Band::new(format!("compte/{}", a.key), &a.label)
            .aside(format!("relevé {}", ago(a.last.t, now)))
            .glyph("📊");
        if worst >= 80.0 {
            // Visible même plié.
            band = band.badge(readings::pct_text(worst)).tone(tone(worst));
        }
        s.band(band);
        for kind in Kind::ALL {
            let w = current(kind);
            let pct = w.map_or(0.0, |w| w.pct);
            let text = match w {
                Some(w) => {
                    let fmt = if kind == Kind::FiveHour { "%H:%M" } else { "%d/%m à %H:%M" };
                    format!("{} · remise à zéro {}", readings::pct_text(pct), readings::local(w.resets_at, fmt))
                }
                None => "0 % · pas de fenêtre en cours".into(),
            };
            let label = if kind == Kind::FiveHour { "5 h" } else { "Semaine" };
            s.meter(label, (pct / 100.0) as f32, text, tone(pct));
        }
    }
    s.band(Band::new("jour", "Aujourd'hui").aside(money(sum.today_cost)).glyph("💰"));
    let n = sum.today_convs;
    let convs_text = match n {
        0 => "aucune conversation".to_string(),
        1 => "1 conversation".to_string(),
        n => format!("{n} conversations"),
    };
    s.row(Row::new("Coût équivalent API").subtitle(convs_text).detail(money(sum.today_cost)).glyph("💰"));
    s.band(Band::new("couteuses", "Les plus coûteuses, 7 jours").aside(sum.top.len().to_string()).glyph("🔥"));
    if sum.top.is_empty() {
        s.empty("Aucune conversation cette semaine.");
    }
    for &i in &sum.top {
        let (Some(c), Some(st)) = (convs.get(i), stats.get(i)) else { continue };
        let mut sub = format!("{} · {}", c.repo(), ago(c.ended, now));
        if let Some(share) = share_text(st) {
            sub = format!("{sub} · {share}");
        }
        let mut row = Row::new(title(c))
            .key(format!("conv/{}", c.id))
            .subtitle(sub)
            .detail(money(st.cost))
            .open(format!("conv/{}", c.id));
        if st.overflow > 0.0 {
            row = row.tone(Tone::Warn).badge("dépassement");
        }
        s.row(row);
    }
    s
}

/// Le détail d'une conversation.
pub fn conversation(c: &Conversation, st: Option<&ConvStats>, accounts: &[AccountInfo], now: i64) -> Screen {
    let place = match &c.branch {
        Some(b) => format!("{} · {b}", c.repo()),
        None => c.repo(),
    };
    let mut s = Screen::new(title(c)).subtitle(place);
    if c.gone {
        s.note("Transcription effacée par Claude Code : seul son résumé reste.", Tone::Info);
    }
    let st = st.cloned().unwrap_or_default();
    s.row(Row::new("Coût").subtitle("équivalent API").detail(money(st.cost)));
    s.row(Row::new("Jetons").detail(tokens(st.tokens)));
    match share_text(&st) {
        Some(share) => s.row(Row::new("Part des limites").subtitle(share)),
        None => s.row(Row::new("Part des limites").subtitle("compte inconnu : clé d'API ou profil sans relais")),
    };
    if st.overflow > 0.0 {
        s.row(
            Row::new("Dépassement")
                .subtitle("réponses faites limite atteinte")
                .detail(money(st.overflow))
                .tone(Tone::Warn),
        );
    }
    if let Some(a) = st.account.as_ref().and_then(|k| accounts.iter().find(|a| &a.key == k)) {
        s.row(Row::new("Compte").subtitle(&a.label));
    }
    let models = c.models();
    if !models.is_empty() {
        s.row(Row::new("Modèles").subtitle(models.join(", ")));
    }
    let subagents = match c.subagents {
        0 => String::new(),
        1 => "1 sous-agent".into(),
        n => format!("{n} sous-agents"),
    };
    s.row(Row::new("Demandes").subtitle(subagents).detail(c.prompts.to_string()));
    s.row(Row::new("Début").detail(readings::local(c.started, "%d/%m %H:%M")));
    s.row(Row::new("Dernière réponse").subtitle(ago(c.ended, now)).detail(readings::local(c.ended, "%d/%m %H:%M")));
    if !c.cwd.is_empty() {
        s.row(Row::new("Dossier").subtitle(&c.cwd));
    }
    s
}

#[cfg(test)]
mod tests {
    use suite_sdk::mobile::ui::{Block, Fold};

    use super::*;
    use crate::readings::{Reading, Window};
    use crate::transcripts::Reply;

    fn conv(id: &str, title: &str, at: i64, output: u64) -> Conversation {
        Conversation {
            id: id.into(),
            cwd: r"C:\dev\ozmo".into(),
            title: title.into(),
            started: at - 60,
            ended: at,
            prompts: 2,
            replies: vec![Reply {
                id: format!("m-{id}"),
                at,
                model: "claude-opus-5-5".into(),
                output,
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    #[test]
    fn the_home_shows_the_limits_today_and_the_costliest_conversations() {
        let now = 1_800_000_000;
        let day = day_start(now);
        let convs = vec![
            conv("a", "Export CSV", now - 60, 100_000),
            conv("b", "", now - 2 * 86_400, 400_000),
            conv("vieille", "Il y a un mois", now - 30 * 86_400, 900_000),
        ];
        let stats: Vec<ConvStats> = convs
            .iter()
            .map(|c| ConvStats { cost: c.cost(), account: Some("u-1".into()), share: [4.0, 1.0], ..Default::default() })
            .collect();
        let sum = summarize(&convs, &stats, day, now);
        assert_eq!(sum.today_convs, 1);
        assert!((sum.today_cost - 2.0).abs() < 1e-9, "100 000 jetons de sortie d'Opus 5.5 : {}", sum.today_cost);
        assert_eq!(sum.top, [1, 0], "la plus coûteuse d'abord, rien d'avant la semaine");

        let account = AccountInfo {
            key: "u-1".into(),
            label: "ada@contoso.example · PDLC".into(),
            last: Reading { t: now - 120, ..Default::default() },
            windows: [
                Some(Window { pct: 92.0, resets_at: now + 3600 }),
                Some(Window { pct: 45.0, resets_at: now - 1 }),
            ],
        };
        let s = home(std::slice::from_ref(&account), &convs, &stats, &sum, "3 conversations", now);
        let meters: Vec<(f32, Tone)> = s
            .blocks
            .iter()
            .filter_map(|b| match b {
                Block::Meter { value, tone, .. } => Some((*value, *tone)),
                _ => None,
            })
            .collect();
        assert_eq!(meters, [(0.92, Tone::Warn), (0.0, Tone::Ok)], "semaine remise à zéro depuis le relevé : 0 %");
        // Un bandeau par compte, en alerte au-delà de 80 % (le pourcentage se voit plié), puis le jour et la semaine.
        let bands: Vec<(&str, Fold, &str, Option<Tone>)> = s
            .blocks
            .iter()
            .filter_map(|b| match b {
                Block::Section { key, fold, badge, tone, .. } => Some((key.as_str(), *fold, badge.as_str(), *tone)),
                _ => None,
            })
            .collect();
        assert_eq!(
            bands,
            [
                ("compte/u-1", Fold::Open, "92 %", Some(Tone::Warn)),
                ("jour", Fold::Open, "", None),
                ("couteuses", Fold::Open, "", None)
            ]
        );
        assert_eq!(s.action_ids().into_iter().collect::<Vec<_>>(), ["relire"]);
        assert_eq!(s.opens().into_iter().collect::<Vec<_>>(), ["conv/a", "conv/b"]);
        assert!(s.blocks.iter().any(|b| matches!(b, Block::Row(r) if r.title == "(sans titre)")));

        let empty = home(&[], &[], &[], &Summary::default(), "", now);
        assert!(matches!(&empty.blocks[0], Block::Note { tone: Tone::Info, .. }), "relais à installer");

        let detail = conversation(&convs[0], stats.first(), std::slice::from_ref(&account), now);
        assert_eq!(detail.title, "Export CSV");
        assert!(detail.blocks.iter().any(|b| matches!(b, Block::Row(r) if r.subtitle == account.label)));
    }
}
