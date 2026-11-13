use super::*;
use claude_commander_protocol::api::{CreateOptions, EditSession, ProgramInfo};
use crossterm::event::{KeyCode, KeyEvent};

#[derive(Debug, PartialEq, Eq)]
enum ChoiceKey {
    Program(String),
    Section(Option<String>),
    Base(String),
}

#[derive(Debug, Clone)]
pub struct SessionEditor {
    pub session: SessionRef,
    pub name: Input,
    pub program: Input,
    pub model: claude_commander_protocol::session_editor::SessionEditorModel,
    pub sections: Vec<Option<String>>,
    pub section: usize,
    pub bases: Vec<(Option<SessionId>, String)>,
    pub base: usize,
    pub keep_alive: bool,
    pub focus: usize,
    pub programs: Vec<ProgramInfo>,
    pub restart_now: bool,
    /// Highlight in an open choice list; the form value changes on acceptance.
    pub choice: Option<usize>,
}

impl SessionEditor {
    pub fn new(
        session: SessionRef,
        info: &claude_commander_protocol::api::SessionInfo,
        peers: &[claude_commander_protocol::api::SessionInfo],
        main_branch: &str,
    ) -> Self {
        let model =
            claude_commander_viewmodel::session_editor::model(info, peers, Some(main_branch), None);
        let section = usize::from(model.original.section.is_some());
        let base = model
            .bases
            .iter()
            .position(|b| b.id == model.original.base)
            .unwrap_or(0);
        let mut sections = vec![None];
        sections.extend(model.sections.iter().cloned().map(Some));
        Self {
            session,
            name: model.original.title.clone().into(),
            program: model.original.program.clone().into(),
            sections,
            section,
            bases: model
                .bases
                .iter()
                .map(|b| (b.parent_session_id, b.label.clone()))
                .collect(),
            base,
            keep_alive: model.original.keep_alive,
            focus: 0,
            programs: model.programs.clone(),
            model,
            restart_now: false,
            choice: None,
        }
    }
    pub fn load_options(&mut self, opts: CreateOptions) {
        let open_choice = self.choice.map(|index| self.choice_key(index));
        let section = self.sections[self.section].clone();
        claude_commander_viewmodel::session_editor::load_options(&mut self.model, &opts);
        self.sections = vec![None];
        self.sections
            .extend(self.model.sections.iter().cloned().map(Some));
        self.section = self
            .sections
            .iter()
            .position(|s| *s == section)
            .unwrap_or(0);
        self.programs = self.model.programs.clone();
        if let Some(key) = open_choice {
            self.choice = Some(
                (0..self.choice_labels().len())
                    .find(|&index| self.choice_key(index) == key)
                    .unwrap_or_else(|| self.choice_index()),
            );
        }
    }
    fn plan(
        &self,
    ) -> std::result::Result<claude_commander_protocol::session_editor::SessionEditorPlan, String>
    {
        claude_commander_viewmodel::session_editor::plan(
            &self.model,
            claude_commander_protocol::session_editor::SessionEditorDraft {
                title: self.name.value().into(),
                program: self.program.value().into(),
                section: self.sections[self.section].clone(),
                keep_alive: self.keep_alive,
                base: self.model.bases[self.base].id.clone(),
            },
        )
    }
    fn program_label(&self) -> &str {
        self.programs
            .iter()
            .find(|p| p.command == self.program.value())
            .map_or(self.program.value(), |p| p.label.as_str())
    }
    pub fn edit(&self, restart: bool) -> std::result::Result<EditSession, String> {
        let mut edit = self.plan()?.edit;
        edit.restart = restart;
        Ok(edit)
    }
    fn cycle(&mut self, forward: bool) {
        if self.focus == 4 {
            self.keep_alive = !self.keep_alive;
            return;
        }
        let index = cycle_choice(self.choice_index(), self.choice_labels().len(), forward);
        self.select_choice(index);
    }
    fn choice_index(&self) -> usize {
        match self.focus {
            1 => self
                .programs
                .iter()
                .position(|p| p.command == self.program.value())
                .unwrap_or(0),
            2 => self.section,
            3 => self.base,
            _ => 0,
        }
    }
    fn choice_labels(&self) -> Vec<String> {
        match self.focus {
            1 => self.programs.iter().map(|p| p.label.clone()).collect(),
            2 => self
                .sections
                .iter()
                .map(|s| s.clone().unwrap_or_else(|| "Automatic".into()))
                .collect(),
            3 => self.bases.iter().map(|(_, label)| label.clone()).collect(),
            _ => vec![],
        }
    }
    fn choice_key(&self, index: usize) -> Option<ChoiceKey> {
        match self.focus {
            1 => self
                .programs
                .get(index)
                .map(|p| ChoiceKey::Program(p.command.clone())),
            2 => self.sections.get(index).cloned().map(ChoiceKey::Section),
            3 => self
                .model
                .bases
                .get(index)
                .map(|b| ChoiceKey::Base(b.id.clone())),
            _ => None,
        }
    }
    fn select_choice(&mut self, index: usize) {
        match self.focus {
            1 => {
                if let Some(program) = self.programs.get(index) {
                    self.program = program.command.clone().into();
                }
            }
            2 if index < self.sections.len() => self.section = index,
            3 if index < self.bases.len() => self.base = index,
            _ => {}
        }
    }
    fn handle_choice_key(&mut self, key: KeyEvent) -> bool {
        if !key.modifiers.is_empty() {
            return self.choice.is_some();
        }
        if let Some(selected) = self.choice {
            match key.code {
                KeyCode::Esc => self.choice = None,
                KeyCode::Enter | KeyCode::Char(' ') => {
                    self.select_choice(selected);
                    self.choice = None;
                }
                KeyCode::Up | KeyCode::Down => {
                    self.choice = Some(cycle_choice(
                        selected,
                        self.choice_labels().len(),
                        key.code == KeyCode::Down,
                    ))
                }
                _ => {}
            }
            return true;
        }
        if key.code == KeyCode::Char(' ') && (1..=3).contains(&self.focus) {
            self.choice = Some(self.choice_index());
            return true;
        }
        false
    }
}

