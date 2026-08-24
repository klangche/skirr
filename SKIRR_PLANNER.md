# Skirr Project Planner

**Overall Progress: 17%** *(phase-weighted: Phases 0–1 complete)*

---

## Project Origin & Philosophy

Skirr is a **cross-platform USB analysis tool**, modeled on the philosophy of
[ProAV Shoko](https://github.com/klangche/klangche-proav-shoko) (local reference clone:
`~/Documents/GitHub/klangche-proav-shoko`). Skirr is the Rust rewrite of that concept.

**Core philosophy copied from ProAV Shoko:**

- **Analyze, verify, troubleshoot USB connections** in meeting rooms / BYOD / ProAV environments
- **Scan** all connected USB devices and build the **hierarchical USB tree**
- Calculate **hops** (levels in chain), **tiers** (depth), and count external hubs
- Assess **stability from chain length** using per-platform limits (Apple Silicon is stricter than Windows/Intel — internal Thunderbolt hub costs 1 tier)
- Show **connected displays** with resolution/connection path
- Generate **professional reports** (HTML; Shoko also does PDF) with topology diagrams
- **No admin/sudo required** for core functionality; degrade gracefully and say what is unknown
- Audience: AV technicians, IT support, sales, diagnostics teams

**Per-platform stability limits** (from Shoko's `src/assets/usb_data.csv`):

| System | max_hops | max_tiers | max_hubs |
|--------|----------|-----------|----------|
| Windows x86/ARM | 7 | 7 | 5 |
| macOS Intel | 7 | 7 | 5 |
| macOS Apple Silicon | 6 | 6 | 4 |
| Linux x86 | 7 | 7 | 5 |
| Linux ARM | 6 | 6 | 4 |

These become the default "Skirr Standard Profile v1.0" rules (Phase 1.3).

**Project tree is fixed as the owner wants it:** Cargo workspace with
`skirr-core`, `skirr-windows`, `skirr-macos`, `skirr-linux`, `skirr-cli`, `skirr-gui`.
Do not restructure.

## Release Targets (mandatory)

| OS | Arch | Artifact |
|----|------|----------|
| Windows | x64 | `.exe` (portable) |
| macOS (Apple Silicon) | ARM64 | `.dmg` |
| macOS (Intel) | x64 | `.dmg` |
| Ubuntu 22.04 | x86_64 | `.deb` |

CI must build all four artifacts on every release tag.

---

## Legend
- `[ ]` = Not started
- `[~]` = In progress
- `[x]` = Completed
- `🔴` = Blocked
- `🟡` = Needs review
- `🟢` = Ready for next agent

---

## Phase 0: Project Setup & Data Map (Prerequisite)

### 0.1 Initialize Rust Workspace
- [x] Create Cargo workspace structure
- [x] Add core crates: `skirr-core`, `skirr-windows`, `skirr-macos`, `skirr-linux`, `skirr-cli`
- [x] Configure Cargo.toml with dependencies (tauri, serde, thiserror, etc.)
- [x] Set up GitHub Actions matrix build: `windows-latest` (x64 .exe), `macos-14` (ARM64 .dmg), `macos-13` (Intel x64 .dmg), `ubuntu-22.04` (x86_64 .deb)
- **Progress: 100%**
- **Notes**: Workspace builds green (`cargo check/clippy/fmt/test`). Fixed invalid manifest entries from initial scaffold: removed nonexistent crates/versions (`iokit-sys`, `libudev 0.8`, `libusb 0.6`, `objc2` framework features), replaced `libusb` with maintained `rusb`, bumped `drm` to 0.15, corrected `windows` crate feature name, gated platform deps behind `[target.'cfg(target_os = ...)]`, made Tauri opt-in via skirr-gui `gui` feature. CI: `.github/workflows/ci.yml`; Release matrix: `.github/workflows/release.yml`.

### 0.2 Create Skirr Data Map (Section 27)
- [x] Document VID/PID retrieval per OS
- [x] Document Parent/Child/Hub topology per OS
- [x] Document Port enumeration per OS
- [x] Document Current/Max USB speed per OS
- [x] Document USB-C capabilities per OS
- [x] Document USB-PD information per OS
- [x] Document EDID/Display retrieval per OS
- [x] Document Thunderbolt/USB4 per OS
- [x] Document Hotplug monitoring per OS
- [x] Mark Admin/Driver requirements per datapoint
- [x] Assign confidence scores per datapoint
- **Progress: 100%**
- **Notes**: Delivered as `docs/DATA_MAP.md`. Techniques grounded in Shoko's implementation (PowerShell PnP parent map, system_profiler location_id tree, lsusb -t parsing) plus Rust-native primary paths (SetupAPI/CfgMgr32, IOKit, sysfs/libudev). Includes fallback chain, admin matrix, confidence scale, and Phase 13 crate/dep mapping.

---

## Phase 1: MVP - Core Data Model & Normalization

### 1.1 Define Common Data Model (skirr-core)
- [x] Device struct (VID, PID, manufacturer, product, serial, class, subclass, protocol)
- [x] Hub struct (ports, children, parent, depth)
- [x] Topology struct (host controllers, root hubs, tree)
- [x] Speed struct (max_supported, current_link, bottleneck)
- [x] Display struct (EDID, resolution, refresh, physical size, connection path)
- [x] USB-C struct (capability, alt_mode, power, thunderbolt, usb4)
- [x] DiagnosticEvent struct (timestamp, type, device_id, details)
- [x] Fact/Rule/Verdict structs for rule engine
- [x] Profile struct (version, rules, limits)
- **Progress: 100%**
- **Notes**: `model.rs` landed with initial scaffold; Profile added in `profile.rs` (`PlatformKey`, `StabilityLimits`, `RuleOverride`, `Profile::standard_v1()` with Shoko csv values + mobile reference rows). 4 unit tests; serde roundtrip verified.

### 1.2 Implement Normalization Traits
- [x] `UsbBackend` trait (enumerate, get_topology, get_speeds, monitor)
- [x] `DisplayBackend` trait (enumerate_displays, get_edid)
- [x] `UsbCBackend` trait (get_capabilities, get_power, get_thunderbolt)
- [x] `HotplugBackend` trait (start_monitoring, stop_monitoring, events)
- [x] Error types for each backend
- **Progress: 100%**
- **Notes**: Delivered in `skirr-core/src/backend.rs`. Single `BackendError` (Unsupported / PermissionDenied / OsApi / NotMonitoring) carries backend name context; object-safe traits (`Box<dyn UsbBackend>` verified). Sync design + channel-backed `poll_event(timeout)` keeps skirr-core runtime-free. Includes `StubBackend` (test/CLI bring-up) with lifecycle + event tests; 9 unit tests green.

### 1.3 Rule Engine Implementation
- [x] Profile loader (TOML/JSON)
- [x] Fact collector from normalized model
- [x] Rule evaluator (max hubs, max hops, max tiers, min speed, etc.)
- [x] Verdict generator (PASS/WARNING/FAIL)
- [x] Explanation formatter (FACT/RULE/VERDICT output)
- [x] Default "Skirr Standard Profile v1.0" — port Shoko's `usb_data.csv` limits (see table above)
- **Progress: 100%**
- **Notes**: `rule_engine.rs`: `RuleEngine::evaluate(SystemTopology) -> DiagnosticResult`. Chain metrics recomputed from parent links (not cached counters). Rules: max_hops/max_tiers/max_hubs (at-limit=WARNING, over=FAIL), speed_bottleneck (severity-mapped), orphaned_device. Unknown platform -> overall UNKNOWN. `format_report()` renders FACT/RULE/VERDICT text. Profile loaders in profile.rs (`from_toml_str/from_json_str/from_path/save`, `PlatformKey::detect`). Model additions: `UsbDevice.is_internal`, `FactCategory::Platform`, `Default` derives on EventSummary/HostControllerCapabilities. 18 unit tests green; local fmt/clippy clean (CI stays PAUSED per CI Status).

---

## Phase 2: MVP - Windows Backend (skirr-windows)

### 2.1 PnP/SetupAPI Enumeration
- [x] Enumerate all USB devices via SetupAPI 🟡
- [x] Extract VID, PID, manufacturer, product, serial
- [x] Get device class/subclass/protocol (class from PnP names; subclass/protocol land with 2.3 descriptor reads)
- [x] Get device descriptor information (hardware IDs parsed; full descriptor fetch deferred to 2.3 IOCTLs)
- **Progress: 100%** *(pending real-Windows verification)*
- **Notes**: `skirr-windows` restructured: `hwid.rs` (cross-platform parsers: VID/PID/MI, instance-path heuristics ported from Shoko `_parse_devpath`, class-name mapping), `native.rs` (`RawDeviceInfo`; `#[cfg(windows)]` SetupAPI collector via GUID_DEVCLASS_USB + registry properties + hardware-ID MULTI_SZ; PowerShell fallback = Shoko-proven query; JSON parsing testable everywhere), `backend.rs` (`SkirrWindowsBackend` implements core `UsbBackend`; 2.2–2.4 methods return Unsupported). 15 unit tests green off-Windows. **🟡 Needs review**: native path compiles by inspection only — CI is PAUSED; verify on a Windows runner before release tag (windows-rs 0.59 signature drift possible in `SetupDiGetClassDevsW`/`SetupDiGetDeviceRegistryPropertyW` calls).

### 2.2 Topology Construction
- [ ] Build parent/child relationships via PnP device tree
- [ ] Identify host controllers and root hubs
- [ ] Map hub ports to children
- [ ] Calculate depth, hops, tiers per device
- **Progress: 0%**

### 2.3 USB Speed Detection
- [ ] Implement IOCTL_USB_GET_NODE_CONNECTION_INFORMATION_EX
- [ ] Implement IOCTL_USB_GET_NODE_CONNECTION_INFORMATION_EX_V2
- [ ] Handle admin vs non-admin speed data
- [ ] Detect current link speed vs capability
- **Progress: 0%**

### 2.4 Hotplug Monitoring
- [ ] Register for WM_DEVICECHANGE notifications
- [ ] Track device arrival/removal/re-enumeration
- [ ] Emit normalized events
- **Progress: 0%**

---

## Phase 3: MVP - macOS Backend (skirr-macos)

### 3.1 IOKit/IORegistry Enumeration
- [ ] Enumerate USB devices via IOKit
- [ ] Extract VID, PID, manufacturer, product, serial
- [ ] Get device class/subclass/protocol
- [ ] Get device descriptor information
- [ ] system_profiler fallback for cross-check
- **Progress: 0%**

### 3.2 Topology Construction
- [ ] Build parent/child relationships via IORegistry
- [ ] Identify host controllers and root hubs
- [ ] Map hub ports to children
- [ ] Calculate depth, hops, tiers per device
- **Progress: 0%**

### 3.3 USB Speed Detection
- [ ] Get max supported speed from device properties
- [ ] Get current negotiated link speed
- [ ] Detect bottlenecks (USB 3 device on USB 2 port)
- **Progress: 0%**

### 3.4 Hotplug Monitoring
- [ ] IONotificationPortCreate for USB notifications
- [ ] Track device arrival/removal/re-enumeration
- [ ] Emit normalized events
- **Progress: 0%**

---

## Phase 4: MVP - CLI (skirr-cli)

### 4.1 Command Structure
- [ ] `skirr scan` - full enumeration output
- [ ] `skirr usb` - USB devices only
- [ ] `skirr topology` - tree view with hops/tiers
- [ ] `skirr hubs` - hub details with port mapping
- [ ] `skirr ports` - port-level details
- [ ] `skirr diagnose` - run rule engine, show verdict
- [ ] `skirr monitor` - live hotplug monitoring
- [ ] `skirr report` - generate JSON/HTML report
- **Progress: 0%**

### 4.2 Output Formatting
- [ ] Human-readable table output
- [ ] JSON output (--json flag)
- [ ] Structured diagnostic output (FACT/RULE/VERDICT)
- [ ] Color-coded PASS/WARNING/FAIL
- **Progress: 0%**

### 4.3 CLI Integration
- [ ] Backend selection (auto-detect OS)
- [ ] Profile selection (--profile flag)
- [ ] Monitoring duration (--duration flag)
- [ ] Output file (--output flag)
- **Progress: 0%**

---

## Phase 5: MVP - Distribution & Documentation

### 5.1 Build & Release Pipeline
- [ ] GitHub Actions workflow for 4-target matrix (see Release Targets above)
- [ ] Windows x64: portable `.exe` (no installer)
- [ ] macOS ARM64 (Apple Silicon): `.app` bundle → `.dmg` (with /Applications symlink + how-to-run note, like Shoko's DMG)
- [ ] macOS Intel x64: separate `.app` bundle → `.dmg`
- [ ] Ubuntu 22.04 x86_64: `.deb` package (binary in /usr/bin, desktop entry if GUI ships)
- [ ] Smoke test each artifact on CI runners (`--help`/CLI mode; USB-less runners tolerated, like Shoko)
- [ ] Automatic release on tag push with all 4 artifacts attached
- [ ] Checksums and signatures (optional)
- **Progress: 0%**

### 5.2 Gatekeeper/SmartScreen Documentation
- [ ] macOS: "Open Anyway" flow documentation
- [ ] Windows: "Run anyway" flow documentation
- [ ] In-app first-run guide
- [ ] Support page / README instructions
- **Progress: 0%**

### 5.3 MVP Verification
- [ ] Run on Windows x64 (admin + non-admin)
- [ ] Run on macOS ARM64 (Apple Silicon) + macOS x64 (Intel)
- [ ] Run on Ubuntu 22.04
- [ ] Verify topology matches system_profiler/Device Manager
- [ ] Verify speed detection accuracy
- [ ] Verify rule engine produces correct verdicts
- [ ] Verify CLI commands all work
- [ ] Verify JSON output schema matches spec
- **Progress: 0%**

---

## Phase 6: P1 - Linux Backend (skirr-linux)

### 6.1 sysfs/libusb Enumeration
- [ ] Parse /sys/bus/usb/devices/ for device tree
- [ ] libusb fallback for missing sysfs data
- [ ] Extract VID, PID, manufacturer, product, serial
- [ ] Get device class/subclass/protocol
- **Progress: 0%**

### 6.2 Topology Construction
- [ ] Build parent/child from sysfs symlinks
- [ ] Identify host controllers and root hubs
- [ ] Map hub ports to children
- [ ] Calculate depth, hops, tiers
- **Progress: 0%**

### 6.3 USB Speed Detection
- [ ] Read max speed from sysfs (speed file)
- [ ] Read current speed from sysfs
- [ ] libusb for detailed capability
- **Progress: 0%**

### 6.4 Hotplug Monitoring
- [ ] udev monitor for USB events
- [ ] Track device arrival/removal/re-enumeration
- [ ] Emit normalized events
- **Progress: 0%**

### 6.5 Display/EDID on Linux
- [ ] DRM/KMS for display enumeration
- [ ] EDID parsing
- [ ] Connection path correlation with USB topology
- **Progress: 0%**

---

## Phase 7: P1 - USB-C & Display Diagnostics

### 7.1 USB-C Capabilities (All Platforms)
- [ ] Detect USB-C ports vs USB-A
- [ ] DisplayPort Alt Mode detection
- [ ] USB4/Thunderbolt detection (where exposed)
- [ ] Power Delivery info (where exposed)
- [ ] "Unknown" with reason when not exposed
- **Progress: 0%**

### 7.2 Display Diagnostics (All Platforms)
- [ ] Windows: EnumDisplayDevices + EDID
- [ ] macOS: IOKit display services + EDID
- [ ] Linux: DRM/KMS + EDID
- [ ] Correlation: display → GPU → USB path (dock/hub)
- [ ] HDR detection where available
- **Progress: 0%**

### 7.3 Dock Analysis
- [ ] Identify known dock VID/PIDs
- [ ] Map dock internal hub topology
- [ ] Port mapping (which port = video, which = data, etc.)
- [ ] Power delivery from dock
- **Progress: 0%**

---

## Phase 8: P1 - Live Monitoring & Reports

### 8.1 Live Monitoring Enhancement
- [ ] Configurable monitoring duration
- [ ] Event correlation (re-enumeration chains)
- [ ] Stability scoring (flapping detection)
- [ ] Summary statistics
- **Progress: 0%**

### 8.2 JSON Export
- [ ] Schema matching Section 21
- [ ] All sections: platform, controllers, devices, hubs, topology, displays, usb_c, thunderbolt, usb4, events, diagnostics, rules
- [ ] Pretty-print and compact options
- **Progress: 0%**

### 8.3 HTML Report Generator
- [ ] report.html with embedded CSS/JS
- [ ] Interactive topology tree
- [ ] Speed bottleneck visualization
- [ ] Event timeline
- [ ] PASS/WARNING/FAIL summary
- [ ] Zip bundle (report.html + all .json files)
- **Progress: 0%**

---

## Phase 9: P1 - Tauri GUI (skirr-gui)

### 9.1 Project Setup
- [ ] Tauri 2.x project structure
- [ ] Connect to skirr-core via Rust commands
- [ ] Basic window + menu
- **Progress: 0%**

### 9.2 Main Views
- [ ] System overview (platform, USB summary)
- [ ] Topology tree view (expandable)
- [ ] Hub details (port map)
- [ ] Device details (speed, capabilities)
- [ ] USB-C / Thunderbolt / USB4 panel
- [ ] Displays panel
- [ ] Live monitoring view
- [ ] Diagnostics/Results view
- [ ] Export report button
- **Progress: 0%**

### 9.3 GUI Polish
- [ ] Dark/light theme
- [ ] Responsive layout
- [ ] Loading states
- [ ] Error handling UI
- [ ] First-run Gatekeeper guide
- **Progress: 0%**

---

## Phase 10: P2 - Advanced Features

### 10.1 Thunderbolt/USB4 Deep Dive
- [ ] Thunderbolt topology (separate from USB)
- [ ] USB4 router detection
- [ ] Bandwidth allocation analysis
- [ ] Thunderbolt security levels
- **Progress: 0%**

### 10.2 Power Analysis
- [ ] USB-PD negotiation details
- [ ] CC state detection
- [ ] Cable capability (E-marker)
- [ ] Power role (source/sink/DRP)
- **Progress: 0%**

### 10.3 Bandwidth Analysis
- [ ] Calculate available vs used bandwidth
- [ ] Display bandwidth requirements
- [ ] Hub bottleneck identification
- [ ] Multi-display bandwidth planning
- **Progress: 0%**

### 10.4 Advanced Event Correlation
- [ ] Root cause analysis for re-enumerations
- [ ] Pattern detection (periodic drops)
- [ ] Correlation with display events
- [ ] Export timeline for support
- **Progress: 0%**

---

## Phase 11: P3 - Hardware-Specific Details

### 11.1 USB-PD Deep Details
- [ ] PDO/APDO parsing
- [ ] Voltage/current negotiation history
- [ ] Cable wattage limits
- [ ] PPS support detection
- **Progress: 0%**

### 11.2 Cable & Connector Details
- [ ] Cable VID/PID (E-marker)
- [ ] Cable USB version/speed rating
- [ ] Connector orientation (CC1/CC2)
- [ ] Physical port identification
- **Progress: 0%**

### 11.3 Advanced Error Counters
- [ ] Link error counts (where exposed)
- [ ] Retry counters
- [ ] CRC error rates
- [ ] LTSSM state tracking
- **Progress: 0%**

---

## Current Status Summary

| Phase | Name | Progress | Status |
|-------|------|----------|--------|
| 0 | Project Setup & Data Map | 100% | [x] Completed |
| 1 | Core Data Model & Normalization | 100% | [x] Completed |
| 2 | Windows Backend | 25% | [~] In progress (2.1 done, 🟡 runner review pending) |
| 3 | macOS Backend | 0% | [ ] Not started |
| 4 | CLI | 0% | [ ] Not started |
| 5 | Distribution & Documentation | 0% | [ ] Not started |
| 6 | Linux Backend | 0% | [ ] Not started |
| 7 | USB-C & Display Diagnostics | 0% | [ ] Not started |
| 8 | Live Monitoring & Reports | 0% | [ ] Not started |
| 9 | Tauri GUI | 0% | [ ] Not started |
| 10 | Advanced Features (P2) | 0% | [ ] Not started |
| 11 | Hardware Details (P3) | 0% | [ ] Not started |

**Total Project Progress: 17%** *(phase-weighted: Phases 0–1 complete; MVP core done)*

### CI Status (owner decision, 2026-08-24)

**Automatic CI runs are PAUSED on `dev`** — `.github/workflows/ci.yml` triggers are
commented out (`workflow_dispatch` only) until backends reach a more stable state.
Rules while paused:

1. Agents MUST still verify locally before handoff: `cargo fmt --all --check`,
   `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`.
2. Known re-enable blocker: ubuntu runner needs `pkg-config` + `libudev-dev`
   installed before `cargo clippy/test` (the `libudev` crate links system libudev).
   Fix is already noted as a comment inside ci.yml.
3. Release workflow (`release.yml`) is tag-triggered and unaffected.
4. Re-enable CI when Phase 2 or 3 first lands a working backend, whichever comes
   first. Real device/hardware testing itself stays deferred to Phase 5.3
   (MVP Verification), which is intentionally far out; local unit tests are the
   only quality gate until then.

---

## Agent Coordination Rules

1. **One task at a time**: Pick the first `[ ]` task in order, mark `[~]`, complete, mark `[x]`
2. **Update progress**: After each task, update the phase progress percentage and total project progress
3. **Document blockers**: If blocked, mark `🔴` and add note in task
4. **Handoff ready**: When task is `[x]`, next agent can pick up next `[ ]`
5. **Never skip**: Tasks must be done in order within a phase (dependencies)
6. **Cross-phase**: Phase N+1 cannot start until Phase N is 100%
7. **CI paused**: While CI Status says PAUSED, run the three local verification commands manually on every handoff (see CI Status above); no push-watching expected

---

## Next Task for Agent

**Current**: Phase 2.2 - Topology Construction (skirr-windows)
**Action**: Build parent/child relationships on Windows behind `#[cfg(windows)]`:
- Query `DEVPKEY_Device_Parent` per device (`SetupDiGetDevicePropertyW`, feature `Win32_Devices_Properties`) — bulk PowerShell variant already proven in Shoko as fallback
- Identify host controllers (`GUID_DEVCLASS_USB` roots / enumerator `PCI`) and root hubs
- Map hub ports: parse instance path port segment via existing `hwid::parse_instance_path`
- Compute depth/hops/tiers per device and fill `SystemTopology` (host_controllers, root_hubs, devices with parent_id/children_ids/port_number)
- Reuse core `rule_engine::compute_chain_metrics`-style semantics; keep non-Windows builds green (Unsupported elsewhere)
**Verify locally**: cargo test/check/clippy on macOS must pass; real verification deferred to Windows runner (see CI Status)
**Reference**: DATA_MAP.md §2 Windows column; Shoko usb_topology.py `_win_parent_map`
---
*Last updated: 2026-08-24 | Phase 2.1 done (🟡 native path needs Windows-runner review); next agent: Phase 2.2*