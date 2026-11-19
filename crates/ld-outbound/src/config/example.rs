//! The annotated reference config. Never written: no file means
//! defaults. Authored, not generated, for the comments;
//! `the_example_states_the_defaults_it_claims_to` keeps the values honest.

pub(crate) const EXAMPLE: &str = include_str!("example.toml");

mod tests {
    use ld_domain::config::{Config, Diagnostic, Origin};

    use super::*;

    fn load(source: &str) -> (Config, Vec<Diagnostic>) {
        let origin = Origin::new("example.toml");
        let (layer, mut diagnostics) = crate::config::parse(source, &origin);
        let layer = layer.expect("the example is a valid TOML file");
        let (merged, more) = crate::config::merge::merge(vec![layer]);
        diagnostics.extend(more);
        let (config, more) = crate::config::validate::validate(merged);
        diagnostics.extend(more);

        (config, diagnostics)
    }

    /// Tells settings from prose and continued comments.
    fn is_setting(text: &str) -> bool {
        text.starts_with('[')
            || text.split_once('=').is_some_and(|(key, _)| {
                let key = key.trim();
                !key.is_empty() && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            })
    }

    /// Every setting uncommented, prose left alone.
    fn uncommented(source: &str) -> String {
        source
            .lines()
            .map(|line| match line.trim_start().strip_prefix("# ") {
                Some(rest) if is_setting(rest) => rest.to_owned(),
                _ => line.to_owned(),
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn the_example_states_the_defaults_it_claims_to() {
        let (config, diagnostics) = load(EXAMPLE);

        assert_eq!(
            diagnostics,
            Vec::new(),
            "a copy of the example must not report anything on the run that reads it"
        );
        assert_eq!(
            config,
            Config::default(),
            "every line is commented, so the example must contribute nothing"
        );
    }

    #[test]
    fn uncommenting_the_whole_example_yields_a_valid_config() {
        let (config, diagnostics) = load(&uncommented(EXAMPLE));

        assert_eq!(
            diagnostics,
            Vec::new(),
            "a key spelt wrong, a value out of range or a section referring to one that is not \
             there would surface here"
        );
        assert_ne!(
            config,
            Config::default(),
            "if this matches the default the uncommenter stopped working and the test above it \
             is the only one still doing anything"
        );
        let engines: Vec<_> = config
            .connections
            .iter()
            .map(ld_domain::config::ConnectionConfig::engine)
            .collect();
        assert_eq!(
            engines,
            [
                ld_domain::Engine::Postgres,
                ld_domain::Engine::Postgres,
                ld_domain::Engine::Cassandra,
                ld_domain::Engine::Cassandra,
            ],
            "both Postgres shapes are shown, each a whole entry since `url` and the keys it \
             carries cannot share one, and both Cassandra ones"
        );
        assert_eq!(config.themes.len(), 1);
        assert!(
            config.default_connection.is_some(),
            "`default_connection` names the connection the example declares, which is what lets \
             this test assert on zero diagnostics"
        );

        let default = Config::default();
        let mismatch = "example.toml claims this is the default but the type disagrees — \
                         update the comment in example.toml to match";
        assert_eq!(config.theme, default.theme, "{mismatch}");
        assert_eq!(config.background, default.background, "{mismatch}");
        assert_eq!(config.icons, default.icons, "{mismatch}");
        assert_eq!(config.mouse, default.mouse, "{mismatch}");
        assert_eq!(config.layout, default.layout, "{mismatch}");
        assert_eq!(config.grid, default.grid, "{mismatch}");
        assert_eq!(config.history, default.history, "{mismatch}");
        assert_eq!(
            ld_domain::config::DriverConfig {
                request_timeout: None,
                ..config.driver
            },
            default.driver,
            "{mismatch}"
        );
    }

    /// By hand: `serde` cannot enumerate fields. An undocumented new key fails
    /// here.
    const KEYS: &[&str] = &[
        "theme",
        "background",
        "icons",
        "mouse",
        "default_connection",
        "[layout]",
        "sidebar",
        "ddl",
        "queries",
        "[grid]",
        "max_column_width",
        "page_rows",
        "[history]",
        "limit",
        "[log]",
        "catalog",
        "[driver]",
        "page_size",
        "request_timeout_ms",
        "connect_timeout_ms",
        "[export]",
        "path",
        "prefix",
        "clipboard_mb",
        "[[connections]]",
        "name",
        "url",
        "host",
        "port",
        "database",
        "user",
        "password",
        "{ env = ",
        "{ file = ",
        "posture",
        "sslmode",
        "sslrootcert",
        "sslcert",
        "sslkey",
        "[[themes]]",
        "extends",
        "[themes.colors]",
    ];

    #[test]
    fn every_key_the_schema_accepts_is_in_the_example() {
        for key in KEYS {
            assert!(
                EXAMPLE.contains(key),
                "`{key}` is accepted by wire.rs and undocumented in the example"
            );
        }
    }

    /// From the example's own list: `table` and `comment` are ordinary words.
    fn documented_roles(source: &str) -> Vec<&str> {
        let list = source.split_once("The roles:").map_or("", |(_, rest)| rest);
        let list = list.split_once('.').map_or(list, |(before, _)| before);

        list.split(',')
            .map(|role| role.trim_matches(|c: char| c.is_whitespace() || c == '#'))
            .collect()
    }

    #[test]
    fn every_theme_and_role_is_named_in_the_example() {
        for built_in in ld_domain::theme::BuiltIn::ALL {
            assert!(
                EXAMPLE.contains(built_in.slug()),
                "the built-in theme `{}` is missing from the `theme` comment",
                built_in.slug()
            );
        }

        let expected: Vec<&str> = ld_domain::theme::Role::ALL
            .iter()
            .map(|role| role.key())
            .collect();

        assert_eq!(
            documented_roles(EXAMPLE),
            expected,
            "the roles list must name every role, in the order `Role::ALL` declares them"
        );
    }
}
