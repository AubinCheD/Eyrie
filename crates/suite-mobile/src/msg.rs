//! Les messages, dans les deux sens. MessagePack à champs nommés : un champ ajouté par une version plus récente est
//! ignoré par une plus ancienne ; un écart de [`PROTOCOL`] est refusé dès le bonjour.

use serde::{Deserialize, Serialize};

use crate::term::{TermFrame, TermInput};
use crate::ui::{Screen, Tone};

/// Version du protocole.
pub const PROTOCOL: u32 = 1;

/// Un module qui veut que le téléphone ouvre un écran après une action répond `Ok(Some("@ouvrir <chemin>"))`.
pub const OPEN_PREFIX: &str = "@ouvrir ";

/// … ou un autre module, sur son accueil : `Ok(Some("@module fawkes"))`, ou sur un de ses écrans :
/// `Ok(Some("@module gringotts conversation/12"))` (l'identifiant d'un module n'a pas d'espace). La même forme dans
/// `Act.open`, `Row.open` ou `Tab.path` fait ouvrir au téléphone l'écran d'un autre module, sans rien envoyer au PC.
pub const MODULE_PREFIX: &str = "@module ";

/// Le module et le chemin désignés par « @module <id> [<chemin>] », ou `None` pour un chemin du module lui-même.
pub fn module_target(open: &str) -> Option<(&str, &str)> {
    let rest = open.strip_prefix(MODULE_PREFIX)?;
    let (module, path) = rest.split_once(' ').unwrap_or((rest, ""));
    (!module.is_empty()).then_some((module, path))
}

/// Ce qu'un appareil a le droit de faire.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rights {
    /// Voir les modules (toujours vrai pour un appareil appairé).
    pub see: bool,
    /// Agir dans les modules (actions des écrans, commandes).
    pub act: bool,
    /// Saisir dans les terminaux et en créer.
    pub terminals: bool,
    /// Parcourir les dossiers du PC pour une action qui en demande un (en plus d'`act`). Absent d'un appareil
    /// appairé avant ce droit : accordé, comme les autres.
    #[serde(default = "yes")]
    pub files: bool,
}

fn yes() -> bool {
    true
}

impl Default for Rights {
    fn default() -> Self {
        Rights { see: true, act: true, terminals: true, files: true }
    }
}

/// Téléphone → PC.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ToPc {
    /// Premier message d'un appareil appairé.
    Hello {
        protocol: u32,
        app: String,
        device: String,
        model: String,
        /// Ce que l'application sait faire en plus du protocole de base (pour qu'un PC plus récent s'y adapte).
        #[serde(default)]
        features: Vec<String>,
    },
    /// Premier message d'un appareil qui s'appaire : la preuve qu'il connaît le secret du ticket.
    Pair {
        protocol: u32,
        proof: Vec<u8>,
        device: String,
        model: String,
    },
    /// Ce que le téléphone montre : l'écran `(module, chemin)` et, s'il y a lieu, le terminal `(module, id)`.
    Follow {
        screen: Option<(String, String)>,
        terminal: Option<(String, String)>,
    },
    Action {
        req: u32,
        module: String,
        path: String,
        action: String,
        input: Option<String>,
    },
    Terminal {
        module: String,
        id: String,
        input: TermInput,
    },
    /// Une commande de la palette du PC.
    Command {
        req: u32,
        module: String,
        id: String,
    },
    NotesRead,
    Ping,
    /// Lister un dossier du PC pour l'action `action` de l'écran suivi `(module, path)`, qui demande un dossier
    /// ([`crate::ui::Input::folder`]). `dir: None` : la racine (raccourcis, lecteurs). Envoyé seulement pour une
    /// action qui porte `folder` : un PC qui ne sait pas lister ne le propose pas.
    Browse {
        req: u32,
        module: String,
        path: String,
        action: String,
        dir: Option<String>,
        #[serde(default)]
        filter: String,
        #[serde(default)]
        offset: u32,
        #[serde(default)]
        hidden: bool,
    },
}

/// Une carte de module : ce que montre la barre de navigation.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ModuleCard {
    pub id: String,
    pub name: String,
    pub glyph: String,
    /// 0xRRGGBB.
    pub color: u32,
    /// Le module tourne sur le PC.
    pub running: bool,
    /// Pourquoi il ne tourne pas.
    pub reason: String,
    pub badge: Option<Badge>,
    /// Texte court permanent (barre d'état du PC).
    pub status: String,
    pub summary: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Badge {
    pub count: Option<u32>,
    pub tone: Tone,
    pub tip: String,
}

/// Ce qui attend l'utilisateur, pour la vue d'ensemble.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Waiting {
    pub module: String,
    pub text: String,
    /// Écran du module à ouvrir.
    pub path: Option<String>,
}

