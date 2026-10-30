//! Injectable clock so engine tests are deterministic.

use std::sync::{Mutex, MutexGuard};

pub trait Clock: Send + Sync {
    fn now_unix_ms(&self) -> u64;
}

/// Wall clock for production use.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_unix_ms(&self) -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }
}

/// Manually advanced clock for tests.
#[derive(Debug, Default)]
pub struct FakeClock {
    now_ms: Mutex<u64>,
}

impl FakeClock {
    pub fn new(start_ms: u64) -> Self {
        Self {
            now_ms: Mutex::new(start_ms),
        }
    }

    pub fn advance_ms(&self, ms: u64) {
        let mut now = self.lock();
        *now += ms;
    }

    fn lock(&self) -> MutexGuard<'_, u64> {
        self.now_ms.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl Clock for FakeClock {
    fn now_unix_ms(&self) -> u64 {
        *self.lock()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fake_clock_advances() {
        let clock = FakeClock::new(1_000);
        assert_eq!(clock.now_unix_ms(), 1_000);
        clock.advance_ms(500);
        assert_eq!(clock.now_unix_ms(), 1_500);
    }
}
