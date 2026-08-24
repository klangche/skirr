//! USB-C / Type-C detection for Linux — the one platform where the kernel
//! exposes it properly (DATA_MAP §5, ★★★★★).
//!
//! Sources:
//! - `/sys/class/typec/port*`: data_role, power_role, orientation,
//!   vconn_source; partner at `portN-partner`, DP alt mode under
//!   `portN-partner/.../displayport`
//! - `/sys/class/power_supply/*/uevent`: PD contract hints
//!
//! Pure parsing is testable anywhere; the sysfs walks are cfg(linux).

use skirr_core::{
    AltModeInfo, ConnectorOrientation, DataRole, UsbCCurrentMode, UsbCInfo, UsbCPortType,
};

/// Attribute bundle for one typec port as read from sysfs.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TypeCPortAttrs {
    pub name: String, // "port0"
    pub data_role: Option<String>,
    pub power_role: Option<String>,
    pub preferred_role: Option<String>,
    pub orientation: Option<String>,
    pub vconn_source: Option<String>,
}

/// Parse `source|sink` style role strings ("[source] sink" = preferred
/// marked in brackets on some kernels). USB-C naming maps source→DFP,
/// sink→UFP, dual→DRP.
pub fn parse_data_role(body: &str) -> Option<DataRole> {
    let trimmed = body.trim();
    if trimmed.contains("dual") {
        return Some(DataRole::DRP);
    }
    // Preferred/active role sits in brackets when present.
    let active = match (trimmed.find('['), trimmed.find(']')) {
        (Some(start), Some(end)) if end > start => &trimmed[start + 1..end],
        _ => trimmed,
    };
    match active.trim() {
        "source" => Some(DataRole::DFP),
        "sink" => Some(DataRole::UFP),
        _ => None,
    }
}

pub fn parse_orientation(body: &str) -> ConnectorOrientation {
    match body.trim() {
        "normal" => ConnectorOrientation::Normal,
        "flipped" | "reverse" => ConnectorOrientation::Flipped,
        _ => ConnectorOrientation::Unknown,
    }
}

/// Map power role + current capability markers to the core current mode.
pub fn parse_current_mode(power_role: &str) -> UsbCCurrentMode {
    let role = power_role.trim();
    if role.contains("pd") || role.contains("PD") {
        return UsbCCurrentMode::UsbPd;
    }
    match role {
        r if r.contains("3.0") => UsbCCurrentMode::TypeCCurrent3_0A,
        r if r.contains("1.5") => UsbCCurrentMode::TypeCCurrent1_5A,
        _ => UsbCCurrentMode::DefaultUsb,
    }
}

/// Build core UsbCInfo from parsed attributes. Port type defaults to DRP
/// when both roles advertise (the common typec-class case), Unknown when
/// nothing is readable.
pub fn build_port_info(attrs: &TypeCPortAttrs) -> UsbCInfo {
    let port_type = match (attrs.data_role.as_deref(), attrs.power_role.as_deref()) {
        (Some(d), _) if d.contains("dual") => UsbCPortType::DRP,
        (_, Some(p)) if p.contains("dual") => UsbCPortType::DRP,
        (Some(d), _) if d.starts_with("source") => UsbCPortType::DFP,
        (Some(d), _) if d.starts_with("sink") => UsbCPortType::UFP,
        _ => UsbCPortType::Unknown,
    };
    UsbCInfo {
        port_type,
        current_mode: attrs
            .power_role
            .as_deref()
            .map(parse_current_mode)
            .unwrap_or(UsbCCurrentMode::Unknown),
        pd_supported: false,
        pd_revision: None,
        alt_modes: Vec::new(),
        cable_info: None,
        connector_orientation: Some(
            attrs
                .orientation
                .as_deref()
                .map(parse_orientation)
                .unwrap_or(ConnectorOrientation::Unknown),
        ),
        port_index: attrs.name.strip_prefix("port").and_then(|n| n.parse().ok()),
        partner_info: None,
    }
}

/// Discover alt modes from a partner directory listing: any entry named
/// `displayport` implies DP alt mode (SVID 0xFF01 mode 1).
pub fn alt_modes_from_partner(partner_entries: &[String]) -> Vec<AltModeInfo> {
    let mut out = Vec::new();
    if partner_entries.iter().any(|e| e.contains("displayport")) {
        out.push(AltModeInfo {
            svid: 0xFF01,
            mode: 1,
            description: "DisplayPort".into(),
            active: true,
            vdo: None,
        });
    }
    if partner_entries.iter().any(|e| e.contains("audio")) {
        out.push(AltModeInfo {
            svid: 0xFF01,
            mode: 2,
            description: "Audio Accessory".into(),
            active: true,
            vdo: None,
        });
    }
    out
}

/// PD contract from a power_supply uevent body:
/// `POWER_SUPPLY_USB_TYPE=[C] PD` + max voltage/current in µV/µA.
pub fn parse_pd_from_uevent(uevent: &str) -> (bool, Option<u16>) {
    let usb_type = uevent
        .lines()
        .find(|l| l.starts_with("POWER_SUPPLY_USB_TYPE="))
        .map(|l| &l["POWER_SUPPLY_USB_TYPE=".len()..])
        .unwrap_or("");
    let pd = usb_type.contains("PD");
    let voltage_max = uevent
        .lines()
        .find(|l| l.starts_with("POWER_SUPPLY_VOLTAGE_MAX="))
        .and_then(|l| l.split('=').nth(1))
        .and_then(|v| v.parse::<u32>().ok())
        .map(|uv| (uv / 1000) as u16);
    (pd, voltage_max)
}

