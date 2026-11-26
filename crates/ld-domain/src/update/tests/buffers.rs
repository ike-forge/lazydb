use super::*;
use crate::buffers::Buffers;

fn named(name: &str) -> ConnectionName {
    ConnectionName::try_new(name).expect("a valid name")
}

fn switch_to(state: &mut State, name: &str) -> Vec<Effect> {
    connect(state, ConnectionConfig::named(named(name)))
}

fn saved_buffers(effects: &[Effect]) -> Vec<(&str, &str)> {
    effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::SaveBuffer { connection, text } => Some((connection.as_str(), text.as_str())),
            _ => None,
        })
        .collect()
}

fn on_dev(text: &str) -> State {
    let mut state = State::default();
    switch_to(&mut state, "dev");
    state.queries.replace(text);
    state
}

#[test]
fn switching_keeps_the_buffer_left_and_shows_the_one_chosen() {
    let mut state = on_dev("SELECT 1;");
    let mut stored = Buffers::default();
    stored.restore(&named("prod"), "SELECT 2;".to_owned());
    stored.restore(&named("dev"), "stale".to_owned());
    stored.own(named("dev"));
    state.buffers = stored;

    let effects = switch_to(&mut state, "prod");

    assert_eq!(saved_buffers(&effects), [("dev", "SELECT 1;")]);
    assert_eq!(state.queries.text(), "SELECT 2;");

    switch_to(&mut state, "dev");

    assert_eq!(state.queries.text(), "SELECT 1;", "and back again");
}

#[test]
fn a_connection_with_no_buffer_opens_on_an_empty_one() {
    let mut state = on_dev("SELECT 1;");

    switch_to(&mut state, "prod");

    assert_eq!(state.queries.text(), "");
}

#[test]
fn reconnecting_to_the_same_connection_leaves_the_buffer_alone() {
    let mut state = on_dev("SELECT 1;\nSELECT 2;");
    state.queries.edge_statement(false);
    let cursor = state.queries.cursor();

    let effects = switch_to(&mut state, "dev");

    assert!(saved_buffers(&effects).is_empty());
    assert_eq!(state.queries.cursor(), cursor);
}

#[test]
fn text_typed_before_any_connection_goes_to_the_first_one_chosen() {
    let mut state = State::default();
    state.queries.replace("SELECT 1;");

    switch_to(&mut state, "dev");

    assert_eq!(state.queries.text(), "SELECT 1;");
    assert_eq!(state.buffers.owner(), Some(&named("dev")));
}

#[test]
fn but_not_over_a_buffer_that_connection_already_had() {
    let mut state = State::default();
    state.buffers.restore(&named("dev"), "SELECT 2;".to_owned());
    state.queries.replace("SELECT 1;");

    switch_to(&mut state, "dev");

    assert_eq!(state.queries.text(), "SELECT 2;");
}

#[test]
fn a_failed_connection_still_owns_the_buffer() {
    let mut state = on_dev("SELECT 1;");
    let conn = match &state.connection {
        ConnectionState::Connecting { conn, .. } => *conn,
        other => panic!("connecting, not {other:?}"),
    };
    update(
        &mut state,
        Msg::ConnectFailed {
            conn,
            error: DbError::Unreachable("refused".to_owned()),
            took: Duration::ZERO,
        },
    );

    let effects = switch_to(&mut state, "prod");

    assert_eq!(saved_buffers(&effects), [("dev", "SELECT 1;")]);
}

#[test]
fn a_tick_writes_a_changed_buffer_every_two_seconds() {
    let mut state = on_dev("SELECT 1;");

    let first = update(&mut state, Msg::Tick(Stamp::from_unix(100)));
    let unchanged = update(&mut state, Msg::Tick(Stamp::from_unix(102)));
    state.queries.replace("SELECT 2;");
    let too_soon = update(&mut state, Msg::Tick(Stamp::from_unix(103)));
    let later = update(&mut state, Msg::Tick(Stamp::from_unix(104)));

    assert_eq!(saved_buffers(&first), [("dev", "SELECT 1;")]);
    assert!(saved_buffers(&unchanged).is_empty());
    assert!(saved_buffers(&too_soon).is_empty());
    assert_eq!(saved_buffers(&later), [("dev", "SELECT 2;")]);
}

#[test]
fn an_emptied_buffer_is_written_empty() {
    let mut state = on_dev("SELECT 1;");
    update(&mut state, Msg::Tick(Stamp::from_unix(100)));
    state.queries.replace("");

    let effects = update(&mut state, Msg::Tick(Stamp::from_unix(102)));

    assert_eq!(saved_buffers(&effects), [("dev", "")]);
}

#[test]
fn quitting_writes_the_buffer_before_quitting() {
    let mut state = on_dev("SELECT 1;");

    let effects = update(&mut state, Msg::Quit);

    assert!(
        matches!(
            effects.as_slice(),
            [Effect::SaveBuffer { text, .. }, Effect::Quit] if text == "SELECT 1;"
        ),
        "{effects:?}"
    );
}

#[test]
fn without_a_connection_nothing_is_written() {
    let mut state = State::default();
    state.queries.replace("SELECT 1;");

    let ticked = update(&mut state, Msg::Tick(Stamp::from_unix(100)));
    let quit = update(&mut state, Msg::Quit);

    assert!(saved_buffers(&ticked).is_empty());
    assert_eq!(quit, [Effect::Quit]);
}

#[test]
fn buffers_restored_after_init_show_the_default_connections() {
    let config = Config {
        connections: vec![ConnectionConfig::named(named("dev"))],
        default_connection: Some(named("dev")),
        ..Config::default()
    };
    let (mut state, _effects) = init(config, Vec::new(), Stamp::default());
    let mut stored = Buffers::default();
    stored.restore(&named("dev"), "SELECT 1;".to_owned());

    state.restore_buffers(stored);

    assert_eq!(state.queries.text(), "SELECT 1;");
    assert_eq!(state.buffers.owner(), Some(&named("dev")));
}
