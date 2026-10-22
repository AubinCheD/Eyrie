# État des lieux des six modules (22/10/2026)

Synthèse des audits détaillés de `docs/audit/` (un fichier par dépôt, avec les références au code) et
de la réflexion déjà menée dans Fawkes (`../Fawkes/docs/reflexions.md` §3 « Fawkes, DragonTree et une
suite modulaire », §3.5 « Configurations », §5 « Fawkes sur mobile »).

## 1. Les modules

| Module | Rôle | Taille | Forme | Particularités pour la suite |
|---|---|---|---|---|
| Fawkes | terminaux multi-groupes, restauration des sessions Claude | ~15 600 l. | **binaire seul** (pas de `lib.rs`) | barre de titre dessinée, procédure de fenêtre Win32, polices et thème globaux, Tailscale |
| DragonTree | client git | ~65 600 l. | workspace : `-i18n`, `-git`, `-forges`, `-app` (lib + bin) | **seul module traduit** (fr/en), **comptes de forges**, onglets multi-dépôts |
| Patronus | agents et tâches récurrentes | ~12 300 l. | workspace : `-core` (sans UI) + `-app` (lib + bin) | moteur qui doit tourner vue masquée, Planificateur Windows |
| Owlery | boîte de réception (PR, CI, mentions, tickets) | ~9 400 l. | lib + bin | comptes de forges (copie proche de DragonTree), traits d'injection, contrat par tube implémenté |
| Menagerie | services locaux, ports, Docker, mails de test | ~10 600 l. | lib + bin | supervision dans `ui()`, relais `--relais` qui survit à l'appli |
| Grimoire | configuration de Claude Code, conversations, consommation | ~15 400 l. | lib + 2 bins | lib pensée comme module, exe `grimoire-statusline` lancé par Claude Code |

## 2. Ce qui rend la suite faisable

- **Une seule pile** : Rust édition 2024, eframe/egui **0.37** en rendu glow partout. Un seul processus,
  une seule version d'egui.
- **Le dessin principal se fait déjà dans un `egui::Ui`** : en egui 0.37, `Panel::show(ui)` est rattaché au
  `Ui` parent. Les six modules dessinent leurs panneaux dans le `Ui` reçu : on peut les poser dans une
  case de docking sans réécrire leur interface.
- **Les constructeurs n'utilisent que `cc.egui_ctx`** (sauf Patronus, qui prend aussi le HWND pour la zone
  de notification) : passer un `&egui::Context` suffit.
- **Séparation logique / affichage** déjà faite dans Fawkes, DragonTree et Owlery (`logic()` appelé même
  fenêtre réduite, puis `ui()`).
- **Injection** déjà prévue dans Owlery (`SecretStore`, `Notifier`, `FawkesLink`, `Fetcher`) et dans
  Grimoire (« l'exe est une enveloppe fine », `lib.rs:1-4`).
- **Docking disponible** : `egui_tiles` 0.18 dépend d'egui 0.37 (onglets, lignes, colonnes, grilles,
  glisser-déposer, séparateurs). `egui_dock` 0.21 est encore sur egui 0.36.
- **États robustes** dans Fawkes, Patronus et Owlery : écriture atomique, versionnage, copie avant
  changement de version, champs inconnus gardés.

## 3. Ce qui bloque, commun à tous

Les mêmes problèmes reviennent dans chaque audit : chaque application se croit seule dans sa fenêtre.

| # | Problème | Qui | Conséquence dans la suite |
|---|---|---|---|
| 1 | **Style global** : `set_fonts` (remplace toutes les polices), `set_visuals`, `global_style_mut`, `set_zoom_factor`, `override_text_color` | tous ; polices : Fawkes, DragonTree | le dernier module dessiné impose son thème, ses polices et son zoom aux autres ; Fawkes panique si ses familles `term*` disparaissent |
| 2 | **Raccourcis lus ou consommés sur toute la fenêtre**, sans test de focus | tous ; Owlery a des touches seules (R, U, J, K…) | un raccourci d'Owlery mange les frappes d'un terminal Fawkes ; Ctrl+, Ctrl+Maj+P, Ctrl+N, F5, Ctrl+1…9, Ctrl+Tab en conflit |
| 3 | **Commandes de fenêtre** : `Title`, `Close`, `CancelClose`, `Focus`, `Minimized`, `RequestUserAttention`, géométrie enregistrée | tous | un module renomme, ferme ou bloque la fenêtre de la suite |
| 4 | **Dialogues au niveau `Context`** : `Modal`, `Window`, `Area` (toasts) | tous (DragonTree : 27 endroits, Fawkes : 8 modales) | une modale d'un module grise toute la suite ; toasts superposés |
| 5 | **Identifiants egui génériques** (`"settings"`, `"palette"`, `"toast"`, titres de fenêtre « Réglages »…) | tous | collisions d'état entre modules |
| 6 | **Instance unique et dossier d'état** : chaque exe tient un mutex (et parfois un tube) sur son dossier | tous | sans précaution, l'exe autonome et la suite écriraient le même état |
| 7 | **Zone de notification** : icône `uID=1` sur le HWND principal | Fawkes, Patronus, Owlery | collision ; trois icônes pour une seule appli |
| 8 | **Procédure de fenêtre Win32** cherchée par le titre exact « Fawkes » (`winhook`) | Fawkes | arrêt de Windows (sauvegarde finale) et clic sur la notification perdus |
| 9 | **Travail de fond dans `ui()`** | Menagerie (supervision, relances, sauvegarde), Patronus (Planificateur), Grimoire (demandes) | module masqué = plus de relance de services, plus de sauvegarde |
| 10 | **Réveils** : `request_repaint()` vise le viewport racine | tous | une fenêtre détachée ne se redessine pas quand ses données changent |
| 11 | **Applications ni `Send` ni `Sync`** | tous | fenêtres détachées en viewport **immédiat** (pas différé) |
| 12 | **Langue** : français en dur partout sauf DragonTree | 5 modules sur 6 | la langue commune ne s'applique d'abord qu'à DragonTree ; 3 à 5 j de traduction par module |
| 13 | **Champs inconnus perdus à l'écriture** | DragonTree, Menagerie, Grimoire | dangereux dès que deux programmes écrivent le même fichier |

