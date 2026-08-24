# Skirr Data Map

**Phase 0.2 — Section 27** · How every datapoint is retrieved, per platform.

Skirr's philosophy (inherited from ProAV Shoko): prefer **native APIs**, degrade to
**CLI tool parsing**, and finally report **"Unknown" with the reason** — never guess.
No admin/sudo is required for core functionality unless explicitly marked below.

---

## Conventions

### Confidence scores

| Score | Meaning |
|-------|---------|
| ★★★★★ | Direct OS API value; authoritative |
| ★★★★  | Reliable derived/parsed value; edge cases exist |
| ★★★   | Best-effort heuristic or vendor-dependent |
| ★★    | Often missing; platform hides it without privileges |
| ★     | Not exposed; would require kernel/driver access |

### Requirement levels

- **None** — works as unprivileged user
- **Partial** — some fields need elevation; rest still reported
- **Admin** — datapoint requires elevated process

---

## 1. Device Enumeration & VID/PID

### Windows (`skirr-windows`)

| Technique | Detail | Req | Conf |
|-----------|--------|-----|------|
| **SetupAPI** (primary) | `SetupDiGetClassDevs(DIGCF_PRESENT \| DIGCF_ALLCLASSES, ...)` filtered to `GUID_DEVCLASS_USB`; `SetupDiEnumDeviceInfo`; `SetupDiGetDeviceRegistryPropertyW` for `SPDRP_MANUFACTURER`, `SPDRP_PRODUCT`/`SPDRP_DEVICEDESC`, `SPDRP_MFG`, friendly name | None | ★★★★★ |
| Hardware ID parse | Instance path embeds IDs: `USB\VID_0B05&PID_1ACE&MI_00\<instance>`; `&MI_xx` marks composite interfaces | None | ★★★★★ |
| Device registry key | `SetupDiOpenDevRegKey` → `HKLM\SYSTEM\CurrentControlSet\Enum\USB\...` for serial number, `ParentIdPrefix` (serial-less devices) | None | ★★★★ |
| CfgMgr32 | `CM_Get_Device_IDW`, `CM_Get_DevNode_Status` for presence/problem codes | None | ★★★★★ |
| Fallback: PowerShell | `Get-PnpDevice -PresentOnly` + `Get-PnpDeviceProperty -KeyName DEVPKEY_Device_...` (Shoko's method; slow but robust) | None | ★★★★ |

Class/subclass/protocol: from hardware/compatible IDs (`Class`, `SubClass`, `Protocol`
registry values under the enum key). Descriptor-level detail needs the USB stack (§4).

### macOS (`skirr-macos`)

| Technique | Detail | Req | Conf |
|-----------|--------|-----|------|
| **IOKit** (primary) | Iterate `IOServiceMatching(kIOUSBDeviceClassName)`; properties: `idVendor`, `idProduct`, `bcdDevice`, `bDeviceClass`, `bDeviceSubClass`, `bDeviceProtocol`, `USB Product Name`, `USB Vendor Name`, `USB Serial Number`, `kUSBAddress`/`USB Address` | None | ★★★★★ |
| IORegistry CLI | `ioreg -p IOUSB -l -w0` — same keys, useful cross-check | None | ★★★★★ |
| system_profiler | `system_profiler SPUSBDataType -json`: `_items` tree with `vendor_id` (`0x0b05`), `product_id`, `serial_num`, `manufacturer`, `product`, `location_id` (Shoko's source) | None | ★★★★★ |

Note: Apple Silicon exposes internal Thunderbolt→USB bridges as ordinary hubs (see §8).

### Linux (`skirr-linux`)

| Technique | Detail | Req | Conf |
|-----------|--------|-----|------|
| **sysfs** (primary) | `/sys/bus/usb/devices/*` entries: `idVendor`, `idProduct`, `manufacturer`, `product`, `serial`, `bcdUSB`, `bDeviceClass`, `bDeviceSubClass`, `bDeviceProtocol`, `busnum`, `devpath`, `devnum`, `removable` | None | ★★★★★ |
| udev database | `libudev` (`udev_enumerate_scan_devices`, subsystem `"usb"`): same attrs plus `ID_VENDOR_FROM_DATABASE`, `ID_MODEL_FROM_DATABASE` (usb.ids lookup done by udev) | None | ★★★★★ |
| rusb/libusb | Full descriptor walk (config/interface/endpoint) without sysfs parsing; needed for descriptor detail (§4) | None (udev backend) | ★★★★★ |
| lsusb | `lsusb`, `lsusb -t` fallback when sysfs is unavailable (containerized) | usbutils pkg | ★★★★ |

---

## 2. Topology: Parent/Child, Hubs, Depth

Hops = edges from host port to device. Tiers = depth levels. External hubs counted
per-path. **Apple Silicon penalty:** the internal Thunderbolt/USB bridge hub consumes
1 tier before any external hub (limits: see §12).

### Windows

| Technique | Detail | Req | Conf |
|-----------|--------|-----|------|
| PnP device tree (primary) | `CM_Get_Parent` / `CM_Get_Child` / `CM_Get_Sibling` (CfgMgr32) walk of devnode tree; root hubs are children of host controllers (`GUID_DEVCLASS_USB`) | None | ★★★★★ |
| `DEVPKEY_Device_Parent` | Per-instance parent key via `SetupDiGetDevicePropertyW` (equivalent, simpler bulk query — Shoko used this via PowerShell) | None | ★★★★★ |
| Hub ports | Open hub device handle; `IOCTL_USB_GET_NODE_INFORMATION` → `HubInformation.HubDescriptor.bNumberOfPorts` | Partial (see §3/§4) | ★★★★ |

### macOS

| Technique | Detail | Req | Conf |
|-----------|--------|-----|------|
| IORegistry plane (primary) | Walk children of each `IOUSBHostDevice` entry (`IORegistryEntryGetChildEntry(parent, gIOUSBPlane)`); root hubs live under `AppleUSBXHCIPCI`/`AppleT2USB`/etc. controllers | None | ★★★★★ |
| system_profiler tree | `SPUSBDataType._items` nesting gives parent/child directly; `location_id` hex encodes port path (`<hub_port><parent_hub_port>...`) — Shoko matched parents by location ID | None | ★★★★★ |
| Companion-hub merge | Apple Type-C adapters enumerate as sibling hubs sharing serial suffix (`MSFT20xxxx`/`MSFT30xxxx` instance pattern); group them under one virtual node (Shoko `group_companion_hubs`) | None | ★★★★ |

### Linux

| Technique | Detail | Req | Conf |
|-----------|--------|-----|------|
| sysfs symlinks (primary) | Device dir name encodes path: `<busnum>-<devpath[.config]>`; parent = strip last `.port` segment (e.g. `3-2.1.4` → parent `3-2.1`); root hub = `<busnum>-0:...` wait — root hub device is `<busnum>-0` (no port suffix) | None | ★★★★★ |
| `devpath` + `maxchild` | Read `devpath`, count hops by dot segments; `maxchild` on hubs gives port count | None | ★★★★★ |
| lsusb -t | Indentation-based tree fallback (Shoko's parser); fragile to locale/format changes | usbutils | ★★★ |

Internal-device heuristics (all OSes, ported from Shoko `_is_internal`):
keyword match (`integrated`, `bluetooth`, `camera`, …), `removable` attr (Linux),
`LocationIds` under internal controller roots (macOS), devnode ancestry (Windows).
Confidence ★★★ — heuristic by nature.

---

## 3. Port Enumeration

| OS | Source | Detail | Req | Conf |
|----|--------|--------|-----|------|
| Windows | Hub IOCTLs | `IOCTL_USB_GET_NODE_CONNECTION_NAMES` enumerates connection names; per-port info via `IOCTL_USB_GET_NODE_CONNECTION_INFORMATION_EX` (`ConnectionIndex`) | Partial: works unprivileged for topology; some fields zeroed without admin | ★★★★ |
| macOS | IORegistry | Parent hub entry children carry `PortNum` property; port ordering from `location_id` nibbles | None | ★★★★ |
| Linux | sysfs | Child device's `devport` attribute + `devpath` position; hub's `maxchild`; empty ports not enumerated (only occupied ports appear) | None | ★★★★ |

Empty-port detection: Windows only (via hub IOCTL port status
`IOCTL_USB_GET_STATUS`/`GET_PORT_STATUS`). Mark "occupied ports only" elsewhere.

---

## 4. Speed: Max Supported vs Current Negotiated

| OS | Current link speed | Max supported | Req | Conf |
|----|--------------------|---------------|-----|------|
| Windows | `IOCTL_USB_GET_NODE_CONNECTION_INFORMATION_EX` → `Speed` (UsbLowSpeed…UsbHighSpeed); superseding `IOCTL_USB_GET_NODE_CONNECTION_INFORMATION_EX_V2` → `RxLaneCount/TxLaneCount` + `Usb30Speed` era flags for SuperSpeed+/x2 rates | Device descriptor `bcdUSB` + class heuristics; hub capability bits | Partial: EX_V2 may return zeros without admin | ★★★★ current / ★★★ max |
| macOS | IORegistry `Speed` property (0=low,1=full,2=high,3=super…) or system_profiler `speed` string ("Up to 480 Mb/s" is *advertised*, not negotiated — do not confuse) | Same source advertises device capability | None | ★★★★ / ★★★ |
| Linux | sysfs `speed` (Mbps: 1.5/12/480/5000/10000/20000/40000) — authoritative negotiated rate | `bcdUSB` + `rusb` descriptor; caps rarely exposed | None | ★★★★★ / ★★★★ |

Bottleneck rule (model.rs already implements): if `max_supported > current_link`,
flag with severity ratio (≥10× critical, ≥2× major).

---

## 5. USB-C Capabilities

| OS | What's available | Req | Conf |
|----|------------------|-----|------|
| Windows | UCM/UCSI via `root\UCM` devices (Win10+); connector identity through `Microsoft UsbCcMux`/`Type-C` PnP nodes; alt-mode info sparse outside OEM drivers | Partial | ★★ |
| macOS | IOKit `AppleTypeCCRU`/`AppleUSB20XHCITypeCPort`-style entries expose orientation/current on some Macs; not documented, varies by model | None | ★★ |
| Linux | `/sys/class/typec/port*`: `data_role`, `power_role`, `preferred_role`, partner dirs, `orientation`, `vconn_source`; DP alt mode at `portX-partner/.../displayport/` | None | ★★★★★ |

Detection of "this physical jack is Type-C": infer from Billboard-class devices,
Type-C PD controllers visible in PnP/IORegistry, or typec class (Linux).
Where undetectable: report **Unknown — not exposed by platform** (philosophy rule).

---

## 6. USB Power Delivery

| OS | What's available | Req | Conf |
|----|------------------|-----|------|
| Windows | UCSI/UCM reports contract via `IOCTL_ UCx`-family — effectively driver-only; battery/charging WMI (`BatteryStaticData`) hints wattage on laptops | Admin/driver | ★ |
| macOS | `AppleSmartBattery` IORegistry (`AdapterDetails`, `Amperage`, `Voltage`) covers *host input* power, not per-port contracts; per-port PD not public | None (input) | ★★ |
| Linux | `/sys/class/power_supply/*/usb_type`, `current_max`, `voltage_max` per typec port partner; PDO list via `uevent` (`POWER_SUPPLY_USB_TYPE=`) | None | ★★★★ |

Rule: PD details (PDO/APDO lists, negotiation history) are Phase 11 territory;
Phase 7 only reports presence/absence + best-effort contract.

---

## 7. Displays & EDID

| OS | Enumeration | EDID bytes | Req | Conf |
|----|-------------|------------|-----|------|
| Windows | `EnumDisplayDevicesW` (+ `DISPLAY_DEVICE_ACTIVE` filter); monitor friendly names via `EnumDisplayMonitors` | Registry: `HKLM\SYSTEM\CurrentControlSet\Enum\DISPLAY\<MON>\<inst>\Device Parameters\EDID` (128/256 B blob) | None | ★★★★★ |
| macOS | `CoreGraphics` (`CGGetOnlineDisplayList`, bounds/mode/refresh) + IOKit `IODisplayConnect` services | `IODisplayCreateInfoDictionary` → `IODisplayEDID` key (binary CFData) | None | ★★★★ (some virtualized GPUs omit) |
| Linux | DRM connectors: `/sys/class/drm/card*-*/status`, `enabled`, `modes` | `edid` file per connector (raw blob); parse manually (no crate dependency needed beyond fs reads) | None | ★★★★★ |

Correlation display↔USB path (dock video): match GPU connector → PCIe/TB ancestry →
USB-C mux → hub. Feasible reliably on Linux (DRM/TB sysfs) and partially on macOS;
on Windows best-effort via `DEVPKEY` ancestry. Confidence ★★★ overall.

Internal-display detection: Shoko heuristics — EDID manufacturer IDs of laptop panels
(`AUO`, `LGD`, `BOE`, `CMN`, `IVO`, `SDC`, `CHI`, `KTF`, `INN`…) or diagonal < 18".
Reuse as default heuristic, confidence ★★★.

---

## 8. Thunderbolt / USB4

| OS | Source | Detail | Req | Conf |
|----|--------|--------|-----|------|
| Windows | PnP classes `ThunderboltController`/`ThunderboltDevice`; WMI `root\wmi\Thunderbolt*` (OEM-dependent); USB4 router via `PCI\VEN_8086&DEV_9A1x`-class CM devices | Generation/security/NVM sparse | Partial | ★★ |
| macOS | IORegistry: `IONVMeController`-adjacent TB devices under `IOPCIDevice` with `tb` bindings; `system_profiler SPThunderboltDataType` gives domain UUID, device name, firmware, security level | None | ★★★★ |
| Linux | `/sys/bus/thunderbolt/devices/*`: `device_name`, `vendor_name`, `generation` (kernel ≥5.x on some), `nvm_version`, `security` (persistence via `domainX/security`), `unique_id` | None | ★★★★★ |

Apple Silicon: the TB controller also presents the internal hub that costs 1 tier —
cross-reference §2 when computing tiers.

---

## 9. Hotplug Monitoring

| OS | Mechanism | Notes | Req | Conf |
|----|-----------|-------|-----|------|
| Windows | `RegisterDeviceNotification` (`DBT_DEVTYP_DEVICEINTERFACE`, GUID_DEVCLASS_USB) → `WM_DEVICECHANGE` (`DBT_DEVICEARRIVAL/REMOVALCOMPLETE`); re-enumeration inferred by arrival of same `ParentIdPrefix`+serial | Needs a window/message loop; CLI runs hidden window thread | None | ★★★★ |
| macOS | `IONotificationPortCreate` + `IOServiceAddMatchingNotification` on `kIOUSBDeviceClassName` armed/terminated callbacks | Deliver on run loop thread | None | ★★★★★ |
| Linux | udev monitor (`libudev`: `udev_monitor_new_from_netlink(kernel)`, filter subsystem `usb`) | udev events include action add/remove/bind/unbind | None | ★★★★★ |

Shoko note (carried over): packet-level monitoring (usbmon/ETW/IOKit tracing) is
explicitly **out of scope**; Skirr monitors enumeration events only.

Re-enumeration chains: same VID/PID/serial arriving within N seconds after removal
(default 10 s) → classify as `DeviceReEnumerated`, not separate connect/disconnect.

---

## 10. Admin / Driver Requirement Summary

| Datapoint | Win | macOS | Linux |
|-----------|-----|-------|-------|
| Enumeration, VID/PID, strings | None | None | None |
| Topology/hops/tiers | None | None | None |
| Ports (occupied) | None | None | None |
| Empty ports + port status | Partial→Admin | N/A (not exposed) | N/A (not exposed) |
| Current speed | None (EX may zero w/o admin) | None | None |
| USB-C/PD detail | Admin/driver | Sparse | None (typec class) |
| Displays/EDID | None | None | None |
| Thunderbolt | Partial | None | None |
| Hotplug events | None | None | None |

Design consequence: core scan must never hard-fail without elevation — fill what's
available, mark the rest Unknown-with-reason (planner philosophy bullet).

---

## 11. Fallback Chain (per datapoint)

```
Native API ──fail──▶ CLI tool parse ──fail──▶ cached/last-known ──none──▶ Unknown(reason)
   (★★★★★)            (★★★★)                                      (report honestly)
```

Windows CLI fallback: PowerShell `Get-PnpDevice` family (Shoko-proven).
macOS CLI fallback: `ioreg`/`system_profiler`.
Linux CLI fallback: `lsusb`/`lsusb -t`.

---

## 12. Stability Limits (Skirr Standard Profile v1.0 defaults)

From Shoko `src/assets/usb_data.csv` (authoritative rows for our targets):

| System | max_hops | max_tiers | max_hubs | Note |
|--------|----------|-----------|----------|------|
| Windows x86/ARM64 | 7 | 7 | 5 | Standard xHCI limits |
| macOS Intel | 7 | 7 | 5 | No internal tier penalty |
| macOS Apple Silicon | 6 | 6 | 4 | Internal TB hub consumes 1 tier; M2/M3 bus timeouts at 5 hubs |
| Linux x86_64 | 7 | 7 | 5 | Mirrors Windows |
| Linux ARM (RPi/Rockchip) | 6 | 6 | 4 | SoC firmware caps |
| iPhone/iPad/Android (reference) | 4–5 | 4–5 | 2–3 | Client-side reference rows |

Sources cited in the CSV itself (xHCI spec, Biamp EasyConnect MPX app note,
macrumors community threads, RPi forum). Ship these as TOML profile defaults
(Phase 1.3) with user-overridable values.

---

## 13. Rust Implementation Mapping

| Datapoint | Crate | Primary dep |
|-----------|-------|-------------|
| Windows enumeration/topology | skirr-windows | `windows` crate (SetupAPI, CfgMgr32, DeviceAndDriverInstallation features) |
| Windows speeds/ports | skirr-windows | hub `DeviceIoControl` IOCTLs |
| Windows hotplug/displays | skirr-windows | `WM_DEVICECHANGE`, `EnumDisplayDevicesW` |
| macOS all USB | skirr-macos | `objc2` + `core-foundation` (IOKit C API via objc2 message sends) |
| macOS displays | skirr-macos | CoreGraphics + IODisplayConnect |
| Linux all USB | skirr-linux | sysfs reads + `libudev` + `rusb` |
| Linux displays/TB | skirr-linux | DRM sysfs + `/sys/bus/thunderbolt` |
| Cross-platform model/rules | skirr-core | serde, chrono, uuid (done: model.rs) |

---
*Phase 0.2 deliverable. Update confidence ratings as backends land (Phases 2, 3, 6)
and real-world results replace theory.*
