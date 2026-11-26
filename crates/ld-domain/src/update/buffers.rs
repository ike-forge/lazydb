//! The query buffer follows the connection: switching keeps the one left and
//! shows the one chosen.

use crate::config::ConnectionName;
use crate::state::State;

use super::Effect;

/// The editor's text under its owner, written when it changed.
pub(super) fn keep_buffer(state: &mut State) -> Option<Effect> {
    let owner = state.buffers.owner()?.clone();
    let text = state.queries.text();

    state
        .buffers
        .keep(&owner, text.clone())
        .then_some(Effect::SaveBuffer {
            connection: owner,
            text,
        })
}

/// Text typed before any connection was chosen goes to the first one, unless
/// it already has a buffer.
pub(super) fn swap_buffer(state: &mut State, to: &ConnectionName) -> Option<Effect> {
    if state.buffers.owner() == Some(to) {
        return None;
    }

    let kept = keep_buffer(state);
    let stored = state.buffers.of(to);
    if state.buffers.owner().is_some() || !stored.is_empty() {
        let stored = stored.to_owned();
        state.queries.replace(&stored);
    }
    state.buffers.own(to.clone());

    kept
}
