//! Owlery dans la suite.
//!
//! Owlery est pensé pour l'injection (`SecretStore`, `Notifier`, `FawkesLink`, `Fetcher`) : on refait
//! ce que fait son `main.rs`, en lui donnant les services de la suite.
//! - **comptes** : ceux de toute la suite (`suite-accounts`), qu'Owlery lit lui-même ; « Comptes… » ouvre la page
//!   Comptes d'Eyrie ;
//! - **notifications** : dans le centre de notifications de la suite (une seule icône) ;
//! - **Fawkes** : `ouvrir` passe par le bus de la suite, en appel Rust, au Fawkes de la suite ;
//! - même verrou, même `state.lock` et même canal nommé que `owlery.exe` (`afficher`, `etat`).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};

use eframe::egui::Color32;
use owlery::app::{App, FawkesLink, Notifier, StartOptions};
use owlery::contract::{self, Snapshot};
use owlery::forges::{Fetcher, ForgeFetcher, http::UreqHttp};
use owlery::secrets::SecretStore;
use owlery::state;
use suite_sdk::embed::{Embedded, KeyCommand, first_number};
use suite_sdk::{
    Badge, BuildCx, Host, Module, ModuleFactory, ModuleInfo, Notification, Origin, Request, Tone, Unavailable,
};

pub const ID: &str = "owlery";

/// `owlery::win::single_instance` garde son verrou jusqu'à la fin du processus.
static HELD: AtomicBool = AtomicBool::new(false);

pub fn info() -> ModuleInfo {
    ModuleInfo {
        id: ID,
        name: "Owlery",
        icon: "✉",
        color: Color32::from_rgb(0x3a, 0xa8, 0xd8),
        summary: "Boîte de réception : PR à relire, CI en échec, mentions, tickets",
        origin: Origin::Embedded { exe: "owlery.exe", repo: "../Owlery" },
    }
}

pub fn factory() -> ModuleFactory {
    ModuleFactory { info: info(), build, fonts: None }
}

fn commands() -> Vec<KeyCommand> {
    vec![
        KeyCommand::new("actualiser", "Actualiser", "F5"),
        KeyCommand::new("rechercher", "Rechercher", "Ctrl+F"),
        KeyCommand::new("tout_lu", "Tout marquer comme lu", "Maj+R"),
        KeyCommand::new("annuler", "Annuler", "Ctrl+Z"),
        KeyCommand::new("grouper", "Changer le regroupement", "Ctrl+G"),
        KeyCommand::new("en_attente", "En attente seulement", "Ctrl+W"),
        KeyCommand::new("palette", "Palette d'Owlery", "Ctrl+P"),
        KeyCommand::new("reglages", "Réglages d'Owlery", "Ctrl+,"),
    ]
}

fn badge(title: &str) -> Option<Badge> {
    first_number(title).filter(|n| *n > 0).map(|n| Badge::count(n, Tone::Info, format!("{n} élément(s) en attente")))
}

struct SuiteNotifier {
    host: Host,
}

impl Notifier for SuiteNotifier {
    fn show(&self, title: &str, body: &str) {
        self.host.notify(Notification::new(ID, title, body, Tone::Info));
    }

    fn hide(&self) {}
}

/// Le contrat vers Fawkes (`ouvrir`…), porté par le bus de la suite.
struct SuiteFawkes {
    host: Host,
}

impl FawkesLink for SuiteFawkes {
    fn available(&self) -> bool {
        self.host.is_running("fawkes")
    }

