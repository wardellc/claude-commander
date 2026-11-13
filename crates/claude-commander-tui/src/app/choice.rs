//! Working selections for single- and multi-select lists. Values reach the
//! parent editor only on acceptance, so cancellation needs no compensating write.

use super::{PickerOption, cycle_choice};
use crossterm::event::{KeyCode, KeyEvent};

#[derive(Debug, Clone)]
pub struct ChoiceList {
    pub(super) options: Vec<PickerOption>,
    pub(super) checked: Vec<bool>,
    pub(super) highlighted: usize,
    pub(super) multiple: bool,
}

pub(super) enum ChoiceOutcome {
    Open,
    Accept(String),
    Cancel,
}

impl ChoiceList {
    pub(super) fn new(options: Vec<PickerOption>, current: &str, multiple: bool) -> Self {
        let normalize = |s: &str| s.to_ascii_lowercase().replace(['-', '_', ' '], "");
        let values: Vec<_> = current
            .split(',')
            .map(|value| normalize(value.trim()))
            .collect();
        let checked: Vec<_> = options
            .iter()
            .map(|option| values.contains(&normalize(&option.value)))
            .collect();
        let highlighted = checked.iter().position(|&checked| checked).unwrap_or(0);
        Self {
            options,
            checked,
            highlighted,
            multiple,
        }
    }

    pub(super) fn handle_key(&mut self, key: KeyEvent) -> ChoiceOutcome {
        if !key.modifiers.is_empty() {
            return ChoiceOutcome::Open;
        }
        match key.code {
            KeyCode::Esc => ChoiceOutcome::Cancel,
            KeyCode::Up | KeyCode::Char('k') => {
                self.highlighted = cycle_choice(self.highlighted, self.options.len(), false);
                ChoiceOutcome::Open
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.highlighted = cycle_choice(self.highlighted, self.options.len(), true);
                ChoiceOutcome::Open
            }
            KeyCode::Char(' ') if self.multiple => {
                if let Some(checked) = self.checked.get_mut(self.highlighted) {
                    *checked = !*checked;
                }
                ChoiceOutcome::Open
            }
            KeyCode::Enter | KeyCode::Char(' ') if !self.options.is_empty() => {
                let value = if self.multiple {
                    self.options
                        .iter()
                        .zip(&self.checked)
                        .filter(|(_, checked)| **checked)
                        .map(|(option, _)| option.value.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                } else {
                    self.options[self.highlighted].value.clone()
                };
                ChoiceOutcome::Accept(value)
            }
            _ => ChoiceOutcome::Open,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;

    fn press(choice: &mut ChoiceList, code: KeyCode) -> ChoiceOutcome {
        choice.handle_key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    #[test]
    fn space_checks_multiple_items_until_enter_accepts_the_selection() {
        let mut choice = ChoiceList::new(
            ["open", "merged", "closed"]
                .into_iter()
                .map(PickerOption::plain)
                .collect(),
            "",
            true,
        );
        assert!(matches!(
            press(&mut choice, KeyCode::Char(' ')),
            ChoiceOutcome::Open
        ));
        press(&mut choice, KeyCode::Down);
        press(&mut choice, KeyCode::Char(' '));
        assert!(
            matches!(press(&mut choice, KeyCode::Enter), ChoiceOutcome::Accept(value) if value == "open, merged")
        );
        press(&mut choice, KeyCode::Char(' '));
        assert!(
            matches!(press(&mut choice, KeyCode::Enter), ChoiceOutcome::Accept(value) if value == "open")
        );
    }

    #[test]
    fn single_choice_highlight_wraps_and_space_accepts_only_that_value() {
        let mut choice = ChoiceList::new(
            ["auto", "true", "false"]
                .into_iter()
                .map(PickerOption::plain)
                .collect(),
            "auto",
            false,
        );
        press(&mut choice, KeyCode::Up);
        assert_eq!(choice.highlighted, 2);
        assert_eq!(choice.checked, [true, false, false]);
        assert!(
            matches!(press(&mut choice, KeyCode::Char(' ')), ChoiceOutcome::Accept(value) if value == "false")
        );
    }

    #[test]
    fn empty_choices_cannot_accept_and_escape_always_cancels() {
        let mut choice = ChoiceList::new(vec![], "", true);
        for code in [
            KeyCode::Up,
            KeyCode::Down,
            KeyCode::Char(' '),
            KeyCode::Enter,
        ] {
            assert!(matches!(press(&mut choice, code), ChoiceOutcome::Open));
        }
        assert!(matches!(
            press(&mut choice, KeyCode::Esc),
            ChoiceOutcome::Cancel
        ));
    }

    #[test]
    fn modified_space_neither_checks_nor_accepts() {
        for multiple in [true, false] {
            let mut choice = ChoiceList::new(vec![PickerOption::plain("open")], "", multiple);
            assert!(matches!(
                choice.handle_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::CONTROL)),
                ChoiceOutcome::Open
            ));
            assert_eq!(choice.checked, [false]);
        }
    }
}
