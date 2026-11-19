// Diagnostics switches that change PZO's own settings (user 11-19, item 283: black screen on quit + FSR coming back).
// While a switch is ticked, every launch forces the PZO setting off; the value the player had is saved the first time
// and put back when they untick. PZO keeps the settings in two files under %USERPROFILE%\Zomboid\Lua:
//   pzo_config.json   one json object: "fsr_enabled", "texture_handoff" (PZOConfig)
//   MPOptim_Settings.ini   KEY=VALUE lines; F10's FSR switch writes JVM_FSREnabled here as well as fsr_enabled
use crate::files;
use serde_json::{Value, json};
use std::fs;
use std::path::PathBuf;

pub fn zomboid_lua_dir() -> PathBuf {
    let home = std::env::var("USERPROFILE").unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join("Zomboid").join("Lua")
}

fn config_json() -> PathBuf { zomboid_lua_dir().join("pzo_config.json") }
fn settings_ini() -> PathBuf { zomboid_lua_dir().join("MPOptim_Settings.ini") }
/// The player's own values from before we forced them: {"json:<key>": value, "ini:<key>": "text"}
fn saved_file() -> PathBuf { files::data_dir().join("pzo-forced.json") }

fn read_saved() -> Value {
    fs::read_to_string(saved_file()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(json!({}))
}

fn ini_get(text: &str, key: &str) -> Option<String> {
    text.lines().find_map(|l| l.strip_prefix(key).and_then(|r| r.strip_prefix('=')).map(|v| v.trim().to_string()))
}

/// Replaces KEY=... keeping every other line and the file's line endings. None if the key isn't there.
fn ini_set(text: &str, key: &str, val: &str) -> Option<String> {
    let mut hit = false;
    let out: Vec<String> = text.split_inclusive('\n').map(|l| {
        let body = l.trim_end_matches(['\r', '\n']);
        if body.strip_prefix(key).map(|r| r.starts_with('=')).unwrap_or(false) {
            hit = true;
            format!("{key}={val}{}", &l[body.len()..])
        } else {
            l.to_string()
        }
    }).collect();
    if hit { Some(out.concat()) } else { None }
}

/// One switch: (json key, optional ini key). Forces false while `on`, restores the saved value when not.
fn apply_one(saved: &mut Value, cfg: &mut Option<Value>, ini: &mut Option<String>, jkey: &str, ikey: Option<&str>,
             on: bool, notes: &mut Vec<String>) {
    let js = format!("json:{jkey}");
    if let Some(c) = cfg.as_mut() {
        if on {
            if saved.get(&js).is_none() {
                saved[&js] = c.get(jkey).cloned().unwrap_or(Value::Null);
            }
            c[jkey] = Value::Bool(false);
        } else if let Some(prev) = saved.as_object_mut().and_then(|o| o.remove(&js)) {
            match prev {
                Value::Null => { if let Some(o) = c.as_object_mut() { o.remove(jkey); } }
                v => c[jkey] = v,
            }
            notes.push(format!("PZO {jkey} put back as it was"));
        }
    } else if on {
        notes.push("PZO hasn't written its settings yet (launch once with PZO), so the PZO switches can't apply".into());
    }
    let Some(ik) = ikey else { return };
    let is = format!("ini:{ik}");
    if let Some(t) = ini.as_mut() {
        if on {
            if let Some(cur) = ini_get(t, ik) {
                if saved.get(&is).is_none() { saved[&is] = Value::String(cur); }
                if let Some(n) = ini_set(t, ik, "false") { *t = n; }
            }
        } else if let Some(Value::String(prev)) = saved.as_object_mut().and_then(|o| o.remove(&is)) {
            if let Some(n) = ini_set(t, ik, &prev) { *t = n; }
        }
    }
}

/// Brings PZO's files in line with the two switches. Call only while the game is closed (PZO rewrites them in game).
pub fn apply_pzo(no_handoff: bool, no_fsr: bool) -> Result<Vec<String>, String> {
    let mut saved = read_saved();
    let cfg_text = fs::read_to_string(config_json()).ok();
    let mut cfg: Option<Value> = match &cfg_text {
        Some(t) => Some(serde_json::from_str(t).map_err(|e| format!("PZO's pzo_config.json isn't valid json: {e}"))?),
        None => None,
    };
    let ini_text = fs::read_to_string(settings_ini()).ok();
    let mut ini = ini_text.clone();
    let mut notes = Vec::new();
    apply_one(&mut saved, &mut cfg, &mut ini, "texture_handoff", None, no_handoff, &mut notes);
    apply_one(&mut saved, &mut cfg, &mut ini, "fsr_enabled", Some("JVM_FSREnabled"), no_fsr, &mut notes);
    notes.dedup();
    if let (Some(c), Some(old)) = (&cfg, &cfg_text) {
        let new = serde_json::to_string(c).map_err(|e| e.to_string())?; // PZO writes it compact, one line
        if &new != old {
            fs::write(config_json(), new).map_err(|e| format!("can't write pzo_config.json: {e}"))?;
        }
    }
    if let (Some(n), Some(old)) = (&ini, &ini_text) {
        if n != old {
            fs::write(settings_ini(), n).map_err(|e| format!("can't write MPOptim_Settings.ini: {e}"))?;
        }
    }
    fs::write(saved_file(), serde_json::to_string_pretty(&saved).unwrap_or_default())
        .map_err(|e| format!("can't save the PZO values to restore: {e}"))?;
    Ok(notes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn force_then_restore() {
        let mut saved = json!({});
        let mut cfg = Some(json!({"a":1,"fsr_enabled":true,"texture_handoff":true}));
        let mut ini = Some("X=1\r\nJVM_FSREnabled=true\r\nJVM_FSRScale=59\r\n".to_string());
        let mut n = Vec::new();
        apply_one(&mut saved, &mut cfg, &mut ini, "fsr_enabled", Some("JVM_FSREnabled"), true, &mut n);
        assert_eq!(cfg.as_ref().unwrap()["fsr_enabled"], json!(false));
        assert_eq!(ini.as_deref().unwrap(), "X=1\r\nJVM_FSREnabled=false\r\nJVM_FSRScale=59\r\n");
        // a second launch keeps the first saved value
        apply_one(&mut saved, &mut cfg, &mut ini, "fsr_enabled", Some("JVM_FSREnabled"), true, &mut n);
        assert_eq!(saved["json:fsr_enabled"], json!(true));
        apply_one(&mut saved, &mut cfg, &mut ini, "fsr_enabled", Some("JVM_FSREnabled"), false, &mut n);
        assert_eq!(cfg.as_ref().unwrap()["fsr_enabled"], json!(true));
        assert_eq!(ini.as_deref().unwrap(), "X=1\r\nJVM_FSREnabled=true\r\nJVM_FSRScale=59\r\n");
        assert_eq!(saved, json!({}));
        // key order kept (PZO's compact one-line form)
        assert_eq!(serde_json::to_string(cfg.as_ref().unwrap()).unwrap(), r#"{"a":1,"fsr_enabled":true,"texture_handoff":true}"#);
    }
}
