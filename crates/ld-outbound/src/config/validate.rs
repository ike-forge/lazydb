//! The merged layer into the total config type. The smallest unit is dropped: a
//! bad entry loses the entry, a bad scalar loses the key.

use std::collections::BTreeMap;
use std::path::PathBuf;

use ld_domain::Engine;
use ld_domain::config::{
    Background, CassandraTarget, CatalogQueries, ClientIdentity, ClipboardMb, Config,
    ConnectionConfig, ConnectionName, Consistency, Credential, Credentials, CustomTheme,
    Diagnostic, Discovery, DriverConfig, ExportConfig, GridConfig, HistoryConfig, HistoryLimit,
    Icons, LogConfig, MaxColumnWidth, Millis, Mouse, Origin, OutOfRange, PageRows, PageSize,
    PanelRatios, PostgresTarget, Rgb, Span, SslMode, Target, Tls, TlsSettings, Trust, Verification,
};
use ld_domain::posture::Posture;
use ld_domain::style::Severity;
use ld_domain::theme::{BuiltIn, Ours, Role, ThemeChoice, ThemeName};

use super::url::{self, Url};
use super::wire::{
    ConnectionLayer, CredentialLayer, CredentialSource, DriverLayer, ExportLayer, GridLayer,
    HistoryLayer, Layer, LayoutLayer, Located, ThemeLayer, TlsLayer, TlsTable,
};

/// Each diagnostic takes its origin from the value that failed, so it names the
/// file the key was written in.
struct Report {
    fallback: Origin,
    diagnostics: Vec<Diagnostic>,
}

impl Report {
    fn at<V>(&self, located: &Located<V>) -> (Origin, Option<Span>) {
        (
            located
                .origin()
                .cloned()
                .unwrap_or_else(|| self.fallback.clone()),
            located.span(),
        )
    }

    fn error<V>(&mut self, located: &Located<V>, message: String) {
        let (origin, span) = self.at(located);

        self.diagnostics.push(Diagnostic {
            origin,
            span,
            severity: Severity::Danger,
            message,
        });
    }

    fn error_unlocated(&mut self, message: String) {
        self.diagnostics.push(Diagnostic {
            origin: self.fallback.clone(),
            span: None,
            severity: Severity::Danger,
            message,
        });
    }

    fn error_at(&mut self, at: Option<(Origin, Span)>, message: String) {
        let (origin, span) = at.map_or_else(
            || (self.fallback.clone(), None),
            |(origin, span)| (origin, Some(span)),
        );

        self.diagnostics.push(Diagnostic {
            origin,
            span,
            severity: Severity::Danger,
            message,
        });
    }

    fn warn<V>(&mut self, located: &Located<V>, message: String) {
        let (origin, span) = self.at(located);

        self.diagnostics.push(Diagnostic {
            origin,
            span,
            severity: Severity::Warning,
            message,
        });
    }

    fn keep<T, V>(
        &mut self,
        result: Result<T, OutOfRange>,
        at: &Located<V>,
        key: &str,
    ) -> Option<T> {
        match result {
            Ok(value) => Some(value),
            Err(error) => {
                let (origin, span) = self.at(at);

                self.diagnostics.push(Diagnostic {
                    origin,
                    span,
                    severity: Severity::Danger,
                    message: format!("{key}: {error}"),
                });
                None
            }
        }
    }
}

pub(crate) fn validate(merged: Layer) -> (Config, Vec<Diagnostic>) {
    let mut report = Report {
        fallback: Origin::new("<config>"),
        diagnostics: Vec::new(),
    };
    let themes = themes(merged.themes, &mut report);
    let connections = connections(merged.connections, &mut report);
    let config = Config {
        // Against the merged `themes`: the other file may define it.
        theme: theme(merged.theme, &themes, &mut report),
        background: background(merged.background, &mut report),
        icons: merged.icons.map_or_else(Default::default, bool_to_icons),
        mouse: merged.mouse.map_or_else(Default::default, bool_to_mouse),
        layout: layout(merged.layout, &mut report),
        grid: grid(merged.grid, &mut report),
        history: history(merged.history, &mut report),
        log: LogConfig {
            catalog: match merged.log.and_then(|log| log.catalog) {
                Some(true) => CatalogQueries::Shown,
                Some(false) | None => CatalogQueries::Hidden,
            },
        },
        driver: driver(merged.driver, &mut report),
        export: export(merged.export, &mut report),
        default_connection: default_connection(
            merged.default_connection,
            &connections,
            &mut report,
        ),
        themes,
        connections,
    };

    (config, report.diagnostics)
}

fn grid(layer: Option<GridLayer>, report: &mut Report) -> GridConfig {
    let Some(layer) = layer else {
        return GridConfig::default();
    };
    let width = layer.max_column_width.and_then(|value| {
        report.keep(
            MaxColumnWidth::try_new(value.value),
            &value,
            "grid.max_column_width",
        )
    });
    let rows = layer
        .page_rows
        .and_then(|value| report.keep(PageRows::try_new(value.value), &value, "grid.page_rows"));

    GridConfig {
        max_column_width: width.unwrap_or_default(),
        page_rows: rows.unwrap_or_default(),
    }
}

fn history(layer: Option<HistoryLayer>, report: &mut Report) -> HistoryConfig {
    let limit = layer
        .and_then(|layer| layer.limit)
        .and_then(|value| report.keep(HistoryLimit::try_new(value.value), &value, "history.limit"));

    HistoryConfig {
        limit: limit.unwrap_or_default(),
    }
}

