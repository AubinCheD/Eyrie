//! Le sélecteur de dossiers du PC : ce que le téléphone montre d'une saisie « dossier » (`Input.folder`). Le PC liste
//! un dossier à la fois (`ToPc::Browse`, `ToPhone::Folder`) pour l'action de l'écran suivi ; le téléphone montre, et
//! le chemin choisi part comme la saisie de l'action, par le chemin habituel (confirmation, action en cours).
//!
//! Sans interface : le contrôleur le suit, lui donne les numéros de demande et l'affiche. Une réponse qui n'est pas
//! celle de la dernière demande est ignorée ; un PC qui ne sait pas lister, ou qui refuse (droit « Dossiers du PC »
//! coupé), fait passer au champ de texte.

use suite_mobile::msg::{
    ENTRY_CLOUD, ENTRY_DENIED, ENTRY_DRIVE, ENTRY_HIDDEN, ENTRY_KNOWN, ENTRY_LINK, ENTRY_REPO, FolderEntry, Listing,
    ToPc,
};
use suite_mobile::ui::{FOLDER_REPO, FolderInput};

use crate::{PickCrumb, PickRow};

/// Au-delà de ce nombre de dossiers (ou pour un dossier trop grand pour être lu en entier), un champ filtre le dossier
/// sur le PC.
const FILTER_FROM: u32 = 40;

/// Une demande au PC : son numéro, le dossier (`None` : la racine) et la position de la première entrée voulue.
#[derive(Clone, Debug, PartialEq)]
pub struct Ask {
    pub req: u32,
    pub dir: Option<String>,
    pub offset: u32,
}

/// Ce qu'une réponse du PC a fait.
#[derive(Debug, PartialEq)]
pub enum Answer {
    /// Pas la réponse attendue (une demande plus récente est partie, ou le délai est passé) : ignorée.
    Stale,
    /// Montrée.
    Shown,
    /// Le dossier de départ n'a pas pu être lu et rien n'est montré encore : la racine est à demander.
    Root,
    /// Le PC ne liste pas ses dossiers (plus ancien) ou le refuse à ce téléphone : le champ de texte, avec ce message.
    Text(String),
}

pub struct Picker {
    /// L'écran (module, chemin) et l'action qui demandent le dossier.
    pub module: String,
    pub path: String,
    pub act: String,
    pub spec: FolderInput,
    /// Le libellé de la saisie (titre du sélecteur), et le nom du module (titre de ses raccourcis).
    pub title: String,
    pub module_name: String,
    /// Montrer les dossiers cachés et système.
    pub hidden: bool,
    /// Le filtre du dossier montré (appliqué sur le PC).
    pub filter: String,
    /// Ce qui est montré : le dernier dossier reçu, ses pages réunies.
    pub listing: Option<Listing>,
    /// La demande sans réponse encore.
    pub asked: Option<Ask>,
    /// Ce qui a manqué (pas de réponse, hors ligne) : dit à la place, avec « Réessayer ».
    pub trouble: Option<String>,
    /// La demande restée sans réponse (délai passé, coupure) : refaite par « Réessayer ».
    lost: Option<Ask>,
    /// La liaison a été coupée : au retour du PC, la demande perdue est refaite.
    offline: bool,
    /// Une réponse a déjà été montrée : un refus ensuite se dit dans le sélecteur, sans passer au champ de texte.
    answered: bool,
}

impl Picker {
    pub fn new(module: &str, path: &str, act: &str, title: &str, module_name: &str, spec: FolderInput) -> Picker {
        Picker {
            module: module.into(),
            path: path.into(),
            act: act.into(),
            spec,
            title: title.into(),
            module_name: module_name.into(),
            hidden: false,
            filter: String::new(),
            listing: None,
            asked: None,
            trouble: None,
            lost: None,
            offline: false,
            answered: false,
        }
    }

    /// Le dossier où s'ouvre le sélecteur (`None` : la racine).
    pub fn start(&self) -> Option<String> {
        Some(self.spec.start.trim().to_string()).filter(|s| !s.is_empty())
    }

