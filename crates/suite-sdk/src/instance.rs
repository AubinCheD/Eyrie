//! Instance unique et identité de processus.
//!
//! Règle de Fawkes (§3.1, §3.3) : un module ne tourne jamais deux fois sur le même dossier d'état.
//! Quand la suite héberge un module, elle prend **le même verrou** que son exécutable autonome : un
//! exe lancé ensuite voit le module déjà ouvert, et la suite ne démarre pas un module dont l'exe
//! tourne déjà.

/// Un mutex nommé Windows, tenu tant que la valeur vit.
pub struct NamedMutex {
    #[cfg(windows)]
    handle: windows_sys::Win32::Foundation::HANDLE,
    pub name: String,
}

// Le handle n'est qu'un numéro du noyau : il se ferme depuis n'importe quel fil.
unsafe impl Send for NamedMutex {}
unsafe impl Sync for NamedMutex {}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

impl NamedMutex {
    /// `None` si un autre processus tient déjà ce nom.
    pub fn try_acquire(name: &str) -> Option<NamedMutex> {
        #[cfg(windows)]
        unsafe {
            use windows_sys::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError};
            use windows_sys::Win32::System::Threading::CreateMutexW;
            let w = wide(name);
            let handle = CreateMutexW(std::ptr::null(), 0, w.as_ptr());
            if handle.is_null() {
                return None;
            }
            if GetLastError() == ERROR_ALREADY_EXISTS {
                CloseHandle(handle);
                return None;
            }
            Some(NamedMutex { handle, name: name.to_string() })
        }
        #[cfg(not(windows))]
        Some(NamedMutex { name: name.to_string() })
    }
}

impl Drop for NamedMutex {
    fn drop(&mut self) {
        #[cfg(windows)]
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.handle);
        }
    }
}

/// Les processus dont l'exécutable s'appelle `exe` (sans casse), hors le processus courant.
pub fn find_processes(exe: &str) -> Vec<u32> {
    let mut out = Vec::new();
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
        use windows_sys::Win32::System::Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
        };
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap == INVALID_HANDLE_VALUE {
            return out;
        }
        let mut e: PROCESSENTRY32W = std::mem::zeroed();
        e.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let me = std::process::id();
        let mut ok = Process32FirstW(snap, &mut e) != 0;
        while ok {
            let len = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(e.szExeFile.len());
            let name = String::from_utf16_lossy(&e.szExeFile[..len]);
            if name.eq_ignore_ascii_case(exe) && e.th32ProcessID != me {
                out.push(e.th32ProcessID);
            }
            ok = Process32NextW(snap, &mut e) != 0;
        }
        CloseHandle(snap);
    }
    #[cfg(not(windows))]
    let _ = exe;
    out
}

/// Demande poliment à un processus de se fermer : `WM_CLOSE` à ses fenêtres visibles, comme un clic
/// sur leur croix (l'application garde la main : sauvegarde, confirmation). Renvoie le nombre de
/// fenêtres prévenues.
pub fn ask_to_close(pid: u32) -> usize {
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::Foundation::{HWND, LPARAM};
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            EnumWindows, GetWindowThreadProcessId, IsWindowVisible, PostMessageW, WM_CLOSE,
        };
        struct Search {
            pid: u32,
            found: Vec<HWND>,
        }
        unsafe extern "system" fn each(hwnd: HWND, lparam: LPARAM) -> i32 {
            let s = unsafe { &mut *(lparam as *mut Search) };
            let mut pid = 0;
            unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
            if pid == s.pid && unsafe { IsWindowVisible(hwnd) } != 0 {
                s.found.push(hwnd);
            }
            1
        }
        let mut s = Search { pid, found: Vec::new() };
        EnumWindows(Some(each), &mut s as *mut Search as LPARAM);
        for h in &s.found {
            PostMessageW(*h, WM_CLOSE, 0, 0);
        }
        s.found.len()
    }
    #[cfg(not(windows))]
    {
        let _ = pid;
        0
    }
}

/// Nom d'un dossier pour un nom d'objet du noyau : barres obliques, minuscules, sans antislash.
pub fn dir_key(dir: &std::path::Path) -> String {
    dir.to_string_lossy().replace('\\', "/").trim_end_matches('/').to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_is_held_once() {
        let name = format!("Local\\suite-sdk-test-{}", std::process::id());
        let first = NamedMutex::try_acquire(&name);
        assert!(first.is_some());
        // Même processus : CreateMutexW rend le même objet et signale qu'il existe.
        assert!(NamedMutex::try_acquire(&name).is_none());
        drop(first);
        assert!(NamedMutex::try_acquire(&name).is_some());
    }

    #[test]
    fn dir_key_has_no_backslash() {
        assert_eq!(dir_key(std::path::Path::new(r"C:\Temp\Essai\")), "c:/temp/essai");
    }
}
