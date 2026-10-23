# Eyrie : réglages des modules dans Eyrie, renommage terminé, comptes — plan d'exécution

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:executing-plans (choisi : exécution dans la session). Steps use checkbox (`- [ ]`) syntax.

**Goal :** dans Eyrie, un seul endroit pour tout régler (Eyrie et chaque module), limité à Eyrie + ses modules dans une fenêtre détachée ; « SuperApp » et « la suite » (l'application) disparaissent de l'interface ; les comptes gagnent les finitions laissées par la revue.

**Architecture :** chaque module expose `settings_ui(&mut self, ui)` (ses réglages dessinés dans un `Ui` fourni) et `set_settings_link(open)` (son propre « Réglages… » renvoie à l'hôte) ; l'adaptateur `Embedded` relie `settings_ui` à `Module::settings_ui` ; Eyrie dessine ces panneaux dans sa fenêtre de réglages, dans le bac à sable, et ouvre cette fenêtre dans la fenêtre (principale ou détachée) d'où vient la demande.

**Tech :** Rust 2024, egui/eframe 0.37, egui_kittest ; toolchain GNU (`--config ../SuperApp/.cargo/config.toml` hors Eyrie).

## Exigences (message du 23/10/2026, après les commits dac4859 / 71f1b34 / 7e196ac)

1. Dans Eyrie (fenêtre principale) → Réglages : accès aux réglages de **tous** les modules.
2. Module dans une fenêtre à lui : exe seul → ses seuls réglages ; dans Eyrie mais dans une fenêtre détachée → les réglages d'Eyrie et ceux de ce(s) module(s) seulement.
3. Dans Eyrie, le thème (et la langue) d'un module n'apparaît nulle part (DragonTree › Affichage › Thème compris) ; seulement dans l'exe seul.
4. Depuis un module intégré, son « Réglages… » ouvre les réglages d'Eyrie sur sa page (décision du 23/10).
5. Passe complète sur les comptes, le renommage SuperApp → Eyrie et le paramétrage.

## Global Constraints

- Branches : Eyrie `suite/etape-2` ; les six modules `suite/module` (accords du 23/10 : DragonTree, Owlery, Fawkes, Patronus, Menagerie, Grimoire). Commits locaux, un par dépôt à la fin de la passe (demande d'historique).
- Même API dans les six modules : `pub fn settings_ui(&mut self, ui: &mut egui::Ui)`, `pub fn set_settings_link(&mut self, open: Box<dyn Fn()>)`, `pub fn set_hosted(&mut self, hosted: bool)` (qui masque thème et langue).
- Textes d'Eyrie par `tr()` + `locales/en.json` (test `catalog_matches_sources`) ; DragonTree par `tr!` + son catalogue (test `every_text_has_a_translation…`).
- Les tests ne touchent ni au vrai coffre ni à `%APPDATA%`.

## Review Focus

- Un module qui panique ou change le style global pendant son panneau de réglages : Eyrie reste debout, le style revient.
- Fenêtre détachée avec deux modules : ses réglages montrent les deux, pas les autres.
- Réglages ouverts depuis un module détaché : la fenêtre de réglages s'ouvre dans cette fenêtre-là, sur la page du module.
- Brouillon de réglages (DragonTree, Menagerie) : rien n'est enregistré sans « Enregistrer », comme seul.
- Deux applications qui écrivent `accounts.json` au même instant : aucun compte perdu.

---

### Task 1 : comptes — finitions (`suite-accounts`, Eyrie)
- `Store` : verrou entre processus (`accounts.lock` créé en exclusif, attente courte, verrou de plus de 10 s considéré abandonné) autour de chaque lecture-modification-écriture ; `update` garde les champs inconnus (`extra`).
- Reprise SuperApp : les jetons copiés sont effacés de l'ancien coffre « SuperApp » (le renommage est fini).
- Page Comptes d'Eyrie : le jeton saisi n'est effacé qu'après succès ; « Retirer » demande confirmation (« Retirer de toute la suite ? ») ; un nouvel ajout efface l'ancien statut du compte.
- Tests : `concurrent_writers_lose_nothing`, `update_keeps_unknown_fields`, `superapp_tokens_leave_the_old_vault`, `the_token_stays_until_the_account_is_added`, `removing_asks_for_confirmation`.

### Task 2 : renommage terminé (Eyrie)
- Interface : « la suite » qui désigne l'application devient « Eyrie » (menu « Suite » → « Eyrie », « Réglages d'Eyrie », « Fermer Eyrie ? », « Relancer Eyrie », journal, vues…) ; « toute la suite » (comptes, Tailscale) reste.
- Test : `interface_texts_name_eyrie` (aucune clé `tr()` d'Eyrie égale à « Suite » ou contenant « la suite » hors « toute la suite »).

### Task 3 : SDK et Eyrie — réglages des modules dans Eyrie, fenêtre de réglages par fenêtre
- SDK : `Embedded::settings_panel(f)` → `Module::settings_ui` renvoie vrai ; `Host::open_settings("module:<id>")`.
- Eyrie : page d'un module = son panneau (appel dans `sandbox::call`, panique → module arrêté et journalisé) ; `SettingsState::window: Option<u64>` ; la fenêtre de réglages se dessine dans ce viewport ; la navigation n'y montre que les modules de cette fenêtre ; bouton « ⚙ Réglages » dans la barre des fenêtres détachées ; `open_settings("module:<id>")` ouvre dans la fenêtre qui contient le module.
- Tests : `embedded_settings_panel_is_drawn`, `module_page_draws_the_module_settings`, `a_panicking_settings_panel_stops_only_its_module`, `settings_of_a_detached_window_list_only_its_modules`, `a_module_opens_its_settings_page_where_it_is`.

### Tasks 4 à 9 : chaque module (DragonTree, Owlery, Fawkes, Patronus, Menagerie, Grimoire)
Pour chacun : extraire le contenu de sa fenêtre de réglages dans `settings_ui(ui)` (la fenêtre le réutilise) ; `set_settings_link` (son « Réglages… » appelle le lien) ; `set_hosted(true)` masque thème et langue (fenêtre, panneau, menus) ; l'adaptateur d'Eyrie branche `settings_panel` et le lien (`open_settings("module:<id>")`).
- Owlery : son panneau intégré montre aussi l'état de lecture de chaque compte (lu à…, erreur), en lecture seule.
- Tests (par module) : `settings_panel_draws_in_a_host_ui`, `hosted_settings_command_opens_the_host`, `hosted_settings_hide_the_theme`.

### Task 10 : vérification, documentation, commits
- Suites complètes des sept dépôts, clippy ; captures des réglages (principale, fenêtre détachée) ; `eyrie.exe` (release) recompilé ; docs (`docs/comptes.md`, `README`, `premier-jet.md`) ; un commit par dépôt.
