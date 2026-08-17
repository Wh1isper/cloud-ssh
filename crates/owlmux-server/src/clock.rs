use std::{fmt, sync::Mutex, time::Duration};

use nix::time::{ClockId, clock_gettime};

pub trait BootTimeSource: Send + Sync + 'static {
    /// Read elapsed boot time.
    ///
    /// # Errors
    ///
    /// Returns [`ClockError::Unavailable`] when the platform clock cannot be read.
    fn read(&self) -> Result<Duration, ClockError>;
}

#[derive(Clone, Copy, Default)]
pub struct LinuxBootTime;

impl BootTimeSource for LinuxBootTime {
    fn read(&self) -> Result<Duration, ClockError> {
        let value = clock_gettime(ClockId::CLOCK_BOOTTIME).map_err(|_| ClockError::Unavailable)?;
        let seconds = u64::try_from(value.tv_sec()).map_err(|_| ClockError::Unavailable)?;
        let nanoseconds = u32::try_from(value.tv_nsec()).map_err(|_| ClockError::Unavailable)?;
        Ok(Duration::new(seconds, nanoseconds))
    }
}

pub struct BootClock<S = LinuxBootTime> {
    source: S,
    last: Mutex<Option<Duration>>,
}

impl Default for BootClock<LinuxBootTime> {
    fn default() -> Self {
        Self::new(LinuxBootTime)
    }
}

impl<S: BootTimeSource> BootClock<S> {
    pub const fn new(source: S) -> Self {
        Self {
            source,
            last: Mutex::new(None),
        }
    }

    /// Read and validate the Linux boot clock.
    ///
    /// # Errors
    ///
    /// Returns an error when the clock is unavailable or moves backwards.
    pub fn now(&self) -> Result<Duration, ClockError> {
        let current = self.source.read()?;
        let mut last = self.last.lock().map_err(|_| ClockError::Unavailable)?;
        if last.is_some_and(|previous| current < previous) {
            return Err(ClockError::MovedBackwards);
        }
        *last = Some(current);
        Ok(current)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClockError {
    Unavailable,
    MovedBackwards,
}

impl fmt::Display for ClockError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable => formatter.write_str("CLOCK_BOOTTIME is unavailable"),
            Self::MovedBackwards => formatter.write_str("CLOCK_BOOTTIME moved backwards"),
        }
    }
}

impl std::error::Error for ClockError {}

#[cfg(test)]
mod tests {
    use std::{collections::VecDeque, sync::Mutex};

    use super::*;

    struct FakeClock {
        values: Mutex<VecDeque<Result<Duration, ClockError>>>,
    }

    impl BootTimeSource for FakeClock {
        fn read(&self) -> Result<Duration, ClockError> {
            self.values
                .lock()
                .expect("fake clock lock")
                .pop_front()
                .expect("fake clock value")
        }
    }

    fn fake(values: Vec<Result<Duration, ClockError>>) -> BootClock<FakeClock> {
        BootClock::new(FakeClock {
            values: Mutex::new(values.into()),
        })
    }

    #[test]
    fn reads_linux_boottime() {
        assert!(BootClock::default().now().expect("clock read") > Duration::ZERO);
    }

    #[test]
    fn propagates_read_failure() {
        let clock = fake(vec![Err(ClockError::Unavailable)]);
        assert_eq!(clock.now(), Err(ClockError::Unavailable));
    }

    #[test]
    fn rejects_backward_movement() {
        let clock = fake(vec![
            Ok(Duration::from_secs(20)),
            Ok(Duration::from_secs(19)),
        ]);
        assert_eq!(clock.now(), Ok(Duration::from_secs(20)));
        assert_eq!(clock.now(), Err(ClockError::MovedBackwards));
    }
}
