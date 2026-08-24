//! Speed detection for Linux. sysfs `speed` is the authoritative negotiated
//! rate (DATA_MAP §4); `bcdUSB` provides the capability floor.

use crate::native::{self, RawDeviceInfo};
use skirr_core::{
    BottleneckSeverity, HostController, RootHub, SpeedBottleneck, SpeedReport, SystemTopology,
    UsbDevice, UsbSpeed,
};

/// Negotiated + capability-derived speeds for one raw device.
pub fn speeds_for(raw: &RawDeviceInfo) -> (UsbSpeed, UsbSpeed) {
    let negotiated = raw
        .speed_mbps
        .map(native::map_speed_code)
        .unwrap_or(UsbSpeed::Unknown);
    let floor = native::derive_min_speed(raw.bcd_usb.as_deref());
    // Capability floor only raises the max; it never claims a slower device
    // is faster than its negotiated link.
    let max = if speed_rank(floor) > speed_rank(negotiated) && negotiated != UsbSpeed::Unknown {
        floor
    } else {
        negotiated
    };
    (negotiated, max)
}

fn speed_rank(speed: UsbSpeed) -> u8 {
    match speed {
        UsbSpeed::Unknown => 0,
        UsbSpeed::LowSpeed => 1,
        UsbSpeed::FullSpeed => 2,
        UsbSpeed::HighSpeed => 3,
        UsbSpeed::SuperSpeed => 4,
        UsbSpeed::SuperSpeedPlus10 => 5,
        UsbSpeed::SuperSpeedPlus20 => 6,
        _ => 7,
    }
}

/// Inline bottleneck check mirroring the other backends: a device running
/// below its own capability through no fault of the link itself.
pub fn bottleneck_for(dev: &UsbDevice) -> Option<(BottleneckSeverity, String)> {
    let rank_current = speed_rank(dev.current_link_speed);
    let rank_max = speed_rank(dev.max_supported_speed);
    if rank_max <= rank_current || dev.max_supported_speed == UsbSpeed::Unknown {
        return None;
    }
    let gap = rank_max - rank_current;
    let severity = match gap {
        1 => BottleneckSeverity::Minor,
        2 => BottleneckSeverity::Major,
        _ => BottleneckSeverity::Critical,
    };
    Some((
        severity,
        format!(
            "device supports {:?} but links at {:?}",
            dev.max_supported_speed, dev.current_link_speed
        ),
    ))
}

/// Full speed report over an existing topology snapshot.
pub fn build_report(topo: &SystemTopology) -> Vec<SpeedReport> {
    let mut reports = Vec::new();
    for dev in &topo.devices {
        if let Some((severity, _detail)) = bottleneck_for(dev) {
            reports.push(SpeedReport {
                device_id: dev.id,
                max_supported: dev.max_supported_speed,
                current_link: dev.current_link_speed,
                bottleneck: Some(SpeedBottleneck {
                    device_id: dev.id,
                    max_speed: dev.max_supported_speed,
                    current_speed: dev.current_link_speed,
                    severity,
                }),
            });
        }
    }
    reports
}

#[allow(unused)]
fn type_surface(_: (&HostController, &RootHub)) {}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(speed: Option<u32>, bcd: Option<&str>) -> RawDeviceInfo {
        RawDeviceInfo {
            name: "3-1".into(),
            busnum: 3,
            devnum: 0,
            vendor_id: 0x0781,
            product_id: 0x5583,
            device_class: 0,
            device_subclass: 0,
            device_protocol: 0,
            bcd_usb: bcd.map(String::from),
            serial: None,
            manufacturer: None,
            product: None,
            maxchild: 0,
            removable: true,
            speed_mbps: speed,
        }
    }

    #[test]
    fn negotiated_sysfs_speed_is_authoritative() {
        let (current, max) = speeds_for(&raw(Some(480), Some(" 3.10")));
        assert_eq!(current, UsbSpeed::HighSpeed);
        // bcdUSB floor lifts capability above the negotiated link…
        assert_eq!(max, UsbSpeed::SuperSpeedPlus10);
    }

    #[test]
    fn matching_speeds_yield_no_floor() {
        let (current, max) = speeds_for(&raw(Some(5000), Some(" 3.10")));
        assert_eq!(current, UsbSpeed::SuperSpeed);
        assert_eq!(max, UsbSpeed::SuperSpeedPlus10);
    }

    #[test]
    fn unknown_negotiated_stays_unknown() {
        let (current, _) = speeds_for(&raw(None, Some(" 2.00")));
        assert_eq!(current, UsbSpeed::Unknown);
    }
}
