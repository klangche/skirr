// Skirr GUI — vanilla JS frontend. All data comes from Rust commands via
// Tauri IPC; this file only renders and never talks to the OS directly.
const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const { save } = window.__TAURI__.dialog;

const $ = (sel) => document.querySelector(sel);

// --- Tabs -----------------------------------------------------------------

document.querySelectorAll("#tabs .tab").forEach((tab) => {
  tab.addEventListener("click", () => switchView(tab.dataset.view));
});

function switchView(name) {
  document.querySelectorAll("#tabs .tab").forEach((t) =>
    t.classList.toggle("active", t.dataset.view === name)
  );
  document.querySelectorAll(".view").forEach((v) =>
    v.classList.toggle("active", v.id === `view-${name}`)
  );
}

function showError(id, message) {
  const el = $(id);
  el.textContent = message;
  el.classList.remove("hidden");
  toast(message, "error");
}
function clearError(id) {
  $(id).classList.add("hidden");
}

// --- Toasts -------------------------------------------------------------------

function toast(message, kind = "info", ttlMs = 6000) {
  const el = document.createElement("div");
  el.className = `toast ${kind}`;
  el.textContent = message;
  $("#toasts").appendChild(el);
  setTimeout(() => {
    el.classList.add("leaving");
    setTimeout(() => el.remove(), 300);
  }, ttlMs);
}

// --- Loading spinner ------------------------------------------------------------

// Wraps an async render function: swaps the container's content for a spinner
// while the promise is in flight, then restores it.
async function withSpinner(containerSel, fn) {
  const el = $(containerSel);
  if (!el || el.querySelector(".spinner-wrap")) return fn();
  const prev = el.innerHTML;
  el.innerHTML =
    '<div class="spinner-wrap"><div class="spinner" role="status" aria-label="loading"></div></div>';
  try {
    await fn();
  } finally {
    const spin = el.querySelector(".spinner-wrap");
    if (spin) spin.remove();
    // If fn() didn't render anything (early return / error), restore prior view
    // only when it stayed empty so we never blank out working content.
    if (!el.innerHTML.trim()) el.innerHTML = prev;
  }
}

// --- Theme toggle ------------------------------------------------------------------

const THEME_KEY = "skirr-theme";

function initTheme() {
  const saved = localStorage.getItem(THEME_KEY);
  if (saved === "light" || saved === "dark") {
    document.documentElement.dataset.theme = saved;
  }
  $("#theme-toggle").addEventListener("click", () => {
    const current =
      document.documentElement.dataset.theme ||
      (matchMedia("(prefers-color-scheme: light)").matches ? "light" : "dark");
    const next = current === "dark" ? "light" : "dark";
    document.documentElement.dataset.theme = next;
    localStorage.setItem(THEME_KEY, next);
  });
}

// --- First-run Gatekeeper guide (macOS) -----------------------------------------------

const GATEKEEPER_KEY = "skirr-gatekeeper-dismissed";

function initGatekeeperCard() {
  const isMac = navigator.userAgent.includes("Macintosh") || navigator.platform.startsWith("Mac");
  if (!isMac || localStorage.getItem(GATEKEEPER_KEY)) return;
  const card = $("#gatekeeper-card");
  card.classList.remove("hidden");
  $("#gatekeeper-dismiss").addEventListener("click", () => {
    localStorage.setItem(GATEKEEPER_KEY, "1");
    card.classList.add("hidden");
  });
}

function esc(s) {
  const d = document.createElement("div");
  d.textContent = String(s);
  return d.innerHTML;
}

// --- Overview ---------------------------------------------------------------

async function loadOverview() {
  clearError("#overview-error");
  return withSpinner("#overview-cards", async () => {
  try {
    const o = await invoke("get_overview");
    const verdictClass =
      { PASS: "sev-info", WARNING: "sev-warning", FAIL: "sev-critical" }[o.verdict] ?? "";
    $("#overview-cards").innerHTML = [
      card("Verdict", `<span class="${verdictClass}">${esc(o.verdict)}</span>`,
        `${o.warning_rules} warning · ${o.failed_rules} failed rule(s)`),
      card("Devices", o.connected_devices, `${o.device_count} enumerated on ${o.hub_count} hub(s)`),
      card("Displays", o.display_count, ""),
      card("Controllers", o.controller_count, `backend: ${esc(o.backend)}`),
      card("OS", `${esc(o.os)} ${esc(o.os_version)}`, `${esc(o.architecture)}${o.is_admin ? " · admin" : ""}${o.is_virtual_machine ? " · VM" : ""}`),
      card("Host", esc(o.hostname ?? "(unknown)"), ""),
    ].join("");
  } catch (e) {
    showError("#overview-error", String(e));
  }
  });
}

