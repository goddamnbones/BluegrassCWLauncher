// Java runtime choice (0.1.13, user 11-25): standard (the game's Zulu 25) or GraalVM CE 25, opt-in.
// PZ loads the JVM from <game>\jre64 only, so a GraalVM launch swaps jre64 for a junction to our GraalVM folder and
// puts Zulu back after the game exits. Between sessions Steam only ever sees Zulu.
use crate::files;
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Pinned GraalVM build; changes only with a launcher release (same rule as files::PINNED).
pub const GRAAL_VERSION: &str = "25.0.5.1";
const GRAAL_URL: &str = "https://github.com/graalvm/graalvm-ce-builds/releases/download/graal-25.6.5.1/graalvm-community-jdk-25i6-25.0.5.1_windows-x64_bin.zip";
const GRAAL_SHA256: &str = "1f534c46337f838880553c47f973a3d769b4058036288afcec930e2abddf06a1";
pub const GRAAL_BYTES: u64 = 316_322_078;
/// the zip's one top folder
const GRAAL_TOP: &str = "graalvm-community-25.6.5.1+2.1";
const GRAAL_DIR: &str = "graalvm-ce-25.0.5.1";

pub const ZULU: &str = "zulu";
pub const GRAALVM: &str = "graalvm";

/// settings.json `runtime`: "graalvm", anything else (missing, unknown) = standard
pub fn parse(s: Option<&str>) -> &'static str {
    if s == Some(GRAALVM) { GRAALVM } else { ZULU }
}

pub fn label(rt: &str) -> String {
    if rt == GRAALVM { format!("GraalVM CE {GRAAL_VERSION}") } else { "standard".into() }
}

/// GCs each runtime's build supports (launch::GCS ids). This GraalVM pin runs all four (checked on our server 11-25).
pub fn gcs(rt: &str) -> &'static [&'static str] {
    match rt {
        GRAALVM => &["g1", "zgc", "shen", "shen_gen"],
        _ => &crate::launch::GCS,
    }
}

/// The saved GC if this runtime supports it, else the default G1.
pub fn gc_for(rt: &str, saved: Option<&str>) -> String {
    match saved {
        Some(g) if gcs(rt).contains(&g) => g.to_string(),
        _ => "g1".into(),
    }
}

pub fn runtimes_root() -> PathBuf { files::data_dir().join("runtimes") }
pub fn graal_dir() -> PathBuf { runtimes_root().join(GRAAL_DIR) }
pub fn java_exe(home: &Path) -> PathBuf { home.join("bin").join("java.exe") }
pub fn graal_ready() -> bool { java_exe(&graal_dir()).is_file() }

/// Download progress for the page: (bytes done, total, phase); phase "" = idle
#[derive(Clone, serde::Serialize, Default)]
pub struct Progress { pub done: u64, pub total: u64, pub phase: String, pub error: Option<String> }
static PROGRESS: Mutex<Option<Progress>> = Mutex::new(None);
// one download at a time; a second caller waits, then finds it ready
static DOWNLOAD: Mutex<()> = Mutex::new(());

pub fn progress() -> Progress { PROGRESS.lock().ok().and_then(|p| p.clone()).unwrap_or_default() }
fn set_progress(p: Progress) { if let Ok(mut g) = PROGRESS.lock() { *g = Some(p); } }

/// GraalVM home, downloaded and extracted first if missing.
pub fn ensure_graal() -> Result<PathBuf, String> {
    let _one = DOWNLOAD.lock().map_err(|_| "download lock failed".to_string())?;
    let dir = graal_dir();
    if java_exe(&dir).is_file() { return Ok(dir); }
    let r = download_graal(&runtimes_root(), &dir);
    let p = progress();
    set_progress(Progress { phase: String::new(), error: r.as_ref().err().cloned(), ..p });
    r.map(|_| dir)
}

