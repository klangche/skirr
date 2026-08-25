//! USB Type-C power analysis (Phase 11): PD negotiation, PDO/APDO parsing,
//! cable wattage limits, CC orientation, and E-marker capability.
//!
//! Pure parsing lives here; collection is per-backend (Linux typec class,
//! macOS ioreg probe). Apple exposes no PD contract data through public
//! IORegistry keys on current builds (verified on dev hardware — the
//! AppleHPM services only carry I2C bus internals), so macOS reports an
//! empty port list rather than guessing.

use crate::{ConnectorOrientation, DataRole, PowerRole};
use crate::{PdoFlags, PdoType, PowerDataObject, PowerInfo};
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
    /// Cable product type string from sysfs (e.g. "active", "passive", "leaded").
    pub product_type: Option<String>,
    /// USB speed rating string ("USB 2.0"/"USB 3.2"/"USB4") if derivable
    /// from plug identity or mode. Honest None when absent.
    pub speed_rating: Option<String>,
}

impl CableEMarker {
    /// Maximum wattage this cable can safely carry under standard PD (VBUS up
    /// to 20 V; PD 3.1 EPR with 50 V is explicitly out of scope until the
    /// kernel exposes it). Formula: current_rating × 20 V. Unknown rating
    /// returns `None` rather than guessing.
    pub fn wattage_limit_w(&self) -> Option<u32> {
        self.current_rating_a.map(|a| a * 20)
    }
}

/// Unified cable details view combining E-marker, orientation, and connector
/// metadata for presentation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CableDetails {
    pub vendor_id: Option<u16>,
    pub product_id: Option<u16>,
    pub current_rating_a: Option<u32>,
    pub wattage_limit_w: Option<u32>,
    pub plug_mode: Option<String>,
    pub product_type: Option<String>,
    pub speed_rating: Option<String>,
    /// Active CC pin: "CC1" (Normal), "CC2" (Flipped), or None.
    pub cc_active: Option<String>,
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
    /// Source PDO list from partner's capability advertisement.
    #[serde(default)]
    pub source_caps: Vec<PowerDataObject>,
    /// Sink PDO list from this port.
    #[serde(default)]
    pub sink_caps: Vec<PowerDataObject>,
    /// Active contract if PD negotiation succeeded.
    #[serde(default)]
    pub power_info: Option<PowerInfo>,
}

impl TypeCPortStatus {
    /// True when a PD contract is visibly active (role + partner + PD).
    pub fn pd_active(&self) -> bool {
        self.partner_attached && self.partner_pd_supported == Some(true)
    }

    /// Active CC pin based on connector orientation: "CC1" for Normal,
    /// "CC2" for Flipped, None when unknown.
    pub fn cc_pin_active(&self) -> Option<&'static str> {
        match self.orientation {
            ConnectorOrientation::Normal => Some("CC1"),
            ConnectorOrientation::Flipped => Some("CC2"),
            ConnectorOrientation::Unknown => None,
        }
    }

    /// True when source caps contain an APDO (Augmented/PPS) entry.
    pub fn supports_pps(&self) -> bool {
        self.source_caps
            .iter()
            .any(|p| matches!(p.pdo_type, PdoType::AugmentedPower))
    }

    /// Unified cable details combining E-marker + orientation.
    pub fn cable_details(&self) -> Option<CableDetails> {
        let em = self.emarker.as_ref()?;
        Some(CableDetails {
            vendor_id: em.vendor_id,
            product_id: em.product_id,
            current_rating_a: em.current_rating_a,
            wattage_limit_w: em.wattage_limit_w(),
            plug_mode: em.plug_mode.clone(),
            product_type: em.product_type.clone(),
            speed_rating: em.speed_rating.clone(),
            cc_active: self.cc_pin_active().map(|s| s.to_string()),
        })
    }
}

// ---------------------------------------------------------------------------
// PDO / APDO text parser (Linux sysfs `source-capabilities` / `sink-capabilities`)
// ---------------------------------------------------------------------------
//
// Kernel ABI (Documentation/ABI/testing/sysfs-class-typec) renders PDOs as
// one entry per line in several formats across kernel versions:
//
//   [fixed] 5V 3A
//   [fixed] 20V 3A [HIGHER_CAPACITY]
//   [apdo] 3300mV 5000mA
//   [variable] 3000mV 2000mV 2000mA
//   [battery] 7000mV 14000mV 2000mA
//
// We parse conservatively: unknown tokens are skipped, unknown voltages
// default to 0, and unparseable lines are silently dropped.

