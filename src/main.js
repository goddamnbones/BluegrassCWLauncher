// Bluegrass launcher page. Only calls the launcher's own commands (src-tauri/src/lib.rs); never loads remote content.
const { invoke } = window.__TAURI__.core;
const $ = (id) => document.getElementById(id);

const STATE_TEXT = {
  approved: "Installed",
  missing: "Not installed",
  other: "Different version (not approved)",
};

function log(msg, bad) {
  const line = document.createElement("div");
  line.textContent = new Date().toLocaleTimeString() + "  " + msg;
  if (bad) line.className = "bad";
  $("log").prepend(line);
}

// Diagnostics checkboxes: element id = settings key = set_diag key (lib.rs)
const DIAG = ["nmt", "no_pzo", "no_handoff", "no_fsr"];

// RAM dropdown -> heap GB. Same table as launch.rs heap_for_ram; keep them matching.
const RAM_HEAP = { 8: 4, 12: 6, 16: 8, 32: 12, 64: 16 };

// the option whose heap matches the game memory setting, else "custom"
function ramChoice(heap) {
  const hit = Object.keys(RAM_HEAP).find((k) => RAM_HEAP[k] === heap);
  return hit || "custom";
}

let guided = ""; // memory + GC line from the choices (Default button)
let stray = []; // [flag, keep|null] from get_status
let strayKey = "";

// one "keep" checkbox per flag; data-flag holds the flag
function flagRows(list) {
  return list.map(([flag, keep]) => {
    const l = Object.assign(document.createElement("label"), { className: "check" });
    const c = Object.assign(document.createElement("input"), { type: "checkbox", checked: keep !== false });
    c.dataset.flag = flag;
    const t = Object.assign(document.createElement("code"), { textContent: flag });
    l.append(c, " Keep ", t, keep === null ? " (new)" : "");
    return l;
  });
}

function flagChoices(box) {
  const out = {};
  for (const c of box.querySelectorAll("input[data-flag]")) out[c.dataset.flag] = c.checked;
  return out;
}

async function refresh() {
  const s = await invoke("get_status");
  guided = s.guided;
  $("version").textContent = "launcher " + s.launcher;
  if (document.activeElement !== $("jvm")) $("jvm").value = s.jvm_args;
  if (document.activeElement !== $("game")) $("game").value = s.game_dir || "";
  if (document.activeElement !== $("ram")) $("ram").value = ramChoice(s.heap_gb);
  if (document.activeElement !== $("heap")) $("heap").value = s.heap_gb;
  if (document.activeElement !== $("gc")) $("gc").value = s.gc;
  $("ramNote").textContent = "This PC has " + s.ram_gb + " GB (Windows reports a little under the real size). Recommended game memory: " + s.default_heap + " GB.";
  $("heapWarn").hidden = !(s.heap_gb > s.default_heap);
  $("heapWarn").textContent = "Game memory " + s.heap_gb + " GB is above the " + s.default_heap + " GB recommended for this PC. Windows can run out of memory and the game can crash.";
  $("ignoredNote").hidden = s.ignored.length === 0;
  $("ignoredNote").textContent = "Ignored (set by the choices above): " + s.ignored.join(" ");
  stray = s.stray;
  const key = JSON.stringify(stray);
  if (key !== strayKey) { // rebuild only on change, so a half-edited list isn't reset every 5 s
    strayKey = key;
    $("strayBox").hidden = stray.length === 0;
    $("strayList").replaceChildren(...flagRows(stray));
    $("straySummary").textContent = "Flags from your own game settings (" + stray.filter(([, k]) => k === false).length + " of " + stray.length + " dropped)";
  }
  $("jfr").checked = s.jfr;
  $("potato").checked = s.potato;
  for (const k of DIAG) $(k).checked = s[k];
  if (DIAG.some((k) => s[k])) $("diag").open = true; // a ticked switch is never hidden
  $("jfrNote").textContent = "Saved to " + s.jfr_dir + " when you quit through the game menu (last 20 minutes). Applies from the next launch.";
  $("launch").disabled = s.running || !s.game_dir;
  $("launch").textContent = s.running ? "Project Zomboid is running" : "Launch Project Zomboid";
  const w = $("warnings");
  w.replaceChildren(...s.warnings.map((t) => Object.assign(document.createElement("p"), { className: "warn", textContent: t })));
  const box = $("components");
  box.replaceChildren(...s.components.map((c) => {
    const card = document.createElement("div");
    card.className = "card " + c.state;
    const title = Object.assign(document.createElement("h2"), { textContent: c.name });
    const ver = Object.assign(document.createElement("p"), { className: "muted", textContent: "Approved version " + c.version });
    const st = Object.assign(document.createElement("p"), { className: "state", textContent: STATE_TEXT[c.state] || c.state });
    const btn = Object.assign(document.createElement("button"), {
      textContent: c.state === "approved" ? "Reinstall" : "Install",
      disabled: s.running || !s.game_dir,
    });
    btn.addEventListener("click", () => run(btn, () => invoke("install", { component: c.id })));
    card.append(title, ver, st, btn);
    if (c.state === "approved") { // only our approved files; a different version is left alone
      const un = Object.assign(document.createElement("button"), { textContent: "Uninstall", disabled: s.running });
      un.addEventListener("click", async () => {
        if (await confirmBox("Remove " + c.name + " from the game folder? Launches go without it until you install it again."))
          run(un, () => invoke("uninstall", { component: c.id }));
      });
      card.append(un);
    }
    return card;
  }));
}

