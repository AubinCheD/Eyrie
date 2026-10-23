//! Configuration d'écrans : une disposition peut être associée à chacune (portable seul, station
//! d'accueil à deux écrans) et s'applique au branchement.

use std::time::{Duration, Instant};

/// « 2 écrans : 2560×1440, 1920×1080 », dans un ordre stable.
pub fn signature() -> String {
    let mut sizes = monitors();
    sizes.sort();
    let list: Vec<String> = sizes.iter().map(|(w, h)| format!("{w}×{h}")).collect();
    match list.len() {
        0 => "écran inconnu".into(),
        1 => format!("1 écran : {}", list[0]),
        n => format!("{n} écrans : {}", list.join(", ")),
    }
}

#[cfg(windows)]
fn monitors() -> Vec<(i32, i32)> {
    use windows_sys::Win32::Foundation::{LPARAM, RECT};
    use windows_sys::Win32::Graphics::Gdi::{EnumDisplayMonitors, HDC, HMONITOR};
    unsafe extern "system" fn each(_m: HMONITOR, _dc: HDC, rect: *mut RECT, data: LPARAM) -> i32 {
        let out = unsafe { &mut *(data as *mut Vec<(i32, i32)>) };
        let r = unsafe { &*rect };
        out.push((r.right - r.left, r.bottom - r.top));
        1
    }
    let mut out: Vec<(i32, i32)> = Vec::new();
    unsafe {
        EnumDisplayMonitors(std::ptr::null_mut(), std::ptr::null(), Some(each), &mut out as *mut _ as LPARAM);
    }
    out
}

#[cfg(not(windows))]
fn monitors() -> Vec<(i32, i32)> {
    Vec::new()
}

/// La position enregistrée tombe-t-elle sur un écran encore branché ? (repris de Fawkes : la
/// position est en points logiques, on essaie les facteurs d'échelle courants).
pub fn on_some_monitor(pos: [f32; 2]) -> bool {
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::POINT;
        use windows_sys::Win32::Graphics::Gdi::{MONITOR_DEFAULTTONULL, MonitorFromPoint};
        [1.0f32, 1.25, 1.5, 1.75, 2.0].iter().any(|s| {
            let pt = POINT { x: ((pos[0] + 60.0) * s) as i32, y: ((pos[1] + 12.0) * s) as i32 };
            unsafe { !MonitorFromPoint(pt, MONITOR_DEFAULTTONULL).is_null() }
        })
    }
    #[cfg(not(windows))]
    {
        let _ = pos;
        true
    }
}

/// Relève la configuration toutes les 2 s.
#[derive(Default)]
pub struct Screens {
    last: Option<(Instant, String)>,
}

impl Screens {
    pub fn current(&mut self) -> String {
        match &self.last {
            Some((_, s)) => s.clone(),
            None => signature(),
        }
    }

    /// La nouvelle configuration, si elle a changé depuis le dernier relevé.
    pub fn changed(&mut self) -> Option<String> {
        let due = self.last.as_ref().is_none_or(|(at, _)| at.elapsed() > Duration::from_secs(2));
        if !due {
            return None;
        }
        let now = signature();
        let previous = self.last.replace((Instant::now(), now.clone())).map(|(_, s)| s);
        match previous {
            Some(p) if p != now => Some(now),
            _ => None,
        }
    }
}
