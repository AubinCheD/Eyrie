//! Le pont mobile : le seul point d'entrée du téléphone dans Eyrie (voir `docs/mobile.md`).
//!
//! - [`server`] écoute dans ses fils (port local relayé par le compagnon Tailscale), fait la poignée de main et
//!   l'appairage, et remet tout ce qui arrive dans une boîte de réception ;
//! - [`sync`], appelé à chaque image depuis l'hôte, vide cette boîte, vérifie chaque demande (droits de l'appareil,
//!   action proposée dans le dernier écran envoyé), appelle les modules dans le bac à sable et pousse aux téléphones ce
//!   qui a changé ;
//! - [`store`] garde la clé du PC, les appareils et le journal ;
//! - [`browse`] liste les dossiers du PC pour le sélecteur du téléphone, dans un fil, et contrôle le chemin choisi ;
//! - [`page`] : la page Téléphones des réglages et la confirmation d'un appairage.

pub mod browse;
pub mod page;
pub mod server;
pub mod store;
pub mod sync;
#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui;
use suite_mobile::PROTOCOL;
use suite_mobile::msg::{Rights, ToPhone};
use suite_mobile::pairing::{self, Ticket};

pub use sync::poll;

/// Le ticket d'appairage montré dans la page Téléphones.
pub struct Shown {
    pub uri: String,
    /// Code à saisir (`K7QF2-M9XP4`).
    pub code: String,
    /// Nom du PC sur le tailnet, à saisir avec le code.
    pub host: String,
    pub created: Instant,
}

impl Shown {
    pub fn remaining(&self) -> Duration {
        Duration::from_secs(pairing::TICKET_SECS).saturating_sub(self.created.elapsed())
    }
}

pub struct Bridge {
    /// Le dossier d'Eyrie (ses données : interdites au sélecteur de dossiers).
    dir: PathBuf,
    pub store: store::Store,
    server: Option<server::Server>,
    pub sessions: Vec<sync::Session>,
    /// Appairages qui attendent le clic sur le PC.
    pub requests: Vec<server::PairRequest>,
    pub ticket: Option<Shown>,
    /// Jeton que le compagnon présente à chaque connexion relayée.
    token: String,
    /// `EYRIE_MOBILE_LOCAL=<port>` : écoute directe sur ce port local, sans compagnon (essais, développement).
    local_port: Option<u16>,
    /// Pourquoi l'écoute n'a pas démarré.
    pub error: Option<String>,
    ctx: egui::Context,
    pub(crate) sync: sync::State,
    pub page: page::PageState,
    /// Ce que le sélecteur de dossiers sait du PC, fait au premier usage.
    folders: Option<Arc<browse::Env>>,
    /// La clé du veilleur (« Démarrer Eyrie depuis le téléphone ») : chaque téléphone en tire son secret.
    wake: Option<[u8; 32]>,
}

/// Nom de ce PC (`COMPUTERNAME`).
pub fn pc_name() -> String {
    std::env::var("COMPUTERNAME").ok().filter(|n| !n.is_empty()).unwrap_or_else(|| "PC".into())
}

impl Bridge {
    pub fn new(dir: &Path, ctx: egui::Context) -> Bridge {
        let local_port = std::env::var("EYRIE_MOBILE_LOCAL").ok().and_then(|p| p.trim().parse().ok());
        Bridge {
            dir: dir.to_path_buf(),
            store: store::Store::open(&store::mobile_dir(dir)),
            server: None,
            sessions: Vec::new(),
            requests: Vec::new(),
            ticket: None,
            token: store::hex(&pairing::random::<32>()),
            local_port,
            error: None,
            ctx,
            sync: sync::State::default(),
            page: page::PageState::default(),
            folders: None,
            wake: None,
        }
    }

    /// Ce que le sélecteur de dossiers et le contrôle des chemins savent du PC (zones interdites, lecteurs).
    pub(crate) fn folder_env(&mut self) -> Arc<browse::Env> {
        let (dir, mobile) = (&self.dir, store::mobile_dir(&self.dir));
        self.folders.get_or_insert_with(|| Arc::new(browse::Env::system(&[dir, &mobile]))).clone()
    }

