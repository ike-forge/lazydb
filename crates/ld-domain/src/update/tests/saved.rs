use super::*;
use crate::Engine;
use crate::config::{HistoryConfig, HistoryLimit};
use crate::history::Layer;
use crate::saved::{QueryName, SaveForm};
use crate::view::SaveView;
use crate::view::hints::bar;

fn roomy() -> Geometry {
    Geometry::new([8, 14, 20, 1], 37, 14)
        .with_history_rows(10)
        .with_save_fits(true)
}

/// Editing `text` on `local`, with room for the form and the picker.
fn writing(text: &str) -> State {
    let mut state = editing(text);
    state.geometry = roomy();
    state
}

fn save_key(state: &mut State) -> Vec<Effect> {
    ctrl(state, Key::Char('s'))
}

fn form(state: &State) -> &SaveForm {
    let Some(Overlay::Save(form)) = &state.overlay else {
        panic!("the save form is up");
    };
    form
}

fn drawn(state: &State) -> SaveView {
    let Some(OverlayView::Save(view)) = crate::view::view_model(state).overlay else {
        panic!("the save form is drawn");
    };
    *view
}

/// Names the statement under the cursor `name` and presses `⏎`.
fn save_as(state: &mut State, name: &str) -> Vec<Effect> {
    save_key(state);
    type_into(state, name);
    press(state, Key::Enter)
}

fn saved_on_local(state: &State) -> Vec<(String, String)> {
    state
        .saved
        .of(&ConnectionName::try_new("local").expect("a valid name"))
        .iter()
        .map(|query| {
            (
                query.name.as_str().to_owned(),
                query.entry.statement.clone(),
            )
        })
        .collect()
}

fn pairs(of: &[(&str, &str)]) -> Vec<(String, String)> {
    of.iter()
        .map(|(name, statement)| ((*name).to_owned(), (*statement).to_owned()))
        .collect()
}

#[test]
fn ctrl_s_in_the_buffer_opens_the_form_on_the_statement_under_the_cursor() {
    let mut state = writing("SELECT 1;\n\nSELECT\n  2;");

    save_key(&mut state);

    let view = drawn(&state);
    assert_eq!(view.connection, "local");
    assert_eq!(view.name, "");
    assert_eq!(view.sql, ["SELECT", "  2;"]);
    assert!(!view.valid, "a blank name cannot be saved");
}

#[test]
fn ctrl_s_works_while_browsing_too() {
    let mut state = writing("SELECT 1;");
    press(&mut state, Key::Escape);

    save_key(&mut state);

    assert_eq!(form(&state).statement(), "SELECT 1;");
}

#[test]
fn enter_saves_under_the_typed_name_and_writes_the_list() {
    let mut state = writing("SELECT 1;");
    update(&mut state, Msg::Tick(Stamp::from_unix(42)));

    let effects = save_as(&mut state, "  one ");

    assert!(state.overlay.is_none());
    assert_eq!(saved_on_local(&state), pairs(&[("one", "SELECT 1;")]));
    assert_eq!(
        state
            .saved
            .of(&ConnectionName::try_new("local").expect("valid"))[0]
            .entry
            .at,
        Some(Stamp::from_unix(42))
    );
    assert!(matches!(
        effects.as_slice(),
        [Effect::SaveQueries { connection, queries }]
            if connection.as_str() == "local" && queries.len() == 1
    ));
    assert_eq!(
        toasted(&state),
        Some((Severity::Info, "saved one for local"))
    );
}

#[test]
fn enter_on_a_blank_name_does_nothing() {
    let mut state = writing("SELECT 1;");

    let effects = save_as(&mut state, "   ");

    assert!(effects.is_empty());
    assert!(matches!(state.overlay, Some(Overlay::Save(_))));
    assert!(state.saved.is_empty());
}

#[test]
fn every_letter_is_the_names_even_q() {
    let mut state = writing("SELECT 1;");
    save_key(&mut state);

    type_into(&mut state, "quit");

    assert_eq!(drawn(&state).name, "quit");
    assert!(!state.should_quit);
}