/// Une notification du centre de notifications du PC.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Note {
    pub module: String,
    pub title: String,
    pub body: String,
    pub tone: Tone,
    /// Secondes depuis l'époque Unix.
    pub at: u64,
    pub read: bool,
    /// L'écran du module où elle mène (`t/12` : la session Claude qui attend).
    #[serde(default)]
    pub path: Option<String>,
}

/// Une commande de la palette du PC.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CommandCard {
    pub module: String,
    pub id: String,
    pub title: String,
}

/// PC → téléphone.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ToPhone {
    Welcome {
        protocol: u32,
        pc: String,
        eyrie: String,
        rights: Rights,
        /// Le veilleur du PC est actif : de quoi lui demander de démarrer Eyrie quand elle est fermée (voir
        /// [`crate::veille`]). Absent : le PC ne se démarre pas à distance.
        #[serde(default)]
        wake: Option<WakeKey>,
    },
    /// La preuve du PC a été vérifiée par lui ; il attend le clic de l'utilisateur. Le téléphone vérifie la preuve du
    /// PC et affiche `sas`, que le PC affiche aussi.
    PairPending {
        proof: Vec<u8>,
        sas: String,
    },
    /// Appairé : le téléphone garde la clé du PC (vue dans la poignée de main).
    Paired {
        pc: String,
    },
    /// Connexion refusée ; elle sera fermée.
    Refused {
        reason: String,
    },
    Modules {
        modules: Vec<ModuleCard>,
    },
    /// Logos des modules (PNG), une fois par connexion.
    Logos {
        logos: Vec<(String, Vec<u8>)>,
    },
    Overview {
        waiting: Vec<Waiting>,
    },
    Notes {
        notes: Vec<Note>,
        unread: u32,
    },
    Commands {
        commands: Vec<CommandCard>,
    },
    Screen {
        module: String,
        path: String,
        screen: Screen,
    },
    /// Le module ne peut pas montrer cet écran (arrêté, chemin inconnu).
    NoScreen {
        module: String,
        path: String,
        reason: String,
    },
    Term(TermFrame),
    Result {
        req: u32,
        ok: bool,
        message: String,
    },
    /// Ouvrir cet écran (le terminal qu'une action vient de créer…).
    Open {
        module: String,
        path: String,
    },
    Toast {
        text: String,
        tone: Tone,
    },
    /// Le PC coupe l'accès mobile ou révoque l'appareil.
    Bye {
        reason: String,
        /// Eyrie se ferme (le PC reste allumé) : le téléphone dit « Eyrie est fermée » plutôt que « hors ligne ».
        #[serde(default)]
        closing: bool,
    },
    Pong,
    /// Réponse à [`ToPc::Browse`].
    Folder {
        req: u32,
        listing: Listing,
    },
}

/// De quoi demander au veilleur du PC de démarrer Eyrie : son port sur le tailnet, le secret de cet appareil et
/// l'identifiant que le PC lui a donné à l'appairage (le champ `appareil` de la demande).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WakeKey {
    pub port: u16,
    pub secret: Vec<u8>,
    #[serde(default)]
    pub device: String,
}

/// Drapeaux d'un [`FolderEntry`].
pub const ENTRY_REPO: u8 = 1;
/// Déjà connu du module (un dépôt déjà dans DragonTree).
pub const ENTRY_KNOWN: u8 = 2;
pub const ENTRY_HIDDEN: u8 = 4;
/// Lien ou jonction.
pub const ENTRY_LINK: u8 = 8;
/// Accès refusé.
pub const ENTRY_DENIED: u8 = 16;
/// Fichiers en ligne seulement (OneDrive).
pub const ENTRY_CLOUD: u8 = 32;
/// Un lecteur.
pub const ENTRY_DRIVE: u8 = 64;

/// Un dossier dans une liste du sélecteur.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FolderEntry {
    pub name: String,
    /// Chemin complet, à renvoyer tel quel dans `Browse` ou dans la saisie de l'action.
    pub path: String,
    #[serde(default)]
    pub flags: u8,
    /// Texte court (« dépôt git », « déjà dans DragonTree »).
    #[serde(default)]
    pub detail: String,
}

/// Un groupe de raccourcis à la racine du sélecteur (« Récents », « DragonTree », « Accès rapide », « Lecteurs »).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlaceGroup {
    pub title: String,
    pub places: Vec<FolderEntry>,
}

