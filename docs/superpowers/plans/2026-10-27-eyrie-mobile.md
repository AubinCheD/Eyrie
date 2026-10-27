# Eyrie mobile — plan d'implémentation

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** piloter Eyrie depuis un téléphone Android (application Rust/Slint), synchro temps réel par Tailscale,
appairage par QR code.

**Architecture:** protocole partagé `suite-mobile` (Noise + MessagePack, écrans génériques, terminal en différences) ;
pont dans Eyrie (`crates/eyrie/src/mobile/`) qui seul appelle les modules par `Module::mobile_*` ; compagnon Go
d'Eyrie qui relaie le port 4747 du tailnet vers 127.0.0.1 ; application `crates/eyrie-mobile` (Slint, bureau et
Android).

**Tech Stack:** Rust 2024, snow 0.10, rmp-serde 1.3, blake2, slint 1.18, qrcode 0.14, rqrr 0.11, Go 1.27 + tsnet,
Android NDK 28 / SDK 36.

**Spec:** `docs/mobile.md`

## Global Constraints

- Ne pas modifier les dépôts voisins (Fawkes, DragonTree…) : tout passe par leur API `pub` depuis les adaptateurs.
- Textes de l'interface en français, `tr("…")` dans Eyrie avec leur traduction dans `crates/eyrie/locales/en.json`.
- Port tailnet du pont : 4747. Protocole : version 1, prologue `eyrie-mobile/1`.
- Ticket d'appairage : 32 octets, 5 min, usage unique, 5 essais. SAS : 6 chiffres.
- Données mobiles du PC : `<dossier local d'Eyrie>\mobile\` (`cle` DPAPI, `appareils.json`, `journal.jsonl`).
- Formater crate par crate (`cargo fmt -p …`), jamais `--all`. Tests un dépôt à la fois, `-j 4`.

## Review Focus

- Téléphone qui se reconnecte pendant qu'une ancienne session vit encore : l'ancienne est fermée, pas de doublon.
- Action envoyée pour un écran périmé (le PC a changé l'écran entre-temps) : refusée proprement (`Resultat` en erreur), sans panique.
- Terminal qui défile vite (sortie de compilation) : le téléphone reste aligné (décalage + lignes changées), pas de dérive.
- Module arrêté ou planté pendant qu'un téléphone le regarde : écran « module indisponible », pas de blocage du pont.
- Interrupteur coupé pendant un appairage en attente : la fenêtre de confirmation disparaît, le ticket est annulé.

---

### Task 1: `suite-mobile` — protocole, écrans, terminal, appairage, canal

**Files:** Create `crates/suite-mobile/{Cargo.toml,src/lib.rs,src/ui.rs,src/term.rs,src/msg.rs,src/pairing.rs,src/channel.rs,src/client.rs}` ; Modify `Cargo.toml` (membre, dépendances).

**Interfaces (Produces):**
- `ui::{Screen, Block, Row, Act, ActStyle, Tone, Input, Field}` ; `Screen::acts() -> Vec<(String /*chemin*/, String /*act*/)>` non : `Screen::action_ids() -> BTreeSet<String>`.
- `term::{TermView, Line, Run, Color, Key, TermInput, TermFrame, QuickReply}` ; `TermFrame::diff(old: &TermView, new: &TermView) -> TermFrame` ; `TermFrame::apply(&self, lines: &mut Vec<Line>)`.
- `msg::{ToPc, ToPhone, ModuleCard, Waiting, Note, PROTOCOL}`.
- `pairing::{Ticket, ManualCode, proof(secret, hash, role) -> [u8;32], sas(hash) -> String}` ; `Ticket::to_uri()/from_uri()`.
- `channel::{Keypair, Channel, Handshake}` : `Channel::accept(stream, &Keypair) -> Result<(Channel, Peer)>`, `Channel::connect(stream, &Keypair) -> Result<(Channel, Peer)>`, `Channel::split() -> (Sender, Receiver)`, `Sender::send(&T)`, `Receiver::recv::<T>()` ; `Peer { static_key: [u8;32], handshake_hash: Vec<u8> }`.
- `client::Client` : connexion côté téléphone (bonjour, appairage), utilisée par l'application et les essais d'Eyrie.

- [ ] Tests : URI du ticket aller-retour ; code manuel aller-retour et refus d'un caractère ambigu ; poignée de main entre deux fils sur TCP local, clés vues des deux côtés ; message de 200 Ko découpé puis reçu ; trame altérée refusée ; preuves différentes selon le rôle ; SAS identique des deux côtés ; `TermFrame` : défilement de 3 lignes + 1 ligne changée appliqué = vue cible.
- [ ] Implémenter, `cargo test -p suite-mobile`, commit.

### Task 2: `Module::mobile_*` et `Embedded`

**Files:** Modify `crates/suite-sdk/{Cargo.toml,src/lib.rs,src/module.rs,src/embed.rs}`.

**Produces:** dans `Module` :
`fn mobile_screen(&mut self, path: &str, cx: &mut Cx) -> Option<ui::Screen>` (défaut `None`),
`fn mobile_action(&mut self, path: &str, action: &str, input: Option<&str>, cx: &mut Cx) -> Result<Option<String>, String>`,
`fn mobile_term(&mut self, id: &str, history: usize) -> Option<term::TermView>`,
`fn mobile_term_input(&mut self, id: &str, input: &term::TermInput, cx: &mut Cx) -> Result<(), String>`.
`Embedded` : `.mobile_screen(f)`, `.mobile_action(f)`, `.mobile_term(f)`, `.mobile_term_input(f)`.

- [ ] Test : un `Embedded` avec ses fermetures répond, un sans renvoie `None`/erreur. Commit.

### Task 3: pont mobile dans Eyrie — magasin, serveur, appairage

**Files:** Create `crates/eyrie/src/mobile/{mod.rs,store.rs,server.rs}` ; Modify `app.rs` (champ `mobile` dans `Core`, appel par image), `persist.rs` (`MobileSettings { enabled }`), `i18n.rs` (test récursif).

**Produces:** `mobile::Bridge::new(dir) / poll(&mut Core…) / set_enabled(bool) / relay() -> Option<(SocketAddr, String)>` ; `store::{Devices, Device, Rights, Journal, pc_keypair(dir)}` ; `server::{Server, Inbound}`.

- [ ] Tests (fil réel, TCP local, mode essai `EYRIE_MOBILE_LOCAL`) : appairage complet avec confirmation simulée ; ticket expiré refusé ; secret faux refusé ; appareil inconnu refusé ; appareil révoqué déconnecté ; compte Tailscale différent refusé ; jeton du compagnon faux refusé. Commit.

### Task 4: synchro et aiguillage

**Files:** Create `crates/eyrie/src/mobile/sync.rs`.

- [ ] Tests (modules factices) : cartes envoyées une fois puis seulement si elles changent ; écran suivi renvoyé quand il change ; action non proposée refusée ; action proposée exécutée par le module puis bulle et journal ; saisie de terminal refusée sans le droit *terminaux* ; terminal : décalage et lignes changées. Commit.

### Task 5: page Téléphones, confirmation, barre d'état

**Files:** Create `crates/eyrie/src/mobile/page.rs` ; Modify `settings.rs` (`Page::Phones`, clé `telephones`), `shell.rs` (indicateur 📱, fenêtre de confirmation), `palette.rs` (commande « Couper l'accès mobile »), `locales/en.json`.

- [ ] Essai egui_kittest : la page montre le QR après « Appairer un téléphone » ; la confirmation apparaît quand un téléphone demande. Commit.

### Task 6: compagnon d'Eyrie (relais)

**Files:** Create `compagnon/{go.mod,go.sum,main.go,etat.go,etat_test.go,relais.go,relais_test.go}`, `tools/compagnon.ps1` ; Modify `crates/eyrie/src/tailnet.rs` (pilote propre, configuration par l'entrée standard).

- [ ] `go test` du relais (en-tête, jeton, refus d'une adresse non locale) ; essai Rust du pilote (lignes d'état, configuration écrite). Commit.

### Task 7: écrans mobiles des modules

**Files:** Modify `crates/modules/{fawkes,patronus,owlery,menagerie,dragontree,grimoire,gringotts,carnet}/src/*` (+ `Cargo.toml` : `alacritty_terminal = "0.26.0"` pour Fawkes, `dragontree-git` pour DragonTree).

- [ ] Par module : test de l'écran d'accueil (sections attendues) et d'une action. Fawkes : `TermView` d'un terminal factice. Commit par module.

### Task 8: application mobile (Slint), bureau

**Files:** Create `crates/eyrie-mobile/{Cargo.toml,build.rs,src/*.rs,ui/*.slint}`.

- [ ] Tests : tampon de terminal (différences), aplatissement d'un écran en lignes, magasin (identité, PC appairés). Capture `EYRIE_MOBILE_CAPTURE` de chaque écran sur des données de démonstration ; essai de bout en bout contre un Eyrie en mode local. Commit.

### Task 9: Android

**Files:** Create `crates/eyrie-mobile/android/{AndroidManifest.xml,res/…}`, `tools/android.ps1`, `crates/eyrie-mobile/src/platform/android.rs` (caméra NDK + rqrr, permission, liens, vibration).

- [ ] `tools\android.ps1` produit un APK signé (clé de débogage) ; lancement dans l'émulateur si l'accélération est disponible, sinon sur le téléphone de l'utilisateur. Commit.
