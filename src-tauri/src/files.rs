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
    // PZO: always the latest GitHub release (user 11-20); no hash, any PZOptimEngine.jar present counts as installed
    Pinned {
        component: "pzo",
        version: LATEST,
        file: "PZOptimEngine.jar",
        url: "https://api.github.com/repos/prop11/PZO-Launcher/releases/latest",
        sha256: "",
    },
];

/// version of an unpinned component: url is the GitHub "latest release" API, the asset named `file` is downloaded
pub const LATEST: &str = "latest";

/// download url + release tag of the asset named `file` in the latest release
fn latest_asset(api: &str, file: &str) -> Result<(String, String), String> {
    let body = ureq::get(api)
        .set("User-Agent", "BluegrassLauncher")
        .set("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| format!("latest release lookup failed: {e}"))?
        .into_string()
        .map_err(|e| format!("latest release lookup failed: {e}"))?;
    let v: serde_json::Value = serde_json::from_str(&body).map_err(|e| format!("latest release lookup failed: {e}"))?;
    let tag = v["tag_name"].as_str().unwrap_or("?").to_string();
    let url = v["assets"]
        .as_array()
        .and_then(|a| a.iter().find(|x| x["name"].as_str() == Some(file)))
        .and_then(|x| x["browser_download_url"].as_str())
        .ok_or_else(|| format!("latest release {tag} has no {file}"))?;
    if !url.starts_with("https://github.com/") {
        return Err(format!("unexpected download url {url}"));
    }
    Ok((url.to_string(), tag))
}

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
            Some(h) if p.version != LATEST && h != p.sha256 => any_other = true,
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
    let mut version = files[0].version.to_string();
    for p in &files {
        let latest = p.version == LATEST;
        let url = if latest {
            let (u, tag) = latest_asset(p.url, p.file)?;
            version = tag;
            u
        } else {
            p.url.to_string()
        };
        let resp = ureq::get(&url)
            .set("User-Agent", "BluegrassLauncher")
            .call()
            .map_err(|e| format!("download {} failed: {e}", p.file))?;
        let mut body = Vec::new();
        // one byte over the limit, so a file that doesn't fit is an error, not a silently cut one
        resp.into_reader()
            .take(MAX_DOWNLOAD + 1)
            .read_to_end(&mut body)
            .map_err(|e| format!("download {} failed: {e}", p.file))?;
        if body.len() as u64 > MAX_DOWNLOAD {
            return Err(format!("download {} failed: larger than {} MiB; nothing was installed", p.file, MAX_DOWNLOAD >> 20));
        }
        let h = sha256_bytes(&body);
        if !latest && h != p.sha256 {
            return Err(format!("{} failed the hash check (got {h}); nothing was installed", p.file));
        }
        got.push((p, body));
    }
    put_all(game, &got.iter().map(|(p, b)| (p.file, &b[..])).collect::<Vec<_>>())?;
    Ok(format!("{} {} installed", component, version))
}

const MAX_DOWNLOAD: u64 = 64 * 1024 * 1024;

/// Writes every file to a temp first; only when all are written are they moved in. If a move fails, the files
/// already moved get their old content back (or are removed if they were new), so a component is never half-installed.
fn put_all(game: &Path, items: &[(&str, &[u8])]) -> Result<(), String> {
    let tmp = |f: &str| game.join(format!("{f}.bgtmp"));
    let clean = || for (f, _) in items { let _ = fs::remove_file(tmp(f)); };
    for (f, body) in items {
        if let Err(e) = fs::write(tmp(f), body) {
            clean();
            return Err(format!("write {f} failed: {e}"));
        }
    }
    let mut done: Vec<(&str, Option<Vec<u8>>)> = Vec::new();
    for (f, _) in items {
        let dest = game.join(f);
        let old = fs::read(&dest).ok();
        if let Err(e) = fs::rename(tmp(f), &dest) {
            for (d, old) in &done {
                match old {
                    Some(b) => { let _ = fs::write(game.join(d), b); }
                    None => { let _ = fs::remove_file(game.join(d)); }
                }
            }
            clean();
            return Err(format!("replace {f} failed (is the game running?): {e}; nothing was changed"));
        }
        done.push((f, old));
    }
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn put_all_rolls_back_the_first_file() {
        let d = std::env::temp_dir().join(format!("bgl-putall-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        fs::write(d.join("a.jar"), b"old a").unwrap();
        // b.dll is a folder: the move over it fails
        fs::create_dir_all(d.join("b.dll")).unwrap();
        fs::write(d.join("b.dll").join("x"), b"x").unwrap();
        assert!(put_all(&d, &[("a.jar", b"new a"), ("b.dll", b"new b")]).is_err());
        assert_eq!(fs::read(d.join("a.jar")).unwrap(), b"old a");
        assert!(!d.join("a.jar.bgtmp").exists() && !d.join("b.dll.bgtmp").exists());
        assert!(put_all(&d, &[("a.jar", b"new a"), ("c.dll", b"new c")]).is_ok());
        assert_eq!(fs::read(d.join("a.jar")).unwrap(), b"new a");
        let _ = fs::remove_dir_all(&d);
    }
}
