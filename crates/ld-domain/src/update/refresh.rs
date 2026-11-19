//! `r`: Objects re-reads the catalog, Contents re-runs the read on screen.

use crate::connection::ConnectionState;
use crate::contents::{ContentsState, Subject};
use crate::log::Trace;
use crate::model::{ConnId, Statement};
use crate::panel::Panel;
use crate::state::State;
use crate::style::Severity;

use super::Effect;
use super::commit::uncommitted;
use super::connections::read_schema;
use super::run::submit_into_contents;

pub(super) fn refresh(state: &mut State) -> Vec<Effect> {
    match state.focus() {
        Panel::Objects => refresh_schema(state),
        Panel::Contents => rerun(state),
        Panel::Structure | Panel::Queries => Vec::new(),
    }
}

/// `None` when there is no connection or its catalog is already on the way.
pub(crate) fn schema_refresh(state: &State) -> Option<ConnId> {
    let ConnectionState::Connected(connection) = &state.connection else {
        return None;
    };
    let conn = connection.id;

    (!state.log.is_pending(Trace::Catalog(conn))).then_some(conn)
}

fn refresh_schema(state: &mut State) -> Vec<Effect> {
    let Some(conn) = schema_refresh(state) else {
        if state.conn_id().is_some() {
            state.notify(Severity::Info, "already reading the schema");
        }
        return Vec::new();
    };

    state.notify(Severity::Info, "refreshing the schema");
    state.mark_dirty();

    read_schema(state, conn)
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Rerun {
    Read {
        subject: Subject,
        stmt: Statement,
    },
    /// A re-read discards staged edits.
    Edited,
    /// Run again only from the buffer, where it was typed.
    Write,
    Nothing,
}

pub(crate) fn rerun_of(state: &State) -> Rerun {
    let subject = match state.contents.state() {
        ContentsState::Empty => return Rerun::Nothing,
        ContentsState::Loading { subject, .. }
        | ContentsState::Loaded { subject, .. }
        | ContentsState::Failed { subject, .. } => subject.clone(),
    };
    if uncommitted(state).0 > 0 {
        return Rerun::Edited;
    }

    // The catalog may have changed the key `browse` orders by since the last
    // read, so the statement is rebuilt rather than kept.
    let stmt = match &subject {
        Subject::Table(table) => state.browse(table),
        Subject::Typed(sql) => Statement::typed(state.engine(), sql),
    };
    if stmt.mutates() {
        return Rerun::Write;
    }

    Rerun::Read { subject, stmt }
}

/// Not recorded in the history: the statement is already there, or was never
/// typed.
fn rerun(state: &mut State) -> Vec<Effect> {
    match rerun_of(state) {
        Rerun::Read { subject, stmt } => submit_into_contents(state, subject, stmt),
        Rerun::Edited => {
            state.notify(
                Severity::Warning,
                "uncommitted edits, ctrl-s to commit them first",
            );
            Vec::new()
        }
        Rerun::Write => {
            state.notify(
                Severity::Warning,
                "not re-run: it writes, ctrl-r runs it from the editor",
            );
            Vec::new()
        }
        Rerun::Nothing => Vec::new(),
    }
}
