//! Ce que les relevés et les transcriptions disent ensemble.
//!
//! - **Part des limites d'une conversation** : entre deux relevés d'un même compte, la hausse du taux est partagée
//!   entre les réponses faites dans l'intervalle (sessions de ce compte), au prorata de leur coût équivalent API —
//!   le modèle, l'effort (la réflexion est comptée en sortie) et le cache pèsent ainsi ce qu'ils coûtent. Sans réponse
//!   connue dans l'intervalle, la hausse va à la session du relevé. C'est une mesure tant que les relevés couvrent
//!   la période ; avant le premier relevé (ou hors relevés), la part est **estimée** avec le rapport moyen du compte
//!   (points de pourcentage par dollar), et marquée comme telle.
//! - **Relevés périmés** : une session restée inactive redonne ses anciens taux (`readings`) ; un relevé plus bas que
//!   le plus haut de sa fenêtre, ou d'une fenêtre plus ancienne, est ignoré.
//! - **Fenêtres** : pour chaque fenêtre de 5 h et chaque semaine relevées, le maximum atteint, l'heure où 100 % a été
//!   touché, et le coût des réponses faites ensuite jusqu'à la remise à zéro (le **dépassement**, facturé à part si
//!   l'abonnement le permet).
//! - Une conversation sans compte connu (profil sans relais, clé d'API) n'a qu'un coût : c'est ce qu'elle a coûté.

use std::collections::{BTreeMap, HashMap, HashSet};

use crate::readings::{Kind, Parts, Reading, Window, same_window};
use crate::transcripts::Conversation;

/// Ce qu'on sait d'une conversation.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ConvStats {
    pub cost: f64,
    pub tokens: u64,
    /// Part cumulée des fenêtres de 5 h, puis des semaines, en points de pourcentage.
    pub share: [f64; 2],
    /// Une partie de la part est estimée (hors relevés).
    pub estimated: bool,
    /// Coût des réponses faites limite atteinte.
    pub overflow: f64,
    /// Compte de la conversation (clé de `Reading::key`).
    pub account: Option<String>,
}

