//! Exact durable-text change predicates for the existing input operations.
//! Read only the affected region; retain the draft allocation/preedit ownership.
use jian_core::text_input::{prev_char_boundary, TextInputState};

pub(super) fn pending_range(input: &TextInputState) -> (usize, usize) {
    if input.is_select_all() {
        (0, input.text().len())
    } else {
        input.selection().ordered()
    }
}

pub(super) fn replacement_changes(
    input: &TextInputState,
    range: (usize, usize),
    text: &str,
) -> bool {
    input.text().get(range.0..range.1) != Some(text)
}

pub(super) fn composition_changes(input: &TextInputState) -> bool {
    let Some(composition) = input.composition() else {
        return false;
    };
    let range = if let Some((start, end)) = composition.region {
        // TextInputState::commit_composition delegates this branch to its
        // replace_range, which orders endpoints and rounds both backward.
        (
            prev_char_boundary(input.text(), start.min(end)),
            prev_char_boundary(input.text(), start.max(end)),
        )
    } else {
        pending_range(input)
    };
    replacement_changes(input, range, &composition.text)
}
