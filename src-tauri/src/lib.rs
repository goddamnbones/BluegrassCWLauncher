// Bluegrass launcher (2026-11-18, docs in README.md). The page only calls the commands below; it never loads
// anything remote. All downloads are pinned files checked against their SHA-256 (files.rs).
mod files;
mod launch;

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Default, Clone)]
struct Settings {
    game_dir: Option<String>,
    jvm_args: Option<String>,
    #[serde(default)]
    jfr: bool,
}

fn settings_path() -> PathBuf { files::data_dir().join("settings.json") }

fn load() -> Settings {
    fs::read_to_string(settings_path()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

fn save(s: &Settings) -> Result<(), String> {
    fs::write(settings_path(), serde_json::to_string_pretty(s).unwrap_or_default()).map_err(|e| e.to_string())
}

fn game_dir(s: &Settings) -> Option<PathBuf> {
    s.game_dir.as_ref().map(PathBuf::from).filter(|d| files::valid_game_dir(d)).or_else(files::find_game_dir)
}

#[derive(Serialize)]
struct Component {
    id: &'static str,
    name: &'static str,
    version: &'static str,
    state: String,
}

#[derive(Serialize)]
struct Status {
    launcher: &'static str,
    game_dir: Option<String>,
    components: Vec<Component>,
    jvm_args: String,
    default_jvm_args: String,
    running: bool,
    jfr: bool,
    jfr_dir: String,
    ram_gb: u64,
    warnings: Vec<String>,
}

#[tauri::command]
async fn get_status() -> Status {
    let s = load();
    let game = game_dir(&s);
    let mut warnings = Vec::new();
    let comps = [("zombiebuddy", "ZombieBuddy"), ("pzo", "PZO")]
        .iter()
        .map(|(id, name)| Component {
            id,
            name,
            version: files::PINNED.iter().find(|p| p.component == *id).map(|p| p.version).unwrap_or("?"),
            state: game.as_ref().map(|g| files::component_state(g, id)).unwrap_or_else(|| "missing".into()),
        })
        .collect();
    if let Some(g) = &game {
        let stray = files::stray_jvm_dlls(g);
        if !stray.is_empty() {
            warnings.push(format!("Stray Java files in the game folder ({}) can break ZombieBuddy's networking. \
                Steam > Verify files, or remove them.", stray.join(", ")));
        }
        if !launch::game_running() {
            if let Some(note) = launch::restore(g) { warnings.push(note); }
        }
    }
    let def = launch::default_jvm_args();
    Status {
        launcher: env!("CARGO_PKG_VERSION"),
        game_dir: game.map(|g| g.display().to_string()),
        components: comps,
        jvm_args: s.jvm_args.clone().unwrap_or_else(|| def.clone()),
        default_jvm_args: def,
        running: launch::game_running(),
        jfr: s.jfr,
        jfr_dir: launch::JFR_DIR.into(),
        ram_gb: files::total_ram_gb(),
        warnings,
    }
}

#[tauri::command]
async fn set_game_dir(dir: String) -> Result<String, String> {
    let d = PathBuf::from(dir.trim());
    if !files::valid_game_dir(&d) {
        return Err("That folder has no ProjectZomboid64.exe and ProjectZomboid64.json".into());
    }
    let mut s = load();
    s.game_dir = Some(d.display().to_string());
    save(&s)?;
    Ok("Game folder saved".into())
}

#[tauri::command]
async fn set_jvm_args(args: String) -> Result<String, String> {
    let mut s = load();
    let t = args.split_whitespace().collect::<Vec<_>>().join(" ");
    if t.split_whitespace().any(|a| !a.starts_with('-')) {
        return Err("Every JVM setting starts with '-'".into());
    }
    s.jvm_args = if t.is_empty() { None } else { Some(t) };
    save(&s)?;
    Ok("JVM settings saved".into())
}

// Opt-in JFR recording (user 11-18): staff ask a player to tick it when a problem needs a Java profile.
#[tauri::command]
async fn set_jfr(on: bool) -> Result<String, String> {
    let mut s = load();
    s.jfr = on;
    save(&s)?;
    Ok(if on {
        format!("JFR recording on from the next launch: files go to {}. Quit through the game menu, or no file is written", launch::JFR_DIR)
    } else {
        "JFR recording off from the next launch".into()
    })
}

#[tauri::command]
async fn install(component: String) -> Result<String, String> {
    let game = game_dir(&load()).ok_or("Game folder not found; set it first")?;
    if launch::game_running() {
        return Err("Close Project Zomboid first".into());
    }
    tauri::async_runtime::spawn_blocking(move || files::install(&game, &component))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn launch_game() -> Result<String, String> {
    let s = load();
    let game = game_dir(&s).ok_or("Game folder not found; set it first")?;
    let args = s.jvm_args.clone().unwrap_or_else(launch::default_jvm_args);
    launch::launch(&game, &args, s.jfr)
}

// Self-update: Tauri's updater checks our GitHub release's latest.json and refuses any installer whose signature
// doesn't match the public key in tauri.conf.json (private key: Resources\keys, never in a repo).
#[tauri::command]
async fn check_update(app: tauri::AppHandle) -> Result<Option<(String, String)>, String> {
    use tauri_plugin_updater::UpdaterExt;
    let up = app.updater().map_err(|e| e.to_string())?;
    match up.check().await {
        // version + the release's notes (latest.json "notes": the change summary shown under the update button)
        Ok(Some(u)) => Ok(Some((u.version.clone(), u.body.clone().unwrap_or_default()))),
        Ok(None) => Ok(None),
        Err(e) => Err(format!("update check failed: {e}")),
    }
}

#[tauri::command]
async fn apply_update(app: tauri::AppHandle) -> Result<String, String> {
    use tauri_plugin_updater::UpdaterExt;
    if launch::game_running() {
        return Err("Close Project Zomboid first".into());
    }
    let up = app.updater().map_err(|e| e.to_string())?;
    let u = up.check().await.map_err(|e| e.to_string())?.ok_or("No update available")?;
    u.download_and_install(|_, _| {}, || {}).await.map_err(|e| format!("update failed: {e}"))?;
    app.restart();
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_updater::Builder::new().build())
        .invoke_handler(tauri::generate_handler![get_status, set_game_dir, set_jvm_args, set_jfr, install, launch_game,
            check_update, apply_update])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
