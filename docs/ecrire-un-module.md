# Écrire un module

Deux façons d'entrer dans la suite :

- **un module natif**, écrit pour la suite : il implémente `suite_sdk::Module`. Carnet
  (`crates/modules/carnet`) est le gabarit ;
- **une application eframe existante**, embarquée sans changer son code : un adaptateur la passe à
  `suite_sdk::embed::Embedded` (les six modules d'aujourd'hui).

Dans les deux cas : une crate dans `crates/modules/<id>`, qui expose `factory()`, et une ligne dans
`crates/eyrie/src/registry.rs`. Rien d'autre à retoucher : la barre d'activité, la palette, la vue
d'ensemble, les réglages et les dispositions le prennent en compte.

## Module natif

```rust
use suite_sdk::{BuildCx, Cx, Module, ModuleFactory, ModuleInfo, Origin, Unavailable, ViewInfo};

pub fn factory() -> ModuleFactory {
    ModuleFactory { info: info(), build, fonts: None }
}

pub fn info() -> ModuleInfo {
    ModuleInfo {
        id: "exemple",                 // stable : clés de disposition, de réglages, de raccourcis
        name: "Exemple",
        icon: "★",                     // couvert par Segoe UI Symbol ou les emoji d'egui
        color: egui::Color32::from_rgb(0x5b, 0x8d, 0xef),
        summary: "Une ligne : ce que fait le module",
        origin: Origin::Native,
    }
}

fn build(bcx: &BuildCx) -> Result<Box<dyn Module>, Unavailable> {
    Ok(Box::new(Exemple { info: info(), host: bcx.host.clone() }))
}

impl Module for Exemple {
    fn info(&self) -> &ModuleInfo { &self.info }
    fn views(&self) -> Vec<ViewInfo> { vec![ViewInfo::main("Exemple")] }
    fn logic(&mut self, _ctx: &egui::Context, _cx: &mut Cx) { /* chaque image, même masqué */ }
    fn ui(&mut self, ui: &mut egui::Ui, _view: &str, _cx: &mut Cx) { /* dans la case */ }
}
```

Le reste de l'interface a des valeurs par défaut :

| Méthode | Pour |
|---|---|
| `views` | plusieurs vues, chacune posable dans sa case (Carnet : Notes, Bus) |
| `status` | titre, pastille (`Badge`), ce qui attend (vue d'ensemble) |
| `commands`, `run_command` | entrées de la palette, raccourcis (registre et conflits) |
| `settings_ui` | sa page dans les réglages d'Eyrie (sous « Modules ») ; une fenêtre sortie ne montre que ses modules |
| `on_event` | événements du bus (`contexte`, `pr_ouverte`, `service_arrete`…) |
| `on_request` | demandes des autres modules (le contrat local, en appel Rust) |
| `needs_hidden_ui` | travail resté dans `ui`, à appeler même masqué (à éviter : mettre ce travail dans `logic`) |
| `can_close` | travail en cours, modifications non enregistrées |
| `on_exit` | dernière sauvegarde |

Règles de bonne conduite (ce que le bac à sable contient chez les modules embarqués) :

- ne pas toucher au style global : lire `cx.host.palette()` et colorer ses propres widgets ;
- ne pas appeler `set_fonts` : déclarer ses polices dans `ModuleFactory::fonts` (`FontPlan`) ;
- ne pas lire le clavier sans regarder `cx.active` (le bac à sable le garantit de toute façon) ;
- pas de `ViewportCommand` : `status().title` pour le titre, `host.show(id)` pour « montre-moi »,
  `host.attention(id)` pour la barre des tâches, `host.notify(…)` pour les notifications ;
- rien de bloquant dans `logic` ou `ui` : un fil, et `host.repaint()` quand le résultat arrive ;
- son état dans `host.module_dir(id)`, avec `suite_sdk::state` (écriture atomique, version,
  champs inconnus gardés par `#[serde(flatten)] extra`).

## Application eframe existante

L'adaptateur refait ce que fait le `main.rs` de l'application, sans `run_native` :

```rust
fn build(bcx: &BuildCx) -> Result<Box<dyn Module>, Unavailable> {
    // 1. Le même verrou d'instance que l'exe : jamais deux fois sur le même dossier d'état.
    let guard = NamedMutex::try_acquire(&instance_name(&dir)).ok_or(Unavailable::AlreadyRunning { exe: "mon.exe" })?;
    // 2. Son état, comme le fait son main.rs.
    let loaded = mon_app::state::load(&dir);
    // 3. Son eframe::App.
    let app = mon_app::App::new(bcx.cc, loaded);
    Ok(Box::new(
        Embedded::new(info(), bcx.host.clone(), app)
            .title_badge(|t| first_number(t).map(|n| Badge::count(n, Tone::Info, format!("{n} en attente"))))
            .key_commands(vec![KeyCommand::new("actualiser", "Actualiser", "F5")])
            .hidden_ui(false)
            .keep(guard),
    ))
}
```

- `title_badge` : la pastille lue dans le titre que l'application donne à sa fenêtre ;
- `key_commands` : ses raccourcis connus, proposés dans la palette (les exécuter, c'est lui envoyer
  la frappe) ;
- `hidden_ui(true)` si son travail de fond est dans `ui` ;
- `before_ui` / `after_ui` : ajuster temporairement ce qu'elle expose publiquement (Fawkes : son thème
  et la géométrie de sa fenêtre) ;
- `status`, `on_request`, `can_close` : ce qu'elle expose publiquement, branché sur l'interface ;
- `keep` : ce qui doit vivre autant que le module.

Ses réglages vont dans ceux d'Eyrie, sans doublon :

```rust
let settings = bcx.host.clone();
app.set_hosted(true); // thème, langue, démarrage de Windows : ceux d'Eyrie, masqués chez lui
app.set_settings_link(Box::new(move || settings.open_settings(&format!("module:{ID}"))));
Embedded::new(info(), bcx.host.clone(), app).settings_panel(|app, ui| app.settings_ui(ui))
```

- `settings_ui(&mut self, ui)` : le contenu de sa fenêtre de réglages, dessiné dans la page de l'hôte
  (sans la fenêtre, sans ce que masque `set_hosted`) ;
- `set_settings_link` : son « Réglages… » (menu, raccourci) ouvre les réglages d'Eyrie sur sa page,
  dans la fenêtre où il est, au lieu de sa propre fenêtre ;
- les comptes ne sont pas dans sa page : `set_accounts_link` mène à Réglages › Comptes (`docs/comptes.md`).

Si l'application n'a pas de bibliothèque (Fawkes), une crate peut compiler ses sources depuis le
dépôt voisin : voir `crates/modules/fawkes/build.rs`.

Ce que la suite fait pour elle, sans rien lui demander : voir `premier-jet.md` §4.
