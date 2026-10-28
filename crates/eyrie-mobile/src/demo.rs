//! Démonstration sans PC (`EYRIE_MOBILE_DEMO=<scène>`), pour les captures et la mise au point de l'interface :
//! `ensemble`, `fawkes`, `terminal`, `patronus`, `plus`, `notes`, `reglages`, `accueil`, `accueil-tailscale`, `code`,
//! `appairage`, `hors-ligne`.

use suite_mobile::msg::{Badge, CommandCard, ModuleCard, Note, ToPhone, Waiting};
use suite_mobile::term::{self, Color, Line, Run, TermFrame, TermView};
use suite_mobile::ui::{Act, Row, Screen, Tone};

use crate::AppWindow;
use crate::app::with;
use crate::tailnet::Tailnet;

/// L'état d'un nœud Tailscale, à l'écran (le bureau n'en a pas).
fn show_tailnet(ui: &AppWindow, t: Tailnet) {
    let (state, text) = t.display();
    ui.set_tailnet_state(state);
    ui.set_tailnet_text(text.into());
}

fn card(id: &str, name: &str, glyph: &str, color: u32, badge: Option<(u32, Tone)>, status: &str) -> ModuleCard {
    ModuleCard {
        id: id.into(),
        name: name.into(),
        glyph: glyph.into(),
        color,
        running: true,
        reason: String::new(),
        badge: badge.map(|(n, tone)| Badge { count: Some(n), tone, tip: String::new() }),
        status: status.into(),
        summary: String::new(),
    }
}

fn modules() -> Vec<ModuleCard> {
    vec![
        card("fawkes", "Fawkes", "🔥", 0xf26d3b, Some((1, Tone::Warn)), "6 terminaux"),
        card("dragontree", "DragonTree", "🌳", 0xe08a5c, None, "Ozmo · main ↑2"),
        card("owlery", "Owlery", "🦉", 0x3aa8d8, Some((3, Tone::Info)), "3 PR à relire"),
        card("patronus", "Patronus", "🦌", 0x9fc5ff, Some((2, Tone::Info)), "1 tâche en cours"),
        card("menagerie", "Menagerie", "🐾", 0x4cb872, None, "4 services"),
        card("grimoire", "Grimoire", "📖", 0xb07ce0, None, "2 profils"),
        card("gringotts", "Gringotts", "🏦", 0xe8b84a, None, "5 h 38 % · 7 j 61 %"),
        card("carnet", "Carnet", "🗒", 0x8a919d, None, ""),
    ]
}

fn fawkes_home() -> Screen {
    let mut s = Screen::new("Fawkes").act(Act::new("@pc", "Montrer sur le PC"));
    s.note("Une session Claude attend une réponse.", Tone::Warn);
    s.row(Row::new("Nouveau terminal").glyph("➕").subtitle("Shell ou Claude, dans un dossier récent").open("nouveau"));
    s.section("Eyrie", "3");
    s.row(
        Row::new("Claude · appli mobile")
            .subtitle("Claude attend une réponse · …\\TestAubin\\Eyrie")
            .glyph("✳️")
            .color(0x5b8def)
            .tone(Tone::Warn)
            .badge("attend")
            .open("t/3")
            .act(Act::new("redemarrer:3", "Redémarrer").glyph("🔄")),
    );
    s.row(
        Row::new("cargo test")
            .subtitle("Commande en cours · …\\TestAubin\\Eyrie")
            .glyph("›_")
            .color(0x5b8def)
            .detail("cargo")
            .open("t/4")
            .act(Act::new("veille:4", "Mettre en veille").glyph("🌙")),
    );
    s.row(Row::new("PowerShell").subtitle("Shell · …\\TestAubin\\Eyrie").glyph("›_").color(0x5b8def).open("t/5"));
    s.section("Ozmo", "2");
    s.row(
        Row::new("Claude · revue de code")
            .subtitle("Claude travaille · …\\DEV\\Ozmo")
            .glyph("✳️")
            .color(0x4cb872)
            .tone(Tone::Ok)
            .open("t/6"),
    );
    s.row(Row::new("Git Bash").subtitle("En veille · …\\DEV\\Ozmo").glyph("›_").color(0x4cb872).open("t/7"));
    s
}

