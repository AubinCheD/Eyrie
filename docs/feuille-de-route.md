# Feuille de route

Le raisonnement est dans `reflexions.md`, le constat technique dans `etat-des-lieux.md`.

## En cours

| Étape | État |
|---|---|
| Dépôt créé (`git init`, branche `main`) | fait (22/10) |
| État des lieux des six modules (audits en lecture seule) | fait (22/10) : `etat-des-lieux.md`, `audit/` |
| Conception de l'intégration (questions ouvertes de `reflexions.md` §11) | en cours |

## Ensuite (proposé, ordre à valider)

1. **Conception écrite** : `docs/superpowers/specs/<date>-integration-design.md`, relue, puis le plan
   d'implémentation.
2. **Essai jetable** de l'hôte avec de faux modules, pour vérifier tôt ce qui est risqué :
   - `egui_tiles` : cases, onglets, grilles, glisser-déposer ;
   - fenêtres détachées en viewports immédiats, rattachement, réveils ciblés ;
   - filtrage du clavier par module actif (§7) ;
   - `catch_unwind` autour d'un module qui panique.
3. **Crate commune** : interface de module et services de l'hôte, palette sémantique et polices,
   écriture atomique et état versionné (champs inconnus gardés), instance unique et tube (contrat
   unifié), registre des raccourcis, notifications.
4. **Intégration module par module**. Ordre proposé, du plus simple au plus lourd :
   1. Grimoire (lib déjà pensée comme module) ;
   2. Owlery (logique séparée, injection prévue) ;
   3. Menagerie (sortir la supervision de `ui()`) ;
   4. Patronus (identité de processus, Planificateur) ;
   5. Fawkes (pas encore de `lib.rs`, procédure de fenêtre, polices) ;
   6. DragonTree (le plus gros ; réglages à séparer).
5. **Réglages communs** : thème, puis comptes (crate de forges commune, import), puis langue (catalogues
   par module), puis Tailscale (nœud de la suite).
6. **Actions croisées** entre modules, bus d'événements, contexte partagé.

## Chantiers à part

- Traduction de Fawkes, Patronus, Owlery, Menagerie, Grimoire (3 à 5 j chacun).
- Contrat par tube côté Fawkes (rien n'existe encore, alors que tous les modules le visent).
- Harmonisation des contrats existants (noms de tube SID ou nom d'utilisateur, `demande` / `action` /
  `op`, `erreur` / `error`).
- Champs inconnus gardés à l'écriture dans DragonTree, Menagerie et Grimoire.
