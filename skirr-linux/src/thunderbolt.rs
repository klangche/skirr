//! Thunderbolt/USB4 router discovery via `/sys/bus/thunderbolt/devices`
//! (Phase 10.1). Attribute parsing is shared with the core module; this file
//! only walks sysfs and reads attribute files. Absent bus support yields an
//! empty list — never an error.

use skirr_core::ThunderboltRouter;
use std::path::Path;

/// Well-known sysfs mount for the thunderbolt bus.
const SYSFS_BUS: &str = "/sys/bus/thunderbolt/devices";

/// Walk every device directory and build routers. Directory names look like
/// `domain0`, `0-0` (host router), `0-3.1` (nested device).
pub fn collect() -> Vec<ThunderboltRouter> {
    let mut routers = Vec::new();
    let Ok(entries) = std::fs::read_dir(SYSFS_BUS) else {
        return routers; // no TB bus on this kernel/driver: honest empty
    };
    let mut dirs: Vec<_> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    for dir in dirs {
        let name = match dir.file_name().and_then(|n| n.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };
        let attrs = |attr: &str| read_attr(&dir, attr);
        if let Some(router) = skirr_core::thunderbolt::parse_linux_router(&name, &attrs) {
            routers.push(router);
        }
    }
    routers
}

fn read_attr(dir: &Path, attr: &str) -> Option<String> {
    std::fs::read_to_string(dir.join(attr))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn missing_bus_yields_empty_not_error() {
        // On non-Linux dev hosts /sys/bus/thunderbolt does not exist.
        // The contract is empty output, never panic/error.
        let routers = collect();
        assert!(routers.is_empty() || cfg!(target_os = "linux"));
    }

    #[test]
    fn parse_linux_router_matches_core_contract() {
        let mut attrs: HashMap<&str, String> = HashMap::new();
        attrs.insert("unique_id", "abc123".into());
        attrs.insert("device_name", "Dock".into());
        attrs.insert("generation", "4".into());
        let lookup = |k: &str| attrs.get(k).cloned();
        let r = skirr_core::thunderbolt::parse_linux_router("0-3", &lookup).expect("router");
        assert_eq!(r.id, "abc123");
        assert_eq!(r.depth, 1);
    }
}
