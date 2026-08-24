//! Normalization traits - the contract every platform backend implements.
//!
//! All methods are synchronous; hotplug events are delivered through a
//! bounded-poll interface backed by a channel inside each backend, so
//! `skirr-core` stays free of any async runtime dependency.
//!
//! See docs/DATA_MAP.md §11: backends must degrade gracefully - return what
//! is available and surface gaps as `Err(BackendError::Unsupported)` or
//! `Unknown` model values, never fabricate data.

use crate::model::{
    DiagnosticEvent, DisplayInfo, PlatformInfo, PowerInfo, SpeedBottleneck, SystemTopology,
    ThunderboltInfo, Usb4Info, UsbCInfo, UsbDevice, UsbSpeed,
};
use std::time::Duration;
use uuid::Uuid;

/// Errors every backend may return.
#[derive(Debug, thiserror::Error)]
pub enum BackendError {
    /// Datapoint not exposed by this platform (report as Unknown upstream).
    #[error("{backend}: not exposed by platform ({reason})")]
    Unsupported {
        backend: &'static str,
        reason: String,
    },
    /// Operation requires elevation (see DATA_MAP.md §10 admin matrix).
    #[error("{backend}: permission denied, elevation required ({reason})")]
    PermissionDenied {
        backend: &'static str,
        reason: String,
    },
    /// Underlying OS API returned an error.
    #[error("{backend}: os api error ({reason})")]
    OsApi {
        backend: &'static str,
        reason: String,
    },
    /// Monitoring session was not started.
    #[error("{backend}: monitoring not started")]
    NotMonitoring { backend: &'static str },
}

impl BackendError {
    pub fn unsupported(backend: &'static str, reason: impl Into<String>) -> Self {
        Self::Unsupported {
            backend,
            reason: reason.into(),
        }
    }

    pub fn permission_denied(backend: &'static str, reason: impl Into<String>) -> Self {
        Self::PermissionDenied {
            backend,
            reason: reason.into(),
        }
    }

    pub fn os_api(backend: &'static str, reason: impl Into<String>) -> Self {
        Self::OsApi {
            backend,
            reason: reason.into(),
        }
    }
}

/// Convenience alias for backend results.
pub type BackendResult<T> = Result<T, BackendError>;

/// Max/current speed pair for one device, with bottleneck precomputed.
#[derive(Debug, Clone)]
pub struct SpeedReport {
    pub device_id: Uuid,
    pub max_supported: UsbSpeed,
    pub current_link: UsbSpeed,
    pub bottleneck: Option<SpeedBottleneck>,
}

/// USB enumeration + topology contract (Windows/macOS/Linux backends).
pub trait UsbBackend: Send + Sync {
    /// Backend identifier (e.g. "skirr-macos").
    fn name(&self) -> &'static str;

    /// Host platform facts for reports.
    fn platform_info(&self) -> BackendResult<PlatformInfo>;

    /// Snapshot of all present USB devices (flat list; tree links included).
    fn enumerate_devices(&self) -> BackendResult<Vec<UsbDevice>>;

    /// Full topology snapshot: controllers, root hubs, devices, hubs.
    fn get_topology(&self) -> BackendResult<SystemTopology>;

    /// Single device lookup by id.
    fn get_device(&self, device_id: Uuid) -> BackendResult<UsbDevice> {
        self.enumerate_devices()?
            .into_iter()
            .find(|d| d.id == device_id)
            .ok_or_else(|| {
                BackendError::os_api(self.name(), format!("device {device_id} not found"))
            })
    }

    /// Max supported vs current negotiated speed for one device.
    fn get_speeds(&self, device_id: Uuid) -> BackendResult<SpeedReport>;

    /// Start a live-monitoring session for enumeration events.
    fn monitor(&self) -> BackendResult<Box<dyn HotplugBackend + '_>>;
}

/// Display enumeration + EDID contract.
pub trait DisplayBackend: Send + Sync {
    fn name(&self) -> &'static str;

    /// All online displays with parsed EDID metadata where available.
    fn enumerate_displays(&self) -> BackendResult<Vec<DisplayInfo>>;

    /// Raw EDID bytes for one display.
    fn get_edid(&self, display_id: Uuid) -> BackendResult<Vec<u8>>;
}

/// USB-C / Thunderbolt / USB4 / PD capability contract.
///
/// Per project philosophy: when the OS hides a datapoint, return
/// `Err(BackendError::Unsupported)` rather than guessing.
pub trait UsbCBackend: Send + Sync {
    fn name(&self) -> &'static str;

    /// USB-C connector capabilities (alt modes, orientation, roles).
    fn get_capabilities(&self, device_id: Uuid) -> BackendResult<UsbCInfo>;

