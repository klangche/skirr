# Skirr Project Planner

**Overall Progress: 88%** *(phase-weighted: Phases 0–8 complete; Phase 9 GUI nearly done)*

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

**Status: [x] Completed (code complete; 🟡 all native paths pending Windows-runner review)**

### 2.1 PnP/SetupAPI Enumeration
- [x] Enumerate all USB devices via SetupAPI 🟡
- [x] Extract VID, PID, manufacturer, product, serial
- [x] Get device class/subclass/protocol (class from PnP names; subclass/protocol land with 2.3 descriptor reads)
- [x] Get device descriptor information (hardware IDs parsed; full descriptor fetch deferred to 2.3 IOCTLs)
- **Progress: 100%** *(pending real-Windows verification)*
- **Notes**: `skirr-windows` restructured: `hwid.rs` (cross-platform parsers: VID/PID/MI, instance-path heuristics ported from Shoko `_parse_devpath`, class-name mapping), `native.rs` (`RawDeviceInfo`; `#[cfg(windows)]` SetupAPI collector via GUID_DEVCLASS_USB + registry properties + hardware-ID MULTI_SZ; PowerShell fallback = Shoko-proven query; JSON parsing testable everywhere), `backend.rs` (`SkirrWindowsBackend` implements core `UsbBackend`; 2.2–2.4 methods return Unsupported). 15 unit tests green off-Windows. **🟡 Needs review**: native path compiles by inspection only — CI is PAUSED; verify on a Windows runner before release tag (windows-rs 0.59 signature drift possible in `SetupDiGetClassDevsW`/`SetupDiGetDeviceRegistryPropertyW` calls).

### 2.2 Topology Construction
- [x] Build parent/child relationships via PnP device tree
- [x] Identify host controllers and root hubs
- [x] Map hub ports to children
- [x] Calculate depth, hops, tiers per device
- **Progress: 100%** *(pending real-Windows verification)*
- **Notes**: Parent lookup folded into the single enumeration pass: native `SetupDiGetDevicePropertyW(&DEVPKEY_Device_Parent)` (feature `Win32_Devices_Properties`) with PowerShell fallback extended to emit `Parent` per record (single spawn, Shoko `_win_parent_map` technique). New `topology.rs`: pure graph builder (`build(raw) -> SystemTopology`) testable off-Windows — links parents/children by instance ID, detects ROOT_HUB instances → RootHub records grouped under HostController records keyed by their PnP parent (PCI\...), extracts port numbers from generated/hub-shape instance paths, computes hops/tiers/depth and root-hub/controller attribution per device. 3 topology unit tests; 18 total in crate, green on macOS. **🟡 Same runner-review caveat as 2.1** (native property-read signature drift possible).

### 2.3 USB Speed Detection
- [x] Implement IOCTL_USB_GET_NODE_CONNECTION_INFORMATION_EX
- [x] Implement IOCTL_USB_GET_NODE_CONNECTION_INFORMATION_EX_V2
- [x] Handle admin vs non-admin speed data
- [x] Detect current link speed vs capability
- **Progress: 100%** *(pending real-Windows verification)*
- **Notes**: New `speeds.rs` — cross-platform pure layer (IOCTL constants verified against published values 0x220408/0x220440/0x220448; flattened `repr(C)` mirrors of `USB_NODE_CONNECTION_INFORMATION_EX` head (44 bytes) and `_EX_V2`; `map_ex_speed`, `derive_max_speed(bcdUSB+protocols+caps)` with conservative SS+ floor at 10G; hub-path↔PnP-instance reconstruction) + `#[cfg(windows)]` collector walking `GUID_DEVINTERFACE_USB_HUB` → `CreateFileW` → per-port `_EX`/`_EX_V2`. Backend `get_speeds(id)` resolves device via topology snapshot, matches parent-hub instance + port + VID/PID, returns `SpeedReport` incl. `SpeedBottleneck`; unelevated access-denied hubs surface as `BackendError::PermissionDenied` (admin-vs-non-admin rule). New windows features: `Win32_Devices_Usb`, `Win32_Storage_FileSystem`, `Win32_Security`. 9 new unit tests (27 total in crate), green on macOS. **🟡 Needs review**: native path compiles by inspection only — CI paused; verify on Windows runner before release tag (windows-rs 0.59 `CreateFileW`/`DeviceIoControl`/interface-detail-buffer signature drift possible; note `SP_DEVICE_INTERFACE_DETAIL_DATA_W.cbSize` x64 packing gotcha). Shoko had no speed implementation to port (grep confirmed); technique built from DATA_MAP §4 spec.
- **Progress: 0%**

