//! USB Type-C power analysis (Phase 10.2): PD negotiation, CC orientation,
//! cable E-marker capability, and power role.
//!
//! Pure parsing lives here; collection is per-backend (Linux typec class,
//! macOS ioreg probe). Apple exposes no PD contract data through public
//! IORegistry keys on current builds (verified on dev hardware — the
//! AppleHPM services only carry I2C bus internals), so macOS reports an
//! empty port list rather than guessing.

use crate::{ConnectorOrientation, DataRole, PowerRole};
use serde::{Deserialize, Serialize};

/// Cable E-marker (SOP controller) capability as exposed by the OS.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CableEMarker {
    /// Raw plug mode string ("audio", "alt", …) where available.
    pub plug_mode: Option<String>,
    /// Current rating from cable identity ("3A"/"5A") where derivable.
    pub current_rating_a: Option<u32>,
    pub vendor_id: Option<u16>,
    pub product_id: Option<u16>,
}

/// One Type-C connector's live power picture.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TypeCPortStatus {
    /// OS port identifier ("port0").
    pub port_name: String,
    pub power_role: Option<PowerRole>,
    pub data_role: Option<DataRole>,
    /// PD spec revision of the negotiated link ("3.1" style).
    pub pd_revision: Option<String>,
    /// Connector capability ("source"/"sink"/"dual").
    pub port_type: Option<String>,
    pub orientation: ConnectorOrientation,
    pub vconn_source: Option<bool>,
    pub partner_attached: bool,
    /// Partner advertises USB-PD.
    pub partner_pd_supported: Option<bool>,
    pub emarker: Option<CableEMarker>,
}

impl TypeCPortStatus {
    /// True when a PD contract is visibly active (role + partner + PD).
    pub fn pd_active(&self) -> bool {
        self.partner_attached && self.partner_pd_supported == Some(true)
    }
}

/// Linux sysfs bracket convention: `[source] sink` marks the ACTIVE value
/// with brackets; the rest are alternatives.
fn active_value(raw: &str) -> Option<String> {
    let start = raw.find('[')?;
    let end = raw[start..].find(']')? + start;
    Some(raw[start + 1..end].trim().to_string())
}

fn parse_power_role(s: &str) -> Option<PowerRole> {
    #![allow(clippy::match_single_binding)]

    match s.trim() {
        "source" => Some(PowerRole::Source),
        "sink" => Some(PowerRole::Sink),
        _ => None,
    }
}

fn parse_data_role(s: &str) -> Option<DataRole> {
    match s.trim() {
        "host" | "dfp" => Some(DataRole::DFP),
        "device" | "ufp" => Some(DataRole::UFP),
        _ => None,
    }
}

fn parse_orientation(s: &str) -> ConnectorOrientation {
    match s.trim() {
        "normal" => ConnectorOrientation::Normal,
        "flipped" => ConnectorOrientation::Flipped,
        _ => ConnectorOrientation::Unknown,
    }
}

fn parse_bool01(s: &str) -> Option<bool> {
    match s.trim() {
        "1" => Some(true),
        "0" => Some(false),
        _ => None,
    }
}

/// Build one port's status from its sysfs attribute accessor. `dir` is the
/// directory name (`port0`); missing attributes simply stay `None`/Unknown —
/// kernels older than ~5.10 expose fewer files.
pub fn parse_linux_typec_port(
    dir: &str,
    attrs: &dyn Fn(&str) -> Option<String>,
) -> Option<TypeCPortStatus> {
    let stem = dir.strip_prefix("port").filter(|s| !s.is_empty())?;
    let _ = stem;
    let partner_attached = attrs("partner_attached")
        .and_then(|s| parse_bool01(&s))
        .or_else(|| attrs("partner").is_some().then_some(true))
        .unwrap_or(false);
    let pd_revision = attrs("usb_power_delivery_revision")
        .or_else(|| attrs("pd_revision"))
        .map(|s| s.trim().trim_end_matches(".0").to_string());

    let emarker = if partner_attached {
        build_emarker(&attrs)
    } else {
        None
    };

    // Partner dir exposes its own PD revision when PD is in use.
    let partner_pd_supported = if partner_attached {
        Some(attrs("partner_usb_power_delivery_revision").is_some() || pd_revision.is_some())
    } else {
        None
    };

    Some(TypeCPortStatus {
        port_name: dir.to_string(),
        power_role: attrs("power_role")
            .as_deref()
            .and_then(active_value)
            .as_deref()
            .and_then(parse_power_role),
        data_role: attrs("data_role")
            .as_deref()
            .and_then(active_value)
            .as_deref()
            .and_then(parse_data_role),
        pd_revision,
        port_type: attrs("port_type"),
        orientation: attrs("orientation")
            .map(|o| parse_orientation(&o))
            .unwrap_or(ConnectorOrientation::Unknown),
        vconn_source: attrs("vconn_source").as_deref().and_then(parse_bool01),
        partner_attached,
        partner_pd_supported,
        emarker,
    })
}

