// Bluegrass launcher (2026-11-18, docs in README.md). The page only calls the commands below; it never loads
// anything remote. All downloads are pinned files checked against their SHA-256 (files.rs).
mod diag;
mod files;
mod launch;

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Default, Clone)]
struct Settings {
    game_dir: Option<String>,
    /// Free-text extra flags (0.1.8: heap and GC moved to heap_gb / gc; before that this held the whole set).
    jvm_args: Option<String>,
    /// JVM tuner (user 11-20). None = the default for this PC's RAM (launch::heap_for_ram).
    #[serde(default)]
    heap_gb: Option<u64>,
    /// launch::GCS: "g1" (default), "zgc", "shen", "shen_gen".
    #[serde(default)]
    gc: Option<String>,
    /// Advanced (0.1.11): None = launch::Tune default
    #[serde(default)]
    pause_ms: Option<u64>,
    #[serde(default)]
    code_mb: Option<u64>,
    #[serde(default)]
    meta_mb: Option<u64>,
    /// Manual override (0.1.11): the whole line as typed; guided choices off
    #[serde(default)]
    manual: bool,
    #[serde(default)]
    manual_line: Option<String>,
    /// Keep (true) / drop (false) per stray flag from the player's own json, asked once per flag.
    #[serde(default)]
    flags: BTreeMap<String, bool>,
    #[serde(default)]
    jfr: bool,
    // Diagnostics panel (user 11-19, item 283); stay ticked until the player unticks them
    #[serde(default)]
    nmt: bool,
    #[serde(default)]
    no_pzo: bool,
    #[serde(default)]
    no_handoff: bool,
    #[serde(default)]
    no_fsr: bool,
}

fn settings_path() -> PathBuf { files::data_dir().join("settings.json") }

