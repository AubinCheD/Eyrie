//! Le veilleur du PC, vu du téléphone (voir `suite_mobile::veille`) : quand la liaison est coupée, ce que l'on sait du
//! PC (téléphone hors ligne, PC hors du tailnet, Eyrie fermée, en démarrage, accès coupé sur le PC), et la demande
//! « Démarrer Eyrie ».
//!
//! - l'échange en lignes JSON avec le veilleur : [`probe`] (son salut seul : que dit-il d'Eyrie ?) et [`start`]
//!   (salut et défi, preuve, réponse) ;
//! - le rythme des essais de la liaison coupée : [`Watch`] (le veilleur n'est sondé qu'après une connexion ratée, au
//!   plus toutes les quelques secondes) ;
//! - ce que l'interface en montre : [`shown`].

use std::io::{BufRead, BufReader, Read, Write};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use suite_mobile::client::Stream;
use suite_mobile::msg::WakeKey;
use suite_mobile::veille;

use crate::store::{from_hex, hex};
use crate::tailnet::{Peer, Tailnet};

/// Eyrie, d'après le veilleur.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Eyrie {
    Fermee,
    Demarrage,
    Ouverte,
    /// « Accès depuis le téléphone » est coupé sur le PC : le veilleur ne démarre rien.
    AccesCoupe,
}

impl Eyrie {
    fn parse(s: &str) -> Option<Eyrie> {
        match s {
            "fermee" => Some(Eyrie::Fermee),
            "demarrage" => Some(Eyrie::Demarrage),
            "ouverte" => Some(Eyrie::Ouverte),
            "acces-coupe" => Some(Eyrie::AccesCoupe),
            _ => None,
        }
    }
}

/// Ce que l'on sait du PC quand la liaison est coupée.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Reach {
    /// Rien de plus que l'échec de la connexion : pas encore sondé, ou le PC est en ligne (ou sans nœud pour le dire,
    /// sur le bureau) et son veilleur ne répond pas.
    #[default]
    Unknown,
    /// Le nœud Tailscale du téléphone n'est pas en ligne.
    PhoneOffline,
    /// Le PC n'est pas en ligne sur le tailnet (éteint, en veille, hors de Tailscale) ; vu pour la dernière fois
    /// (secondes Unix ; 0 : on l'ignore).
    PcAway { seen: i64 },
    /// Le veilleur du PC répond.
    Standby(Eyrie),
}

/// Ce que le nœud du téléphone dit du PC avant toute sonde ; `None` quand rien n'empêche de sonder le veilleur (pas de
/// nœud sur le bureau : connexion directe ; PC en ligne sur le tailnet). `peer` n'est demandé qu'au nœud en marche :
/// sans réponse de lui, le téléphone n'est pas en ligne. Le nœud qui démarre, attend la connexion au compte ou a
/// échoué : rien de plus, l'échec de la connexion dit déjà pourquoi.
pub fn from_node(node: &Tailnet, peer: impl FnOnce() -> Option<Peer>) -> Option<Reach> {
    match node {
        Tailnet::Absent => None,
        Tailnet::Running { .. } => match peer() {
            None => Some(Reach::PhoneOffline),
            Some(p) if !p.reachable() => Some(Reach::PcAway { seen: p.seen }),
            Some(_) => None,
        },
        Tailnet::Starting | Tailnet::NeedsLogin(_) | Tailnet::Failed(_) => Some(Reach::Unknown),
    }
}

/// La liaison coupée, telle que l'interface la montre.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Away {
    pub reach: Reach,
    /// Le PC a dit qu'Eyrie se fermait (`Bye { closing }`), depuis la dernière connexion.
    pub closed: bool,
    /// « Démarrer Eyrie » a été accepté il y a moins d'une minute : la liaison réessaie souvent.
    pub starting: bool,
}

// --- L'échange avec le veilleur ---

/// Une ligne du veilleur fait 1 Kio au plus (comme celles qu'il accepte).
const LINE_MAX: usize = 1024;
/// La connexion au veilleur, à chaque adresse.
const DIAL: Duration = Duration::from_secs(3);
/// Le veilleur répond en 5 s au plus (sa propre limite pour tout l'échange).
const ANSWER: Duration = Duration::from_secs(5);

/// Le refus qui dit que la clé de ce téléphone ne vaut pas (ou plus) : pas de nouvel essai avant qu'Eyrie en donne une
/// autre (5 preuves fausses bloquent l'appareil un quart d'heure).
pub const PROOF_REFUSED: &str = "preuve refusée";

