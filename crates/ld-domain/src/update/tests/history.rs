use super::*;
use crate::Engine;
use crate::config::{HistoryConfig, HistoryLimit};
use crate::history::{Entry, Layer, Picker};
use crate::model::Server;
use crate::view::hints::bar;

fn open_history(state: &mut State) -> Vec<Effect> {
    ctrl(state, Key::Char('r'))
}

fn roomy() -> Geometry {
    Geometry::new([8, 14, 20, 1], 37, 14).with_history_rows(10)
}

fn having_run(statements: &[&str]) -> State {
    let mut state = editing("");
    state.geometry = roomy();

    for statement in statements {
        state.focus_on(Panel::Queries);
        state.queries.replace(statement);
        run_query(&mut state);
    }

    state.focus_on(Panel::Queries);
    state.queries.replace("");
    state
}

/// Recorded on another connection, as an earlier session would have.
fn ran_elsewhere(state: &mut State, connection: &str, engine: Engine, statement: &str, at: u64) {
    state.histories.record(
        &ConnectionName::try_new(connection).expect("a valid name"),
        Entry {
            at: Some(Stamp::from_unix(at)),
            ..Entry::bare(statement, engine)
        },
        state.config.history.limit,
    );
}

fn run_query(state: &mut State) -> Vec<Effect> {
    ctrl(state, Key::Char('x'))
}

fn statements(state: &State) -> Vec<&str> {
    state
        .history()
        .iter()
        .map(|entry| entry.statement.as_str())
        .collect()
}

fn buffer(state: &State) -> String {
    state.queries.text()
}

fn picker(state: &State) -> &Picker {
    let Some(Overlay::History(picker)) = &state.overlay else {
        panic!("the history is open");
    };
    picker
}

/// What the picker lists, top down.
fn listed(state: &State) -> Vec<String> {
    let Some(OverlayView::History(history)) = crate::view::view_model(state).overlay else {
        panic!("the history is drawn");
    };
    history.rows.into_iter().map(|row| row.statement).collect()
}

#[test]
fn a_statement_run_from_the_buffer_is_remembered() {
    let state = having_run(&["SELECT 1;", "SELECT 2;"]);

    assert_eq!(statements(&state), ["SELECT 2;", "SELECT 1;"]);
}

#[test]
fn a_remembered_statement_carries_its_time_engine_and_database() {
    let mut state = editing("SELECT 1;");
    state.geometry = roomy();
    update(&mut state, Msg::Tick(Stamp::from_unix(1_764_153_015)));
    if let ConnectionState::Connected(connection) = &mut state.connection
        && let SchemaState::Loaded(catalog) = &mut connection.schema
    {
        catalog.server = Some(Server {
            current: "shop".to_owned(),
            databases: vec!["shop".to_owned()],
        });
    }

    run_query(&mut state);

    assert_eq!(
        state.history(),
        [Entry {
            statement: "SELECT 1;".to_owned(),
            at: Some(Stamp::from_unix(1_764_153_015)),
            engine: Engine::Postgres,
            database: Some("shop".to_owned()),
        }]
    );
}

#[test]
fn a_table_read_from_the_tree_is_not_remembered() {
    let mut state = browsing_tables(&[("shop", &["orders"])]);
    press(&mut state, Key::Char('l'));
    press(&mut state, Key::Char('l'));
    press(&mut state, Key::Enter);

    assert!(state.history().is_empty());
}

#[test]
fn the_configured_limit_bounds_what_is_kept() {
    let mut state = editing("");
    state.config.history = HistoryConfig {
        limit: HistoryLimit::try_new(2).expect("in range"),
    };

    for statement in ["SELECT 1;", "SELECT 2;", "SELECT 3;"] {
        state.focus_on(Panel::Queries);
        state.queries.replace(statement);
        run_query(&mut state);
    }

    assert_eq!(statements(&state), ["SELECT 3;", "SELECT 2;"]);
}

fn saves(effects: &[Effect]) -> Vec<Vec<&str>> {
    effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::SaveHistory { entries, .. } => Some(
                entries
                    .iter()
                    .map(|entry| entry.statement.as_str())
                    .collect(),
            ),
            _ => None,
        })
        .collect()
}

#[test]
fn a_run_saves_the_whole_list_for_its_connection() {
    let mut state = having_run(&["SELECT 1;"]);
    state.focus_on(Panel::Queries);
    state.queries.replace("SELECT 2;");

    let effects = run_query(&mut state);

    assert_eq!(saves(&effects), [vec!["SELECT 2;", "SELECT 1;"]]);
    assert!(effects.iter().any(|effect| matches!(
        effect,
        Effect::SaveHistory { connection, .. } if connection.as_str() == "local"
    )));
}

