// Option B (user 2026-11-18): for one launch the game's ProjectZomboid64.json is ours (ZombieBuddy agent, our JVM set,
// PZO's entry point); afterwards the player's own json goes back, so a Steam launch is whatever it was before.
// PZO's entry point rewrites the json early in every launch, so we restore 120 s after start and again after exit.
use crate::files::{self, GAME_EXE, GAME_JSON};
use crate::runtime;
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
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
    /// GraalVM home when this launch runs on GraalVM (jre64 swapped for the session, runtime.rs); None = standard
    pub java_home: Option<PathBuf>,
}

/// A GraalVM session is being watched in this launcher; the status repair leaves jre64 alone meanwhile.
static SWAP_WATCH: AtomicUsize = AtomicUsize::new(0);
/// The game wasn't running 30 s after a GraalVM launch (user 11-25): the page offers one standard-Java launch.
pub static GRAAL_FAILED: AtomicBool = AtomicBool::new(false);
const GRAAL_CHECK_SECS: u64 = 30;

fn set_swap_flag(on: bool) {
    let mut st = read_state();
    st["jre_swap"] = Value::Bool(on);
    write_state(&st);
}

/// Puts Zulu back if a GraalVM session left jre64 swapped (crash, launcher closed). Only while the game is closed and
/// no session of this launcher is watching it. Returns a note for the page, if any.
pub fn repair_jre(game: &Path) -> Option<String> {
    if SWAP_WATCH.load(Ordering::SeqCst) > 0 || !game.join(runtime::JRE_ZULU).exists() || game_running() { return None; }
    match runtime::restore(game, &runtime::runtimes_root()) {
        Ok(runtime::Restore::Restored) => { set_swap_flag(false); Some("Standard Java is back in the game folder (Steam launches as before)".into()) }
        Ok(runtime::Restore::Nothing) => None,
        Ok(runtime::Restore::Refused(n)) => Some(n),
        Err(e) => Some(format!("Couldn't put standard Java back yet ({e}); retrying")),
    }
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

/// A json only our launch (or PZO during it) writes: PZO's entry point as main class, or the ZombieBuddy agent. Decided
/// by content, so a lost or reset launch-state can't make us save one of these as the player's own json.
fn looks_ours(b: &[u8]) -> bool {
    let Ok(v) = serde_json::from_slice::<Value>(b) else { return false };
    v["mainClass"].as_str() == Some("com/pzoptimizer/PZOEntrypoint")
        || v["vmArgs"].as_array().into_iter().flatten().filter_map(|x| x.as_str()).any(|a| a.trim().starts_with("-agentlib:zbNative"))
}

/// Ours by recorded hash or by content.
fn ours_json(b: &[u8]) -> bool { looks_ours(b) || is_ours(&read_state(), &files::sha256_bytes(b)) }

const STAGED_JSON: &str = "ProjectZomboid64.json.new";

/// JVM flags are split on spaces and written one per vmArgs entry, so a quote or a space inside one flag can't be
/// carried (the game would get pieces). None = fine.
pub fn arg_problem(line: &str) -> Option<String> {
    line.split_whitespace().find(|a| a.contains('"') || a.contains('\'')).map(|a| format!(
        "{a}: JVM flags can't contain quotes, and one flag can't hold a space (a space starts a new flag). \
         Use a path without spaces"))
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

/// Flag check (user 11-21): run the selected runtime's java.exe (0.1.13) with the line + -version. Ok(None) = accepted,
/// Ok(Some(note)) = couldn't check (save anyway), Err = Java's own error text.
pub fn check_flags(java: Option<PathBuf>, line: &str) -> Result<Option<String>, String> {
    let Some(java) = java.filter(|j| j.is_file()) else {
        return Ok(Some("flags not checked: the selected Java's java.exe wasn't found".into()));
    };
    let mut c = Command::new(&java);
    c.args(line.split_whitespace()).arg("-version");
    #[cfg(windows)]
    { use std::os::windows::process::CommandExt; c.creation_flags(0x0800_0000); } // CREATE_NO_WINDOW
    let out = match c.output() { Ok(o) => o, Err(e) => return Ok(Some(format!("flags not checked: {e}"))) };
    if out.status.success() { return Ok(None); }
    let err = String::from_utf8_lossy(&out.stderr);
    let lines: Vec<&str> = err.lines().map(str::trim).filter(|l| !l.is_empty()).take(4).collect();
    Err(format!("Not saved: the selected Java refused the flags: {}", lines.join(" / ")))
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
    let own = if ours_json(&cur) { fs::read(own_copy()).unwrap_or(cur) } else { cur };
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
    // a swap left from an earlier session goes back first, so a standard launch never runs on GraalVM
    if game.join(runtime::JRE_ZULU).exists() {
        match runtime::restore(game, &runtime::runtimes_root()) {
            Ok(runtime::Restore::Restored) | Ok(runtime::Restore::Nothing) => set_swap_flag(false),
            Ok(runtime::Restore::Refused(n)) => return Err(n),
            Err(e) => return Err(format!("standard Java isn't back from the last GraalVM session yet: {e}")),
        }
    }
    GRAAL_FAILED.store(false, Ordering::SeqCst);
    // JFR doesn't create missing folders: no folder, no recording
    if jfr {
        fs::create_dir_all(JFR_DIR).map_err(|e| format!("can't create {JFR_DIR} for the JFR recording: {e}"))?;
    }
    let zb = files::component_state(game, "zombiebuddy") == "approved";
    let pzo_installed = files::component_state(game, "pzo") == "approved";
    let pzo = pzo_installed && !o.no_pzo;
    let path = game.join(GAME_JSON);
    if let Some(p) = arg_problem(jvm_args) { return Err(p); }
    let cur = fs::read(&path).map_err(|e| format!("can't read {GAME_JSON}: {e}"))?;
    // the folder holds the player's own json unless it's one of ours left from a launch that didn't get restored.
    // One of ours never replaces the saved copy; with no saved copy yet, this launch is built from it unsaved.
    let own_bytes = if ours_json(&cur) {
        fs::read(own_copy()).unwrap_or(cur)
    } else {
        fs::write(own_copy(), &cur).map_err(|e| format!("can't save your json: {e}"))?;
        cur
    };
    let own: Value = serde_json::from_slice(&own_bytes).map_err(|e| format!("your {GAME_JSON} isn't valid json: {e}"))?;
    // PZO's staged update json from before this launch isn't ours to delete
    let staged_before = fs::read(game.join(STAGED_JSON)).ok();
    let j = launch_json(&own, jvm_args, &o.drop, zb, pzo, jfr, o.nmt);
    let text = serde_json::to_string_pretty(&j).map_err(|e| e.to_string())?;
    remember_ours(files::sha256_bytes(text.as_bytes()));
    fs::write(&path, &text).map_err(|e| format!("can't write {GAME_JSON}: {e}"))?;
    let mut st = read_state();
    st["pending"] = Value::Bool(true);
    write_state(&st);

    // GraalVM (user 11-25): recorded before the rename, so a crash at any point is repaired at the next start
    let graal = o.java_home.is_some();
    if let Some(home) = &o.java_home {
        set_swap_flag(true);
        if let Err(e) = runtime::swap(game, home) {
            if !game.join(runtime::JRE_ZULU).exists() { set_swap_flag(false); }
            restore(game);
            return Err(e);
        }
        SWAP_WATCH.fetch_add(1, Ordering::SeqCst);
    }
    let unswap = move |g: &Path| {
        if !graal { return; }
        // "exited" = no game process on 3 checks in a row (Steam can relaunch the exe); retry while files are locked
        let mut quiet = 0;
        loop {
            if game_running() { quiet = 0; } else { quiet += 1; }
            if quiet >= 3 {
                match runtime::restore(g, &runtime::runtimes_root()) {
                    Ok(runtime::Restore::Refused(_)) => break, // the page shows the note (repair_jre)
                    Ok(_) => { set_swap_flag(false); break; }
                    Err(_) => {}
                }
            }
            std::thread::sleep(Duration::from_secs(3));
        }
        SWAP_WATCH.fetch_sub(1, Ordering::SeqCst);
    };

    let child = Command::new(game.join(GAME_EXE)).current_dir(game).spawn();
    let mut child = match child {
        Ok(c) => c,
        Err(e) => {
            restore(game);
            if graal {
                let r = runtime::restore(game, &runtime::runtimes_root());
                if matches!(r, Ok(runtime::Restore::Restored)) { set_swap_flag(false); }
                SWAP_WATCH.fetch_sub(1, Ordering::SeqCst);
            }
            return Err(format!("couldn't start the game: {e}"));
        }
    };
    if graal {
        std::thread::spawn(|| {
            std::thread::sleep(Duration::from_secs(GRAAL_CHECK_SECS));
            if !game_running() { GRAAL_FAILED.store(true, Ordering::SeqCst); }
        });
    }
    let g = game.to_path_buf();
    // PZO's staged update json (it moves it over the json shortly after exit): dropped only when this launch made it
    let drop_staged = move |g: &Path| {
        let p = g.join(STAGED_JSON);
        if let Ok(b) = fs::read(&p) {
            if staged_before.as_deref() != Some(&b[..]) { let _ = fs::remove_file(&p); }
        }
    };
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
        drop_staged(&g);
        restore(&g);
        let _ = child.wait();
        // PZO moves a staged update over the json shortly after exit; wait, then make sure the player's own is back
        std::thread::sleep(Duration::from_secs(20));
        if let Ok(b) = fs::read(g.join(GAME_JSON)) {
            // after exit only a json that is ours by content is recorded (the player may have changed theirs)
            if looks_ours(&b) && fs::read(own_copy()).ok().as_deref() != Some(&b[..]) { remember_ours(files::sha256_bytes(&b)); }
        }
        drop_staged(&g);
        restore(&g);
        unswap(&g);
    });
    Ok(format!(
        "Started: Java: {}, ZombieBuddy {}, PZO {}{}",
        runtime::label(if graal { runtime::GRAALVM } else { runtime::ZULU }),
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
    fn ours_by_content() {
        let own = json!({ "vmArgs": ["-Dzomboid.steam=1"], "mainClass": "zombie/gameStates/MainScreenState" });
        let b = |v: &Value| serde_json::to_vec(v).unwrap();
        assert!(!looks_ours(&b(&own)));
        assert!(looks_ours(&b(&launch_json(&own, "-Xmx8g", &[], false, true, false, false)))); // PZO entry point
        assert!(looks_ours(&b(&launch_json(&own, "-Xmx8g", &[], true, false, false, false)))); // zbNative
        assert!(!looks_ours(b"not json"));
    }

    #[test]
    fn refuses_quotes_in_args() {
        assert!(arg_problem("-Xmx8g -XX:+UseG1GC").is_none());
        assert!(arg_problem("-Dpath=\"C:\\My").is_some());
        assert!(arg_problem("-Dx='a'").is_some());
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
