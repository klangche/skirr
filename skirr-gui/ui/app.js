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
}
function clearError(id) {
  $(id).classList.add("hidden");
}

function esc(s) {
  const d = document.createElement("div");
  d.textContent = String(s);
  return d.innerHTML;
}

// --- Overview ---------------------------------------------------------------

async function loadOverview() {
  clearError("#overview-error");
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
}

function card(k, v, s) {
  return `<div class="card"><div class="k">${k}</div><div class="v">${v}</div><div class="s">${s}</div></div>`;
}

// --- Topology ----------------------------------------------------------------

function nodeHtml(n, depth) {
  const ids = `<span class="ids">${n.vid.toString(16).padStart(4, "0")}:${n.pid.toString(16).padStart(4, "0")}</span>`;
  const speed = n.speed_mbps ? ` <span class="speed">${n.speed_mbps} Mbps</span>` : "";
  const hubPorts = n.hub_ports != null ? `[HUB ${n.hub_ports}p]` : n.is_hub ? "[HUB]" : "";
  const hubmark = hubPorts ? ` <span class="hubmark">${hubPorts}</span>` : "";
  const dock = n.dock_family ? ` <span class="dock">${esc(n.dock_family)}</span>` : "";
  const kids = n.children.length
    ? `<ul>${n.children.map((c) => `<li>${nodeHtml(c, depth + 1)}</li>`).join("")}</ul>`
    : "";
  return `<span class="dev">${esc(n.label)} ${ids}${hubmark}${speed}${dock}</span>${kids}`;
}

async function loadTopology() {
  clearError("#topology-error");
  try {
    const t = await invoke("get_port_chains");
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

    if (!html.replace(/<h3[^>]*>[^<]*<\/h3>/g, "").trim()) {
      html += "<p class='free'>(nothing attached)</p>";
    }
    $("#topology-content").innerHTML = html;

    document.querySelectorAll(".rh-section .rh-title").forEach((title) => {
      title.addEventListener("click", () => title.parentElement.classList.toggle("closed"));
    });
  } catch (e) {
    showError("#topology-error", String(e));
  }
}

$("#topology-refresh").addEventListener("click", loadTopology);

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

loadOverview();
initMonitor();

// Lazy-load heavier views the first time they're opened.
let topologyLoaded = false;
new MutationObserver(() => {
  if ($("#view-topology").classList.contains("active") && !topologyLoaded) {
    topologyLoaded = true;
    loadTopology();
  }
}).observe($("#view-topology"), { attributes: true });

switchView("overview");
