//! `buffers.toml`: the query editor's text per connection. Written and locked
//! by [`FileStore`](crate::history::FileStore).

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

use ld_domain::buffers::Buffers;
use ld_domain::config::{ConnectionName, Diagnostic};
use serde::Deserialize;

use crate::config::save::quote;
use crate::history::warning;

pub(crate) const FILE: &str = "buffers.toml";

const HEADER: &str = "# LazyDB query buffers, one per connection. Rewritten as you type.\n";

#[derive(Debug, Default, Deserialize)]
struct File {
    #[serde(default)]
    connections: BTreeMap<String, Section>,
}

#[derive(Debug, Deserialize)]
struct Section {
    text: String,
}

pub(crate) type Sections = BTreeMap<String, String>;

pub(crate) fn parse(source: &str) -> Result<Sections, String> {
    let file: File = toml::from_str(source).map_err(|error| error.message().to_owned())?;

    Ok(file
        .connections
        .into_iter()
        .map(|(name, section)| (name, section.text))
        .collect())
}

pub(crate) fn render(sections: &Sections) -> String {
    let mut out = String::from(HEADER);

    for (name, text) in sections.iter().filter(|(_, text)| !text.is_empty()) {
        // Writing into a String cannot fail.
        let _written = write!(
            out,
            "\n[connections.{}]\ntext = {}\n",
            quote(name),
            block(text)
        );
    }

    out
}

/// A multi-line basic string, so the file reads as the editor did. Every
/// quote is escaped, so no run of three can close it early.
fn block(text: &str) -> String {
    let mut out = String::from("\"\"\"\n");

    for character in text.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' | '\t' => out.push(character),
            control if control.is_control() => {
                // Writing into a String cannot fail.
                let _written = write!(out, "\\u{:04X}", control as u32);
            }
            other => out.push(other),
        }
    }

    out.push_str("\"\"\"");
    out
}

/// A connection name the domain refuses is reported and skipped.
pub(crate) fn restore(sections: Sections, path: &Path) -> (Buffers, Vec<Diagnostic>) {
    let mut buffers = Buffers::default();
    let mut diagnostics = Vec::new();

    for (name, text) in sections {
        match ConnectionName::try_new(&name) {
            Ok(connection) => buffers.restore(&connection, text),
            Err(error) => diagnostics.push(warning(
                path,
                format!("query buffer for `{name}` ignored: {error}"),
            )),
        }
    }

    (buffers, diagnostics)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sections(of: &[(&str, &str)]) -> Sections {
        of.iter()
            .map(|(name, text)| ((*name).to_owned(), (*text).to_owned()))
            .collect()
    }

    #[test]
    fn what_is_rendered_parses_back() {
        let held = sections(&[
            ("dev", "SELECT \"a\"\"\"\nFROM t\\x;\n\n\t-- é\r\u{7}"),
            ("prod", "SELECT 1;"),
            ("trailing", "SELECT 1;\n"),
        ]);

        assert_eq!(parse(&render(&held)), Ok(held));
    }

    #[test]
    fn a_buffer_reads_as_its_lines() {
        let rendered = render(&sections(&[("dev", "SELECT 1;\nSELECT 2;")]));

        assert!(
            rendered.contains("[connections.\"dev\"]\ntext = \"\"\"\nSELECT 1;\nSELECT 2;\"\"\"\n"),
            "{rendered}"
        );
    }

    #[test]
    fn an_empty_buffer_is_left_out() {
        let rendered = render(&sections(&[("dev", ""), ("prod", "SELECT 1;")]));

        assert!(!rendered.contains("dev"));
        assert!(rendered.contains("prod"));
    }

    #[test]
    fn a_refused_name_is_reported_and_the_rest_kept() {
        let held = sections(&[("  ", "SELECT 1;"), ("dev", "SELECT 2;")]);

        let (buffers, diagnostics) = restore(held, Path::new("/s/buffers.toml"));

        let dev = ConnectionName::try_new("dev").expect("a valid name");
        assert_eq!(buffers.of(&dev), "SELECT 2;");
        assert_eq!(diagnostics.len(), 1);
    }
}