    /// Le dossier montré (`None` : la racine, ou rien de reçu).
    fn dir(&self) -> Option<String> {
        self.listing.as_ref().map(|l| l.dir.clone()).filter(|d| !d.is_empty())
    }

    fn message(&self, ask: &Ask) -> ToPc {
        ToPc::Browse {
            req: ask.req,
            module: self.module.clone(),
            path: self.path.clone(),
            action: self.act.clone(),
            dir: ask.dir.clone(),
            filter: self.filter.clone(),
            offset: ask.offset,
            hidden: self.hidden,
        }
    }

    fn ask(&mut self, ask: Ask) -> ToPc {
        let m = self.message(&ask);
        self.asked = Some(ask);
        self.trouble = None;
        self.lost = None;
        self.offline = false;
        m
    }

    /// Ouvre un dossier (`None` ou vide : la racine), depuis son début et sans filtre.
    pub fn browse(&mut self, req: u32, dir: Option<&str>) -> ToPc {
        self.filter.clear();
        let dir = dir.map(str::trim).filter(|d| !d.is_empty()).map(String::from);
        self.ask(Ask { req, dir, offset: 0 })
    }

    /// La page suivante du dossier montré ; `None` : il n'en a pas, ou elle est déjà demandée.
    pub fn more(&mut self, req: u32) -> Option<ToPc> {
        let l = self.listing.as_ref()?;
        if !l.more || self.asked.as_ref().is_some_and(|a| a.offset > 0) {
            return None;
        }
        let offset = l.offset + l.entries.len() as u32;
        let dir = self.dir();
        Some(self.ask(Ask { req, dir, offset }))
    }

    /// Le dossier montré de nouveau, depuis son début (filtre ou dossiers cachés changés).
    fn reload(&mut self, req: u32) -> ToPc {
        let dir = match (&self.listing, &self.asked) {
            (Some(_), _) => self.dir(),
            (None, Some(a)) => a.dir.clone(),
            (None, None) => self.start(),
        };
        self.ask(Ask { req, dir, offset: 0 })
    }

    /// Montre ou masque les dossiers cachés.
    pub fn set_hidden(&mut self, req: u32, on: bool) -> ToPc {
        self.hidden = on;
        self.reload(req)
    }

    /// Filtre le dossier montré (sur le PC).
    pub fn set_filter(&mut self, req: u32, text: &str) -> ToPc {
        self.filter = text.trim().to_string();
        self.reload(req)
    }

    /// Refait la dernière demande (« Réessayer », ou au retour du PC après une coupure) ; sans demande perdue, le
    /// dossier montré.
    pub fn again(&mut self, req: u32) -> ToPc {
        match self.lost.take().or_else(|| self.asked.clone()) {
            Some(a) => self.ask(Ask { req, ..a }),
            None => self.reload(req),
        }
    }

    /// Le PC est de nouveau là après une coupure (l'écran suivi est arrivé) : `true` s'il faut refaire la demande
    /// perdue ([`Picker::again`]) ; sinon, le message de la coupure s'efface.
    pub fn back_online(&mut self) -> bool {
        if !std::mem::take(&mut self.offline) {
            return false;
        }
        if self.lost.is_some() {
            return true;
        }
        self.trouble = None;
        false
    }

