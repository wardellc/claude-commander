# Dialog and input interactions

Use these rules when adding or changing inputs. New Session, Edit Session and
Settings should use the same controls for the same kind of value. Keep platform
controls on web and Flutter, but preserve the same meaning of selection,
acceptance and cancellation. This document defines the intended convention; it
is not a claim that every existing frontend has already been migrated.

## Focus and forms

- Show which field has focus and which list row is highlighted.
- Tab and Shift+Tab move between form fields. Up/Down may also move between
  fields in a compact TUI form. Skip absent or disabled fields.
- Enter saves/submits the form when no nested editor or list is open. Do not use
  Enter to open an ordinary field's choice list.
- Escape cancels the current interaction, starting with the innermost open
  editor or list. A key consumed there must never also act on the parent form.
- Keep invalid input visible and editable, with a useful error beside the field.
  Do not silently discard an invalid value or pretend it was saved.
- Advertise the actions available in the current state, rather than showing one
  generic footer for every field.

Settings is an immediate settings editor: an accepted field change is persisted
at once. Enter finishes/closes Settings; Escape closes an active editor without
applying it, or closes Settings if no editor is open. Closing Settings does not
undo previously accepted changes. A future screen that stages changes must
explicitly offer Save/Cancel and preserve a draft until Save succeeds.

Settings uses Tab/Shift+Tab to switch tabs and Up/Down to move between rows;
inside an open searchable choice, Tab stays within that choice.

List managers (Sections, Programs and Workspaces) can use Enter to activate a
selected item or enter its detail pane. Their embedded editors follow the same
accept/cancel rules. Paths may use Tab for completion, with that exception shown
in their hint. Command/search palettes use Enter to activate their result.

## Booleans

A boolean is a two-value choice:

- Left or Right flips the value; neither direction means a fixed On or Off.
- Space also flips it.
- Display the current state clearly. Do not open a list for a boolean.
- Enter belongs to the enclosing form, not to the toggle.

## One selected value

A single-choice field has exactly one selected value. A list with many available
values is still single choice if only one can be selected.

- Left/Right cycles backward/forward, wrapping at the ends, without opening a
  list. Display the value and an affordance such as `←→` or a dropdown indicator.
- Space opens the available choices, highlighting the current value.
- Up/Down moves the highlight, wrapping at the ends. Moving a highlight is not
  acceptance.
- Space or Enter accepts the highlighted value and returns to the form.
- Escape restores the value from before the list opened and returns to the form.
- Opening and closing a list without changing its value must not reset dependent
  fields. An accepted server change may reload project/program choices; merely
  highlighting a server may not.
- Empty/loading lists cannot produce an invalid index or a fabricated selection.

## Several selected values

Use a multi-select list only when the data actually allows several values.
Section PR states, review decisions and approval statuses are examples:

- Space opens the list; show the selected values or a count on the form.
- Up/Down moves the highlight. Space checks/unchecks the highlighted item and
  keeps the list open, so several items can be changed.
- Enter accepts the entire selection and returns to the form.
- Escape restores the entire selection from before the list opened.
- Mark checked items separately from the current highlight. Never make moving
  the highlight toggle an item. Do not submit the parent form when the list
  accepts its selection.

## Searchable choices

The displayed selected value is not the search query.

- Typing on a collapsed searchable choice opens it and starts a fresh query,
  replacing any previous query. It does not append to the selected label.
- Subsequent typing extends that query. Text editing keys edit the query.
- Space inserts a space when the search field has focus. It selects/checks a
  result only when the results list has focus.
- Up/Down moves into the results. Tab/Shift+Tab switches between search and
  results within the open choice. Make the active part visible.
- Typing from results starts a new search; returning to search with Tab preserves
  its current query for editing.
- Paste follows the same focused-field and fresh-search rules as typing.
- Enter accepts the highlighted valid result; no matches leaves the list open.
- Escape discards the search and restores the original selection and dependent
  display state. Filtering must not commit a value to the parent form.

## Text, paste and keyboard ownership

Use the shared text-editing path for cursor movement, deletion and word editing.
Left/Right moves the cursor while editing text; it must not cycle another field.
Paste and dictation must target the same active text editor as typing, including
text editors embedded in Settings. Preserve the existing restrictions for
secrets and colour values. Pasting over an unrelated non-text control must not
silently modify the form's name field.

Do not let global navigation bindings consume ordinary search characters.
Modified shortcuts should not accidentally insert text or activate a control.

Mouse/touch activation should offer the same actions as the keyboard. Platform
focus behavior, screen-reader semantics and focus restoration are part of an
input's implementation, not substitutes for its selection/cancellation rules.

## Implementation and regression tests

Keep choice movement shared (`app::cycle_choice` in the TUI). Use
`app::choice::ChoiceList` for reversible selections, including multi-select
working sets, and use the shared
text/paste routing rather than implementing a separate editor for each dialog.
Store an open list's working selection separately, or snapshot its original
value. Do not rely on every caller remembering to undo highlight changes.

For a new or changed control, test the user-visible sequence:

1. Cycle in both directions, including wrapping and an empty list.
2. Open the list, move, cancel, and verify the original value remains.
3. Open, move, accept, and verify the parent form is still open.
4. Save the parent form and verify the accepted value reaches the service.
5. For search, replace an old query, type spaces, edit the cursor, paste, show no
   matches, and cancel back to the original selection.
6. For multi-select, change several items before acceptance and verify full
   rollback on cancellation.
7. Verify the rendered focus, selection markers and current-state hints.

Update this guide, the README shortcuts and in-app help when intentionally
changing a convention. New exceptions must explain the user task they support.
