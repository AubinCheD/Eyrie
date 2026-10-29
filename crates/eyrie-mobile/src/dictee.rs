//! La dictée : ce que l'on dit au téléphone devient du texte, ajouté à la saisie du terminal suivi (une session Claude
//! de Fawkes…) ou d'une saisie sur plusieurs lignes (une consigne de Patronus…). La reconnaissance se fait sur le
//! téléphone (Android, `SpeechRecognizer` : `android.rs` et `android/java/…/Dictee.java`) : le son ne va jamais au PC,
//! seul le texte y part, par la saisie du terminal (le droit *terminaux* vérifié par le PC, comme pour le clavier),
//! quand l'utilisateur l'envoie, ou aussitôt s'il l'a choisi dans les réglages.
//!
//! Ici, ce qui ne dépend pas d'Android : l'état d'une écoute, le texte ajouté à la saisie, les erreurs dites en
//! français, la reconnaissance utilisée (pour les réglages) et la réponse à la demande du micro.

use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

/// Où va le texte dicté.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// Le champ de saisie du terminal suivi (module, identifiant du terminal).
    Term { module: String, id: String },
    /// Le champ d'une saisie sur plusieurs lignes : l'action qui l'a ouverte, et son écran.
    Input { module: String, path: String, act: String },
}

/// Ce que dit la reconnaissance vocale pendant une écoute.
#[derive(Clone, Debug, PartialEq)]
pub enum Heard {
    /// L'écoute est demandée à Android (la question du micro passée) : le micro doit s'ouvrir dans [`OPEN_WITHIN`].
    Opening,
    /// Le micro est ouvert : on peut parler (Android est prêt, ou entend déjà parler).
    Ready,
    /// Le texte provisoire, en entier depuis le début de l'écoute.
    Partial(String),
    /// Le texte final (vide : rien de reconnu) ; l'écoute est finie.
    Final(String),
    /// Une erreur (`SpeechRecognizer.ERROR_*`, ou [`NOT_STARTED`]) ; l'écoute est finie.
    Error(i32),
    /// Le niveau du son, de 0 à 1 (voir [`level`]).
    Level(f32),
}

/// L'écoute n'a pas pu démarrer (une exception de Java) : propre à Eyrie.
pub const NOT_STARTED: i32 = -1;
/// `SpeechRecognizer.ERROR_INSUFFICIENT_PERMISSIONS`, que dit aussi le micro refusé à la demande d'Android.
pub const NO_PERMISSION: i32 = 9;

pub const NOTHING_HEARD: &str = "Rien entendu.";
/// Finie, ou abandonnée (voir [`OPEN_WITHIN`]), sans que le micro se soit jamais dit ouvert : on n'a pas été invité à
/// parler.
pub const NOT_OPENED: &str =
    "Le micro ne s'est pas ouvert : la reconnaissance vocale du téléphone n'a pas répondu à temps. Réessayez.";

/// `SpeechRecognizer.ERROR_SPEECH_TIMEOUT`, `ERROR_NO_MATCH` : rien de reconnu.
const SILENCE: [i32; 2] = [6, 7];
/// `SpeechRecognizer.ERROR_LANGUAGE_NOT_SUPPORTED`, `ERROR_LANGUAGE_UNAVAILABLE` (Android 12 et suivants) : la
/// reconnaissance n'a pas (encore) la langue.
const NO_LANGUAGE: [i32; 2] = [12, 13];

/// Où en est l'écoute.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Phase {
    #[default]
    Idle,
    /// Demandée : le micro s'ouvre (après la question d'Android, la première fois).
    Starting,
    /// Le micro est ouvert.
    Listening,
    /// Finie par l'utilisateur : le texte final arrive.
    Stopping,
}

/// Ce que le toucher du micro demande au téléphone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tap {
    /// Écouter ; l'écoute porte ce numéro (une écoute d'avant est annulée).
    Start(u32),
    /// Finir l'écoute : ce qui a été dit est reconnu, le texte final arrive.
    Stop,
    /// Abandonner l'écoute, le micro pas encore ouvert.
    Cancel,
    /// Rien : la fin est déjà demandée.
    Nothing,
}

/// Ce que devient ce que dit la reconnaissance.
#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    /// D'une écoute finie ou abandonnée : rien.
    Stale,
    /// L'écoute continue : l'interface se met à jour.
    Update,
    /// Le texte final, pour cette cible.
    Text(Target, String),
    /// Finie sans texte : ce qu'il faut dire (une bulle) et son ton.
    Failed(String, i32),
    /// Finie : la reconnaissance n'a pas la langue de la dictée (son erreur) ; celle du téléphone peut peut-être la
    /// télécharger (voir [`download_question`]).
    NoLanguage(i32),
}

