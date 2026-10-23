//! Dispositions : un arbre de cases `egui_tiles` par fenêtre (onglets, lignes, colonnes, grilles,
//! glisser-déposer, séparateurs), des dispositions en un clic qui produisent un arbre, et des
//! dispositions enregistrées sous un nom.
//!
//! L'unité posée dans une case est une **vue** de module (choix du premier jet) : un module embarqué
//! n'en a qu'une, un module natif peut en proposer plusieurs. Une vue n'apparaît qu'une fois.

use egui_tiles::{Container, Tile, TileId, Tiles, Tree};
use serde::{Deserialize, Serialize};

use crate::i18n::mark;

/// Les vues propres à la suite.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SuiteView {
    /// Ce qui attend l'utilisateur dans tous les modules.
    Overview,
    Notifications,
    /// Temps passé par module (un seul processus pour tous).
    Resources,
    /// Journal de la suite : modules construits, indisponibles, arrêtés, débordements contenus.
    Journal,
}

impl SuiteView {
    pub const ALL: [SuiteView; 4] = [SuiteView::Overview, SuiteView::Notifications, SuiteView::Resources, SuiteView::Journal];

    pub fn title(self) -> &'static str {
        match self {
            SuiteView::Overview => mark("Vue d'ensemble"),
            SuiteView::Notifications => mark("Notifications"),
            SuiteView::Resources => mark("Ressources"),
            SuiteView::Journal => mark("Journal"),
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            SuiteView::Overview => "⌂",
            SuiteView::Notifications => "🔔",
            SuiteView::Resources => "📊",
            SuiteView::Journal => "📝",
        }
    }
}

/// Ce qu'on pose dans une case.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Pane {
    Module { module: String, view: String },
    Suite { view: SuiteView },
}

impl Pane {
    pub fn module(module: &str, view: &str) -> Pane {
        Pane::Module { module: module.to_string(), view: view.to_string() }
    }

    pub fn module_id(&self) -> Option<&str> {
        match self {
            Pane::Module { module, .. } => Some(module),
            Pane::Suite { .. } => None,
        }
    }
}

/// Dispositions en un clic, comme celles de Fawkes (1, 2, 3 colonnes, 2×2, 1 grand + 2…).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Preset {
    /// Tout en onglets dans une seule case.
    Tabs,
    /// Côte à côte.
    Columns,
    /// Les unes sous les autres.
    Rows,
    /// Grille automatique.
    Grid,
    /// Le premier à gauche, les autres empilés en onglets à droite.
    MainAndStack,
}

impl Preset {
    pub const ALL: [Preset; 5] = [Preset::Tabs, Preset::Columns, Preset::Rows, Preset::Grid, Preset::MainAndStack];

    pub fn label(self) -> &'static str {
        match self {
            Preset::Tabs => mark("Tout en onglets"),
            Preset::Columns => mark("Côte à côte"),
            Preset::Rows => mark("Empilés"),
            Preset::Grid => mark("Grille"),
            Preset::MainAndStack => mark("Un grand + pile"),
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            Preset::Tabs => "▭",
            Preset::Columns => "▥",
            Preset::Rows => "▤",
            Preset::Grid => "▦",
            Preset::MainAndStack => "◧",
        }
    }
}

pub fn tree_id(window: u64) -> egui::Id {
    egui::Id::unique(("suite-cases", window))
}

pub fn preset(window: u64, kind: Preset, panes: Vec<Pane>) -> Tree<Pane> {
    let id = tree_id(window);
    match kind {
        Preset::Tabs => Tree::new_tabs(id, panes),
        Preset::Columns => Tree::new_horizontal(id, panes),
        Preset::Rows => Tree::new_vertical(id, panes),
        Preset::Grid => Tree::new_grid(id, panes),
        Preset::MainAndStack => {
            let mut panes = panes.into_iter();
            let Some(first) = panes.next() else { return Tree::empty(id) };
            let rest: Vec<Pane> = panes.collect();
            if rest.is_empty() {
                return Tree::new_tabs(id, vec![first]);
            }
            let mut tiles = Tiles::default();
            let main = tiles.insert_pane(first);
            let main = tiles.insert_tab_tile(vec![main]);
            let stack: Vec<TileId> = rest.into_iter().map(|p| tiles.insert_pane(p)).collect();
            let stack = tiles.insert_tab_tile(stack);
            let root = tiles.insert_horizontal_tile(vec![main, stack]);
            if let Some(Tile::Container(Container::Linear(l))) = tiles.get_mut(root) {
                l.shares.set_share(main, 2.0);
                l.shares.set_share(stack, 1.0);
            }
            Tree::new(id, root, tiles)
        }
    }
}

/// Les cases d'un arbre, dans l'ordre de l'affichage (de la racine, enfants dans l'ordre).
pub fn panes(tree: &Tree<Pane>) -> Vec<(TileId, Pane)> {
    let mut out = Vec::new();
    let mut stack: Vec<TileId> = tree.root.into_iter().collect();
    let mut seen = std::collections::HashSet::new();
    while let Some(id) = stack.pop() {
        if !seen.insert(id) {
            continue;
        }
        match tree.tiles.get(id) {
            Some(Tile::Pane(p)) => out.push((id, p.clone())),
            Some(Tile::Container(c)) => stack.extend(c.children_vec().into_iter().rev()),
            None => {}
        }
    }
    // Une case détachée de l'arbre (rare : avant la simplification) reste comptée.
    for (id, t) in tree.tiles.iter() {
        if let Tile::Pane(p) = t
            && !seen.contains(id)
        {
            out.push((*id, p.clone()));
        }
    }
    out
}