fn download_graal(root: &Path, dir: &Path) -> Result<(), String> {
    fs::create_dir_all(root).map_err(|e| format!("can't create {}: {e}", root.display()))?;
    let zip = root.join(format!("{GRAAL_DIR}.zip.part"));
    let tmp = root.join(format!("{GRAAL_DIR}.tmp"));
    let _ = fs::remove_file(&zip);
    let _ = fs::remove_dir_all(&tmp);
    set_progress(Progress { done: 0, total: GRAAL_BYTES, phase: "download".into(), error: None });
    let got = (|| -> Result<String, String> {
        let resp = ureq::get(GRAAL_URL).set("User-Agent", "BluegrassLauncher").call()
            .map_err(|e| format!("GraalVM download failed: {e}"))?;
        let mut rd = resp.into_reader().take(GRAAL_BYTES + 1);
        let mut f = fs::File::create(&zip).map_err(|e| format!("can't write {}: {e}", zip.display()))?;
        let mut h = Sha256::new();
        let mut buf = vec![0u8; 1 << 16];
        let mut done = 0u64;
        loop {
            let n = rd.read(&mut buf).map_err(|e| format!("GraalVM download failed: {e}"))?;
            if n == 0 { break; }
            h.update(&buf[..n]);
            f.write_all(&buf[..n]).map_err(|e| format!("can't write {}: {e}", zip.display()))?;
            done += n as u64;
            if done > GRAAL_BYTES { return Err("GraalVM download is larger than the pinned file".into()); }
            set_progress(Progress { done, total: GRAAL_BYTES, phase: "download".into(), error: None });
        }
        Ok(h.finalize().iter().map(|x| format!("{:02x}", x)).collect())
    })();
    let hash = match got { Ok(h) => h, Err(e) => { let _ = fs::remove_file(&zip); return Err(e); } };
    if hash != GRAAL_SHA256 {
        let _ = fs::remove_file(&zip);
        return Err(format!("GraalVM download failed the hash check (got {hash}); deleted, nothing was installed"));
    }
    set_progress(Progress { done: GRAAL_BYTES, total: GRAAL_BYTES, phase: "extract".into(), error: None });
    let r = extract_into(&zip, &tmp, dir);
    let _ = fs::remove_file(&zip);
    let _ = fs::remove_dir_all(&tmp);
    r
}

/// Extracts to `tmp`, then renames the zip's top folder to `dir`; `dir` only appears complete.
fn extract_into(zip: &Path, tmp: &Path, dir: &Path) -> Result<(), String> {
    let f = fs::File::open(zip).map_err(|e| format!("can't open the GraalVM zip: {e}"))?;
    let mut a = zip::ZipArchive::new(f).map_err(|e| format!("GraalVM zip unreadable: {e}"))?;
    a.extract(tmp).map_err(|e| format!("GraalVM extract failed: {e}"))?;
    let top = tmp.join(GRAAL_TOP);
    if !java_exe(&top).is_file() { return Err(format!("GraalVM zip has no {GRAAL_TOP}\\bin\\java.exe")); }
    let _ = fs::remove_dir_all(dir);
    fs::rename(&top, dir).map_err(|e| format!("can't move GraalVM into place: {e}"))?;
    if !java_exe(dir).is_file() { return Err("GraalVM bin\\java.exe missing after extract".into()); }
    Ok(())
}

// ---- jre64 swap ----

pub const JRE: &str = "jre64";
pub const JRE_ZULU: &str = "jre64_zulu";

fn is_link(p: &Path) -> bool { fs::symlink_metadata(p).map(|m| m.file_type().is_symlink()).unwrap_or(false) }

fn norm(p: &Path) -> String {
    let s = p.to_string_lossy().replace('/', "\\");
    let s = s.strip_prefix("\\\\?\\").or_else(|| s.strip_prefix("\\??\\")).unwrap_or(&s).to_string();
    s.trim_end_matches('\\').to_lowercase()
}

/// jre64 is a junction into our runtimes folder (any version, so a launcher update can still repair an old swap)
fn ours_link(jre: &Path, root: &Path) -> bool {
    is_link(jre) && fs::read_link(jre).map(|t| norm(&t).starts_with(&norm(root))).unwrap_or(false)
}