impl App {
    pub(super) fn handle_edit_session(&mut self) {
        let Some(session) = self.ui_state.selected_session_id else {
            return;
        };
        let Some(info) = self.session(session) else {
            return;
        };
        let Some(handle) = self.backend(session.backend) else {
            return;
        };
        let main = handle
            .view
            .snapshot
            .projects
            .iter()
            .find(|p| p.id == info.project_id)
            .map(|p| p.main_branch.as_str())
            .unwrap_or("main");
        let editor = SessionEditor::new(session, info, &handle.view.snapshot.sessions, main);
        self.ui_state.modal = Modal::EditSession(editor);
        let backend = self.backend_arc(session.backend);
        let tx = self.event_loop.sender();
        tokio::spawn(async move {
            let result = backend.create_options().await.map_err(|e| e.to_string());
            let _ = tx
                .send(AppEvent::StateUpdate(
                    StateUpdate::SessionEditOptionsLoaded { session, result },
                ))
                .await;
        });
    }
    pub(super) fn handle_edit_session_key(&mut self, key: KeyEvent) {
        let modal = std::mem::replace(&mut self.ui_state.modal, Modal::None);
        let (mut editor, warning) = match modal {
            Modal::EditSession(editor) => (editor, false),
            Modal::EditSessionRestart(editor) => (editor, true),
            other => {
                self.ui_state.modal = other;
                return;
            }
        };
        if warning {
            match key.code {
                KeyCode::Esc => self.ui_state.modal = Modal::EditSession(editor),
                KeyCode::Left
                | KeyCode::Right
                | KeyCode::Char(' ')
                | KeyCode::Tab
                | KeyCode::BackTab => {
                    editor.restart_now = !editor.restart_now;
                    self.ui_state.modal = Modal::EditSessionRestart(editor);
                }
                KeyCode::Char('y') => self.save_session_edit(editor, true),
                KeyCode::Char('n') => self.save_session_edit(editor, false),
                KeyCode::Enter => {
                    let restart = editor.restart_now;
                    self.save_session_edit(editor, restart);
                }
                _ => self.ui_state.modal = Modal::EditSessionRestart(editor),
            }
            return;
        }
        if editor.handle_choice_key(key) {
            self.ui_state.modal = Modal::EditSession(editor);
            return;
        }
        match key.code {
            KeyCode::Esc => return,
            KeyCode::Tab | KeyCode::Down => editor.focus = (editor.focus + 1) % 5,
            KeyCode::BackTab | KeyCode::Up => editor.focus = (editor.focus + 4) % 5,
            KeyCode::Enter => {
                if editor.name.value().trim().is_empty() || editor.program.value().trim().is_empty()
                {
                    self.ui_state.status_message = Some((
                        "Name and program cannot be empty".into(),
                        Instant::now() + Duration::from_secs(4),
                    ));
                } else if editor.plan().is_ok_and(|p| p.requires_restart) {
                    self.ui_state.modal = Modal::EditSessionRestart(editor);
                    return;
                } else {
                    self.save_session_edit(editor, false);
                    return;
                }
            }
            KeyCode::Left | KeyCode::Right if editor.focus >= 1 && key.modifiers.is_empty() => {
                editor.cycle(key.code == KeyCode::Right)
            }
            KeyCode::Char(' ') if editor.focus >= 1 && key.modifiers.is_empty() => {
                editor.cycle(true)
            }
            _ if editor.focus == 0 => {
                edit_text_input(&mut editor.name, key);
            }
            _ => {}
        }
        self.ui_state.modal = Modal::EditSession(editor);
    }
    fn save_session_edit(&mut self, editor: SessionEditor, restart: bool) {
        let edit = match editor.edit(restart) {
            Ok(edit) => edit,
            Err(message) => {
                self.ui_state.status_message =
                    Some((message, Instant::now() + Duration::from_secs(5)));
                self.ui_state.modal = Modal::EditSession(editor);
                return;
            }
        };
        let backend = self.backend_arc(editor.session.backend);
        let tx = self.event_loop.sender();
        tokio::spawn(async move {
            match backend.edit_session(editor.session.id, edit).await {
                Ok(outcome) => {
                    if !outcome.warnings.is_empty() {
                        let _ = tx
                            .send(AppEvent::StateUpdate(StateUpdate::Error {
                                message: outcome.warnings.join("\n"),
                            }))
                            .await;
                    }
                    let _ = tx
                        .send(AppEvent::StateUpdate(StateUpdate::SessionMutationApplied {
                            backend_id: editor.session.backend.0,
                            session_id: editor.session.id,
                        }))
                        .await;
                }
                Err(e) => {
                    let _ = tx
                        .send(AppEvent::StateUpdate(StateUpdate::Error {
                            message: format!("Failed to edit session: {e}"),
                        }))
                        .await;
                }
            }
        });
    }
}