fn build_emarker(attrs: &dyn Fn(&str) -> Option<String>) -> Option<CableEMarker> {
    let plug_mode = attrs("plug_mode");
    let vendor_id = attrs("plug_identity_vid")
        .or_else(|| attrs("id_vendor_id"))
        .and_then(|v| u16::from_str_radix(v.trim().trim_start_matches("0x"), 16).ok());
    let product_id = attrs("plug_identity_pid")
        .or_else(|| attrs("id_product_id"))
        .and_then(|v| u16::from_str_radix(v.trim().trim_start_matches("0x"), 16).ok());
    let current_rating_a = attrs("cable_current_rating")
        .and_then(|c| c.trim().strip_suffix('A').unwrap_or(c.trim()).parse().ok());
    if plug_mode.is_none() && vendor_id.is_none() && current_rating_a.is_none() {
        return None;
    }
    Some(CableEMarker {
        plug_mode,
        current_rating_a,
        vendor_id,
        product_id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_full_linux_port_with_pd_and_cable() {
        let attrs = |k: &str| match k {
            "power_role" => Some("[source] sink".into()),
            "data_role" => Some("[host] device".into()),
            "usb_power_delivery_revision" => Some("3.1".into()),
            "port_type" => Some("dual".into()),
            "orientation" => Some("flipped".into()),
            "vconn_source" => Some("1".into()),
            "partner" => Some("port0-partner".into()),
            "partner_usb_power_delivery_revision" => Some("3.1".into()),
            "plug_mode" => Some("audio".into()),
            "plug_identity_vid" => Some("0x18d1".into()),
            "plug_identity_pid" => Some("0x4e11".into()),
            "cable_current_rating" => Some("5A".into()),
            _ => None,
        };
        let port = parse_linux_typec_port("port0", &attrs).expect("port parsed");
        assert_eq!(port.power_role, Some(PowerRole::Source));
        assert_eq!(port.data_role, Some(DataRole::DFP));
        assert_eq!(port.pd_revision.as_deref(), Some("3.1"));
        assert_eq!(port.port_type.as_deref(), Some("dual"));
        assert_eq!(port.orientation, ConnectorOrientation::Flipped);
        assert_eq!(port.vconn_source, Some(true));
        assert!(port.partner_attached);
        assert!(port.partner_pd_supported.unwrap());
        assert!(port.pd_active());
        let emarker = port.emarker.expect("emarker parsed");
        assert_eq!(emarker.current_rating_a, Some(5));
        assert_eq!(emarker.vendor_id, Some(0x18D1));
        assert_eq!(emarker.product_id, Some(0x4E11));
        assert_eq!(emarker.plug_mode.as_deref(), Some("audio"));
    }

    #[test]
    fn minimal_kernel_reports_degrade_gracefully() {
        let attrs = |k: &str| match k {
            "power_role" => Some("[sink] source".into()),
            "port_type" => Some("sink".into()),
            _ => None,
        };
        let port = parse_linux_typec_port("port1", &attrs).expect("port parsed");
        assert_eq!(port.power_role, Some(PowerRole::Sink));
        assert_eq!(port.data_role, None);
        assert!(!port.partner_attached);
        assert_eq!(port.partner_pd_supported, None);
        assert!(!port.pd_active(), "no partner → no active contract");
        assert!(port.emarker.is_none());
        assert_eq!(port.orientation, ConnectorOrientation::Unknown);
    }

    #[test]
    fn non_port_dirs_are_rejected() {
        let attrs = |_: &str| -> Option<String> { None };
        assert!(parse_linux_typec_port("", &attrs).is_none());
        assert!(parse_linux_typec_port("connector0", &attrs).is_none());
    }

    #[test]
    fn active_value_picks_bracketed_token_only() {
        assert_eq!(active_value("[device] host").as_deref(), Some("device"));
        assert_eq!(active_value("no brackets here"), None);
    }
}
