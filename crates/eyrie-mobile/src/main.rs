//! Eyrie mobile sur le bureau : la même application dans une fenêtre de téléphone, pour le développement et les
//! captures (`EYRIE_MOBILE_DEMO`, `EYRIE_MOBILE_CAPTURE`).
//!
//! `eyrie-mobile --icones <dossier res>` écrit les icônes Android (le nid d'Eyrie) : `tools\android.ps1` s'en sert.

fn main() -> Result<(), slint::PlatformError> {
    let args: Vec<String> = std::env::args().collect();
    if let [_, flag, dir] = args.as_slice()
        && flag == "--icones"
    {
        if let Err(e) = eyrie_mobile::write_icons(std::path::Path::new(dir)) {
            eprintln!("icônes : {e}");
            std::process::exit(1);
        }
        return Ok(());
    }
    eyrie_mobile::run()
}
