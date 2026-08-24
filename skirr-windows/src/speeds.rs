//! Per-port USB speed detection via hub IOCTLs (Windows).
//!
//! Pure parsing/mapping logic compiles everywhere so it stays unit-testable
//! on any host; the actual `DeviceIoControl` collection lives behind
//! `#[cfg(windows)]`. Technique: walk `GUID_DEVINTERFACE_USB_HUB` interfaces,
//! open each hub, then `IOCTL_USB_GET_NODE_INFORMATION` (port count),
//! `IOCTL_USB_GET_NODE_CONNECTION_INFORMATION_EX` (negotiated speed + VID/PID)
//! and `_EX_V2` (protocol capability flags for SuperSpeed+).
//!
//! Off-Windows the wire structs and IOCTL constants are exercised by tests
//! only, hence the blanket dead-code allowance.
#![cfg_attr(not(windows), allow(dead_code))]

use skirr_core::{BackendError, UsbSpeed};

// ---------------------------------------------------------------------------
// IOCTL plumbing (values cross-checked against usbioctl.h / public listings)
// ---------------------------------------------------------------------------

const FILE_DEVICE_USB: u32 = 0x0000_0022;

const fn ctl_code(device_type: u32, function: u32, method: u32, access: u32) -> u32 {
    (device_type << 16) | (access << 14) | (function << 2) | method
}

pub(crate) const IOCTL_USB_GET_NODE_INFORMATION: u32 = ctl_code(FILE_DEVICE_USB, 258, 0, 0);
pub(crate) const IOCTL_USB_GET_NODE_CONNECTION_INFORMATION_EX: u32 =
    ctl_code(FILE_DEVICE_USB, 272, 0, 0);
pub(crate) const IOCTL_USB_GET_NODE_CONNECTION_INFORMATION_EX_V2: u32 =
    ctl_code(FILE_DEVICE_USB, 274, 0, 0);

// ---------------------------------------------------------------------------
// Wire structs (mirrors of usbioctl.h, flattened to avoid bitfield UB)
// ---------------------------------------------------------------------------

/// `USB_NODE_CONNECTION_INFORMATION_EX` fixed head (before `PipeList[]`).
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(crate) struct ConnInfoEx {
    connection_index: u32,
    // USB_PIPE_INFO for the control endpoint (endpoint descriptor + schedule)
    ep_b_length: u8,
    ep_b_descriptor_type: u8,
    ep_endpoint_address: u8,
    ep_bm_attributes: u8,
    ep_max_packet_size: u16,
    ep_interval: u8,
    schedule_offset: u32,
    // USB_DEVICE_DESCRIPTOR
    desc_b_length: u8,
    desc_type: u8,
    bcd_usb: u16,
    device_class: u8,
    device_sub_class: u8,
    device_protocol: u8,
    max_packet_size_0: u8,
    vendor_id: u16,
    product_id: u16,
    bcd_device: u16,
    i_manufacturer: u8,
    i_product: u8,
    i_serial_number: u8,
    num_configurations: u8,
    current_configuration_value: u8,
    /// `USB_DEVICE_SPEED`: 0 low, 1 full, 2 high, 3 super
    speed: u8,
    device_is_hub: u8,
    device_address: u16,
    number_of_open_pipes: u32,
}

const CONN_INFO_EX_SIZE: usize = std::mem::size_of::<ConnInfoEx>();

/// Input for `_EX_V2`; caller sets `length` to the full struct size (16).
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(crate) struct ConnInfoExV2In {
    connection_index: u32,
    length: u32,
}

/// Output of `_EX_V2`: protocols + device-capability bitmasks.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(crate) struct ConnInfoExV2Out {
    connection_index: u32,
    length: u32,
    /// `USB_PROTOCOLS`: bit0 Usb110, bit1 Usb200, bit2 Usb300
    protocols: u32,
    /// `USB_DEVICE_CAPABILITIES`: bit0 HighSpeed, bit1 SuperSpeed, bit2 SuperSpeedPlus
    capabilities: u32,
}