function card(k, v, s) {
  return `<div class="card"><div class="k">${k}</div><div class="v">${v}</div><div class="s">${s}</div></div>`;
}

// --- Topology ----------------------------------------------------------------

let detailsCache = null; // DetailsPayload from get_details, keyed by device id

async function loadDetails() {
  if (!detailsCache) detailsCache = await invoke("get_details");
  return detailsCache;
}

function nodeHtml(n, depth) {
  const ids = `<span class="ids">${n.vid.toString(16).padStart(4, "0")}:${n.pid.toString(16).padStart(4, "0")}</span>`;
  const speed = n.speed_mbps ? ` <span class="speed">${n.speed_mbps} Mbps</span>` : "";
  const hubPorts = n.hub_ports != null ? `[HUB ${n.hub_ports}p]` : n.is_hub ? "[HUB]" : "";
  const hubmark = hubPorts ? ` <span class="hubmark">${hubPorts}</span>` : "";
  const dock = n.dock_family ? ` <span class="dock">${esc(n.dock_family)}</span>` : "";
  const kids = n.children.length
    ? `<ul>${n.children.map((c) => `<li>${nodeHtml(c, depth + 1)}</li>`).join("")}</ul>`
    : "";
  return `<span class="dev clickable" data-device-id="${n.id}" title="Show details">`
    + `${esc(n.label)} ${ids}${hubmark}${speed}${dock}</span>${kids}`;
}

async function loadTopology() {
  clearError("#topology-error");
  return withSpinner("#topology-content", async () => {
  try {
    const [t] = await Promise.all([invoke("get_port_chains"), loadDetails()]);
    let html = "";

    if (t.internal.length) {
      html += `<h3 class="internal-h">Internal</h3><ul class="tree">${
        t.internal.map((n) => `<li>${nodeHtml(n, 0)}</li>`).join("")}</ul>`;
    }

    html += `<h3 class="internal-h">External — one chain per port</h3>`;
    for (const rh of t.external) {
      const portsHtml = rh.ports
        .map((p) => {
          const label = p.port != null ? `Port ${p.port}` : "Port ?";
          return `<div class="port-line">${label}</div><ul class="tree"><li>${nodeHtml(p.root, 0)}</li></ul>`;
        })
        .join("");
      const free = rh.free_ports.length
        ? `<div class="free">Ports free: ${rh.free_ports.join(", ")}</div>`
        : "";
      html += `<details class="rh-section" open><summary class="rh-title">${
        esc(rh.platform_id)} (${rh.port_count} ports)</summary><div class="rh-body">${portsHtml}${free}</div></details>`;
    }

    html += renderHubMaps();

    if (!html.replace(/<h3[^>]*>[^<]*<\/h3>/g, "").trim()) {
      html += "<p class='free'>(nothing attached)</p>";
    }
    $("#topology-content").innerHTML = html;

    document.querySelectorAll(".rh-section .rh-title").forEach((title) => {
      title.addEventListener("click", () => title.parentElement.classList.toggle("closed"));
    });
    document.querySelectorAll(".dev.clickable").forEach((el) => {
      el.addEventListener("click", () => showDeviceDetails(el.dataset.deviceId));
    });
  } catch (e) {
      showError("#topology-error", String(e));
  }
  });
}

// Hub port maps: one collapsible map per physical hub, rendered after the
// root-hub chain sections.
function renderHubMaps() {
  if (!detailsCache?.hubs?.length) return "";
  return detailsCache.hubs
    .map((hub) => {
      const slots = hub.ports
        .map((s) => {
          const occupant = s.device_label
            ? `${esc(s.device_label)} <span class="ids">${s.vid.toString(16).padStart(4, "0")}:${s.pid.toString(16).padStart(4, "0")}</span>`
            : "<span class='free'>empty</span>";
          return `<tr><td>p${s.number}</td><td>${occupant}</td></tr>`;
        })
        .join("");
      return `<details class="hub-map"><summary class="rh-title">Port map — ${esc(hub.label)} (${hub.port_count}p)</summary>`
        + `<table class="rule-table"><tr><th>Port</th><th>Device</th></tr>${slots}</table></details>`;
    })
    .join("");
}