/// One unit: a partial set falls back to the default whole.
fn layout(layer: Option<LayoutLayer>, report: &mut Report) -> PanelRatios {
    let Some(layer) = layer else {
        return PanelRatios::default();
    };
    let default = PanelRatios::default();
    let sidebar = layer
        .sidebar
        .as_ref()
        .map_or(default.sidebar(), |located| located.value);
    let ddl = layer
        .ddl
        .as_ref()
        .map_or(default.ddl(), |located| located.value);
    let queries = layer
        .queries
        .as_ref()
        .map_or(default.queries(), |located| located.value);

    if let Ok(ratios) = PanelRatios::try_new(sidebar, ddl, queries) {
        return ratios;
    }

    // Each share is checked alone, so the diagnostic blames the one out of
    // range.
    let failing = [
        (
            layer.sidebar.as_ref(),
            PanelRatios::try_new(sidebar, default.ddl(), default.queries()),
        ),
        (
            layer.ddl.as_ref(),
            PanelRatios::try_new(default.sidebar(), ddl, default.queries()),
        ),
        (
            layer.queries.as_ref(),
            PanelRatios::try_new(default.sidebar(), default.ddl(), queries),
        ),
    ]
    .into_iter()
    .find_map(|(located, isolated)| located.zip(isolated.err()));

    match failing {
        Some((located, error)) => report.error(located, format!("layout: {error}")),
        // Unreachable while shares are checked only against `5..=95`, but
        // reported for when joint validation lands.
        None => report.error_unlocated(
            "layout: shares are individually in range but not jointly valid — using default"
                .to_owned(),
        ),
    }

    PanelRatios::default()
}

fn driver(layer: Option<DriverLayer>, report: &mut Report) -> DriverConfig {
    let Some(layer) = layer else {
        return DriverConfig::default();
    };
    let page_size = layer
        .page_size
        .and_then(|value| report.keep(PageSize::try_new(value.value), &value, "driver.page_size"))
        .unwrap_or_default();
    let request_timeout = layer.request_timeout_ms.and_then(|value| {
        report.keep(
            Millis::try_new(value.value),
            &value,
            "driver.request_timeout_ms",
        )
    });
    let connect_timeout = layer
        .connect_timeout_ms
        .and_then(|value| {
            report.keep(
                Millis::try_new(value.value),
                &value,
                "driver.connect_timeout_ms",
            )
        })
        .unwrap_or_default();
    DriverConfig {
        page_size,
        request_timeout,
        connect_timeout,
    }
}

fn export(layer: Option<ExportLayer>, report: &mut Report) -> ExportConfig {
    let Some(layer) = layer else {
        return ExportConfig::default();
    };
    let clipboard = layer.clipboard_mb.and_then(|value| {
        report.keep(
            ClipboardMb::try_new(value.value),
            &value,
            "export.clipboard_mb",
        )
    });

    ExportConfig {
        path: layer.path.map(PathBuf::from),
        prefix: layer.prefix,
        clipboard_mb: clipboard.unwrap_or_default(),
    }
}

fn background(layer: Option<Located<String>>, report: &mut Report) -> Background {
    let Some(located) = layer else {
        return Background::default();
    };

    match located.value.as_str() {
        "theme" => Background::Theme,
        "terminal" => Background::Terminal,
        _ => match Rgb::parse(&located.value) {
            Ok(rgb) => Background::Fixed(rgb),
            Err(error) => {
                report.error(&located, format!("background: {error}"));
                Background::Theme
            }
        },
    }
}

fn themes(layers: Vec<ThemeLayer>, report: &mut Report) -> Vec<CustomTheme> {
    let mut themes = Vec::new();

    for layer in layers {
        let name = match ThemeName::try_new(&layer.name.value) {
            Ok(name) => name,
            Err(error) => {
                report.error(&layer.name, format!("theme.name: {error}"));
                continue;
            }
        };

        if ThemeChoice::builtin_from_slug(name.as_str()).is_some() {
            report.error(
                &layer.name,
                format!("theme `{}` claims a built-in slug", name.as_str()),
            );
            continue;
        }

        let extends = match layer.extends {
            None => BuiltIn::Ours(Ours::DefaultDark),
            Some(located) => {
                let Some(built_in) = ThemeChoice::builtin_from_slug(&located.value) else {
                    report.error(
                        &located,
                        format!(
                            "theme `{}`: extends unknown theme `{}`",
                            name.as_str(),
                            located.value
                        ),
                    );
                    continue;
                };
                built_in
            }
        };

        let mut colors = BTreeMap::new();
        for (key, located) in layer.colors {
            match Role::from_key(&key) {
                Some(role) => match Rgb::parse(&located.value) {
                    Ok(rgb) => {
                        colors.insert(role, rgb);
                    }
                    Err(error) => {
                        report.error(
                            &located,
                            format!("theme `{}`.{key}: {error}", name.as_str()),
                        );
                    }
                },
                None => report.warn(
                    &located,
                    format!("theme `{}`: unknown colour role `{key}`", name.as_str()),
                ),
            }
        }

        themes.push(CustomTheme {
            name,
            extends,
            colors,
        });
    }

    themes
}