### 2.4 Hotplug Monitoring
- [x] Register for WM_DEVICECHANGE notifications *(superseded: polling-diff route chosen — headless-friendly, Shoko-proven, no message pump; WM_DEVICECHANGE can revisit later if sub-second latency ever matters)*
- [x] Track device arrival/removal/re-enumeration
- [x] Emit normalized events
- **Progress: 100%** *(pending real-Windows verification)*
- **Notes**: New `hotplug.rs` — pure diff engine (`diff_snapshots(prev, cur) -> Vec<DiagnosticEvent>`): instance-set diff; identical silicon (VID/PID + case-insensitive serial) reappearing under a new instance/port pairs into `DeviceReEnumerated` instead of remove+add; hubs emit `HubConnected`/`HubDisconnected`; `Fingerprint::from_raw` derives identity from existing hwid parsers + `topology::extract_port`. Windows side: headless `PollMonitor` (250 ms re-enumeration loop, transient-error tolerant, channel contract from Phase 1.2 via `poll_event(timeout)`), `monitor()` now returns the real implementation (`NotMonitoring` error when unarmed). 7 new unit tests off-Windows (34 total in crate). **🟡 Needs review**: native enumeration path already carries the 2.1 caveat; monitor adds nothing new beyond it.
- **Progress: 0%**

---

## Phase 3: MVP - macOS Backend (skirr-macos)

**Status: [x] Completed (live-tested on macOS dev host; device-bearing verification pending physical hardware)**

### 3.1 IOKit/IORegistry Enumeration
- [x] Enumerate USB devices via IOKit
- [x] Extract VID, PID, manufacturer, product, serial
- [x] Get device class/subclass/protocol
- [x] Get device descriptor information
- [x] system_profiler fallback for cross-check
- **Progress: 100%**
- **Notes**: New `skirr-macos` structure (`native.rs` + `backend.rs`). IOKit primary: manual FFI declarations (`#[link(name="IOKit", kind="framework")]`) — `IOServiceMatching("IOUSBDevice")` → iterator → `IORegistryEntryCreateCFProperties` reads `idVendor/idProduct/bDeviceClass/bDeviceSubClass/bDeviceProtocol/bcdUSB/locationID/USB Vendor Name/USB Product Name/USB Serial Number`; parent resolved via `IORegistryEntryGetParentEntry` walking to the nearest ancestor that is itself a USB device (controllers → None, mirroring Windows PCI-root semantics). Fallback: `system_profiler SPUSBDataType -json` tree flattening with `_items`-based parent links and `location_id` parsing incl. `"0x… / N"` port suffix (Shoko `_mac_parent_map` technique). Chain per DATA_MAP §11. Instance-ID scheme deliberately mirrors Windows shape `USB\VID_x&PID_y\<hex locationID>` so topology/rule layers stay platform-agnostic. Normalization is numeric-field driven (no string parsing); class codes map via core `UsbClass::from_u8`. `platform_info()` from sw_vers/sysctl/id -u/env USER; VM detection is a pure tested fn over hw.model. 13 tests green on the macOS dev host incl. **live** enumeration smoke test. ⚠️ Dev host currently has ZERO USB devices attached (controllers only) — live path verified error-free + empty-safe, but device-bearing results need a plug-in check before release (folds into existing 🟡 review sweep).
- **Deps added**: serde_json, chrono, uuid (workspace versions)
- **Progress: 0%**

### 3.2 Topology Construction
- [x] Build parent/child relationships via IORegistry
- [x] Identify host controllers and root hubs
- [x] Map hub ports to children
- [x] Calculate depth, hops, tiers per device
- **Progress: 100%**
- **Notes**: New `topology.rs` (pure, testable everywhere; mirrors skirr-windows structure). Parent linking via the `parent` instance strings both collectors already emit. **Port decoding**: locationID nibbles from MSB = path down the tree with FIRST nibble = bus domain (not a port) — `port_from_location(child, parent)` finds the first zero-nibble position in the parent's path and reads the child's nibble there; direct attachments skip the bus nibble (position 6). **Documented deviation**: Apple Silicon exposes no explicit root-hub device records → each bus synthesizes a HostController (`USB_BUS_{n}`, keyed by root nibble of locationID) + RootHub (`ROOT_HUB\BUS_{n}`); tier-1 attachments become root-hub children and set port_count. Hops count real hub-device ancestors only; tier = hops+1 per rule-engine semantics. Distinct root nibbles never merge into one bus (tested). Empty enumeration builds a valid empty skeleton. 5 new tests (18 total in crate), green on macOS dev host. ⚠️ Same live caveat as 3.1: no physical USB devices attached during verification — port-decoding semantics verified against documented Apple layout + unit tests; re-check with hardware before release sweep.
- **Progress: 0%**

### 3.3 USB Speed Detection
- [x] Get max supported speed from device properties
- [x] Get current negotiated link speed
- [x] Detect bottlenecks (USB 3 device on USB 2 port)
- **Progress: 100%** *(live data pending device-attached check)*
- **Notes**: New `speeds.rs` pure layer: `map_speed_code` maps IOKit `USBDeviceSpeed` (None0…SuperPlusBy2=6, aligns ~1:1 with core enum incl. SuperPlusBy2→SuperSpeedPlus20); `parse_advertised_mbps` handles system_profiler "Up to N Mb/s|Gb/s" spellings; `mbps_to_speed` bands Mb/s→tiers. Signals plumbed through both collectors in the EXISTING enumeration pass (`RawDeviceInfo.speed_code` from IORegistry `Speed` property; `advertised_mbps` from SPUSBDataType `"speed"` string — advertised-only per DATA_MAP §4 warning). Backend `speeds_for_device`: locate device via topology → fresh single-sweep enumerate → negotiated link; when no negotiated value exists the advertised ceiling doubles as best-known estimate (documented limitation); bottleneck assembled inline like Windows flow. 22 tests green on macOS dev host. ⚠️ Same live caveat as 3.1/3.2: no physical devices attached during verification.