    /// Power-Delivery info if exposed; `None` = known absent, Err = unknown.
    fn get_power(&self, device_id: Uuid) -> BackendResult<Option<PowerInfo>>;

    /// Thunderbolt details if the device sits on a TB path.
    fn get_thunderbolt(&self, device_id: Uuid) -> BackendResult<Option<ThunderboltInfo>>;

    /// USB4 router details if applicable.
    fn get_usb4(&self, device_id: Uuid) -> BackendResult<Option<Usb4Info>>;
}

/// Live hotplug monitoring session.
///
/// Lifecycle: `start_monitoring()` arms OS notifications; `poll_event()`
/// drains normalized [`DiagnosticEvent`]s; `stop_monitoring()` disarms.
pub trait HotplugBackend: Send {
    fn start_monitoring(&mut self) -> BackendResult<()>;

    /// Non-blocking-ish wait for the next event; `Ok(None)` on timeout.
    fn poll_event(&mut self, timeout: Duration) -> BackendResult<Option<DiagnosticEvent>>;

    fn stop_monitoring(&mut self) -> BackendResult<()>;

    /// Whether the session is currently armed.
    fn is_active(&self) -> bool;
}

#[cfg(test)]
pub(crate) mod stub {
    //! Minimal in-memory backend for tests and CLI bring-up (Phases < 2).
    //! Not compiled into release binaries' logic paths by consumers.

    use super::*;
    use crate::model::{
        ConnectionStatus, DiagnosticEvent, EventType, HostController, PlatformInfo, RootHub,
        SystemTopology, UsbDevice,
    };
    use std::collections::VecDeque;
    use std::sync::Mutex;

    pub struct StubBackend {
        devices: Mutex<Vec<UsbDevice>>,
        events: Mutex<VecDeque<DiagnosticEvent>>,
        active: Mutex<bool>,
    }

    impl StubBackend {
        pub fn new() -> Self {
            Self {
                devices: Mutex::new(Vec::new()),
                events: Mutex::new(VecDeque::new()),
                active: Mutex::new(false),
            }
        }

        pub fn with_device(self, device: UsbDevice) -> Self {
            self.devices.lock().unwrap().push(device);
            self
        }

        pub fn push_event(&self, event: DiagnosticEvent) {
            self.events.lock().unwrap().push_back(event);
        }

        fn platform_info_inner(&self) -> PlatformInfo {
            PlatformInfo {
                os: "stub-os".to_string(),
                os_version: "0.0".to_string(),
                kernel_version: None,
                architecture: std::env::consts::ARCH.to_string(),
                hostname: Some("stub".to_string()),
                username: None,
                is_admin: false,
                is_virtual_machine: false,
                boot_time: None,
            }
        }
    }

    impl Default for StubBackend {
        fn default() -> Self {
            Self::new()
        }
    }

    impl UsbBackend for StubBackend {
        fn name(&self) -> &'static str {
            "skirr-stub"
        }

        fn platform_info(&self) -> BackendResult<PlatformInfo> {
            Ok(self.platform_info_inner())
        }

        fn enumerate_devices(&self) -> BackendResult<Vec<UsbDevice>> {
            Ok(self.devices.lock().unwrap().clone())
        }

        fn get_topology(&self) -> BackendResult<SystemTopology> {
            let devices = self.enumerate_devices()?;
            Ok(SystemTopology {
                timestamp: chrono::Utc::now(),
                host_controllers: Vec::<HostController>::new(),
                root_hubs: Vec::<RootHub>::new(),
                devices: devices.clone(),
                hubs: devices.iter().filter(|d| d.is_hub).cloned().collect(),
                displays: Vec::new(),
                thunderbolt_routers: Vec::new(),
                events: Vec::new(),
                platform_info: self.platform_info_inner(),
            })
        }

        fn get_speeds(&self, device_id: Uuid) -> BackendResult<SpeedReport> {
            let device = self.get_device(device_id)?;
            Ok(SpeedReport {
                device_id,
                max_supported: device.max_supported_speed,
                current_link: device.current_link_speed,
                bottleneck: device.speed_bottleneck(),
            })
        }

