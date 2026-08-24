//! Windows device sources: native SetupAPI + PowerShell fallback.
//!
//! `RawDeviceInfo`, `parse_powershell_json`, and `split_multi_sz` are compiled
//! on every platform so parsing logic is unit-testable off-Windows; only the
//! collectors are `#[cfg(windows)]`. On non-Windows builds those shared items
//! have no production callers, hence the blanket dead-code allowance.
#![allow(dead_code)]

use serde::Deserialize;

/// OS-level facts gathered before normalization into `UsbDevice`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RawDeviceInfo {
    pub instance_id: String,
    pub hardware_ids: Vec<String>,
    pub manufacturer: Option<String>,
    pub description: Option<String>,
    pub friendly_name: Option<String>,
    pub class_name: Option<String>,
    pub status: Option<String>,
    /// PnP parent device instance ID (`DEVPKEY_Device_Parent`), when known.
    pub parent: Option<String>,
}

impl RawDeviceInfo {
    /// Best display name: friendly name, else description.
    pub fn display_name(&self) -> Option<&str> {
        self.friendly_name
            .as_deref()
            .or(self.description.as_deref())
    }

    /// First hardware ID line (primary).
    pub fn primary_hwid(&self) -> Option<&str> {
        self.hardware_ids.first().map(String::as_str)
    }
}

// ---------------------------------------------------------------------------
// PowerShell fallback output handling
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct PsDevice {
    instance_id: String,
    #[serde(default)]
    friendly_name: Option<String>,
    #[serde(default)]
    manufacturer: Option<String>,
    #[serde(default)]
    class: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    parent: Option<String>,
}

impl From<PsDevice> for RawDeviceInfo {
    fn from(p: PsDevice) -> Self {
        RawDeviceInfo {
            instance_id: p.instance_id,
            hardware_ids: Vec::new(),
            manufacturer: p.manufacturer,
            description: None,
            friendly_name: p.friendly_name,
            class_name: p.class,
            status: p.status,
            parent: p.parent,
        }
    }
}

/// Parse JSON emitted by
/// `Get-PnpDevice ... | Select-Object InstanceId,FriendlyName,Manufacturer,Class,Status | ConvertTo-Json`.
///
/// PowerShell emits one object for a single result, an array otherwise.
pub fn parse_powershell_json(text: &str) -> Result<Vec<RawDeviceInfo>, String> {
    let trimmed = text.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("null") {
        return Ok(Vec::new());
    }
    let value: serde_json::Value =
        serde_json::from_str(trimmed).map_err(|e| format!("invalid PowerShell JSON: {e}"))?;
    let items: Vec<serde_json::Value> = match value {
        serde_json::Value::Array(items) => items,
        obj @ serde_json::Value::Object(_) => vec![obj],
        _ => return Err("unexpected JSON shape".to_string()),
    };
    items
        .into_iter()
        .map(|item| {
            serde_json::from_value::<PsDevice>(item)
                .map(RawDeviceInfo::from)
                .map_err(|e| e.to_string())
        })
        .collect()
}

/// Inline PowerShell query kept next to its parser so they stay in sync.
/// Emits one JSON record per present USB device, including its PnP parent
/// (Shoko's `_win_parent_map` technique, single spawn).
pub const POWERSHELL_QUERY: &str = "Get-PnpDevice -PresentOnly | \
Where-Object { $_.InstanceId -like 'USB*' } | \
ForEach-Object { $p = $null; try { $p = ($_ | Get-PnpDeviceProperty -KeyName 'DEVPKEY_Device_Parent' -ErrorAction Stop).Data } catch {}; \
[PSCustomObject]@{ InstanceId = $_.InstanceId; Parent = $p; FriendlyName = $_.FriendlyName; Manufacturer = $_.Manufacturer; Class = $_.Class; Status = $_.Status } } | ConvertTo-Json -Compress";