/// Une fenêtre relevée.
#[derive(Clone, Debug, PartialEq)]
pub struct WindowStat {
    pub account: String,
    pub kind: Kind,
    pub start: i64,
    pub resets_at: i64,
    pub max_pct: f64,
    pub last_pct: f64,
    /// Premier relevé à 100 % ou plus.
    pub reached_at: Option<i64>,
    /// Coût des réponses faites entre `reached_at` et la remise à zéro.
    pub overflow: f64,
    /// Coût de toutes les réponses du compte dans la fenêtre.
    pub cost: f64,
    pub readings: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AccountInfo {
    pub key: String,
    pub label: String,
    /// Le dernier relevé du compte.
    pub last: Reading,
    /// Les fenêtres du compte (5 h, semaine) : la plus récente relevée, à son taux le plus haut. Le dernier relevé
    /// peut venir d'une session restée inactive, avec d'anciens taux.
    pub windows: [Option<Window>; 2],
}

impl AccountInfo {
    pub fn window(&self, kind: Kind) -> Option<Window> {
        self.windows[idx(kind)]
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Analysis {
    /// Dans l'ordre des conversations données.
    pub conversations: Vec<ConvStats>,
    pub windows: Vec<WindowStat>,
    /// Le plus récemment relevé d'abord.
    pub accounts: Vec<AccountInfo>,
    /// Parts des fenêtres en cours, pour le relais.
    pub parts: Parts,
}

fn idx(kind: Kind) -> usize {
    match kind {
        Kind::FiveHour => 0,
        Kind::SevenDay => 1,
    }
}

pub fn analyze(readings: &[Reading], convs: &[Conversation], now: i64) -> Analysis {
    let mut out = Analysis { parts: Parts { written_at: now, ..Default::default() }, ..Default::default() };
    // Comptes, et à qui appartient chaque session, chaque dossier de configuration.
    let mut latest: BTreeMap<String, &Reading> = BTreeMap::new();
    let mut by_session: HashMap<&str, String> = HashMap::new();
    let mut by_config: HashMap<&str, String> = HashMap::new();
    for r in readings {
        let key = r.key();
        if latest.get(&key).is_none_or(|l| l.t <= r.t) {
            latest.insert(key.clone(), r);
        }
        if let Some(s) = &r.session {
            by_session.insert(s, key.clone());
        }
        if let Some(c) = &r.config {
            by_config.insert(c, key.clone());
        }
    }
    let mut accounts: Vec<AccountInfo> = latest
        .iter()
        .map(|(k, r)| AccountInfo { key: k.clone(), label: r.label(), last: (*r).clone(), windows: [None; 2] })
        .collect();
    accounts.sort_by_key(|a| std::cmp::Reverse(a.last.t));
    out.accounts = accounts;

    out.conversations = convs
        .iter()
        .map(|c| ConvStats {
            cost: c.cost(),
            tokens: c.tokens(),
            account: by_session.get(c.id.as_str()).or_else(|| by_config.get(c.config.as_str())).cloned(),
            ..Default::default()
        })
        .collect();

    // Réponses de chaque compte, dans l'ordre des heures : (heure, conversation, réponse, coût).
    let mut replies: HashMap<String, Vec<(i64, usize, usize, f64)>> = HashMap::new();
    for (ci, c) in convs.iter().enumerate() {
        let Some(acc) = &out.conversations[ci].account else { continue };
        let list = replies.entry(acc.clone()).or_default();
        for (ri, r) in c.replies.iter().enumerate() {
            if r.at > 0 {
                list.push((r.at, ci, ri, r.cost()));
            }
        }
    }
    for list in replies.values_mut() {
        list.sort_by_key(|x| x.0);
    }
    let conv_of_session: HashMap<&str, usize> = convs.iter().enumerate().map(|(i, c)| (c.id.as_str(), i)).collect();

    let mut covered: [HashSet<(usize, usize)>; 2] = [HashSet::new(), HashSet::new()];
    // (compte, fenêtre) → (points de pourcentage, dollars) mesurés ensemble.
    let mut measured: HashMap<(String, Kind), (f64, f64)> = HashMap::new();
    let mut windows: BTreeMap<(String, Kind, i64), WindowStat> = BTreeMap::new();
    // (conversation, fenêtre, remise à zéro) → part.
    let mut instance_share: HashMap<(usize, Kind, i64), f64> = HashMap::new();

    let mut by_account: BTreeMap<String, Vec<&Reading>> = BTreeMap::new();
    for r in readings {
        by_account.entry(r.key()).or_default().push(r);
    }
    for (acc, list) in &by_account {
        let empty = Vec::new();
        let reps = replies.get(acc).unwrap_or(&empty);
        for kind in Kind::ALL {
            // La fenêtre en cours : sa remise à zéro (celle de son premier relevé), son plus haut taux, l'heure du
            // dernier relevé retenu.
            let mut cur: Option<(i64, f64, i64)> = None;
            for r in list.iter() {
                let Some(w) = r.window(kind) else { continue };
                let (resets_at, rise, from) = match cur {
                    Some((at, level, pt)) if same_window(at, w.resets_at) => {
                        if w.pct < level - 1e-9 {
                            continue;
                        }
                        (at, w.pct - level, pt)
                    }
                    Some((at, _, pt)) if w.resets_at > at => {
                        (w.resets_at, w.pct.max(0.0), pt.max(w.resets_at - kind.duration()))
                    }
                    Some(_) => continue,
                    None => (w.resets_at, w.pct.max(0.0), w.resets_at - kind.duration()),
                };
                let start = resets_at - kind.duration();
                cur = Some((resets_at, w.pct, r.t));
                let ws = windows.entry((acc.clone(), kind, resets_at)).or_insert_with(|| WindowStat {
                    account: acc.clone(),
                    kind,
                    start,
                    resets_at,
                    max_pct: 0.0,
                    last_pct: 0.0,
                    reached_at: None,
                    overflow: 0.0,
                    cost: 0.0,
                    readings: 0,
                });
                ws.readings += 1;
                ws.max_pct = ws.max_pct.max(w.pct);
                ws.last_pct = w.pct;
                if w.pct >= 100.0 && ws.reached_at.is_none() {
                    ws.reached_at = Some(r.t);
                }
                // Réponses du compte dans (from, t].
                let lo = reps.partition_point(|x| x.0 <= from);
                let hi = reps.partition_point(|x| x.0 <= r.t).max(lo);
                let slice = &reps[lo..hi];
                let total: f64 = slice.iter().map(|x| x.3).sum();
                for x in slice {
                    covered[idx(kind)].insert((x.1, x.2));
                }
                if rise <= 0.0 {
                    continue;
                }
                if total > 0.0 {
                    let m = measured.entry((acc.clone(), kind)).or_default();
                    m.0 += rise;
                    m.1 += total;
                    for x in slice {
                        let part = rise * x.3 / total;
                        out.conversations[x.1].share[idx(kind)] += part;
                        *instance_share.entry((x.1, kind, resets_at)).or_default() += part;
                    }
                } else if let Some(&ci) = r.session.as_deref().and_then(|s| conv_of_session.get(s)) {
                    out.conversations[ci].share[idx(kind)] += rise;
                    *instance_share.entry((ci, kind, resets_at)).or_default() += rise;
                }
            }
            if let Some((resets_at, pct, _)) = cur
                && let Some(a) = out.accounts.iter_mut().find(|a| &a.key == acc)
            {
                a.windows[idx(kind)] = Some(Window { pct, resets_at });
            }
        }
    }

    // Hors relevés : estimation au rapport moyen du compte (sinon de tous les comptes).
    let global: HashMap<Kind, f64> = Kind::ALL
        .iter()
        .map(|k| {
            let (p, d) =
                measured.iter().filter(|((_, kk), _)| kk == k).fold((0.0, 0.0), |a, (_, m)| (a.0 + m.0, a.1 + m.1));
            (*k, if d > 0.0 { p / d } else { 0.0 })
        })
        .collect();
    for (ci, c) in convs.iter().enumerate() {
        let Some(acc) = out.conversations[ci].account.clone() else { continue };
        for kind in Kind::ALL {
            let ratio =
                measured.get(&(acc.clone(), kind)).filter(|m| m.1 > 0.0).map(|m| m.0 / m.1).unwrap_or(global[&kind]);
            let loose: f64 = c
                .replies
                .iter()
                .enumerate()
                .filter(|(ri, _)| !covered[idx(kind)].contains(&(ci, *ri)))
                .map(|(_, r)| r.cost())
                .sum();
            if loose > 0.0 && ratio > 0.0 {
                out.conversations[ci].share[idx(kind)] += loose * ratio;
                out.conversations[ci].estimated = true;
            }
        }
    }

    // Coût et dépassement de chaque fenêtre ; dépassement de chaque conversation (une réponse comptée une fois).
    let mut over: HashSet<(usize, usize)> = HashSet::new();
    for ws in windows.values_mut() {
        let Some(reps) = replies.get(&ws.account) else { continue };
        let lo = reps.partition_point(|x| x.0 < ws.start);
        let hi = reps.partition_point(|x| x.0 < ws.resets_at);
        for x in &reps[lo..hi] {
            ws.cost += x.3;
            if ws.reached_at.is_some_and(|at| x.0 >= at) {
                ws.overflow += x.3;
                if over.insert((x.1, x.2)) {
                    out.conversations[x.1].overflow += x.3;
                }
            }
        }
    }

    // Les parts des fenêtres en cours de chaque compte, pour le relais.
    for a in &out.accounts {
        for kind in Kind::ALL {
            let Some(w) = a.window(kind) else { continue };
            for ((ci, k, at), pct) in &instance_share {
                if *k == kind && *at == w.resets_at && out.conversations[*ci].account.as_deref() == Some(a.key.as_str())
                {
                    out.parts.set(&convs[*ci].id, kind, *at, *pct);
                }
            }
        }
    }
    let mut ws: Vec<WindowStat> = windows.into_values().collect();
    ws.sort_by_key(|w| std::cmp::Reverse(w.resets_at));
    out.windows = ws;
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transcripts::Reply;

    fn reading(t: i64, session: &str, five: f64, resets: i64) -> Reading {
        Reading {
            t,
            account: "u".into(),
            config: Some("cfg".into()),
            session: Some(session.into()),
            five_hour: Some(Window { pct: five, resets_at: resets }),
            ..Default::default()
        }
    }

    /// Une réponse d'Opus 5.5 qui coûte `dollars` (sortie seule : 20 $ le million).
    fn reply(id: &str, at: i64, dollars: f64) -> Reply {
        Reply {
            id: id.into(),
            at,
            model: "claude-opus-5-5".into(),
            output: (dollars * 50_000.0) as u64,
            ..Default::default()
        }
    }

    fn conv(id: &str, config: &str, replies: Vec<Reply>) -> Conversation {
        Conversation { id: id.into(), config: config.into(), replies, ..Default::default() }
    }

    const R: i64 = 100_000;

    #[test]
    fn a_rise_is_shared_by_cost_between_the_conversations_that_worked() {
        let readings = [reading(90_000, "a", 10.0, R), reading(91_000, "b", 40.0, R)];
        let convs = [
            conv("a", "cfg", vec![reply("1", 90_500, 1.0)]),
            conv("b", "cfg", vec![reply("2", 90_600, 2.0)]),
            conv("api", "autre", vec![reply("3", 90_700, 5.0)]),
        ];
        let an = analyze(&readings, &convs, 92_000);
        let [a, b, api] = &an.conversations[..] else { panic!() };
        // Première fenêtre : 10 % avant le premier relevé (aucune réponse alors : à « a ») ; puis 30 % à partager 1 : 2.
        assert!((a.share[0] - (10.0 + 10.0)).abs() < 1e-9, "{a:?}");
        assert!((b.share[0] - 20.0).abs() < 1e-9, "{b:?}");
        assert!(!a.estimated && !b.estimated, "mesurées");
        assert_eq!(api.account, None, "profil sans relais : un coût, pas de part");
        assert_eq!(api.share, [0.0, 0.0]);
        assert!((api.cost - 5.0).abs() < 1e-9);
        let w = &an.windows[0];
        assert_eq!((w.max_pct, w.readings, w.reached_at), (40.0, 2, None));
        assert!((an.parts.sessions["b"]["five_hour"].1 - 20.0).abs() < 1e-9, "parts pour le relais");
    }

    #[test]
    fn replies_before_the_first_reading_are_estimated_from_the_measured_ratio() {
        // 30 % pour 3 $ mesurés : 10 points par dollar ; une vieille réponse de 0,5 $ vaut 5 points estimés.
        let readings = [reading(90_000, "a", 10.0, R), reading(91_000, "a", 40.0, R)];
        let convs =
            [conv("a", "cfg", vec![reply("1", 90_500, 3.0)]), conv("vieille", "cfg", vec![reply("2", 10, 0.5)])];
        let an = analyze(&readings, &convs, 92_000);
        let old = &an.conversations[1];
        assert!(old.estimated);
        assert!((old.share[0] - 5.0).abs() < 1e-9, "{old:?}");
    }

    #[test]
    fn reaching_the_limit_and_what_followed_are_counted() {
        let readings = [reading(90_000, "a", 90.0, R), reading(91_000, "a", 100.0, R), reading(92_000, "a", 100.0, R)];
        let convs = [conv("a", "cfg", vec![reply("1", 90_500, 1.0), reply("2", 91_500, 4.0)])];
        let an = analyze(&readings, &convs, 92_500);
        let w = &an.windows[0];
        assert_eq!(w.reached_at, Some(91_000));
        assert!((w.overflow - 4.0).abs() < 1e-9, "{w:?}");
        assert!((an.conversations[0].overflow - 4.0).abs() < 1e-9);
        assert!((w.cost - 5.0).abs() < 1e-9);
    }

    #[test]
    fn a_stale_session_neither_lowers_the_rate_nor_hides_a_rise() {
        // 10 %, une réponse de « a », une vieille session redit 5 %, puis 20 % : la hausse de 10 points va à la
        // réponse de « a », faite avant le relevé périmé ; le taux du compte reste le plus haut relevé.
        let readings = [
            reading(90_000, "a", 10.0, R),
            reading(90_200, "vieille", 5.0, R),
            reading(91_000, "a", 20.0, R),
            reading(91_100, "vieille", 5.0, R),
        ];
        let convs = [conv("a", "cfg", vec![reply("1", 90_100, 1.0)]), conv("vieille", "cfg", vec![])];
        let an = analyze(&readings, &convs, 92_000);
        assert!((an.conversations[0].share[0] - 20.0).abs() < 1e-9, "{:?}", an.conversations[0]);
        assert_eq!(an.conversations[1].share[0], 0.0);
        let a = &an.accounts[0];
        assert_eq!(a.last.session.as_deref(), Some("vieille"), "le dernier relevé vient de la vieille session");
        assert_eq!(a.window(Kind::FiveHour), Some(Window { pct: 20.0, resets_at: R }));
        let w = &an.windows[0];
        assert_eq!((w.max_pct, w.last_pct, w.readings), (20.0, 20.0, 2));
    }

    #[test]
    fn a_new_window_counts_from_its_reset() {
        let readings = [reading(1000, "a", 90.0, 1500), reading(1600 + FIVE, "b", 5.0, 1500 + FIVE)];
        let convs = [conv("a", "cfg", vec![]), conv("b", "cfg", vec![reply("1", 1700 + FIVE, 1.0)])];
        let an = analyze(&readings, &convs, 1700 + FIVE);
        assert!((an.conversations[1].share[0] - 5.0).abs() < 1e-9);
        assert_eq!(an.windows.len(), 2, "deux fenêtres de 5 h");
    }

    const FIVE: i64 = crate::readings::FIVE_HOURS;
}