pub(super) fn render(
    frame: &mut Frame,
    area: Rect,
    editor: &SessionEditor,
    warning: bool,
    theme: &Theme,
) {
    let area = super::modals::centered_rect(
        75,
        14 + if editor.choice.is_some() {
            editor.choice_labels().len().min(6) as u16
        } else {
            0
        },
        area,
    );
    frame.render_widget(Clear, area);
    let title = if warning {
        " Restart session? "
    } else {
        " Edit session "
    };
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border_focused));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let lines = if warning {
        vec![
            Line::from("Changing the program requires a fresh restart."),
            Line::from("Restarting stops the current agent and clears its conversation."),
            Line::from("Saving for later keeps the current agent running."),
            Line::from("The new program starts fresh on the next restart."),
            Line::from(""),
            Line::from(if editor.restart_now {
                "[Yes, restart and clear]    No, save for next restart"
            } else {
                " Yes, restart and clear    [No, save for next restart]"
            }),
            Line::from("←/→ choose · Enter confirm · Esc back"),
        ]
    } else {
        let values = [
            format!(
                "Name: {}",
                if editor.focus == 0 {
                    input_with_caret(&editor.name)
                } else {
                    editor.name.value().into()
                }
            ),
            format!("Program: {}", editor.program_label()),
            format!(
                "Section: {}",
                editor.sections[editor.section]
                    .as_deref()
                    .unwrap_or("Automatic")
            ),
            format!("Stack base: {}", editor.bases[editor.base].1),
            format!(
                "Keep alive: {}",
                if editor.keep_alive { "Yes" } else { "No" }
            ),
        ];
        let mut lines: Vec<Line> = values
            .into_iter()
            .enumerate()
            .map(|(i, text)| {
                Line::styled(
                    format!("{} {text}", if editor.focus == i { ">" } else { " " }),
                    Style::default().fg(if editor.focus == i {
                        theme.text_primary
                    } else {
                        theme.text_secondary
                    }),
                )
            })
            .collect();
        if let Some(selected) = editor.choice {
            let labels = editor.choice_labels();
            let start = super::actions::adjust_list_scroll(selected, 0, 6);
            lines.splice(
                editor.focus + 1..editor.focus + 1,
                labels
                    .into_iter()
                    .enumerate()
                    .skip(start)
                    .take(6)
                    .map(|(i, label)| {
                        Line::styled(
                            format!("    {label}"),
                            if i == selected {
                                theme.selection()
                            } else {
                                Style::default().fg(theme.text_secondary)
                            },
                        )
                    }),
            );
        }
        lines.extend([
            Line::from(""),
            Line::from(if editor.choice.is_some() {
                "↑↓ highlight · Space/Enter select · Esc cancel list"
            } else {
                "Tab/↑/↓ field · ←→ change · Space list/toggle"
            }),
            Line::from(if editor.choice.is_some() {
                ""
            } else {
                "Enter save · Esc cancel"
            }),
            Line::from("Stack base changes metadata and PR target; git history is unchanged."),
        ]);
        lines
    };
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}
