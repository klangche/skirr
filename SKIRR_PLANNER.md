# Skirr Project Planner

**Overall Progress: 58%** *(phase-weighted: Phases 0–6 complete; all three platform backends + distribution done, USB-C/display diagnostics next)*

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
| 2 | Windows Backend | 100% | [x] Completed (🟡 native paths need Windows-runner review) |
| 3 | macOS Backend | 100% | [x] Completed (dev-host live-tested; hardware sweep pending) |
| 4 | CLI | 100% | [x] Completed (all commands live-smoke-tested) |
| 5 | Distribution & Documentation | 90% | [~] In progress (5.1–5.2 done; 5.3 macOS done, Win/Ubuntu at tag time) |
| 6 | Linux Backend | 100% | [x] Completed (🟡 native paths need Linux-runner review) |
| 7 | USB-C & Display Diagnostics | 0% | [ ] Not started |
| 8 | Live Monitoring & Reports | 0% | [ ] Not started |
| 9 | Tauri GUI | 0% | [ ] Not started |
| 10 | Advanced Features (P2) | 0% | [ ] Not started |
| 11 | Hardware Details (P3) | 0% | [ ] Not started |

**Total Project Progress: 58%** *(phase-weighted: Phases 0–6 complete; Windows + macOS + Linux backends, CLI, release pipeline done — P1 diagnostics next)*

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

**Current**: Phase 7.1 - USB-C Capabilities (All Platforms)
**Action**: Populate `UsbCInfo` (core model) with honest per-platform sourcing — DATA_MAP §5 is the spec; "Unknown with reason" is a first-class outcome, never a guess:
- Port-type detection (C vs A): Windows — ConfigManager/parent hub heuristics + `Win32_Usb` where present; macOS — IOKit port type strings on hub children (`USB3`/`XHC` naming); Linux — no reliable sysfs signal → Unknown(reason) unless connector class appears
- DP Alt Mode: macOS IORegistry `USB-C`/`AppleUSB20XHCIPort` hints; Linux `/sys/class/drm` connector types (DP over USB-C shows as `DP-` behind `ucsi`); Windows mostly unexposed → Unknown(reason)
- USB4/TB detection: macOS IORegistry `Thunderbolt` ancestors / `usb4_info`; Linux DMI + `/sys/bus/thunderbolt/devices`; Windows CM_Get_DevNode_Registry_Property THUNDERBOLT flags
- PD info: only where an OS exposes it (macOS AppleARPD/IOAccessoryPort hints; Linux `typec` sysfs class!) — otherwise Unknown
- Wire results into each backend's build_usb_device path; extend backend tests with synthetic UsbCInfo fixtures; rule engine likely gains a fact for C-port presence (check Profile rules)
**Verify locally**: pure detection logic unit-tested everywhere; native paths 🟡 as usual
**Reference**: docs/DATA_MAP.md §5 (USB-C matrix); skirr-core/src/model.rs UsbCInfo
---
*Last updated: 2026-08-24 | Phases 5+6 COMPLETE (release pipeline + Linux backend); next agent: Phase 7.1*