/// Le salut du veilleur.
#[derive(Deserialize)]
struct Hello {
    v: u32,
    defi: String,
    eyrie: String,
}

/// La demande de démarrage.
#[derive(Serialize)]
struct Request<'a> {
    appareil: &'a str,
    demande: &'a str,
    preuve: String,
}

#[derive(Deserialize)]
struct Reply {
    ok: bool,
    #[serde(default)]
    raison: String,
}

/// La réponse à « Démarrer Eyrie ».
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Answer {
    /// Eyrie démarre (ou démarrait déjà).
    Starting,
    /// Refusé : la raison du veilleur, telle quelle.
    Refused(String),
    /// Eyrie n'était pas fermée (ouverte, accès coupé) : rien n'a été demandé.
    NotClosed(Eyrie),
}

/// Une ligne, sans sa fin ; le veilleur qui ferme sans rien dire (appareil qu'il ne connaît pas, réglage illisible)
/// est une erreur.
fn read_line(r: &mut impl BufRead) -> Result<String, String> {
    let mut buf = Vec::new();
    let n = Read::take(&mut *r, LINE_MAX as u64 + 1).read_until(b'\n', &mut buf).map_err(|e| e.to_string())?;
    if n == 0 {
        return Err("le veilleur du PC n'a rien répondu".into());
    }
    if buf.last() != Some(&b'\n') {
        return Err(if n > LINE_MAX { "ligne du veilleur trop longue" } else { "réponse du veilleur coupée" }.into());
    }
    String::from_utf8(buf).map(|s| s.trim_end().to_string()).map_err(|_| "réponse du veilleur illisible".into())
}

/// Le salut : le défi, et ce que le veilleur dit d'Eyrie.
fn greeting(line: &str) -> Result<([u8; 32], Eyrie), String> {
    let h: Hello = serde_json::from_str(line).map_err(|_| "salut du veilleur illisible")?;
    if h.v != 1 {
        return Err(format!("veilleur en version {}, inconnue de cette application", h.v));
    }
    let challenge = from_hex(&h.defi).and_then(|b| b.try_into().ok()).ok_or("défi du veilleur illisible")?;
    let eyrie = Eyrie::parse(&h.eyrie).ok_or_else(|| format!("état d'Eyrie inconnu : {}", h.eyrie))?;
    Ok((challenge, eyrie))
}

/// La demande de démarrage, une ligne JSON terminée par `\n` : la preuve que cet appareil connaît son secret, liée au
/// défi. `None` si la clé ne sert pas (secret qui n'a pas 32 octets).
pub fn request(wake: &WakeKey, challenge: &[u8; 32]) -> Option<String> {
    let secret: [u8; 32] = wake.secret.as_slice().try_into().ok()?;
    let preuve = hex(&veille::proof(&secret, challenge, &wake.device, veille::START));
    let mut line = serde_json::to_string(&Request { appareil: &wake.device, demande: veille::START, preuve }).ok()?;
    line.push('\n');
    Some(line)
}

/// Le salut seul : ce que le veilleur dit d'Eyrie. On ne demande rien ; il ferme de lui-même.
pub fn probe(stream: impl Read) -> Result<Eyrie, String> {
    Ok(greeting(&read_line(&mut BufReader::new(stream))?)?.1)
}

/// L'échange entier : le salut, la demande (seulement si Eyrie est fermée), la réponse.
pub fn start<S: Read + Write>(stream: S, wake: &WakeKey) -> Result<Answer, String> {
    let mut r = BufReader::new(stream);
    let (challenge, eyrie) = greeting(&read_line(&mut r)?)?;
    match eyrie {
        Eyrie::Fermee => {}
        Eyrie::Demarrage => return Ok(Answer::Starting),
        other => return Ok(Answer::NotClosed(other)),
    }
    let line = request(wake, &challenge).ok_or("clé de démarrage illisible")?;
    let w = r.get_mut();
    w.write_all(line.as_bytes()).and_then(|_| w.flush()).map_err(|e| e.to_string())?;
    let reply: Reply = serde_json::from_str(&read_line(&mut r)?).map_err(|_| "réponse du veilleur illisible")?;
    Ok(match reply.ok {
        true => Answer::Starting,
        false if reply.raison.is_empty() => Answer::Refused("refus sans raison".into()),
        false => Answer::Refused(reply.raison),
    })
}

