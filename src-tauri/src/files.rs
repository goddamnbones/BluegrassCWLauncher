// Game folder, launcher data folder, pinned components (download + SHA-256 check).
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

pub const GAME_EXE: &str = "ProjectZomboid64.exe";
pub const GAME_JSON: &str = "ProjectZomboid64.json";

/// One file we install into the game folder. Only these exact versions; a newer release stays "not approved"
/// until it is tested and added here (and a new launcher is released).
pub struct Pinned {
    pub component: &'static str,
    pub version: &'static str,
    pub file: &'static str,
    pub url: &'static str,
    pub sha256: &'static str,
}

pub const PINNED: &[Pinned] = &[
    Pinned {
        component: "zombiebuddy",
        version: "2.3.4",
        file: "ZombieBuddy.jar",
        url: "https://github.com/zed-0xff/ZombieBuddy/releases/download/v2.3.4/ZombieBuddy.jar",
        sha256: "eb79b9876332010733a8e0d7ce4fe0377846059a9e28859029e2a0fb449f6cf2",
    },
    Pinned {
        component: "zombiebuddy",
        version: "2.3.4",
        file: "zbNative.dll", // v2.3.4 ships no dll; 2.3.3's is the current one
        url: "https://github.com/zed-0xff/ZombieBuddy/releases/download/v2.3.3/zbNative.dll",
        sha256: "c2ae9335e717ee24b2f4a40d1a3bf77f1519762a72a0459e766a2bbafc077f6c",
    },
    Pinned {
        component: "pzo",
        version: "2.3",
        file: "PZOptimEngine.jar",
        url: "https://github.com/prop11/PZO-Launcher/releases/download/V2.3/PZOptimEngine.jar",
        sha256: "44fa7ea243cbf8d26ec8c24e8152e3fe1e0fdf5ee002541773edda48df01c717",
    },
];

/// %LOCALAPPDATA%\BluegrassLauncher (settings, the player's own json copy, launch state)
pub fn data_dir() -> PathBuf {
    let base = std::env::var("LOCALAPPDATA").unwrap_or_else(|_| ".".into());
    let d = PathBuf::from(base).join("BluegrassLauncher");
    let _ = fs::create_dir_all(&d);
    d
}

// path -> (size, modified, hash): the page asks for status every 5 s; only re-hash a file when it changed
static HASH_CACHE: std::sync::Mutex<Vec<(PathBuf, u64, std::time::SystemTime, String)>> = std::sync::Mutex::new(Vec::new());

pub fn sha256_file(p: &Path) -> Option<String> {
    let meta = fs::metadata(p).ok()?;
    let (size, modified) = (meta.len(), meta.modified().ok()?);
    if let Ok(c) = HASH_CACHE.lock() {
        if let Some(e) = c.iter().find(|e| e.0 == p && e.1 == size && e.2 == modified) {
            return Some(e.3.clone());
        }
    }
    let h = sha256_uncached(p)?;
    if let Ok(mut c) = HASH_CACHE.lock() {
        c.retain(|e| e.0 != p);
        c.push((p.to_path_buf(), size, modified, h.clone()));
    }
    Some(h)
}

fn sha256_uncached(p: &Path) -> Option<String> {
    let mut f = fs::File::open(p).ok()?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = f.read(&mut buf).ok()?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Some(hex(&h.finalize()))
}

pub fn sha256_bytes(b: &[u8]) -> String {
    hex(&Sha256::digest(b))
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{:02x}", x)).collect()
}

/// Steam's install path from the registry, then every library in libraryfolders.vdf; first with ProjectZomboid.
pub fn find_game_dir() -> Option<PathBuf> {
    let hkcu = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER);
    let steam: String = hkcu.open_subkey("Software\\Valve\\Steam").ok()?.get_value("SteamPath").ok()?;
    let steam = PathBuf::from(steam.replace('/', "\\"));
    let mut libs = vec![steam.clone()];
    if let Ok(vdf) = fs::read_to_string(steam.join("steamapps").join("libraryfolders.vdf")) {
        for line in vdf.lines() {
            let t = line.trim();
            if t.starts_with("\"path\"") {
                if let Some(v) = t.split('"').nth(3) {
                    libs.push(PathBuf::from(v.replace("\\\\", "\\")));
                }
            }
        }
    }
    libs.into_iter()
        .map(|l| l.join("steamapps").join("common").join("ProjectZomboid"))
        .find(|d| d.join(GAME_EXE).is_file())
}