fn connections(layers: Vec<ConnectionLayer>, report: &mut Report) -> Vec<ConnectionConfig> {
    let mut connections = Vec::new();

    for layer in layers {
        let name = match ConnectionName::try_new(&layer.name.value) {
            Ok(name) => name,
            Err(error) => {
                report.error(&layer.name, format!("connection.name: {error}"));
                continue;
            }
        };
        let blame = |reason: String| format!("connection `{}`: {reason}", name.as_str());

        let posture = match &layer.posture {
            None => Posture::default(),
            Some(located) => match located.value.as_str() {
                "read-only" => Posture::ReadOnly,
                "writable" => Posture::Writable,
                "protected" => Posture::Protected,
                other => {
                    report.error(located, blame(format!("posture `{other}` unknown")));
                    continue;
                }
            },
        };

        match connection(name.clone(), posture, layer, report) {
            Ok(connection) => connections.push(connection),
            Err((at, reason)) => report.error_at(at, blame(reason)),
        }
    }

    connections
}

type Refusal = (Option<(Origin, Span)>, String);

const POSTGRES_ONLY: &[&str] = &[
    "url",
    "host",
    "port",
    "database",
    "sslmode",
    "sslrootcert",
    "sslcert",
    "sslkey",
];
const CASSANDRA_ONLY: &[&str] = &[
    "hosts",
    "discovery",
    "datacenter",
    "username",
    "cqlshrc",
    "tls",
    "consistency",
];

fn stated_keys(layer: &ConnectionLayer) -> Vec<&'static str> {
    [
        ("url", layer.url.is_some()),
        ("host", layer.host.is_some()),
        ("port", layer.port.is_some()),
        ("database", layer.database.is_some()),
        ("sslmode", layer.sslmode.is_some()),
        ("sslrootcert", layer.sslrootcert.is_some()),
        ("sslcert", layer.sslcert.is_some()),
        ("sslkey", layer.sslkey.is_some()),
        ("hosts", layer.hosts.is_some()),
        ("discovery", layer.discovery.is_some()),
        ("datacenter", layer.datacenter.is_some()),
        ("username", layer.username.is_some()),
        ("cqlshrc", layer.cqlshrc.is_some()),
        ("tls", layer.tls.is_some()),
        ("consistency", layer.consistency.is_some()),
    ]
    .into_iter()
    .filter_map(|(key, stated)| stated.then_some(key))
    .collect()
}

/// Unstated, the keys decide: a `LazyCassandra` entry names `hosts`, a `LazyPg`
/// one never does, so both kinds of file read unchanged.
fn engine(layer: &ConnectionLayer) -> Result<Engine, Refusal> {
    let Some(located) = &layer.engine else {
        let cassandra = stated_keys(layer)
            .iter()
            .any(|key| CASSANDRA_ONLY.contains(key));
        return Ok(if cassandra {
            Engine::Cassandra
        } else {
            Engine::Postgres
        });
    };
    Engine::ALL
        .into_iter()
        .find(|engine| engine.key() == located.value)
        .ok_or_else(|| {
            let known: Vec<_> = Engine::ALL.iter().map(|engine| engine.key()).collect();
            (
                located.at.clone(),
                format!("engine `{}` unknown — {}", located.value, known.join(" | ")),
            )
        })
}

fn connection(
    name: ConnectionName,
    posture: Posture,
    layer: ConnectionLayer,
    report: &mut Report,
) -> Result<ConnectionConfig, Refusal> {
    let engine = engine(&layer)?;
    let foreign = match engine {
        Engine::Postgres => CASSANDRA_ONLY,
        Engine::Cassandra => POSTGRES_ONLY,
    };
    let misplaced: Vec<_> = stated_keys(&layer)
        .into_iter()
        .filter(|key| foreign.contains(key))
        .collect();
    if !misplaced.is_empty() {
        return Err((
            layer.name.at.clone(),
            format!(
                "{} not {} key{}; this is a {} connection",
                misplaced.join(", "),
                if misplaced.len() == 1 { "is" } else { "are" },
                if misplaced.len() == 1 { "" } else { "s" },
                engine.label()
            ),
        ));
    }

    let target = match engine {
        Engine::Postgres => Target::Postgres(postgres(layer, report)?),
        Engine::Cassandra => Target::Cassandra(cassandra(layer)?),
    };
    Ok(ConnectionConfig {
        name,
        posture,
        target,
    })
}

fn password(layer: Option<CredentialLayer>) -> Option<Credential> {
    layer.map(|credential| match credential {
        CredentialLayer::Literal(value) => Credential::Literal(value),
        CredentialLayer::Reference(CredentialSource::Env(name)) => Credential::Env(name),
        CredentialLayer::Reference(CredentialSource::File(path)) => {
            Credential::File(PathBuf::from(path))
        }
    })
}