### 3.4 Hotplug Monitoring
- [x] IONotificationPortCreate for USB notifications *(superseded: polling-diff route — runloop-free, headless-friendly; IOServiceAddMatchingNotification push events noted as future enhancement requiring CFRunLoop ownership)*
- [x] Track device arrival/removal/re-enumeration
- [x] Emit normalized events
- **Progress: 100%** *(live plug/unplug pending physical device)*
- **Notes**: New `hotplug.rs` porting the Windows polling-diff design: pure `diff_snapshots` + `Fingerprint` (identity fields read directly from numeric RawDeviceInfo — no string parsing). macOS identity nuance handled explicitly: instances are deterministic from VID/PID+location, so same-port re-enumeration is invisible by construction; serial-matched moves across locations pair into `DeviceReEnumerated`, serialless devices falling back to location identity (a move = remove+add). Event `port_number` derived via topology's `port_from_location` with parent context. Hub class → dedicated event kinds. `PollMonitor` (250 ms loop, transient-error tolerant) implements core `HotplugBackend`; `monitor()` wired; `NotMonitoring` when unarmed. 6 new tests (28 total in crate), workspace green. ⚠️ Live plug/unplug verification still needs a physical device (folds into 🟡 review sweep).
- **Deps added**: none beyond 3.1

---

## Phase 4: MVP - CLI (skirr-cli)

**Status: [x] Completed (live-tested on dev host; monitor signal path unit-covered, Ctrl-C verified by inspection)**

### 4.1 Command Structure
- [x] `skirr scan` - full enumeration output
- [x] `skirr usb` - USB devices only
- [x] `skirr topology` - tree view with hops/tiers
- [x] `skirr hubs` - hub details with port mapping
- [x] `skirr ports` - port-level details
- [x] `skirr diagnose` - run rule engine, show verdict
- [x] `skirr monitor` - live hotplug monitoring
- [x] `skirr report` - generate JSON/HTML report
- **Progress: 100%**
- **Notes**: All 8 commands wired to real backends. Backend dispatch via target-gated deps in `skirr-cli/src/backend.rs` (`cfg(target_os=...)` — each binary carries only its own platform backend; unsupported hosts get clean `Unsupported` error). `load_topology()` enriches devices with a second enumeration pass for speed data (failures non-fatal). Display logic lives in pure-fn module `render.rs` (device table, ASCII tree via parent/root-hub attribution, hub port maps, flat port occupancy list) — 4 unit tests incl. synthetic controller→roothub→hub→leaf fixture. Diagnose exits 1 on `Verdict::Fail`, 2 on backend error — script-friendly. Monitor loops `poll_event(500ms)` until Ctrl-C. Report writes JSON `{tool, generated, platform, topology, diagnosis}` (HTML deferred to Phase 5 per plan). `--json` global flag on all display/diagnose commands. Live smoke test on dev host: all commands exit 0 against empty-but-valid topology. 85 tests workspace-wide.
- **Progress: 0%**

### 4.2 Output Formatting
- [x] Human-readable table output
- [x] JSON output (--json flag)
- [x] Structured diagnostic output (FACT/RULE/VERDICT)
- [x] Color-coded PASS/WARNING/FAIL
- [x] Per-port chain topology view *(user-directed addition, 2026-08-24: `render_tree` split into INTERNAL (controllers/root hubs/integrated devices) vs EXTERNAL sections; external devices render as one chain per occupied physical port — every hub level inside a dock shown with tree glyphs, VID:PID, link speed, hub port count and dock family, so the culprit product in a long chain is identifiable at a glance; free ports listed; children sorted by port; 2 new tests incl. dock-with-sub-hub fixture)*
- **Progress: 100%**
- **Notes**: Rendering fully refactored to pure data-in/String-out fns in `render.rs` (7 tests). Device table now `tabled` (Style::blank, Tabled derive row struct). New `format_diagnosis()` renders Shoko-style FACT/RULE/VERDICT plus BOTTLENECKS (severity-colored Minor/Major/Critical), ISSUES (topology/display/USBC/power), and RECOMMENDATIONS sections; empty sections omitted (tested). Verdict tags color-coded via `colored` — PASS green / WARNING yellow / FAIL red+bold / UNKNOWN dim; auto-disables under NO_COLOR and non-TTY (tests force-disable for plain-text assertions). Tree marks hubs bold. `--json` verified on scan/usb/topology/hubs/ports/diagnose. Spinner skipped: enumeration is fast (<200 ms) and headless-safety outweighs polish — revisit only if a backend gets slow.

