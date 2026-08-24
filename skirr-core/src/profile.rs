//! Stability profiles - per-platform USB chain limits used by the rule engine.
//!
//! Defaults encode the "Skirr Standard Profile v1.0" limits ported from
//! ProAV Shoko's `usb_data.csv` (see docs/DATA_MAP.md §12).

use crate::model::FactCategory;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Identifies a platform family for limit lookup.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum PlatformKey {
    WindowsX86,
    WindowsArm,
    MacIntel,
    MacAppleSilicon,
    LinuxX86,
    LinuxArm,
    #[serde(rename = "iphone_usbc")]
    IPhoneUsbC,
    #[serde(rename = "android_usbc")]
    AndroidUsbC,
    #[serde(rename = "ipad_usbc")]
    IPadUsbC,
}

impl PlatformKey {
    /// Detect the host platform family from OS/arch strings
    /// (e.g. `("macOS", "aarch64")` -> MacAppleSilicon).
    pub fn detect(os: &str, arch: &str) -> Option<Self> {
        let os = os.to_ascii_lowercase();
        let arch = arch.to_ascii_lowercase();
        let is_arm = arch.starts_with("aarch") || arch.starts_with("arm");
        match os.as_str() {
            "windows" => Some(if is_arm {
                PlatformKey::WindowsArm
            } else {
                PlatformKey::WindowsX86
            }),
            "macos" | "darwin" => Some(if arch.starts_with("aarch") {
                PlatformKey::MacAppleSilicon
            } else {
                PlatformKey::MacIntel
            }),
            "linux" => Some(if is_arm {
                PlatformKey::LinuxArm
            } else {
                PlatformKey::LinuxX86
            }),
            _ => None,
        }
    }

    /// Human-readable display name.
    pub fn display_name(&self) -> &'static str {
        match self {
            PlatformKey::WindowsX86 => "Windows x86/x64",
            PlatformKey::WindowsArm => "Windows ARM",
            PlatformKey::MacIntel => "macOS Intel",
            PlatformKey::MacAppleSilicon => "macOS Apple Silicon",
            PlatformKey::LinuxX86 => "Linux x86/x64",
            PlatformKey::LinuxArm => "Linux ARM",
            PlatformKey::IPhoneUsbC => "iPhone (USB-C)",
            PlatformKey::AndroidUsbC => "Android (USB-C)",
            PlatformKey::IPadUsbC => "iPad (USB-C)",
        }
    }

    /// Whether this platform is a supported *host* for running Skirr itself
    /// (mobile entries are client-side reference targets only).
    pub fn is_host_platform(&self) -> bool {
        matches!(
            self,
            PlatformKey::WindowsX86
                | PlatformKey::WindowsArm
                | PlatformKey::MacIntel
                | PlatformKey::MacAppleSilicon
                | PlatformKey::LinuxX86
                | PlatformKey::LinuxArm
        )
    }
}

/// Chain-length stability limits for one platform family.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct StabilityLimits {
    pub max_hops: u8,
    pub max_tiers: u8,
    pub max_hubs: u8,
}

impl StabilityLimits {
    pub fn new(max_hops: u8, max_tiers: u8, max_hubs: u8) -> Self {
        Self {
            max_hops,
            max_tiers,
            max_hubs,
        }
    }
}

/// A single configurable rule override inside a profile.
///
/// Rules themselves are evaluated by the rule engine (Phase 1.3); the profile
/// only stores thresholds that deviate from built-in defaults.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuleOverride {
    pub rule_id: String,
    pub category: FactCategory,
    pub threshold: i64,
    pub description: Option<String>,
}

/// A named set of platform limits plus rule overrides.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Profile {
    pub version: String,
    pub name: String,
    pub description: Option<String>,
    pub limits: BTreeMap<PlatformKey, StabilityLimits>,
    pub rules: Vec<RuleOverride>,
}

impl Profile {
    /// The built-in "Skirr Standard Profile v1.0".
    ///
    /// Values from Shoko's usb_data.csv; Apple Silicon and Linux ARM carry the
    /// stricter limits because an internal Thunderbolt hub consumes 1 tier.
    pub fn standard_v1() -> Self {
        let mut limits = BTreeMap::new();
        limits.insert(PlatformKey::WindowsX86, StabilityLimits::new(7, 7, 5));
        limits.insert(PlatformKey::WindowsArm, StabilityLimits::new(7, 7, 5));
        limits.insert(PlatformKey::MacIntel, StabilityLimits::new(7, 7, 5));
        limits.insert(PlatformKey::MacAppleSilicon, StabilityLimits::new(6, 6, 4));
        limits.insert(PlatformKey::LinuxX86, StabilityLimits::new(7, 7, 5));
        limits.insert(PlatformKey::LinuxArm, StabilityLimits::new(6, 6, 4));
        // Reference rows: mobile clients (not Skirr hosts, kept for reports).
        limits.insert(PlatformKey::IPhoneUsbC, StabilityLimits::new(4, 4, 2));
        limits.insert(PlatformKey::AndroidUsbC, StabilityLimits::new(5, 5, 3));
        limits.insert(PlatformKey::IPadUsbC, StabilityLimits::new(5, 5, 3));

        Self {
            version: "1.0".to_string(),
            name: "Skirr Standard Profile".to_string(),
            description: Some(
                "Default stability limits ported from ProAV Shoko usb_data.csv".to_string(),
            ),
            limits,
            rules: Vec::new(),
        }
    }