#[test]
fn esc_closes_the_form_and_saves_nothing() {
    let mut state = writing("SELECT 1;");
    save_key(&mut state);
    type_into(&mut state, "one");

    let effects = press(&mut state, Key::Escape);

    assert!(effects.is_empty());
    assert!(state.overlay.is_none());
    assert!(state.saved.is_empty());
}

#[test]
fn a_taken_name_asks_before_it_overwrites_whatever_its_case() {
    let mut state = writing("SELECT 1;");
    save_as(&mut state, "one");
    state.queries.replace("SELECT 2;");

    let effects = save_as(&mut state, "ONE");

    assert!(effects.is_empty());
    assert_eq!(drawn(&state).taken.as_deref(), Some("ONE"));
    assert_eq!(saved_on_local(&state), pairs(&[("one", "SELECT 1;")]));
}

#[test]
fn y_overwrites_the_taken_name() {
    let mut state = writing("SELECT 1;");
    save_as(&mut state, "one");
    state.queries.replace("SELECT 2;");
    save_as(&mut state, "one");

    let effects = press(&mut state, Key::Char('y'));

    assert!(state.overlay.is_none());
    assert_eq!(saved_on_local(&state), pairs(&[("one", "SELECT 2;")]));
    assert!(matches!(effects.as_slice(), [Effect::SaveQueries { .. }]));
}

#[test]
fn n_goes_back_to_the_name_as_typed() {
    let mut state = writing("SELECT 1;");
    save_as(&mut state, "one");
    state.queries.replace("SELECT 2;");
    save_as(&mut state, "one");

    press(&mut state, Key::Char('n'));
    type_into(&mut state, "2");

    assert_eq!(drawn(&state).taken, None);
    assert_eq!(drawn(&state).name, "one2");
}

#[test]
fn esc_on_the_question_closes_the_form() {
    let mut state = writing("SELECT 1;");
    save_as(&mut state, "one");
    state.queries.replace("SELECT 2;");
    save_as(&mut state, "one");

    press(&mut state, Key::Escape);

    assert!(state.overlay.is_none());
    assert_eq!(saved_on_local(&state), pairs(&[("one", "SELECT 1;")]));
}

#[test]
fn the_question_takes_no_text() {
    let mut state = writing("SELECT 1;");
    save_as(&mut state, "one");
    state.queries.replace("SELECT 2;");
    save_as(&mut state, "one");

    press(&mut state, Key::Char('x'));

    assert_eq!(drawn(&state).taken.as_deref(), Some("one"));
    assert_eq!(drawn(&state).name, "one");
}

#[test]
fn the_same_name_on_another_connection_is_not_taken() {
    let mut state = writing("SELECT 1;");
    state.saved.save(
        &ConnectionName::try_new("staging").expect("a valid name"),
        crate::saved::SavedQuery {
            name: QueryName::try_new("one").expect("a valid name"),
            entry: crate::history::Entry::bare("SELECT 9;", Engine::Postgres),
        },
    );

    let effects = save_as(&mut state, "one");

    assert!(state.overlay.is_none());
    assert!(!effects.is_empty());
}

#[test]
fn without_a_connection_ctrl_s_says_why_and_opens_nothing() {
    let mut state = writing("SELECT 1;");
    state.connection = ConnectionState::Disconnected;

    save_key(&mut state);

    assert!(state.overlay.is_none());
    assert_eq!(
        toasted(&state),
        Some((Severity::Info, "connect to save a query"))
    );
}

#[test]
fn an_empty_buffer_has_nothing_to_save() {
    let mut state = writing("");

    save_key(&mut state);

    assert!(state.overlay.is_none());
    assert_eq!(
        toasted(&state),
        Some((Severity::Info, "no statement to save"))
    );
}

#[test]
fn a_frame_too_small_for_the_form_opens_nothing() {
    let mut state = writing("SELECT 1;");
    state.geometry = state.geometry.with_save_fits(false);

    save_key(&mut state);

    assert!(state.overlay.is_none());
}

