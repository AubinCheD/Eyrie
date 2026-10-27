//! Les messages, dans les deux sens. MessagePack à champs nommés : un champ ajouté par une version plus récente est
//! ignoré par une plus ancienne ; un écart de [`PROTOCOL`] est refusé dès le bonjour.

use serde::{Deserialize, Serialize};

use crate::term::{TermFrame, TermInput};
use crate::ui::{Screen, Tone};

/// Version du protocole.
pub const PROTOCOL: u32 = 1;

/// Un module qui veut que le téléphone ouvre un écran après une action répond `Ok(Some("@ouvrir <chemin>"))`.
pub const OPEN_PREFIX: &str = "@ouvrir ";

/// Ce qu'un appareil a le droit de faire.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rights {
    /// Voir les modules (toujours vrai pour un appareil appairé).
    pub see: bool,
    /// Agir dans les modules (actions des écrans, commandes).
    pub act: bool,
    /// Saisir dans les terminaux et en créer.
    pub terminals: bool,
}

impl Default for Rights {
    fn default() -> Self {
        Rights { see: true, act: true, terminals: true }
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
    },
    Pong,
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
