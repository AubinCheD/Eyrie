# Comptes communs de la suite EyrieSuite

On se connecte **une fois**, dans Eyrie, DragonTree ou Owlery : le compte vaut pour toute la suite, que
l'application tourne seule ou dans Eyrie, y compris pour une application installée plus tard. Le code commun est
la crate `crates/suite-accounts` ; DragonTree et Owlery en dépendent par chemin (`../SuperApp/crates/suite-accounts`).

## Où

| Quoi | Où |
|---|---|
| La liste, sans jeton | `%APPDATA%\EyrieSuite\accounts.json` |
| Les jetons | Gestionnaire d'identifiants de Windows, service **EyrieSuite**, utilisateur `account:<id>` |

Deux applications qui écrivent en même temps passent l'une après l'autre : chaque écriture tient le verrou de Windows
sur `accounts.lock` (5 s d'attente au plus), que Windows rend si l'application meurt pendant l'écriture.

Le dossier se choisit ainsi :

1. dans Eyrie, celui qu'Eyrie donne à ses modules (`Host::accounts_dir`, `StartOptions::accounts_dir` de DragonTree,
   `StartOptions::accounts` d'Owlery) : pas de variable d'environnement, que les terminaux et tâches lancés depuis
   Eyrie hériteraient ;
2. sinon `EYRIESUITE_DIR` s'il est posé à la main (`suite_accounts::accounts_dir`) ;
3. sinon le profil isolé de l'application s'il y en a un (`EYRIE_DATA_DIR`, `--profile` de DragonTree,
   `OWLERY_DATA_DIR`) : essais et tests gardent des comptes à part ;
4. sinon `%APPDATA%\EyrieSuite`.

## Le fichier

```json
{
  "version": 1,
  "accounts": [
    { "id": "e7d3af9721b2eb45", "forge": "azure", "host": "dev.azure.com",
      "username": "camille@contoso.test", "display_name": "Camille Martin", "orgs": ["contoso"] }
  ]
}
```

- `id` : 16 caractères hexadécimaux tirés à l'ajout, puis fixes (clé du jeton, dépôts associés dans DragonTree).
- `forge` : `github`, `gitlab`, `azure` ; une forge inconnue est gardée et ignorée par qui ne la connaît pas.
- `host` : **normalisé** (sans schéma, chemin, utilisateur ni majuscules ; vide : celui de la forge).
  `<org>.visualstudio.com` devient `dev.azure.com`, et `<org>` passe dans `orgs` s'il n'y en a pas.
- Les champs inconnus (au premier niveau et dans un compte) sont réécrits tels quels ; un compte illisible aussi.
- Chaque écriture relit le fichier juste avant et le remplace d'un coup (fichier temporaire écrit sur le disque,
  puis renommage). Un fichier illisible n'est jamais écrasé, ni pris pour « aucun compte » : chaque application garde
  la dernière liste lue et le dit (`Store::try_list`).

## Règles

- **Une connexion, un compte** : même forge, même hôte normalisé, même identifiant (casse ignorée). Ajouter une
  connexion déjà connue met à jour ses informations et son jeton, sous l'identifiant existant.
- **Relecture** : chaque application compare la date du fichier au plus toutes les 2 s et relit les comptes quand
  une autre les a changés ; aucun redémarrage n'est nécessaire.
- **Remplacement dit** : ajouter une connexion déjà connue remplace son jeton pour toute la suite, et chaque
  application le dit (« jeton remplacé pour toute la suite »). Les droits recommandés sont ceux de DragonTree, qui
  clone et pousse avec le jeton commun.
- **Reprise** (au premier lancement de la nouvelle version) : chaque application range ses anciens comptes dans ceux
  de la suite, avec le jeton de son ancien coffre, sous leur identifiant ; une connexion déjà connue garde le sien, et
  les références suivent (dépôts associés dans DragonTree). DragonTree impose son jeton s'il diffère (il lui faut les
  droits d'écriture). Les anciens comptes ne quittent les réglages d'une application que si tous ont été repris :
  sinon, la reprise recommence au lancement suivant. Pour Eyrie, l'ancien dossier `%APPDATA%\SuperApp` est renommé,
  ou copié s'il ne peut pas l'être.
- **Retrait** : un compte retiré disparaît de toute la suite, avec ses jetons, anciens coffres compris (« SuperApp »,
  « DragonTree », « Owlery »). Les anciens jetons des comptes repris restent sinon dans ces coffres.

## Seule ou intégrée

| | Seule | Dans Eyrie |
|---|---|---|
| Comptes | les mêmes (fichier et coffre communs) | les mêmes |
| Où les régler | sa propre fenêtre Comptes | **Réglages d'Eyrie › Comptes** : « Comptes… » y mène (`Host::open_settings("comptes")`) |
| Thème, langue | les siens | ceux d'Eyrie, masqués dans ses réglages et ses menus (`set_hosted(true)`) |
| Ses autres réglages | sa fenêtre Réglages | sa page dans les **Réglages d'Eyrie** : « Réglages… » y mène (`Host::open_settings("module:<id>")`) ; une fenêtre sortie montre Eyrie et ses seuls modules |

Les comptes `gh` d'Owlery (GitHub par la CLI, sans jeton) restent dans Owlery.