const USB_PROTOCOL_USB110: u32 = 1 << 0;
const USB_PROTOCOL_USB200: u32 = 1 << 1;
const USB_PROTOCOL_USB300: u32 = 1 << 2;

const CAP_SUPER_SPEED_PLUS: u32 = 1 << 2;

// ---------------------------------------------------------------------------
// Pure mapping helpers (unit-tested off-Windows)
// ---------------------------------------------------------------------------

/// Map the `USB_DEVICE_SPEED` code reported by `_EX` onto the core enum.
pub(crate) fn map_ex_speed(code: u8) -> UsbSpeed {
    match code {
        0 => UsbSpeed::LowSpeed,
        1 => UsbSpeed::FullSpeed,
        2 => UsbSpeed::HighSpeed,
        3 | 4 => UsbSpeed::SuperSpeed,
        _ => UsbSpeed::Unknown,
    }
}

/// Derive the *maximum supported* speed from descriptor `bcdUSB` plus the
/// `_EX_V2` protocol/capability words (whichever were collected). Conservative
/// floor: SuperSpeedPlus capability reports 10G even when 20G lanes exist.
pub(crate) fn derive_max_speed(
    bcd_usb: u16,
    protocols: Option<u32>,
    capabilities: Option<u32>,
) -> UsbSpeed {
    let mut max = match bcd_usb {
        v if v >= 0x0300 => UsbSpeed::SuperSpeed,
        v if v >= 0x0200 => UsbSpeed::HighSpeed,
        v if v >= 0x0110 => UsbSpeed::FullSpeed,
        0 => UsbSpeed::Unknown,
        _ => UsbSpeed::LowSpeed,
    };

    if let Some(p) = protocols {
        if p & USB_PROTOCOL_USB300 != 0 {
            max = max.max(UsbSpeed::SuperSpeedPlus10);
        } else if p & USB_PROTOCOL_USB200 != 0 {
            max = max.max(UsbSpeed::SuperSpeed);
        } else if p & USB_PROTOCOL_USB110 != 0 {
            max = max.max(UsbSpeed::HighSpeed);
        }
    }
    if capabilities.is_some_and(|c| c & CAP_SUPER_SPEED_PLUS != 0) {
        max = max.max(UsbSpeed::SuperSpeedPlus10);
    }
    max
}

