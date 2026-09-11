//! Saturating arithmetic for [`SystemTime`] and [`Instant`].
//!
//! Vendored from saturating-time 0.4.0 with a Windows coarse-clock fix in
//! `internal::find_limit`.

#![forbid(unsafe_code)]

use std::time::{Duration, Instant, SystemTime};

mod internal;

/// Saturating arithmetic for standard-library time values.
pub trait SaturatingTime: internal::SaturatingTime {
    /// Return the greatest representable value on this platform.
    fn max_value() -> Self {
        internal::SaturatingTime::max_value()
    }

    /// Return the least representable value on this platform.
    fn min_value() -> Self {
        internal::SaturatingTime::min_value()
    }

    /// Add a duration, saturating at the greatest representable value.
    fn saturating_add(self, duration: Duration) -> Self {
        self.checked_add(duration)
            .unwrap_or(SaturatingTime::max_value())
    }

    /// Subtract a duration, saturating at the least representable value.
    fn saturating_sub(self, duration: Duration) -> Self {
        self.checked_sub(duration)
            .unwrap_or(SaturatingTime::min_value())
    }

    /// Calculate a duration, saturating at zero when `earlier` is later.
    fn saturating_duration_since(&self, earlier: Self) -> Duration {
        self.checked_duration_since(earlier)
            .unwrap_or(Duration::ZERO)
    }
}

impl SaturatingTime for SystemTime {}

impl SaturatingTime for Instant {
    fn saturating_duration_since(&self, earlier: Self) -> Duration {
        Self::saturating_duration_since(self, earlier)
    }
}
