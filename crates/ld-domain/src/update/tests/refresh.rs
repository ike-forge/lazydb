use super::*;

fn on_objects() -> State {
    let mut state = browsing_tables(&[("shop", &["orders"])]);
    state.focus_on(Panel::Objects);
    state
}

fn showing(subject: Subject) -> State {
    let mut state = browsing_tables(&[("shop", &["orders"])]);
    let query = QueryId::next();
    state.contents.requested(
        query,
        subject,
        Statement::typed(crate::Engine::Postgres, "SELECT 1"),
    );
    state.contents.page_ready(
        query,
        page_of(2),
        state.config.grid.max_column_width,
        QueryId::next,
    );
    state.focus_on(Panel::Contents);
    state
}

fn reads(effects: &[Effect]) -> Vec<String> {
    effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::ExecutePaged { stmt, .. } => Some(stmt.sql.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn r_on_objects_rereads_the_catalog() {
    let mut state = on_objects();
    let conn = current_conn(&state);

    let effects = press(&mut state, Key::Char('r'));

    assert_eq!(effects, vec![Effect::LoadSchema(conn)]);
    assert!(state.log.is_pending(crate::log::Trace::Catalog(conn)));
    assert!(state.schema().is_some(), "the old tree stays up meanwhile");
}

#[test]
fn a_second_r_waits_on_the_catalog_already_coming() {
    let mut state = on_objects();
    press(&mut state, Key::Char('r'));

    let effects = press(&mut state, Key::Char('r'));

    assert!(effects.is_empty());
    assert_eq!(
        toasted(&state),
        Some((Severity::Info, "already reading the schema"))
    );
}

#[test]
fn the_refreshed_catalog_replaces_the_tree() {
    let mut state = on_objects();
    let conn = current_conn(&state);
    press(&mut state, Key::Char('r'));

    update(
        &mut state,
        Msg::SchemaLoaded {
            conn,
            schema: Box::new(tabled_schema(&[("shop", &["orders", "invoices"])])),
            took: Duration::ZERO,
        },
    );

    let shop = &state.schema().expect("loaded").schemas["shop"];
    assert!(shop.tables.contains_key("invoices"));
    assert!(!state.log.is_pending(crate::log::Trace::Catalog(conn)));
}

#[test]
fn r_with_no_connection_does_nothing() {
    let mut state = State::default();

    assert!(press(&mut state, Key::Char('r')).is_empty());
}

#[test]
fn r_on_contents_rereads_the_table() {
    let mut state = showing(Subject::Table(TableRef::new("shop", "orders")));

    let effects = press(&mut state, Key::Char('r'));

    let sql = reads(&effects);
    assert_eq!(sql.len(), 1, "one read: {effects:?}");
    assert!(sql[0].contains("orders"), "{sql:?}");
    assert!(matches!(
        state.contents.state(),
        ContentsState::Loading {
            subject: Subject::Table(_),
            ..
        }
    ));
    assert_eq!(state.focus(), Panel::Contents);
}

#[test]
fn r_on_contents_reruns_a_typed_read_without_recording_it() {
    let sql = "SELECT * FROM shop.orders WHERE id > 2";
    let mut state = showing(Subject::Typed(sql.to_owned()));

    let effects = press(&mut state, Key::Char('r'));

    assert_eq!(reads(&effects), vec![sql.to_owned()]);
    assert!(
        !effects
            .iter()
            .any(|effect| matches!(effect, Effect::SaveHistory { .. })),
        "{effects:?}"
    );
}

#[test]
fn r_does_not_rerun_a_write() {
    let mut state = showing(Subject::Typed(
        "DELETE FROM shop.orders WHERE id = 1".to_owned(),
    ));

    let effects = press(&mut state, Key::Char('r'));

    assert!(effects.is_empty(), "{effects:?}");
    assert_eq!(
        toasted(&state).map(|(severity, _)| severity),
        Some(Severity::Warning)
    );
}

#[test]
fn r_keeps_uncommitted_edits() {
    let mut state = committable(1);
    state.focus_on(Panel::Contents);

    let effects = press(&mut state, Key::Char('r'));

    assert!(effects.is_empty(), "{effects:?}");
    assert_eq!(dirty_cells(&state), 1);
    assert_eq!(
        toasted(&state),
        Some((
            Severity::Warning,
            "uncommitted edits, ctrl-s to commit them first"
        ))
    );
}

#[test]
fn r_over_an_empty_grid_does_nothing() {
    let mut state = browsing_tables(&[("shop", &["orders"])]);
    state.focus_on(Panel::Contents);

    assert!(press(&mut state, Key::Char('r')).is_empty());
}

fn refresh_hint(state: &State) -> Option<crate::view::Liveness> {
    crate::view::hints::bar(state)
        .panel
        .into_iter()
        .find(|hint| hint.action == "refresh")
        .map(|hint| hint.liveness)
}

#[test]
fn the_refresh_hint_greys_while_the_catalog_is_coming() {
    let mut state = on_objects();
    assert_eq!(refresh_hint(&state), Some(crate::view::Liveness::Live));

    press(&mut state, Key::Char('r'));

    assert_eq!(refresh_hint(&state), Some(crate::view::Liveness::Inert));
}

#[test]
fn the_refresh_hint_greys_over_a_write() {
    let read = showing(Subject::Table(TableRef::new("shop", "orders")));
    let write = showing(Subject::Typed("DELETE FROM shop.orders".to_owned()));

    assert_eq!(refresh_hint(&read), Some(crate::view::Liveness::Live));
    assert_eq!(refresh_hint(&write), Some(crate::view::Liveness::Inert));
}
