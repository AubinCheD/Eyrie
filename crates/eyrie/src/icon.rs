//! Icône de la suite (le nid de `nest.rs`, aussi celle de l'exécutable). Et l'image des modules qui en ont une
//! (le phénix de Fawkes), à la place de leur glyphe.

use eframe::egui::{self, Color32, IntoAtoms as _};
use suite_sdk::ModuleInfo;

use crate::nest::render;

/// L'icône des fenêtres, dessinée une fois : les fenêtres détachées la redonnent à chaque image.
pub fn window_icon() -> std::sync::Arc<egui::IconData> {
    static ICON: std::sync::OnceLock<std::sync::Arc<egui::IconData>> = std::sync::OnceLock::new();
    ICON.get_or_init(|| {
        let size = 64;
        std::sync::Arc::new(egui::IconData { rgba: render(size), width: size as u32, height: size as u32 })
    })
    .clone()
}

/// L'icône de la suite, pour sa barre de titre.
pub fn suite_texture(ctx: &egui::Context) -> egui::TextureHandle {
    let id = egui::Id::unique("suite-icone");
    if let Some(t) = ctx.data(|d| d.get_temp::<egui::TextureHandle>(id)) {
        return t;
    }
    const SIZE: usize = 64;
    let image = egui::ColorImage::from_rgba_unmultiplied([SIZE, SIZE], &render(SIZE));
    let options = egui::TextureOptions::LINEAR.with_mipmap_mode(Some(egui::TextureFilter::Linear));
    let t = ctx.load_texture("suite-icone", image, options);
    ctx.data_mut(|d| d.insert_temp(id, t.clone()));
    t
}

/// L'image d'un module qui en a une (`ModuleInfo::logo`), chargée une fois pour toutes les fenêtres.
pub fn logo(ctx: &egui::Context, info: &ModuleInfo) -> Option<egui::TextureHandle> {
    let render = info.logo?;
    let id = egui::Id::unique(("suite-logo", info.id));
    if let Some(t) = ctx.data(|d| d.get_temp::<egui::TextureHandle>(id)) {
        return Some(t);
    }
    // Rendue en grand, réduite à l'affichage : les mipmaps gardent les petites tailles nettes.
    const SIZE: usize = 64;
    let image = egui::ColorImage::from_rgba_unmultiplied([SIZE, SIZE], &render(SIZE as u32));
    let options = egui::TextureOptions::LINEAR.with_mipmap_mode(Some(egui::TextureFilter::Linear));
    let t = ctx.load_texture(format!("logo-{}", info.id), image, options);
    ctx.data_mut(|d| d.insert_temp(id, t.clone()));
    Some(t)
}

/// L'image du module à `size` points, éteinte (`on` faux) pour un module arrêté.
fn logo_image(t: &egui::TextureHandle, size: f32, on: bool) -> egui::Image<'static> {
    let tint = if on { Color32::WHITE } else { Color32::WHITE.gamma_multiply(0.4) };
    egui::Image::new((t.id(), egui::vec2(size, size))).tint(tint)
}

/// L'icône d'un module dans une ligne : son image, sinon son glyphe dans sa couleur.
pub fn module_icon(ui: &mut egui::Ui, info: &ModuleInfo, size: f32, color: Color32) -> egui::Response {
    match logo(ui.ctx(), info) {
        Some(t) => ui.add(logo_image(&t, size + 1.0, true)),
        None => ui.label(egui::RichText::new(info.icon).size(size).color(color)),
    }
}

/// Le module devant le texte d'un bouton ou d'un menu, dans `color` : son image (éteinte si `on` est faux), sinon son
/// glyphe dans le texte.
pub fn with_module(
    ctx: &egui::Context,
    info: &ModuleInfo,
    text: &str,
    color: Color32,
    on: bool,
) -> egui::Atoms<'static> {
    match logo(ctx, info) {
        Some(t) => (logo_image(&t, 15.0, on), egui::RichText::new(text).color(color)).into_atoms(),
        None => egui::RichText::new(format!("{}  {text}", info.icon)).color(color).into_atoms(),
    }
}

