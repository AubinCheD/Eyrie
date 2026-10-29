//! Ce que le téléphone envoie au terminal suivi : les touches de la barre (Ctrl ou Alt « collé » s'applique à la
//! touche suivante), un caractère tapé pendant que l'un d'eux est collé, la saisie du champ (sur plusieurs lignes, elle
//! part en collage) et le texte collé (demandé d'abord s'il a plusieurs lignes ou des caractères de contrôle).
//!
//! Tout passe par le module du terminal sur le PC (Fawkes), qui l'encode comme une frappe ou un collage.

use suite_mobile::term::{Key, TermInput};

/// Ctrl ou Alt collé : il s'applique à la touche suivante, puis se relâche. L'un remplace l'autre.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Latch {
    #[default]
    None,
    Ctrl,
    Alt,
}

impl Latch {
    /// Le code de l'interface : 0 rien, 1 Ctrl, 2 Alt.
    pub fn code(self) -> i32 {
        match self {
            Latch::None => 0,
            Latch::Ctrl => 1,
            Latch::Alt => 2,
        }
    }

    pub fn from_code(code: i32) -> Latch {
        match code {
            1 => Latch::Ctrl,
            2 => Latch::Alt,
            _ => Latch::None,
        }
    }

    /// Toucher Ctrl ou Alt : il se colle, ou se relâche s'il l'était déjà.
    pub fn toggle(self, which: Latch) -> Latch {
        if self == which { Latch::None } else { which }
    }
}

/// Échap, le préfixe d'Alt : Alt+x part en un seul envoi (Échap puis x, comme le terminal du PC l'écrit). En deux
/// envois, le programme lirait Échap seul, qui interrompt Claude.
const ESC: char = '\u{1b}';
/// Effacer, tel que le terminal l'écrit (Alt+Effacer : effacer le mot d'avant).
const DEL: char = '\u{7f}';

/// La touche nommée d'une pastille (`escape`, `ctrl-c`…), avec ce qui est collé ; `None` pour un nom inconnu.
///
/// Ctrl ne va qu'aux lettres (le protocole n'a que Ctrl+lettre) : Ctrl+Effacer devient Ctrl+H, comme sur le PC ;
/// Alt ne va qu'à Effacer. Avec les autres touches, ce qui est collé est laissé : la touche part seule.
pub fn chip(name: &str, latch: Latch) -> Option<Vec<TermInput>> {
    let key = match name {
        "escape" => Key::Escape,
        "tab" => Key::Tab,
        "backtab" => Key::BackTab,
        "up" => Key::Up,
        "down" => Key::Down,
        "left" => Key::Left,
        "right" => Key::Right,
        "enter" => Key::Enter,
        "shift-enter" => Key::ShiftEnter,
        "backspace" => Key::Backspace,
        "delete" => Key::Delete,
        "home" => Key::Home,
        "end" => Key::End,
        "pageup" => Key::PageUp,
        "pagedown" => Key::PageDown,
        k => match k.strip_prefix("ctrl-").map(|c| c.chars().collect::<Vec<_>>()).as_deref() {
            Some(&[c]) if c.is_ascii_alphabetic() => Key::Ctrl(c.to_ascii_lowercase()),
            _ => return None,
        },
    };
    let input = match (latch, key) {
        (Latch::Ctrl, Key::Backspace) => TermInput::Key(Key::Ctrl('h')),
        (Latch::Alt, Key::Backspace) => TermInput::Text(format!("{ESC}{DEL}")),
        _ => TermInput::Key(key),
    };
    Some(vec![input])
}