    /// Look up limits for a platform; returns `None` if unknown to this profile.
    pub fn limits_for(&self, platform: PlatformKey) -> Option<StabilityLimits> {
        self.limits.get(&platform).copied()
    }

    /// Find a rule override by id.
    pub fn rule_override(&self, rule_id: &str) -> Option<&RuleOverride> {
        self.rules.iter().find(|r| r.rule_id == rule_id)
    }

    /// Serialize as pretty TOML (for `skirr profile --dump` style flows).
    pub fn to_toml_pretty(&self) -> Result<String, ProfileError> {
        toml::to_string_pretty(self).map_err(|e| ProfileError::Toml(e.to_string()))
    }

    /// Parse from TOML text.
    pub fn from_toml_str(s: &str) -> Result<Self, ProfileError> {
        toml::from_str(s).map_err(|e| ProfileError::Toml(e.to_string()))
    }

    /// Parse from JSON text.
    pub fn from_json_str(s: &str) -> Result<Self, ProfileError> {
        serde_json::from_str(s).map_err(|e| ProfileError::Json(e.to_string()))
    }

    /// Load from a file; format chosen by extension (.toml/.json).
    pub fn from_path(path: impl AsRef<std::path::Path>) -> Result<Self, ProfileError> {
        let path = path.as_ref();
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .unwrap_or_default();
        let text = std::fs::read_to_string(path)?;
        match ext.as_str() {
            "toml" => Self::from_toml_str(&text),
            "json" => Self::from_json_str(&text),
            other => Err(ProfileError::UnsupportedExtension(other.to_string())),
        }
    }

    /// Save as the file format matching the extension.
    pub fn save(&self, path: impl AsRef<std::path::Path>) -> Result<(), ProfileError> {
        let path = path.as_ref();
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .unwrap_or_default();
        let text = match ext.as_str() {
            "json" => {
                serde_json::to_string_pretty(self).map_err(|e| ProfileError::Json(e.to_string()))?
            }
            "toml" => self.to_toml_pretty()?,
            other => return Err(ProfileError::UnsupportedExtension(other.to_string())),
        };
        std::fs::write(path, text)?;
        Ok(())
    }
}

/// Errors from profile loading/saving.
#[derive(Debug, thiserror::Error)]
pub enum ProfileError {
    #[error("invalid TOML profile: {0}")]
    Toml(String),
    #[error("invalid JSON profile: {0}")]
    Json(String),
    #[error("unsupported profile file extension: {0}")]
    UnsupportedExtension(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard_v1_has_all_host_platforms() {
        let p = Profile::standard_v1();
        for key in [
            PlatformKey::WindowsX86,
            PlatformKey::WindowsArm,
            PlatformKey::MacIntel,
            PlatformKey::MacAppleSilicon,
            PlatformKey::LinuxX86,
            PlatformKey::LinuxArm,
        ] {
            assert!(p.limits_for(key).is_some(), "missing limits for {key:?}");
            assert!(key.is_host_platform());
        }
    }

    #[test]
    fn apple_silicon_is_stricter_than_windows() {
        let p = Profile::standard_v1();
        let mac_arm = p.limits_for(PlatformKey::MacAppleSilicon).unwrap();
        let win = p.limits_for(PlatformKey::WindowsX86).unwrap();
        assert_eq!(win, StabilityLimits::new(7, 7, 5));
        assert_eq!(mac_arm, StabilityLimits::new(6, 6, 4));
        assert!(mac_arm.max_hops < win.max_hops);
    }

    #[test]
    fn serializes_roundtrip() {
        let p = Profile::standard_v1();
        let json = serde_json::to_string_pretty(&p).unwrap();
        let back: Profile = serde_json::from_str(&json).unwrap();
        assert_eq!(p, back);
    }

    #[test]
    fn rule_overrides_are_findable() {
        let mut p = Profile::standard_v1();
        p.rules.push(RuleOverride {
            rule_id: "max_hubs".to_string(),
            category: FactCategory::Topology,
            threshold: 3,
            description: Some("Strict venue policy".to_string()),
        });
        assert_eq!(p.rule_override("max_hubs").unwrap().threshold, 3);
        assert!(p.rule_override("nonexistent").is_none());
    }
}