    /// Écoute directe, sans compagnon (`EYRIE_MOBILE_LOCAL`).
    pub fn local(&self) -> bool {
        self.local_port.is_some()
    }

    /// Essais : écoute directe sur un port libre.
    #[cfg(test)]
    pub fn listen_locally(&mut self) {
        self.local_port = Some(0);
    }

    pub fn listening_on(&self) -> Option<std::net::SocketAddr> {
        self.server.as_ref().map(|s| s.addr)
    }

    fn shared(&self) -> Option<&Arc<server::Shared>> {
        self.server.as_ref().map(|s| s.shared())
    }

    /// Ce que le compagnon doit relayer : l'adresse locale et le jeton (rien en écoute directe).
    pub fn relay(&self) -> Option<(String, String)> {
        if self.local() {
            return None;
        }
        self.server.as_ref().map(|s| (s.addr.to_string(), self.token.clone()))
    }

    /// Applique l'interrupteur « Accès depuis le téléphone ».
    pub fn ensure(&mut self, enabled: bool) {
        if enabled && self.server.is_none() && self.error.is_none() {
            self.start();
        } else if !enabled {
            self.error = None;
            if self.server.is_some() {
                self.stop("Accès depuis le téléphone coupé sur le PC.");
            }
        }
    }

    fn start(&mut self) {
        if let Some(p) = self.store.blocking_problem() {
            self.error = Some(p.to_string());
            return;
        }
        let Some(key) = self.store.key().cloned() else {
            self.error = self.store.problem.clone();
            return;
        };
        let ctx = self.ctx.clone();
        let token = (!self.local()).then(|| self.token.clone());
        let shared = Arc::new(server::Shared::new(key, pc_name(), token, Box::new(move || ctx.request_repaint())));
        shared.set_devices(self.device_auths());
        shared.set_standby_key(self.wake);
        match server::Server::start(shared, self.local_port.unwrap_or(0)) {
            Ok(s) => {
                self.store.journal("", "acces", format!("accès ouvert ({})", s.addr));
                self.server = Some(s);
            }
            Err(e) => self.error = Some(format!("écoute impossible : {e}")),
        }
    }

    /// Coupe tout : écoute, connexions, ticket, demandes en attente.
    pub fn stop(&mut self, reason: &str) {
        for s in self.sessions.drain(..) {
            s.close(reason);
        }
        for r in self.requests.drain(..) {
            r.answer(false);
        }
        self.ticket = None;
        if self.server.take().is_some() {
            self.store.journal("", "acces", "accès coupé");
        }
    }

    /// Réessaie de démarrer après une erreur (bouton de la page).
    pub fn retry(&mut self) {
        self.error = None;
    }

    pub fn device_auths(&self) -> Vec<server::DeviceAuth> {
        self.store
            .devices
            .iter()
            .filter_map(|d| {
                Some(server::DeviceAuth {
                    id: d.id.clone(),
                    name: d.name.clone(),
                    key: d.key_bytes()?,
                    login: d.login.clone(),
                    rights: d.rights,
                })
            })
            .collect()
    }

    fn devices_changed(&mut self) {
        if let Some(s) = self.shared() {
            s.set_devices(self.device_auths());
        }
    }

    /// La clé du veilleur (`None` : pas de démarrage à distance). Les téléphones déjà connectés reçoivent un nouveau
    /// `Welcome`, avec leur secret ou sans.
    pub fn set_wake(&mut self, key: Option<[u8; 32]>) {
        if self.wake == key {
            return;
        }
        self.wake = key;
        if let Some(s) = self.shared() {
            s.set_standby_key(key);
        }
        let auths = self.device_auths();
        for s in &self.sessions {
            let Some(dev) = auths.iter().find(|d| d.id == s.device) else { continue };
            s.send(ToPhone::Welcome {
                protocol: PROTOCOL,
                pc: pc_name(),
                eyrie: env!("CARGO_PKG_VERSION").into(),
                rights: s.rights,
                wake: key.map(|k| server::wake_key(&k, dev)),
            });
        }
    }