// --- Device details panel ------------------------------------------------------

function showDeviceDetails(id) {
  const dev = detailsCache?.devices?.find((d) => d.id === id);
  const panel = $("#details-panel");
  if (!dev) {
    panel.classList.add("hidden");
    return;
  }
  const row = (k, v) => (v == null || v === "" ? "" : `<tr><th>${k}</th><td>${v}</td></tr>`);
  const ids = `${dev.vid.toString(16).padStart(4, "0")}:${dev.pid.toString(16).padStart(4, "0")}`;
  const speedNote =
    dev.max_speed_mbps && dev.current_speed_mbps && dev.current_speed_mbps < dev.max_speed_mbps
      ? `<span class="sev-warning">running below max</span>`
      : "";
  const usbC = dev.usb_c
    ? `<h3>USB-C</h3><table>${
        row("Port type", esc(dev.usb_c.port_type)) +
        row("Mode", esc(dev.usb_c.current_mode)) +
        row("PD", dev.usb_c.pd_supported ? `yes${dev.usb_c.pd_revision ? ` (${esc(dev.usb_c.pd_revision)})` : ""}` : "no") +
        (dev.usb_c.alt_modes.length
          ? row("Alt modes", dev.usb_c.alt_modes.map((m) => esc(m)).join(", "))
          : "")
      }</table>`
    : "";
  panel.innerHTML =
    `<button id="details-close" class="hint">✕ close</button>
     <h2>${esc(dev.label)}</h2>
     <table>
       ${row("IDs", `<span class="ids">${ids}</span>`)}
       ${row("Platform ID", `<code>${esc(dev.platform_id)}</code>`)}
       ${row("Manufacturer", esc(dev.manufacturer ?? ""))}
       ${row("Serial", esc(dev.serial_number ?? ""))}
       ${row("Class", esc(dev.class))}
       ${row("Max speed", dev.max_speed_mbps ? `${dev.max_speed_mbps} Mbps` : "")}
       ${row("Link speed", dev.current_speed_mbps ? `${dev.current_speed_mbps} Mbps ${speedNote}` : "")}
       ${row("Port", dev.port_number ?? "")}
       ${row("Tier / hops", dev.tier ? `${dev.tier} / ${dev.hop_count}` : "")}
       ${row("Status", esc(dev.status))}
       ${row("Dock family", esc(dev.dock_family ?? ""))}
       ${dev.power_contract_mw ? row("PD contract", `${dev.power_contract_mw} mW`) : ""}
       ${dev.has_thunderbolt || dev.has_usb4 ? row("Modes", [dev.has_thunderbolt ? "Thunderbolt" : "", dev.has_usb4 ? "USB4" : ""].filter(Boolean).join(", ")) : ""}
     </table>${usbC}`;
  panel.classList.remove("hidden");
  $("#details-close").addEventListener("click", () => panel.classList.add("hidden"));
}

// --- Displays --------------------------------------------------------------------

async function loadDisplays() {
  clearError("#displays-error");
  return withSpinner("#displays-content", async () => {
  try {
    const details = await loadDetails();
    const list = details.displays;
    $("#displays-content").innerHTML = list.length
      ? `<div class="cards">${list
          .map(
            (d) => `<div class="card">
              <div class="k">${d.primary ? "Primary · " : ""}${d.internal ? "Internal" : "External"}${
                d.connection_type ? ` · ${esc(d.connection_type)}` : ""
              }</div>
              <div class="v" style="font-size:1.05rem">${esc(d.name)}</div>
              <div class="s">
                ${d.current_resolution ? `${esc(d.current_resolution)}${d.refresh_hz ? ` @ ${d.refresh_hz} Hz` : ""}` : "no mode"}
                ${d.preferred_resolution && d.current_resolution && d.preferred_resolution !== d.current_resolution
                  ? `<br/>prefers ${esc(d.preferred_resolution)}` : ""}
                ${d.hdr ? "<br/><span class='sev-info'>HDR capable</span>" : ""}
              </div>
            </div>`
          )
          .join("")}</div>`
      : "<p class='free'>No displays detected.</p>";
  } catch (e) {
      showError("#displays-error", String(e));
  }
  });
}