/// L'écoute en cours, s'il y en a une.
#[derive(Clone, Debug, Default)]
pub struct Dictation {
    /// Le numéro de la dernière écoute demandée : ce que dit une écoute d'avant est ignoré.
    seq: u32,
    pub phase: Phase,
    pub target: Option<Target>,
    /// Le texte provisoire, montré à part (pas dans la saisie) jusqu'au texte final.
    pub partial: String,
    pub level: f32,
}

impl Dictation {
    pub fn active(&self) -> bool {
        self.phase != Phase::Idle
    }

    /// Le numéro de l'écoute en cours.
    pub fn seq(&self) -> u32 {
        self.seq
    }

    /// Le micro touché pour cette cible : écouter, finir l'écoute (le texte est gardé), ou l'abandonner si le micro ne
    /// s'est pas encore ouvert. Une écoute pour une autre cible est remplacée.
    pub fn tap(&mut self, target: Target) -> Tap {
        if self.target.as_ref() == Some(&target) {
            match self.phase {
                Phase::Starting => {
                    self.cancel();
                    return Tap::Cancel;
                }
                Phase::Listening => {
                    self.phase = Phase::Stopping;
                    return Tap::Stop;
                }
                Phase::Stopping => return Tap::Nothing,
                Phase::Idle => {}
            }
        }
        // 0 ne sert pas : c'est « aucune écoute » pour Android (`android.rs`).
        let seq = self.seq.wrapping_add(1).max(1);
        *self = Dictation { seq, phase: Phase::Starting, target: Some(target), ..Dictation::default() };
        Tap::Start(seq)
    }

    /// Ce que dit la reconnaissance pour l'écoute `seq`.
    pub fn heard(&mut self, seq: u32, heard: Heard) -> Outcome {
        if seq != self.seq || !self.active() {
            return Outcome::Stale;
        }
        // Le micro ne s'est jamais dit ouvert : on n'a pas été invité à parler.
        let unopened = self.phase == Phase::Starting;
        match heard {
            Heard::Opening => {}
            // Prêt, un texte provisoire, du son : le micro est ouvert.
            Heard::Ready => self.opened(),
            Heard::Partial(text) => {
                self.opened();
                self.partial = text;
            }
            Heard::Level(level) => {
                self.opened();
                self.level = level;
            }
            Heard::Final(text) => {
                let target = self.target.take();
                self.cancel();
                let text = text.trim();
                return match target {
                    Some(target) if !text.is_empty() => Outcome::Text(target, text.to_string()),
                    _ if unopened => Outcome::Failed(NOT_OPENED.into(), 3),
                    _ => Outcome::Failed(NOTHING_HEARD.into(), 1),
                };
            }
            Heard::Error(code) => {
                self.cancel();
                if NO_LANGUAGE.contains(&code) {
                    return Outcome::NoLanguage(code);
                }
                if unopened && SILENCE.contains(&code) {
                    return Outcome::Failed(NOT_OPENED.into(), 3);
                }
                let (text, tone) = error_text(code);
                return Outcome::Failed(text, tone);
            }
        }
        Outcome::Update
    }

    fn opened(&mut self) {
        if self.phase == Phase::Starting {
            self.phase = Phase::Listening;
        }
    }

    /// Abandonnée (autre écran, autre terminal, saisie fermée, liaison coupée) : le texte provisoire part avec elle ;
    /// `true` : il y avait une écoute.
    pub fn cancel(&mut self) -> bool {
        let was = self.active();
        *self = Dictation { seq: self.seq, ..Dictation::default() };
        was
    }

    /// L'application passe en arrière-plan : l'écoute s'abandonne, même pas encore ouverte (le micro s'ouvrirait en
    /// arrière-plan), sauf celle qui attend la réponse d'Android sur le micro (`asking` : sa question met l'application
    /// en pause). `true` : abandonnée.
    pub fn paused(&mut self, asking: bool) -> bool {
        !(asking && self.phase == Phase::Starting) && self.cancel()
    }

    /// La fin demandée n'a rien donné à temps (voir [`STOP_WITHIN`]) : abandonnée. `true` : c'était le cas.
    pub fn stop_timed_out(&mut self, seq: u32) -> bool {
        seq == self.seq && self.phase == Phase::Stopping && self.cancel()
    }

    /// Le micro, demandé à Android ([`Heard::Opening`]), ne s'est pas ouvert à temps (voir [`OPEN_WITHIN`]) :
    /// abandonnée. `true` : c'était le cas.
    pub fn open_timed_out(&mut self, seq: u32) -> bool {
        seq == self.seq && self.phase == Phase::Starting && self.cancel()
    }
}

/// Le texte final attendu après « finir », au plus.
pub const STOP_WITHIN: Duration = Duration::from_secs(5);