#[test]
fn running_the_newest_statement_again_in_the_same_second_saves_nothing() {
    let mut state = having_run(&["SELECT 1;"]);
    state.focus_on(Panel::Queries);
    state.queries.replace("SELECT 1;");

    let effects = run_query(&mut state);

    assert!(saves(&effects).is_empty());
}

#[test]
fn running_the_newest_statement_again_later_saves_its_new_time() {
    let mut state = having_run(&["SELECT 1;"]);
    update(&mut state, Msg::Tick(Stamp::from_unix(60)));
    state.focus_on(Panel::Queries);
    state.queries.replace("SELECT 1;");

    let effects = run_query(&mut state);

    assert_eq!(saves(&effects), [vec!["SELECT 1;"]]);
    assert_eq!(state.history()[0].at, Some(Stamp::from_unix(60)));
}

#[test]
fn a_statement_the_posture_refused_is_neither_remembered_nor_saved() {
    let mut state = having_run(&[]);
    state
        .queries
        .replace("DELETE FROM shop.orders WHERE id = 7;");

    let effects = run_query(&mut state);

    assert!(saves(&effects).is_empty());
    assert!(state.history().is_empty());
}

#[test]
fn a_failed_save_lands_in_the_trail_and_not_in_a_modal() {
    let mut state = having_run(&[]);
    let before = state.log.len();

    update(
        &mut state,
        Msg::HistorySaveFailed {
            reason: "/x/history.toml: permission denied".to_owned(),
        },
    );

    assert_eq!(state.log.len(), before + 1);
    assert!(state.overlay.is_none());
}

#[test]
fn ctrl_r_opens_the_history_on_the_connections_newest_entry() {
    let mut state = having_run(&["SELECT 1;", "SELECT 2;"]);

    open_history(&mut state);

    assert_eq!(picker(&state).layer, Layer::Default);
    assert_eq!(picker(&state).cursor.index(), 0);
    assert_eq!(listed(&state), ["SELECT 2;", "SELECT 1;"]);
}

#[test]
fn ctrl_r_opens_nothing_with_no_history() {
    let mut state = having_run(&[]);

    open_history(&mut state);

    assert!(state.overlay.is_none());
}

#[test]
fn with_nothing_on_this_connection_ctrl_r_opens_on_the_engines_layer() {
    let mut state = having_run(&[]);
    ran_elsewhere(&mut state, "staging", Engine::Postgres, "SELECT 1;", 1);

    open_history(&mut state);

    assert_eq!(picker(&state).layer, Layer::Engine);
    assert_eq!(listed(&state), ["SELECT 1;"]);
}

#[test]
fn ctrl_r_opens_nothing_where_the_frame_cannot_draw_it() {
    let mut state = having_run(&["SELECT 1;"]);
    state.geometry = Geometry::new([8, 14, 20, 1], 37, 14);

    open_history(&mut state);

    assert!(state.overlay.is_none());
}

#[test]
fn ctrl_r_belongs_to_queries() {
    let mut state = having_run(&["SELECT 1;"]);
    state.focus_on(Panel::Objects);

    open_history(&mut state);

    assert!(state.overlay.is_none());
}

#[test]
fn ctrl_r_while_open_widens_the_layer_and_wraps() {
    let mut state = having_run(&["SELECT 'local';"]);
    ran_elsewhere(
        &mut state,
        "staging",
        Engine::Postgres,
        "SELECT 'staging';",
        2,
    );
    ran_elsewhere(
        &mut state,
        "events",
        Engine::Cassandra,
        "SELECT 'events';",
        3,
    );
    open_history(&mut state);

    assert_eq!(listed(&state), ["SELECT 'local';"]);

    open_history(&mut state);
    assert_eq!(picker(&state).layer, Layer::Saved);
    assert!(listed(&state).is_empty());

    open_history(&mut state);
    assert_eq!(picker(&state).layer, Layer::Connection);
    assert_eq!(listed(&state), ["SELECT 'local';"]);

    open_history(&mut state);
    assert_eq!(picker(&state).layer, Layer::Engine);
    assert_eq!(listed(&state), ["SELECT 'staging';", "SELECT 'local';"]);

    open_history(&mut state);
    assert_eq!(picker(&state).layer, Layer::All);
    assert_eq!(
        listed(&state),
        ["SELECT 'events';", "SELECT 'staging';", "SELECT 'local';"]
    );

    open_history(&mut state);
    assert_eq!(picker(&state).layer, Layer::Default);
}

#[test]
fn widening_returns_the_cursor_to_the_top() {
    let mut state = having_run(&["SELECT 1;", "SELECT 2;"]);
    open_history(&mut state);
    press(&mut state, Key::Down);

    open_history(&mut state);

    assert_eq!(picker(&state).cursor.index(), 0);
}