fn postgres(layer: ConnectionLayer, report: &mut Report) -> Result<PostgresTarget, Refusal> {
    let mut config = PostgresTarget::default();
    let refuse = |at: &Located<String>, reason: String| (at.at.clone(), reason);
    let unlocated = |reason: String| (None, reason);

    if let Some(at) = &layer.url {
        let stated: Vec<_> = [
            ("host", layer.host.is_some()),
            ("port", layer.port.is_some()),
            ("database", layer.database.is_some()),
            ("user", layer.user.is_some()),
            ("password", layer.password.is_some()),
            ("sslmode", layer.sslmode.is_some()),
        ]
        .into_iter()
        .filter_map(|(key, stated)| stated.then_some(key))
        .collect();
        if !stated.is_empty() {
            return Err(refuse(
                at,
                format!(
                    "`url` carries {} itself; state them in one place",
                    stated.join(", ")
                ),
            ));
        }

        let Url {
            host,
            port,
            database,
            user,
            password,
            params,
            ignored,
        } = url::parse(&at.value).map_err(|reason| refuse(at, format!("url: {reason}")))?;
        for param in ignored {
            report.warn(
                at,
                format!("url: `{param}` is not a parameter LazyDB reads; ignored"),
            );
        }

        config.host = host;
        config.port = port;
        config.database = database;
        config.user = user;
        config.password = password.map(Credential::Literal);
        config.sslmode = params
            .sslmode
            .map(|mode| ssl_mode(&mode).map_err(|reason| refuse(at, reason)))
            .transpose()?;
        return tls(config, params.sslrootcert, params.sslcert, params.sslkey).map_err(unlocated);
    }

    config.host = layer.host;
    config.port = match layer.port {
        None => None,
        Some(port) => {
            let valid = u16::try_from(port.value)
                .ok()
                .and_then(std::num::NonZeroU16::new);
            let Some(valid) = valid else {
                return Err((port.at, format!("port {} is not 1..=65535", port.value)));
            };
            Some(valid)
        }
    };
    config.database = layer.database;
    config.user = layer.user;
    config.password = password(layer.password);
    config.sslmode = layer
        .sslmode
        .map(|mode| ssl_mode(&mode.value).map_err(|reason| refuse(&mode, reason)))
        .transpose()?;

    tls(config, layer.sslrootcert, layer.sslcert, layer.sslkey).map_err(unlocated)
}

fn cassandra(layer: ConnectionLayer) -> Result<CassandraTarget, Refusal> {
    let at = layer.name.at.clone();
    let refuse = |reason: String| (at.clone(), reason);

    let Some(hosts) = layer.hosts.filter(|hosts| !hosts.is_empty()) else {
        return Err(refuse("hosts required and non-empty".to_owned()));
    };

    let discovery = match layer.discovery {
        None => Discovery::default(),
        Some(located) => match located.value.as_str() {
            "cluster" => Discovery::Cluster,
            "contact-points" => Discovery::ContactPoints,
            other => return Err((located.at, format!("discovery `{other}` unknown"))),
        },
    };
    if discovery == Discovery::ContactPoints && hosts.len() != 1 {
        return Err(refuse(format!(
            "discovery `contact-points` reaches exactly one host, {} stated",
            hosts.len()
        )));
    }

    let consistency = match layer.consistency {
        None => Consistency::default(),
        Some(located) => {
            // The kebab spelling, or cqlsh's: `local-quorum` and `LOCAL_QUORUM`.
            let spelled = located.value.replace('-', "_");
            Consistency::ALL
                .into_iter()
                .find(|level| level.label().eq_ignore_ascii_case(&spelled))
                .ok_or_else(|| {
                    (
                        located.at.clone(),
                        format!(
                            "consistency `{}` unknown — any | one | two | three | quorum | all | local-quorum | each-quorum | local-one | serial | local-serial",
                            located.value
                        ),
                    )
                })?
        }
    };

    let user = match (layer.user, layer.username) {
        (Some(_), Some(_)) => {
            return Err(refuse(
                "`user` and `username` are one key; state one".to_owned(),
            ));
        }
        (user, username) => user.or(username),
    };
    let credentials = credentials(layer.cqlshrc, user, password(layer.password))
        .map_err(|reason| refuse(reason.to_owned()))?;
    let tls = connection_tls(layer.tls, credentials.as_ref())
        .map_err(|reason| refuse(format!("tls: {reason}")))?;

    Ok(CassandraTarget {
        hosts,
        discovery,
        datacenter: layer.datacenter,
        credentials,
        tls,
        consistency,
    })
}

/// `tls` beside a `cqlshrc` overrides it.
fn connection_tls(
    layer: Option<TlsLayer>,
    credentials: Option<&Credentials>,
) -> Result<Tls, String> {
    layer.map_or_else(
        || {
            Ok(match credentials {
                Some(Credentials::Cqlshrc(path)) => Tls::Cqlshrc(path.clone()),
                _ => Tls::Off,
            })
        },
        cassandra_tls,
    )
}

fn cassandra_tls(layer: TlsLayer) -> Result<Tls, String> {
    let table = match layer {
        TlsLayer::Switch(false) => return Ok(Tls::Off),
        TlsLayer::Switch(true) => return Ok(Tls::On(TlsSettings::default())),
        TlsLayer::Table(table) => table,
    };
    let TlsTable {
        ca,
        cert,
        key,
        verify,
        server_name,
        unknown: _,
    } = table;

    let identity = match (cert, key) {
        (None, None) => None,
        (Some(cert), Some(key)) => Some(ClientIdentity {
            cert: PathBuf::from(cert),
            key: PathBuf::from(key),
        }),
        _ => return Err("`cert` and `key` are stated together or not at all".to_owned()),
    };

    let verification = match (verify.as_deref(), server_name) {
        (None | Some("full"), None) => Verification::Full,
        (None | Some("full"), Some(name)) => Verification::Named(name),
        (Some("chain"), None) => Verification::ChainOnly,
        (Some("none"), None) => Verification::Off,
        (Some("chain" | "none"), Some(_)) => {
            return Err("`server_name` names what `verify = \"full\"` checks".to_owned());
        }
        (Some(other), _) => {
            return Err(format!("verify `{other}` unknown — full | chain | none"));
        }
    };

    Ok(Tls::On(TlsSettings {
        trust: ca.map_or(Trust::System, |path| Trust::CaFile(PathBuf::from(path))),
        identity,
        verification,
    }))
}