/// Le micro ouvert après la demande à Android, au plus : la première fois, la reconnaissance du téléphone se lie à son
/// service et charge son modèle (plusieurs secondes) ; au-delà, elle ne répond plus.
pub const OPEN_WITHIN: Duration = Duration::from_secs(15);

/// Le texte dicté, ajouté au bout de la saisie (une espace entre les deux s'il en faut une) : on le relit, on le
/// corrige, puis on l'envoie.
pub fn append(draft: &str, text: &str) -> String {
    let text = text.trim();
    if draft.is_empty() || draft.ends_with(char::is_whitespace) || text.is_empty() {
        format!("{draft}{text}")
    } else {
        format!("{draft} {text}")
    }
}

/// Dans le terminal seulement, et si l'utilisateur l'a choisi : le texte dicté part aussitôt, avec Entrée (jamais
/// d'une saisie, qui a son bouton).
pub fn sends_now(target: &Target, send_pref: bool) -> bool {
    send_pref && matches!(target, Target::Term { .. })
}

/// Le niveau du son d'Android (`onRmsChanged`, en dB, de -2 à 10 environ), de 0 à 1.
pub fn level(db: f32) -> f32 {
    if db.is_nan() { 0.0 } else { ((db + 2.0) / 12.0).clamp(0.0, 1.0) }
}

/// Une erreur d'Android, dite en français, et son ton (1 info, 3 attention, 4 erreur).
pub fn error_text(code: i32) -> (String, i32) {
    let (text, tone) = match code {
        // ERROR_SPEECH_TIMEOUT, ERROR_NO_MATCH.
        6 | 7 => (NOTHING_HEARD, 1),
        NO_PERMISSION => ("Autorisez le micro pour Eyrie dans les réglages d'Android.", 3),
        // ERROR_NETWORK_TIMEOUT, ERROR_NETWORK.
        1 | 2 => ("La reconnaissance vocale de ce téléphone a besoin d'Internet.", 3),
        // ERROR_LANGUAGE_NOT_SUPPORTED, ERROR_LANGUAGE_UNAVAILABLE (Android 12 et suivants).
        12 | 13 => ("Cette langue n'est pas disponible pour la dictée sur ce téléphone.", 3),
        // ERROR_RECOGNIZER_BUSY.
        8 => ("La reconnaissance vocale sert déjà ailleurs : réessayez dans un instant.", 3),
        // ERROR_AUDIO.
        3 => ("Le micro ne donne rien : une autre application s'en sert peut-être.", 3),
        // ERROR_TOO_MANY_REQUESTS.
        10 => ("Trop de dictées d'affilée : réessayez dans un moment.", 3),
        // ERROR_SERVER, ERROR_SERVER_DISCONNECTED.
        4 | 11 => ("La reconnaissance vocale du téléphone a échoué : réessayez.", 4),
        NOT_STARTED => ("La dictée n'a pas pu démarrer sur ce téléphone.", 4),
        // ERROR_CLIENT et les suivantes : rien à y faire, sinon réessayer.
        _ => return (format!("La dictée s'est arrêtée sur une erreur du téléphone ({code}) : réessayez."), 4),
    };
    (text.to_string(), tone)
}

/// La reconnaissance vocale du téléphone.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Recognizer {
    /// Pas de dictée ici (le bureau).
    #[default]
    Absent,
    /// Android, sans service de reconnaissance vocale.
    Missing,
    /// Celle du téléphone lui-même : le son n'en sort pas.
    OnDevice,
    /// Le service par défaut du téléphone (son nom, s'il se lit) : il peut envoyer le son sur Internet.
    Service(String),
}

pub const UNAVAILABLE: &str =
    "La dictée n'est pas disponible sur ce téléphone (aucun service de reconnaissance vocale).";

impl Recognizer {
    pub fn available(&self) -> bool {
        matches!(self, Recognizer::OnDevice | Recognizer::Service(_))
    }

    /// Pour les réglages : 0 rien à dire (pas de section), 1 pas de dictée, 2 laquelle ; la phrase qui le dit.
    pub fn display(&self) -> (i32, String) {
        const NET: &str = "qui peut envoyer le son sur Internet, comme le micro du clavier.";
        match self {
            Recognizer::Absent => (0, String::new()),
            Recognizer::Missing => (1, "Aucune reconnaissance vocale sur ce téléphone : pas de dictée.".into()),
            Recognizer::OnDevice => (2, "Par le téléphone lui-même, sans Internet : le son n'en sort pas.".into()),
            Recognizer::Service(name) if !name.is_empty() => (2, format!("Par « {name} », {NET}")),
            Recognizer::Service(_) => (2, format!("Par le service de reconnaissance du téléphone, {NET}")),
        }
    }
}