/// Le contenu d'un dossier du PC, tel que le sélecteur le montre : des dossiers seulement, jamais de fichier.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Listing {
    /// Dossier listé ; vide : la racine.
    pub dir: String,
    pub title: String,
    /// Fil d'Ariane : (libellé, chemin), de la racine au dossier.
    #[serde(default)]
    pub crumbs: Vec<(String, String)>,
    /// À la racine : les raccourcis.
    #[serde(default)]
    pub groups: Vec<PlaceGroup>,
    #[serde(default)]
    pub entries: Vec<FolderEntry>,
    /// Pagination : position de la première entrée, nombre total, et s'il en reste.
    #[serde(default)]
    pub offset: u32,
    #[serde(default)]
    pub total: u32,
    #[serde(default)]
    pub more: bool,
    /// Le dossier est trop grand pour être lu en entier : filtrer.
    #[serde(default)]
    pub truncated: bool,
    /// Fichiers et dossiers cachés non montrés (comptés seulement).
    #[serde(default)]
    pub files: u32,
    #[serde(default)]
    pub hidden: u32,
    /// Libellé du bouton de choix (« Choisir ce dossier », « Cloner ici ») ; `None` : ce dossier ne peut pas être
    /// choisi, `why_not` dit pourquoi.
    #[serde(default)]
    pub choose: Option<String>,
    #[serde(default)]
    pub why_not: String,
    /// Ce que donnera le choix (« C:\DEV\Ozmo : sera créé »).
    #[serde(default)]
    pub target: String,
    #[serde(default)]
    pub target_tone: Tone,
    /// Le dossier n'a pas pu être lu (ou la demande est refusée).
    #[serde(default)]
    pub error: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_module_target_names_the_module_and_its_screen() {
        assert_eq!(module_target("@module fawkes"), Some(("fawkes", "")));
        assert_eq!(module_target("@module gringotts conversation/a b"), Some(("gringotts", "conversation/a b")));
        assert_eq!(module_target("@module "), None);
        assert_eq!(module_target("depot/1"), None);
    }

    #[test]
    fn messages_survive_messagepack() {
        let m = ToPc::Action {
            req: 7,
            module: "patronus".into(),
            path: "".into(),
            action: "lancer:build".into(),
            input: Some("main".into()),
        };
        let bytes = rmp_serde::to_vec_named(&m).unwrap();
        assert_eq!(rmp_serde::from_slice::<ToPc>(&bytes).unwrap(), m);
        let mut screen = Screen::new("Fawkes");
        screen.section("Ozmo", "2");
        let back = ToPhone::Screen { module: "fawkes".into(), path: "".into(), screen };
        let bytes = rmp_serde::to_vec_named(&back).unwrap();
        assert_eq!(rmp_serde::from_slice::<ToPhone>(&bytes).unwrap(), back);
    }

    /// Un appareil appairé avant le droit « dossiers » (appareils.json, `Welcome` d'un PC plus ancien) le reçoit,
    /// comme les autres droits.
    #[test]
    fn rights_written_before_the_folder_right_grant_it() {
        #[derive(Serialize)]
        struct OldRights {
            see: bool,
            act: bool,
            terminals: bool,
        }
        let bytes = rmp_serde::to_vec_named(&OldRights { see: true, act: false, terminals: true }).unwrap();
        let r: Rights = rmp_serde::from_slice(&bytes).unwrap();
        assert_eq!(r, Rights { see: true, act: false, terminals: true, files: true });
    }

    #[test]
    fn a_folder_listing_and_the_new_fields_survive_messagepack() {
        let ask = ToPc::Browse {
            req: 3,
            module: "dragontree".into(),
            path: "".into(),
            action: "ajouter".into(),
            dir: Some(r"C:\DEV".into()),
            filter: "oz".into(),
            offset: 200,
            hidden: false,
        };
        assert_eq!(rmp_serde::from_slice::<ToPc>(&rmp_serde::to_vec_named(&ask).unwrap()).unwrap(), ask);
        let listing = Listing {
            dir: r"C:\DEV".into(),
            title: "DEV".into(),
            crumbs: vec![("Ce PC".into(), String::new()), ("C:".into(), "C:\\".into())],
            entries: vec![FolderEntry {
                name: "Ozmo".into(),
                path: r"C:\DEV\Ozmo".into(),
                flags: ENTRY_REPO,
                ..Default::default()
            }],
            choose: Some("Ajouter ce dépôt".into()),
            ..Default::default()
        };
        let back = ToPhone::Folder { req: 3, listing };
        assert_eq!(rmp_serde::from_slice::<ToPhone>(&rmp_serde::to_vec_named(&back).unwrap()).unwrap(), back);
        let bye = ToPhone::Bye { reason: "Eyrie se ferme.".into(), closing: true };
        assert_eq!(rmp_serde::from_slice::<ToPhone>(&rmp_serde::to_vec_named(&bye).unwrap()).unwrap(), bye);
    }
}