/// Does this USB device live on the same controller subtree as a typec
/// port? Both nest under the PCI/controller device, so the typec port's
/// grandparent directory (`…/<controller>/typec/portN` → `…/<controller>`)
/// is the correlation anchor.
pub fn usb_path_under_typec_port(usb_syspath: &str, port_syspath: &str) -> bool {
    let controller = port_syspath
        .rsplit_once('/')
        .and_then(|(rest, _)| rest.rsplit_once('/'))
        .map(|(parent, _)| parent);
    controller
        .map(|prefix| usb_syspath.starts_with(prefix))
        .unwrap_or(false)
}

/// Read all typec ports + their partners. Empty (not an error) when the
/// class doesn't exist — desktops without any Type-C are normal.
#[cfg(target_os = "linux")]
pub fn enumerate_typec_ports() -> skirr_core::BackendResult<Vec<(TypeCPortAttrs, String)>> {
    use skirr_core::BackendError;
    use std::fs;

    const CLASS_DIR: &str = "/sys/class/typec";
    let mut out = Vec::new();
    let entries = match fs::read_dir(CLASS_DIR) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
        Err(e) => {
            return Err(BackendError::os_api(
                crate::BACKEND_NAME,
                format!("read_dir {CLASS_DIR}: {e}"),
            ))
        }
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !name.starts_with("port") {
            continue;
        }
        let read = |f: &str| {
            fs::read_to_string(path.join(f))
                .ok()
                .map(|s| s.trim().to_string())
        };
        let attrs = TypeCPortAttrs {
            name: name.to_string(),
            data_role: read("data_role"),
            power_role: read("power_role"),
            preferred_role: read("preferred_role"),
            orientation: read("orientation"),
            vconn_source: read("vconn_source"),
        };
        // Canonical path so device-path prefix matching works.
        let canonical = fs::canonicalize(&path)
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
        out.push((attrs, canonical));
    }
    out.sort_by(|a, b| a.0.name.cmp(&b.0.name));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roles_parse_with_bracket_preferences() {
        assert_eq!(parse_data_role("[source] sink"), Some(DataRole::DFP));
        assert_eq!(parse_data_role("source [sink]"), Some(DataRole::UFP));
        assert_eq!(parse_data_role("[dual] dual-role"), Some(DataRole::DRP));
        assert_eq!(parse_data_role(""), None);
    }

    #[test]
    fn orientation_maps_both_spellings() {
        assert_eq!(parse_orientation("normal"), ConnectorOrientation::Normal);
        assert_eq!(parse_orientation("flipped"), ConnectorOrientation::Flipped);
        assert_eq!(parse_orientation("reverse"), ConnectorOrientation::Flipped);
        assert_eq!(parse_orientation("weird"), ConnectorOrientation::Unknown);
    }

    #[test]
    fn current_mode_reads_pd_markers() {
        assert_eq!(parse_current_mode("[pd] source"), UsbCCurrentMode::UsbPd);
        assert_eq!(
            parse_current_mode("3.0A"),
            UsbCCurrentMode::TypeCCurrent3_0A
        );
        assert_eq!(parse_current_mode("default"), UsbCCurrentMode::DefaultUsb);
    }

    #[test]
    fn port_info_defaults_to_honest_unknowns() {
        let attrs = TypeCPortAttrs {
            name: "port2".into(),
            ..Default::default()
        };
        let info = build_port_info(&attrs);
        assert_eq!(info.port_type, UsbCPortType::Unknown);
        assert_eq!(info.current_mode, UsbCCurrentMode::Unknown);
        assert_eq!(
            info.connector_orientation,
            Some(ConnectorOrientation::Unknown)
        );
        assert_eq!(info.port_index, Some(2));
        assert!(!info.pd_supported);
    }

    #[test]
    fn dp_alt_mode_detected_from_partner_entries() {
        let entries = ["displayport".to_string(), "org.kde.something".into()];
        let modes = alt_modes_from_partner(&entries);
        assert_eq!(modes.len(), 1);
        assert_eq!(modes[0].svid, 0xFF01);
        assert_eq!(modes[0].description, "DisplayPort");

        assert!(alt_modes_from_partner(&[]).is_empty());
    }

    #[test]
    fn pd_contract_from_uevent_includes_wattage_inputs() {
        let uevent = "POWER_SUPPLY_TYPE=USB\n\
                      POWER_SUPPLY_USB_TYPE=[C] PD\n\
                      POWER_SUPPLY_VOLTAGE_MAX=20000000\n\
                      POWER_SUPPLY_CURRENT_MAX=3000000\n";
        let (pd, vmax_mv) = parse_pd_from_uevent(uevent);
        assert!(pd);
        assert_eq!(vmax_mv, Some(20000));
        let (no_pd, none) = parse_pd_from_uevent("POWER_SUPPLY_USB_TYPE=C\n");
        assert!(!no_pd);
        assert_eq!(none, None);
    }

    #[test]
    fn path_correlation_is_prefix_based() {
        let port = "/sys/devices/pci0000:00/0000:00:08.1/typec/port0";
        assert!(usb_path_under_typec_port(
            "/sys/devices/pci0000:00/0000:00:08.1/usb3/3-1",
            port
        ));
        assert!(!usb_path_under_typec_port(
            "/sys/devices/pci0000:00/0000:00:02.0/usb1/1-2",
            port
        ));
    }
}