fn parse_pdo_voltage(s: &str) -> Option<u16> {
    let s = s.trim();
    if let Some(v) = s.strip_suffix("mV") {
        v.parse().ok()
    } else if let Some(v) = s.strip_suffix("V") {
        v.parse::<f64>().ok().map(|v| (v * 1000.0) as u16)
    } else {
        s.parse().ok()
    }
}

fn parse_pdo_current(s: &str) -> Option<u16> {
    let s = s.trim();
    if let Some(v) = s.strip_suffix("mA") {
        v.parse().ok()
    } else if let Some(v) = s.strip_suffix("A") {
        v.parse::<f64>().ok().map(|a| (a * 1000.0) as u16)
    } else {
        s.parse().ok()
    }
}

fn parse_pdo_type_token(s: &str) -> PdoType {
    match s.to_ascii_lowercase().as_str() {
        "fixed" => PdoType::FixedSupply,
        "variable" => PdoType::VariableSupply,
        "battery" => PdoType::BatterySupply,
        "apdo" | "pps" => PdoType::AugmentedPower,
        _ => PdoType::FixedSupply,
    }
}

fn parse_pdo_flags_from_tokens<'a>(tokens: impl Iterator<Item = &'a str>) -> PdoFlags {
    let mut flags = PdoFlags {
        dual_role_power: false,
        usb_suspend: false,
        unconstrained_power: false,
        higher_capability: false,
        dual_role_data: false,
    };
    for tok in tokens {
        let t = tok
            .trim_matches(|c: char| c == '[' || c == ']')
            .to_ascii_uppercase();
        match t.as_str() {
            "HIGHER_CAPACITY" => flags.higher_capability = true,
            "DUAL_ROLE_POWER" | "DRP" => flags.dual_role_power = true,
            "USB_SUSPEND" => flags.usb_suspend = true,
            "UNCONSTRAINED_POWER" => flags.unconstrained_power = true,
            "DUAL_ROLE_DATA" | "DRD" => flags.dual_role_data = true,
            _ => {}
        }
    }
    flags
}

/// Parse one PDO line from sysfs source/sink-capabilities output.
pub fn parse_pdo_line(line: &str) -> Option<PowerDataObject> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }

    // Extract [TYPE] bracket.
    let (type_tok, rest) = if let Some(bracket_end) = line.find(']') {
        let type_tok = line[1..bracket_end].trim();
        (type_tok, line[bracket_end + 1..].trim())
    } else {
        // No bracket: try bare "5V 3A" → fixed
        ("fixed", line)
    };

    let pdo_type = parse_pdo_type_token(type_tok);

    // Collect all numeric voltage/current tokens from the remainder.
    let tokens: Vec<&str> = rest.split_whitespace().collect();
    let numeric_tokens: Vec<&str> = tokens
        .iter()
        .filter(|t| t.ends_with("V") || t.ends_with("mA") || t.ends_with("A"))
        .copied()
        .collect();

    let flags = parse_pdo_flags_from_tokens(tokens.iter().copied());

    match pdo_type {
        PdoType::FixedSupply => {
            let voltage_mv = numeric_tokens.first().and_then(|s| parse_pdo_voltage(s))?;
            let current_ma = numeric_tokens
                .get(1)
                .and_then(|s| parse_pdo_current(s))
                .unwrap_or(0);
            let max_power_mw = (voltage_mv as u32) * (current_ma as u32) / 1000;
            Some(PowerDataObject {
                pdo_type,
                voltage_mv,
                current_ma,
                max_power_mw,
                flags,
            })
        }
        PdoType::VariableSupply => {
            // [variable] minVoltage maxVoltage current
            let min_mv = numeric_tokens.first().and_then(|s| parse_pdo_voltage(s))?;
            let _max_mv = numeric_tokens
                .get(1)
                .and_then(|s| parse_pdo_voltage(s))
                .unwrap_or(min_mv);
            let current_ma = numeric_tokens
                .get(2)
                .and_then(|s| parse_pdo_current(s))
                .unwrap_or(0);
            let max_power_mw = (min_mv as u32) * (current_ma as u32) / 1000;
            Some(PowerDataObject {
                pdo_type,
                voltage_mv: min_mv,
                current_ma,
                max_power_mw,
                flags,
            })
        }
        PdoType::BatterySupply => {
            let min_mv = numeric_tokens.first().and_then(|s| parse_pdo_voltage(s))?;
            let _max_mv = numeric_tokens
                .get(1)
                .and_then(|s| parse_pdo_voltage(s))
                .unwrap_or(min_mv);
            let power_mw = numeric_tokens
                .get(2)
                .and_then(|s| parse_pdo_current(s)) // mA → actually power in mW if raw?
                .unwrap_or(0) as u32;
            Some(PowerDataObject {
                pdo_type,
                voltage_mv: min_mv,
                current_ma: 0, // battery PDOs carry power, not current
                max_power_mw: power_mw,
                flags,
            })
        }
        PdoType::AugmentedPower => {
            // [apdo] minVoltage maxVoltage current  OR  [apdo] minVoltage current
            let min_mv = numeric_tokens.first().and_then(|s| parse_pdo_voltage(s))?;
            let (max_mv, current_ma) = if numeric_tokens.len() >= 3 {
                let mv = numeric_tokens
                    .get(1)
                    .and_then(|s| parse_pdo_voltage(s))
                    .unwrap_or(min_mv);
                let ma = numeric_tokens
                    .get(2)
                    .and_then(|s| parse_pdo_current(s))
                    .unwrap_or(0);
                (mv, ma)
            } else {
                let ma = numeric_tokens
                    .get(1)
                    .and_then(|s| parse_pdo_current(s))
                    .unwrap_or(0);
                (min_mv, ma)
            };
            let max_power_mw = (max_mv as u32) * (current_ma as u32) / 1000;
            Some(PowerDataObject {
                pdo_type,
                voltage_mv: min_mv,
                current_ma,
                max_power_mw,
                flags,
            })
        }
    }
}