$("#displays-refresh").addEventListener("click", loadDisplays);

$("#topology-refresh").addEventListener("click", () => {
  detailsCache = null;
  loadTopology();
});

// --- Monitor ------------------------------------------------------------------

let monitoring = false;
let monitorUnlisten = null;

$("#monitor-toggle").addEventListener("click", async () => {
  const btn = $("#monitor-toggle");
  if (!monitoring) {
    $("#monitor-log").innerHTML = "";
    monitoring = await invoke("monitor_start");
    if (monitoring) {
      btn.textContent = "Stop monitoring";
      btn.classList.add("danger");
      $("#monitor-state").textContent = "listening…";
    }
  } else {
    await invoke("monitor_stop");
  }
});

async function initMonitor() {
  monitorUnlisten = await listen("monitor-event", (ev) => appendMonitor(ev.payload));
  await listen("monitor-stopped", () => {
    monitoring = false;
    const btn = $("#monitor-toggle");
    btn.textContent = "Start monitoring";
    btn.classList.remove("danger");
    $("#monitor-state").textContent = "";
  });
}

function appendMonitor(p) {
  const log = $("#monitor-log");
  const entry = document.createElement("div");
  entry.className = "log-entry";
  entry.innerHTML =
    `<span class="log-kind sev-${p.severity}">${esc(p.kind)}</span><span>${esc(p.text)}</span>`;
  log.appendChild(entry);
  log.scrollTop = log.scrollHeight;
}

// --- Diagnose -------------------------------------------------------------------

$("#diagnose-run").addEventListener("click", async () => {
  clearError("#diagnose-error");
  try {
    const d = await invoke("diagnose");
    const rows = d.rules_applied
      .map(
        (r) => `<tr><td>${esc(r.rule_id)}</td>
          <td><span class="sev-${r.verdict === "Fail" ? "critical" : r.verdict === "Warning" ? "warning" : "info"}">${esc(String(r.verdict).toUpperCase())}</span></td>
          <td>${esc(r.explanation)}</td></tr>`
      )
      .join("");
    $("#diagnose-result").innerHTML =
      `<span class="verdict-badge verdict-${esc(String(d.overall_verdict).toUpperCase())}">${esc(
        String(d.overall_verdict).toUpperCase()
      )}</span>` +
      (rows
        ? `<table class="rule-table"><tr><th>Rule</th><th>Verdict</th><th>Explanation</th></tr>${rows}</table>`
        : "<p class='hint'>No rules triggered.</p>") +
      (d.recommendations?.length
        ? `<h3 class="internal-h">Recommendations</h3><ul>${d.recommendations
            .map((r) => `<li>${esc(r)}</li>`)
            .join("")}</ul>`
        : "");
  } catch (e) {
    showError("#diagnose-error", String(e));
  }
});

// --- Report -----------------------------------------------------------------------

$("#report-save").addEventListener("click", async () => {
  const path = await save({
    defaultPath: "skirr-report.json",
    filters: [{ name: "JSON", extensions: ["json"] }],
  });
  if (!path) return;
  const htmlToo = $("#report-html").checked;
  try {
    const written = await invoke("generate_report", { path, htmlToo });
    $("#report-status").innerHTML = written.map((w) => `<p>Written: <code>${esc(w)}</code></p>`).join("");
  } catch (e) {
    $("#report-status").innerHTML = `<div class="error">${esc(String(e))}</div>`;
  }
});

// --- Boot ----------------------------------------------------------------------------

initTheme();
initGatekeeperCard();
loadOverview();
initMonitor();

// Lazy-load heavier views the first time they're opened.
let topologyLoaded = false;
let displaysLoaded = false;
const viewObserver = new MutationObserver(() => {
  if ($("#view-topology").classList.contains("active") && !topologyLoaded) {
    topologyLoaded = true;
    loadTopology();
  }
  if ($("#view-displays").classList.contains("active") && !displaysLoaded) {
    displaysLoaded = true;
    loadDisplays();
  }
});
viewObserver.observe($("#view-topology"), { attributes: true });
viewObserver.observe($("#view-displays"), { attributes: true });

switchView("overview");
