//! Gringotts de bout en bout : ses vues avec des conversations et des relevés d'essai (aucune lecture du disque, aucun
//! relais installé : ni `~/.claude` ni `EyrieSuite` ne sont touchés).

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;

use super::*;
use crate::readings::Window;
use crate::transcripts::Reply;

fn host(name: &str) -> Host {
    let dir = std::env::temp_dir().join(format!("suite-gringotts-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    Host::new(egui::Context::default(), dir, suite_sdk::theme::ThemeChoice::Graphite)
}

fn reading(t: i64, session: &str, five: f64, week: f64, now: i64) -> Reading {
    Reading {
        t,
        account: "u-1".into(),
        email: Some("ada@contoso.example".into()),
        org_name: Some("PDLC".into()),
        config: Some("cfg".into()),
        session: Some(session.into()),
        five_hour: Some(Window { pct: five, resets_at: now + 3600 }),
        seven_day: Some(Window { pct: week, resets_at: now + 3 * 86_400 }),
        ..Default::default()
    }
}

fn conversation(id: &str, config: &str, cwd: &str, title: &str, at: i64, output: u64) -> Conversation {
    Conversation {
        id: id.into(),
        config: config.into(),
        cwd: cwd.into(),
        title: title.into(),
        started: at - 60,
        ended: at,
        prompts: 1,
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

fn sample() -> (Vec<Conversation>, Vec<Reading>) {
    let now = now();
    let readings = vec![reading(now - 3600, "s1", 20.0, 40.0, now), reading(now - 60, "s1", 35.0, 45.0, now)];
    let conversations = vec![
        conversation("s1", "cfg", r"C:\dev\ozmo", "Export CSV", now - 1800, 100_000),
        conversation("s2", "api", r"C:\dev\diapaz", "Migration", now - 7200, 50_000),
    ];
    (conversations, readings)
}

fn gringotts(name: &str) -> Gringotts {
    let mut g = Gringotts::open(host(name), host(name).module_dir(ID), vec![Profile::default_profile()], false);
    let (c, r) = sample();
    g.set_data(c, r);
    g
}

fn harness(g: Gringotts, view: &'static str) -> Harness<'static, Gringotts> {
    let mut h = Harness::builder()
        .with_size(egui::vec2(1400.0, 900.0))
        .with_context_setup(|ctx| ctx.set_fonts(suite_sdk::fonts::FontPlan::suite().definitions()))
        .build_ui_state(
            move |ui, g: &mut Gringotts| {
                let h = g.host.clone();
                let mut frame = eframe::Frame::_new_kittest();
                let mut cx = Cx { host: &h, frame: &mut frame, active: true, visible: true };
                g.ui(ui, view, &mut cx);
            },
            g,
        );
    h.run_steps(3);
    h
}

#[test]
fn conversations_show_their_cost_and_share_of_the_limits() {
    let h = harness(gringotts("vue"), ViewInfo::MAIN);
    assert!(h.query_by_label("Export CSV").is_some());
    assert!(h.query_by_label("Migration").is_some());
    assert!(h.query_by_label("ada@contoso.example · PDLC").is_some(), "carte du compte");
    // 100 000 jetons de sortie d'Opus 5.5 : 2 $ ; 50 000 : 1 $.
    assert!(h.query_by_label("3,00 $").is_some(), "total de la période");
    assert_eq!(
        h.query_all_by_label("—").count(),
        2,
        "la conversation du profil sans relais n'a pas de part (5 h, semaine)"
    );
}

#[test]
fn on_a_wide_screen_the_table_stays_on_screen_beside_the_repositories() {
    let h = harness(gringotts("large"), ViewInfo::MAIN);
    let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 900.0));
    let header = h.get_by_label("Conversation").rect();
    let title = h.get_by_label("Export CSV").rect();
    let repo = h.get_by_label("Dépôt ozmo").rect();
    assert!(screen.contains_rect(title), "la conversation à l'écran : {title:?}");
    assert!(title.top() >= header.bottom(), "les lignes sous les titres des colonnes : {header:?} {title:?}");
    assert!(repo.left() > title.right(), "les dépôts à droite du tableau : {repo:?}");
    assert!(screen.contains_rect(repo), "les dépôts à l'écran : {repo:?}");
}

#[test]
fn the_repository_filter_keeps_one_repository() {
    let mut h = harness(gringotts("filtre"), ViewInfo::MAIN);
    h.state_mut().view.repo = Some("diapaz".into());
    h.run_steps(2);
    assert!(h.query_by_label("Export CSV").is_none());
    assert!(h.query_by_label("Migration").is_some());
}

#[test]
fn the_limits_view_shows_the_windows_and_their_maximum() {
    let h = harness(gringotts("limites"), "limites");
    assert!(h.query_by_label_contains("Fenêtres de 5 h à 100 % : 0 sur 1").is_some());
    assert!(h.query_by_label("max 45 %").is_some(), "la semaine en cours");
}

#[test]
fn the_status_line_shows_the_current_limits() {
    let g = gringotts("etat");
    let s = g.status().status_line.expect("toujours là");
    assert!(s.text.starts_with("Claude 5 h 35 % · semaine 45 %"), "{}", s.text);
    assert_eq!(s.tone, Tone::Ok);
    assert!(s.tip.contains("ada@contoso.example"));
    let empty = Gringotts::open(host("vide"), host("vide").module_dir(ID), Vec::new(), false);
    assert!(empty.status().status_line.unwrap().text.contains("relais à installer"));
}

#[test]
fn the_status_line_keeps_the_highest_rate_and_both_windows() {
    let now = now();
    let open = |name: &str, readings: Vec<Reading>| {
        let mut g = Gringotts::open(host(name), host(name).module_dir(ID), vec![Profile::default_profile()], false);
        g.set_data(Vec::new(), readings);
        g.status().status_line.unwrap().text
    };
    // Le dernier relevé vient d'une session restée inactive : semaine 7 %, sans fenêtre de 5 h (passée pour elle).
    let mut stale = reading(now - 60, "vieille", 0.0, 7.0, now);
    stale.five_hour = None;
    let text = open("perime", vec![reading(now - 120, "s1", 1.0, 23.0, now), stale]);
    assert!(text.starts_with("Claude 5 h 1 % · semaine 23 %"), "{text}");
    // Sans fenêtre de 5 h en cours : 0 %, la limite reste affichée.
    let mut week_only = reading(now - 60, "s1", 0.0, 23.0, now);
    week_only.five_hour = None;
    let text = open("sans-5h", vec![week_only]);
    assert!(text.starts_with("Claude 5 h 0 % · semaine 23 %"), "{text}");
}

#[test]
fn without_readings_the_view_offers_to_install_the_relay() {
    let mut g = Gringotts::open(host("sans"), host("sans").module_dir(ID), vec![Profile::default_profile()], false);
    let (c, _) = sample();
    g.set_data(c, Vec::new());
    let h = harness(g, ViewInfo::MAIN);
    assert!(h.query_by_label("Installer pour « Par défaut »").is_some());
}

#[test]
fn the_phone_sees_the_limits_and_a_conversation_and_can_ask_for_a_rereading() {
    let mut g = gringotts("telephone");
    let h = g.host.clone();
    let mut frame = eframe::Frame::_new_kittest();
    let mut cx = Cx { host: &h, frame: &mut frame, active: false, visible: false };
    let home = g.mobile_screen("", &mut cx).expect("accueil");
    assert!(home.opens().contains("conv/s1"));
    assert_eq!(g.mobile_screen("conv/s1", &mut cx).map(|s| s.title), Some("Export CSV".into()));
    assert!(g.mobile_screen("conv/inconnue", &mut cx).is_none());
    assert_eq!(g.mobile_action("", "relire", None, &mut cx), Ok(Some("Relecture lancée".into())));
    assert!(g.mobile_action("", "autre", None, &mut cx).is_err());
}