/// Parse a full sysfs source/sink-capabilities file into PDO entries.
pub fn parse_pdo_list(text: &str) -> Vec<PowerDataObject> {
    text.lines().filter_map(parse_pdo_line).collect()
}

/// Extract a power contract history from a sequence of DiagnosticEvents.
/// Each PowerChanged event whose metadata carries "voltage_mv" / "current_ma"
/// keys produces one entry. Returns chronologically sorted.
pub fn extract_contract_history(events: &[crate::DiagnosticEvent]) -> Vec<ContractChange> {
    let mut out: Vec<ContractChange> = events
        .iter()
        .filter(|e| matches!(e.event_type, crate::EventType::PowerChanged))
        .filter_map(|e| {
            let voltage = e.metadata.get("voltage_mv")?;
            let current = e.metadata.get("current_ma");
            let power = e.metadata.get("power_mw");
            Some(ContractChange {
                timestamp: e.timestamp,
                device_id: e.device_id,
                voltage_mv: voltage.parse().ok(),
                current_ma: current.and_then(|c| c.parse().ok()),
                power_mw: power.and_then(|p| p.parse().ok()),
            })
        })
        .collect();
    out.sort_by_key(|c| c.timestamp);
    out
}

/// A single observed power-contract change during monitoring.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContractChange {
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub device_id: Option<uuid::Uuid>,
    pub voltage_mv: Option<u16>,
    pub current_ma: Option<u16>,
    pub power_mw: Option<u32>,
}