    /// La réponse du PC à la demande `req`.
    pub fn answer(&mut self, req: u32, listing: Listing) -> Answer {
        if self.asked.as_ref().is_none_or(|a| a.req != req) {
            return Answer::Stale;
        }
        let Some(ask) = self.asked.take() else { return Answer::Stale };
        let nothing = listing.dir.is_empty()
            && listing.crumbs.is_empty()
            && listing.groups.is_empty()
            && listing.entries.is_empty();
        if !listing.error.is_empty() && nothing {
            if !self.answered {
                // Le dossier de départ a disparu : la racine ; la racine refusée : le PC ne listera rien.
                return if ask.dir.is_some() { Answer::Root } else { Answer::Text(listing.error) };
            }
            // Un refus plus tard (chemin saisi introuvable…) : dit sur le dossier montré, qui reste.
            if let Some(shown) = &mut self.listing {
                shown.error = listing.error;
                return Answer::Shown;
            }
        }
        self.answered = true;
        // La page suivante du dossier montré : à la suite.
        if ask.offset > 0
            && let Some(shown) = &mut self.listing
            && shown.dir == listing.dir
            && listing.offset == shown.offset + shown.entries.len() as u32
        {
            shown.entries.extend(listing.entries);
            shown.more = listing.more;
            shown.total = listing.total;
            shown.truncated = listing.truncated;
            shown.error = listing.error;
            return Answer::Shown;
        }
        self.listing = Some(listing);
        Answer::Shown
    }

    /// Le délai de la demande `req` est passé sans réponse : `true` si c'était celle attendue.
    pub fn timed_out(&mut self, req: u32) -> bool {
        if self.asked.as_ref().is_none_or(|a| a.req != req) {
            return false;
        }
        self.lost = self.asked.take();
        self.trouble = Some("Le PC n'a pas répondu.".into());
        true
    }

    /// La liaison est coupée : la demande en cours est perdue, refaite au retour du PC.
    pub fn offline(&mut self) {
        if let Some(a) = self.asked.take() {
            self.lost = Some(a);
        }
        self.offline = true;
        self.trouble = Some("Hors ligne : la liste reprendra au retour du PC.".into());
    }

    /// Le chemin que donnerait « Choisir ce dossier » : le dossier montré, s'il peut être choisi.
    pub fn chosen(&self) -> Option<String> {
        let l = self.listing.as_ref()?;
        (l.choose.is_some() && !l.dir.is_empty()).then(|| l.dir.clone())
    }

    fn known(&self, e: &FolderEntry) -> bool {
        e.flags & ENTRY_KNOWN != 0 || self.spec.known.iter().any(|k| same_path(k, &e.path))
    }

    /// Un dépôt choisi d'un toucher, sans l'ouvrir (saisie d'un dépôt existant) : ni refusé, ni déjà connu du module.
    pub fn can_pick(&self, e: &FolderEntry) -> bool {
        self.spec.want == FOLDER_REPO && e.flags & ENTRY_REPO != 0 && e.flags & ENTRY_DENIED == 0 && !self.known(e)
    }

    /// Ce chemin est celui d'une entrée montrée qui peut être choisie d'un toucher.
    pub fn pickable(&self, path: &str) -> bool {
        self.listing.iter().flat_map(|l| &l.entries).any(|e| e.path == path && self.can_pick(e))
    }

    /// Le filtre est proposé : un grand dossier, ou un filtre déjà en cours.
    pub fn filterable(&self) -> bool {
        let big = self.listing.as_ref().is_some_and(|l| !l.dir.is_empty() && (l.truncated || l.total > FILTER_FROM));
        big || !self.filter.is_empty()
    }

    /// Le fil d'Ariane : la racine (« Ce PC »), puis les dossiers jusqu'au dossier montré.
    pub fn crumbs(&self) -> Vec<PickCrumb> {
        let root = PickCrumb { label: "Ce PC".into(), path: "".into() };
        let rest = self.listing.iter().flat_map(|l| &l.crumbs).filter(|(_, path)| !path.is_empty());
        std::iter::once(root)
            .chain(rest.map(|(label, path)| PickCrumb { label: label.as_str().into(), path: path.as_str().into() }))
            .collect()
    }

    /// Le bouton de choix : son libellé, s'il est possible, et sinon pourquoi (ou que faire).
    pub fn choice(&self) -> (String, bool, String) {
        let l = self.listing.as_ref();
        let label = l.and_then(|l| l.choose.clone()).unwrap_or_else(|| "Choisir ce dossier".into());
        let can = self.chosen().is_some();
        let why = match l {
            Some(l) if !can && !l.why_not.is_empty() => l.why_not.clone(),
            _ if can => String::new(),
            _ if self.spec.want == FOLDER_REPO => "Ouvrez un dépôt, ou touchez « Choisir » à côté de lui.".into(),
            _ => "Ouvrez le dossier à choisir.".into(),
        };
        (label, can, why)
    }