/// Les adresses du veilleur : celles du PC, au port du veilleur.
pub fn standby_addrs(addrs: &[String], port: u16) -> Vec<String> {
    let host = |a: &String| a.rsplit_once(':').map(|(h, _)| h.to_string()).unwrap_or_else(|| a.clone());
    addrs.iter().map(|a| format!("{}:{port}", host(a))).collect()
}

/// Se connecte au veilleur : la première adresse qui répond, délais courts.
fn connect(addrs: &[String], dial: &dyn Fn(&str, Duration) -> Result<Stream, String>) -> Result<Stream, String> {
    let mut last = String::from("aucune adresse");
    for a in addrs {
        match dial(a, DIAL) {
            Ok(s) => {
                let _ = s.set_read_timeout(Some(ANSWER));
                let _ = s.set_write_timeout(Some(ANSWER));
                return Ok(s);
            }
            Err(e) => last = e,
        }
    }
    Err(last)
}

/// Sonde le veilleur à ses adresses (voir [`standby_addrs`]).
pub fn ask_state(addrs: &[String], dial: &dyn Fn(&str, Duration) -> Result<Stream, String>) -> Result<Eyrie, String> {
    probe(connect(addrs, dial)?)
}

/// Demande au veilleur de démarrer Eyrie.
pub fn ask_start(
    addrs: &[String],
    wake: &WakeKey,
    dial: &dyn Fn(&str, Duration) -> Result<Stream, String>,
) -> Result<Answer, String> {
    start(connect(addrs, dial)?, wake)
}

// --- Le rythme des essais ---

/// Le rythme des essais de la liaison coupée.
#[derive(Clone, Copy, Debug)]
pub struct Pace {
    /// Le premier délai, doublé à chaque échec jusqu'à `max`.
    pub first: Duration,
    pub max: Duration,
    /// Après « Eyrie se ferme », le premier délai : le temps qu'elle se ferme, sans marteler le PC.
    pub closing: Duration,
    /// Eyrie fermée ou l'accès coupé : un essai à ce rythme (rouverte sur le PC, elle est vue vite).
    pub closed: Duration,
    /// Une sonde du veilleur au plus par ce temps.
    pub probe: Duration,
    /// Après « Démarrer Eyrie », un essai à ce rythme, pendant `starting_for`.
    pub starting: Duration,
    pub starting_for: Duration,
}

pub const PACE: Pace = Pace {
    first: Duration::from_secs(1),
    max: Duration::from_secs(30),
    closing: Duration::from_secs(3),
    closed: Duration::from_secs(10),
    probe: Duration::from_secs(3),
    starting: Duration::from_secs(2),
    starting_for: Duration::from_secs(60),
};

/// Ce que la liaison coupée sait du PC, et quand elle réessaie.
pub struct Watch {
    pace: Pace,
    backoff: Duration,
    away: Away,
    probed_at: Option<Instant>,
    starting_until: Option<Instant>,
}

impl Watch {
    pub fn new(pace: Pace) -> Watch {
        Watch { pace, backoff: pace.first, away: Away::default(), probed_at: None, starting_until: None }
    }

    /// Connecté : tout est oublié.
    pub fn online(&mut self) {
        *self = Watch::new(self.pace);
    }

    /// Le PC a dit qu'Eyrie se fermait : le premier essai lui laisse le temps de se fermer, puis la sonde dira si le
    /// veilleur la garde joignable.
    pub fn closing(&mut self) {
        self.away.closed = true;
        self.backoff = self.pace.closing;
    }

    fn starting(&self, now: Instant) -> bool {
        self.starting_until.is_some_and(|t| now < t)
    }

    /// Une connexion vient d'échouer : le veilleur peut-il être sondé (une fois par `pace.probe` au plus) ?
    pub fn may_probe(&self, now: Instant) -> bool {
        self.probed_at.is_none_or(|t| now.saturating_duration_since(t) >= self.pace.probe)
    }

    /// Ce que la sonde a trouvé.
    pub fn probed(&mut self, reach: Reach, now: Instant) {
        self.probed_at = Some(now);
        // Fermée pendant son démarrage : elle s'est arrêtée, ou n'a pas pu s'ouvrir ; on n'attend plus.
        if reach == Reach::Standby(Eyrie::Fermee) {
            self.starting_until = None;
        }
        self.away.reach = reach;
    }

