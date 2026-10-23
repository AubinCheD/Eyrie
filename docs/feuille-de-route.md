# Feuille de route

Le raisonnement est dans `reflexions.md`, le constat technique dans `etat-des-lieux.md`.

## En cours

| Étape | État |
|---|---|
| Dépôt créé (`git init`, branche `main`) | fait (22/10) |
| État des lieux des six modules (audits en lecture seule) | fait (22/10) : `etat-des-lieux.md`, `audit/` |
| Conception de l'intégration (questions ouvertes de `reflexions.md` §11) | choix provisoires (23/10) : `premier-jet.md` §5 |
| Premier jet de l'hôte, sans toucher au code des modules | fait (23/10) : `premier-jet.md`, `README.md` |

## Fait dans le premier jet (23/10)

- Hôte : cases `egui_tiles`, dispositions en un clic et nommées, une par configuration d'écrans,
  fenêtres détachées (viewports immédiats), mode concentration, barre d'activité et pastilles, vue
  d'ensemble, notifications, palette (F1), journal, ressources, réglages communs.
- Bac à sable autour de chaque appel de module (style, polices, clavier, focus, commandes de
  fenêtre, fermeture, visibilité, plantage) : `premier-jet.md` §4.
- Crate commune `suite-sdk` : interface de module et services de l'hôte, adaptateur eframe, thèmes
  de Fawkes en palette sémantique (contrastes testés), plan de polices, langue par catalogue de
  domaine, raccourcis et conflits, état versionné, instance unique.
- Les six modules embarqués sans changer leur code (Fawkes compilé depuis ses sources), plus Carnet,
  module natif et gabarit (`ecrire-un-module.md`).
- Instance : même verrou et même tube que chaque exe ; mode essai (dossiers d'état d'essai).
- Comptes de la suite (coffre de Windows, partage compte par compte, import DragonTree/Owlery) ;
  Owlery les lit déjà.
- Tailscale : un nœud pour la suite, compagnon de Fawkes.
- Contrat local en appel Rust par le bus (Owlery → Fawkes `ouvrir`).
- Tests : SDK, hôte de bout en bout (egui_kittest), adaptateurs.

## Ensuite (proposé, ordre à valider)

1. **Relecture du premier jet** et des choix provisoires (`premier-jet.md` §5).
2. **Retouches dans les modules**, module par module et avec accord, pour lever les limites qui
   restent (`premier-jet.md` §6), du plus rentable au plus coûteux :
   1. un vrai `impl Module` dans chaque bibliothèque (plus besoin de l'API cachée d'eframe) ;
   2. modales et fenêtres bornées à la case ;
   3. palette de la suite (Patronus, Menagerie, Grimoire), langue de la suite (DragonTree) ;
   4. raccourcis déclarés au registre, plus de clavier global ;
   5. comptes et contexte partagé lus et publiés (DragonTree, Patronus, Fawkes, Owlery) ;
   6. notifications par la suite (une seule icône), crochet d'arrêt de Windows (Fawkes).
3. **Crate commune** étoffée à mesure que les modules en dépendent : forges et comptes
   (`dragontree-forges` + `Owlery/src/forges`), écriture atomique, instance unique et tube unifié.
4. **Actions croisées** : contexte partagé suivi par tous, Patronus sur les événements du bus.

## Chantiers à part

- Traduction de Fawkes, Patronus, Owlery, Menagerie, Grimoire (3 à 5 j chacun).
- Contrat par tube côté Fawkes (rien n'existe encore, alors que tous les modules le visent).
- Harmonisation des contrats existants (noms de tube SID ou nom d'utilisateur, `demande` / `action` /
  `op`, `erreur` / `error`).
- Champs inconnus gardés à l'écriture dans DragonTree, Menagerie et Grimoire.