    fn send(&self, line: &str) -> Result<String, String> {
        let v: serde_json::Value = serde_json::from_str(line).map_err(|e| e.to_string())?;
        let demande = v.get("demande").and_then(|d| d.as_str()).unwrap_or_default().to_string();
        self.host.send(Request { from: ID.into(), to: "fawkes".into(), demande, data: v });
        Ok(r#"{"v":1,"ok":true}"#.into())
    }

    fn find_exe(&self, _configured: &str) -> Option<std::path::PathBuf> {
        None
    }

    fn start(&self, _exe: &std::path::Path) -> Result<(), String> {
        Err("Fawkes est un module d'Eyrie : activez-le dans les réglages d'Eyrie.".into())
    }
}

fn build(bcx: &BuildCx) -> Result<Box<dyn Module>, Unavailable> {
    let demo = std::env::var_os("OWLERY_DEMO").is_some_and(|v| v == "1");
    let dir = if demo { state::data_dir().join("demo") } else { state::data_dir() };
    let scope = (dir != state::default_data_dir()).then_some(dir.as_path());
    // Les comptes de toute la suite, dans le dossier donné par Eyrie ; la démonstration n'y touche jamais.
    let host_accounts = bcx.host.accounts_dir();
    let accounts = (!demo).then(|| {
        let dir = host_accounts.clone().unwrap_or_else(|| suite_accounts::accounts_dir(scope));
        suite_accounts::Store::new(&dir, Arc::new(suite_accounts::OsVault::suite()))
    });
    let sid = owlery::win::pipe::current_user_sid().unwrap_or_default();
    let pipe = contract::pipe_name("owlery", &sid, scope);
    if !HELD.load(Ordering::Relaxed) {
        if !owlery::win::single_instance(&contract::instance_name(scope)) {
            return Err(Unavailable::AlreadyRunning { exe: "owlery.exe" });
        }
        HELD.store(true, Ordering::Relaxed);
    }
    let lock = state::lock(&dir);
    let mut loaded = state::load(&dir);
    let writable = match &lock {
        Ok(_) => loaded.writable,
        Err(e) => {
            loaded
                .problems
                .push(format!("Dossier d'état verrouillé ({e}) : rien ne sera enregistré pendant cette session."));
            false
        }
    };

    let snapshot = Arc::new(Mutex::new(Snapshot::default()));
    let (incoming_tx, incoming_rx) = mpsc::channel();
    let snap = snapshot.clone();
    let served = owlery::win::pipe::serve(&pipe, move |line| {
        let current = snap.lock().map(|s| s.clone()).unwrap_or_default();
        let (reply, forward) = contract::handle(line, &current);
        if let Some(req) = forward {
            let _ = incoming_tx.send(req);
        }
        reply
    });
    if let Err(e) = served {
        loaded
            .problems
            .push(format!("Canal local indisponible ({e}) : les autres applications ne pourront pas joindre Owlery."));
    }

    let host = bcx.host.clone();
    let store: Arc<dyn SecretStore> = owlery::secrets::store_for(demo);
    let fetcher: Arc<dyn Fetcher> = if demo {
        Arc::new(owlery::demo::DemoFetcher)
    } else {
        Arc::new(ForgeFetcher::new(store.clone(), Arc::new(UreqHttp::default())))
    };
    let opts = StartOptions {
        dir,
        store,
        fetcher,
        demo,
        capture: None,
        incoming: Some(incoming_rx),
        snapshot,
        fawkes: Arc::new(SuiteFawkes { host: host.clone() }),
        notifier: Arc::new(SuiteNotifier { host: host.clone() }),
        writable,
        fetch_on_start: true,
        accounts,
    };
    let mut app = App::new(bcx.cc, opts, loaded);
    // La fenêtre est celle de la suite : sa taille n'entre pas dans l'état d'Owlery.
    app.set_hosted(true);
    // Les comptes se règlent dans Eyrie, pour toute la suite.
    let link = host.clone();
    app.set_accounts_link(Box::new(move || link.open_settings("comptes")));
    Ok(Box::new(Embedded::new(info(), host, app).title_badge(badge).key_commands(commands()).keep(lock)))
}

#[cfg(test)]
mod tests {
    #[test]
    fn badge_from_title() {
        assert_eq!(super::badge("Owlery — 4 en attente").and_then(|b| b.count), Some(4));
        assert_eq!(super::badge("Owlery"), None);
    }
}