    /// La réponse à « Démarrer Eyrie » (elle vaut une sonde).
    pub fn started(&mut self, answer: &Answer, now: Instant) {
        self.probed_at = Some(now);
        match answer {
            Answer::Starting => {
                self.starting_until = Some(now + self.pace.starting_for);
                self.away.reach = Reach::Standby(Eyrie::Demarrage);
                self.backoff = self.pace.first;
            }
            Answer::NotClosed(e) => self.away.reach = Reach::Standby(*e),
            Answer::Refused(_) => {}
        }
    }

    pub fn away(&self, now: Instant) -> Away {
        Away { starting: self.starting(now), ..self.away }
    }

    /// Le délai avant le prochain essai.
    pub fn delay(&mut self, now: Instant) -> Duration {
        if self.starting(now) {
            return self.pace.starting;
        }
        if matches!(self.away.reach, Reach::Standby(Eyrie::Fermee | Eyrie::AccesCoupe)) {
            return self.pace.closed;
        }
        let d = self.backoff;
        self.backoff = (self.backoff * 2).min(self.pace.max);
        d
    }
}

// --- Ce que l'interface montre ---

/// La demande « Démarrer Eyrie », vue par l'interface.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Start {
    #[default]
    Idle,
    /// Partie, sans réponse encore.
    Asked,
    /// Refusée : la raison du veilleur, telle quelle.
    Refused(String),
    /// Le veilleur n'a pas répondu.
    Failed(String),
}

/// La liaison coupée, à l'écran.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Shown {
    /// 0 : rien de plus que la raison de la coupure ; 1 PC injoignable ; 2 Eyrie fermée ; 3 Eyrie démarre ; 4 accès
    /// coupé sur le PC.
    pub kind: i32,
    /// La ligne d'état (bandeau du haut, carte du PC) ; vide pour `kind` 0.
    pub line: String,
    pub title: String,
    pub text: String,
    /// Le refus du veilleur (sa raison, telle quelle) ou son silence.
    pub error: String,
    pub can_start: bool,
}

