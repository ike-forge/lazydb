//! The one place the wall-clock is read; the domain is handed [`Stamp`]s.

use std::time::{SystemTime, UNIX_EPOCH};

use ld_domain::clock::Stamp;

/// A clock before the epoch reads as the epoch.
#[must_use]
pub fn now() -> Stamp {
    Stamp::from_unix(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.as_secs()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    // Asserting the value would read the clock twice and race.
    #[test]
    fn the_clock_produces_a_time_of_day() {
        let stamp = now().to_string();

        assert_eq!(stamp.len(), 8);
        assert_eq!(stamp.as_bytes()[2], b':');
        assert_eq!(stamp.as_bytes()[5], b':');
    }

    #[test]
    fn the_clock_is_past_the_epoch() {
        assert!(now().unix() > 1_700_000_000);
    }
}