/// Le téléchargement de la langue de la dictée par la reconnaissance du téléphone (Android 13 et suivants), quand
/// elle ne l'a pas ([`Outcome::NoLanguage`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Download {
    /// Demandé à Android.
    Asked,
    /// Fini : la dictée a sa langue.
    Done,
    /// Android le fera plus tard (en Wi-Fi…).
    Scheduled,
    /// Échoué (`SpeechRecognizer.ERROR_*` ; [`NOT_STARTED`] : pas même demandé).
    Failed(i32),
}

/// La langue de la dictée (« fr-FR »), dite dans une phrase : « le français », sinon « la langue du téléphone (…) ».
fn language_said(lang: &str) -> String {
    if lang == "fr" || lang.starts_with("fr-") {
        "le français".into()
    } else {
        format!("la langue du téléphone ({lang})")
    }
}

/// La question quand la reconnaissance du téléphone n'a pas la langue de la dictée : son texte, et le libellé du
/// bouton. `maker` : qui fait le modèle téléchargé (« Google »), vide s'il ne se lit pas.
pub fn download_question(lang: &str, maker: &str) -> (String, String) {
    let said = language_said(lang);
    let model = if maker.is_empty() { String::new() } else { format!(" de {maker}") };
    let text = format!(
        "La reconnaissance vocale du téléphone n'a pas encore {said}.\n\nTélécharger le modèle de reconnaissance \
         vocale{model} pour {said} ? Il s'installe sur le téléphone : la dictée se fait ensuite sans Internet, le son \
         n'en sort pas."
    );
    let ok = if said == "le français" { "Télécharger le français" } else { "Télécharger la langue" };
    (text, ok.into())
}

/// Ce que devient le téléchargement, dit en bulle, et son ton.
pub fn download_text(download: &Download, lang: &str) -> (String, i32) {
    let model = format!("le modèle de dictée pour {}", language_said(lang));
    match download {
        Download::Asked => (format!("Téléchargement demandé : {model}…"), 1),
        Download::Done => (format!("C'est fait, {model} est téléchargé : vous pouvez dicter."), 1),
        Download::Scheduled => (
            format!("Android téléchargera {model} plus tard (en Wi-Fi, par exemple) : réessayez la dictée ensuite."),
            1,
        ),
        // ERROR_CANNOT_LISTEN_TO_DOWNLOAD_EVENTS : le téléchargement est demandé, sa fin ne sera pas dite.
        Download::Failed(15) => (
            "Téléchargement demandé : Android ne dit pas quand il sera fini. Réessayez la dictée dans un moment."
                .into(),
            1,
        ),
        Download::Failed(NOT_STARTED) => (format!("Le téléchargement n'a pas pu être demandé à Android ({model})."), 3),
        Download::Failed(code) => (format!("Le téléchargement a échoué ({code}) : {model}."), 3),
    }
}

/// L'écoute demandée à Android (son numéro ; 0 : aucune). Le fil qui ouvre le micro vérifie qu'elle l'est toujours et
/// poste l'ouverture d'un tenant : une annulation ne passe pas entre les deux, elle est postée après l'ouverture qu'elle
/// annule (le fil principal d'Android prend ce qu'on lui poste dans l'ordre), et la ferme.
#[derive(Default)]
pub struct Requested(Mutex<u32>);

impl Requested {
    pub const fn new() -> Self {
        Requested(Mutex::new(0))
    }

    fn lock(&self) -> MutexGuard<'_, u32> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// L'écoute `seq` est demandée ; une d'avant ne l'est plus.
    pub fn set(&self, seq: u32) {
        *self.lock() = seq;
    }

    pub fn is(&self, seq: u32) -> bool {
        *self.lock() == seq
    }

    /// `open` (poster l'ouverture à Android) si l'écoute `seq` est toujours demandée ; `None` : elle ne l'est plus.
    pub fn open_if<R>(&self, seq: u32, open: impl FnOnce() -> R) -> Option<R> {
        let current = self.lock();
        // Le verrou est tenu pendant `open` : `clear` attend qu'elle soit postée.
        (*current == seq).then(open)
    }

    /// Plus d'écoute demandée, puis `cancel` (poster l'annulation), après une ouverture en cours.
    pub fn clear(&self, cancel: impl FnOnce()) {
        let mut current = self.lock();
        *current = 0;
        cancel();
    }
}

/// La réponse d'Android à la demande du micro, sans `onRequestPermissionsResult` (NativeActivity ne le transmet pas) :
/// relevée régulièrement, la permission et le premier plan.
#[derive(Clone, Debug, Default)]
pub struct Asking {
    /// La question d'Android a mis l'application en pause.
    paused: bool,
    /// Revenue au premier plan à cet instant, sans la permission.
    back: Option<Duration>,
}