pub fn valid_game_dir(d: &Path) -> bool {
    d.join(GAME_EXE).is_file() && d.join(GAME_JSON).is_file()
}

/// "approved" (pinned hash), "other" (present, different file), "missing" for every file of a component
pub fn component_state(game: &Path, component: &str) -> String {
    let mut any_missing = false;
    let mut any_other = false;
    for p in PINNED.iter().filter(|p| p.component == component) {
        match sha256_file(&game.join(p.file)) {
            None => any_missing = true,
            Some(h) if h != p.sha256 => any_other = true,
            _ => {}
        }
    }
    if any_other { "other".into() } else if any_missing { "missing".into() } else { "approved".into() }
}

/// Downloads every file of a component, checks its SHA-256 before anything touches the game folder, then writes it.
pub fn install(game: &Path, component: &str) -> Result<String, String> {
    let files: Vec<&Pinned> = PINNED.iter().filter(|p| p.component == component).collect();
    if files.is_empty() {
        return Err(format!("unknown component {component}"));
    }
    let mut got = Vec::new();
    for p in &files {
        let resp = ureq::get(p.url)
            .set("User-Agent", "BluegrassLauncher")
            .call()
            .map_err(|e| format!("download {} failed: {e}", p.file))?;
        let mut body = Vec::new();
        resp.into_reader()
            .take(64 * 1024 * 1024)
            .read_to_end(&mut body)
            .map_err(|e| format!("download {} failed: {e}", p.file))?;
        let h = sha256_bytes(&body);
        if h != p.sha256 {
            return Err(format!("{} failed the hash check (got {h}); nothing was installed", p.file));
        }
        got.push((p, body));
    }
    for (p, body) in got {
        let dest = game.join(p.file);
        let tmp = game.join(format!("{}.bgtmp", p.file));
        fs::write(&tmp, &body).map_err(|e| format!("write {} failed: {e}", p.file))?;
        fs::rename(&tmp, &dest).map_err(|e| format!("replace {} failed (is the game running?): {e}", p.file))?;
    }
    Ok(format!("{} {} installed", component, files[0].version))
}

/// Deletes a component's files, only when every one is our approved version (a different version is not ours to remove).
pub fn uninstall(game: &Path, component: &str) -> Result<String, String> {
    let files: Vec<&Pinned> = PINNED.iter().filter(|p| p.component == component).collect();
    if files.is_empty() {
        return Err(format!("unknown component {component}"));
    }
    if component_state(game, component) != "approved" {
        return Err(format!("{component} is not the approved version; nothing was removed"));
    }
    for p in &files {
        fs::remove_file(game.join(p.file)).map_err(|e| format!("remove {} failed (is the game running?): {e}", p.file))?;
    }
    Ok(format!("{} {} uninstalled", component, files[0].version))
}

/// ZombieBuddy issue #88: stray JVM dlls in the game root make zbNative load a second libjava (breaks networking).
pub fn stray_jvm_dlls(game: &Path) -> Vec<String> {
    ["java.dll", "instrument.dll", "jvm.dll"]
        .iter()
        .filter(|f| game.join(f).is_file())
        .map(|f| f.to_string())
        .collect()
}

pub fn total_ram_gb() -> u64 {
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    unsafe {
        let mut m: MEMORYSTATUSEX = std::mem::zeroed();
        m.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
        if GlobalMemoryStatusEx(&mut m) == 0 {
            return 16;
        }
        m.ullTotalPhys / (1024 * 1024 * 1024)
    }
}