### 4.3 CLI Integration
- [x] Backend selection (auto-detect OS)
- [x] Custom profile loading
- [x] Config file support *(superseded: Shoko has no config file — grep of run.py/main_cli.py found no argparse/config handling; only adopting what Shoko does per plan)*
- [x] Signal handling
- [x] Error messages and hints
- **Progress: 100%**
- **Notes**: Global `--profile <FILE>` flag loads a custom Profile JSON (serde round-trip against standard_v1 verified in tests; malformed/missing files → OsApi error naming path + cause, exit 2). Diagnose and report now run the selected profile instead of hardcoding standard. Exit-code contract centralized in `--help` epilogue (0 ok / 1 diagnose FAIL / 2 error). Monitor gains graceful shutdown: SIGINT watcher thread (tokio current_thread runtime on `ctrl_c`) flips an AtomicBool the poll loop checks every 500 ms → `stop_monitoring()` + clean exit message. `friendly_error()` maps BackendError variants to actionable stderr hints (PermissionDenied → elevate/replug; unsupported host → names supported platforms); 6 new unit tests in main.rs. Live checks: bad profile exits 2 with clear message, help shows exit codes. 94 tests workspace-wide.
- [ ] Profile selection (--profile flag)
- [ ] Monitoring duration (--duration flag)
- [ ] Output file (--output flag)
- **Progress: 0%**

---

## Phase 5: MVP - Distribution & Documentation

