//! The query buffer each connection was left with, keyed by connection name
//! as the history is. Text only: the cursor and results are not kept.

use std::collections::BTreeMap;

use crate::clock::Stamp;
use crate::config::ConnectionName;

/// Seconds between two looks at the editor while it stays on one connection.
const EVERY: u64 = 2;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Buffers {
    /// An empty buffer is absent.
    kept: BTreeMap<ConnectionName, String>,
    /// The connection last chosen, connected or not: the editor is its buffer.
    owner: Option<ConnectionName>,
    looked: Stamp,
}

impl Buffers {
    pub fn restore(&mut self, connection: &ConnectionName, text: String) {
        if !text.is_empty() {
            self.kept.insert(connection.clone(), text);
        }
    }

    #[must_use]
    pub fn of(&self, connection: &ConnectionName) -> &str {
        self.kept.get(connection).map_or("", String::as_str)
    }

    #[must_use]
    pub const fn owner(&self) -> Option<&ConnectionName> {
        self.owner.as_ref()
    }

    pub fn own(&mut self, connection: ConnectionName) {
        self.owner = Some(connection);
    }

    /// `false` when `text` is what was already kept: nothing to write.
    pub fn keep(&mut self, connection: &ConnectionName, text: String) -> bool {
        if self.of(connection) == text {
            return false;
        }

        if text.is_empty() {
            self.kept.remove(connection);
        } else {
            self.kept.insert(connection.clone(), text);
        }
        true
    }

    /// Whether `now` is [`EVERY`] seconds past the last look; marks a look.
    pub fn due(&mut self, now: Stamp) -> bool {
        if now.unix() < self.looked.unix().saturating_add(EVERY) {
            return false;
        }

        self.looked = now;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dev() -> ConnectionName {
        ConnectionName::try_new("dev").expect("a valid name")
    }

    #[test]
    fn keeping_the_same_text_twice_writes_once() {
        let mut buffers = Buffers::default();

        assert!(buffers.keep(&dev(), "SELECT 1;".to_owned()));
        assert!(!buffers.keep(&dev(), "SELECT 1;".to_owned()));
        assert_eq!(buffers.of(&dev()), "SELECT 1;");
    }

    #[test]
    fn an_emptied_buffer_is_forgotten_and_written() {
        let mut buffers = Buffers::default();
        buffers.restore(&dev(), "SELECT 1;".to_owned());

        assert!(buffers.keep(&dev(), String::new()));
        assert_eq!(buffers, Buffers::default());
    }

    #[test]
    fn a_look_is_due_every_two_seconds() {
        let mut buffers = Buffers::default();

        assert!(buffers.due(Stamp::from_unix(10)));
        assert!(!buffers.due(Stamp::from_unix(11)));
        assert!(buffers.due(Stamp::from_unix(12)));
    }
}