/// Ce que l'interface montre de la liaison coupée : `wake`, ce téléphone a une clé de démarrage pour ce PC ; `now`,
/// en secondes Unix (depuis quand le PC est hors ligne).
pub fn shown(away: &Away, start: &Start, wake: bool, now: u64) -> Shown {
    let error = match start {
        Start::Refused(r) => format!("Démarrage refusé : {r}"),
        Start::Failed(e) => format!("Le veilleur du PC n'a pas répondu ({e})."),
        Start::Idle | Start::Asked => String::new(),
    };
    let proof_refused = matches!(start, Start::Refused(r) if r == PROOF_REFUSED);
    // La carte suit celle du PC, qui le nomme : son titre est plus court que la ligne d'état.
    let (closed_line, closed_title) = match away.closed {
        true => ("Eyrie a été fermée sur le PC", "Eyrie a été fermée"),
        false => ("Eyrie est fermée sur le PC", "Eyrie est fermée"),
    };
    let s = |kind: i32, line: &str, title: &str, text: String| Shown {
        kind,
        line: line.into(),
        title: title.into(),
        text,
        ..Shown::default()
    };
    if *start == Start::Asked {
        return s(3, "Démarrage d'Eyrie demandé…", "Démarrage d'Eyrie…", "La demande part au PC.".into());
    }
    if away.starting || away.reach == Reach::Standby(Eyrie::Demarrage) {
        let text = "Ce téléphone s'y connecte dès qu'elle est prête.".into();
        return s(3, "Eyrie démarre sur le PC…", "Eyrie démarre…", text);
    }
    match away.reach {
        Reach::Standby(Eyrie::Fermee) => {
            let can_start = wake && !proof_refused;
            let text = if can_start {
                "Le PC est allumé : vous pouvez la démarrer d'ici."
            } else if proof_refused {
                "Ce téléphone pourra de nouveau la démarrer après une connexion à Eyrie ouverte sur le PC."
            } else {
                "Le PC est allumé, mais ce téléphone ne peut pas encore la démarrer : il en reçoit le droit à sa \
                 prochaine connexion à Eyrie."
            };
            Shown { error, can_start, ..s(2, closed_line, closed_title, text.into()) }
        }
        Reach::Standby(Eyrie::AccesCoupe) => {
            let text = "« Accès depuis le téléphone » est coupé dans Eyrie, sur le PC : rien ne passe avant qu'il y \
                        soit rouvert.";
            Shown { error, ..s(4, "Accès coupé sur le PC", "Accès coupé", text.into()) }
        }
        Reach::PhoneOffline => {
            let text = "Pas de réseau, ou Tailscale n'est pas connecté sur ce téléphone (Réglages › Tailscale).";
            s(1, "PC injoignable : ce téléphone est hors ligne", "Ce téléphone est hors ligne", text.into())
        }
        Reach::PcAway { .. } if away.closed => {
            let text = "Le PC n'est plus sur Tailscale : pour démarrer Eyrie d'ici, cochez « Garder le PC joignable \
                        quand Eyrie est fermée » dans Eyrie, sur le PC.";
            s(2, closed_line, closed_title, text.into())
        }
        Reach::PcAway { seen } => {
            let mut text = String::from("Éteint, en veille ou hors de Tailscale.");
            if seen > 0 {
                let seen = seen as u64;
                match crate::view::ago(seen, now) {
                    a if now.saturating_sub(seen) < 60 => text.push_str(&format!(" Vu sur Tailscale {a}.")),
                    a => text.push_str(&format!(" Vu sur Tailscale il y a {a}.")),
                }
            }
            s(1, "PC injoignable", "PC injoignable", text)
        }
        // Le veilleur ne dit rien (ou qu'Eyrie est encore ouverte) : on sait seulement qu'elle s'est fermée.
        Reach::Unknown | Reach::Standby(Eyrie::Ouverte) if away.closed => {
            s(2, closed_line, closed_title, String::new())
        }
        Reach::Unknown | Reach::Standby(_) => Shown::default(),
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    /// Un flux en mémoire : ce que le veilleur dit, ce que le téléphone écrit.
    struct Wire {
        from_pc: Cursor<Vec<u8>>,
        to_pc: Vec<u8>,
    }

    impl Wire {
        fn new(lines: &str) -> Wire {
            Wire { from_pc: Cursor::new(lines.as_bytes().to_vec()), to_pc: Vec::new() }
        }
    }

    impl Read for &mut Wire {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            self.from_pc.read(buf)
        }
    }

    impl Write for &mut Wire {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.to_pc.write(buf)
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn hello(eyrie: &str) -> String {
        format!("{{\"v\":1,\"defi\":\"{}\",\"eyrie\":\"{eyrie}\"}}\n", "03".repeat(32))
    }

    /// La clé des vecteurs partagés avec `compagnon/veille_test.go` et `suite_mobile::veille` : clé du PC 0x01…, clé
    /// publique 0x02…, appareil `appareil-1`.
    fn shared_key() -> WakeKey {
        WakeKey {
            port: veille::PORT,
            secret: veille::device_secret(&[1; 32], &[2; 32]).to_vec(),
            device: "appareil-1".into(),
        }
    }

    /// Le défi 0x03… donne la preuve des vecteurs partagés, dans la ligne qu'attend le veilleur (terminée par `\n`).
    #[test]
    fn the_start_request_carries_the_shared_vector_proof() {
        let mut w = Wire::new(&(hello("fermee") + "{\"ok\":true,\"etat\":\"demarrage\"}\n"));
        assert_eq!(start(&mut w, &shared_key()), Ok(Answer::Starting));
        assert_eq!(
            String::from_utf8(w.to_pc).unwrap(),
            "{\"appareil\":\"appareil-1\",\"demande\":\"demarrer\",\
             \"preuve\":\"ad8399515aa176a2c4cd1622d9763ab2df25c9133b54198cbd680b04c4310953\"}\n"
        );
    }

    /// Un refus est rendu tel quel ; le PC qui ne démarre pas Eyrie parce qu'elle n'est pas fermée ne reçoit rien.
    #[test]
    fn refusals_are_verbatim_and_nothing_is_asked_unless_eyrie_is_closed() {
        let reason = "trop de démarrages : réessayez dans 25 s";
        let mut w = Wire::new(&(hello("fermee") + &format!("{{\"ok\":false,\"raison\":\"{reason}\"}}\n")));
        assert_eq!(start(&mut w, &shared_key()), Ok(Answer::Refused(reason.into())));
        let mut w = Wire::new(&(hello("fermee") + "{\"ok\":false}\n"));
        assert_eq!(start(&mut w, &shared_key()), Ok(Answer::Refused("refus sans raison".into())));

        for (state, answer) in [
            ("demarrage", Answer::Starting),
            ("ouverte", Answer::NotClosed(Eyrie::Ouverte)),
            ("acces-coupe", Answer::NotClosed(Eyrie::AccesCoupe)),
        ] {
            let mut w = Wire::new(&hello(state));
            assert_eq!(start(&mut w, &shared_key()), Ok(answer), "{state}");
            assert!(w.to_pc.is_empty(), "{state} : rien n'est demandé");
        }
    }

    /// La sonde lit le salut seul ; un veilleur muet, une ligne trop longue, coupée ou illisible sont des erreurs.
    #[test]
    fn the_probe_reads_the_greeting_and_rejects_anything_else() {
        assert_eq!(probe(Cursor::new(hello("fermee"))), Ok(Eyrie::Fermee));
        assert_eq!(probe(Cursor::new(hello("acces-coupe"))), Ok(Eyrie::AccesCoupe));
        assert!(probe(Cursor::new("")).unwrap_err().contains("rien répondu"));
        assert!(probe(Cursor::new(hello("fermee").trim_end().to_string())).unwrap_err().contains("coupée"));
        let long = format!("{{\"v\":1,\"x\":\"{}\"}}\n", "a".repeat(LINE_MAX));
        assert!(probe(Cursor::new(long)).unwrap_err().contains("trop longue"));
        assert!(probe(Cursor::new(hello("endormie"))).unwrap_err().contains("endormie"));
        assert!(probe(Cursor::new(hello("fermee").replace("\"v\":1", "\"v\":2"))).is_err());
        assert!(probe(Cursor::new("{\"v\":1,\"defi\":\"0303\",\"eyrie\":\"fermee\"}\n")).is_err(), "défi court");
        // Une clé abîmée ne part pas.
        let bad = WakeKey { secret: vec![1; 31], ..shared_key() };
        assert!(start(&mut Wire::new(&hello("fermee")), &bad).is_err());
    }

    #[test]
    fn the_standby_is_reached_at_the_pcs_addresses_on_its_own_port() {
        let addrs = vec!["100.1.2.3:4747".to_string(), "suite-pc.tail1.ts.net:4747".to_string()];
        assert_eq!(standby_addrs(&addrs, 4748), vec!["100.1.2.3:4748", "suite-pc.tail1.ts.net:4748"]);
        assert_eq!(standby_addrs(&["suite-pc".to_string()], 4748), vec!["suite-pc:4748"]);
    }

    /// Le nœud du téléphone tranche avant toute sonde : hors ligne, ou PC absent du tailnet ; le bureau (sans nœud) et
    /// un PC en ligne laissent sonder le veilleur ; un nœud pas prêt ne fait rien sonder (la connexion dit pourquoi).
    #[test]
    fn the_phones_node_tells_an_unreachable_pc_before_any_probe() {
        let running = Tailnet::Running { name: "eyrie-tel".into(), ip: "100.64.0.7".into() };
        let peer = |known, online| move || Some(Peer { known, online, seen: 42 });
        let unasked = || panic!("le nœud ne sait rien du PC");
        assert_eq!(from_node(&Tailnet::Absent, unasked), None, "bureau");
        assert_eq!(from_node(&Tailnet::Starting, unasked), Some(Reach::Unknown));
        assert_eq!(from_node(&Tailnet::NeedsLogin("u".into()), unasked), Some(Reach::Unknown));
        assert_eq!(from_node(&Tailnet::Failed("x".into()), unasked), Some(Reach::Unknown));
        assert_eq!(from_node(&running, || None), Some(Reach::PhoneOffline), "nœud pas en ligne");
        assert_eq!(from_node(&running, peer(true, false)), Some(Reach::PcAway { seen: 42 }));
        assert_eq!(from_node(&running, peer(false, false)), Some(Reach::PcAway { seen: 42 }));
        assert_eq!(from_node(&running, peer(true, true)), None);
    }

    const TEST_PACE: Pace = Pace {
        first: Duration::from_secs(1),
        max: Duration::from_secs(8),
        closing: Duration::from_secs(3),
        closed: Duration::from_secs(10),
        probe: Duration::from_secs(3),
        starting: Duration::from_secs(2),
        starting_for: Duration::from_secs(60),
    };

    #[test]
    fn the_standby_is_probed_at_most_every_few_seconds() {
        let t0 = Instant::now();
        let mut w = Watch::new(TEST_PACE);
        assert!(w.may_probe(t0));
        w.probed(Reach::Standby(Eyrie::Fermee), t0);
        assert!(!w.may_probe(t0 + Duration::from_secs(2)));
        assert!(w.may_probe(t0 + Duration::from_secs(3)));
        assert_eq!(w.away(t0).reach, Reach::Standby(Eyrie::Fermee));
    }

    /// Un PC qui ne répond pas : délai croissant ; Eyrie fermée (ou l'accès coupé) : un essai toutes les 10 s, sans
    /// croître ; « Eyrie se ferme » : un premier essai après 3 s, pas tout de suite.
    #[test]
    fn retries_slow_down_unless_the_standby_says_eyrie_is_closed() {
        let t0 = Instant::now();
        let mut w = Watch::new(TEST_PACE);
        let delays: Vec<u64> = (0..5).map(|_| w.delay(t0).as_secs()).collect();
        assert_eq!(delays, [1, 2, 4, 8, 8]);
        w.probed(Reach::Standby(Eyrie::AccesCoupe), t0);
        assert_eq!((w.delay(t0).as_secs(), w.delay(t0).as_secs()), (10, 10));

        let mut w = Watch::new(TEST_PACE);
        w.closing();
        assert_eq!(w.away(t0), Away { reach: Reach::Unknown, closed: true, starting: false });
        assert_eq!(w.delay(t0).as_secs(), 3, "le temps qu'elle se ferme");
        w.probed(Reach::Standby(Eyrie::Fermee), t0);
        assert_eq!(w.delay(t0).as_secs(), 10);
        assert!(w.away(t0).closed, "« a été fermée » jusqu'à la prochaine connexion");
        w.online();
        assert_eq!(w.away(t0), Away::default());
        assert_eq!(w.delay(t0).as_secs(), 1);
    }

    /// « Démarrer Eyrie » accepté : un essai toutes les 2 s pendant une minute, puis le rythme ordinaire ; une Eyrie
    /// qui se referme pendant son démarrage arrête l'attente ; un refus ne change rien.
    #[test]
    fn after_a_start_the_link_retries_often_for_a_minute() {
        let t0 = Instant::now();
        let mut w = Watch::new(TEST_PACE);
        w.probed(Reach::Standby(Eyrie::Fermee), t0);
        w.started(&Answer::Refused("trop de démarrages : réessayez dans 25 s".into()), t0);
        assert_eq!(w.away(t0).reach, Reach::Standby(Eyrie::Fermee));
        assert_eq!(w.delay(t0).as_secs(), 10);

        w.started(&Answer::Starting, t0);
        assert!(!w.may_probe(t0), "la réponse vaut une sonde");
        assert_eq!(w.away(t0), Away { reach: Reach::Standby(Eyrie::Demarrage), closed: false, starting: true });
        let t30 = t0 + Duration::from_secs(30);
        w.probed(Reach::Standby(Eyrie::Ouverte), t30);
        assert_eq!(w.delay(t30).as_secs(), 2);
        assert!(w.away(t30).starting, "ouverte, mais pas encore jointe");
        let t61 = t0 + Duration::from_secs(61);
        assert!(!w.away(t61).starting);
        assert_eq!((w.delay(t61).as_secs(), w.delay(t61).as_secs()), (1, 2), "le rythme ordinaire, de nouveau");

        let mut w = Watch::new(TEST_PACE);
        w.started(&Answer::Starting, t0);
        w.probed(Reach::Standby(Eyrie::Fermee), t0 + Duration::from_secs(5));
        assert!(!w.away(t0 + Duration::from_secs(5)).starting, "elle s'est arrêtée en démarrant");

        let mut w = Watch::new(TEST_PACE);
        w.started(&Answer::NotClosed(Eyrie::AccesCoupe), t0);
        assert_eq!(w.away(t0).reach, Reach::Standby(Eyrie::AccesCoupe));
    }

    fn away(reach: Reach) -> Away {
        Away { reach, ..Away::default() }
    }

    /// Les états distincts : PC injoignable (téléphone hors ligne, PC hors du tailnet), Eyrie fermée (démarrable ou
    /// non), Eyrie démarre, accès coupé ; sans rien de sûr, la raison de la coupure suffit.
    #[test]
    fn each_state_is_told_apart() {
        let now = 1_761_600_000;
        let s = shown(&away(Reach::Standby(Eyrie::Fermee)), &Start::Idle, true, now);
        assert_eq!((s.kind, s.line.as_str(), s.can_start), (2, "Eyrie est fermée sur le PC", true));
        let s = shown(&away(Reach::Standby(Eyrie::Fermee)), &Start::Idle, false, now);
        assert!(!s.can_start && s.text.contains("prochaine connexion"), "sans clé : {s:?}");

        let s = shown(&away(Reach::PcAway { seen: now as i64 - 7200 }), &Start::Idle, true, now);
        assert_eq!((s.kind, s.line.as_str()), (1, "PC injoignable"));
        assert_eq!(s.text, "Éteint, en veille ou hors de Tailscale. Vu sur Tailscale il y a 2 h.");
        assert!(!s.can_start);
        let s = shown(&away(Reach::PcAway { seen: 0 }), &Start::Idle, true, now);
        assert_eq!(s.text, "Éteint, en veille ou hors de Tailscale.");
        let s = shown(&away(Reach::PhoneOffline), &Start::Idle, true, now);
        assert_eq!((s.kind, s.title.as_str()), (1, "Ce téléphone est hors ligne"));

        let s = shown(&away(Reach::Standby(Eyrie::Demarrage)), &Start::Idle, true, now);
        assert_eq!((s.kind, s.line.as_str(), s.can_start), (3, "Eyrie démarre sur le PC…", false));
        let s = shown(&Away { starting: true, ..away(Reach::Standby(Eyrie::Ouverte)) }, &Start::Idle, true, now);
        assert_eq!(s.kind, 3, "jusqu'à la connexion");
        assert_eq!(shown(&away(Reach::Standby(Eyrie::Fermee)), &Start::Asked, true, now).kind, 3);

        let s = shown(&away(Reach::Standby(Eyrie::AccesCoupe)), &Start::Idle, true, now);
        assert_eq!((s.kind, s.line.as_str(), s.can_start), (4, "Accès coupé sur le PC", false));

        assert_eq!(shown(&away(Reach::Unknown), &Start::Idle, true, now).kind, 0);
        assert_eq!(shown(&away(Reach::Standby(Eyrie::Ouverte)), &Start::Idle, true, now).kind, 0);
    }

    /// « Eyrie se ferme » : dit tout de suite, avant toute sonde ; affiné ensuite par le veilleur.
    #[test]
    fn a_closing_eyrie_is_told_before_the_standby_is_probed() {
        let now = 1_761_600_000;
        let closed = |reach| Away { reach, closed: true, starting: false };
        let s = shown(&closed(Reach::Unknown), &Start::Idle, true, now);
        assert_eq!((s.kind, s.line.as_str(), s.can_start), (2, "Eyrie a été fermée sur le PC", false));
        let s = shown(&closed(Reach::Standby(Eyrie::Fermee)), &Start::Idle, true, now);
        assert_eq!((s.kind, s.line.as_str(), s.can_start), (2, "Eyrie a été fermée sur le PC", true));
        let s = shown(&closed(Reach::PcAway { seen: 0 }), &Start::Idle, true, now);
        assert!(s.kind == 2 && !s.can_start && s.text.contains("Garder le PC joignable"), "{s:?}");
    }

    /// Un refus du veilleur est montré tel quel ; « preuve refusée » ôte le bouton (pas de nouvel essai avec la même
    /// clé), les autres refus le laissent ; un veilleur muet aussi.
    #[test]
    fn a_refusal_is_shown_verbatim_and_a_refused_proof_is_not_retried() {
        let now = 1_761_600_000;
        let fermee = away(Reach::Standby(Eyrie::Fermee));
        let s = shown(&fermee, &Start::Refused(PROOF_REFUSED.into()), true, now);
        assert_eq!((s.error.as_str(), s.can_start), ("Démarrage refusé : preuve refusée", false));
        let reason = "Eyrie n'est pas à l'abri sur le PC (modifiable par un autre compte) : voir le journal du PC";
        let s = shown(&fermee, &Start::Refused(reason.into()), true, now);
        assert_eq!(s.error, format!("Démarrage refusé : {reason}"));
        assert!(s.can_start);
        let s = shown(&fermee, &Start::Failed("délai dépassé".into()), true, now);
        assert_eq!((s.error.as_str(), s.can_start), ("Le veilleur du PC n'a pas répondu (délai dépassé).", true));
    }
}
