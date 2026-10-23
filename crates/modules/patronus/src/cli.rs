//! `superapp.exe lancer <tâche>` et `superapp.exe planifie <tâche>`.
//!
//! Dans la suite, Patronus inscrit `current_exe()` — donc `superapp.exe` — dans le Planificateur de
//! Windows et la clé `Run`. Ces deux commandes sont reprises de `Patronus/crates/patronus-app/src/
//! main.rs` (`run_cli`, `run_planned`), qui n'est pas une bibliothèque, avec le cœur de Patronus
//! (`patronus-core`) tel quel.

use std::path::{Path, PathBuf};

use patronus_core::engine::Engine;
use patronus_core::exec::Context;
use patronus_core::run::now;

/// `Some(code de sortie)` si la ligne de commande est une commande de Patronus.
pub fn cli(args: &[String]) -> Option<i32> {
    let mut data_dir = None;
    let mut rest = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--donnees" => data_dir = it.next().map(PathBuf::from),
            _ => rest.push(a.as_str()),
        }
    }
    let run = |f: fn(&Path, &str) -> i32, id: &str| {
        attach_console();
        let dir = data_dir.clone().unwrap_or_else(patronus_core::fsutil::data_dir);
        f(&dir, id)
    };
    match rest.as_slice() {
        ["lancer", id] => Some(run(run_cli, id)),
        ["planifie", id] => Some(run(run_planned, id)),
        _ => None,
    }
}

fn attach_console() {
    #[cfg(windows)]
    unsafe {
        windows_sys::Win32::System::Console::AttachConsole(windows_sys::Win32::System::Console::ATTACH_PARENT_PROCESS);
    }
}

fn run_cli(data_dir: &Path, task_id: &str) -> i32 {
    let mut engine = Engine::open_headless(data_dir, now());
    match engine.loaded.iter().find(|l| l.id == task_id) {
        None => {
            let known: Vec<_> = engine.loaded.iter().map(|l| l.id.as_str()).collect();
            eprintln!("Tâche inconnue : {task_id}. Tâches : {}", known.join(", "));
            return 2;
        }
        Some(l) if l.task.is_err() => {
            let why = l.task.as_ref().err().cloned().unwrap_or_default();
            eprintln!("Tâche illisible : {task_id} ({why}) ; corriger le fichier {}", l.path.display());
            return 2;
        }
        Some(_) => {}
    }
    let before: Vec<String> = engine.runs.iter().map(|r| r.id.clone()).collect();
    let launched = engine.try_launch(
        task_id,
        Context { trigger: "ligne de commande".into(), files: vec![], ..Default::default() },
        now(),
    );
    let Some(id) = engine.runs.iter().find(|r| r.tache == task_id && !before.contains(&r.id)).map(|r| r.id.clone())
    else {
        eprintln!("{}", launched.err().unwrap_or_default());
        return 3;
    };
    while engine.is_running(task_id) {
        std::thread::sleep(std::time::Duration::from_millis(100));
        engine.tick(now());
    }
    let Some(run) = engine.runs.iter().find(|r| r.id == id) else { return 1 };
    println!("{} : {}\n{}", run.tache_nom, run.statut.label(), run.resume);
    if run.statut.is_ok() { 0 } else { 1 }
}

/// La fenêtre de Patronus (seule, ou dans la suite) tourne-t-elle sur ce dossier ?
fn window_running(data_dir: &Path) -> bool {
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Threading::{OpenMutexW, SYNCHRONIZATION_SYNCHRONIZE};
        let name: Vec<u16> = super::instance_name(data_dir).encode_utf16().chain(std::iter::once(0)).collect();
        let h = OpenMutexW(SYNCHRONIZATION_SYNCHRONIZE, 0, name.as_ptr());
        if h.is_null() {
            return false;
        }
        CloseHandle(h);
        true
    }
    #[cfg(not(windows))]
    {
        let _ = data_dir;
        false
    }
}

fn planner_log(data_dir: &Path, task_id: &str, text: &str) {
    use std::io::Write;
    let path = data_dir.join("planificateur.log");
    trim_log(&path, 256 * 1024);
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(f, "{} {task_id} : {text}", now().format("%Y-%m-%d %H:%M:%S"));
    }
}

fn trim_log(path: &Path, max: u64) {
    if std::fs::metadata(path).is_ok_and(|m| m.len() > max)
        && let Ok(old) = std::fs::read(path)
    {
        let half = old.len() / 2;
        let cut = old[half..].iter().position(|&b| b == b'\n').map_or(old.len(), |i| half + i + 1);
        let _ = std::fs::write(path, &old[cut..]);
    }
}

fn run_planned(data_dir: &Path, task_id: &str) -> i32 {
    if window_running(data_dir) {
        planner_log(data_dir, task_id, "fenêtre ouverte : elle tient elle-même ses échéances");
        return 0;
    }
    let mut engine = Engine::open_headless(data_dir, now());
    let at = match engine.planned_run(task_id, now()) {
        Ok(at) => at,
        Err(why) => {
            planner_log(data_dir, task_id, &format!("rien à faire ({why})"));
            return 0;
        }
    };
    let trigger = format!("Planificateur de Windows (échéance de {})", at.format("%H:%M"));
    if let Err(why) = engine.try_launch(task_id, Context { trigger, ..Default::default() }, now()) {
        planner_log(data_dir, task_id, &format!("non lancée : {why}"));
        return 3;
    }
    while engine.is_running(task_id) {
        std::thread::sleep(std::time::Duration::from_millis(200));
        engine.tick(now());
    }
    let statut = engine.runs.iter().find(|r| r.tache == task_id).map(|r| r.statut);
    planner_log(data_dir, task_id, &format!("exécutée : {}", statut.map_or("?", |s| s.label())));
    if statut.is_some_and(|s| s.is_ok()) { 0 } else { 1 }
}

#[cfg(test)]
mod tests {
    #[test]
    fn only_patronus_commands_are_taken() {
        assert_eq!(super::cli(&[]), None);
        assert_eq!(super::cli(&["--relais".into()]), None);
        assert_eq!(super::cli(&["lancer".into()]), None);
    }
}