    /// Un nouveau ticket : QR code et code à saisir, valables cinq minutes. `node` : le nœud d'Eyrie sur le tailnet
    /// (nom MagicDNS, adresse IPv4).
    pub fn new_ticket(&mut self, node: Option<(String, String)>) -> Result<(), String> {
        let Some(shared) = self.shared().cloned() else { return Err("accès depuis le téléphone coupé".into()) };
        let local = self.server.as_ref().map(|s| s.addr.port()).filter(|_| self.local());
        let (host, ip, port) = match (local, node) {
            (Some(p), _) => ("localhost".to_string(), "127.0.0.1".to_string(), p),
            (None, Some((name, ip))) => (name, ip, suite_mobile::PORT),
            (None, None) => return Err("Eyrie n'est pas encore joignable sur Tailscale".into()),
        };
        let secret = pairing::new_secret();
        let code = pairing::new_code();
        let code_secret = pairing::code_secret(&code).expect("code tiré dans l'alphabet");
        shared.add_ticket(&[secret.clone(), code_secret], Duration::from_secs(pairing::TICKET_SECS));
        let uri = Ticket { pc: pc_name(), host: host.clone(), ip, port, key: shared.key.public, secret }.to_uri();
        // Le nom court suffit sur le tailnet (MagicDNS) : c'est lui qu'on saisit.
        let short = host.split('.').next().unwrap_or(&host).to_string();
        self.ticket = Some(Shown { uri, code, host: short, created: Instant::now() });
        Ok(())
    }

    pub fn cancel_ticket(&mut self) {
        self.ticket = None;
        if let Some(s) = self.shared() {
            s.clear_tickets();
        }
    }

    /// La réponse de l'utilisateur à une demande d'appairage.
    pub fn decide(&mut self, index: usize, yes: bool) {
        if index >= self.requests.len() {
            return;
        }
        let r = self.requests.remove(index);
        if yes {
            self.store.add(&r.name, &r.model, &r.key, &r.identity.login, &r.identity.node);
            self.devices_changed();
            let from = if r.identity.login.is_empty() { String::new() } else { format!(" ({})", r.identity.login) };
            self.store.journal(&r.name, "appairage", format!("appairé{from}"));
            self.ticket = None;
        } else {
            self.store.journal(&r.name, "appairage", "refusé sur le PC");
        }
        // L'appareil est connu des fils avant que la réponse ne parte : son `Hello` sera accepté.
        r.answer(yes);
    }

    /// Retire un appareil : ses connexions sont fermées tout de suite.
    pub fn revoke(&mut self, id: &str) {
        let Some(d) = self.store.remove(id) else { return };
        self.devices_changed();
        self.sessions.retain(|s| {
            let keep = s.device != id;
            if !keep {
                s.close("Ce téléphone a été retiré sur le PC.");
            }
            keep
        });
        self.store.journal(&d.name, "appairage", "retiré sur le PC");
    }

    pub fn set_rights(&mut self, id: &str, rights: Rights) {
        let Some(d) = self.store.device_mut(id) else { return };
        if d.rights == rights {
            return;
        }
        d.rights = rights;
        let name = d.name.clone();
        self.store.save();
        self.devices_changed();
        for s in self.sessions.iter_mut().filter(|s| s.device == id) {
            s.rights = rights;
            s.send(ToPhone::Toast { text: "Vos droits ont changé sur le PC.".into(), tone: Default::default() });
        }
        self.store.journal(
            &name,
            "acces",
            format!(
                "droits : agir {}, terminaux {}, dossiers {}",
                yes_no(rights.act),
                yes_no(rights.terminals),
                yes_no(rights.files)
            ),
        );
    }

    /// Les noms des appareils connectés.
    pub fn connected(&self) -> Vec<&str> {
        self.sessions.iter().map(|s| s.name.as_str()).collect()
    }
}

fn yes_no(b: bool) -> &'static str {
    if b { "oui" } else { "non" }
}

impl Drop for Bridge {
    fn drop(&mut self) {
        self.stop("Eyrie se ferme.");
    }
}