/// jre64 -> jre64_zulu, then junction jre64 -> `home`. Any failure undoes what was done.
pub fn swap(game: &Path, home: &Path) -> Result<(), String> {
    let (jre, zulu) = (game.join(JRE), game.join(JRE_ZULU));
    if zulu.exists() || is_link(&zulu) { return Err(format!("{JRE_ZULU} already exists in the game folder; restart the launcher to repair it")); }
    if is_link(&jre) || !jre.is_dir() { return Err(format!("the game's {JRE} isn't a normal folder; nothing was changed")); }
    if !java_exe(home).is_file() { return Err("GraalVM bin\\java.exe is missing; nothing was changed".into()); }
    fs::rename(&jre, &zulu).map_err(|e| format!("can't rename {JRE} (is the game running?): {e}; nothing was changed"))?;
    let undo = |why: String| -> String {
        if is_link(&jre) { let _ = fs::remove_dir(&jre); }
        match fs::rename(&zulu, &jre) {
            Ok(_) => format!("{why}; standard Java put back"),
            Err(e) => format!("{why}; putting {JRE_ZULU} back failed ({e}): rename it to {JRE} by hand"),
        }
    };
    if let Err(e) = make_junction(&jre, home) { return Err(undo(format!("can't link {JRE} to GraalVM: {e}"))); }
    if !java_exe(&jre).is_file() { return Err(undo(format!("{JRE} link to GraalVM doesn't work"))); }
    Ok(())
}

#[derive(Debug, PartialEq)]
pub enum Restore { Nothing, Restored, Refused(String) }

/// Puts Zulu back: removes our junction (the link only) and renames jre64_zulu to jre64. Err = try again later.
pub fn restore(game: &Path, root: &Path) -> Result<Restore, String> {
    let (jre, zulu) = (game.join(JRE), game.join(JRE_ZULU));
    if !zulu.is_dir() { return Ok(Restore::Nothing); }
    if is_link(&jre) {
        if !ours_link(&jre, root) {
            return Ok(Restore::Refused(format!("{JRE} links somewhere the launcher didn't put it; nothing changed. Check the game folder ({JRE_ZULU} is the standard Java)")));
        }
        fs::remove_dir(&jre).map_err(|e| format!("can't remove the GraalVM link: {e}"))?; // link only, never recursive
    } else if jre.is_dir() && fs::read_dir(&jre).map(|mut d| d.next().is_none()).unwrap_or(false) {
        // crash between create_dir and the reparse point in make_junction: an empty real folder, safe to drop
        fs::remove_dir(&jre).map_err(|e| format!("can't remove the empty {JRE}: {e}"))?;
    } else if jre.exists() {
        return Ok(Restore::Refused(format!("The game folder has both {JRE} and {JRE_ZULU} (Steam may have re-downloaded {JRE}); \
            nothing changed. Check the game folder: keep one, or Steam > Verify files")));
    }
    fs::rename(&zulu, &jre).map_err(|e| format!("can't rename {JRE_ZULU} back: {e}"))?;
    Ok(Restore::Restored)
}