#[test]
fn a_wider_layer_names_the_connection_each_row_ran_on() {
    let mut state = having_run(&["SELECT 1;"]);
    ran_elsewhere(&mut state, "staging", Engine::Postgres, "SELECT 2;", 0);
    for _ in [
        Layer::Default,
        Layer::Saved,
        Layer::Connection,
        Layer::Engine,
    ] {
        open_history(&mut state);
    }

    let Some(OverlayView::History(history)) = crate::view::view_model(&state).overlay else {
        panic!("the history is drawn");
    };

    let places: Vec<&str> = history.rows.iter().map(|row| row.place.as_str()).collect();
    assert_eq!(places, ["local", "staging"]);
    assert_eq!(history.layers[2], "local");
    assert_eq!(history.layers[3], "PostgreSQL");
}

#[test]
fn each_row_says_how_long_ago_it_ran() {
    let mut state = having_run(&["SELECT 1;"]);
    update(&mut state, Msg::Tick(Stamp::from_unix(3 * 3_600)));
    open_history(&mut state);

    let Some(OverlayView::History(history)) = crate::view::view_model(&state).overlay else {
        panic!("the history is drawn");
    };

    assert_eq!(history.rows[0].age, "3h");
}

#[test]
fn typing_filters_at_once_and_fuzzily() {
    let mut state = having_run(&[
        "SELECT * FROM shop.orders;",
        "SELECT * FROM shop.users;",
        "DELETE FROM shop.orders WHERE id = 1;",
    ]);
    open_history(&mut state);

    type_into(&mut state, "sel ordrs");

    assert_eq!(listed(&state), ["SELECT * FROM shop.orders;"]);
    assert_eq!(picker(&state).query.text(), "sel ordrs");
}

#[test]
fn q_is_a_letter_of_the_query_not_a_way_out() {
    let mut state = having_run(&["SELECT q FROM t;"]);
    open_history(&mut state);

    let effects = press(&mut state, Key::Char('q'));

    assert!(effects.is_empty());
    assert_eq!(picker(&state).query.text(), "q");
}

#[test]
fn backspace_widens_the_filter_again() {
    let mut state = having_run(&["SELECT 1 FROM users;", "SELECT 2 FROM orders;"]);
    open_history(&mut state);
    type_into(&mut state, "users");

    for _ in 0.."users".len() {
        press(&mut state, Key::Backspace);
    }

    assert_eq!(listed(&state).len(), 2);
}

#[test]
fn a_changed_query_returns_the_cursor_to_the_best_hit() {
    let mut state = having_run(&["SELECT 1 FROM users;", "SELECT 2 FROM orders;"]);
    open_history(&mut state);
    press(&mut state, Key::Down);

    type_into(&mut state, "from");

    assert_eq!(picker(&state).cursor.index(), 0);
}

#[test]
fn enter_appends_the_best_hit() {
    let mut state = having_run(&["SELECT 1 FROM users;", "SELECT 2 FROM orders;"]);
    open_history(&mut state);
    type_into(&mut state, "users");

    press(&mut state, Key::Enter);

    assert!(state.overlay.is_none());
    assert_eq!(buffer(&state), "SELECT 1 FROM users;");
}

#[test]
fn enter_with_nothing_matched_keeps_the_picker_open() {
    let mut state = having_run(&["SELECT 1;"]);
    open_history(&mut state);
    type_into(&mut state, "zzz");

    press(&mut state, Key::Enter);

    assert!(matches!(state.overlay, Some(Overlay::History(_))));
    assert_eq!(buffer(&state), "");
}

#[test]
fn enter_loads_the_entry_into_the_buffer_and_runs_nothing() {
    let mut state = having_run(&["SELECT 1;", "SELECT 2;"]);
    open_history(&mut state);

    let effects = press(&mut state, Key::Enter);

    assert!(effects.is_empty(), "loading is not running");
    assert!(state.overlay.is_none());
    assert_eq!(buffer(&state), "SELECT 2;");
}

#[test]
fn enter_appends_the_entry_after_what_the_buffer_holds() {
    let mut state = having_run(&["SELECT 1;", "SELECT 2;"]);
    state.queries.replace("SELECT half");
    open_history(&mut state);

    press(&mut state, Key::Enter);

    assert_eq!(buffer(&state), "SELECT half\n\nSELECT 2;");
}

#[test]
fn the_arrows_walk_back_through_older_entries() {
    let mut state = having_run(&["SELECT 1;", "SELECT 2;", "SELECT 3;"]);
    open_history(&mut state);
    press(&mut state, Key::Down);
    press(&mut state, Key::Down);
    press(&mut state, Key::Up);

    press(&mut state, Key::Enter);

    assert_eq!(buffer(&state), "SELECT 2;");
}