pub fn find(tree: &Tree<Pane>, pane: &Pane) -> Option<TileId> {
    tree.tiles.find_pane(pane)
}

/// Le premier conteneur d'onglets en partant de la racine (en largeur).
fn first_tabs(tree: &Tree<Pane>) -> Option<TileId> {
    let mut queue: std::collections::VecDeque<TileId> = tree.root.into_iter().collect();
    while let Some(id) = queue.pop_front() {
        if let Some(Tile::Container(c)) = tree.tiles.get(id) {
            if matches!(c, Container::Tabs(_)) {
                return Some(id);
            }
            queue.extend(c.children().copied());
        }
    }
    None
}

/// Ajoute une case : en onglet dans `near` (le conteneur d'onglets de cette case) s'il est donné,
/// sinon dans le premier conteneur d'onglets, sinon à côté de tout le reste. Elle devient active.
pub fn add_pane(tree: &mut Tree<Pane>, pane: Pane, near: Option<TileId>) -> TileId {
    let id = tree.tiles.insert_pane(pane);
    let Some(root) = tree.root else {
        let tabs = tree.tiles.insert_tab_tile(vec![id]);
        tree.root = Some(tabs);
        return id;
    };
    let target = near
        .and_then(|n| {
            if matches!(tree.tiles.get(n), Some(Tile::Container(Container::Tabs(_)))) {
                Some(n)
            } else {
                tree.tiles.parent_of(n).filter(|p| matches!(tree.tiles.get(*p), Some(Tile::Container(Container::Tabs(_)))))
            }
        })
        .or_else(|| first_tabs(tree));
    match target.and_then(|t| tree.tiles.get_mut(t)) {
        Some(Tile::Container(Container::Tabs(tabs))) => {
            tabs.add_child(id);
            tabs.set_active(id);
        }
        _ => {
            let new_root = tree.tiles.insert_horizontal_tile(vec![root, id]);
            tree.root = Some(new_root);
        }
    }
    activate(tree, id);
    id
}

pub fn remove_pane(tree: &mut Tree<Pane>, tile: TileId) -> Option<Pane> {
    let removed = tree.remove_recursively(tile);
    removed.into_iter().find_map(|t| if let Tile::Pane(p) = t { Some(p) } else { None })
}

/// Rend la case visible (onglet actif dans tous ses conteneurs).
pub fn activate(tree: &mut Tree<Pane>, tile: TileId) {
    tree.make_active(|id, _| id == tile);
}

/// Case suivante (ou précédente) parmi les cases visibles, pour Ctrl+F6.
pub fn cycle(visible: &[TileId], current: Option<TileId>, forward: bool) -> Option<TileId> {
    if visible.is_empty() {
        return None;
    }
    let i = current.and_then(|c| visible.iter().position(|v| *v == c));
    let n = visible.len();
    Some(match (i, forward) {
        (None, _) => visible[0],
        (Some(i), true) => visible[(i + 1) % n],
        (Some(i), false) => visible[(i + n - 1) % n],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(m: &str) -> Pane {
        Pane::module(m, "principal")
    }

    #[test]
    fn presets_hold_every_pane() {
        for kind in Preset::ALL {
            let t = preset(0, kind, vec![p("fawkes"), p("dragontree"), p("owlery")]);
            assert_eq!(panes(&t).len(), 3, "{kind:?}");
        }
        assert!(preset(0, Preset::MainAndStack, vec![]).is_empty());
    }

    #[test]
    fn add_goes_into_tabs_and_becomes_active() {
        let mut t = preset(0, Preset::Columns, vec![p("fawkes"), p("dragontree")]);
        let id = add_pane(&mut t, p("owlery"), None);
        assert!(find(&t, &p("owlery")).is_some());
        assert!(t.active_tiles().contains(&id) || t.tiles.get(id).is_some());
        let removed = remove_pane(&mut t, id);
        assert_eq!(removed, Some(p("owlery")));
        assert!(find(&t, &p("owlery")).is_none());
    }

    #[test]
    fn add_to_empty_tree() {
        let mut t = Tree::empty(tree_id(1));
        add_pane(&mut t, p("grimoire"), None);
        assert_eq!(panes(&t).len(), 1);
    }

    #[test]
    fn cycle_wraps() {
        let ids: Vec<TileId> = (1..=3).map(TileId::from_u64).collect();
        assert_eq!(cycle(&ids, Some(ids[2]), true), Some(ids[0]));
        assert_eq!(cycle(&ids, Some(ids[0]), false), Some(ids[2]));
        assert_eq!(cycle(&ids, None, true), Some(ids[0]));
        assert_eq!(cycle(&[], None, true), None);
    }

    #[test]
    fn layout_round_trips_through_json() {
        let t = preset(0, Preset::MainAndStack, vec![p("fawkes"), Pane::Suite { view: SuiteView::Overview }]);
        let json = serde_json::to_string(&t).unwrap();
        let back: Tree<Pane> = serde_json::from_str(&json).unwrap();
        assert_eq!(panes(&back), panes(&t));
    }
}