fn load() -> Settings {
    let mut s: Settings = fs::read_to_string(settings_path()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
    if migrate(&mut s) { let _ = save(&s); }
    s
}

/// 0.1.7 and older kept heap + GC inside jvm_args: move them to the tuner's settings once, keep the rest as extras.
fn migrate(s: &mut Settings) -> bool {
    if s.heap_gb.is_some() || s.gc.is_some() { return false; }
    let Some(old) = s.jvm_args.clone() else { return false };
    if !old.split_whitespace().any(launch::guided_owned) { return false; }
    for a in old.split_whitespace() {
        if let Some(v) = a.strip_prefix("-Xmx") { s.heap_gb = heap_from(v); }
    }
    // the old default stays "default", so it follows the RAM table
    if s.heap_gb == Some(launch::heap_for_ram(files::total_ram_gb())) { s.heap_gb = None; }
    if old.contains("-XX:+UseZGC") { s.gc = Some("zgc".into()); } else { s.gc = Some("g1".into()); }
    let rest: Vec<&str> = old.split_whitespace().filter(|a| !launch::guided_owned(a)).collect();
    s.jvm_args = if rest.is_empty() { None } else { Some(rest.join(" ")) };
    true
}

/// "12g" / "12288m" / "12G" -> 12
fn heap_from(v: &str) -> Option<u64> {
    let v = v.to_ascii_lowercase();
    if let Some(n) = v.strip_suffix('g') { return n.parse().ok(); }
    if let Some(n) = v.strip_suffix('m') { return n.parse::<u64>().ok().map(|m| (m + 512) / 1024); }
    None
}

fn heap_of(s: &Settings) -> u64 { s.heap_gb.unwrap_or_else(|| launch::heap_for_ram(files::total_ram_gb())) }
fn gc_of(s: &Settings) -> String { s.gc.clone().unwrap_or_else(|| "g1".into()) }

/// Flags typed in the free-text field that the guided heap/GC choices override.
fn ignored_extras(s: &Settings) -> Vec<String> {
    s.jvm_args.as_deref().unwrap_or("").split_whitespace().filter(|a| launch::guided_owned(a)).map(String::from).collect()
}

fn tune_of(s: &Settings) -> launch::Tune {
    let d = launch::Tune::default();
    launch::Tune { pause_ms: s.pause_ms.unwrap_or(d.pause_ms), code_mb: s.code_mb.unwrap_or(d.code_mb), meta_mb: s.meta_mb.unwrap_or(d.meta_mb) }
}

fn extras_of(s: &Settings) -> Vec<String> {
    s.jvm_args.as_deref().unwrap_or("").split_whitespace().filter(|a| !launch::guided_owned(a)).map(String::from).collect()
}

/// Guided set, then the extras minus anything the guided set owns.
fn guided_line(s: &Settings) -> String {
    let mut v = vec![launch::guided_args(heap_of(s), &gc_of(s), &tune_of(s))];
    v.extend(extras_of(s));
    v.join(" ")
}

/// The JVM flags for a launch: the manual line when the override is on, else the guided line.
fn launch_args(s: &Settings) -> String {
    match (&s.manual, &s.manual_line) {
        (true, Some(l)) if !l.trim().is_empty() => l.clone(),
        _ => guided_line(s),
    }
}

/// -Xmx of the manual line (for the memory warning), else the guided heap.
fn launch_heap(s: &Settings) -> u64 {
    if s.manual {
        if let Some(gb) = launch_args(s).split_whitespace().filter_map(|a| a.strip_prefix("-Xmx")).filter_map(heap_from).last() { return gb; }
    }
    heap_of(s)
}

/// The flag check runs the line the launch will use: kept flags from the player's own json first, then ours.
fn check_line(s: &Settings, game: Option<&std::path::Path>, ours: &str) -> Result<Option<String>, String> {
    let mut v: Vec<String> = game.map(launch::stray_flags).unwrap_or_default().into_iter().filter(|f| s.flags.get(f) != Some(&false)).collect();
    v.push(ours.to_string());
    launch::check_flags(game, &v.join(" "))
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
    /// the whole launch line: guided set + extras (user 11-20: keep showing it in the JVM box)
    jvm_args: String,
    /// guided set + extras ("Start from guided line" in manual override)
    guided: String,
    /// the extras box (guided mode) or the full typed line (manual)
    box_text: String,
    manual: bool,
    pause_ms: u64,
    code_mb: u64,
    meta_mb: u64,
    /// game memory setting (input box)
    heap_set: u64,
    /// heap the launch uses (manual: its -Xmx); the warning uses this
    heap_gb: u64,
    /// the heap for this PC's RAM; the page warns above it (user 11-20: warn only)
    default_heap: u64,
    gc: String,
    /// extras the guided choices override (shown as ignored)
    ignored: Vec<String>,
    /// flags from the player's own json: (flag, Some(keep) or None = not asked yet)
    stray: Vec<(String, Option<bool>)>,
    running: bool,
    jfr: bool,
    jfr_dir: String,
    nmt: bool,
    no_pzo: bool,
    no_handoff: bool,
    no_fsr: bool,
    potato: bool,
    ram_gb: u64,
    warnings: Vec<String>,
}

#[tauri::command]
async fn get_status() -> Status {
    let s = load();
    let game = game_dir(&s);
    let mut warnings = Vec::new();
    // one process scan per status (the page polls)
    let running = launch::game_running();
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
        if !running {
            if let Some(note) = launch::restore(g) { warnings.push(note); }
        }
    }
    let stray = game.as_ref().map(|g| launch::stray_flags(g)).unwrap_or_default()
        .into_iter().map(|f| { let k = s.flags.get(&f).copied(); (f, k) }).collect();
    Status {
        launcher: env!("CARGO_PKG_VERSION"),
        game_dir: game.as_ref().map(|g| g.display().to_string()),
        components: comps,
        jvm_args: launch_args(&s),
        guided: guided_line(&s),
        box_text: if s.manual { launch_args(&s) } else { extras_of(&s).join(" ") },
        manual: s.manual,
        pause_ms: tune_of(&s).pause_ms,
        code_mb: tune_of(&s).code_mb,
        meta_mb: tune_of(&s).meta_mb,
        heap_set: heap_of(&s),
        heap_gb: launch_heap(&s),
        default_heap: launch::heap_for_ram(files::total_ram_gb()),
        gc: gc_of(&s),
        ignored: ignored_extras(&s),
        stray,
        running,
        jfr: s.jfr,
        jfr_dir: launch::JFR_DIR.into(),
        nmt: s.nmt,
        no_pzo: s.no_pzo,
        no_handoff: s.no_handoff,
        no_fsr: s.no_fsr,
        potato: potato_file(game.as_deref()).is_file(),
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
    if let Some(p) = launch::arg_problem(&t) { return Err(p); }
    let game = game_dir(&s);
    // 0.1.11 (user 11-21): manual override saves the line as typed; otherwise the box holds extras only and flags the
    // guided choices own are ignored (guided wins). Either way the game's Java checks the line first.
    let mut ign: Vec<String> = Vec::new();
    let full = if s.manual {
        if t.is_empty() { return Err("Type the full JVM line, or turn manual override off".into()); }
        t.clone()
    } else {
        ign = t.split_whitespace().filter(|a| launch::guided_owned(a)).map(String::from).collect();
        let extras: Vec<&str> = t.split_whitespace().filter(|a| !launch::guided_owned(a)).collect();
        let mut v = vec![launch::guided_args(heap_of(&s), &gc_of(&s), &tune_of(&s))];
        v.extend(extras.iter().map(|e| e.to_string()));
        v.join(" ")
    };
    let note = check_line(&s, game.as_deref(), &full)?;
    if s.manual {
        s.manual_line = Some(t);
    } else {
        let extras: Vec<&str> = t.split_whitespace().filter(|a| !launch::guided_owned(a)).collect();
        s.jvm_args = if extras.is_empty() { None } else { Some(extras.join(" ")) };
    }
    save(&s)?;
    let mut msg = match note { None => "Flags checked with the game's Java: OK; JVM flags saved".to_string(), Some(n) => format!("JVM flags saved ({n})") };
    if !ign.is_empty() {
        msg += &format!("; ignored because the choices above decide them: {}", ign.join(" "));
    }
    Ok(msg)
}

#[tauri::command]
async fn set_manual(on: bool) -> Result<String, String> {
    let mut s = load();
    s.manual = on;
    if on && s.manual_line.as_deref().map(|l| l.trim().is_empty()).unwrap_or(true) { s.manual_line = Some(guided_line(&s)); }
    save(&s)?;
    Ok(if on { "Manual override on: the JVM line is used exactly as saved".into() } else { "Manual override off: memory and garbage collector choices are used again".into() })
}

/// Advanced fields (0.1.11). Defaults are stored as None so they follow future default changes.
#[tauri::command]
async fn set_tune(pause_ms: u64, code_mb: u64, meta_mb: u64) -> Result<String, String> {
    if !(1..=1000).contains(&pause_ms) { return Err("G1 pause target must be 1 to 1000 ms".into()); }
    if !(64..=2048).contains(&code_mb) { return Err("Code cache must be 64 to 2048 MB".into()); }
    if !(64..=4096).contains(&meta_mb) { return Err("Metaspace must be 64 to 4096 MB".into()); }
    let d = launch::Tune::default();
    let mut s = load();
    s.pause_ms = (pause_ms != d.pause_ms).then_some(pause_ms);
    s.code_mb = (code_mb != d.code_mb).then_some(code_mb);
    s.meta_mb = (meta_mb != d.meta_mb).then_some(meta_mb);
    save(&s)?;
    Ok(format!("Advanced: pause target {pause_ms} ms, code cache {code_mb} MB, metaspace {meta_mb} MB from the next launch"))
}

// JVM tuner (user 11-20): guided heap (warn above the RAM default, never block) and GC (G1 or ZGC).
#[tauri::command]
async fn set_heap(gb: u64) -> Result<String, String> {
    if !(2..=64).contains(&gb) { return Err("Game memory must be between 2 and 64 GB".into()); }
    let mut s = load();
    let def = launch::heap_for_ram(files::total_ram_gb());
    s.heap_gb = if gb == def { None } else { Some(gb) };
    save(&s)?;
    Ok(if gb > def {
        format!("Game memory {gb} GB from the next launch. That's above the {def} GB recommended for this PC: Windows can run out of memory and the game can crash")
    } else {
        format!("Game memory {gb} GB from the next launch")
    })
}

#[tauri::command]
async fn set_gc(gc: String) -> Result<String, String> {
    if !launch::GCS.contains(&gc.as_str()) { return Err(format!("unknown garbage collector {gc}")); }
    let mut s = load();
    s.gc = Some(gc.clone());
    save(&s)?;
    Ok(format!("Garbage collector {} from the next launch", launch::gc_name(&gc)))
}

/// Keep/drop answers for stray flags from the player's own json; remembered per flag (user 11-20).
#[tauri::command]
async fn set_flags(choices: BTreeMap<String, bool>) -> Result<String, String> {
    let mut s = load();
    let dropped = choices.values().filter(|k| !**k).count();
    for (f, k) in choices { s.flags.insert(f, k); }
    save(&s)?;
    Ok(format!("Flag choices saved ({dropped} dropped from launcher launches; your own game settings aren't changed)"))
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

// Potato Mode (user 11-18): a flag file BluegrassCore reads at join (Compat_UIPerf.lua): lower graphics settings on
// Bluegrass only, the player's own come back when they leave. The file existing is the setting; nothing in settings.json.
fn potato_file(game: Option<&std::path::Path>) -> PathBuf {
    diag::zomboid_lua_dir(game).join("bluegrass_potato.txt")
}

// Diagnostics panel (user 11-19): staff ask a player to tick one when chasing a problem. The two PZO switches change
// PZO's own files (diag.rs) at once when the game is closed, and again at every launch.
#[tauri::command]
async fn set_diag(key: String, on: bool) -> Result<String, String> {
    let mut s = load();
    let what = match key.as_str() {
        "nmt" => { s.nmt = on; "Native memory tracking" }
        "no_pzo" => { s.no_pzo = on; "Launch without PZO" }
        "no_handoff" => { s.no_handoff = on; "PZO texture handoff off" }
        "no_fsr" => { s.no_fsr = on; "PZO FSR off" }
        _ => return Err(format!("unknown switch {key}")),
    };
    save(&s)?;
    let mut msg = format!("{what}: {} from the next launch", if on { "on" } else { "off" });
    if (key == "no_handoff" || key == "no_fsr") && !launch::game_running() {
        for n in diag::apply_pzo(game_dir(&s).as_deref(), s.no_handoff, s.no_fsr)? { msg += "; "; msg += &n; }
    }
    Ok(msg)
}

#[tauri::command]
async fn set_potato(on: bool) -> Result<String, String> {
    let f = potato_file(game_dir(&load()).as_deref());
    if on {
        if let Some(dir) = f.parent() { fs::create_dir_all(dir).map_err(|e| format!("can't create {}: {e}", dir.display()))?; }
        fs::write(&f, "on, written by the Bluegrass launcher\n").map_err(|e| format!("can't write {}: {e}", f.display()))?;
        Ok("Potato Mode on from your next Bluegrass join".into())
    } else {
        if f.exists() { fs::remove_file(&f).map_err(|e| format!("can't remove {}: {e}", f.display()))?; }
        Ok("Potato Mode off from your next Bluegrass join".into())
    }
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
async fn uninstall(component: String) -> Result<String, String> {
    let game = game_dir(&load()).ok_or("Game folder not found; set it first")?;
    if launch::game_running() {
        return Err("Close Project Zomboid first".into());
    }
    tauri::async_runtime::spawn_blocking(move || files::uninstall(&game, &component))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn launch_game() -> Result<String, String> {
    let s = load();
    let game = game_dir(&s).ok_or("Game folder not found; set it first")?;
    let args = launch_args(&s);
    let notes = if launch::game_running() { Vec::new() } else { diag::apply_pzo(Some(&game), s.no_handoff, s.no_fsr)? };
    // not-yet-answered stray flags are kept (the page asks before launching)
    let drop = launch::stray_flags(&game).into_iter().filter(|f| s.flags.get(f) == Some(&false)).collect();
    let o = launch::Opts { jfr: s.jfr, nmt: s.nmt, no_pzo: s.no_pzo, drop };
    let mut msg = launch::launch(&game, &args, &o)?;
    if s.no_handoff { msg += ", PZO texture handoff off"; }
    if s.no_fsr { msg += ", PZO FSR off"; }
    for n in notes { msg += "; "; msg += &n; }
    Ok(msg)
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
        .invoke_handler(tauri::generate_handler![get_status, set_game_dir, set_jvm_args, set_manual, set_tune, set_heap, set_gc, set_flags, set_jfr, set_diag, set_potato, install, uninstall, launch_game,
            check_update, apply_update])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heap_units() {
        assert_eq!(heap_from("20g"), Some(20));
        assert_eq!(heap_from("12288m"), Some(12));
        assert_eq!(heap_from("x"), None);
    }

    #[test]
    fn migrates_old_combined_field() {
        let mut s = Settings { jvm_args: Some("-Xms20g -Xmx20g -XX:+UseZGC -XX:MaxGCPauseMillis=50 -XX:+UseSuperWord".into()), ..Default::default() };
        assert!(migrate(&mut s));
        assert_eq!(s.heap_gb, Some(20));
        assert_eq!(s.gc.as_deref(), Some("zgc"));
        assert_eq!(s.jvm_args.as_deref(), Some("-XX:+UseSuperWord"));
        assert!(!migrate(&mut s)); // once only
        assert_eq!(launch_args(&s), "-Xms20g -Xmx20g -XX:+UseZGC -XX:ReservedCodeCacheSize=512m -XX:MetaspaceSize=512m -XX:+UseSuperWord");
    }

    #[test]
    fn guided_wins_over_typed() {
        let s = Settings { heap_gb: Some(10), gc: Some("g1".into()), jvm_args: Some("-Xmx20g -XX:+UseZGC -Dfoo=1".into()), ..Default::default() };
        assert_eq!(ignored_extras(&s), ["-Xmx20g", "-XX:+UseZGC"]);
        assert!(launch_args(&s).ends_with("-XX:MetaspaceSize=512m -Dfoo=1"));
    }

    #[test]
    fn manual_override_used_as_typed() {
        let mut s = Settings { heap_gb: Some(10), manual: true, manual_line: Some("-Xms20g -Xmx20g -XX:+UseShenandoahGC".into()), ..Default::default() };
        assert_eq!(launch_args(&s), "-Xms20g -Xmx20g -XX:+UseShenandoahGC");
        assert_eq!(launch_heap(&s), 20);
        s.manual = false;
        assert!(launch_args(&s).starts_with("-Xms10g -Xmx10g -XX:+UseG1GC"));
        assert_eq!(launch_heap(&s), 10);
    }

    #[test]
    fn advanced_values_in_guided_line() {
        let s = Settings { heap_gb: Some(8), gc: Some("g1".into()), pause_ms: Some(80), meta_mb: Some(768), ..Default::default() };
        assert_eq!(guided_line(&s), "-Xms8g -Xmx8g -XX:+UseG1GC -XX:MaxGCPauseMillis=80 -XX:ReservedCodeCacheSize=512m -XX:MetaspaceSize=768m");
    }
}
