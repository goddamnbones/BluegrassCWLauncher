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

/// Opt-in JFR (user 11-18). Same folder + settings as Resources\Debugging\clients.md "JFR on a client"; %t puts the
/// start time in the name so a later launch doesn't overwrite the last recording. Written when the game quits normally.
pub const JFR_DIR: &str = "C:\\BGjfr";
const JFR_ARG: &str = "-XX:StartFlightRecording=name=bg,settings=profile,method-profiling=high,maxage=20m,dumponexit=true,filename=C:/BGjfr/live-%t.jfr";
/// Diagnostics (user 11-19): lets `jcmd <pid> VM.native_memory summary` split the JVM's own native memory.
const NMT_ARG: &str = "-XX:NativeMemoryTracking=summary";

/// Per-launch switches from the launcher's settings.
pub struct Opts {
    pub jfr: bool,
    pub nmt: bool,
    /// Diagnostics "Launch without PZO": PZO stays installed, this launch skips it.
    pub no_pzo: bool,
    /// Flags from the player's own json they chose to drop (JVM tuner, user 11-20); left out of our launch only.
    pub drop: Vec<String>,
}

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

// Process list straight from Windows (no tasklist: that flashed a console window and froze the page, user 11-18).
pub fn game_running() -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
    };
    let want: Vec<u16> = GAME_EXE.to_lowercase().encode_utf16().collect();
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap == INVALID_HANDLE_VALUE {
            return false;
        }
        let mut e: PROCESSENTRY32W = std::mem::zeroed();
        e.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut found = false;
        let mut ok = Process32FirstW(snap, &mut e) != 0;
        while ok {
            let len = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(e.szExeFile.len());
            let name: Vec<u16> = String::from_utf16_lossy(&e.szExeFile[..len]).to_lowercase().encode_utf16().collect();
            if name == want {
                found = true;
                break;
            }
            ok = Process32NextW(snap, &mut e) != 0;
        }
        CloseHandle(snap);
        found
    }
}

/// Heap for a PC's RAM (GB). The page's RAM dropdown uses the same table (main.js RAM_HEAP); keep them matching.
/// 0.1.2: 4/6/8 GB heap at 8/12/16 GB RAM. 0.1.4 (user 11-18): 12 GB at 32 GB (was 10), 16 GB at 64 GB+ (was 12):
/// JustPunished (4K, zoomed out 2.5 while driving) filled a 10 GB heap, 53 GCs a minute, 0.6-1.1 s stutters.
pub fn heap_for_ram(ram: u64) -> u64 {
    if ram >= 60 { 16 } else if ram >= 30 { 12 } else if ram >= 15 { 8 } else if ram >= 11 { 6 } else { 4 }
}

/// JVM tuner (user 11-20, after Weber's out-of-memory crashes with 16/20 GB heaps on 32 GB RAM): heap and GC are
/// guided choices. G1 = Debugging\clients.md "Advice given" 2026-11-14 (50 ms pause target); ZGC for trials (no pause
/// target, ZGC ignores it). Both get the bigger code cache and metaspace.
/// 0.1.11 (user 11-21): Shenandoah classic + generational (Zulu 25 ships it) as equal choices; pause target, code
/// cache and metaspace are Advanced fields (pause target G1 only).
pub const GCS: [&str; 4] = ["g1", "zgc", "shen", "shen_gen"];

pub struct Tune { pub pause_ms: u64, pub code_mb: u64, pub meta_mb: u64 }
impl Default for Tune { fn default() -> Self { Tune { pause_ms: 50, code_mb: 512, meta_mb: 512 } } }

pub fn gc_name(gc: &str) -> &'static str {
    match gc { "zgc" => "ZGC", "shen" => "Shenandoah", "shen_gen" => "Shenandoah generational", _ => "G1" }
}

pub fn guided_args(heap: u64, gc: &str, t: &Tune) -> String {
    let gc_set = match gc {
        "zgc" => "-XX:+UseZGC".to_string(),
        "shen" => "-XX:+UseShenandoahGC".to_string(),
        "shen_gen" => "-XX:+UseShenandoahGC -XX:ShenandoahGCMode=generational".to_string(),
        _ => format!("-XX:+UseG1GC -XX:MaxGCPauseMillis={}", t.pause_ms),
    };
    format!("-Xms{heap}g -Xmx{heap}g {gc_set} -XX:ReservedCodeCacheSize={}m -XX:MetaspaceSize={}m", t.code_mb, t.meta_mb)
}

/// Flags the guided choices own. Typed in the free-text field they're ignored: guided wins (user 11-20).
/// Exact prefixes, so -XX:MaxMetaspaceSize etc. stay the player's (0.1.11).
pub fn guided_owned(a: &str) -> bool {
    let a = a.trim();
    a.starts_with("-Xms") || a.starts_with("-Xmx") || (a.starts_with("-XX:") && a.ends_with("GC") && a.contains("Use"))
        || ["-XX:MaxGCPauseMillis=", "-XX:ReservedCodeCacheSize=", "-XX:MetaspaceSize=", "-XX:ShenandoahGCMode="]
            .iter().any(|k| a.starts_with(k))
}