#[test]
fn ctrl_n_and_ctrl_p_walk_as_in_a_shell() {
    let mut state = having_run(&["SELECT 1;", "SELECT 2;", "SELECT 3;"]);
    open_history(&mut state);
    ctrl(&mut state, Key::Char('n'));
    ctrl(&mut state, Key::Char('n'));
    ctrl(&mut state, Key::Char('p'));

    assert_eq!(picker(&state).cursor.index(), 1);
}

#[test]
fn ctrl_x_in_the_picker_runs_nothing() {
    let mut state = having_run(&["SELECT 1;"]);
    open_history(&mut state);

    let effects = run_query(&mut state);

    assert!(effects.is_empty());
    assert!(matches!(state.overlay, Some(Overlay::History(_))));
}

#[test]
fn a_loaded_entry_can_be_run_straight_away() {
    let mut state = having_run(&["SELECT 1;"]);
    open_history(&mut state);
    press(&mut state, Key::Enter);
    state.focus_on(Panel::Queries);

    let effects = run_query(&mut state);

    assert!(
        effects
            .iter()
            .any(|effect| matches!(effect, Effect::ExecutePaged { .. }))
    );
}

#[test]
fn esc_closes_it_and_leaves_the_buffer_alone() {
    let mut state = having_run(&["SELECT 1;"]);
    state.queries.replace("SELECT half");
    open_history(&mut state);
    type_into(&mut state, "sel");

    press(&mut state, Key::Escape);

    assert!(state.overlay.is_none());
    assert_eq!(buffer(&state), "SELECT half");
}

#[test]
fn a_multi_line_entry_is_listed_on_one_line_and_loaded_whole() {
    let mut state = having_run(&["SELECT *\nFROM shop.orders;"]);
    open_history(&mut state);

    assert_eq!(listed(&state), ["SELECT * FROM shop.orders;"]);

    press(&mut state, Key::Enter);

    assert_eq!(buffer(&state), "SELECT *\nFROM shop.orders;");
}

#[test]
fn the_bar_names_ctrl_r_only_once_there_is_something_to_open() {
    let empty = having_run(&[]);
    let kept = having_run(&["SELECT 1;"]);
    let mut elsewhere = having_run(&[]);
    ran_elsewhere(&mut elsewhere, "staging", Engine::Postgres, "SELECT 1;", 0);

    let named = |state: &State| bar(state).panel.iter().any(|hint| hint.action == "history");

    assert!(!named(&empty));
    assert!(named(&kept));
    assert!(named(&elsewhere));
}

#[test]
fn the_open_picker_names_its_own_keys() {
    let mut state = having_run(&["SELECT 1;"]);
    open_history(&mut state);

    let hints = bar(&state);
    let actions: Vec<&str> = hints
        .panel
        .iter()
        .chain(&hints.global)
        .map(|hint| hint.action)
        .collect();

    assert_eq!(actions, ["layer", "append", "close"]);
}

fn disabled() -> State {
    let mut state = having_run(&[]);
    state.config.history = HistoryConfig {
        limit: HistoryLimit::try_new(0).expect("in range"),
    };
    state
}

#[test]
fn a_zero_limit_records_nothing_and_saves_nothing() {
    let mut state = disabled();
    state.queries.replace("SELECT 1;");

    let effects = run_query(&mut state);

    assert!(state.history().is_empty());
    assert!(saves(&effects).is_empty());
}

#[test]
fn ctrl_r_with_the_history_disabled_and_nothing_saved_toasts_instead_of_opening() {
    let mut state = disabled();

    open_history(&mut state);

    assert!(state.overlay.is_none());
    assert_eq!(
        state.toast.as_ref().map(crate::feedback::Toast::message),
        Some("nothing saved, and the history is off")
    );
}

#[test]
fn the_toast_leaves_after_its_ticks() {
    let mut state = disabled();
    open_history(&mut state);

    for _ in 0..crate::feedback::TOAST_TICKS {
        assert!(state.toast.is_some());
        update(&mut state, Msg::Tick(Stamp::default()));
    }

    assert!(state.toast.is_none());
}

#[test]
fn ctrl_r_outside_queries_toasts_nothing() {
    let mut state = disabled();
    state.focus_on(Panel::Objects);

    open_history(&mut state);

    assert!(state.toast.is_none());
}

#[test]
fn ctrl_r_with_nothing_remembered_toasts_instead_of_opening() {
    let mut state = State::default();
    state.geometry = roomy();
    state.focus_on(Panel::Queries);

    open_history(&mut state);

    assert!(state.overlay.is_none());
    assert_eq!(
        toasted(&state),
        Some((Severity::Info, "nothing in the history yet"))
    );
}

#[test]
fn a_failed_history_save_warns() {
    let mut state = State::default();

    update(
        &mut state,
        Msg::HistorySaveFailed {
            reason: "disk full".to_owned(),
        },
    );

    assert_eq!(
        toasted(&state),
        Some((Severity::Warning, "history not saved, see the logs"))
    );
}
