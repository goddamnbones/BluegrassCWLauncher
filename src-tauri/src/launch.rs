// Option B (user 2026-11-18): for one launch the game's ProjectZomboid64.json is ours (ZombieBuddy agent, our JVM set,
// PZO's entry point); afterwards the player's own json goes back, so a Steam launch is whatever it was before.
// PZO's entry point rewrites the json early in every launch, so we restore 120 s after start and again after exit.
use crate::files::{self, GAME_EXE, GAME_JSON};
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

const RESTORE_AFTER_SECS: u64 = 120;

fn own_copy() -> PathBuf { files::data_dir().join("player-json-backup.json") }
fn state_file() -> PathBuf { files::data_dir().join("launch-state.json") }

/// What the player's own json (the Steam launch) is: saved once, refreshed when the player changes it themselves.
/// Any json we wrote, or that PZO rewrote during our launch, has its hash in launch-state "ours".
fn read_state() -> Value {
    fs::read_to_string(state_file()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(json!({}))
}

fn write_state(v: &Value) {
    let _ = fs::write(state_file(), serde_json::to_string_pretty(v).unwrap_or_default());
}

fn is_ours(state: &Value, hash: &str) -> bool {
    state["ours"].as_array().map(|a| a.iter().any(|h| h.as_str() == Some(hash))).unwrap_or(false)
}

fn remember_ours(hash: String) {
    let mut st = read_state();
    let mut list = st["ours"].as_array().cloned().unwrap_or_default();
    if !list.iter().any(|h| h.as_str() == Some(&hash)) {
        list.push(Value::String(hash));
    }
    let n = list.len();
    if n > 20 { list.drain(0..n - 20); }
    st["ours"] = Value::Array(list);
    write_state(&st);
}

/// Puts the player's own json back if the game folder still holds one of ours. Returns a note for the log, if any.
pub fn restore(game: &Path) -> Option<String> {
    let backup = fs::read(own_copy()).ok()?;
    let path = game.join(GAME_JSON);
    let _ = fs::remove_file(game.join("ProjectZomboid64.json.new")); // PZO's staged update json
    let cur = fs::read(&path).ok()?;
    let h = files::sha256_bytes(&cur);
    if cur == backup || !is_ours(&read_state(), &h) {
        return None;
    }
    fs::write(&path, &backup).ok()?;
    let mut st = read_state();
    st["pending"] = Value::Bool(false);
    write_state(&st);
    Some("Your own ProjectZomboid64.json is back (Steam launches as before)".into())
}

pub fn game_running() -> bool {
    Command::new("tasklist")
        .args(["/FI", &format!("IMAGENAME eq {GAME_EXE}"), "/NH"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).to_lowercase().contains(&GAME_EXE.to_lowercase()))
        .unwrap_or(false)
}

/// Default JVM set (Debugging\clients.md "Advice given" 2026-11-14): G1, 50 ms pause target, bigger code cache and
/// metaspace; heap by RAM (10 GB at 32 GB+, 8 GB at 16 GB+, else 6 GB).
pub fn default_jvm_args() -> String {
    let ram = files::total_ram_gb();
    let heap = if ram >= 30 { 10 } else if ram >= 15 { 8 } else { 6 };
    format!("-Xms{heap}g -Xmx{heap}g -XX:+UseG1GC -XX:MaxGCPauseMillis=50 -XX:ReservedCodeCacheSize=512m -XX:MetaspaceSize=512m")
}

// flags our set (or the launch) decides; dropped from the player's own vmArgs for this launch
fn replaced(a: &str) -> bool {
    let a = a.trim();
    a.starts_with("-Xms") || a.starts_with("-Xmx") || (a.starts_with("-XX:") && a.ends_with("GC") && a.contains("Use"))
        || ["MaxGCPauseMillis", "ReservedCodeCacheSize", "MetaspaceSize", "AlwaysPreTouch", "ZAllocationSpikeTolerance",
            "UseNUMA", "UseTypeSpeculation", "EnableDynamicAgentLoading", "StartFlightRecording"]
            .iter().any(|k| a.contains(k))
        || a.starts_with("-agentlib:pzo_native64") || a.starts_with("-agentlib:zbNative") || a.starts_with("-javaagent:")
}

/// The json for this launch, built from the player's own one.
fn launch_json(own: &Value, jvm_args: &str, zb: bool, pzo: bool) -> Value {
    let mut j = own.clone();
    let mut vm: Vec<Value> = own["vmArgs"].as_array().cloned().unwrap_or_default()
        .into_iter().filter(|v| v.as_str().map(|s| !replaced(s)).unwrap_or(false)).collect();
    for a in jvm_args.split_whitespace() {
        vm.push(Value::String(a.to_string()));
    }
    if zb { vm.push(json!("-agentlib:zbNative")); }
    if pzo { vm.push(json!("-XX:+EnableDynamicAgentLoading")); }
    j["vmArgs"] = Value::Array(vm);
    if let Some(o) = j.as_object_mut() { o.remove("windows"); } // per-OS GC flags would clash with ours
    let mut cp: Vec<Value> = vec![json!(".")];
    if pzo { cp.push(json!("PZOptimEngine.jar")); }
    if zb { cp.push(json!("ZombieBuddy.jar")); }
    cp.push(json!("projectzomboid.jar"));
    j["classpath"] = Value::Array(cp);
    j["mainClass"] = json!(if pzo { "com/pzoptimizer/PZOEntrypoint" } else { "zombie/gameStates/MainScreenState" });
    j
}

pub fn launch(game: &Path, jvm_args: &str) -> Result<String, String> {
    if game_running() {
        return Err("Project Zomboid is already running".into());
    }
    let zb = files::component_state(game, "zombiebuddy") == "approved";
    let pzo = files::component_state(game, "pzo") == "approved";
    let path = game.join(GAME_JSON);
    let cur = fs::read(&path).map_err(|e| format!("can't read {GAME_JSON}: {e}"))?;
    let h = files::sha256_bytes(&cur);
    // the folder holds the player's own json unless it's one of ours left from a launch that didn't get restored
    if !is_ours(&read_state(), &h) {
        fs::write(own_copy(), &cur).map_err(|e| format!("can't save your json: {e}"))?;
    }
    let own: Value = serde_json::from_slice(&fs::read(own_copy()).map_err(|e| e.to_string())?)
        .map_err(|e| format!("your {GAME_JSON} isn't valid json: {e}"))?;
    let j = launch_json(&own, jvm_args, zb, pzo);
    let text = serde_json::to_string_pretty(&j).map_err(|e| e.to_string())?;
    remember_ours(files::sha256_bytes(text.as_bytes()));
    fs::write(&path, &text).map_err(|e| format!("can't write {GAME_JSON}: {e}"))?;
    let mut st = read_state();
    st["pending"] = Value::Bool(true);
    write_state(&st);

    let child = Command::new(game.join(GAME_EXE)).current_dir(game).spawn();
    let mut child = match child {
        Ok(c) => c,
        Err(e) => {
            restore(game);
            return Err(format!("couldn't start the game: {e}"));
        }
    };
    let g = game.to_path_buf();
    std::thread::spawn(move || {
        // record what PZO rewrites the json to during start-up, then put the player's own back
        for _ in 0..(RESTORE_AFTER_SECS / 2) {
            std::thread::sleep(Duration::from_secs(2));
            if let Ok(b) = fs::read(g.join(GAME_JSON)) {
                let bh = files::sha256_bytes(&b);
                let own = fs::read(own_copy()).ok();
                if own.as_deref() != Some(&b[..]) { remember_ours(bh); }
            }
            if let Ok(Some(_)) = child.try_wait() { break; }
        }
        restore(&g);
        let _ = child.wait();
        // PZO moves a staged update over the json shortly after exit; wait, then make sure the player's own is back
        std::thread::sleep(Duration::from_secs(20));
        if let Ok(b) = fs::read(g.join(GAME_JSON)) {
            if fs::read(own_copy()).ok().as_deref() != Some(&b[..]) { remember_ours(files::sha256_bytes(&b)); }
        }
        restore(&g);
    });
    Ok(format!(
        "Started: ZombieBuddy {}, PZO {}",
        if zb { "on" } else { "off (not installed)" },
        if pzo { "on" } else { "off (not installed)" }
    ))
}