/// Directory junction (no admin right needed, works across drives), via FSCTL_SET_REPARSE_POINT.
fn make_junction(link: &Path, target: &Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::{CloseHandle, GENERIC_WRITE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, OPEN_EXISTING};
    use windows_sys::Win32::System::IO::DeviceIoControl;
    const FSCTL_SET_REPARSE_POINT: u32 = 0x0009_00A4;
    const IO_REPARSE_TAG_MOUNT_POINT: u32 = 0xA000_0003;
    let t = std::path::absolute(target).map_err(|e| e.to_string())?;
    let t = t.to_string_lossy().replace('/', "\\");
    let t = t.strip_prefix("\\\\?\\").unwrap_or(&t).to_string();
    let subst: Vec<u16> = format!("\\??\\{t}").encode_utf16().collect();
    let print: Vec<u16> = t.encode_utf16().collect();
    let path_bytes = (subst.len() + 1 + print.len() + 1) * 2;
    let mut buf: Vec<u8> = Vec::with_capacity(16 + path_bytes);
    buf.extend(IO_REPARSE_TAG_MOUNT_POINT.to_le_bytes());
    buf.extend(((8 + path_bytes) as u16).to_le_bytes()); // ReparseDataLength
    buf.extend(0u16.to_le_bytes());
    buf.extend(0u16.to_le_bytes()); // SubstituteNameOffset
    buf.extend(((subst.len() * 2) as u16).to_le_bytes());
    buf.extend((((subst.len() + 1) * 2) as u16).to_le_bytes()); // PrintNameOffset
    buf.extend(((print.len() * 2) as u16).to_le_bytes());
    for w in subst.iter().chain(&[0]).chain(print.iter()).chain(&[0]) { buf.extend(w.to_le_bytes()); }
    fs::create_dir(link).map_err(|e| e.to_string())?;
    let wl: Vec<u16> = link.as_os_str().encode_wide().chain([0]).collect();
    // the OS error is read before CloseHandle, which can overwrite it
    let failed: Option<std::io::Error> = unsafe {
        let h = CreateFileW(wl.as_ptr(), GENERIC_WRITE, 0, std::ptr::null(), OPEN_EXISTING,
            FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS, std::ptr::null_mut());
        if h == INVALID_HANDLE_VALUE { Some(std::io::Error::last_os_error()) } else {
            let mut ret = 0u32;
            let r = DeviceIoControl(h, FSCTL_SET_REPARSE_POINT, buf.as_ptr() as _, buf.len() as u32,
                std::ptr::null_mut(), 0, &mut ret, std::ptr::null_mut());
            let err = if r == 0 { Some(std::io::Error::last_os_error()) } else { None };
            CloseHandle(h);
            err
        }
    };
    if let Some(e) = failed {
        let _ = fs::remove_dir(link);
        return Err(e.to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("bgl-rt-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    /// game folder with a fake Zulu jre64, runtimes root with a fake GraalVM
    fn setup(name: &str) -> (PathBuf, PathBuf, PathBuf) {
        let d = temp(name);
        let (game, root) = (d.join("game"), d.join("runtimes"));
        let home = root.join(GRAAL_DIR);
        fs::create_dir_all(game.join(JRE).join("bin")).unwrap();
        fs::write(java_exe(&game.join(JRE)), b"zulu").unwrap();
        fs::create_dir_all(home.join("bin")).unwrap();
        fs::write(java_exe(&home), b"graal").unwrap();
        (d, game, home)
    }

    #[test]
    fn runtime_parsing() {
        assert_eq!(parse(None), ZULU);
        assert_eq!(parse(Some("zulu")), ZULU);
        assert_eq!(parse(Some("GraalVM")), ZULU);
        assert_eq!(parse(Some("x")), ZULU);
        assert_eq!(parse(Some("graalvm")), GRAALVM);
        assert_eq!(label(GRAALVM), "GraalVM CE 25.0.5.1");
    }

    #[test]
    fn gc_fallback_per_runtime() {
        for rt in [ZULU, GRAALVM] {
            assert_eq!(gc_for(rt, Some("shen_gen")), "shen_gen");
            assert_eq!(gc_for(rt, Some("bogus")), "g1");
            assert_eq!(gc_for(rt, None), "g1");
        }
        assert!(crate::launch::GCS.iter().all(|g| gcs(GRAALVM).contains(g)));
    }

    #[test]
    fn swap_and_restore() {
        let (d, game, home) = setup("swap");
        let root = home.parent().unwrap().to_path_buf();
        swap(&game, &home).unwrap();
        assert!(is_link(&game.join(JRE)));
        assert_eq!(fs::read(java_exe(&game.join(JRE))).unwrap(), b"graal");
        assert_eq!(fs::read(java_exe(&game.join(JRE_ZULU))).unwrap(), b"zulu");
        assert!(swap(&game, &home).is_err()); // already swapped
        assert_eq!(restore(&game, &root).unwrap(), Restore::Restored);
        assert!(!is_link(&game.join(JRE)) && !game.join(JRE_ZULU).exists());
        assert_eq!(fs::read(java_exe(&game.join(JRE))).unwrap(), b"zulu");
        assert_eq!(fs::read(java_exe(&home)).unwrap(), b"graal"); // link removed, GraalVM untouched
        assert_eq!(restore(&game, &root).unwrap(), Restore::Nothing);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn refuses_when_jre64_is_a_real_folder() {
        let (d, game, home) = setup("refuse");
        fs::create_dir_all(game.join(JRE_ZULU).join("bin")).unwrap();
        fs::write(java_exe(&game.join(JRE_ZULU)), b"old").unwrap();
        assert!(matches!(restore(&game, home.parent().unwrap()).unwrap(), Restore::Refused(_)));
        assert_eq!(fs::read(java_exe(&game.join(JRE))).unwrap(), b"zulu");
        assert_eq!(fs::read(java_exe(&game.join(JRE_ZULU))).unwrap(), b"old");
        assert!(swap(&game, &home).is_err()); // never swaps over a leftover jre64_zulu
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn repairs_a_crash_between_rename_and_link() {
        let (d, game, home) = setup("repair");
        fs::rename(game.join(JRE), game.join(JRE_ZULU)).unwrap(); // crash right after the rename
        assert_eq!(restore(&game, home.parent().unwrap()).unwrap(), Restore::Restored);
        assert_eq!(fs::read(java_exe(&game.join(JRE))).unwrap(), b"zulu");
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn repairs_a_crash_inside_make_junction() {
        let (d, game, home) = setup("repair-empty");
        fs::rename(game.join(JRE), game.join(JRE_ZULU)).unwrap();
        fs::create_dir(game.join(JRE)).unwrap(); // crash after create_dir, before the reparse point
        assert_eq!(restore(&game, home.parent().unwrap()).unwrap(), Restore::Restored);
        assert_eq!(fs::read(java_exe(&game.join(JRE))).unwrap(), b"zulu");
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn swap_without_graal_changes_nothing() {
        let (d, game, home) = setup("nograal");
        fs::remove_file(java_exe(&home)).unwrap();
        assert!(swap(&game, &home).is_err());
        assert!(game.join(JRE).is_dir() && !game.join(JRE_ZULU).exists());
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn extracts_top_folder_into_place() {
        let d = temp("extract");
        let z = d.join("g.zip");
        let mut w = zip::ZipWriter::new(fs::File::create(&z).unwrap());
        let o = zip::write::SimpleFileOptions::default();
        w.start_file(format!("{GRAAL_TOP}/bin/java.exe"), o).unwrap();
        w.write_all(b"graal").unwrap();
        w.start_file(format!("{GRAAL_TOP}/release"), o).unwrap();
        w.finish().unwrap();
        let dir = d.join(GRAAL_DIR);
        extract_into(&z, &d.join("tmp"), &dir).unwrap();
        assert_eq!(fs::read(java_exe(&dir)).unwrap(), b"graal");
        assert!(dir.join("release").is_file());
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn foreign_link_is_left_alone() {
        let (d, game, home) = setup("foreign");
        let other = d.join("elsewhere");
        fs::create_dir_all(other.join("bin")).unwrap();
        fs::write(java_exe(&other), b"other").unwrap();
        fs::rename(game.join(JRE), game.join(JRE_ZULU)).unwrap();
        make_junction(&game.join(JRE), &other).unwrap();
        assert!(matches!(restore(&game, home.parent().unwrap()).unwrap(), Restore::Refused(_)));
        assert_eq!(fs::read(java_exe(&game.join(JRE))).unwrap(), b"other");
        fs::remove_dir(game.join(JRE)).unwrap();
        let _ = fs::remove_dir_all(&d);
    }
}
