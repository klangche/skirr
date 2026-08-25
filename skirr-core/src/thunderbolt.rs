//! Thunderbolt / USB4 router discovery (Phase 10.1).
//!
//! Pure parsing lives here so it is testable without hardware; collection is
//! per-backend (`system_profiler SPThunderboltDataType -json` on macOS,
//! `/sys/bus/thunderbolt/devices` on Linux). Windows exposes no reliable
//! public API for TB topology — routers stay empty there by design.

use crate::model::{ThunderboltGeneration, ThunderboltSecurityLevel};
use serde_json::Value;

/// One Thunderbolt/USB4 switch (router) in the TB fabric. Host routers
/// (depth 0) appear alongside device routers; receptacles are the physical
/// ports on that router.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ThunderboltRouter {
    /// Switch UID (macOS `switch_uid_key`) or sysfs `unique_id`.
    pub id: String,
    pub name: String,
    pub vendor_name: Option<String>,
    pub route_string: Option<String>,
    pub domain_uuid: Option<String>,
    pub generation: Option<ThunderboltGeneration>,
    /// True when the router identifies as USB4 (e.g. Apple's
    /// "thunderboltusb4_bus" naming or Linux `usb4_` device prefix).
    pub is_usb4: bool,
    pub security_level: Option<ThunderboltSecurityLevel>,
    pub nvm_version: Option<String>,
    /// Hop depth in the TB fabric; host routers are 0.
    pub depth: u8,
    /// OS-reported health ("ok", …) where available.
    pub status: Option<String>,
    pub receptacles: Vec<TbReceptacle>,
}

/// A physical port on a TB router.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TbReceptacle {
    pub id: Option<String>,
    pub status: Option<String>,
    /// Advertised lane speed as reported ("Up to 40 Gb/s").
    pub current_speed: Option<String>,
    pub link_status: Option<String>,
}

// --- macOS: system_profiler SPThunderboltDataType -json ----------------------