/// Dessine l'image d'un module dans `rect` (barre d'activité, onglets).
pub fn paint_logo(painter: &egui::Painter, t: &egui::TextureHandle, rect: egui::Rect, on: bool) {
    let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
    let tint = if on { Color32::WHITE } else { Color32::WHITE.gamma_multiply(0.4) };
    painter.image(t.id(), rect, uv, tint);
}

#[cfg(test)]
mod tests {
    #[test]
    fn icon_is_the_nest_with_its_eggs_and_golden_heart() {
        let size = 64;
        let px = super::render(size);
        let at = |x: usize, y: usize| <[u8; 4]>::try_from(&px[(y * size + x) * 4..][..4]).unwrap();
        assert_eq!(at(0, 0)[3], 0, "coin hors du nid, transparent");
        for (x, y) in [(1, 32), (62, 32), (32, 1), (32, 62)] {
            assert_eq!(at(x, y)[3], 255, "le nid remplit l'icône, bord à bord ({x}, {y})");
        }
        assert_eq!(at(31, 31), [0xe8, 0xb8, 0x4a, 255], "le cœur doré");
        assert_eq!(at(31, 21), [0xf2, 0x6d, 0x3b, 255], "l'œuf du haut, aux couleurs de Fawkes");
    }

    #[cfg(windows)]
    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain([0]).collect()
    }

    /// L'exécutable porte l'icône (Explorateur, raccourcis, barre des tâches épinglée) : `build.rs`.
    #[cfg(windows)]
    #[test]
    fn exe_has_icon() {
        use windows_sys::Win32::UI::Shell::ExtractIconExW;
        let path = wide(&std::env::current_exe().unwrap().to_string_lossy());
        // SAFETY : chemin terminé par zéro ; -1 demande seulement le nombre d'icônes.
        let count = unsafe { ExtractIconExW(path.as_ptr(), -1, std::ptr::null_mut(), std::ptr::null_mut(), 0) };
        assert!(count >= 1, "aucune icône dans l'exécutable");
    }

    /// Et ses informations de version (Propriétés > Détails, Gestionnaire des tâches).
    #[cfg(windows)]
    #[test]
    fn exe_has_version_info() {
        use windows_sys::Win32::Storage::FileSystem::{GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW};
        let path = wide(&std::env::current_exe().unwrap().to_string_lossy());
        // Français (France), Unicode : la table écrite par `build.rs`.
        let read = |key: &str| -> Option<String> {
            // SAFETY : tampon de la taille annoncée ; la valeur lue pointe dans ce tampon, encore vivant.
            unsafe {
                let size = GetFileVersionInfoSizeW(path.as_ptr(), std::ptr::null_mut());
                let mut data = vec![0u8; size as usize];
                if size == 0 || GetFileVersionInfoW(path.as_ptr(), 0, size, data.as_mut_ptr().cast()) == 0 {
                    return None;
                }
                let (mut p, mut len) = (std::ptr::null_mut(), 0u32);
                let q = wide(&format!(r"\StringFileInfo\040C04B0\{key}"));
                if VerQueryValueW(data.as_ptr().cast(), q.as_ptr(), &mut p, &mut len) == 0 || len == 0 {
                    return None;
                }
                let s = std::slice::from_raw_parts(p.cast::<u16>(), len as usize);
                Some(String::from_utf16_lossy(s).trim_end_matches('\0').to_string())
            }
        };
        assert_eq!(read("ProductName").as_deref(), Some("Eyrie"));
        assert_eq!(read("OriginalFilename").as_deref(), Some("eyrie.exe"));
        assert_eq!(read("ProductVersion").as_deref(), Some(env!("CARGO_PKG_VERSION")));
    }
}
