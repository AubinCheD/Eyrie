"""Produit crates/eyrie-mobile/ui/icons.slint : les icônes du téléphone, tirées de Material Symbols Rounded.

Material Symbols (Google, licence Apache 2.0) : https://github.com/google/material-design-icons, dossier
`symbols/web/<nom>/materialsymbolsrounded/` ; contour (`<nom>_24px.svg`, poids 400) et plein (`<nom>_fill1_24px.svg`).

    python tools/icones.py [<dossier de cache>]

Les fichiers SVG sont téléchargés dans le cache (par défaut `target/icones`) s'ils n'y sont pas. Pour ajouter une
icône : une ligne dans ICONES (le nom employé par l'application, celui de Material Symbols), puis relancer.
"""

import re
import sys
import urllib.request
from pathlib import Path

RACINE = Path(__file__).resolve().parent.parent
SORTIE = RACINE / "crates" / "eyrie-mobile" / "ui" / "icons.slint"
SOURCE = "https://raw.githubusercontent.com/google/material-design-icons/master/symbols/web/{n}/materialsymbolsrounded/{n}{v}_24px.svg"

# (nom dans l'application, nom Material Symbols). L'ordre ne compte pas.
ICONES = [
    # Navigation et barre du haut.
    ("back", "arrow_back"),
    ("close", "close"),
    ("more", "more_vert"),
    ("send", "send"),
    ("search", "search"),
    ("chevron", "chevron_right"),
    ("down", "expand_more"),
    ("up", "expand_less"),
    ("home", "home"),
    ("apps", "apps"),
    ("bell", "notifications"),
    ("gear", "settings"),
    ("qr", "qr_code_scanner"),
    ("keyboard", "keyboard"),
    ("pc", "computer"),
    ("add", "add"),
    ("zoom-in", "add"),
    ("remove", "remove"),
    ("zoom-out", "remove"),
    ("check", "check"),
    ("star", "star"),
    ("star-outline", "star"),
    ("edit", "edit"),
    ("edit-note", "edit_note"),
    ("folder", "folder"),
    ("folder-open", "folder_open"),
    ("repo", "folder_code"),
    ("drive", "hard_drive"),
    ("cloud", "cloud"),
    ("cloud-off", "cloud_off"),
    ("link", "link"),
    ("power", "power_settings_new"),
    ("lock", "lock"),
    ("eye", "visibility"),
    ("eye-off", "visibility_off"),
    ("mic", "mic"),
    # Actions des modules (voir `view::icon_for`).
    ("done-all", "done_all"),
    ("mark-read", "drafts"),
    ("mark-unread", "mark_email_unread"),
    ("mail", "mail"),
    ("snooze", "snooze"),
    ("alarm", "alarm"),
    ("undo", "undo"),
    ("redo", "redo"),
    ("refresh", "refresh"),
    ("sync", "sync"),
    ("restart", "restart_alt"),
    ("notifications-off", "notifications_off"),
    ("stop", "stop"),
    ("stop-circle", "stop_circle"),
    ("play", "play_arrow"),
    ("pause", "pause"),
    ("delete", "delete"),
    ("arrow-down", "arrow_downward"),
    ("arrow-up", "arrow_upward"),
    ("arrow-forward", "arrow_forward"),
    ("download", "download"),
    ("upload", "upload"),
    ("cloud-download", "cloud_download"),
    ("cloud-upload", "cloud_upload"),
    ("terminal", "terminal"),
    ("sparkle", "asterisk"),
    ("sleep", "bedtime"),
    ("wake", "light_mode"),
    ("remote", "wifi_tethering"),
    ("label", "sell"),
    ("archive", "archive"),
    ("stash", "inventory_2"),
    ("sweep", "cleaning_services"),
    ("swap", "swap_horiz"),
    ("swap-vert", "swap_vert"),
    ("warning", "warning"),
    ("error", "error"),
    ("info", "info"),
    ("wallet", "account_balance_wallet"),
    ("savings", "savings"),
    ("merge", "merge"),
    ("branch", "fork_right"),
    ("call-split", "call_split"),
    ("tree", "account_tree"),
    ("commit", "commit"),
    ("diff", "difference"),
    ("compare", "compare_arrows"),
    ("cherry", "nutrition"),
    ("open-in-new", "open_in_new"),
    ("key", "key"),
    ("copy", "content_copy"),
    ("paste", "content_paste"),
    ("select-all", "select_all"),
    ("chart", "bar_chart"),
    ("history", "history"),
    ("schedule", "schedule"),
    ("rewind", "fast_rewind"),
    ("pin", "keep"),
    ("place", "location_on"),
    ("current", "radio_button_checked"),
    ("circle", "circle"),
    ("globe", "public"),
    ("person", "person"),
    ("container", "deployed_code"),
    ("extension", "extension"),
    ("doc", "description"),
    ("draft", "draft"),
    ("book", "menu_book"),
    ("cancel", "cancel"),
    ("check-circle", "check_circle"),
    ("checkbox", "check_box"),
    ("contrast", "contrast"),
    ("review", "rate_review"),
    ("task", "assignment"),
    ("task-done", "task_alt"),
    ("inbox", "inbox"),
    ("agent", "smart_toy"),
    ("dns", "dns"),
    ("tune", "tune"),
    ("code", "code"),
    ("fire", "local_fire_department"),
    ("filter", "filter_list"),
    ("unfold-more", "unfold_more"),
    ("unfold-less", "unfold_less"),
    ("new", "new_releases"),
    # La saisie du terminal.
    ("attach", "attach_file"),
    ("photo", "add_photo_alternate"),
    ("image", "image"),
    ("camera", "photo_camera"),
    ("upload-file", "upload_file"),
    ("left", "keyboard_arrow_left"),
    ("right", "keyboard_arrow_right"),
    ("first", "first_page"),
    ("last", "last_page"),
    ("backspace", "backspace"),
]