/// Flag check (user 11-21): run the game's own Java with the line + -version. Ok(None) = accepted,
/// Ok(Some(note)) = couldn't check (save anyway), Err = Java's own error text.
pub fn check_flags(game: Option<&Path>, line: &str) -> Result<Option<String>, String> {
    let Some(java) = game.map(|g| g.join("jre64").join("bin").join("java.exe")).filter(|j| j.is_file()) else {
        return Ok(Some("flags not checked: the game's java.exe wasn't found".into()));
    };
    let mut c = Command::new(&java);
    c.args(line.split_whitespace()).arg("-version");
    #[cfg(windows)]
    { use std::os::windows::process::CommandExt; c.creation_flags(0x0800_0000); } // CREATE_NO_WINDOW
    let out = match c.output() { Ok(o) => o, Err(e) => return Ok(Some(format!("flags not checked: {e}"))) };
    if out.status.success() { return Ok(None); }
    let err = String::from_utf8_lossy(&out.stderr);
    let lines: Vec<&str> = err.lines().map(str::trim).filter(|l| !l.is_empty()).take(4).collect();
    Err(format!("Not saved: the game's Java refused the flags: {}", lines.join(" / ")))
}

// flags our set (or the launch) decides; dropped from the player's own vmArgs for this launch
fn replaced(a: &str) -> bool {
    let a = a.trim();
    guided_owned(a)
        || ["AlwaysPreTouch", "ZAllocationSpikeTolerance",
            "UseNUMA", "UseTypeSpeculation", "EnableDynamicAgentLoading", "StartFlightRecording", "NativeMemoryTracking"]
            .iter().any(|k| a.contains(k))
        || a.starts_with("-agentlib:pzo_native64") || a.starts_with("-agentlib:zbNative") || a.starts_with("-javaagent:")
}

/// Flags the game, PZO or ZombieBuddy need: never asked about. There's no stock client json on the server, so this is
/// the vanilla 42.22 server json's args plus what PZO's ZomboidConfigMigrator always writes (user OK 11-20).
fn needed(a: &str) -> bool {
    let a = a.trim();
    ["--enable-native-access", "--add-exports", "--add-opens", "-Djava.library.path", "-Djava.awt.headless", "-Dzomboid.",
        "-XX:-CreateCoredumpOnCrash", "-XX:-OmitStackTraceInFastThrow"]
        .iter().any(|k| a.starts_with(k))
}

/// Flags in the player's own json (their Steam launch) that are neither needed nor ours (JVM tuner "stray flags", user
/// 11-20): the page asks keep/drop once per flag. PZO adds its "performance args" (UseSuperWord, inline flags, ...)
/// to the player's json, so those show up here too.
pub fn stray_flags(game: &Path) -> Vec<String> {
    let Ok(cur) = fs::read(game.join(GAME_JSON)) else { return Vec::new() };
    let own = if is_ours(&read_state(), &files::sha256_bytes(&cur)) { fs::read(own_copy()).unwrap_or_default() } else { cur };
    let Ok(v) = serde_json::from_slice::<Value>(&own) else { return Vec::new() };
    let mut out: Vec<String> = Vec::new();
    for a in v["vmArgs"].as_array().into_iter().flatten().filter_map(|x| x.as_str()) {
        let a = a.trim();
        if !a.is_empty() && !replaced(a) && !needed(a) && !out.iter().any(|o| o == a) { out.push(a.to_string()); }
    }
    out
}