fn patronus_home() -> Screen {
    let mut s = Screen::new("Patronus").subtitle("1 tâche en cours").act(Act::new("tout", "Tout arrêter").danger());
    s.meter("Budget du mois", 0.42, "8,40 € / 20 €", Tone::Ok);
    s.section("Tâches", "3");
    s.row(
        Row::new("Revue des PR du matin")
            .subtitle("Chaque jour à 8 h · prochaine demain 8:00")
            .glyph("🦌")
            .tone(Tone::Ok)
            .detail("réussie")
            .open("tache/revue")
            .act(Act::new("lancer:revue", "Lancer maintenant").glyph("▶️")),
    );
    s.row(
        Row::new("Mise à jour des dépendances")
            .subtitle("En cours depuis 4 min")
            .glyph("🔄")
            .tone(Tone::Info)
            .badge("en cours")
            .open("tache/deps")
            .act(Act::new("arreter:deps", "Arrêter").glyph("⏹️").danger()),
    );
    s.row(Row::new("Nettoyage des branches").subtitle("Le lundi à 9 h").glyph("🧹").open("tache/menage"));
    s.section("Boîte de réception", "2 non lus");
    s.row(Row::new("Revue des PR du matin").subtitle("Réussie · 0,42 €").detail("8:03").glyph("●").open("run/41"));
    s.row(
        Row::new("Tests de nuit")
            .subtitle("Échec : 2 tests rouges")
            .detail("hier")
            .glyph("●")
            .tone(Tone::Danger)
            .open("run/40"),
    );
    s
}

fn run(col: u16, text: &str, fg: Color, flags: u8) -> Run {
    Run { col, text: text.into(), fg, bg: Color::Default, flags }
}

fn terminal() -> TermView {
    let dim = Color::Indexed(8);
    let mut lines = vec![
        Line {
            runs: vec![run(0, "╭─────────────────────────────────────────────────╮", dim, 0)]
        },
        Line { runs: vec![run(0, "│ ✻ Welcome to Claude Code!                       │", Color::Indexed(13), 0)] },
        Line {
            runs: vec![run(0, "╰─────────────────────────────────────────────────╯", dim, 0)]
        },
        Line::default(),
        Line { runs: vec![run(0, "> ajoute le lecteur de QR code à l'application", Color::Default, term::BOLD)] },
        Line::default(),
        Line {
            runs: vec![
                run(0, "● ", Color::Indexed(2), 0),
                run(2, "Je lis crates/eyrie-mobile/src/platform.rs.", Color::Default, 0),
            ],
        },
        Line::default(),
        Line {
            runs: vec![
                run(0, "● ", Color::Indexed(2), 0),
                run(2, "Update(crates/eyrie-mobile/src/android.rs)", Color::Default, term::BOLD),
            ],
        },
        Line { runs: vec![run(2, "⎿  Ajout de 142 lignes", dim, 0)] },
        Line { runs: vec![run(4, "+ pub fn start_scan(on_frame: …, on_code: …) {", Color::Indexed(2), 0)] },
        Line { runs: vec![run(4, "+     let camera = Camera::open_back()?;", Color::Indexed(2), 0)] },
        Line { runs: vec![run(4, "-     // pas encore de caméra", Color::Indexed(1), 0)] },
        Line::default(),
        Line {
            runs: vec![run(0, "╭─────────────────────────────────────────────────╮", Color::Indexed(4), 0)]
        },
        Line {
            runs: vec![run(0, "│ Do you want to make this edit to android.rs?    │", Color::Default, term::BOLD)]
        },
        Line { runs: vec![run(0, "│ ❯ 1. Yes                                        │", Color::Indexed(4), 0)] },
        Line { runs: vec![run(0, "│   2. Yes, allow all edits during this session   │", Color::Default, 0)] },
        Line { runs: vec![run(0, "│   3. No, and tell Claude what to do differently │", Color::Default, 0)] },
        Line {
            runs: vec![run(0, "╰─────────────────────────────────────────────────╯", Color::Indexed(4), 0)]
        },
    ];
    lines.insert(0, Line::plain("PS C:\\Users\\aubin\\DEV\\TestAubin\\Eyrie> claude"));
    let replies = term::numbered_choices(&lines);
    TermView {
        cols: 52,
        rows: 20,
        cursor: None,
        title: "Claude · appli mobile".into(),
        waiting: true,
        replies,
        lines,
        ..Default::default()
    }
}

