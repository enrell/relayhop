//! Sealed implementation details.

use std::{
    cmp,
    sync::LazyLock,
    time::{Duration, Instant, SystemTime},
};

static MAX_SYSTEM_TIME: LazyLock<SystemTime> = LazyLock::new(find_max);
static MIN_SYSTEM_TIME: LazyLock<SystemTime> = LazyLock::new(find_min);
static MAX_INSTANT: LazyLock<Instant> = LazyLock::new(find_max);
static MIN_INSTANT: LazyLock<Instant> = LazyLock::new(find_min);

pub trait SaturatingTime: Sized + Copy + PartialEq {
    fn anchor() -> Self;
    fn max_value() -> Self;
    fn min_value() -> Self;
    fn checked_add(&self, duration: Duration) -> Option<Self>;
    fn checked_sub(&self, duration: Duration) -> Option<Self>;
    fn checked_duration_since(&self, earlier: Self) -> Option<Duration>;
}

impl SaturatingTime for SystemTime {
    fn anchor() -> Self {
        Self::UNIX_EPOCH
    }

    fn max_value() -> Self {
        *MAX_SYSTEM_TIME
    }

    fn min_value() -> Self {
        *MIN_SYSTEM_TIME
    }

    fn checked_add(&self, duration: Duration) -> Option<Self> {
        Self::checked_add(self, duration)
    }

    fn checked_sub(&self, duration: Duration) -> Option<Self> {
        Self::checked_sub(self, duration)
    }

    fn checked_duration_since(&self, earlier: Self) -> Option<Duration> {
        Self::duration_since(self, earlier).ok()
    }
}

impl SaturatingTime for Instant {
    fn anchor() -> Self {
        Self::now()
    }

    fn max_value() -> Self {
        *MAX_INSTANT
    }

    fn min_value() -> Self {
        *MIN_INSTANT
    }

    fn checked_add(&self, duration: Duration) -> Option<Self> {
        Self::checked_add(self, duration)
    }

    fn checked_sub(&self, duration: Duration) -> Option<Self> {
        Self::checked_sub(self, duration)
    }

    fn checked_duration_since(&self, _earlier: Self) -> Option<Duration> {
        unreachable!("Instant uses its standard-library saturating method")
    }
}

fn find_max<T: SaturatingTime>() -> T {
    find_limit(T::checked_add)
}

fn find_min<T: SaturatingTime>() -> T {
    find_limit(T::checked_sub)
}

fn find_limit<T, F>(f: F) -> T
where
    T: SaturatingTime,
    F: Fn(&T, Duration) -> Option<T>,
{
    const INITIAL_STEP: Duration = Duration::new(1_000_000_000_000_000_000, 0);
    const ONE_NS: Duration = Duration::new(0, 1);

    let mut step = INITIAL_STEP;
    let mut result = T::anchor();

    loop {
        match f(&result, step) {
            Some(next) if next != result => result = next,
            // Windows SystemTime is backed by 100 ns FILETIME ticks. A
            // sub-tick operation can therefore succeed without changing the
            // value; treat that exactly like reaching the platform limit.
            Some(_) | None if step == ONE_NS => return result,
            Some(_) | None => step = cmp::max(ONE_NS, step / 2),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    struct CoarseTime(u64);

    impl SaturatingTime for CoarseTime {
        fn anchor() -> Self {
            Self(0)
        }

        fn max_value() -> Self {
            find_max::<Self>()
        }

        fn min_value() -> Self {
            Self(0)
        }

        fn checked_add(&self, duration: Duration) -> Option<Self> {
            let ticks = u64::try_from(duration.as_nanos() / 100).ok()?;
            self.0
                .checked_add(ticks)
                .filter(|value| *value <= 5)
                .map(Self)
        }

        fn checked_sub(&self, duration: Duration) -> Option<Self> {
            let ticks = u64::try_from(duration.as_nanos() / 100).ok()?;
            self.0.checked_sub(ticks).map(Self)
        }

        fn checked_duration_since(&self, earlier: Self) -> Option<Duration> {
            self.0
                .checked_sub(earlier.0)
                .map(|ticks| Duration::from_nanos(ticks * 100))
        }
    }

    #[test]
    fn limit_search_terminates_when_small_steps_make_no_progress() {
        assert_eq!(find_max::<CoarseTime>(), CoarseTime(5));
    }

    #[test]
    fn system_time_limits_terminate_on_this_platform() {
        let max = <SystemTime as SaturatingTime>::max_value();
        let min = <SystemTime as SaturatingTime>::min_value();
        assert!(max >= SystemTime::UNIX_EPOCH);
        assert!(min <= SystemTime::UNIX_EPOCH);
    }
}
