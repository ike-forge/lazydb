//! Naming the statement under the cursor: `ctrl-s` in the buffer, `⏎` saves,
//! a taken name asks before it overwrites.

use crate::model::Key;
use crate::overlay::Overlay;
use crate::panel::Panel;
use crate::queries::QueriesTab;
use crate::saved::{QueryName, SaveForm};
use crate::state::State;
use crate::style::Severity;

use super::keys::edit_line;
use super::run::entry_now;
use super::{Effect, escape};

/// Whether `ctrl-s` is this form's rather than the commit's.
pub(super) fn saves_here(state: &State) -> bool {
    state.focus() == Panel::Queries && state.queries.tab() == QueriesTab::Sql
}

pub(super) fn open_save(state: &mut State) {
    let Some(connection) = state.connected_name().cloned() else {
        state.notify(Severity::Info, "connect to save a query");
        return;
    };
    let Some(statement) = state.queries.statement() else {
        state.notify(Severity::Info, "no statement to save");
        return;
    };
    if !state.geometry.save_fits() {
        state.notify(Severity::Info, "no room to draw the form");
        return;
    }

    let entry = entry_now(state, statement.trim());
    state.overlay = Some(Overlay::Save(SaveForm::opened(connection, entry)));
}

pub(super) fn save_key(state: &mut State, key: Key) -> Vec<Effect> {
    let Some(Overlay::Save(form)) = &mut state.overlay else {
        return Vec::new();
    };

    if let Some(taken) = form.taken().cloned() {
        return match key {
            Key::Char('y') => save(state, taken),
            Key::Char('n') => {
                form.back_to_name();
                Vec::new()
            }
            Key::Escape => {
                escape(state);
                Vec::new()
            }
            _ => Vec::new(),
        };
    }

    match key {
        Key::Enter => {
            let Some(name) = form.valid_name() else {
                return Vec::new();
            };
            if state.saved.find(form.connection(), &name).is_some() {
                form.ask_overwrite(name);
                return Vec::new();
            }
            save(state, name)
        }
        Key::Escape => {
            escape(state);
            Vec::new()
        }
        _ => {
            if let Some(line) = form.name_mut() {
                edit_line(line, key);
            }
            Vec::new()
        }
    }
}

fn save(state: &mut State, name: QueryName) -> Vec<Effect> {
    let Some(Overlay::Save(form)) = state.overlay.take() else {
        return Vec::new();
    };
    let (connection, mut query) = form.into_query(name);
    query.entry.at = Some(state.now);

    state.notify(
        Severity::Info,
        format!("saved {} for {}", query.name.as_str(), connection.as_str()),
    );
    state.saved.save(&connection, query);

    vec![Effect::SaveQueries {
        queries: state.saved.of(&connection).to_vec(),
        connection,
    }]
}