/// The json for this launch, built from the player's own one.
fn launch_json(own: &Value, jvm_args: &str, drop: &[String], zb: bool, pzo: bool, jfr: bool, nmt: bool) -> Value {
    let mut j = own.clone();
    let mut vm: Vec<Value> = own["vmArgs"].as_array().cloned().unwrap_or_default()
        .into_iter().filter(|v| v.as_str().map(|s| !replaced(s) && !drop.iter().any(|d| d == s.trim())).unwrap_or(false)).collect();
    for a in jvm_args.split_whitespace() {
        vm.push(Value::String(a.to_string()));
    }
    if zb { vm.push(json!("-agentlib:zbNative")); }
    if pzo { vm.push(json!("-XX:+EnableDynamicAgentLoading")); }
    if jfr { vm.push(json!(JFR_ARG)); }
    if nmt { vm.push(json!(NMT_ARG)); }
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

pub fn launch(game: &Path, jvm_args: &str, o: &Opts) -> Result<String, String> {
    let jfr = o.jfr;
    if game_running() {
        return Err("Project Zomboid is already running".into());
    }
    // JFR doesn't create missing folders: no folder, no recording
    if jfr {
        fs::create_dir_all(JFR_DIR).map_err(|e| format!("can't create {JFR_DIR} for the JFR recording: {e}"))?;
    }
    let zb = files::component_state(game, "zombiebuddy") == "approved";
    let pzo_installed = files::component_state(game, "pzo") == "approved";
    let pzo = pzo_installed && !o.no_pzo;
    let path = game.join(GAME_JSON);
    let cur = fs::read(&path).map_err(|e| format!("can't read {GAME_JSON}: {e}"))?;
    let h = files::sha256_bytes(&cur);
    // the folder holds the player's own json unless it's one of ours left from a launch that didn't get restored
    if !is_ours(&read_state(), &h) {
        fs::write(own_copy(), &cur).map_err(|e| format!("can't save your json: {e}"))?;
    }
    let own: Value = serde_json::from_slice(&fs::read(own_copy()).map_err(|e| e.to_string())?)
        .map_err(|e| format!("your {GAME_JSON} isn't valid json: {e}"))?;
    let j = launch_json(&own, jvm_args, &o.drop, zb, pzo, jfr, o.nmt);
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
        "Started: ZombieBuddy {}, PZO {}{}",
        if zb { "on" } else { "off (not installed)" },
        if pzo { "on" } else if pzo_installed { "off (Diagnostics)" } else { "off (not installed)" },
        (if jfr { format!(", JFR recording to {JFR_DIR} (quit through the game menu)") } else { String::new() })
            + if o.nmt { ", native memory tracking on" } else { "" }
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    // Weber's crash-time args (hs_err_pid3864, 11-20)
    const WEBER: &str = "--enable-native-access=ALL-UNNAMED --add-exports=java.base/jdk.internal.misc=ALL-UNNAMED -Djava.library.path=win64/;.;natives/ -Dzomboid.steam=1 -Dzomboid.znetlog=1 -XX:-CreateCoredumpOnCrash -XX:-OmitStackTraceInFastThrow -XX:+PerfDisableSharedMem -XX:+UnlockExperimentalVMOptions -XX:+UseSuperWord -XX:MaxInlineLevel=15 -XX:InlineSmallCode=2500 -XX:+UseStringDeduplication -XX:+IgnoreUnrecognizedVMOptions -Doshi.util.wmi.timeout=5000 -XX:ShenandoahGCMode=generational -Xms20g -Xmx20g -XX:+UseG1GC -XX:MaxGCPauseMillis=50 -XX:ReservedCodeCacheSize=512m -XX:MetaspaceSize=512m -agentlib:zbNative -XX:+EnableDynamicAgentLoading";

    #[test]
    fn sorts_weber_args() {
        let stray: Vec<&str> = WEBER.split(' ').filter(|a| !replaced(a) && !needed(a)).collect();
        assert_eq!(stray, ["-XX:+PerfDisableSharedMem", "-XX:+UnlockExperimentalVMOptions", "-XX:+UseSuperWord",
            "-XX:MaxInlineLevel=15", "-XX:InlineSmallCode=2500", "-XX:+UseStringDeduplication",
            "-XX:+IgnoreUnrecognizedVMOptions", "-Doshi.util.wmi.timeout=5000"]); // ShenandoahGCMode: the GC choice owns it (0.1.11)
    }

    #[test]
    fn guided_sets() {
        let d = Tune::default();
        assert_eq!(guided_args(10, "g1", &d), "-Xms10g -Xmx10g -XX:+UseG1GC -XX:MaxGCPauseMillis=50 -XX:ReservedCodeCacheSize=512m -XX:MetaspaceSize=512m");
        assert_eq!(guided_args(12, "zgc", &d), "-Xms12g -Xmx12g -XX:+UseZGC -XX:ReservedCodeCacheSize=512m -XX:MetaspaceSize=512m");
        assert_eq!(guided_args(12, "shen", &d), "-Xms12g -Xmx12g -XX:+UseShenandoahGC -XX:ReservedCodeCacheSize=512m -XX:MetaspaceSize=512m");
        let t = Tune { pause_ms: 100, code_mb: 256, meta_mb: 1024 };
        assert_eq!(guided_args(8, "shen_gen", &t), "-Xms8g -Xmx8g -XX:+UseShenandoahGC -XX:ShenandoahGCMode=generational -XX:ReservedCodeCacheSize=256m -XX:MetaspaceSize=1024m");
        assert!(guided_args(8, "g1", &t).contains("-XX:MaxGCPauseMillis=100"));
        assert!(guided_owned("-XX:+UseShenandoahGC") && guided_owned("-XX:ShenandoahGCMode=generational"));
        assert!(!guided_owned("-XX:MaxMetaspaceSize=1g"));
    }

    #[test]
    fn drop_leaves_flag_out() {
        let own = json!({ "vmArgs": ["-Dzomboid.steam=1", "-XX:+UseSuperWord", "-Xmx3072m"] });
        let j = launch_json(&own, &guided_args(10, "g1", &Tune::default()), &["-XX:+UseSuperWord".to_string()], false, false, false, false);
        let vm: Vec<&str> = j["vmArgs"].as_array().unwrap().iter().filter_map(|v| v.as_str()).collect();
        assert!(vm.contains(&"-Dzomboid.steam=1") && vm.contains(&"-Xmx10g"));
        assert!(!vm.contains(&"-XX:+UseSuperWord") && !vm.contains(&"-Xmx3072m"));
    }
}