/// `cqlshrc` supplies both halves, so anything beside it is refused rather than
/// merged.
fn credentials(
    cqlshrc: Option<String>,
    user: Option<String>,
    password: Option<Credential>,
) -> Result<Option<Credentials>, &'static str> {
    match (cqlshrc, user, password) {
        (None, None, None) => Ok(None),
        (None, Some(username), Some(password)) => {
            Ok(Some(Credentials::Stated { username, password }))
        }
        (Some(path), None, None) => Ok(Some(Credentials::Cqlshrc(PathBuf::from(path)))),
        (Some(_), _, _) => {
            Err("`cqlshrc` supplies the user and the password, so neither is stated beside it")
        }
        (None, Some(_), None) => {
            Err("user stated without a password — a cluster is sent both or neither")
        }
        (None, None, Some(_)) => {
            Err("password stated without a user — a cluster is sent both or neither")
        }
    }
}

fn ssl_mode(word: &str) -> Result<SslMode, String> {
    SslMode::ALL
        .into_iter()
        .find(|mode| mode.label() == word)
        .ok_or_else(|| {
            let known: Vec<_> = SslMode::ALL.iter().map(|mode| mode.label()).collect();
            format!("sslmode `{word}` unknown — {}", known.join(" | "))
        })
}

fn tls(
    mut config: PostgresTarget,
    rootcert: Option<String>,
    cert: Option<String>,
    key: Option<String>,
) -> Result<PostgresTarget, String> {
    config.identity = match (cert, key) {
        (None, None) => None,
        (Some(cert), Some(key)) => Some(ClientIdentity {
            cert: PathBuf::from(cert),
            key: PathBuf::from(key),
        }),
        _ => return Err("`sslcert` and `sslkey` are stated together or not at all".to_owned()),
    };
    config.trust = rootcert.map_or(Trust::System, |path| Trust::CaFile(PathBuf::from(path)));

    if config.sslmode == Some(SslMode::Disable)
        && (config.identity.is_some() || config.trust != Trust::System)
    {
        return Err(
            "certificates are stated but `sslmode = \"disable\"` never uses them".to_owned(),
        );
    }
    Ok(config)
}

fn theme(
    layer: Option<Located<String>>,
    themes: &[CustomTheme],
    report: &mut Report,
) -> ThemeChoice {
    let Some(located) = layer else {
        return ThemeChoice::default();
    };

    if let Some(built_in) = ThemeChoice::builtin_from_slug(&located.value) {
        return built_in.into();
    }

    if let Some(custom) = themes
        .iter()
        .find(|theme| theme.name.as_str() == located.value)
    {
        return ThemeChoice::Custom(custom.name.clone());
    }

    report.error(
        &located,
        format!("theme `{}` is not defined", located.value),
    );
    ThemeChoice::default()
}

fn default_connection(
    layer: Option<Located<String>>,
    connections: &[ConnectionConfig],
    report: &mut Report,
) -> Option<ConnectionName> {
    let located = layer?;

    if let Some(connection) = connections
        .iter()
        .find(|c| c.name.as_str() == located.value)
    {
        return Some(connection.name.clone());
    }

    report.error(
        &located,
        format!("default_connection `{}` is not defined", located.value),
    );
    None
}

fn bool_to_icons(value: bool) -> Icons {
    if value { Icons::On } else { Icons::Off }
}