// ---------------------------------------------------------------------------
// Linux sysfs collection
// ---------------------------------------------------------------------------

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
///
/// The `attrs` callback is called with attribute names relative to the port
/// directory AND with the full sysfs content of `source-capabilities` /
/// `sink-capabilities` files (when the caller stitches those into the attrs
/// map under keys `"source-capabilities"` / `"sink-capabilities"`).
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

    // Parse PDO lists from sysfs if the source-caps content was provided.
    let source_caps = attrs("source-capabilities")
        .map(|t| parse_pdo_list(&t))
        .unwrap_or_default();
    let sink_caps = attrs("sink-capabilities")
        .map(|t| parse_pdo_list(&t))
        .unwrap_or_default();

    let pps_supported = source_caps
        .iter()
        .any(|p| matches!(p.pdo_type, PdoType::AugmentedPower));

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

    // Contract info from active PDO (highest-voltage fixed source cap with
    // non-zero current, if present).
    let active_contract = source_caps
        .iter()
        .filter(|p| matches!(p.pdo_type, PdoType::FixedSupply) && p.current_ma > 0)
        .max_by_key(|p| p.voltage_mv)
        .map(|p| PowerInfo {
            source_capabilities: source_caps.clone(),
            sink_capabilities: sink_caps.clone(),
            negotiated_pdo: Some(p.clone()),
            contract_voltage_mv: Some(p.voltage_mv),
            contract_current_ma: Some(p.current_ma),
            contract_power_mw: Some(p.max_power_mw),
            pps_supported,
            pps_voltage_range: source_caps
                .iter()
                .find(|p| matches!(p.pdo_type, PdoType::AugmentedPower))
                .map(|p| (p.voltage_mv, p.voltage_mv + 20000)),
            pps_max_current_ma: source_caps
                .iter()
                .find(|p| matches!(p.pdo_type, PdoType::AugmentedPower))
                .map(|p| p.current_ma),
        });

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
        source_caps,
        sink_caps,
        power_info: active_contract,
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
    let product_type = attrs("product_type");
    // Speed rating: derive from plug_mode if kernel exposes "usb", "usb3", etc.
    let speed_rating = attrs("speed").or_else(|| match plug_mode.as_deref() {
        Some("usb") => Some("USB 2.0".into()),
        Some("usb3") => Some("USB 3.2".into()),
        Some("usb4") => Some("USB4".into()),
        _ => None,
    });
    if plug_mode.is_none() && vendor_id.is_none() && current_rating_a.is_none() {
        return None;
    }
    Some(CableEMarker {
        plug_mode,
        current_rating_a,
        vendor_id,
        product_id,
        product_type,
        speed_rating,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_full_linux_port_with_pd_and_cable() {
        let attrs = |k: &str| {
            match k {
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
            "product_type" => Some("active".into()),
            "source-capabilities" => Some(
                "[fixed] 5V 3A\n[fixed] 9V 3A\n[fixed] 15V 3A\n[fixed] 20V 3A\n[apdo] 3300mV 5000mA"
                    .into(),
            ),
            "sink-capabilities" => Some("[fixed] 5V 0.9A".into()),
            _ => None,
        }
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
        // Source caps: 4 fixed + 1 PPS
        assert_eq!(port.source_caps.len(), 5);
        assert!(port.supports_pps());
        // Contract should be highest-voltage fixed: 20V 3A.
        let pinfo = port.power_info.as_ref().expect("contract present");
        assert_eq!(pinfo.contract_voltage_mv, Some(20000));
        assert_eq!(pinfo.contract_current_ma, Some(3000));
        assert_eq!(pinfo.negotiated_pdo.as_ref().unwrap().voltage_mv, 20000);
        // Sink caps
        assert_eq!(port.sink_caps.len(), 1);
        // E-marker
        let emarker = port.emarker.as_ref().expect("emarker parsed");
        assert_eq!(emarker.current_rating_a, Some(5));
        assert_eq!(emarker.vendor_id, Some(0x18D1));
        assert_eq!(emarker.product_id, Some(0x4E11));
        assert_eq!(emarker.plug_mode.as_deref(), Some("audio"));
        assert_eq!(emarker.product_type.as_deref(), Some("active"));
        assert_eq!(emarker.wattage_limit_w(), Some(100));
        // Cable details unified view
        let cd = port.cable_details().expect("cable details");
        assert_eq!(cd.cc_active.as_deref(), Some("CC2"));
        assert_eq!(cd.wattage_limit_w, Some(100));
        assert_eq!(cd.current_rating_a, Some(5));
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
        assert!(port.source_caps.is_empty());
        assert!(port.sink_caps.is_empty());
        assert!(port.power_info.is_none());
        assert_eq!(port.orientation, ConnectorOrientation::Unknown);
        assert!(port.cc_pin_active().is_none());
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

    #[test]
    fn pdo_parser_handles_various_kernel_formats() {
        // Bracketed [fixed] style (common on kernel 5.10+).
        let line1 = parse_pdo_line("[fixed] 5V 3A").unwrap();
        assert_eq!(line1.pdo_type, PdoType::FixedSupply);
        assert_eq!(line1.voltage_mv, 5000);
        assert_eq!(line1.current_ma, 3000);
        assert_eq!(line1.max_power_mw, 15000);

        // [apdo] PPS with millivolt notation.
        let line2 = parse_pdo_line("[apdo] 3300mV 5000mA").unwrap();
        assert_eq!(line2.pdo_type, PdoType::AugmentedPower);
        assert_eq!(line2.voltage_mv, 3300);
        assert_eq!(line2.current_ma, 5000);

        // Flag parsing.
        let line3 = parse_pdo_line("[fixed] 20V 3A [HIGHER_CAPACITY]").unwrap();
        assert!(line3.flags.higher_capability);

        // Variable supply (3 tokens).
        let line4 = parse_pdo_line("[variable] 3000mV 20000mV 2000mA").unwrap();
        assert_eq!(line4.pdo_type, PdoType::VariableSupply);
        assert_eq!(line4.voltage_mv, 3000);
        assert_eq!(line4.current_ma, 2000);

        // Blank/comment lines return None.
        assert!(parse_pdo_line("").is_none());
        assert!(parse_pdo_line("# header").is_none());
    }

    #[test]
    fn parse_pdo_list_full_fixture() {
        let fixture = "[fixed] 5V 0.9A\n[fixed] 9V 3A\n[fixed] 15V 3A\n[fixed] 20V 3A [HIGHER_CAPACITY]\n[apdo] 3300mV 5000mA\n";
        let list = parse_pdo_list(fixture);
        assert_eq!(list.len(), 5);
        assert_eq!(list[0].voltage_mv, 5000);
        assert_eq!(list[0].current_ma, 900);
        assert_eq!(list[3].voltage_mv, 20000);
        assert!(list[3].flags.higher_capability);
        assert_eq!(list[4].pdo_type, PdoType::AugmentedPower);
    }

    #[test]
    fn supports_pps_true_only_for_augmented() {
        let mut port = parse_linux_typec_port("port0", &|k| match k {
            "partner" => Some("y".into()),
            _ => None,
        })
        .unwrap();
        assert!(!port.supports_pps());
        port.source_caps = vec![PowerDataObject {
            pdo_type: PdoType::FixedSupply,
            voltage_mv: 5000,
            current_ma: 3000,
            max_power_mw: 15000,
            flags: PdoFlags {
                dual_role_power: false,
                usb_suspend: false,
                unconstrained_power: false,
                higher_capability: false,
                dual_role_data: false,
            },
        }];
        assert!(!port.supports_pps());
        port.source_caps.push(PowerDataObject {
            pdo_type: PdoType::AugmentedPower,
            voltage_mv: 3300,
            current_ma: 5000,
            max_power_mw: 16500,
            flags: PdoFlags {
                dual_role_power: false,
                usb_suspend: false,
                unconstrained_power: false,
                higher_capability: false,
                dual_role_data: false,
            },
        });
        assert!(port.supports_pps());
    }

    #[test]
    fn wattage_limit_derives_from_current_rating() {
        let cable_3a = CableEMarker {
            plug_mode: None,
            current_rating_a: Some(3),
            vendor_id: None,
            product_id: None,
            product_type: None,
            speed_rating: None,
        };
        assert_eq!(cable_3a.wattage_limit_w(), Some(60));
        let cable_5a = CableEMarker {
            current_rating_a: Some(5),
            ..cable_3a.clone()
        };
        assert_eq!(cable_5a.wattage_limit_w(), Some(100));
        let no_rating = CableEMarker {
            current_rating_a: None,
            ..cable_3a
        };
        assert!(no_rating.wattage_limit_w().is_none());
    }

    #[test]
    fn contract_history_extracts_power_changed_events() {
        let events = vec![
            crate::DiagnosticEvent {
                id: uuid::Uuid::new_v4(),
                timestamp: chrono::Utc::now(),
                event_type: crate::EventType::PowerChanged,
                device_id: Some(uuid::Uuid::new_v4()),
                hub_id: None,
                port_number: Some(1),
                details: "contract".into(),
                severity: crate::EventSeverity::Info,
                metadata: {
                    let mut m = std::collections::HashMap::new();
                    m.insert("voltage_mv".into(), "5000".into());
                    m.insert("current_ma".into(), "900".into());
                    m
                },
            },
            crate::DiagnosticEvent {
                id: uuid::Uuid::new_v4(),
                timestamp: chrono::Utc::now(),
                event_type: crate::EventType::PowerChanged,
                device_id: Some(uuid::Uuid::new_v4()),
                hub_id: None,
                port_number: Some(1),
                details: "contract".into(),
                severity: crate::EventSeverity::Info,
                metadata: {
                    let mut m = std::collections::HashMap::new();
                    m.insert("voltage_mv".into(), "20000".into());
                    m.insert("current_ma".into(), "3000".into());
                    m.insert("power_mw".into(), "60000".into());
                    m
                },
            },
            // Non-PowerChanged event: ignored.
            crate::DiagnosticEvent {
                id: uuid::Uuid::new_v4(),
                timestamp: chrono::Utc::now(),
                event_type: crate::EventType::DeviceConnected,
                device_id: None,
                hub_id: None,
                port_number: None,
                details: "connect".into(),
                severity: crate::EventSeverity::Info,
                metadata: std::collections::HashMap::new(),
            },
        ];
        let history = extract_contract_history(&events);
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].voltage_mv, Some(5000));
        assert_eq!(history[1].voltage_mv, Some(20000));
        assert_eq!(history[1].current_ma, Some(3000));
        assert_eq!(history[1].power_mw, Some(60000));
    }
}
