//! The wall-clock as the domain may know it: a [`Stamp`] handed in through
//! [`Msg::Tick`](crate::Msg::Tick) or [`init`](crate::init), never read
//! (architecture.md §2).

use std::fmt;

const MINUTE: u64 = 60;
const HOUR: u64 = 60 * MINUTE;
const DAY: u64 = 24 * HOUR;
const WEEK: u64 = 7 * DAY;
const YEAR: u64 = 365 * DAY;

/// Seconds since the Unix epoch, UTC. `Copy`, so refreshing it 25 times a
/// second allocates nothing.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Stamp(u64);

impl Stamp {
    /// That time of day on the epoch's first day.
    #[must_use]
    pub const fn new(hour: u8, minute: u8, second: u8) -> Self {
        Self(hour as u64 * HOUR + minute as u64 * MINUTE + second as u64)
    }

    #[must_use]
    pub const fn from_unix(seconds: u64) -> Self {
        Self(seconds)
    }

    #[must_use]
    pub const fn unix(self) -> u64 {
        self.0
    }

    /// The largest whole unit: `42s`, `5m`, `3h`, `2d`, `6w`, `1y`. A stamp
    /// after `now`, from a clock set back, reads as `0s`.
    #[must_use]
    pub fn age(self, now: Self) -> String {
        let elapsed = now.0.saturating_sub(self.0);

        let (unit, suffix) = [
            (YEAR, 'y'),
            (WEEK, 'w'),
            (DAY, 'd'),
            (HOUR, 'h'),
            (MINUTE, 'm'),
        ]
        .into_iter()
        .find(|(unit, _)| elapsed >= *unit)
        .unwrap_or((1, 's'));

        format!("{}{suffix}", elapsed / unit)
    }
}

impl fmt::Display for Stamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let day = self.0 % DAY;

        write!(
            f,
            "{:02}:{:02}:{:02}",
            day / HOUR,
            (day % HOUR) / MINUTE,
            day % MINUTE
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stamp_reads_as_a_wall_clock_time() {
        assert_eq!(Stamp::new(14, 2, 9).to_string(), "14:02:09");
    }

    #[test]
    fn a_stamp_pads_every_field_to_two_digits() {
        assert_eq!(Stamp::new(0, 0, 0).to_string(), "00:00:00");
        assert_eq!(Stamp::new(23, 59, 59).to_string(), "23:59:59");
    }

    #[test]
    fn the_default_stamp_is_midnight() {
        assert_eq!(Stamp::default(), Stamp::new(0, 0, 0));
    }

    #[test]
    fn a_unix_stamp_reads_as_its_utc_time_of_day() {
        // 2025-11-26T10:30:15Z
        assert_eq!(Stamp::from_unix(1_764_153_015).to_string(), "10:30:15");
    }

    #[test]
    fn an_age_is_its_largest_whole_unit() {
        let then = Stamp::from_unix(1_000_000_000);
        let after = |seconds| Stamp::from_unix(1_000_000_000 + seconds);

        assert_eq!(then.age(after(0)), "0s");
        assert_eq!(then.age(after(59)), "59s");
        assert_eq!(then.age(after(60)), "1m");
        assert_eq!(then.age(after(3 * HOUR + 59 * MINUTE)), "3h");
        assert_eq!(then.age(after(2 * DAY)), "2d");
        assert_eq!(then.age(after(13 * DAY)), "1w");
        assert_eq!(then.age(after(400 * DAY)), "1y");
    }

    #[test]
    fn a_stamp_from_the_future_is_no_age() {
        assert_eq!(Stamp::from_unix(100).age(Stamp::from_unix(50)), "0s");
    }
}