        fn monitor(&self) -> BackendResult<Box<dyn HotplugBackend + '_>> {
            Ok(Box::new(StubHotplug {
                backend: self,
                active: false,
            }))
        }
    }

    pub struct StubHotplug<'a> {
        backend: &'a StubBackend,
        active: bool,
    }

    impl<'a> HotplugBackend for StubHotplug<'a> {
        fn start_monitoring(&mut self) -> BackendResult<()> {
            self.active = true;
            *self.backend.active.lock().unwrap() = true;
            Ok(())
        }

        fn poll_event(&mut self, timeout: Duration) -> BackendResult<Option<DiagnosticEvent>> {
            let _ = timeout;
            if !self.active {
                return Err(BackendError::NotMonitoring {
                    backend: "skirr-stub",
                });
            }
            Ok(self.backend.events.lock().unwrap().pop_front())
        }

        fn stop_monitoring(&mut self) -> BackendResult<()> {
            self.active = false;
            *self.backend.active.lock().unwrap() = false;
            Ok(())
        }

        fn is_active(&self) -> bool {
            self.active
        }
    }

    /// Build a synthetic connect event for tests.
    pub fn connect_event(details: impl Into<String>) -> DiagnosticEvent {
        DiagnosticEvent {
            id: Uuid::new_v4(),
            timestamp: chrono::Utc::now(),
            event_type: EventType::DeviceConnected,
            device_id: None,
            hub_id: None,
            port_number: None,
            details: details.into(),
            severity: crate::model::EventSeverity::Info,
            metadata: Default::default(),
        }
    }

    /// Device with a given connection status, for stub trees.
    pub fn device_with_status(status: ConnectionStatus) -> UsbDevice {
        let mut d = UsbDevice::new(0x1234, 0x5678);
        d.connection_status = status;
        d
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ConnectionStatus, EventSeverity};
    use std::time::Duration;

    #[test]
    fn traits_are_object_safe() {
        let backend: Box<dyn UsbBackend> = Box::new(stub::StubBackend::new());
        assert_eq!(backend.name(), "skirr-stub");
        let displays: Box<dyn DisplayBackend> = unimplemented_display();
        assert_eq!(displays.name(), "unused");
        let usbc: Option<Box<dyn UsbCBackend>> = None;
        assert!(usbc.is_none());
    }

    fn unimplemented_display() -> Box<dyn DisplayBackend> {
        struct X;
        impl DisplayBackend for X {
            fn name(&self) -> &'static str {
                "unused"
            }
            fn enumerate_displays(&self) -> BackendResult<Vec<DisplayInfo>> {
                Err(BackendError::unsupported("unused", "test only"))
            }
            fn get_edid(&self, _display_id: Uuid) -> BackendResult<Vec<u8>> {
                Err(BackendError::unsupported("unused", "test only"))
            }
        }
        Box::new(X)
    }

    #[test]
    fn stub_enumerate_and_topology_roundtrip() {
        let b = stub::StubBackend::new()
            .with_device(stub::device_with_status(ConnectionStatus::Connected));
        let devices = b.enumerate_devices().unwrap();
        assert_eq!(devices.len(), 1);
        let topo = b.get_topology().unwrap();
        assert_eq!(topo.devices.len(), 1);
        assert_eq!(topo.platform_info.os, "stub-os");

        let report = b.get_speeds(devices[0].id).unwrap();
        assert_eq!(report.max_supported, UsbSpeed::Unknown);
        assert!(report.bottleneck.is_none());
    }

    #[test]
    fn stub_get_device_unknown_id_errors() {
        let b = stub::StubBackend::new();
        let err = b.get_device(Uuid::new_v4()).unwrap_err();
        assert!(matches!(err, BackendError::OsApi { .. }));
        assert!(err.to_string().contains("not found"));
    }

    #[test]
    fn hotplug_lifecycle_and_events() {
        let b = stub::StubBackend::new();
        let mut session = b.monitor().unwrap();
        // Poll before start must fail.
        assert!(matches!(
            session.poll_event(Duration::ZERO),
            Err(BackendError::NotMonitoring { .. })
        ));

        session.start_monitoring().unwrap();
        assert!(session.is_active());
        assert!(session.poll_event(Duration::ZERO).unwrap().is_none());

        b.push_event(stub::connect_event("keyboard plugged"));
        let ev = session.poll_event(Duration::from_millis(10)).unwrap();
        assert_eq!(ev.unwrap().details, "keyboard plugged");

        session.stop_monitoring().unwrap();
        assert!(!session.is_active());
    }

    #[test]
    fn error_constructors_carry_context() {
        let e = BackendError::unsupported("skirr-windows", "empty ports hidden");
        assert_eq!(
            e.to_string(),
            "skirr-windows: not exposed by platform (empty ports hidden)"
        );
        let e = BackendError::permission_denied("skirr-macos", "tb security");
        assert_eq!(
            e.to_string(),
            "skirr-macos: permission denied, elevation required (tb security)"
        );
        assert_eq!(EventSeverity::Info, EventSeverity::Info);
    }
}
