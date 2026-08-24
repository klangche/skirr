//! Linux hotplug via polling-diff (see hotplug.rs for the design notes).

use crate::hotplug::{diff_snapshots, Fingerprint};
use skirr_core::{BackendError, BackendResult, DiagnosticEvent, HotplugBackend};
use std::collections::HashSet;
use std::time::Duration;

pub struct PollMonitor {
    active: bool,
    last: Vec<Fingerprint>,
}

impl PollMonitor {
    pub fn new() -> Self {
        Self {
            active: false,
            last: Vec::new(),
        }
    }

    fn snapshot() -> BackendResult<Vec<Fingerprint>> {
        #[cfg(target_os = "linux")]
        {
            Ok(crate::native::enumerate()?
                .iter()
                .map(Fingerprint::from_raw)
                .collect())
        }
        #[cfg(not(target_os = "linux"))]
        {
            Err(BackendError::unsupported(
                "skirr-linux",
                "hotplug requires Linux",
            ))
        }
    }
}

impl HotplugBackend for PollMonitor {
    fn is_active(&self) -> bool {
        self.active
    }

    fn start_monitoring(&mut self) -> BackendResult<()> {
        if self.active {
            return Err(BackendError::os_api(
                "skirr-linux",
                "monitor already started",
            ));
        }
        self.last = Self::snapshot()?;
        self.active = true;
        Ok(())
    }

    /// Wait up to `timeout`, then diff the live sysfs state against the
    /// previous snapshot. Transient sysfs errors are tolerated (empty poll).
    fn poll_event(&mut self, timeout: Duration) -> BackendResult<Option<DiagnosticEvent>> {
        std::thread::sleep(timeout);
        if !self.active {
            return Err(BackendError::NotMonitoring {
                backend: "skirr-linux",
            });
        }

        let current = match Self::snapshot() {
            Ok(snap) => snap,
            Err(_) => return Ok(None),
        };

        let seen: HashSet<&Fingerprint> = current.iter().collect();
        let mut events = diff_snapshots(&self.last.clone(), &current, chrono::Utc::now());
        let _ = seen;
        self.last = current;
        Ok(if events.is_empty() {
            None
        } else {
            Some(events.remove(0))
        })
    }

    fn stop_monitoring(&mut self) -> BackendResult<()> {
        self.active = false;
        Ok(())
    }
}

impl Default for PollMonitor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unarmed_poll_is_not_monitoring() {
        let mut m = PollMonitor::new();
        assert!(matches!(
            m.poll_event(Duration::from_millis(1)),
            Err(BackendError::NotMonitoring { .. })
        ));
    }
}
