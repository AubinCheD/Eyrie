//! Compile l'interface (`ui/app.slint`) ; le style « material » des widgets standard est celui d'Android. Sur Android,
//! la caméra du NDK (`camera2ndk`) et son lecteur d'images (`mediandk`) sont liés, ainsi que le nœud Tailscale de
//! l'application (`libeyrie_tailnet.so`, compilé par tools\android.ps1 dans le dossier `EYRIE_TAILNET_DIR`).
fn main() {
    let config = slint_build::CompilerConfiguration::new().with_style("material".into());
    slint_build::compile_with_config("ui/app.slint", config).expect("interface Slint");
    println!("cargo:rerun-if-env-changed=EYRIE_TAILNET_DIR");
    if std::env::var("TARGET").is_ok_and(|t| t.contains("android")) {
        println!("cargo:rustc-link-lib=camera2ndk");
        println!("cargo:rustc-link-lib=mediandk");
        match std::env::var("EYRIE_TAILNET_DIR") {
            Ok(dir) => println!("cargo:rustc-link-search=native={dir}"),
            Err(_) => println!("cargo:warning=EYRIE_TAILNET_DIR absent : construire l'APK par tools\\android.ps1"),
        }
    }
}