    /// Les lignes de la liste : à la racine, les raccourcis du module puis les groupes du PC ; dans un dossier, ses
    /// sous-dossiers, « N de plus » et les dossiers cachés. Une erreur du PC est dite en tête.
    pub fn rows(&self) -> Vec<PickRow> {
        let mut out = Vec::new();
        let Some(l) = &self.listing else { return out };
        if !l.error.is_empty() {
            out.push(note("erreur", &l.error, 4));
        }
        if l.dir.is_empty() {
            let places: Vec<FolderEntry> = self
                .spec
                .places
                .iter()
                .map(|p| FolderEntry { name: p.label.clone(), path: p.path.clone(), ..Default::default() })
                .collect();
            let glyphs: Vec<&str> = self.spec.places.iter().map(|p| p.glyph.as_str()).collect();
            let title = if self.module_name.is_empty() { "Raccourcis" } else { self.module_name.as_str() };
            self.group(&mut out, title, &places, &glyphs);
            for g in &l.groups {
                // Un raccourci du module que le PC répète n'est montré qu'une fois, avec ceux du module.
                let rest: Vec<FolderEntry> = g
                    .places
                    .iter()
                    .filter(|e| !self.spec.places.iter().any(|p| same_path(&p.path, &e.path)))
                    .cloned()
                    .collect();
                self.group(&mut out, &g.title, &rest, &[]);
            }
            return out;
        }
        if l.truncated {
            out.push(note("trop", "Dossier trop grand pour être lu en entier : filtrez-le.", 3));
        }
        if l.entries.is_empty() && l.error.is_empty() {
            let text = if self.filter.is_empty() { "Aucun sous-dossier." } else { "Aucun dossier ne correspond." };
            out.push(note("vide", text, 0));
        }
        let n = l.entries.len();
        out.extend(l.entries.iter().enumerate().map(|(i, e)| self.entry(e, "", i == 0, i + 1 == n)));
        if l.more {
            let shown = l.offset + n as u32;
            let title = match l.total.checked_sub(shown) {
                Some(rest) if rest > 0 => format!("{rest} de plus"),
                _ => "Plus de dossiers".into(),
            };
            out.push(PickRow { kind: 2, key: "plus".into(), title: title.into(), ..Default::default() });
        }
        let toggle = match (self.hidden, l.hidden) {
            (true, _) => Some("Masquer les dossiers cachés".to_string()),
            (false, 1) => Some("1 dossier caché · Afficher".into()),
            (false, h) if h > 1 => Some(format!("{h} dossiers cachés · Afficher")),
            _ => None,
        };
        if let Some(t) = toggle {
            out.push(PickRow { kind: 4, key: "caches".into(), title: t.into(), ..Default::default() });
        }
        out
    }

    fn group(&self, out: &mut Vec<PickRow>, title: &str, places: &[FolderEntry], glyphs: &[&str]) {
        if places.is_empty() {
            return;
        }
        out.push(PickRow {
            kind: 0,
            key: format!("g:{title}").into(),
            title: title.to_uppercase().into(),
            ..Default::default()
        });
        let n = places.len();
        for (i, e) in places.iter().enumerate() {
            let glyph = glyphs.get(i).copied().unwrap_or("");
            let mut row = self.entry(e, glyph, i == 0, i + 1 == n);
            // Plusieurs groupes peuvent mener au même dossier : la clé reste unique dans la liste.
            row.key = format!("{title}\u{1f}{}", e.path).into();
            if row.subtitle.is_empty() && e.name != e.path {
                row.subtitle = e.path.as_str().into();
            }
            out.push(row);
        }
    }