/// Ce qu'un caractère tapé devient avec ce qui est collé.
#[derive(Debug, PartialEq)]
pub enum Typed {
    Send(Vec<TermInput>),
    /// Rien ne part : ce qui est collé ne va pas avec ce caractère (le texte dit pourquoi).
    Refused(&'static str),
    /// Rien n'était collé (relâché entre-temps) : le texte va dans le champ de saisie, pas au terminal.
    Draft(String),
}

/// Le texte tapé pendant que Ctrl ou Alt est collé : son premier caractère seulement (« la touche suivante »).
pub fn typed(text: &str, latch: Latch) -> Typed {
    let Some(c) = text.chars().next() else { return Typed::Send(Vec::new()) };
    match latch {
        Latch::Ctrl if c.is_ascii_alphabetic() => Typed::Send(vec![TermInput::Key(Key::Ctrl(c.to_ascii_lowercase()))]),
        Latch::Ctrl => Typed::Refused("Ctrl va avec une lettre : rien n'est parti."),
        // Entrée, tabulation… : le PC retire les retours à la ligne d'un texte, il ne resterait qu'Échap.
        Latch::Alt if c.is_control() => Typed::Refused("Alt va avec une lettre ou un signe : rien n'est parti."),
        Latch::Alt => Typed::Send(vec![TermInput::Text(format!("{ESC}{c}"))]),
        Latch::None => Typed::Draft(text.replace(['\r', '\n'], "")),
    }
}

/// Un caractère de contrôle qu'un shell peut lire comme une touche (Échap, Ctrl+O…, Effacer) : tous, sauf la
/// tabulation et les retours à la ligne.
fn is_key_control(c: char) -> bool {
    c.is_control() && !matches!(c, '\t' | '\r' | '\n')
}

/// La saisie du champ, envoyée avec Entrée. Sur plusieurs lignes, elle part en collage : le programme la reçoit d'un
/// bloc (encadrée s'il le demande), puis Entrée la valide. Les retours à la ligne de la fin ne comptent pas.
///
/// Les caractères de contrôle qu'un collage dans le champ y a mis sont retirés : le champ ne les montre pas, et un shell
/// les lirait comme des touches (Échap suivi de `[13;28;13;1;0;1_` est Entrée pour la console Windows).
pub fn line(text: &str) -> Vec<TermInput> {
    let text: String = text.chars().filter(|&c| !is_key_control(c)).collect();
    let text = text.trim_end_matches(['\r', '\n']);
    let enter = TermInput::Key(Key::Enter);
    if text.is_empty() {
        vec![enter]
    } else if text.contains(['\r', '\n']) {
        vec![TermInput::Paste(text.to_string()), enter]
    } else {
        vec![TermInput::Text(text.to_string()), enter]
    }
}

/// Le texte du presse-papiers, à coller dans le terminal.
#[derive(Debug, PartialEq)]
pub enum Paste {
    Empty,
    /// Une ligne sans retour ni caractère de contrôle : elle part tout de suite.
    Now(String),
    /// Des retours à la ligne, ou des caractères de contrôle : un shell sans collage encadré exécuterait chaque ligne,
    /// ou lirait Échap et la suite comme des touches, Entrée comprise. On demande d'abord.
    Ask {
        text: String,
        question: String,
    },
}

/// Les lignes montrées dans la question, et leur longueur au plus.
const PREVIEW_LINES: usize = 4;
const PREVIEW_CHARS: usize = 48;

/// Une ligne telle que la question la montre : ses caractères de contrôle nommés (‹Échap›, ‹Ctrl+O›, ‹Effacer›), que le
/// texte ne dessinerait pas (les symboles d'Unicode, ␛…, manquent aux polices courantes).
fn shown(line: &str) -> String {
    let mut s = String::with_capacity(line.len());
    for c in line.chars() {
        match c {
            '\u{1b}' => s.push_str("‹Échap›"),
            '\r' => s.push_str("‹Entrée›"),
            '\u{7f}' => s.push_str("‹Effacer›"),
            '\t' => s.push(c),
            '\u{0}'..='\u{1f}' => s.push_str(&format!("‹Ctrl+{}›", char::from(c as u8 + 0x40))),
            c if c.is_control() => s.push_str(&format!("‹U+{:04X}›", c as u32)),
            c => s.push(c),
        }
    }
    s
}

pub fn paste(clip: &str) -> Paste {
    if clip.is_empty() {
        return Paste::Empty;
    }
    let controls = clip.chars().any(is_key_control);
    if !controls && !clip.contains(['\r', '\n']) {
        return Paste::Now(clip.to_string());
    }
    let lines: Vec<&str> = clip.trim_end_matches(['\r', '\n']).lines().collect();
    let head = match lines.len() {
        _ if controls => {
            "Ce texte contient des caractères de contrôle (Échap…) qu'un shell peut lire comme des touches, \
             Entrée comprise. Le coller ?"
                .to_string()
        }
        0 | 1 => {
            "Coller cette ligne et son retour à la ligne dans le terminal ? Un shell peut l'exécuter tout de suite."
                .to_string()
        }
        n => format!("Coller {n} lignes dans le terminal ? Un shell peut les exécuter tout de suite."),
    };
    let mut preview: Vec<String> = lines
        .iter()
        .take(PREVIEW_LINES)
        .map(|l| {
            let l = shown(l);
            let l = l.trim_end();
            if l.chars().count() > PREVIEW_CHARS {
                format!("{}…", l.chars().take(PREVIEW_CHARS - 1).collect::<String>())
            } else {
                l.to_string()
            }
        })
        .collect();
    if lines.len() > PREVIEW_LINES {
        preview.push(format!("… et {} de plus", lines.len() - PREVIEW_LINES));
    }
    Paste::Ask { text: clip.to_string(), question: format!("{head}\n\n{}", preview.join("\n")) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(k: Key) -> Vec<TermInput> {
        vec![TermInput::Key(k)]
    }

    #[test]
    fn chips_send_their_key_and_take_what_is_latched_when_they_can() {
        assert_eq!(chip("escape", Latch::None), Some(key(Key::Escape)));
        assert_eq!(chip("ctrl-c", Latch::None), Some(key(Key::Ctrl('c'))));
        assert_eq!(chip("ctrl-", Latch::None), None);
        assert_eq!(chip("ctrl-cd", Latch::None), None);
        assert_eq!(chip("inconnue", Latch::Ctrl), None);
        // Ctrl+Effacer : Ctrl+H, l'effacement d'un mot ; Alt+Effacer : Échap et Effacer d'un seul envoi.
        assert_eq!(chip("backspace", Latch::Ctrl), Some(key(Key::Ctrl('h'))));
        assert_eq!(chip("backspace", Latch::Alt), Some(vec![TermInput::Text("\u{1b}\u{7f}".into())]));
        // Ce que le protocole ne sait pas dire : la touche part seule (le collé est consommé par le contrôleur).
        assert_eq!(chip("up", Latch::Ctrl), Some(key(Key::Up)));
        assert_eq!(chip("enter", Latch::Alt), Some(key(Key::Enter)));
        assert_eq!(chip("ctrl-d", Latch::Ctrl), Some(key(Key::Ctrl('d'))));
    }

    #[test]
    fn a_latched_modifier_goes_with_the_next_typed_character() {
        assert_eq!(typed("c", Latch::Ctrl), Typed::Send(key(Key::Ctrl('c'))));
        // Le clavier du téléphone met parfois une majuscule : Ctrl+C reste Ctrl+C.
        assert_eq!(typed("R", Latch::Ctrl), Typed::Send(key(Key::Ctrl('r'))));
        assert_eq!(typed("ab", Latch::Ctrl), Typed::Send(key(Key::Ctrl('a'))), "la touche suivante, une seule");
        assert!(matches!(typed("1", Latch::Ctrl), Typed::Refused(_)));
        // Alt : Échap devant, dans le même envoi.
        assert_eq!(typed("b", Latch::Alt), Typed::Send(vec![TermInput::Text("\u{1b}b".into())]));
        assert_eq!(typed(".", Latch::Alt), Typed::Send(vec![TermInput::Text("\u{1b}.".into())]));
        assert!(matches!(typed("\n", Latch::Alt), Typed::Refused(_)), "Échap seul interromprait Claude");
        assert_eq!(typed("", Latch::Alt), Typed::Send(Vec::new()));
        assert_eq!(typed("ls\n", Latch::None), Typed::Draft("ls".into()), "rien de collé : rien ne part");
    }

    #[test]
    fn toggling_a_latch_replaces_or_releases_it() {
        assert_eq!(Latch::None.toggle(Latch::Ctrl), Latch::Ctrl);
        assert_eq!(Latch::Ctrl.toggle(Latch::Ctrl), Latch::None);
        assert_eq!(Latch::Ctrl.toggle(Latch::Alt), Latch::Alt);
        for l in [Latch::None, Latch::Ctrl, Latch::Alt] {
            assert_eq!(Latch::from_code(l.code()), l);
        }
    }

    #[test]
    fn a_line_on_several_lines_is_sent_as_a_paste() {
        let enter = TermInput::Key(Key::Enter);
        assert_eq!(line("git status"), vec![TermInput::Text("git status".into()), enter.clone()]);
        assert_eq!(line(""), vec![enter.clone()]);
        assert_eq!(line("ls\n"), vec![TermInput::Text("ls".into()), enter.clone()], "le retour final ne compte pas");
        assert_eq!(
            line("Relis le diff\net propose un titre\n"),
            vec![TermInput::Paste("Relis le diff\net propose un titre".into()), enter.clone()]
        );
        assert_eq!(line("a\r\nb"), vec![TermInput::Paste("a\r\nb".into()), enter]);
    }

    #[test]
    fn a_line_loses_its_control_characters_but_its_tabs() {
        let enter = TermInput::Key(Key::Enter);
        // Collé dans le champ : Échap et la suite que ConPTY lit comme Entrée ne partent pas.
        assert_eq!(line("a\u{1b}[13;28;13;1;0;1_b"), vec![TermInput::Text("a[13;28;13;1;0;1_b".into()), enter.clone()]);
        assert_eq!(line("a\u{f}b\u{7f}\u{9b}"), vec![TermInput::Text("ab".into()), enter.clone()]);
        assert_eq!(line("a\tb"), vec![TermInput::Text("a\tb".into()), enter.clone()]);
        assert_eq!(line("a\u{1b}\nb"), vec![TermInput::Paste("a\nb".into()), enter.clone()]);
        assert_eq!(line("\u{1b}"), vec![enter], "il ne reste rien : Entrée seule");
    }

    #[test]
    fn pasting_control_characters_asks_first_and_shows_them() {
        // Sur une ligne, sans retour : Échap ou Ctrl+O suffisent à un shell pour exécuter ce qui précède.
        let clip = "ls\u{1b}[13;28;13;1;0;1_whoami";
        let Paste::Ask { text, question } = paste(clip) else { panic!("demande attendue") };
        assert_eq!(text, clip, "confirmé, le collage part tel quel");
        assert!(question.starts_with("Ce texte contient des caractères de contrôle"), "{question}");
        assert!(question.ends_with("ls‹Échap›[13;28;13;1;0;1_whoami"), "{question}");
        let Paste::Ask { question, .. } = paste("a\u{f}b\u{7f}c\u{9b}d\u{0}e\rf") else { panic!("demande attendue") };
        assert!(question.ends_with("a‹Ctrl+O›b‹Effacer›c‹U+009B›d‹Ctrl+@›e‹Entrée›f"), "{question}");
        // Avec des retours aussi : c'est la question des caractères de contrôle, sur chaque ligne.
        let Paste::Ask { question, .. } = paste("git status\u{1b}\nls\n") else { panic!("demande attendue") };
        assert!(question.starts_with("Ce texte contient des caractères de contrôle"), "{question}");
        assert!(question.ends_with("git status‹Échap›\nls"), "{question}");
        // Une tabulation n'est pas une touche à craindre.
        assert_eq!(paste("a\tb"), Paste::Now("a\tb".into()));
    }

    #[test]
    fn pasting_several_lines_asks_first() {
        assert_eq!(paste(""), Paste::Empty);
        assert_eq!(paste("cargo test"), Paste::Now("cargo test".into()));
        let Paste::Ask { text, question } = paste("git fetch\ngit status\n") else { panic!("demande attendue") };
        assert_eq!(text, "git fetch\ngit status\n", "le texte part tel quel ; le PC normalise les retours");
        assert!(question.starts_with("Coller 2 lignes dans le terminal ?"), "{question}");
        assert!(question.ends_with("git fetch\ngit status"), "{question}");
        let Paste::Ask { question, .. } = paste("rm -rf build\n") else { panic!("demande attendue") };
        assert!(question.starts_with("Coller cette ligne et son retour"), "{question}");
        let long = (1..=7).map(|i| format!("ligne {i} {}", "x".repeat(60))).collect::<Vec<_>>().join("\n");
        let Paste::Ask { question, .. } = paste(&long) else { panic!("demande attendue") };
        assert!(question.contains("Coller 7 lignes"), "{question}");
        assert!(question.ends_with("… et 3 de plus"), "{question}");
        assert!(question.lines().all(|l| l.chars().count() <= 110), "{question}");
        assert!(question.contains(&format!("ligne 1 {}…", "x".repeat(39))), "{question}");
    }
}