fn bool_to_mouse(value: bool) -> Mouse {
    if value { Mouse::On } else { Mouse::Off }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{merge::merge, parse};

    fn config_of(sources: &[&str]) -> (Config, Vec<Diagnostic>) {
        let origin = Origin::new("test.toml");
        let layers = sources
            .iter()
            .filter_map(|source| parse(source, &origin).0)
            .collect();
        let (merged, mut diagnostics) = merge(layers);
        let (config, more) = validate(merged);
        diagnostics.extend(more);

        (config, diagnostics)
    }

    #[test]
    fn a_connection_with_an_unknown_posture_is_dropped_and_the_rest_of_the_layer_stands() {
        let (config, diagnostics) = config_of(&["theme = \"ash-dark\"\n\
             [[connections]]\nname = \"dev\"\nposture = \"read\"\n\
             [[connections]]\nname = \"prod\"\n"]);

        assert_eq!(
            config.theme,
            ThemeChoice::Ours(Ours::AshDark),
            "the layer still applies"
        );
        assert_eq!(config.connections.len(), 1);
        assert_eq!(config.connections[0].name.as_str(), "prod");
        assert!(
            diagnostics
                .iter()
                .any(|d| d.message.contains("dev") && d.message.contains("posture"))
        );
    }

    #[test]
    fn a_key_out_of_range_falls_through_to_the_default_and_reports() {
        let (config, diagnostics) = config_of(&["[grid]\nmax_column_width = 0\n"]);

        assert_eq!(config.grid.max_column_width, MaxColumnWidth::default());
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].severity, Severity::Danger);
    }

    #[test]
    fn a_stated_page_size_for_the_grid_is_kept() {
        let (config, diagnostics) = config_of(&["[grid]\npage_rows = 50\n"]);

        assert_eq!(config.grid.page_rows.get(), 50);
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn a_stated_history_limit_is_kept() {
        let (config, diagnostics) = config_of(&["[history]\nlimit = 25\n"]);

        assert_eq!(config.history.limit.get(), 25);
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn a_history_limit_of_zero_turns_the_history_off() {
        let (config, diagnostics) = config_of(&["[history]\nlimit = 0\n"]);

        assert!(config.history.limit.is_off());
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn a_history_limit_out_of_range_falls_back_to_the_default() {
        let (config, diagnostics) = config_of(&["[history]\nlimit = 10001\n"]);

        assert_eq!(config.history.limit.get(), 100);
        assert_eq!(diagnostics.len(), 1);
        assert!(diagnostics[0].message.contains("history.limit"));
    }

    #[test]
    fn a_page_size_for_the_grid_out_of_range_falls_back_to_the_default() {
        let (config, diagnostics) = config_of(&["[grid]\npage_rows = 0\n"]);

        assert_eq!(config.grid.page_rows.get(), 200);
        assert_eq!(diagnostics.len(), 1);
        assert!(diagnostics[0].message.contains("grid.page_rows"));
    }

    #[test]
    fn a_stated_column_width_is_kept() {
        let (config, diagnostics) = config_of(&["[grid]\nmax_column_width = 40\n"]);

        assert_eq!(config.grid.max_column_width.get(), 40);
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn a_layout_failure_blames_the_share_that_is_out_of_range_not_the_first_one_stated() {
        let (config, diagnostics) = config_of(&["[layout]\nsidebar = 50\nddl = 3\n"]);

        assert_eq!(config.layout, PanelRatios::default());
        assert_eq!(diagnostics.len(), 1);
        assert!(
            diagnostics[0].message.contains("ddl"),
            "must name the failing share, not sidebar: {diagnostics:?}"
        );
        assert_eq!(
            diagnostics[0].span.map(|span| span.line),
            Some(3),
            "must point at ddl's line, not sidebar's: {diagnostics:?}"
        );
    }

    #[test]
    fn a_theme_named_in_one_file_and_defined_in_the_other_resolves() {
        let (config, diagnostics) = config_of(&[
            "theme = \"midnight\"\n",
            "[[themes]]\nname = \"midnight\"\nextends = \"ash-dark\"\n[themes.colors]\nbase = \"#0b0d12\"\n",
        ]);

        assert!(diagnostics.is_empty(), "{diagnostics:?}");
        assert_eq!(
            config.theme,
            ThemeChoice::Custom(ThemeName::try_new("midnight").expect("valid")),
            "a cross-reference is resolved after the merge, not within a layer"
        );
        assert_eq!(
            config.themes[0].colors[&Role::Base],
            Rgb {
                red: 0x0b,
                green: 0x0d,
                blue: 0x12
            }
        );
    }

    #[test]
    fn a_theme_that_names_nothing_falls_back_to_the_default_and_reports() {
        let (config, diagnostics) = config_of(&["theme = \"nowhere\"\n"]);

        assert_eq!(config.theme, ThemeChoice::default());
        assert_eq!(diagnostics.len(), 1);
        assert!(diagnostics[0].message.contains("nowhere"));
    }

    #[test]
    fn a_default_connection_naming_nothing_falls_back_to_none_and_reports() {
        let (config, diagnostics) = config_of(&["default_connection = \"nowhere\"\n"]);

        assert!(config.default_connection.is_none());
        assert_eq!(diagnostics.len(), 1);
    }

    #[test]
    fn a_custom_theme_claiming_a_built_in_slug_is_dropped() {
        let (config, diagnostics) = config_of(&["[[themes]]\nname = \"default-dark\"\n"]);

        assert!(config.themes.is_empty());
        assert!(diagnostics[0].message.contains("default-dark"));
    }

    #[test]
    fn a_background_is_a_word_or_a_hex() {
        assert_eq!(
            config_of(&["background = \"terminal\"\n"]).0.background,
            Background::Terminal
        );
        assert_eq!(
            config_of(&["background = \"theme\"\n"]).0.background,
            Background::Theme
        );
        assert_eq!(
            config_of(&["background = \"#0b0d12\"\n"]).0.background,
            Background::Fixed(Rgb {
                red: 0x0b,
                green: 0x0d,
                blue: 0x12
            })
        );
        let (config, diagnostics) = config_of(&["background = \"puce\"\n"]);
        assert_eq!(config.background, Background::Theme);
        assert!(
            diagnostics
                .iter()
                .any(|d| d.message.starts_with("background: ")),
            "the diagnostic must name the key it is about, not only the value: {diagnostics:?}"
        );
    }

    #[test]
    fn an_unknown_colour_role_is_a_warning_and_the_theme_survives() {
        let (config, diagnostics) = config_of(&[
            "[[themes]]\nname = \"midnight\"\n[themes.colors]\nbaes = \"#0b0d12\"\nfocus = \"#7aa2f7\"\n",
        ]);

        assert_eq!(config.themes.len(), 1);
        assert_eq!(config.themes[0].colors.len(), 1);
        let unknown_role = diagnostics
            .iter()
            .find(|d| d.message.contains("baes"))
            .expect("the unknown role is reported");
        assert_eq!(unknown_role.severity, Severity::Warning);
    }

    #[test]
    fn an_unknown_role_is_a_warning_but_a_malformed_hex_on_a_real_role_is_an_error_and_only_that_theme_survives_with_what_was_valid()
     {
        let (config, diagnostics) = config_of(&[
            "[[themes]]\nname = \"midnight\"\n[themes.colors]\nbaes = \"#0b0d12\"\nfocus = \"not-a-colour\"\nbase = \"#7aa2f7\"\n",
        ]);

        assert_eq!(config.themes.len(), 1, "the theme survives both drops");
        assert_eq!(
            config.themes[0].colors.len(),
            1,
            "only the valid colour is kept"
        );

        let unknown_role = diagnostics
            .iter()
            .find(|d| d.message.contains("baes"))
            .expect("the unknown role is reported");
        assert_eq!(unknown_role.severity, Severity::Warning);

        let malformed_hex = diagnostics
            .iter()
            .find(|d| d.message.contains("focus"))
            .expect("the malformed hex is reported");
        assert_eq!(malformed_hex.severity, Severity::Danger);
    }

    fn pg(connection: &ConnectionConfig) -> &PostgresTarget {
        match &connection.target {
            Target::Postgres(target) => target,
            Target::Cassandra(_) => panic!("{} is a Cassandra entry", connection.name.as_str()),
        }
    }

    fn cassandra(connection: &ConnectionConfig) -> &CassandraTarget {
        match &connection.target {
            Target::Cassandra(target) => target,
            Target::Postgres(_) => panic!("{} is a Postgres entry", connection.name.as_str()),
        }
    }

    #[test]
    fn a_password_is_read_as_a_literal_or_as_a_reference() {
        let (config, _) = config_of(&["[[connections]]\nname = \"a\"\npassword = \"hunter2\"\n\
             [[connections]]\nname = \"b\"\npassword = { env = \"DEV_PW\" }\n\
             [[connections]]\nname = \"c\"\n"]);

        assert_eq!(
            pg(&config.connections[0]).password,
            Some(Credential::Literal("hunter2".to_owned()))
        );
        assert_eq!(
            pg(&config.connections[1]).password,
            Some(Credential::Env("DEV_PW".to_owned()))
        );
        assert_eq!(
            pg(&config.connections[2]).password,
            None,
            "left to PGPASSWORD and the passfile"
        );
    }

    #[test]
    fn a_connection_stating_only_its_name_leaves_everything_to_libpq() {
        let (config, diagnostics) = config_of(&["[[connections]]\nname = \"bare\"\n"]);

        assert!(diagnostics.is_empty(), "{diagnostics:?}");
        assert_eq!(
            config.connections[0],
            ConnectionConfig::named(ConnectionName::try_new("bare").unwrap())
        );
    }

    #[test]
    fn the_libpq_keys_are_read_as_stated() {
        let (config, diagnostics) = config_of(&["[[connections]]\nname = \"prod\"\n\
             host = \"db\"\nport = 6543\ndatabase = \"orders\"\nuser = \"ana\"\n\
             sslmode = \"verify-full\"\nsslrootcert = \"/ca.pem\"\n\
             sslcert = \"/c.pem\"\nsslkey = \"/k.pem\"\n"]);

        assert!(diagnostics.is_empty(), "{diagnostics:?}");
        assert_eq!(config.connections[0].address(), "db:6543/orders");
        let prod = pg(&config.connections[0]);
        assert_eq!(prod.user.as_deref(), Some("ana"));
        assert_eq!(prod.sslmode, Some(SslMode::VerifyFull));
        assert_eq!(prod.trust, Trust::CaFile(PathBuf::from("/ca.pem")));
        assert_eq!(
            prod.identity,
            Some(ClientIdentity {
                cert: PathBuf::from("/c.pem"),
                key: PathBuf::from("/k.pem"),
            })
        );
    }

    fn refused(body: &str, about: &str) {
        let (config, diagnostics) =
            config_of(&[format!("[[connections]]\nname = \"prod\"\n{body}\n").as_str()]);

        assert!(config.connections.is_empty(), "{body}");
        assert!(
            diagnostics
                .iter()
                .any(|d| d.message.contains("prod") && d.message.contains(about)),
            "{body}: {diagnostics:?}"
        );
    }

    #[test]
    fn a_connection_that_contradicts_itself_is_dropped_and_says_why() {
        refused("port = 0", "port");
        refused("port = 70000", "port");
        refused("sslmode = \"allow\"", "sslmode");
        refused("sslcert = \"/c.pem\"", "sslkey");
        refused(
            "sslmode = \"disable\"\nsslrootcert = \"/ca.pem\"",
            "disable",
        );
        refused("url = \"mysql://db/app\"", "url");
        refused("url = \"postgresql://db/app\"\nhost = \"db\"", "host");
    }

    #[test]
    fn a_url_fills_the_keys_it_carries() {
        let (config, diagnostics) = config_of(&["[[connections]]\nname = \"app\"\n\
             url = \"jdbc:postgresql://db:5433/orders?user=svc&password=pw&sslmode=require&ssl=true\"\n"]);

        assert_eq!(config.connections[0].address(), "db:5433/orders");
        let app = pg(&config.connections[0]);
        assert_eq!(app.user.as_deref(), Some("svc"));
        assert_eq!(app.password, Some(Credential::Literal("pw".to_owned())));
        assert_eq!(app.sslmode, Some(SslMode::Require));
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].severity, Severity::Warning);
        assert!(diagnostics[0].message.contains("`ssl`"));
    }

    #[test]
    fn the_catalog_queries_are_logged_only_when_asked_for() {
        let (unset, _) = config_of(&[""]);
        let (shown, diagnostics) = config_of(&["[log]\ncatalog = true\n"]);

        assert_eq!(unset.log.catalog, CatalogQueries::Hidden);
        assert_eq!(shown.log.catalog, CatalogQueries::Shown);
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn a_lazycassandra_entry_is_read_as_cassandra_without_an_engine_key() {
        let (config, diagnostics) = config_of(&["[[connections]]\nname = \"dev\"\n\
             hosts = [\"127.0.0.1:9042\"]\ndatacenter = \"dc1\"\n\
             username = \"u\"\npassword = \"p\"\nconsistency = \"local-one\"\n"]);

        assert!(diagnostics.is_empty(), "{diagnostics:?}");
        let dev = &config.connections[0];
        assert_eq!(dev.engine(), Engine::Cassandra);
        let dev = cassandra(dev);
        assert_eq!(dev.hosts, ["127.0.0.1:9042"]);
        assert_eq!(dev.datacenter.as_deref(), Some("dc1"));
        assert_eq!(dev.consistency, Consistency::LocalOne);
        assert_eq!(
            dev.credentials,
            Some(Credentials::Stated {
                username: "u".to_owned(),
                password: Credential::Literal("p".to_owned()),
            })
        );
    }

    #[test]
    fn an_engine_stated_outright_is_kept_and_an_unknown_one_drops_the_entry() {
        let (config, _) = config_of(&["[[connections]]\nname = \"pg\"\nengine = \"postgres\"\n\
             [[connections]]\nname = \"cass\"\nengine = \"cassandra\"\nhosts = [\"h:9042\"]\n"]);
        let engines: Vec<_> = config
            .connections
            .iter()
            .map(ConnectionConfig::engine)
            .collect();
        assert_eq!(engines, [Engine::Postgres, Engine::Cassandra]);

        refused("engine = \"oracle\"", "engine");
    }

    #[test]
    fn a_key_of_the_other_engine_drops_the_entry_rather_than_being_ignored() {
        refused("engine = \"postgres\"\nhosts = [\"h:9042\"]", "hosts");
        refused("hosts = [\"h:9042\"]\nsslmode = \"require\"", "sslmode");
        refused("engine = \"cassandra\"", "hosts required");
    }

    #[test]
    fn discovery_is_read_per_connection_and_defaults_to_cluster() {
        let (config, diagnostics) =
            config_of(
                &["[[connections]]\nname = \"direct\"\nhosts = [\"a:9042\"]\n\
             [[connections]]\nname = \"tunnelled\"\nhosts = [\"127.0.0.1:9042\"]\n\
             discovery = \"contact-points\"\n"],
            );

        assert!(diagnostics.is_empty());
        assert_eq!(
            cassandra(&config.connections[0]).discovery,
            Discovery::Cluster
        );
        assert_eq!(
            cassandra(&config.connections[1]).discovery,
            Discovery::ContactPoints
        );
    }

    #[test]
    fn a_cassandra_entry_that_contradicts_itself_is_dropped_and_says_why() {
        let hosts = "hosts = [\"h:9042\"]\n";
        refused(&format!("{hosts}discovery = \"single\""), "discovery");
        refused(
            "hosts = [\"a:9043\", \"b:9044\"]\ndiscovery = \"contact-points\"",
            "one host",
        );
        refused(&format!("{hosts}username = \"u\""), "password");
        refused(&format!("{hosts}password = \"p\""), "user");
        refused(
            &format!("{hosts}user = \"u\"\nusername = \"u\"\npassword = \"p\""),
            "username",
        );
        refused(&format!("{hosts}consistency = \"most\""), "consistency");
        refused(
            &format!("{hosts}cqlshrc = \"rc\"\nuser = \"u\"\npassword = \"p\""),
            "cqlshrc",
        );
        for tls in [
            "tls = { cert = \"c.pem\" }",
            "tls = { verify = \"chain\", server_name = \"x\" }",
            "tls = { verify = \"sometimes\" }",
        ] {
            refused(&format!("{hosts}{tls}"), "tls");
        }
    }

    #[test]
    fn a_tls_table_states_the_ca_the_identity_and_the_verification() {
        let (config, diagnostics) =
            config_of(&["[[connections]]\nname = \"prod\"\nhosts = [\"h:9042\"]\n\
             tls = { ca = \"ca.pem\", cert = \"c.pem\", key = \"k.pem\", verify = \"chain\" }\n"]);

        assert!(diagnostics.is_empty());
        assert_eq!(
            cassandra(&config.connections[0]).tls,
            Tls::On(TlsSettings {
                trust: Trust::CaFile(PathBuf::from("ca.pem")),
                identity: Some(ClientIdentity {
                    cert: PathBuf::from("c.pem"),
                    key: PathBuf::from("k.pem"),
                }),
                verification: Verification::ChainOnly,
            })
        );
    }

    #[test]
    fn a_cqlshrc_is_the_whole_pair_and_carries_its_tls_unless_tls_is_stated() {
        let (config, _) = config_of(&["[[connections]]\nname = \"prod\"\n\
             hosts = [\"h:9042\"]\ncqlshrc = \"rc\"\n"]);
        let prod = cassandra(&config.connections[0]);
        assert_eq!(
            prod.credentials,
            Some(Credentials::Cqlshrc(PathBuf::from("rc")))
        );
        assert_eq!(prod.tls, Tls::Cqlshrc(PathBuf::from("rc")));

        let (config, diagnostics) = config_of(&["[[connections]]\nname = \"prod\"\n\
             hosts = [\"h:9042\"]\ncqlshrc = \"rc\"\ntls = false\n"]);
        assert!(diagnostics.is_empty());
        assert_eq!(cassandra(&config.connections[0]).tls, Tls::Off);
    }
}
