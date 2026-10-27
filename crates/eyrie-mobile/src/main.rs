//! Eyrie mobile sur le bureau : la même application dans une fenêtre de téléphone, pour le développement et les
//! captures (`EYRIE_MOBILE_DEMO`, `EYRIE_MOBILE_CAPTURE`).

fn main() -> Result<(), slint::PlatformError> {
    eyrie_mobile::run()
}