    /// Un dossier : son icône et le ton de celle-ci d'après ses drapeaux, ce qu'en dit le PC, et ses marques (lien,
    /// en ligne seulement, caché) ; refusé : grisé, sans toucher.
    fn entry(&self, e: &FolderEntry, glyph: &str, first: bool, last: bool) -> PickRow {
        let f = e.flags;
        let has = |flag: u8| f & flag != 0;
        let known = self.known(e);
        let (icon, tone) = if has(ENTRY_DENIED) {
            ("lock", 0)
        } else if has(ENTRY_DRIVE) {
            ("drive", 0)
        } else if has(ENTRY_REPO) {
            ("repo", 2)
        } else if has(ENTRY_CLOUD) {
            ("cloud", 1)
        } else if has(ENTRY_LINK) {
            ("link", 1)
        } else {
            ("folder", 1)
        };
        let mut said: Vec<&str> = Vec::new();
        if !e.detail.is_empty() {
            said.push(&e.detail);
        } else if has(ENTRY_DENIED) {
            said.push("accès refusé");
        } else if known {
            said.push("déjà là");
        } else if has(ENTRY_REPO) {
            said.push("dépôt git");
        }
        if has(ENTRY_LINK) {
            said.push("lien");
        }
        if has(ENTRY_CLOUD) {
            said.push("en ligne seulement");
        }
        if has(ENTRY_HIDDEN) {
            said.push("caché");
        }
        PickRow {
            kind: 1,
            key: e.path.as_str().into(),
            title: if e.name.is_empty() { e.path.as_str().into() } else { e.name.as_str().into() },
            subtitle: said.join(" · ").into(),
            icon: icon.into(),
            glyph: glyph.into(),
            tone,
            path: e.path.as_str().into(),
            enabled: !has(ENTRY_DENIED),
            dim: has(ENTRY_HIDDEN) || known,
            pick: self.can_pick(e),
            first,
            last,
        }
    }
}

fn note(key: &str, text: &str, tone: i32) -> PickRow {
    PickRow { kind: 3, key: key.into(), title: text.into(), tone, ..Default::default() }
}

/// Deux chemins Windows désignent le même dossier (sans tenir compte de la casse ni du `\` final).
fn same_path(a: &str, b: &str) -> bool {
    let norm = |p: &str| p.trim_end_matches(['\\', '/']).replace('/', "\\").to_lowercase();
    norm(a) == norm(b)
}

#[cfg(test)]
mod tests {
    use suite_mobile::msg::PlaceGroup;
    use suite_mobile::ui::{FOLDER_ANY, FOLDER_PARENT, Place};

    use super::*;

    fn entry(name: &str, flags: u8) -> FolderEntry {
        FolderEntry { name: name.into(), path: format!(r"C:\DEV\{name}"), flags, detail: String::new() }
    }

