//! The cell inspector.

use crate::contents::{ContentsState, SelectionMode};
use crate::feedback::{Flash, Lit};
use crate::inspect::{Inspected, Inspector, Strip};
use crate::model::{Key, Value};
use crate::overlay::Overlay;
use crate::panel::Panel;
use crate::search::Direction;
use crate::state::State;

use super::{
    Effect, copied, editable_here, escape, field_key, open_insert_commit, quit, set_null,
    start_edit,
};

pub(super) const fn inspector_extent(state: &State) -> (usize, usize) {
    (
        state.geometry.inspector_lines(),
        state.geometry.inspector_rows(),
    )
}

/// `None` in column mode, over a held range, and where the block does not fit.
/// Read by the hint bar too.
pub(crate) fn inspection(state: &State) -> Option<Inspector> {
    if state.focus() != Panel::Contents
        || state.geometry.inspector_rows() == 0
        || state.contents.rows.is_active()
        || state.contents.columns.is_active()
    {
        return None;
    }

    let ContentsState::Loaded { subject, page, .. } = state.contents.state() else {
        return None;
    };
    let index = state.contents.cursor.index();
    page.row(index)?;
    let columns = &page.page().columns;

    let inspected = match state.contents.mode {
        SelectionMode::Row => Inspected::Row {
            number: index + 1,
            columns: columns.clone(),
            values: (0..columns.len())
                .map(|at| page.value(index, at).cloned().unwrap_or(Value::Null))
                .collect(),
        },
        SelectionMode::Column => return None,
        SelectionMode::Cell => {
            let at = state.contents.column.index();

            Inspected::Cell {
                at,
                column: columns.get(at)?.clone(),
                value: page.value(index, at)?.clone(),
            }
        }
    };

    // Carries the grid's dirty marks, so a reopened inspector agrees with the
    // grid.
    let edited = match state.contents.mode {
        SelectionMode::Row => (0..columns.len())
            .filter(|at| page.is_edited(index, *at))
            .collect(),
        SelectionMode::Cell | SelectionMode::Column => {
            let at = state.contents.column.index();
            if page.is_edited(index, at) {
                vec![0]
            } else {
                Vec::new()
            }
        }
    };

    Some(Inspector::opened(index, subject.clone(), inspected).with_edited(edited))
}

pub(super) fn open_inspector(state: &mut State) {
    if let Some(inspector) = inspection(state) {
        state.overlay = Some(Overlay::Inspect(Box::new(inspector)));
    }
}

pub(super) fn edit_from_grid(state: &mut State) -> Vec<Effect> {
    open_inspector(state);
    if !matches!(state.overlay, Some(Overlay::Inspect(_))) {
        return Vec::new();
    }
    // A row's first column is nearly always the primary key, where `e`
    // only refuses. The field stays closed so the user picks which to edit.
    if state.contents.mode == SelectionMode::Row {
        focus_first_editable(state);
        return Vec::new();
    }
    start_edit(state)
}

/// Stays on the first when none is editable, so the refusal names it.
fn focus_first_editable(state: &mut State) {
    let Some(Overlay::Inspect(inspector)) = &state.overlay else {
        return;
    };
    let count = inspector.field_count();

    for at in 0..count {
        if let Some(Overlay::Inspect(inspector)) = &mut state.overlay {
            inspector.focus_field(at);
        }
        if editable_here(state) {
            return;
        }
    }
    if let Some(Overlay::Inspect(inspector)) = &mut state.overlay {
        inspector.focus_field(0);
    }
}

/// Unclaimed keys are inert. A stacked message claims every key, then the `/`
/// prompt, then an open field.
pub(super) fn inspect_key(state: &mut State, key: Key) -> Vec<Effect> {
    let Some(Overlay::Inspect(inspector)) = &mut state.overlay else {
        return Vec::new();
    };

    if inspector.message().is_some() {
        if matches!(key, Key::Escape | Key::Enter) {
            inspector.dismiss_message();
        }
        return Vec::new();
    }

    if inspector.is_searching() {
        match key {
            Key::Char(character) => inspector.search_push(character),
            Key::Backspace => inspector.search_pop(),
            Key::Enter => inspector.accept_search(),
            Key::Escape => inspector.cancel_search(),
            _ => {}
        }
        return Vec::new();
    }

    if inspector.field().is_some() {
        return field_key(state, key);
    }

    match key {
        Key::Char('e') => return start_edit(state),
        // Over a row not yet inserted, `⏎` asks to write it.
        Key::Enter if inspector.is_inserting() => {
            open_insert_commit(state);
            return Vec::new();
        }
        Key::Char('x') if !inspector.is_inserting() => return set_null(state),
        Key::Char('q') => return quit(state),
        Key::Enter => {
            escape(state);
            return Vec::new();
        }
        Key::Escape => {
            if !inspector.back_out() {
                escape(state);
            }
            return Vec::new();
        }
        Key::Char('y') => {
            let effects = copied(inspector.yank());
            // Spending the range here would punish a `y` on a null.
            if !effects.is_empty() {
                let (entries, strips) = inspector.extent();
                inspector.collapse_selection();
                state.flash = Some(Flash::over(Lit::Inspector, entries, strips));
            }
            return effects;
        }
        _ => {}
    }

    let (_, height) = inspector_extent(state);
    let Some(Overlay::Inspect(inspector)) = &mut state.overlay else {
        return Vec::new();
    };

    match key {
        Key::Char('j') | Key::Down => inspector.down(),
        Key::Char('k') | Key::Up => inspector.up(),
        Key::Char('h') | Key::Left => inspector.to_strip(Strip::Name),
        Key::Char('l') | Key::Right => inspector.to_strip(Strip::Value),
        Key::PageDown => inspector.page_down(height),
        Key::PageUp => inspector.page_up(height),
        Key::Home => inspector.to_start(),
        Key::End => inspector.to_end(),
        Key::Char('m') => inspector.cycle_mode(),
        Key::Char('v') => inspector.toggle_visual(),
        Key::Char('/') => inspector.open_search(),
        Key::Char('n') => inspector.jump(Direction::Forwards),
        Key::Char('N') => inspector.jump(Direction::Backwards),
        Key::Char('x') => inspector.unset(),
        Key::Char('Y') => return copied(inspector.line()),
        _ => {}
    }

    Vec::new()
}
