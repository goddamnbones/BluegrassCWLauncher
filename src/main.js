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

let defaults = "";

// RAM dropdown -> heap GB. Same table as launch.rs heap_for_ram; keep them matching.
const RAM_HEAP = { 8: 4, 12: 6, 16: 8, 32: 12, 64: 16 };

function heapOf(args) {
  const m = /(?:^|\s)-Xmx(\d+)g(?:\s|$)/i.exec(args);
  return m ? Number(m[1]) : null;
}

// the option whose heap matches the saved -Xmx, else "custom"
function ramChoice(args) {
  const h = heapOf(args);
  const hit = Object.keys(RAM_HEAP).find((k) => RAM_HEAP[k] === h);
  return hit || "custom";
}

function withHeap(args, gb) {
  const rest = args.split(/\s+/).filter((a) => a && !/^-Xm[sx]/i.test(a));
  return ["-Xms" + gb + "g", "-Xmx" + gb + "g", ...rest].join(" ");
}

async function refresh() {
  const s = await invoke("get_status");
  defaults = s.default_jvm_args;
  $("version").textContent = "launcher " + s.launcher;
  if (document.activeElement !== $("jvm")) $("jvm").value = s.jvm_args;
  if (document.activeElement !== $("game")) $("game").value = s.game_dir || "";
  if (document.activeElement !== $("ram")) $("ram").value = ramChoice(s.jvm_args);
  $("ramNote").textContent = "This PC has " + s.ram_gb + " GB (Windows reports a little under the real size).";
  $("jfr").checked = s.jfr;
  $("potato").checked = s.potato;
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
    return card;
  }));
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

$("launch").addEventListener("click", () => run($("launch"), () => invoke("launch_game")));
$("saveJvm").addEventListener("click", () => run($("saveJvm"), () => invoke("set_jvm_args", { args: $("jvm").value })));
$("ram").addEventListener("change", () => {
  const gb = RAM_HEAP[$("ram").value];
  if (!gb) return;
  const args = withHeap($("jvm").value || defaults, gb);
  $("jvm").value = args;
  run($("ram"), () => invoke("set_jvm_args", { args }));
});
$("jfr").addEventListener("change", () => run($("jfr"), () => invoke("set_jfr", { on: $("jfr").checked })));
$("potato").addEventListener("change", () => run($("potato"), () => invoke("set_potato", { on: $("potato").checked })));
$("resetJvm").addEventListener("click", () => { $("jvm").value = defaults; });
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
