//! USB speed mapping for macOS sources.
//!
//! Two independent signals, per DATA_MAP §4:
//! - IORegistry `Speed` property (IOKit `USBDeviceSpeed` enum) → **negotiated**
//!   link speed; codes align 1:1-ish with the core enum.
//! - system_profiler `speed` string ("Up to 480 Mb/s") → **advertised** max
//!   only; never treated as negotiated.
//!
//! Pure logic compiles and tests everywhere.

use skirr_core::UsbSpeed;

/// IOKit `USBDeviceSpeed`: None=0, Low=1, Full=2, High=3, Super=4,
/// SuperPlus=5, SuperPlusBy2=6 (20 Gb/s lane-pairing).
pub(crate) fn map_speed_code(code: u32) -> UsbSpeed {
    match code {
        0 => UsbSpeed::Unknown,
        1 => UsbSpeed::LowSpeed,
        2 => UsbSpeed::FullSpeed,
        3 => UsbSpeed::HighSpeed,
        4 => UsbSpeed::SuperSpeed,
        5 => UsbSpeed::SuperSpeedPlus10,
        6 => UsbSpeed::SuperSpeedPlus20,
        _ => UsbSpeed::Unknown,
    }
}

/// Parse a system_profiler speed string into Mb/s:
/// `"Up to 480 Mb/s"` → 480, `"Up to 5 Gb/s"` → 5000.
/// Accepts `Mb/s`, `Mb/sec`, `Gb/s`, `Gb/sec` spellings.
pub(crate) fn parse_advertised_mbps(raw: &str) -> Option<u32> {
    let lower = raw.to_ascii_lowercase();
    let value: f64 = lower.split_whitespace().find_map(|tok| tok.parse().ok())?;
    let mbit = if lower.contains("gb/s") || lower.contains("gb/sec") {
        value * 1000.0
    } else if lower.contains("mb/s") || lower.contains("mb/sec") {
        value
    } else {
        return None;
    };
    u32::try_from(mbit as i64).ok()
}

/// Advertised bandwidth → core enum tier.
pub(crate) fn mbps_to_speed(mbps: u32) -> UsbSpeed {
    match mbps {
        0..=2 => UsbSpeed::LowSpeed,
        3..=100 => UsbSpeed::FullSpeed,
        101..=4000 => UsbSpeed::HighSpeed,
        4001..=7500 => UsbSpeed::SuperSpeed,
        7501..=15_000 => UsbSpeed::SuperSpeedPlus10,
        15_001..=30_000 => UsbSpeed::SuperSpeedPlus20,
        _ => UsbSpeed::USB4Gen4x2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speed_codes_align_with_apple_enum() {
        assert_eq!(map_speed_code(0), UsbSpeed::Unknown);
        assert_eq!(map_speed_code(1), UsbSpeed::LowSpeed);
        assert_eq!(map_speed_code(2), UsbSpeed::FullSpeed);
        assert_eq!(map_speed_code(3), UsbSpeed::HighSpeed);
        assert_eq!(map_speed_code(4), UsbSpeed::SuperSpeed);
        assert_eq!(map_speed_code(5), UsbSpeed::SuperSpeedPlus10);
        assert_eq!(map_speed_code(6), UsbSpeed::SuperSpeedPlus20);
        assert_eq!(map_speed_code(9), UsbSpeed::Unknown);
    }

    #[test]
    fn advertised_strings_parse_both_units() {
        assert_eq!(parse_advertised_mbps("Up to 480 Mb/s"), Some(480));
        assert_eq!(parse_advertised_mbps("Up to 12 Mb/sec"), Some(12));
        assert_eq!(parse_advertised_mbps("Up to 5 Gb/s"), Some(5000));
        assert_eq!(parse_advertised_mbps("Up to 10 Gb/s"), Some(10_000));
        assert_eq!(parse_advertised_mbps("Up to 40 Gb/s"), Some(40_000));
        assert_eq!(parse_advertised_mbps("nonsense"), None);
    }

    #[test]
    fn mbps_bands_map_to_tiers() {
        assert_eq!(mbps_to_speed(12), UsbSpeed::FullSpeed);
        assert_eq!(mbps_to_speed(480), UsbSpeed::HighSpeed);
        assert_eq!(mbps_to_speed(5000), UsbSpeed::SuperSpeed);
        assert_eq!(mbps_to_speed(10_000), UsbSpeed::SuperSpeedPlus10);
        assert_eq!(mbps_to_speed(20_000), UsbSpeed::SuperSpeedPlus20);
        assert_eq!(mbps_to_speed(40_000), UsbSpeed::USB4Gen4x2);
    }
}