// in-page OK/Cancel (index.html #confirm); resolves true on OK
function confirmBox(text) {
  const d = $("confirm");
  $("confirmText").textContent = text;
  d.returnValue = "";
  d.showModal();
  return new Promise((res) => d.addEventListener("close", () => res(d.returnValue === "ok"), { once: true }));
}

async function run(btn, fn) {
  btn.disabled = true;
  try {
    log(await fn());
  } catch (e) {
    log(String(e), true);
  }
  btn.disabled = false;
  refresh();
}

// new stray flags are asked about once before launching (user 11-20); Cancel doesn't launch
async function askNewFlags() {
  const fresh = stray.filter(([, k]) => k === null);
  if (fresh.length === 0) return true;
  const d = $("flagsDlg");
  $("flagsList").replaceChildren(...flagRows(fresh));
  d.returnValue = "";
  d.showModal();
  const ok = await new Promise((res) => d.addEventListener("close", () => res(d.returnValue === "ok"), { once: true }));
  if (ok) log(await invoke("set_flags", { choices: flagChoices($("flagsList")) }));
  return ok;
}

$("launch").addEventListener("click", async () => {
  try {
    if (!(await askNewFlags())) return;
  } catch (e) {
    log(String(e), true);
    return;
  }
  run($("launch"), () => invoke("launch_game"));
});
$("saveJvm").addEventListener("click", () => run($("saveJvm"), () => invoke("set_jvm_args", { args: $("jvm").value })));
$("ram").addEventListener("change", () => {
  const gb = RAM_HEAP[$("ram").value];
  if (!gb) return;
  $("heap").value = gb;
  run($("ram"), () => invoke("set_heap", { gb }));
});
$("saveHeap").addEventListener("click", () => run($("saveHeap"), () => invoke("set_heap", { gb: Number($("heap").value) })));
$("gc").addEventListener("change", () => run($("gc"), () => invoke("set_gc", { gc: $("gc").value })));
$("saveStray").addEventListener("click", () => {
  strayKey = ""; // redraw from the saved answers
  run($("saveStray"), () => invoke("set_flags", { choices: flagChoices($("strayList")) }));
});
$("jfr").addEventListener("change", () => run($("jfr"), () => invoke("set_jfr", { on: $("jfr").checked })));
$("potato").addEventListener("change", () => run($("potato"), () => invoke("set_potato", { on: $("potato").checked })));
for (const k of DIAG) {
  $(k).addEventListener("change", () => run($(k), () => invoke("set_diag", { key: k, on: $(k).checked })));
}
$("resetJvm").addEventListener("click", () => { $("jvm").value = guided; });
$("saveGame").addEventListener("click", () => run($("saveGame"), () => invoke("set_game_dir", { dir: $("game").value })));

$("update").addEventListener("click", () => run($("update"), () => invoke("apply_update")));

async function checkUpdate() {
  try {
    const u = await invoke("check_update");
    if (u) {
      const [v, notes] = u;
      $("update").textContent = "Update to " + v;
      $("update").hidden = false;
      if (notes) {
        $("updateNotes").textContent = "New in " + v + ": " + notes;
        $("updateNotes").hidden = false;
      }
    }
  } catch (e) {
    log(String(e), true);
  }
}

refresh().catch((e) => log(String(e), true));
checkUpdate();
let busy = false; // never stack status calls
setInterval(async () => {
  if (busy) return;
  busy = true;
  try { await refresh(); } catch (_) {}
  busy = false;
}, 5000);
