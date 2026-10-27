//! Compile l'interface (`ui/app.slint`) ; le style « material » des widgets standard est celui d'Android. Sur Android,
//! la caméra du NDK (`camera2ndk`) et son lecteur d'images (`mediandk`) sont liés.
fn main() {
    let config = slint_build::CompilerConfiguration::new().with_style("material".into());
    slint_build::compile_with_config("ui/app.slint", config).expect("interface Slint");
    if std::env::var("TARGET").is_ok_and(|t| t.contains("android")) {
        println!("cargo:rustc-link-lib=camera2ndk");
        println!("cargo:rustc-link-lib=mediandk");
    }
}