/// Le temps laissé à la question d'Android pour paraître : sans elle, le micro est refusé pour de bon (« Ne plus
/// demander »), Android ne demande plus.
pub const ASK_SHOWS_WITHIN: Duration = Duration::from_secs(3);
/// Revenue au premier plan sans la permission depuis ce temps : refusée.
pub const ASK_SETTLES: Duration = Duration::from_millis(600);
/// Sans réponse passé ce temps, la demande est abandonnée.
pub const ASK_GIVES_UP: Duration = Duration::from_secs(90);

impl Asking {
    /// `at` : le temps depuis la demande. `Some(true)` : accordée, et l'application revenue au premier plan (le micro
    /// ne s'ouvre pas en arrière-plan) ; `Some(false)` : refusée, ou pas de retour à temps ; `None` : pas encore.
    pub fn answer(&mut self, granted: bool, foreground: bool, at: Duration) -> Option<bool> {
        if granted && foreground {
            return Some(true);
        }
        if at > ASK_GIVES_UP {
            return Some(false);
        }
        if !foreground {
            self.paused = true;
            self.back = None;
            return None;
        }
        if !self.paused {
            return (at > ASK_SHOWS_WITHIN).then_some(false);
        }
        let back = *self.back.get_or_insert(at);
        (at.saturating_sub(back) > ASK_SETTLES).then_some(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn term() -> Target {
        Target::Term { module: "fawkes".into(), id: "3".into() }
    }

    fn input() -> Target {
        Target::Input { module: "patronus".into(), path: String::new(), act: "lancer-note:revue".into() }
    }

    /// Un toucher écoute, le texte provisoire se montre, un second toucher finit l'écoute et garde ce qui a été dit.
    #[test]
    fn a_tap_listens_and_a_second_one_keeps_what_was_said() {
        let mut d = Dictation::default();
        assert!(!d.active());
        let Tap::Start(seq) = d.tap(term()) else { panic!("écoute") };
        assert_eq!(d.phase, Phase::Starting);
        assert_eq!(d.heard(seq, Heard::Ready), Outcome::Update);
        assert_eq!(d.phase, Phase::Listening);
        assert_eq!(d.heard(seq, Heard::Partial("ajoute un".into())), Outcome::Update);
        assert_eq!(d.heard(seq, Heard::Level(0.5)), Outcome::Update);
        assert_eq!((d.partial.as_str(), d.level), ("ajoute un", 0.5));
        assert_eq!(d.tap(term()), Tap::Stop);
        assert_eq!(d.phase, Phase::Stopping);
        assert_eq!(d.tap(term()), Tap::Nothing, "la fin est déjà demandée");
        let done = d.heard(seq, Heard::Final("  ajoute un test  ".into()));
        assert_eq!(done, Outcome::Text(term(), "ajoute un test".into()));
        assert!(!d.active());
        assert_eq!((d.partial.as_str(), d.target.clone()), ("", None));
    }

    /// Android finit aussi l'écoute de lui-même (un silence) ; rien de reconnu, ou une erreur : une bulle.
    #[test]
    fn an_end_without_text_is_told() {
        let mut d = Dictation::default();
        let Tap::Start(seq) = d.tap(term()) else { panic!("écoute") };
        d.heard(seq, Heard::Ready);
        assert_eq!(d.heard(seq, Heard::Final(" ".into())), Outcome::Failed(NOTHING_HEARD.into(), 1));
        let Tap::Start(seq) = d.tap(input()) else { panic!("écoute") };
        let (text, tone) = error_text(NO_PERMISSION);
        assert_eq!(d.heard(seq, Heard::Error(NO_PERMISSION)), Outcome::Failed(text, tone));
        assert!(!d.active());
    }

    /// Ce que dit une écoute abandonnée ou remplacée ne compte plus (Android peut encore parler d'elle).
    #[test]
    fn what_an_old_listening_says_is_ignored() {
        let mut d = Dictation::default();
        let Tap::Start(first) = d.tap(term()) else { panic!("écoute") };
        d.heard(first, Heard::Ready);
        assert!(d.cancel());
        assert!(!d.cancel(), "plus rien à abandonner");
        assert_eq!(d.heard(first, Heard::Final("trop tard".into())), Outcome::Stale);
        // Une autre cible remplace l'écoute en cours.
        let Tap::Start(second) = d.tap(term()) else { panic!("écoute") };
        let Tap::Start(third) = d.tap(input()) else { panic!("une autre cible : une nouvelle écoute") };
        assert!(second != first && third != second);
        assert_eq!(d.heard(second, Heard::Partial("ancien".into())), Outcome::Stale);
        assert_eq!(d.partial, "");
        assert_eq!(d.target, Some(input()));
    }

    /// Un second toucher avant que le micro s'ouvre abandonne : il n'y a encore rien à garder.
    #[test]
    fn a_second_tap_before_the_mic_opens_gives_up() {
        let mut d = Dictation::default();
        let Tap::Start(seq) = d.tap(term()) else { panic!("écoute") };
        assert_eq!(d.tap(term()), Tap::Cancel);
        assert!(!d.active());
        assert_eq!(d.heard(seq, Heard::Ready), Outcome::Stale);
        // Un texte provisoire avant `Ready` (un service pressé) : le micro était bien ouvert.
        let Tap::Start(seq) = d.tap(term()) else { panic!("écoute") };
        d.heard(seq, Heard::Partial("bonjour".into()));
        assert_eq!(d.phase, Phase::Listening);
        assert_eq!(d.tap(term()), Tap::Stop);
    }

    /// En arrière-plan, une écoute s'abandonne, même pas encore ouverte ; pas celle qui attend la réponse d'Android sur
    /// le micro (`asking` : sa question met l'application en pause).
    #[test]
    fn going_to_the_background_drops_the_mic_but_not_the_permission_question() {
        let mut d = Dictation::default();
        let Tap::Start(seq) = d.tap(term()) else { panic!("écoute") };
        assert!(!d.paused(true), "la question d'Android met l'application en pause");
        assert_eq!(d.phase, Phase::Starting);
        d.heard(seq, Heard::Ready);
        d.heard(seq, Heard::Partial("ajoute".into()));
        assert!(d.paused(false));
        assert_eq!((d.phase, d.partial.as_str()), (Phase::Idle, ""));
        let Tap::Start(seq) = d.tap(term()) else { panic!("écoute") };
        d.heard(seq, Heard::Ready);
        d.tap(term());
        assert!(d.paused(true), "la fin demandée aussi");
        assert!(!d.paused(false));
        d.tap(term());
        assert!(d.paused(false), "pas encore ouverte, sans question : abandonnée (sinon ouverte en arrière-plan)");
        assert!(!d.active());
    }

    /// La fin demandée sans texte final à temps : abandonnée ; pas une écoute suivante.
    #[test]
    fn a_stop_that_never_ends_is_given_up() {
        let mut d = Dictation::default();
        let Tap::Start(seq) = d.tap(term()) else { panic!("écoute") };
        d.heard(seq, Heard::Ready);
        assert!(!d.stop_timed_out(seq), "pas de fin demandée");
        d.tap(term());
        assert!(!d.stop_timed_out(seq + 1));
        assert!(d.stop_timed_out(seq));
        assert!(!d.active());
        let Tap::Start(next) = d.tap(term()) else { panic!("écoute") };
        d.heard(next, Heard::Ready);
        d.tap(term());
        assert!(!d.stop_timed_out(seq), "le délai d'une écoute d'avant");
        assert!(d.active());
    }

    /// Android ne dit pas toujours « prêt » (`onReadyForSpeech`) : le son qui arrive dit aussi que le micro est ouvert
    /// (le début de la parole passe par `Ready`, voir `Dictee.java`).
    #[test]
    fn the_sound_level_also_says_the_mic_is_open() {
        let mut d = Dictation::default();
        let Tap::Start(seq) = d.tap(term()) else { panic!("écoute") };
        assert_eq!(d.heard(seq, Heard::Level(0.3)), Outcome::Update);
        assert_eq!((d.phase, d.level), (Phase::Listening, 0.3));
        assert_eq!(d.tap(term()), Tap::Stop, "ouvert : un second toucher garde ce qui a été dit");
    }

    /// Le micro demandé à Android (la question du micro passée) qui ne s'ouvre pas à temps : l'écoute est abandonnée ;
    /// pas une écoute ouverte, ni une écoute suivante. Le délai laisse au premier modèle le temps de se charger.
    #[test]
    fn a_mic_that_never_opens_is_given_up() {
        let mut d = Dictation::default();
        let Tap::Start(seq) = d.tap(term()) else { panic!("écoute") };
        assert_eq!(d.heard(seq, Heard::Opening), Outcome::Update);
        assert_eq!(d.phase, Phase::Starting, "demandée, pas encore ouverte");
        assert!(!d.open_timed_out(seq + 1));
        assert!(d.open_timed_out(seq));
        assert!(!d.active());
        let Tap::Start(next) = d.tap(term()) else { panic!("écoute") };
        assert!(!d.open_timed_out(seq), "le délai d'une écoute d'avant");
        d.heard(next, Heard::Ready);
        assert!(!d.open_timed_out(next), "ouvert à temps");
        assert!(d.active());
        assert!(OPEN_WITHIN >= Duration::from_secs(10), "le modèle de la reconnaissance du téléphone se charge");
    }

    /// Finie sans rien avant que le micro se soit dit ouvert : « Rien entendu. » laisserait croire qu'on a parlé trop
    /// bas ; on n'a jamais été invité à parler.
    #[test]
    fn an_end_before_the_mic_opened_says_so() {
        let mut d = Dictation::default();
        for heard in [Heard::Error(6), Heard::Error(7), Heard::Final(String::new())] {
            let Tap::Start(seq) = d.tap(term()) else { panic!("écoute") };
            assert_eq!(d.heard(seq, heard), Outcome::Failed(NOT_OPENED.into(), 3));
        }
        let Tap::Start(seq) = d.tap(term()) else { panic!("écoute") };
        assert_eq!(d.heard(seq, Heard::Final("bonjour".into())), Outcome::Text(term(), "bonjour".into()));
        let Tap::Start(seq) = d.tap(term()) else { panic!("écoute") };
        d.heard(seq, Heard::Ready);
        assert_eq!(d.heard(seq, Heard::Error(6)), Outcome::Failed(NOTHING_HEARD.into(), 1), "ouvert : rien entendu");
    }

    /// La reconnaissance sans la langue de la dictée (pas encore téléchargée) : pas une simple bulle, le contrôleur
    /// peut proposer son téléchargement.
    #[test]
    fn a_missing_language_is_told_apart() {
        let mut d = Dictation::default();
        for code in [12, 13] {
            let Tap::Start(seq) = d.tap(term()) else { panic!("écoute") };
            assert_eq!(d.heard(seq, Heard::Error(code)), Outcome::NoLanguage(code));
            assert!(!d.active());
        }
    }

    /// La question du téléchargement dit ce qui est téléchargé et par qui (le modèle de Google, qui reste sur le
    /// téléphone) ; le bouton nomme le français.
    #[test]
    fn the_download_question_names_the_language_and_who_makes_the_model() {
        let (text, ok) = download_question("fr-FR", "Google");
        assert_eq!(ok, "Télécharger le français");
        assert!(text.contains("pour le français") && text.contains("de Google"), "{text}");
        assert!(text.contains("sans Internet") && text.contains("le son n'en sort pas"), "{text}");
        let (text, ok) = download_question("de-DE", "");
        assert_eq!(ok, "Télécharger la langue");
        assert!(text.contains("(de-DE)") && !text.contains("français") && !text.contains("Google"), "{text}");
    }

    /// Ce que devient le téléchargement, dit en bulle.
    #[test]
    fn the_download_outcome_is_told() {
        let (text, tone) = download_text(&Download::Asked, "fr-FR");
        assert!(text.contains("pour le français") && tone == 1, "{text}");
        let (text, tone) = download_text(&Download::Done, "fr-FR");
        assert!(text.contains("est téléchargé") && text.contains("dicter") && tone == 1, "{text}");
        let (text, tone) = download_text(&Download::Scheduled, "fr-FR");
        assert!(text.contains("plus tard") && tone == 1, "{text}");
        let (text, tone) = download_text(&Download::Failed(4), "fr-FR");
        assert!(text.contains("(4)") && tone == 3, "{text}");
        // ERROR_CANNOT_LISTEN_TO_DOWNLOAD_EVENTS : demandé, mais Android ne dira pas quand c'est fini.
        let (text, tone) = download_text(&Download::Failed(15), "fr-FR");
        assert!(text.contains("demandé") && tone == 1, "{text}");
        let (text, tone) = download_text(&Download::Failed(NOT_STARTED), "fr-FR");
        assert!(text.contains("pas pu") && tone == 3, "{text}");
    }

    #[test]
    fn dictated_text_goes_at_the_end_of_the_draft() {
        assert_eq!(append("", "ajoute un test"), "ajoute un test");
        assert_eq!(append("relis", "le diff"), "relis le diff");
        assert_eq!(append("relis ", "le diff"), "relis le diff");
        assert_eq!(append("relis\n", "le diff"), "relis\nle diff");
        assert_eq!(append("relis", "  le diff \n"), "relis le diff");
        assert_eq!(append("relis", " "), "relis");
    }

    #[test]
    fn only_the_terminal_sends_at_once_and_only_when_chosen() {
        assert!(sends_now(&term(), true));
        assert!(!sends_now(&term(), false));
        assert!(!sends_now(&input(), true), "une saisie a son bouton");
    }

    #[test]
    fn the_sound_level_is_between_zero_and_one() {
        assert_eq!(level(-2.0), 0.0);
        assert_eq!(level(-40.0), 0.0);
        assert_eq!(level(10.0), 1.0);
        assert_eq!(level(4.0), 0.5);
        assert_eq!(level(f32::NAN), 0.0);
    }

    #[test]
    fn android_errors_are_told_in_french() {
        assert_eq!(error_text(7), (NOTHING_HEARD.to_string(), 1));
        assert_eq!(error_text(6).0, NOTHING_HEARD);
        assert_eq!(error_text(9).0, "Autorisez le micro pour Eyrie dans les réglages d'Android.");
        for network in [1, 2] {
            assert_eq!(error_text(network).0, "La reconnaissance vocale de ce téléphone a besoin d'Internet.");
        }
        for language in [12, 13] {
            assert_eq!(error_text(language).0, "Cette langue n'est pas disponible pour la dictée sur ce téléphone.");
        }
        for code in [3, 4, 5, 8, 10, 11, 14, NOT_STARTED, 99] {
            let (text, tone) = error_text(code);
            assert!(!text.is_empty() && (3..=4).contains(&tone), "{code} : {text}");
        }
        assert!(error_text(99).0.contains("99"), "une erreur inconnue donne son numéro");
    }

    /// Les réglages disent quelle reconnaissance sert, et si le son peut passer par Internet.
    #[test]
    fn the_settings_tell_which_recognition_is_used() {
        assert!(!Recognizer::Absent.available() && !Recognizer::Missing.available());
        assert!(Recognizer::OnDevice.available() && Recognizer::Service(String::new()).available());
        assert_eq!(Recognizer::Absent.display().0, 0);
        assert_eq!(Recognizer::Missing.display().0, 1);
        let (kind, text) = Recognizer::OnDevice.display();
        assert!(kind == 2 && text.contains("sans Internet"), "{text}");
        let (kind, text) = Recognizer::Service("Google".into()).display();
        assert!(kind == 2 && text.contains("« Google »") && text.contains("Internet"), "{text}");
        let (_, text) = Recognizer::Service(String::new()).display();
        assert!(text.contains("Internet") && !text.contains("«"), "{text}");
    }

    /// Une annulation pendant que le fil de la dictée ouvre le micro est postée après l'ouverture, jamais avant : la
    /// reconnaissance qu'elle doit fermer est bien là quand elle passe.
    #[test]
    fn a_cancel_is_never_posted_before_the_opening_it_cancels() {
        use std::sync::{Arc, mpsc};
        let requested = Arc::new(Requested::new());
        let posted = Arc::new(Mutex::new(Vec::new()));
        requested.set(4);
        let (checked, wait) = mpsc::channel();
        let opener = {
            let (requested, posted) = (requested.clone(), posted.clone());
            std::thread::spawn(move || {
                requested.open_if(4, || {
                    checked.send(()).unwrap();
                    // Le temps d'attacher le fil à Java, avant de poster.
                    std::thread::sleep(Duration::from_millis(100));
                    posted.lock().unwrap().push("ecouter");
                })
            })
        };
        wait.recv().unwrap();
        requested.clear(|| posted.lock().unwrap().push("annuler"));
        assert_eq!(opener.join().unwrap(), Some(()));
        assert_eq!(*posted.lock().unwrap(), ["ecouter", "annuler"]);
        assert!(!requested.is(4));
        assert_eq!(requested.open_if(4, || ()), None, "annulée : le micro ne s'ouvre plus");
        requested.set(5);
        assert_eq!(requested.open_if(4, || ()), None, "remplacée");
        assert_eq!(requested.open_if(5, || 7), Some(7));
    }

    /// La question d'Android met l'application en pause : la permission accordée, ou l'application revenue sans elle.
    #[test]
    fn the_answer_to_the_mic_question_is_read_without_android_telling_it() {
        let ms = Duration::from_millis;
        let mut a = Asking::default();
        assert_eq!(a.answer(false, true, ms(100)), None);
        assert_eq!(a.answer(false, false, ms(400)), None, "la question est posée");
        assert_eq!(a.answer(false, false, ms(20_000)), None, "on y réfléchit");
        // Accordée, mais l'application n'est pas encore revenue : le micro ne s'ouvre pas en arrière-plan (ce qu'il
        // dirait serait abandonné aussitôt).
        assert_eq!(a.answer(true, false, ms(20_300)), None, "accordée, pas encore revenue");
        assert_eq!(a.answer(true, true, ms(20_500)), Some(true));
        let mut a = Asking::default();
        a.answer(false, false, ms(300));
        assert_eq!(a.answer(true, false, ASK_GIVES_UP + ms(1)), Some(false), "jamais revenue");
        // Refusée : l'application revient sans la permission.
        let mut a = Asking::default();
        a.answer(false, false, ms(300));
        assert_eq!(a.answer(false, true, ms(5_000)), None, "le temps que la permission se lise");
        assert_eq!(a.answer(false, true, ms(5_700)), Some(false));
        // Refusée pour de bon : la question ne vient pas.
        let mut a = Asking::default();
        assert_eq!(a.answer(false, true, ms(2_000)), None);
        assert_eq!(a.answer(false, true, ASK_SHOWS_WITHIN + ms(1)), Some(false));
        // Laissée sans réponse.
        let mut a = Asking::default();
        a.answer(false, false, ms(300));
        assert_eq!(a.answer(false, false, ASK_GIVES_UP + ms(1)), Some(false));
    }
}
