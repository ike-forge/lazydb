//! The query history picker: typing filters at once, `ctrl-r` widens the
//! layer, `⏎` appends.

use crate::connection::ConnectionState;
use crate::history::{Layer, Picker, Ranked, ranked};
use crate::model::Key;
use crate::overlay::Overlay;
use crate::panel::Panel;
use crate::state::State;
use crate::style::Severity;

use super::keys::edit_line;
use super::{Effect, escape};

/// What the picker lists for `layer` and `query`; shared with the view.
pub(crate) fn listed<'a>(state: &'a State, layer: Layer, query: &str) -> Vec<Ranked<'a>> {
    let connection = match &state.connection {
        ConnectionState::Connected(connection) => Some(&connection.name),
        ConnectionState::Unchosen
        | ConnectionState::Disconnected
        | ConnectionState::Connecting { .. }
        | ConnectionState::Failed(_) => None,
    };

    ranked(
        state
            .histories
            .recall(layer, connection, state.queries.engine()),
        query,
    )
}

/// Refused with nothing to list, so the bar greys it instead. Opens on the
/// narrowest layer with anything in it.
pub(super) fn open_history(state: &mut State) {
    if state.focus() != Panel::Queries || matches!(state.overlay, Some(Overlay::Search(_))) {
        return;
    }
    if state.config.history.limit.is_off() {
        state.notify(Severity::Info, "history is disabled");
        return;
    }
    if state.geometry.history_rows() == 0 {
        state.notify(Severity::Info, "no room to draw the history");
        return;
    }
    let Some(layer) = Layer::ALL
        .into_iter()
        .find(|layer| !listed(state, *layer, "").is_empty())
    else {
        state.notify(Severity::Info, "nothing in the history yet");
        return;
    };

    state.overlay = Some(Overlay::History(Picker {
        layer,
        ..Picker::default()
    }));
}

/// `ctrl-r` while open: wraps back to the narrowest layer.
pub(super) fn widen(state: &mut State) {
    if let Some(Overlay::History(picker)) = &mut state.overlay {
        picker.layer = picker.layer.next();
        picker.cursor.to_start();
    }
}

/// `ctrl-n`/`ctrl-p` while open, as in a shell.
pub(super) fn step(state: &mut State, older: bool) {
    move_cursor(state, if older { Key::Down } else { Key::Up });
}

/// No `q`: it is a letter of the query.
pub(super) fn history_key(state: &mut State, key: Key) -> Vec<Effect> {
    match key {
        Key::Enter => append(state),
        Key::Escape => escape(state),
        Key::Up | Key::Down | Key::PageUp | Key::PageDown => move_cursor(state, key),
        Key::Char(_)
        | Key::Backspace
        | Key::Delete
        | Key::Left
        | Key::Right
        | Key::Home
        | Key::End => {
            if let Some(Overlay::History(picker)) = &mut state.overlay {
                let before = picker.query.text().to_owned();
                edit_line(&mut picker.query, key);
                if picker.query.text() != before {
                    picker.cursor.to_start();
                }
            }
        }
        _ => {}
    }

    Vec::new()
}

fn move_cursor(state: &mut State, key: Key) {
    let Some(Overlay::History(picker)) = &state.overlay else {
        return;
    };
    let len = listed(state, picker.layer, picker.query.text()).len();
    let height = state.geometry.history_rows();
    let Some(Overlay::History(picker)) = &mut state.overlay else {
        return;
    };
    let cursor = &mut picker.cursor;

    match key {
        Key::Down => cursor.down(len),
        Key::Up => cursor.up(),
        Key::PageDown => cursor.page_down(len, height),
        Key::PageUp => cursor.page_up(height),
        _ => {}
    }
}

/// Appends, not runs: `ctrl-x` is the one key that sends.
fn append(state: &mut State) {
    let Some(Overlay::History(picker)) = &state.overlay else {
        return;
    };
    let Some(statement) = listed(state, picker.layer, picker.query.text())
        .get(picker.cursor.index())
        .map(|ranked| ranked.recalled.entry.statement.clone())
    else {
        return;
    };

    state.overlay = None;
    state.queries.append(&statement);
}