pub fn play(ui: &AppWindow, scene: &str) {
    let feed = |m: ToPhone| {
        with(|c| c.on_message(m));
    };
    with(|c| c.demo_online("PORTABLE-AUBIN"));
    feed(ToPhone::Modules { modules: modules() });
    feed(ToPhone::Overview {
        waiting: vec![
            Waiting { module: "fawkes".into(), text: "Claude attend : appli mobile".into(), path: None },
            Waiting { module: "owlery".into(), text: "3 PR à relire".into(), path: None },
            Waiting { module: "patronus".into(), text: "2 exécutions non lues".into(), path: None },
        ],
    });
    feed(ToPhone::Notes {
        notes: vec![
            Note {
                module: "fawkes".into(),
                title: "Claude attend une réponse".into(),
                body: "appli mobile".into(),
                tone: Tone::Warn,
                at: now() - 60,
                read: false,
            },
            Note {
                module: "patronus".into(),
                title: "Tests de nuit : échec".into(),
                body: "2 tests rouges".into(),
                tone: Tone::Danger,
                at: now() - 3600 * 9,
                read: false,
            },
            Note {
                module: "owlery".into(),
                title: "Nouvelle revue demandée".into(),
                body: "Ozmo #412".into(),
                tone: Tone::Info,
                at: now() - 3600 * 26,
                read: true,
            },
        ],
        unread: 2,
    });
    feed(ToPhone::Commands {
        commands: vec![
            CommandCard { module: "fawkes".into(), id: "terminal".into(), title: "Nouveau terminal".into() },
            CommandCard { module: "dragontree".into(), id: "fetch".into(), title: "Récupérer (fetch)".into() },
            CommandCard { module: "gringotts".into(), id: "relire".into(), title: "Relire les conversations".into() },
        ],
    });
    feed(ToPhone::Screen { module: "fawkes".into(), path: String::new(), screen: fawkes_home() });
    feed(ToPhone::Screen { module: "patronus".into(), path: String::new(), screen: patronus_home() });
    let term_screen = Screen::new("Claude · appli mobile")
        .subtitle("Claude attend une réponse")
        .terminal("3")
        .act(Act::new("redemarrer:3", "Redémarrer"))
        .act(Act::new("montrer:3", "Montrer sur le PC"));
    feed(ToPhone::Screen { module: "fawkes".into(), path: "t/3".into(), screen: term_screen });
    match scene {
        "fawkes" => {
            with(|c| c.nav_select("fawkes"));
        }
        "terminal" => {
            with(|c| {
                c.nav_select("fawkes");
                c.open_path("t/3");
            });
            feed(ToPhone::Term(TermFrame::full("fawkes", "3", &terminal())));
        }
        "patronus" => {
            with(|c| c.nav_select("patronus"));
        }
        "plus" | "notes" => {
            with(|c| c.nav_select(scene));
        }
        // Comme sur Android : le nœud Tailscale de l'application, connecté.
        "reglages" => {
            show_tailnet(ui, Tailnet::Running { name: "eyrie-pixel-8.tail1234.ts.net".into(), ip: "100.64.0.7".into() });
            with(|c| c.nav_select(scene));
        }
        "accueil" => ui.set_page(0),
        // L'accueil sur Android, avant la connexion du téléphone à Tailscale.
        "accueil-tailscale" => {
            show_tailnet(ui, Tailnet::NeedsLogin("https://login.tailscale.com/a/demo".into()));
            ui.set_page(0);
        }
        "code" => {
            ui.set_page(1);
            ui.set_code_host("suite-portable-aubin".into());
            ui.set_code_code("K7QF2-M9XP4".into());
        }
        "appairage" => {
            ui.set_page(2);
            ui.set_pairing_pc("PORTABLE-AUBIN".into());
            ui.set_pairing_sas("482 913".into());
            ui.set_pairing_stage(1);
        }
        "hors-ligne" => {
            with(|c| c.demo_offline());
        }
        // `ecran:<module>:<fichier.json>` : un écran enregistré (par l'essai d'un module), montré dans son module.
        s if s.starts_with("ecran:") => {
            let mut parts = s.splitn(3, ':');
            let (_, module, file) = (parts.next(), parts.next().unwrap_or("fawkes"), parts.next().unwrap_or(""));
            let screen: Option<Screen> = std::fs::read(file).ok().and_then(|b| serde_json::from_slice(&b).ok());
            if let Some(screen) = screen {
                feed(ToPhone::Screen { module: module.into(), path: "apercu".into(), screen });
                with(|c| {
                    c.nav_select(module);
                    c.open_path("apercu");
                });
            }
        }
        _ => {}
    }
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}