/// Split a REG_MULTI_SZ buffer into strings (stops at empty terminator).
pub fn split_multi_sz(buf: &[u16]) -> Vec<String> {
    String::from_utf16_lossy(buf)
        .split('\0')
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

/// Decode a NUL-padded UTF-16 buffer.
pub fn decode_utf16(buf: &[u16]) -> Option<String> {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    if end == 0 {
        None
    } else {
        Some(String::from_utf16_lossy(&buf[..end]))
    }
}

/// Decode a NUL-terminated UTF-16 string from a little-endian byte buffer
/// (as returned by `SetupDiGetDevicePropertyW` for `DEVPROP_TYPE_STRING`).
pub fn decode_utf16_bytes(buf: &[u8]) -> Option<String> {
    let units: Vec<u16> = buf
        .chunks_exact(2)
        .take_while(|c| !(c[0] == 0 && c[1] == 0))
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    if units.is_empty() {
        None
    } else {
        Some(String::from_utf16_lossy(&units))
    }
}

// ---------------------------------------------------------------------------
// Native collectors (Windows only)
// ---------------------------------------------------------------------------

#[cfg(windows)]
pub(crate) mod win {
    use super::{decode_utf16, parse_powershell_json, split_multi_sz, RawDeviceInfo};
    use skirr_core::BackendError;
    use std::os::windows::process::CommandExt;
    use windows::Win32::Devices::DeviceAndDriverInstallation::*;
    use windows::Win32::Devices::Properties::{DEVPKEY_Device_Parent, DEVPROPTYPE};
    use windows::Win32::Foundation::*;

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    /// Enumerate present USB devices: native SetupAPI first, PowerShell when
    /// the native API errors or yields nothing (docs/DATA_MAP.md §11 chain).
    pub(crate) fn enumerate() -> Result<Vec<RawDeviceInfo>, BackendError> {
        match unsafe { setupapi_enumerate() } {
            Ok(devices) if !devices.is_empty() => Ok(devices),
            _ => powershell_enumerate(),
        }
    }

    /// True when the current process holds an admin token.
    pub(crate) fn is_elevated() -> bool {
        unsafe { windows::Win32::UI::Shell::IsUserAnAdmin().as_bool() }
    }

    unsafe fn setupapi_enumerate() -> windows::core::Result<Vec<RawDeviceInfo>> {
        let mut devices = Vec::new();

        unsafe {
            let hset = SetupDiGetClassDevsW(
                Some(&GUID_DEVCLASS_USB),
                None,
                None,
                DIGCF_PRESENT | DIGCF_DEVICEINTERFACE,
            )?;

            for index in 0..u32::MAX {
                let mut data = SP_DEVINFO_DATA::default();
                data.cbSize = std::mem::size_of::<SP_DEVINFO_DATA>() as u32;
                if SetupDiEnumDeviceInfo(hset, index, &mut data).is_err() {
                    break; // ERROR_NO_MORE_ITEMS
                }

                let mut id_buf = [0u16; 512];
                let instance_id =
                    match SetupDiGetDeviceInstanceIdW(hset, &data, Some(&mut id_buf), None) {
                        Ok(_) => decode_utf16(&id_buf).unwrap_or_default(),
                        Err(_) => String::new(),
                    };

                let str_prop = |prop| read_string_property(hset, &data, prop);
                let parent = read_device_property_string(hset, &data, &DEVPKEY_Device_Parent);
                let mut hw_buf = [0u16; 2048];
                let hardware_ids = SetupDiGetDeviceRegistryPropertyW(
                    hset,
                    &data,
                    SPDRP_HARDWAREID,
                    None,
                    Some(std::slice::from_raw_parts_mut(
                        hw_buf.as_mut_ptr().cast::<u8>(),
                        std::mem::size_of_val(&hw_buf),
                    )),
                    None,
                )
                .map(|_| split_multi_sz(&hw_buf))
                .unwrap_or_default();

                devices.push(RawDeviceInfo {
                    instance_id,
                    hardware_ids,
                    manufacturer: str_prop(SPDRP_MFG),
                    description: str_prop(SPDRP_DEVICEDESC),
                    friendly_name: str_prop(SPDRP_FRIENDLYNAME),
                    class_name: str_prop(SPDRP_CLASS),
                    status: Some("Present".to_string()),
                    parent,
                });
            }

            let _ = SetupDiDestroyDeviceInfoList(hset);
        }

        Ok(devices)
    }

    unsafe fn read_string_property(
        hset: HDEVINFO,
        data: &SP_DEVINFO_DATA,
        prop: SETUP_DI_REGISTRY_PROPERTY,
    ) -> Option<String> {
        let mut buf = [0u16; 1024];
        unsafe {
            SetupDiGetDeviceRegistryPropertyW(
                hset,
                data,
                prop,
                None,
                Some(std::slice::from_raw_parts_mut(
                    buf.as_mut_ptr().cast::<u8>(),
                    std::mem::size_of_val(&buf),
                )),
                None,
            )
            .ok()?;
        }
        decode_utf16(&buf)
    }

    /// Read a `DEVPROP_TYPE_STRING` device property (e.g. parent key).
    unsafe fn read_device_property_string(
        hset: HDEVINFO,
        data: &SP_DEVINFO_DATA,
        key: &windows::Win32::Foundation::DEVPROPKEY,
    ) -> Option<String> {
        let mut ptype = DEVPROPTYPE::default();
        let mut buf = [0u8; 1024];
        unsafe {
            SetupDiGetDevicePropertyW(hset, data, key, &mut ptype, Some(&mut buf), None, 0).ok()?;
        }
        super::decode_utf16_bytes(&buf)
    }

    fn powershell_enumerate() -> Result<Vec<RawDeviceInfo>, BackendError> {
        let output = std::process::Command::new("powershell")
            .args(["-NoProfile", "-Command", super::POWERSHELL_QUERY])
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .map_err(|e| BackendError::os_api("skirr-windows", format!("spawn powershell: {e}")))?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        parse_powershell_json(&stdout)
            .map_err(|e| BackendError::os_api("skirr-windows", format!("powershell parse: {e}")))
    }

    /// Present monitors with their EDID bytes (DATA_MAP §7): SetupAPI for
    /// presence, driver-key registry value `Device Parameters\EDID` for
    /// bytes. Errors only when SetupAPI itself fails; monitors without an
    /// EDID value are skipped (honest absence).
    pub(crate) fn display_records() -> Result<Vec<crate::displays::DisplayRecord>, BackendError> {
        use windows::Win32::System::Registry::{
            RegCloseKey, RegOpenKeyExW, RegQueryValueExW, HKEY, KEY_QUERY_VALUE, REG_BINARY,
        };

        fn to_utf16z(s: &str) -> Vec<u16> {
            s.encode_utf16().chain(std::iter::once(0)).collect()
        }

        unsafe {
            let hset =
                SetupDiGetClassDevsW(Some(&GUID_DEVCLASS_MONITOR), None, None, DIGCF_PRESENT)
                    .map_err(|e| {
                        BackendError::os_api(
                            "skirr-windows",
                            format!("SetupDiGetClassDevsW(MONITOR): {e}"),
                        )
                    })?;

            let mut out = Vec::new();
            for index in 0..u32::MAX {
                let mut data = SP_DEVINFO_DATA::default();
                data.cbSize = std::mem::size_of::<SP_DEVINFO_DATA>() as u32;
                if SetupDiEnumDeviceInfo(hset, index, &mut data).is_err() {
                    break;
                }
                let mut id_buf = [0u16; 512];
                let instance_id =
                    match SetupDiGetDeviceInstanceIdW(hset, &data, Some(&mut id_buf), None) {
                        Ok(_) => decode_utf16(&id_buf).unwrap_or_default(),
                        Err(_) => continue,
                    };

                // Software (driver) key of this devnode.
                let mut driver_key = HKEY::default();
                if CM_Open_DevNode_Key(
                    data.DevInst,
                    KEY_QUERY_VALUE.0,
                    0,
                    RegDisposition_OpenExisting,
                    &mut driver_key,
                    0,
                ) != CR_SUCCESS
                {
                    continue;
                }

                let edid_raw = || -> Option<Vec<u8>> {
                    let subkey = to_utf16z("Device Parameters");
                    let mut params_key = HKEY::default();
                    if RegOpenKeyExW(
                        driver_key,
                        windows::core::PCWSTR(subkey.as_ptr()),
                        None,
                        KEY_QUERY_VALUE,
                        &mut params_key,
                    )
                    .is_err()
                    {
                        return None;
                    }
                    let value_name = to_utf16z("EDID");
                    let mut value_type = REG_BINARY;
                    let mut size: u32 = 0;
                    let more = RegQueryValueExW(
                        params_key,
                        windows::core::PCWSTR(value_name.as_ptr()),
                        None,
                        Some(&mut value_type),
                        None,
                        Some(&mut size),
                    );
                    if more != ERROR_SUCCESS || size == 0 || size > 32 * 1024 {
                        let _ = RegCloseKey(params_key);
                        return None;
                    }
                    let mut buf = vec![0u8; size as usize];
                    let ok = RegQueryValueExW(
                        params_key,
                        windows::core::PCWSTR(value_name.as_ptr()),
                        None,
                        None,
                        Some(buf.as_mut_ptr()),
                        Some(&mut size),
                    );
                    let _ = RegCloseKey(params_key);
                    (ok == ERROR_SUCCESS).then_some(buf)
                }();
                let _ = RegCloseKey(driver_key);

                if let Some(edid_raw) = edid_raw {
                    out.push(crate::displays::DisplayRecord {
                        instance_id,
                        edid_raw,
                    });
                }
            }
            let _ = SetupDiDestroyDeviceInfoList(hset);
            Ok(out)
        }
    }
}