# Ces icônes ont aussi leur version pleine (`filled: true`) : l'onglet actif du bas, une étoile, une épingle…
PLEINES = {
    "home", "apps", "bell", "gear", "terminal", "tree", "inbox", "agent", "dns", "book", "savings", "edit-note",
    "star", "pin", "check-circle", "circle", "mail", "folder", "pc", "warning", "error", "info", "label", "person",
}


def chemin(cache: Path, nom: str, plein: bool) -> str:
    v = "_fill1" if plein else ""
    f = cache / f"{nom}{v}.svg"
    if not f.exists() or f.stat().st_size == 0:
        with urllib.request.urlopen(SOURCE.format(n=nom, v=v), timeout=30) as r:
            f.write_bytes(r.read())
    svg = f.read_text(encoding="utf-8")
    trouves = re.findall(r'<path d="([^"]+)"', svg)
    if len(trouves) != 1 or 'viewBox="0 -960 960 960"' not in svg:
        raise SystemExit(f"{f} : un seul tracé attendu, dans la boîte 0 -960 960 960")
    d = trouves[0]
    if not re.fullmatch(r"[MmLlHhVvCcSsQqTtAaZz0-9.,\- ]+", d):
        raise SystemExit(f"{f} : tracé inattendu")
    return d


def chaine(paires: list[tuple[str, str]], defaut: str, indent: str, suite: bool) -> str:
    """Une suite de conditions sur `kind` ; `suite` : elle continue une condition (chaque ligne commence par « : »)."""
    lignes = []
    for i, (k, d) in enumerate(paires):
        tete = ": " if suite or i > 0 else ""
        lignes.append(f'{indent}{tete}root.kind == "{k}" ? "{d}"')
    lignes.append(f"{indent}: {defaut}")
    return "\n".join(lignes)


def main() -> None:
    cache = Path(sys.argv[1]) if len(sys.argv) > 1 else RACINE / "target" / "icones"
    cache.mkdir(parents=True, exist_ok=True)
    noms = [k for k, _ in ICONES]
    if len(set(noms)) != len(noms):
        raise SystemExit("nom en double dans ICONES")
    if not PLEINES <= set(noms):
        raise SystemExit(f"PLEINES inconnues : {PLEINES - set(noms)}")
    contour = [(k, chemin(cache, s, False)) for k, s in ICONES]
    pleines = [(k, chemin(cache, s, True)) for k, s in ICONES if k in PLEINES]
    texte = f"""// Généré par tools/icones.py : ne pas modifier à la main (y ajouter l'icône, puis le relancer).
//
// Les icônes du téléphone : Material Symbols Rounded de Google (licence Apache 2.0,
// https://github.com/google/material-design-icons), contour au poids 400, et la version pleine de quelques-unes
// (`filled`). Elles ne dépendent pas des polices du téléphone. Les glyphes envoyés par les modules y sont ramenés par
// `view::icon_for`.

import {{ Theme }} from "theme.slint";

export component Icon inherits Rectangle {{
    in property <string> kind;
    in property <color> tint: Theme.text;
    in property <length> size: 24px;
    // La version pleine, quand l'icône en a une (sinon le contour).
    in property <bool> filled;
    width: size;
    height: size;
    property <string> outline:
{chaine(contour, '""', "        ", False)};
    Path {{
        width: parent.width;
        height: parent.height;
        viewbox-x: 0;
        viewbox-y: -960;
        viewbox-width: 960;
        viewbox-height: 960;
        fill: root.tint;
        commands: !root.filled ? root.outline
{chaine(pleines, 'root.outline', "            ", True)};
    }}
}}

// Les noms connus (essais : chaque icône demandée par `view::icon_for` existe).
// {" ".join(noms)}
"""
    SORTIE.write_text(texte, encoding="utf-8", newline="\n")
    print(f"{SORTIE} : {len(contour)} icônes, {len(pleines)} pleines")


if __name__ == "__main__":
    main()