#[test]
fn ctrl_s_on_the_logs_tab_is_still_the_commit() {
    let mut state = writing("SELECT 1;");
    press(&mut state, Key::Escape);
    state.queries.show(QueriesTab::Logs);

    save_key(&mut state);

    assert!(!matches!(state.overlay, Some(Overlay::Save(_))));
}

#[test]
fn the_form_names_its_keys_and_then_the_questions() {
    let mut state = writing("SELECT 1;");
    save_as(&mut state, "one");
    state.queries.replace("SELECT 2;");
    save_key(&mut state);

    let actions = |state: &State| -> Vec<&'static str> {
        let hints = bar(state);
        hints
            .panel
            .iter()
            .chain(&hints.global)
            .map(|hint| hint.action)
            .collect()
    };

    assert_eq!(actions(&state), ["save", "cancel"]);

    type_into(&mut state, "one");
    press(&mut state, Key::Enter);

    assert_eq!(actions(&state), ["overwrite", "rename", "cancel"]);
}

#[test]
fn the_bar_offers_ctrl_s_in_the_buffer_once_connected() {
    let connected = writing("SELECT 1;");
    let mut away = writing("SELECT 1;");
    away.connection = ConnectionState::Disconnected;

    let offered = |state: &State| bar(state).panel.iter().any(|hint| hint.action == "save");

    assert!(offered(&connected));
    assert!(!offered(&away));
}

#[test]
fn a_failed_write_warns_and_leaves_its_reason_in_the_trail() {
    let mut state = State::default();
    let before = state.log.len();

    update(
        &mut state,
        Msg::SavedQueriesFailed {
            reason: "disk full".to_owned(),
        },
    );

    assert_eq!(state.log.len(), before + 1);
    assert_eq!(
        toasted(&state),
        Some((Severity::Warning, "saved queries not written, see the logs"))
    );
}

fn picked(state: &State) -> Vec<(String, String)> {
    let Some(OverlayView::History(history)) = crate::view::view_model(state).overlay else {
        panic!("the history is drawn");
    };
    history
        .rows
        .into_iter()
        .map(|row| (row.name, row.statement))
        .collect()
}

#[test]
fn ctrl_r_opens_on_the_saved_queries_then_the_history() {
    let mut state = writing("SELECT 1;");
    ctrl(&mut state, Key::Char('x'));
    state.focus_on(Panel::Queries);
    state.queries.replace("SELECT 2;");
    save_as(&mut state, "two");
    state.queries.replace("");

    ctrl(&mut state, Key::Char('r'));

    assert_eq!(
        picked(&state),
        pairs(&[("two", "SELECT 2;"), ("", "SELECT 1;")])
    );
}

#[test]
fn a_statement_both_run_and_saved_is_listed_once_by_name() {
    let mut state = writing("SELECT 1;");
    ctrl(&mut state, Key::Char('x'));
    state.focus_on(Panel::Queries);
    save_as(&mut state, "one");

    ctrl(&mut state, Key::Char('r'));

    assert_eq!(picked(&state), pairs(&[("one", "SELECT 1;")]));
}

#[test]
fn typing_in_the_picker_finds_a_query_by_its_name() {
    let mut state = writing("SELECT 1;");
    save_as(&mut state, "pending orders");
    state.queries.replace("SELECT * FROM orders;");
    save_as(&mut state, "all");
    state.queries.replace("");

    ctrl(&mut state, Key::Char('r'));
    type_into(&mut state, "pend");

    assert_eq!(picked(&state), pairs(&[("pending orders", "SELECT 1;")]));
}

#[test]
fn enter_in_the_picker_appends_a_saved_query() {
    let mut state = writing("SELECT 1;");
    save_as(&mut state, "one");
    state.queries.replace("");

    ctrl(&mut state, Key::Char('r'));
    press(&mut state, Key::Enter);

    assert_eq!(state.queries.text(), "SELECT 1;");
}