/// Reconstruct the PnP instance ID from a hub interface device path:
/// `\\?\USB#VID_05AC&PID_1000#0100#{f18a0e88-…}` → `USB\VID_05AC&PID_1000\0100`.
pub(crate) fn instance_from_hub_path(device_path: &str) -> Option<String> {
    let body = device_path.strip_prefix(r"\\?\")?;
    let body = match body.split_once("#{") {
        Some((prefix, _)) => prefix,
        None => body,
    };
    Some(body.replace('#', "\\"))
}

/// Parse the port count out of an `IOCTL_USB_GET_NODE_INFORMATION` reply
/// (`USB_NODE_INFORMATION`: `NodeType` u32, then `USB_HUB_DESCRIPTOR`:
/// bLength, bDescriptorType, `bNumberOfPorts` at byte offset 6).
pub(crate) fn parse_node_info_port_count(buf: &[u8]) -> Option<u8> {
    if buf.len() < 7 {
        return None;
    }
    Some(buf[6])
}

/// Interpret one `_EX` reply buffer.
pub(crate) fn parse_conn_ex(buf: &[u8]) -> Option<(ConnInfoEx, ())> {
    if buf.len() < CONN_INFO_EX_SIZE {
        return None;
    }
    // Safety: repr(C) POD, unaligned read from a plain byte buffer.
    let info = unsafe { std::ptr::read_unaligned(buf.as_ptr() as *const ConnInfoEx) };
    Some((info, ()))
}

/// Interpret one `_EX_V2` reply buffer.
pub(crate) fn parse_conn_v2(buf: &[u8]) -> Option<ConnInfoExV2Out> {
    if buf.len() < std::mem::size_of::<ConnInfoExV2Out>() {
        return None;
    }
    Some(unsafe { std::ptr::read_unaligned(buf.as_ptr() as *const ConnInfoExV2Out) })
}

// ---------------------------------------------------------------------------
// Collection model shared with the backend
// ---------------------------------------------------------------------------

/// One occupied downstream port observed on a hub.
#[derive(Debug, Clone)]
pub(crate) struct PortSnapshot {
    /// PnP instance ID of the *hub* this port belongs to.
    pub hub_instance: String,
    pub port: u8,
    pub vendor_id: u16,
    pub product_id: u16,
    pub bcd_usb: u16,
    /// Raw `USB_DEVICE_SPEED` code from `_EX`.
    pub speed_code: u8,
    #[allow(dead_code)] // kept for parity with Linux PortSnapshot; consumed in Phase 8
    pub is_hub: bool,
    pub protocols: Option<u32>,
    pub capabilities: Option<u32>,
}

impl PortSnapshot {
    /// Negotiated link speed, upgraded to SuperSpeed+ when `_EX_V2` says so.
    pub fn current_link(&self) -> UsbSpeed {
        let base = map_ex_speed(self.speed_code);
        if base == UsbSpeed::SuperSpeed
            && self
                .capabilities
                .is_some_and(|c| c & CAP_SUPER_SPEED_PLUS != 0)
        {
            UsbSpeed::SuperSpeedPlus10
        } else {
            base
        }
    }

    pub fn max_supported(&self) -> UsbSpeed {
        derive_max_speed(self.bcd_usb, self.protocols, self.capabilities).max(self.current_link())
    }
}

/// Result of one full hub walk.
#[derive(Debug, Default)]
pub(crate) struct HubWalk {
    pub ports: Vec<PortSnapshot>,
    /// Hubs present but unreadable (access denied unelevated).
    pub denied_hubs: u32,
}

// ---------------------------------------------------------------------------
// Native collector (Windows only)
// ---------------------------------------------------------------------------

#[cfg(windows)]
mod win {
    use super::*;
    use crate::native::decode_utf16;
    use windows::Win32::Devices::DeviceAndDriverInstallation::*;
    use windows::Win32::Devices::Usb::GUID_DEVINTERFACE_USB_HUB;
    use windows::Win32::Foundation::{ERROR_ACCESS_DENIED, HANDLE, INVALID_HANDLE_VALUE};
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    use windows::Win32::System::IO::DeviceIoControl;

    const GENERIC_READ_WRITE: u32 = 0x8000_0000 | 0x4000_0000;

    fn is_access_denied(err: &windows::core::Error) -> bool {
        (err.code().0 as u32) & 0xFFFF == ERROR_ACCESS_DENIED.0
    }

    fn hub_device_paths() -> Result<Vec<String>, BackendError> {
        let mut paths = Vec::new();
        unsafe {
            let hset = SetupDiGetClassDevsW(
                Some(&GUID_DEVINTERFACE_USB_HUB),
                None,
                None,
                DIGCF_PRESENT | DIGCF_DEVICEINTERFACE,
            )
            .map_err(|e| {
                BackendError::os_api(
                    crate::BACKEND_NAME,
                    format!("SetupDiGetClassDevsW(HUB): {e}"),
                )
            })?;

            for index in 0..u32::MAX {
                let mut dia = SP_DEVICE_INTERFACE_DATA::default();
                dia.cbSize = std::mem::size_of::<SP_DEVICE_INTERFACE_DATA>() as u32;
                if SetupDiEnumDeviceInterfaces(
                    hset,
                    None,
                    &GUID_DEVINTERFACE_USB_HUB,
                    index,
                    &mut dia,
                )
                .is_err()
                {
                    break;
                }

                let mut required = 0u32;
                let _ = SetupDiGetDeviceInterfaceDetailW(
                    hset,
                    &dia,
                    None,
                    0,
                    Some(&mut required),
                    None,
                );
                if required == 0 {
                    continue;
                }
                let mut buf = vec![0u8; required as usize];
                let detail = buf.as_mut_ptr() as *mut SP_DEVICE_INTERFACE_DETAIL_DATA_W;
                (*detail).cbSize = std::mem::size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() as u32;
                if SetupDiGetDeviceInterfaceDetailW(hset, &dia, Some(detail), required, None, None)
                    .is_err()
                {
                    continue;
                }
                let wide = std::slice::from_raw_parts(
                    (*detail).DevicePath.as_ptr(),
                    (required as usize - std::mem::size_of::<u32>()) / 2,
                );
                if let Some(p) = decode_utf16(wide) {
                    paths.push(p);
                }
            }

            let _ = SetupDiDestroyDeviceInfoList(hset);
        }
        Ok(paths)
    }

    struct HubHandle(HANDLE);

    impl HubHandle {
        fn open(path: &str) -> Result<Self, BackendError> {
            let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
            let handle = unsafe {
                CreateFileW(
                    windows::core::PCWSTR(wide.as_ptr()),
                    GENERIC_READ_WRITE,
                    FILE_SHARE_READ | FILE_SHARE_WRITE,
                    None,
                    OPEN_EXISTING,
                    FILE_ATTRIBUTE_NORMAL,
                    None,
                )
            }
            .map_err(|e| {
                if is_access_denied(&e) {
                    BackendError::permission_denied("skirr-windows", format!("open hub {path}"))
                } else {
                    BackendError::os_api("skirr-windows", format!("open hub {path}: {e}"))
                }
            })?;
            if handle == INVALID_HANDLE_VALUE {
                return Err(BackendError::os_api(
                    "skirr-windows",
                    format!("open hub {path}: invalid handle"),
                ));
            }
            Ok(Self(handle))
        }

        fn ioctl(&self, code: u32, in_buf: &[u8], out_len: usize) -> Result<Vec<u8>, BackendError> {
            let mut out = vec![0u8; out_len];
            let mut returned = 0u32;
            unsafe {
                DeviceIoControl(
                    self.0,
                    code,
                    Some(in_buf.as_ptr().cast()),
                    in_buf.len() as u32,
                    Some(out.as_mut_ptr().cast()),
                    out.len() as u32,
                    Some(&mut returned),
                    None,
                )
            }
            .map_err(|e| BackendError::os_api("skirr-windows", format!("ioctl {code:#x}: {e}")))?;
            out.truncate(returned as usize);
            Ok(out)
        }
    }

    /// Walk every present hub and snapshot its occupied downstream ports.
    pub(super) fn collect_hub_walk() -> Result<HubWalk, BackendError> {
        let mut walk = HubWalk::default();

        for hub_path in hub_device_paths()? {
            let Some(hub_instance) = instance_from_hub_path(&hub_path) else {
                continue;
            };

            let handle = match HubHandle::open(&hub_path) {
                Ok(h) => h,
                Err(BackendError::PermissionDenied { .. }) => {
                    walk.denied_hubs += 1;
                    continue;
                }
                Err(e) => return Err(e),
            };

            let node = handle.ioctl(IOCTL_USB_GET_NODE_INFORMATION, &[], 256)?;
            let Some(port_count) = parse_node_info_port_count(&node) else {
                continue;
            };

            for port in 1..=port_count {
                let idx = ConnInfoEx {
                    connection_index: u32::from(port),
                    ..Default::default()
                };
                let in_bytes = unsafe {
                    std::slice::from_raw_parts((&idx as *const ConnInfoEx).cast::<u8>(), 4)
                };

                let ex_buf =
                    handle.ioctl(IOCTL_USB_GET_NODE_CONNECTION_INFORMATION_EX, in_bytes, 1024)?;
                let Some((info, ())) = parse_conn_ex(&ex_buf) else {
                    continue;
                };
                // Unoccupied ports report empty descriptors; skip them.
                if info.vendor_id == 0 && info.product_id == 0 {
                    continue;
                }

                let v2_in = ConnInfoExV2In {
                    connection_index: u32::from(port),
                    length: std::mem::size_of::<ConnInfoExV2Out>() as u32,
                };
                let v2_bytes = unsafe {
                    std::slice::from_raw_parts(
                        (&v2_in as *const ConnInfoExV2In).cast::<u8>(),
                        std::mem::size_of::<ConnInfoExV2In>(),
                    )
                };
                let v2 = handle
                    .ioctl(
                        IOCTL_USB_GET_NODE_CONNECTION_INFORMATION_EX_V2,
                        v2_bytes,
                        64,
                    )
                    .ok()
                    .as_deref()
                    .and_then(parse_conn_v2);

                walk.ports.push(PortSnapshot {
                    hub_instance: hub_instance.clone(),
                    port,
                    vendor_id: info.vendor_id,
                    product_id: info.product_id,
                    bcd_usb: info.bcd_usb,
                    speed_code: info.speed,
                    is_hub: info.device_is_hub != 0,
                    protocols: v2.map(|v| v.protocols),
                    capabilities: v2.map(|v| v.capabilities),
                });
            }
        }

        Ok(walk)
    }
}

/// Public entry used by the backend; falls back to an error off-Windows.
#[cfg(windows)]
pub(crate) fn collect_hub_walk() -> Result<HubWalk, BackendError> {
    win::collect_hub_walk()
}

#[cfg(not(windows))]
pub(crate) fn collect_hub_walk() -> Result<HubWalk, BackendError> {
    Err(BackendError::unsupported(
        "skirr-windows",
        "speed detection requires Windows",
    ))
}

// ---------------------------------------------------------------------------
// Tests (pure logic, run on every host)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ioctl_codes_match_published_values() {
        assert_eq!(IOCTL_USB_GET_NODE_INFORMATION, 0x0022_0408);
        assert_eq!(IOCTL_USB_GET_NODE_CONNECTION_INFORMATION_EX, 0x0022_0440);
        assert_eq!(IOCTL_USB_GET_NODE_CONNECTION_INFORMATION_EX_V2, 0x0022_0448);
    }

    #[test]
    fn conn_info_ex_layout_is_forty_four_bytes() {
        // Offsets hand-derived from usbioctl.h; guards silent ABI drift.
        assert_eq!(CONN_INFO_EX_SIZE, 44);
    }

    #[test]
    fn ex_speed_codes_map_correctly() {
        assert_eq!(map_ex_speed(0), UsbSpeed::LowSpeed);
        assert_eq!(map_ex_speed(1), UsbSpeed::FullSpeed);
        assert_eq!(map_ex_speed(2), UsbSpeed::HighSpeed);
        assert_eq!(map_ex_speed(3), UsbSpeed::SuperSpeed);
        assert_eq!(map_ex_speed(9), UsbSpeed::Unknown);
    }

    #[test]
    fn derive_max_prefers_strongest_signal() {
        assert_eq!(derive_max_speed(0x0200, None, None), UsbSpeed::HighSpeed);
        assert_eq!(
            derive_max_speed(0x0200, Some(USB_PROTOCOL_USB200), None),
            UsbSpeed::SuperSpeed
        );
        assert_eq!(
            derive_max_speed(0x0200, Some(USB_PROTOCOL_USB300), None),
            UsbSpeed::SuperSpeedPlus10
        );
        assert_eq!(
            derive_max_speed(0x0300, None, Some(CAP_SUPER_SPEED_PLUS)),
            UsbSpeed::SuperSpeedPlus10
        );
        assert_eq!(derive_max_speed(0x0000, None, None), UsbSpeed::Unknown);
    }

    #[test]
    fn snapshot_current_upgrades_ss_plus_only_on_super() {
        let mut snap = PortSnapshot {
            hub_instance: String::new(),
            port: 1,
            vendor_id: 0x1111,
            product_id: 0x2222,
            bcd_usb: 0x0310,
            speed_code: 3,
            is_hub: false,
            protocols: Some(USB_PROTOCOL_USB300),
            capabilities: Some(CAP_SUPER_SPEED_PLUS),
        };
        assert_eq!(snap.current_link(), UsbSpeed::SuperSpeedPlus10);
        assert_eq!(snap.max_supported(), UsbSpeed::SuperSpeedPlus10);

        // High-speed link never upgrades to SS+ regardless of caps.
        snap.capabilities = Some(CAP_SUPER_SPEED_PLUS);
        snap.speed_code = 2;
        assert_eq!(snap.current_link(), UsbSpeed::HighSpeed);
        // But advertised capability still raises the ceiling.
        assert_eq!(snap.max_supported(), UsbSpeed::SuperSpeedPlus10);
    }

    #[test]
    fn hub_path_round_trips_to_instance() {
        let path = r"\\?\USB#VID_05AC&PID_1000#0100#{f18a0e88-c30c-11d0-8815-00a0c906bed8}";
        assert_eq!(
            instance_from_hub_path(path).as_deref(),
            Some(r"USB\VID_05AC&PID_1000\0100")
        );
        assert_eq!(instance_from_hub_path(r"C:\not\a\hub"), None);
    }

    #[test]
    fn node_info_port_count_parses() {
        let good = [0x02, 0, 0, 0, 0x09, 0x29, 0x04];
        assert_eq!(parse_node_info_port_count(&good), Some(4));
        assert_eq!(parse_node_info_port_count(&[0u8; 3]), None);
    }

    #[test]
    fn conn_ex_buffer_parses_end_to_end() {
        let mut buf = vec![0u8; 1024];
        let info = ConnInfoEx {
            connection_index: 3,
            bcd_usb: 0x0210,
            vendor_id: 0x1234,
            product_id: 0xABCD,
            speed: 2,
            device_is_hub: 1,
            device_address: 42,
            ..Default::default()
        };
        unsafe {
            std::ptr::copy_nonoverlapping(
                (&info as *const ConnInfoEx).cast::<u8>(),
                buf.as_mut_ptr(),
                CONN_INFO_EX_SIZE,
            );
        }
        let (got, ()) = parse_conn_ex(&buf).unwrap();
        assert_eq!(got.vendor_id, 0x1234);
        assert_eq!(got.product_id, 0xABCD);
        assert_eq!(got.speed, 2);
        assert!(got.device_is_hub != 0);
        assert_eq!(got.connection_index, 3);
        assert!(parse_conn_ex(&[0u8; 10]).is_none());
    }

    #[test]
    fn conn_v2_buffer_parses_flags() {
        let out = ConnInfoExV2Out {
            connection_index: 1,
            length: 16,
            protocols: USB_PROTOCOL_USB300,
            capabilities: CAP_SUPER_SPEED_PLUS,
        };
        let buf: Vec<u8> = unsafe {
            std::slice::from_raw_parts(
                (&out as *const ConnInfoExV2Out).cast::<u8>(),
                std::mem::size_of::<ConnInfoExV2Out>(),
            )
        }
        .to_vec();
        let parsed = parse_conn_v2(&buf).unwrap();
        assert_eq!(parsed.protocols, USB_PROTOCOL_USB300);
        assert_eq!(parsed.capabilities, CAP_SUPER_SPEED_PLUS);
        assert!(parse_conn_v2(&[0u8; 8]).is_none());
    }
}