/// Parse `system_profiler SPThunderboltDataType -json` output.
///
/// Observed key names carry a `_key` suffix on this macOS build (e.g.
/// `switch_uid_key`, `receptacle_1_tag`); older/newer builds drop the suffix,
/// so matching is done on the suffix-stripped name.
pub fn parse_system_profiler_thunderbolt(json: &str) -> Result<Vec<ThunderboltRouter>, String> {
    let doc: Value =
        serde_json::from_str(json).map_err(|e| format!("system_profiler json: {e}"))?;
    let entries = doc
        .get("SPThunderboltDataType")
        .and_then(Value::as_array)
        .ok_or_else(|| "missing SPThunderboltDataType array".to_string())?;

    let mut routers = Vec::new();
    for entry in entries {
        if !entry.is_object() {
            continue;
        }
        // Flatten one entry: direct string fields + nested receptacle_N_tag
        // objects, all keyed by their suffix-stripped names.
        let mut fields: Vec<(String, String)> = Vec::new();
        let mut receptacles: Vec<TbReceptacle> = Vec::new();
        for (key, val) in entry.as_object().expect("checked above") {
            let bare = key.strip_suffix("_key").unwrap_or(key);
            match val {
                Value::String(s) => fields.push((bare.to_string(), s.clone())),
                Value::Object(_) if bare.starts_with("receptacle_") => {
                    receptacles.push(parse_receptacle(bare, val));
                }
                _ => {}
            }
        }

        let get = |name: &str| {
            fields
                .iter()
                .find(|(k, _)| k == name || k == &format!("switch_{name}"))
                .map(|(_, v)| v.clone())
        };

        let uid = get("uid")
            .or_else(|| get("unique_id"))
            .unwrap_or_else(|| format!("unknown-{}", routers.len()));
        let raw_name = entry
            .get("_name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let gen_str = get("generation");
        let is_usb4 = raw_name.contains("usb4")
            || raw_name.contains("usb_4")
            || gen_str
                .as_deref()
                .map(|g| g.to_ascii_lowercase().contains("usb4"))
                .unwrap_or(false);
        routers.push(ThunderboltRouter {
            depth: depth_from_route(get("route_string").as_deref()),
            name: get("device_name")
                .filter(|s| !s.is_empty())
                .unwrap_or(raw_name),
            vendor_name: get("vendor_name"),
            route_string: get("route_string"),
            domain_uuid: get("domain_uuid"),
            generation: get("generation").as_deref().and_then(parse_generation),
            is_usb4,
            security_level: get("security_level").as_deref().and_then(parse_security),
            nvm_version: get("nvm_version"),
            status: get("device_status"),
            id: uid,
            receptacles,
        });
    }
    routers.sort_by(|a, b| a.depth.cmp(&b.depth).then_with(|| a.id.cmp(&b.id)));
    Ok(routers)
}

fn parse_receptacle(bare_key: &str, obj: &Value) -> TbReceptacle {
    let str_at = |k: &str| {
        obj.get(k)
            .or_else(|| obj.get(format!("{k}_key")))
            .and_then(Value::as_str)
            .map(str::to_string)
    };
    TbReceptacle {
        id: bare_key
            .strip_prefix("receptacle_")
            .and_then(|s| s.strip_suffix("_tag"))
            .map(str::to_string),
        status: str_at("receptacle_status"),
        current_speed: str_at("current_speed"),
        link_status: str_at("link_status"),
    }
}

fn parse_generation(s: &str) -> Option<ThunderboltGeneration> {
    let lower = s.to_ascii_lowercase();
    Some(match lower.as_str() {
        s if s.contains("usb4") => return None, // USB4 routers: is_usb4 flag, no TB gen
        s if s.contains('5') => ThunderboltGeneration::Thunderbolt5,
        s if s.contains('4') => ThunderboltGeneration::Thunderbolt4,
        s if s.contains('3') => ThunderboltGeneration::Thunderbolt3,
        s if s.contains('2') && s.contains("thunderbolt") => ThunderboltGeneration::Thunderbolt2,
        s if s.contains('1') && s.contains("thunderbolt") => ThunderboltGeneration::Thunderbolt1,
        _ => return None,
    })
}

fn parse_security(s: &str) -> Option<ThunderboltSecurityLevel> {
    Some(match s.to_ascii_lowercase().as_str() {
        "none" | "no_security" | "nosecurity" => ThunderboltSecurityLevel::NoSecurity,
        "user" | "user_authorization" | "userauthorization" => {
            ThunderboltSecurityLevel::UserAuthorization
        }
        "secure" | "secure_connect" | "secureconnect" => ThunderboltSecurityLevel::SecureConnect,
        "dponly" | "dp_only" => ThunderboltSecurityLevel::DPOnly,
        "usbonly" | "usb_only" => ThunderboltSecurityLevel::UsbOnly,
        _ => return None,
    })
}

/// Route strings are hop sequences ("0" host, "0/1"/"3.1" children); depth is
/// the number of separators + base. Unparsable routes report 0.
fn depth_from_route(route: Option<&str>) -> u8 {
    route
        .map(|r| {
            let r = r.trim_start_matches('0');
            if r.is_empty() {
                0
            } else {
                r.chars().filter(|c| *c == '.' || *c == '/').count() as u8 + 1
            }
        })
        .unwrap_or(0)
}

// --- Linux: /sys/bus/thunderbolt/devices attribute sets -----------------------

/// Build a router from one sysfs device directory's attributes. `dir` is the
/// directory name (e.g. `0-1`, `0-3.1`, `domain0`); entries without a usable
/// identity yield None and are skipped.
pub fn parse_linux_router(
    dir: &str,
    attrs: &dyn Fn(&str) -> Option<String>,
) -> Option<ThunderboltRouter> {
    if dir.starts_with("domain") {
        return None;
    }
    let unique_id = attrs("unique_id");
    let device_name = attrs("device_name");
    if unique_id.is_none() && device_name.is_none() {
        return None;
    }
    let usb4_marker = attrs("usb4_version").is_some();
    let route = dir.split_once('-').map(|(_, r)| r.to_string());
    // Route "0" is the host router (depth 0); otherwise each dot-separated
    // segment is one fabric level ("1" → depth 1, "3.1" → depth 2).
    let depth = match route.as_deref() {
        Some("0") | None => 0,
        Some(r) => r.split('.').count() as u8,
    };
    let generation = attrs("generation")
        .as_deref()
        .map(|g| parse_generation(g).unwrap_or_else(|| gen_from_number(g)));
    Some(ThunderboltRouter {
        id: unique_id.unwrap_or_else(|| dir.to_string()),
        name: device_name.unwrap_or_else(|| dir.to_string()),
        vendor_name: attrs("vendor_name"),
        route_string: route,
        domain_uuid: attrs("domain_id").map(|d| format!("domain-{d}")),
        generation,
        is_usb4: usb4_marker || dir.contains("usb4"),
        security_level: attrs("security").as_deref().and_then(parse_security),
        nvm_version: attrs("nvm_version"),
        depth,
        status: attrs("authorized").map(|a| format!("authorized={a}")),
        receptacles: Vec::new(),
    })
}

/// Linux `generation` files hold plain numbers ("3", "4").
fn gen_from_number(g: &str) -> ThunderboltGeneration {
    match g.trim() {
        "1" => ThunderboltGeneration::Thunderbolt1,
        "2" => ThunderboltGeneration::Thunderbolt2,
        "3" => ThunderboltGeneration::Thunderbolt3,
        "4" => ThunderboltGeneration::Thunderbolt4,
        _ => ThunderboltGeneration::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_real_system_profiler_host_routers() {
        let json = r#"{
          "SPThunderboltDataType" : [
            {
              "_name" : "thunderboltusb4_bus_2",
              "device_name_key" : "MacBook Pro",
              "domain_uuid_key" : "5693EDBE-82A0-4A7D-ADD4-E861D14CBE36",
              "receptacle_1_tag" : {
                "current_speed_key" : "Up to 40 Gb/s",
                "link_status_key" : "0x100",
                "receptacle_id_key" : "3",
                "receptacle_status_key" : "receptacle_no_devices_connected"
              },
              "route_string_key" : "0",
              "switch_uid_key" : "0x05AC9DB544B7AC62",
              "vendor_name_key" : "Apple Inc."
            }
          ]
        }"#;
        let routers = parse_system_profiler_thunderbolt(json).expect("parses");
        assert_eq!(routers.len(), 1);
        let r = &routers[0];
        assert_eq!(r.id, "0x05AC9DB544B7AC62");
        assert_eq!(r.name, "MacBook Pro");
        assert_eq!(r.vendor_name.as_deref(), Some("Apple Inc."));
        assert_eq!(r.route_string.as_deref(), Some("0"));
        assert_eq!(
            r.domain_uuid.as_deref(),
            Some("5693EDBE-82A0-4A7D-ADD4-E861D14CBE36")
        );
        assert!(r.is_usb4, "thunderboltusb4_bus naming implies USB4-capable");
        assert_eq!(r.depth, 0, "host router at route 0");
        assert_eq!(r.receptacles.len(), 1);
        let rec = &r.receptacles[0];
        assert_eq!(rec.id.as_deref(), Some("1"));
        assert_eq!(
            rec.status.as_deref(),
            Some("receptacle_no_devices_connected")
        );
        assert_eq!(rec.current_speed.as_deref(), Some("Up to 40 Gb/s"));
        assert_eq!(rec.link_status.as_deref(), Some("0x100"));
    }

    #[test]
    fn parses_attached_device_router_and_depth() {
        let json = r#"{
          "SPThunderboltDataType" : [
            { "_name": "dock", "device_name_key": "CalDigit TS4",
              "switch_uid_key": "0xAABB", "route_string_key": "3",
              "generation_key": "USB4", "security_level_key": "none",
              "nvm_version_key": "41.5", "device_status_key": "ok",
              "receptacle_2_tag": {"receptacle_status_key": "receptacle_connected",
                                   "current_speed_key": "Up to 40 Gb/s"} }
          ]
        }"#;
        let routers = parse_system_profiler_thunderbolt(json).expect("parses");
        let r = &routers[0];
        assert_eq!(r.name, "CalDigit TS4");
        assert!(r.is_usb4);
        assert_eq!(r.generation, None, "USB4 has no TB generation");
        assert_eq!(r.security_level, Some(ThunderboltSecurityLevel::NoSecurity));
        assert_eq!(r.nvm_version.as_deref(), Some("41.5"));
        assert_eq!(r.status.as_deref(), Some("ok"));
        assert_eq!(r.depth, 1, "route '3' is one hop");
        assert_eq!(r.receptacles[0].id.as_deref(), Some("2"));
    }

    #[test]
    fn tb_generations_and_depths_parse() {
        use ThunderboltGeneration::*;
        assert_eq!(parse_generation("Thunderbolt 3"), Some(Thunderbolt3));
        assert_eq!(parse_generation("Thunderbolt 4"), Some(Thunderbolt4));
        assert_eq!(parse_generation("USB4"), None);
        assert_eq!(depth_from_route(Some("0")), 0);
        assert_eq!(depth_from_route(Some("3")), 1);
        assert_eq!(depth_from_route(Some("3/1")), 2);
        assert_eq!(depth_from_route(None), 0);
    }

    #[test]
    fn linux_attrs_build_router() {
        let attrs = |k: &str| match k {
            "unique_id" => Some("048f3a".into()),
            "device_name" => Some("ThinkPad Dock".into()),
            "vendor_name" => Some("Lenovo".into()),
            "generation" => Some("4".into()),
            "security" => Some("user".into()),
            "nvm_version" => Some("44.0".into()),
            "authorized" => Some("1".into()),
            _ => None,
        };
        let r = parse_linux_router("0-3.1", &attrs).expect("router parsed");
        assert_eq!(r.id, "048f3a");
        assert_eq!(r.name, "ThinkPad Dock");
        assert_eq!(r.route_string.as_deref(), Some("3.1"));
        assert_eq!(r.depth, 2);
        assert_eq!(
            r.security_level,
            Some(ThunderboltSecurityLevel::UserAuthorization)
        );
        assert_eq!(r.nvm_version.as_deref(), Some("44.0"));
        assert_eq!(r.status.as_deref(), Some("authorized=1"));

        // Host router: no unique_id/device_name beyond the dir name.
        let host = parse_linux_router("0-0", &|k| (k == "unique_id").then(|| "root".into()))
            .expect("host router");
        assert_eq!(host.depth, 0);

        // Domain dirs are not routers.
        assert!(parse_linux_router("domain0", &|_| None).is_none());
    }

    #[test]
    fn garbage_input_is_a_clean_error() {
        assert!(parse_system_profiler_thunderbolt("not json").is_err());
        assert!(parse_system_profiler_thunderbolt("{}").is_err());
        assert!(
            parse_system_profiler_thunderbolt(r#"{"SPThunderboltDataType": []}"#)
                .map(|v| v.is_empty())
                .unwrap_or(false)
        );
    }
}