#[test]
fn the_history_off_still_recalls_saved_queries() {
    let mut state = writing("SELECT 1;");
    state.config.history = HistoryConfig {
        limit: HistoryLimit::try_new(0).expect("in range"),
    };
    save_as(&mut state, "one");

    ctrl(&mut state, Key::Char('r'));

    let Some(Overlay::History(picker)) = &state.overlay else {
        panic!("the history is open");
    };
    assert_eq!(picker.layer, Layer::Default);
}

fn delete_key(state: &mut State) -> Vec<Effect> {
    press_binding(state, KeyPress::alt(Key::Char('d')))
}

/// `one` and `two` saved, `SELECT 3;` run, the picker open on them.
fn picking() -> State {
    let mut state = writing("SELECT 1;");
    save_as(&mut state, "one");
    state.queries.replace("SELECT 2;");
    save_as(&mut state, "two");
    state.queries.replace("SELECT 3;");
    ctrl(&mut state, Key::Char('x'));
    state.focus_on(Panel::Queries);
    state.queries.replace("");
    ctrl(&mut state, Key::Char('r'));
    state
}

#[test]
fn alt_d_on_a_saved_query_asks_before_deleting() {
    let mut state = picking();

    let effects = delete_key(&mut state);

    assert!(effects.is_empty());
    assert_eq!(saved_on_local(&state).len(), 2);
    assert_eq!(
        toasted(&state),
        Some((Severity::Warning, "alt-d again deletes `two`"))
    );
}

#[test]
fn alt_d_twice_deletes_the_saved_query_and_writes_the_rest() {
    let mut state = picking();

    delete_key(&mut state);
    let effects = delete_key(&mut state);

    assert_eq!(saved_on_local(&state), pairs(&[("one", "SELECT 1;")]));
    assert!(matches!(
        effects.as_slice(),
        [Effect::SaveQueries { connection, queries }]
            if connection.as_str() == "local" && queries.len() == 1
    ));
    assert_eq!(
        picked(&state),
        pairs(&[("one", "SELECT 1;"), ("", "SELECT 3;")])
    );
    assert_eq!(toasted(&state), Some((Severity::Info, "deleted `two`")));
}

#[test]
fn any_other_key_between_takes_the_question_back() {
    let mut state = picking();

    delete_key(&mut state);
    press(&mut state, Key::Down);
    press(&mut state, Key::Up);
    let effects = delete_key(&mut state);

    assert!(effects.is_empty());
    assert_eq!(saved_on_local(&state).len(), 2);
}

#[test]
fn the_second_press_on_another_saved_query_asks_about_that_one() {
    let mut state = picking();
    delete_key(&mut state);
    if let Some(Overlay::History(picker)) = &mut state.overlay {
        picker.cursor.down(3);
    }

    let effects = delete_key(&mut state);

    assert!(effects.is_empty());
    assert_eq!(
        toasted(&state),
        Some((Severity::Warning, "alt-d again deletes `one`"))
    );
}

#[test]
fn the_bar_offers_alt_d_and_then_the_confirmation() {
    let mut state = picking();
    let offered = |state: &State| -> Option<(String, &'static str)> {
        bar(state)
            .panel
            .into_iter()
            .find(|hint| hint.keys == "alt-d")
            .map(|hint| (hint.keys, hint.action))
    };

    assert_eq!(offered(&state), Some(("alt-d".to_owned(), "delete")));

    delete_key(&mut state);

    assert_eq!(offered(&state), Some(("alt-d".to_owned(), "delete it")));
}

#[test]
fn deleting_the_last_saved_query_in_the_saved_layer_moves_to_the_next_layer() {
    let mut state = writing("SELECT 1;");
    save_as(&mut state, "one");
    state.queries.replace("SELECT 2;");
    ctrl(&mut state, Key::Char('x'));
    state.focus_on(Panel::Queries);
    state.queries.replace("");
    ctrl(&mut state, Key::Char('r'));
    ctrl(&mut state, Key::Char('r'));

    delete_key(&mut state);
    delete_key(&mut state);

    let Some(Overlay::History(picker)) = &state.overlay else {
        panic!("the history is open");
    };
    assert_eq!(picker.layer, Layer::Connection);
}