## 4. Les réglages à homogénéiser : ce qui existe

**Thème**

| Module | Thèmes | Palette | Polices |
|---|---|---|---|
| Fawkes | Graphite, Fluent, Nuit colorée (contrastes WCAG testés) | ~40 couleurs dont terminal et 16 ANSI | Segoe UI, Cascadia Mono ou Consolas |
| DragonTree | Système, Clair, Sombre + échelle 0,8–1,6 | 28 couleurs métier + 10 de branches ; suit `dark_mode` d'egui | Segoe UI (+ Semibold), Cascadia, Phosphor (icônes), police du diff |
| Patronus | Système, Sombre, Clair + zoom | 11 couleurs | egui par défaut |
| Owlery | Système, Sombre, Clair | visuels d'egui | egui + emoji monochromes |
| Menagerie | Sombre, Clair (ne suit pas le système) | 7 couleurs | egui par défaut |
| Grimoire | Sombre, Clair | gabarit de Fawkes, couleurs propres | egui par défaut |

Six palettes différentes (accent `#9fc5ff` chez Patronus, `#5b8def` chez Fawkes…).

**Langue** : `dragontree-i18n` (le texte français sert de clé, catalogue `en.json` de 1 397 entrées,
`tr!`/`trn!`, langue globale par `AtomicU8`, changement à chaud, test des catalogues). Réutilisable à
condition de passer à des **catalogues par module**.

**Comptes**

| Module | Comptes | Stockage |
|---|---|---|
| DragonTree | GitHub (+ Enterprise), GitLab (+ auto-hébergé), Azure DevOps (+ Server) ; PAT | keyring, service `DragonTree`, clé `account:<id>` ; liste dans `settings.json` |
| Owlery | les mêmes forges, modèle presque identique ; PAT ou CLI `gh` | keyring, service `Owlery`, clé `account:<id>` ; liste dans `state.json` |
| Grimoire | « profils » = comptes Claude (`CLAUDE_CONFIG_DIR`) | lus dans `.claude.json` |
| Patronus | aucun (CLI `claude` et git du poste) ; la spec prévoit les forges « partagées avec DragonTree » | — |
| Fawkes, Menagerie | aucun | — |

Le contrat d'Owlery (§5) propose déjà des **comptes partagés** : service keyring « Suite », clé
`account:<id>`, liste sans jeton dans `%APPDATA%\<Suite>\accounts.json`, partage compte par compte.

**Tailscale** : Fawkes seul. Compagnon Go (tsnet) `fawkes-tailnet.exe` lancé par Fawkes, dialogue en
lignes JSON sur stdin/stdout, état du nœud dans `%APPDATA%\Fawkes\tailnet` (en clair, DPAPI prévu),
page d'essai sur le port 80. tailscale-rs prévu en remplacement quand il sera stable.

## 5. Contrats entre modules : déjà divergents

Chaque module a écrit son contrat (`docs/contrat-*.md`) de son côté :

| | Owlery | Grimoire | Patronus | Menagerie (vers Fawkes) |
|---|---|---|---|---|
| Tube | `owlery-<SID>` | `grimoire-<USERNAME>` | `<app>-<SID>` | `fawkes-<SID>` |
| Verbe | `demande` | `action` | `demande` | `op` |
| Erreur | `erreur` | `error` | `erreur` | `error` |
| Implémenté | oui (serveur) | oui (serveur) | non | non |

Fawkes, la cible de presque toutes les demandes (`ouvrir` un terminal ou une session Claude), **n'a
encore aucun tube**. Dans la suite, ces échanges deviennent des appels Rust dans le même processus ; le
tube ne sert plus qu'aux exe autonomes.

## 6. Code dupliqué (candidat à une crate commune)

- écriture atomique : 5 versions ;
- instance unique (mutex, tube, vérification du SID) : 5 versions ;
- capture d'écran de test (`capture.rs`) : 4 copies ;
- analyse et affichage des raccourcis : au moins 3 modèles ;
- zone de notification : 3 versions ;
- thème et palette, toasts, `on_some_monitor`, `open_url`, `attach_parent_console`, `build.rs` (icône) ;
- forges et comptes : `dragontree-forges` et `Owlery/src/forges` (2 420 lignes) ;
- outillage : `setup.ps1`, `.cargo/config.toml`, `rustfmt.toml`, `tests/common`.

## 7. Efforts estimés par les audits

Ordre de grandeur, intégration seule (sans traduction ni contrat par tube) :

| Module | Effort | Le plus lourd |
|---|---|---|
| Fawkes | 6 à 9 j | thème et polices, crochets Windows, réglages |
| DragonTree | 10 à 17 j | séparer réglages communs et propres, extraire l'API de module |
| Patronus | 4,5 à 7 j | identité de processus (Planificateur, `current_exe`), extraction du module |
| Menagerie | 4 à 6 j | supervision hors de `ui()`, API hôte |
| Owlery | 6,75 à 9,75 j | comptes communs |
| Grimoire | 3,75 à 5,25 j | contrat, thème |
| Crate commune | 2 à 3 j | |

Traduction de Fawkes, Patronus, Owlery, Menagerie et Grimoire : 3 à 5 j chacun.