    fn listing(dir: &str, names: &[(&str, u8)]) -> Listing {
        Listing {
            dir: dir.into(),
            title: dir.rsplit('\\').next().unwrap_or_default().into(),
            crumbs: vec![("C:".into(), r"C:\".into()), ("DEV".into(), r"C:\DEV".into())],
            entries: names.iter().map(|&(n, f)| entry(n, f)).collect(),
            total: names.len() as u32,
            ..Default::default()
        }
    }

    fn picker(want: u8) -> Picker {
        let spec = FolderInput { want, ..Default::default() };
        Picker::new("dragontree", "", "ajouter", "Dépôt à ajouter", "DragonTree", spec)
    }

    fn browse_req(m: &ToPc) -> (u32, Option<String>, u32, bool, String) {
        match m {
            ToPc::Browse { req, dir, offset, hidden, filter, .. } => {
                (*req, dir.clone(), *offset, *hidden, filter.clone())
            }
            _ => panic!("pas une demande de dossier : {m:?}"),
        }
    }

    fn titles(p: &Picker) -> Vec<String> {
        p.rows().iter().map(|r| r.title.to_string()).collect()
    }

    /// Seule la réponse à la dernière demande compte : un dossier touché puis un autre, la première réponse arrivée
    /// en retard ne remplace pas la seconde ; un délai passé, sa réponse ne fait plus rien.
    #[test]
    fn a_late_answer_does_not_replace_the_folder_asked_after_it() {
        let mut p = picker(FOLDER_ANY);
        let first = p.browse(1, None);
        assert_eq!(browse_req(&first), (1, None, 0, false, String::new()), "la racine");
        assert_eq!(p.answer(1, Listing { title: "Ce PC".into(), ..Default::default() }), Answer::Shown);
        p.browse(2, Some(r"C:\DEV"));
        p.browse(3, Some(r"C:\Users"));
        assert_eq!(p.answer(2, listing(r"C:\DEV", &[("Eyrie", 0)])), Answer::Stale, "une demande plus récente");
        assert_eq!(p.answer(3, listing(r"C:\Users", &[("aubin", 0)])), Answer::Shown);
        assert_eq!(p.listing.as_ref().map(|l| l.dir.as_str()), Some(r"C:\Users"));
        assert_eq!(p.answer(3, listing(r"C:\DEV", &[])), Answer::Stale, "une seule fois");

        p.browse(4, Some(r"C:\DEV"));
        assert!(!p.timed_out(3), "pas la demande attendue");
        assert!(p.timed_out(4));
        assert_eq!(p.trouble.as_deref(), Some("Le PC n'a pas répondu."));
        assert_eq!(p.answer(4, listing(r"C:\DEV", &[])), Answer::Stale, "arrivée après le délai");
        let retry = p.again(5);
        assert_eq!(browse_req(&retry).1.as_deref(), Some(r"C:\DEV"), "« Réessayer » refait la demande perdue");
        assert_eq!(p.trouble, None);
    }

    /// « N de plus » demande la suite du dossier, qui s'ajoute à ce qui est montré ; une seule fois à la fois ; plus
    /// rien quand le PC dit que tout est là.
    #[test]
    fn more_appends_the_next_page_of_the_folder() {
        let mut p = picker(FOLDER_ANY);
        p.browse(1, Some(r"C:\DEV"));
        let mut first = listing(r"C:\DEV", &[("a", 0), ("b", 0)]);
        (first.total, first.more) = (5, true);
        p.answer(1, first);
        assert_eq!(titles(&p), ["a", "b", "3 de plus"]);
        let next = p.more(2).expect("une page de plus");
        assert_eq!(browse_req(&next), (2, Some(r"C:\DEV".into()), 2, false, String::new()));
        assert!(p.more(3).is_none(), "déjà demandée");
        let mut page = listing(r"C:\DEV", &[("c", 0), ("d", 0), ("e", 0)]);
        (page.offset, page.total, page.more) = (2, 5, false);
        assert_eq!(p.answer(2, page), Answer::Shown);
        assert_eq!(titles(&p), ["a", "b", "c", "d", "e"]);
        let rows = p.rows();
        assert!(rows[0].first && !rows[1].last && rows[4].last, "une seule carte");
        assert!(p.more(4).is_none(), "tout est là");
    }

    /// Un PC qui ne liste pas ses dossiers (plus ancien), ou qui le refuse à ce téléphone, fait passer au champ de
    /// texte avec son message ; un dossier de départ illisible ramène d'abord à la racine. Une fois un dossier montré,
    /// un refus se dit dans le sélecteur.
    #[test]
    fn a_pc_that_cannot_list_falls_back_to_the_text_field() {
        let refused = |text: &str| Listing { error: text.into(), ..Default::default() };
        let mut p = picker(FOLDER_ANY);
        p.browse(1, None);
        assert_eq!(
            p.answer(1, refused("Ce PC ne sait pas encore lister ses dossiers.")),
            Answer::Text("Ce PC ne sait pas encore lister ses dossiers.".into())
        );

        let mut p = picker(FOLDER_ANY);
        p.spec.start = r"C:\DEV\parti".into();
        let first = p.browse(1, p.start().as_deref());
        assert_eq!(browse_req(&first).1.as_deref(), Some(r"C:\DEV\parti"));
        assert_eq!(p.answer(1, refused("Dossier introuvable")), Answer::Root);
        p.browse(2, None);
        assert_eq!(
            p.answer(2, refused("Droit « Dossiers du PC » coupé")),
            Answer::Text("Droit « Dossiers du PC » coupé".into())
        );

        let mut p = picker(FOLDER_ANY);
        p.browse(1, Some(r"C:\DEV"));
        p.answer(1, listing(r"C:\DEV", &[("Eyrie", 0)]));
        p.browse(2, Some(r"C:\nulle\part"));
        assert_eq!(p.answer(2, refused("C:\\nulle\\part : introuvable")), Answer::Shown);
        assert_eq!(titles(&p), ["C:\\nulle\\part : introuvable", "Eyrie"], "le dossier montré reste");
    }

    /// Une coupure perd la demande en cours : elle est dite, et refaite au retour du PC.
    #[test]
    fn a_request_lost_offline_is_made_again() {
        let mut p = picker(FOLDER_ANY);
        p.browse(1, Some(r"C:\DEV"));
        p.offline();
        assert!(p.asked.is_none() && p.trouble.is_some());
        assert_eq!(p.answer(1, listing(r"C:\DEV", &[])), Answer::Stale, "la connexion d'avant");
        assert!(p.back_online(), "à refaire");
        let again = p.again(7);
        assert_eq!(browse_req(&again).0, 7);
        assert_eq!(browse_req(&again).1.as_deref(), Some(r"C:\DEV"));
        assert!(p.trouble.is_none());
        assert!(!p.back_online(), "une seule fois");

        // Coupé sans rien attendre : le message s'efface au retour.
        p.answer(7, listing(r"C:\DEV", &[]));
        p.offline();
        assert!(!p.back_online() && p.trouble.is_none());
    }

    /// Les dossiers cachés et le filtre redemandent le dossier montré, depuis son début ; ouvrir un autre dossier
    /// efface le filtre.
    #[test]
    fn hidden_folders_and_the_filter_reload_the_folder() {
        let mut p = picker(FOLDER_ANY);
        p.browse(1, Some(r"C:\DEV"));
        let mut l = listing(r"C:\DEV", &[("a", 0)]);
        l.hidden = 2;
        p.answer(1, l);
        assert_eq!(titles(&p), ["a", "2 dossiers cachés · Afficher"]);
        assert_eq!(browse_req(&p.set_hidden(2, true)), (2, Some(r"C:\DEV".into()), 0, true, String::new()));
        p.answer(2, listing(r"C:\DEV", &[("a", 0), (".git", ENTRY_HIDDEN)]));
        assert_eq!(titles(&p).last().map(String::as_str), Some("Masquer les dossiers cachés"));
        assert!(p.rows()[1].dim, "un dossier caché est pâle");
        assert_eq!(browse_req(&p.set_filter(3, " ozm ")).4, "ozm");
        assert!(p.filterable(), "le filtre reste tant qu'il y a du texte");
        assert_eq!(browse_req(&p.browse(4, Some(r"C:\Users"))), (4, Some(r"C:\Users".into()), 0, true, String::new()));
    }

    /// Les drapeaux d'une entrée : icône, ton, marques ; refusée, elle ne se touche pas ; un dépôt se choisit d'un
    /// toucher dans une saisie de dépôt, sauf s'il est déjà connu du module.
    #[test]
    fn entries_show_their_flags_and_repos_can_be_picked() {
        let mut p = picker(FOLDER_REPO);
        p.spec.known = vec![r"c:\dev\ozmo\".into()];
        p.browse(1, Some(r"C:\DEV"));
        let mut denied = entry("System Volume Information", ENTRY_DENIED | ENTRY_HIDDEN);
        denied.detail = String::new();
        let mut l =
            listing(r"C:\DEV", &[("Eyrie", ENTRY_REPO), ("Ozmo", ENTRY_REPO), ("OneDrive", ENTRY_CLOUD | ENTRY_LINK)]);
        l.entries.push(denied);
        l.why_not = "Ce dossier n'est pas un dépôt git.".into();
        p.answer(1, l);
        let r = p.rows();
        assert_eq!((r[0].icon.as_str(), r[0].tone, r[0].subtitle.as_str(), r[0].pick), ("repo", 2, "dépôt git", true));
        assert_eq!((r[1].subtitle.as_str(), r[1].pick, r[1].dim), ("déjà là", false, true), "déjà dans le module");
        assert_eq!((r[2].icon.as_str(), r[2].subtitle.as_str()), ("cloud", "lien · en ligne seulement"));
        assert_eq!((r[3].icon.as_str(), r[3].enabled, r[3].subtitle.as_str()), ("lock", false, "accès refusé · caché"));
        assert!(p.pickable(r"C:\DEV\Eyrie") && !p.pickable(r"C:\DEV\Ozmo"));
        assert_eq!(p.choice(), ("Choisir ce dossier".into(), false, "Ce dossier n'est pas un dépôt git.".into()));
        assert_eq!(p.chosen(), None);

        let mut any = picker(FOLDER_ANY);
        any.browse(1, Some(r"C:\DEV"));
        any.answer(1, listing(r"C:\DEV", &[("Eyrie", ENTRY_REPO)]));
        assert!(!any.rows()[0].pick, "hors d'une saisie de dépôt : on l'ouvre");
    }

    /// Un dossier qui peut être choisi : le bouton prend le libellé du PC ; la destination d'un clone est dite.
    #[test]
    fn the_choice_follows_the_listing() {
        let mut p = picker(FOLDER_PARENT);
        p.browse(1, Some(r"C:\DEV"));
        let mut l = listing(r"C:\DEV", &[]);
        l.choose = Some("Cloner ici".into());
        l.target = r"C:\DEV\Ozmo : sera créé".into();
        p.answer(1, l);
        assert_eq!(p.choice(), ("Cloner ici".into(), true, String::new()));
        assert_eq!(p.chosen().as_deref(), Some(r"C:\DEV"));
        assert_eq!(titles(&p), ["Aucun sous-dossier."]);
        assert_eq!(p.crumbs().iter().map(|c| c.label.to_string()).collect::<Vec<_>>(), ["Ce PC", "C:", "DEV"]);
    }

    /// À la racine : les raccourcis du module d'abord (avec leur glyphe), puis les groupes du PC, sans répéter un
    /// raccourci du module.
    #[test]
    fn the_root_shows_the_module_places_first() {
        let mut p = picker(FOLDER_REPO);
        p.spec.places =
            vec![Place { label: "Dossier de clonage".into(), path: r"C:\DEV".into(), glyph: "📥".into() }];
        p.browse(1, None);
        let drive = FolderEntry { name: "C:".into(), path: r"C:\".into(), flags: ENTRY_DRIVE, ..Default::default() };
        let again = FolderEntry { name: "DEV".into(), path: r"c:\dev\".into(), ..Default::default() };
        let docs = FolderEntry { name: "Documents".into(), path: r"C:\Users\a\Documents".into(), ..Default::default() };
        let l = Listing {
            groups: vec![
                PlaceGroup { title: "Récents".into(), places: vec![again] },
                PlaceGroup { title: "Accès rapide".into(), places: vec![docs] },
                PlaceGroup { title: "Lecteurs".into(), places: vec![drive] },
            ],
            ..Default::default()
        };
        p.answer(1, l);
        assert_eq!(
            titles(&p),
            ["DRAGONTREE", "Dossier de clonage", "ACCÈS RAPIDE", "Documents", "LECTEURS", "C:"],
            "« Récents » ne répétait que le raccourci du module"
        );
        let r = p.rows();
        assert_eq!((r[1].glyph.as_str(), r[1].subtitle.as_str()), ("📥", r"C:\DEV"));
        assert_eq!(r[5].icon, "drive");
        assert!(!p.choice().1, "la racine ne se choisit pas");
    }
}