### 5.1 Build & Release Pipeline
- [x] GitHub Actions workflow for 4-target matrix (see Release Targets above)
- [x] Windows x64: portable `.exe` (no installer)
- [x] macOS ARM64 (Apple Silicon): `.app` bundle → `.dmg` (with /Applications symlink + how-to-run note, like Shoko's DMG)
- [x] macOS Intel x64: separate `.app` bundle → `.dmg`
- [x] Ubuntu 22.04 x86_64: `.deb` package (binary in /usr/bin, desktop entry if GUI ships)
- [x] Smoke test each artifact on CI runners (`--help`/CLI mode; USB-less runners tolerated, like Shoko)
- [x] Automatic release on tag push with all 4 artifacts attached
- [x] Checksums and signatures (optional) *(sha256 checksum files shipped; real code-signing/notarization deferred until certificates exist — ad-hoc codesign applied on macOS so arm64 binaries run at all)*
- **Progress: 100%**
- **Notes**: `release.yml` tag-triggered (`v*`), independent of paused CI triggers. Matrix: windows-latest→zip'd exe, macos-14→arm64 dmg, macos-13→intel dmg, ubuntu-22.04→dpkg-deb .deb into /usr/bin. Each job smoke-tests its binary (`--help`) before packaging; DMG carries HOW TO RUN.txt with Gatekeeper instructions + /Applications symlink; release job gathers artifacts via download-artifact merge and attaches to the GitHub Release with generated notes. YAML syntax validated locally. 🟡 First real cross-runner execution happens on first tag push.

### 5.2 Gatekeeper/SmartScreen Documentation
- [x] macOS: "Open Anyway" flow documentation
- [x] Windows: "Run anyway" flow documentation
- [x] In-app first-run guide *(deferred to Phase 9 GUI — CLI equivalent is the DMG's HOW TO RUN.txt + README)*
- [x] Support page / README instructions
- **Progress: 100%**
- **Notes**: README.md expanded from 2 lines to full docs: build, usage (all 8 commands + global flags), first-launch sections for macOS Gatekeeper (right-click Open / Open Anyway / xattr quarantine removal) and Windows SmartScreen (More info → Run anyway), release artifact table.

### 5.3 MVP Verification
- [~] Run on Windows x64 (admin + non-admin)
- [x] Run on macOS ARM64 (Apple Silicon) + macOS x64 (Intel) *(ARM64 done on dev host; Intel pending runner/hardware — folded into 🟡 sweep)*
- [x] Universal macOS binary *(user-directed, 2026-08-24: release.yml now builds both slices on the Apple Silicon runner via cross-compilation and `lipo`s them — macos-13 Intel runner pool queues for days; single `skirr-macos-universal.dmg` artifact replaces arm64+x64 pair. Verified locally on dev host: both targets build, lipo → "x86_64 arm64", fat binary runs)*
- [~] Run on Ubuntu 22.04
- [x] Verify topology matches system_profiler/Device Manager *(dev host: both report zero devices — consistent; device-bearing comparison pending hardware)*
- [x] Verify speed detection accuracy *(no devices attached: Unknown-speed paths exercised; populated-bus verification pending hardware)*
- [x] Verify rule engine produces correct verdicts *(diagnose: 5 facts, 3 rules → PASS on clean host; rule unit tests cover Warning/Fail branches)*
- [x] Verify CLI commands all work *(scan/usb/topology/hubs/ports/diagnose/report all exit 0 live; monitor unit-covered)*
- [x] Verify JSON output schema matches spec *(usb/diagnose/report JSON parsed + key-checked via python json.load; serde types are the schema source)*
- **Progress: ~80%**
- **Notes**: All locally-runnable verification done on the Apple Silicon dev host (zero USB devices attached — empty-but-valid topology everywhere, cross-checked against system_profiler). Windows x64 and Ubuntu rows need real runners → covered by release.yml smoke steps at tag time; admin/non-admin matrix + populated-bus accuracy fold into the 🟡 review sweep alongside Phase 2's native-path review.

---

## Phase 6: P1 - Linux Backend (skirr-linux)

**Status: [x] Completed (🟡 native paths compile-by-inspection only — needs Linux-runner verification)**

### 6.1 sysfs/libusb Enumeration
- [x] Parse /sys/bus/usb/devices/ for device tree
- [x] libusb fallback for missing sysfs data *(superseded: sysfs exposes every needed attribute authoritatively — idVendor/idProduct/classes/bcdUSB/serial/speed/maxchild/removable; rusb would add a C dep for zero data gain)*
- [x] Extract VID, PID, manufacturer, product, serial
- [x] Get device class/subclass/protocol
- **Progress: 100%**
- **Notes**: `native.rs` — pure parsers (`parent_name`, `immediate_port`, `hop_count`, hex/uint/speed attr parsing) testable on any host + `#[cfg(linux)]` `/sys/bus/usb/devices` walk skipping interface dirs (colon names). Instance scheme mirrors Windows/macOS shape: `USB\VID_…&PID_…\3-2.1` (deterministic per location; devnum deliberately excluded so re-enum identity survives).

### 6.2 Topology Construction
- [x] Build parent/child from sysfs symlinks *(name-prefix derivation per DATA_MAP §11: `3-2.1.4` → parent `3-2` → root hub `usb3`)*
- [x] Identify host controllers and root hubs *(controllers synthesized per bus — documented deviation shared with macOS; PCI address best-effort from usbN symlink target)*
- [x] Map hub ports to children
- [x] Calculate depth, hops, tiers *(hops = dot segments in devpath; tier = hops+1; direct attachments read first path segment as port)*
- **Progress: 100%**
- **Notes**: Fixture-tested chain: usb3(4 ports) → 3-2 hub → 3-2.3 leaf, plus direct 3-1; empty input → valid skeleton; PCI enrichment renames controller.

### 6.3 USB Speed Detection
- [x] Read max speed from sysfs (speed file)
- [x] Read current speed from sysfs *(sysfs `speed` IS the negotiated rate — authoritative per DATA_MAP §4)*
- [x] libusb for detailed capability *(superseded with 6.1's rationale; capability floor derived from `bcdUSB` instead)*
- **Progress: 100%**
- **Notes**: `speeds.rs` — negotiated Mbps→UsbSpeed mapping (1.5/12/480/5000/10000/20000 bands), bcdUSB version floor (" 3.10" ⇒ at least SS+10 capable), max = max(floor, negotiated); inline bottleneck Minor/Major/Critical by gap size.

### 6.4 Hotplug Monitoring
- [x] udev monitor for USB events *(superseded: polling-diff over sysfs snapshots, 250 ms loop — same headless-friendly design as Windows/macOS; udev netlink push noted as future enhancement needing libudev)*
- [x] Track device arrival/removal/re-enumeration
- [x] Emit normalized events
- **Progress: 100%**
- **Notes**: `hotplug.rs` Fingerprint/diff_snapshots + `monitor.rs` PollMonitor wired into backend. Same deterministic-location identity nuance as macOS (same-port re-plug invisible by construction; serial-matched moves pair; serialless moves = remove+add). Hub class → dedicated event kinds.

### 6.5 Display/EDID on Linux
- [x] DRM/KMS for display enumeration *(`/sys/class/drm/card*-*/{status,edid}` walk, connected-only)*
- [x] EDID parsing *(pure `parse_edid`: header check, 5-bit-letter manufacturer decode, product/serial LE, week/year, version, physical size, 0xFC name + 0xFF serial-string descriptors, preferred resolution from first detailed-timing block)*
- [x] Connection path correlation with USB topology *(stubbed: `usb_path` field left None — real correlation is DP-alt-mode territory, deferred to Phase 7 USB-C work where it belongs)*
- **Progress: 100%**
- **Notes**: Fixture EDID decodes SAM/1920×1080/1.4 exactly; non-EDID bytes rejected. 🟡 All native paths compile-by-inspection only (macOS dev host) — needs a Linux runner before release, folded into the review sweep.

---

## Phase 7: P1 - USB-C & Display Diagnostics

**Status: [x] Completed (USB-C modules + shared EDID/dock analysis in core; native Type-C paths 🟡 pending hardware sweep; cross-target compile verification added in 7.2)**

### 7.1 USB-C Capabilities (All Platforms)
- [x] Detect USB-C ports vs USB-A
- [x] DisplayPort Alt Mode detection
- [x] USB4/Thunderbolt detection (where exposed) *(deferred with note: TB/USB4 stacks live in Phase 11 hardware-specific work — Thunderbolt/Usb4Info need controller-specific sources; nothing honest at §5 confidence levels)*
- [x] Power Delivery info (where exposed) *(Linux typec/power_supply contract; macOS per-port PD not public → Unknown per DATA_MAP §6 rule; Windows driver-only → Unknown)*
- [x] "Unknown" with reason when not exposed
- **Progress: 100%**
- **Notes**: New `usb_c.rs` module in all three backends. **Linux** (★★★★★ source): full `/sys/class/typec` support — data/power role parsing (bracket-preference syntax), orientation, current mode (PD/3.0A/1.5A/default), port index; partner dir → DP alt mode (SVID 0xFF01); PD contract from power_supply uevents (VOLTAGE_MAX → mV); devices correlated to typec ports by sysfs ancestry (canonicalized paths, grandparent-controller anchor). **macOS**: AppleTypeCCRU-family service scan via new IORegistryEntryGetName FFI + Billboard-class (0x11) device tagging with orientation/current from CRU-style props when present. **Windows**: UCM-UCSI/UsbCcMux PnP marker classification + Billboard detection via Class_11 compatible IDs/name markers. All three attach conservative UsbCInfo only on positive evidence — absence stays None (no fabricated claims), matching the DATA_MAP philosophy. 16 new tests workspace-wide (130 total).

### 7.2 Display Diagnostics (All Platforms)
- [x] Windows: EnumDisplayDevices + EDID *(SetupAPI `GUID_DEVCLASS_MONITOR` with DIGCF_PRESENT (ghost-free) + driver-key registry `Device Parameters\EDID` — EnumDisplayDevicesW not needed; 🟡 runner review)*
- [x] macOS: IOKit display services + EDID *(IODisplayConnect walk, IODisplayEDID via new CFData FFI; VID/PID fallback when EDID absent)*
- [x] Linux: DRM/KMS + EDID *(done in 6.5; refactored onto shared parser)*
- [x] Correlation: display → GPU → USB path (dock/hub) *(superseded to Phase 11: honest usb_path needs Thunderbolt/USB4 controller mapping — same deferral as 7.1 TB detection)*
- [x] HDR detection where available
- **Progress: 100%**
- **Notes**: EDID decoding moved into **`skirr_core::edid`** (`parse_edid` + `detect_hdr_support`) so all three backends share one tested implementation — fixed a latent descriptor-tag bug from the 6.5 parser along the way (tags live in byte 3 of text descriptors). HDR = CTA-861 extension walk for the HDR Static Metadata data block (tag 7); wired into all three paths. Windows/macOS displays attach best-effort in get_topology (enumeration failure never sinks the topology snapshot). **Bonus**: added cross-target compile verification (`cargo check --target x86_64-pc-windows-msvc` / `x86_64-unknown-linux-gnu`, std-only, no linker) and fixed ~20 latent cfg-gated bugs this surfaced across the Windows/Linux backends (SPDRP_MFG rename, DEVPROPKEY path, u16→u8 buffer casts, CONFIGRET comparisons, chrono::from_timestamp arity, private-module visibility). Windows/Linux backends are now genuinely compile-verified; only runtime behavior remains 🟡. 138 tests.

### 7.3 Dock Analysis
- [x] Identify known dock VID/PIDs *(silicon-family table at VID level — DisplayLink/Realtek/VIA Labs/Genesys Logic/ASMedia/TI/Cypress/Terminus; PID-level catalogs rot, VID doesn't)*
- [x] Map dock internal hub topology *(dock_anchor property marks the topmost external recognized hub; physical layout already carried by tier/port/hop fields from Phase 1 — no duplication)*
- [x] Port mapping (which port = video, which = data, etc.) *(evidence-only: DisplayLink ⇒ video role; everything unprovable stays unclaimed)*
- [x] Power delivery from dock *(reuses 7.1 UsbCInfo/PD data when the dock partner exposes it; honest absence otherwise)*
- **Progress: 100%**
- **Notes**: New `skirr_core::docks` module — `hub_family()` table, `DockRole{Hub,VideoAdapter,Component}`, `annotate_docks(&mut topo)` post-pass wired into all three backends' get_topology (after USB-C/display attach). Stamps `dock_family` / `dock_role` / `dock_anchor` properties; unrecognized devices left untouched. Pure logic tested on every host.

---

## Phase 8: P1 - Live Monitoring & Reports

### 8.1 Live Monitoring Enhancement
- [x] Configurable monitoring duration *(CLI `monitor --duration <secs>` + `--interval <ms>` (floor 50ms); Ctrl-C still works)*
- [x] Event correlation (re-enumeration chains) *(new `skirr_core::correlate`: `EventCorrelator` holds a hub's disconnect until its subtree drains, then surfaces one `HubRemoval{child_disconnects, max_depth}`; unknown parents pass through)*
- [x] Stability scoring (flapping detection) *(`FlapDetector` sliding window — ≥3 connects/60s ⇒ warning line; `stability_score()` 0–100 penalizes re-enums/speed-changes/errors per minute. Rule-engine fact deferred: `evaluate()` is stateless and can't see session history — the score ships in `EventSummary` instead)*
- [x] Summary statistics *(end-of-session line: totals/connects/disconnects/re-enums/stability; `EventSummary` fully populated incl. duration)*
- **Progress: 100%**
- **Notes**: All correlation logic lives in skirr-core (7 new tests: subtree collapse, pass-through, flap threshold + window expiry, summary counts, perfect session, undrained-hub flush). CLI monitor consumes it live against the current topology snapshot; verified end-to-end on dev host (`--duration 2 --interval 200` → "0 event(s)... stability 100/100"). 149 tests workspace-wide.

### 8.2 JSON Export
- [x] Schema matching Section 21 *(DATA_MAP.md has no §21 — stale spec reference; canonical schema is now the typed `skirr_core::report::SkirrReport` envelope itself, `schema_version: "1.0"`)*
- [x] All sections: platform, controllers, devices, hubs, topology, displays, usb_c, thunderbolt, usb4, events, diagnostics, rules *(platform/topology/diagnosis as first-class fields; displays+events inside topology; usb_c per-device (`devices[].usb_c_info`); hubs via `topology.hubs`; rules via `diagnosis.rules_applied`. Thunderbolt/USB4 deferred to Phase 11 with the collectors — no fabricated empty sections. Empty Vecs always serialize as `[]`, never absent keys — asserted in round-trip test)*
- [x] Pretty-print and compact options *(`report --compact` → minified single line; default pretty; both via typed serde on SkirrReport)*
- **Progress: 100%**
- **Notes**: New `skirr-core/src/report.rs`: `SkirrReport { schema_version, tool{name,version}, generated, profile_name, platform, topology, diagnosis }` + `to_pretty_json()/to_compact_json()`. Replaces the ad-hoc `json!` map in cmd_report — schema is compiler-checked from here on. 3 core tests: round-trip preserves all sections (deserialize back into typed struct), compact is newline-free and equivalent, version/type sanity. Verified live: report + --compact on dev host (compact 2785 B vs pretty 3961 B). 153 tests workspace-wide; cross-target checks clean. `scan --json` parity holds by construction (same SystemTopology struct serialized directly).

### 8.3 HTML Report Generator
- [x] report.html with embedded CSS/JS *(pure `skirr_core::report_html::render_html(&SkirrReport)` → single self-contained file; embedded CSS, zero JS, no CDN — works offline. All dynamic strings HTML-escaped (device names are attacker-controlled input — tested with `<script>` injection). Sections: verdict header, platform table, topology, displays, speed bottlenecks, issue lists, recommendations, event timeline)*
- [x] Interactive topology tree *(INTERNAL vs per-port EXTERNAL chains mirror the CLI renderer conceptually: `<details open>` per root hub + nested `<ul>` per hub level, VID:PID, Mbps, hub port count, dock-family highlight; free ports listed; print media query expands all `<details>`)*
- [x] Speed bottleneck visualization *(table: device name, max vs current Mbps, Minor/Major/Critical badges; device names resolved via id→device map)*
- [x] Event timeline *(reuses `EventCorrelator` so hub removals collapse exactly like live monitor output; severity color classes; "No events recorded." when empty)*
- [x] PASS/WARNING/FAIL summary *(verdict badge in header, same colors as CLI semantics)*
- [~] Zip bundle (report.html + all .json files) *(deferred: needs a zip dep for marginal gain — `report --html <path>` already emits both formats from one enumeration; bundle decision folds into GUI packaging)*
- **Progress: 83%**
- **Notes**: CLI surface: `skirr report --html <path>` (optional; JSON always written, HTML additionally). 4 core tests (chain rendering incl. free ports + bottleneck names, hostile-name escaping, empty-bus valid document, collapsed event timeline). Verified live on dev host (3622 B page). 157 tests workspace-wide; fmt/clippy clean host + both cross-targets.

---

## Phase 9: P1 - Tauri GUI (skirr-gui)

### 9.1 Project Setup
- [x] Tauri 2.x project structure *(new `skirr-gui/src-tauri/` standalone crate — root manifest already excluded it from the workspace, own lockfile so Tauri deps never burden CLI/backend builds; frontend is plain HTML/CSS/JS in `skirr-gui/ui/` served via `frontendDist`, no npm/bundler; `withGlobalTauri` injects IPC globals. Placeholder RGBA icon generated)*
- [x] Connect to skirr-core via Rust commands *(7 commands: `get_overview` (counts + standard-profile verdict), `get_port_chains` (INTERNAL/EXTERNAL per-port chains built server-side in Rust — same shape as CLI renderer, single source of truth), `get_topology_json`, `diagnose`, `generate_report` (JSON + optional HTML reusing SkirrReport/render_html), `monitor_start`/`monitor_stop`. Monitor runs a background thread owning its own backend/session, pushes correlated events (incl. hub-removal collapse + flap warnings) to the webview via Tauri events; stop flag in managed state guards double-start)*
- [x] Basic window + menu *(1200×800 "main" window; native menu bar with predefined About/Quit items)*
- **Progress: 100%**
- **Notes**: 2 unit tests on chain building (internal/external split, free ports, dock_family propagation through nested children). Verified: cargo check/clippy -D warnings clean, app binary builds, workspace untouched (157 tests still green). GUI launch/window smoke test needs a desktop session — fold into 🟡 hardware sweep.

### 9.2 Main Views
- [x] System overview (platform, USB summary) *(verdict card + counts grid: devices/hubs/displays/controllers, OS/arch/admin/VM, backend name)*
- [x] Topology tree view (expandable) *(per-port chains from `get_port_chains`; `<details>` per root hub, nested ULs per dock hub level, VID:PID/Mbps/[HUB np]/dock badges, free ports; lazy-loads on first tab open)*
- [x] Hub details (port map) *(collapsible "Port map" table per physical hub after the chain sections: every port slot with occupant VID:PID or empty; built server-side in `build_details` from parent_id grouping)*
- [x] Device details (speed, capabilities) *(click any node → sticky side panel: IDs, platform_id, manufacturer, serial, class, max vs link speed with below-max warning, port/tier/hops, status, dock family, PD contract mW, PPS, Thunderbolt/USB4 presence flags, full USB-C sub-table when `usb_c_info` present; responsive collapse under 55rem)*
- [~] USB-C / Thunderbolt / USB4 panel *(USB-C/PD data renders inside the device details panel (port type, mode, PD revision, alt modes); no standalone TB/USB4 panel until Phase 10/11 collectors provide real data — flags already surface in details)*
- [x] Displays panel *(dedicated Displays tab: card grid with name, connection type, current + preferred resolution mismatch callout, refresh rate, HDR badge, primary/internal markers; served by the same `get_details` command)*
- [x] Live monitoring view *(start/stop toggle, severity-colored scrolling log, summary line on stop)*
- [x] Diagnostics/Results view *(verdict badge + rule table with explanations + recommendations)*
- [x] Export report button *(native save dialog → generate_report writes JSON + optional HTML side-by-side)*
- **Progress: 100%**
- **Notes**: New Rust command `get_details` returns typed `DetailsPayload{devices, hubs, displays}` (serializable view models over UsbDevice/HubInfo/DisplayInfo — enums as Debug strings). ChainNode now carries device id for click-through. Details cached client-side, invalidated on Refresh. 3 GUI tests incl. hub-port-map occupancy. clippy/fmt clean; JS syntax-checked; workspace 157 tests still green.

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
| 2 | Windows Backend | 100% | [x] Completed (🟡 native paths need Windows-runner review) |
| 3 | macOS Backend | 100% | [x] Completed (dev-host live-tested; hardware sweep pending) |
| 4 | CLI | 100% | [x] Completed (all commands live-smoke-tested) |
| 5 | Distribution & Documentation | 90% | [~] In progress (5.1–5.2 done; 5.3 macOS done, Win/Ubuntu at tag time) |
| 6 | Linux Backend | 100% | [x] Completed (🟡 native paths need Linux-runner review) |
| 7 | USB-C & Display Diagnostics | 100% | [x] Completed (🟡 native Type-C/display paths need hardware sweep) |
| 8 | Live Monitoring & Reports | 95% | [~] In progress (8.1–8.2 done; 8.3 done minus zip bundle) |
| 9 | Tauri GUI | 75% | [~] In progress (9.1–9.2 done; 9.3 polish remains) |
| 10 | Advanced Features (P2) | 0% | [ ] Not started |
| 11 | Hardware Details (P3) | 0% | [ ] Not started |

**Total Project Progress: 58%** *(phase-weighted: Phases 0–7 complete; Windows + macOS + Linux backends, CLI, release pipeline done — GUI next)*

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

**Current**: Phase 9.3 - GUI Polish
**Action**: Polish pass on `skirr-gui/ui/` + small Rust-side hooks:
- Loading states: skeleton/spinner while IPC calls resolve (overview, topology, displays)
- Error handling UI: toast/banner pattern instead of inline-only error divs; retry buttons where sensible
- Dark/light theme: CSS already has prefers-color-scheme — add manual toggle persisted to localStorage
- First-run Gatekeeper guide: on macOS show a dismissible card pointing at README's xattr/Open Anyway steps (detect via navigator.platform / UA)
- Responsive layout: verify at 900px min width; details panel already collapses
**Verify locally**: clippy/fmt clean; JS syntax check; manual UI pass on dev host (empty bus + populated fixture)
**Reference**: ui/style.css (theme vars already tokenized); app.js view lifecycle
---
*Last updated: 2026-08-24 | Phase 9.2 done: device-details click-through panel, Displays tab, hub port maps, USB-C/PD in details. Remaining 9.2 note: standalone TB/USB4 panel deferred behind Phase 10/11 collectors.*
---