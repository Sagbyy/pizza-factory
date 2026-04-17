use std::{io::stdout, sync::Arc};

use crossterm::event::{Event, KeyCode, KeyEventKind};
use ratatui::DefaultTerminal;
use tui_logger::{TuiWidgetEvent, TuiWidgetState};

use crate::{cli::start_tui::StartTuiArgs, node::NodeState, tui::ui};

pub enum Mode {
    Normal,
    Editing,
}

pub struct App {
    pub mode: Mode,
    pub input: String,
    pub logger_state: TuiWidgetState,
    pub state: Arc<NodeState>,
}

impl App {
    pub fn new(state: Arc<NodeState>) -> Self {
        Self {
            mode: Mode::Normal,
            input: String::new(),
            logger_state: TuiWidgetState::new(),
            state,
        }
    }

    pub fn handle_key(&mut self, event: Event) -> bool {
        match event {
            Event::Mouse(_) => false,
            Event::Key(key) => {
                if key.kind != KeyEventKind::Press {
                    return false;
                }
                match self.mode {
                    Mode::Normal => match key.code {
                        KeyCode::Char('q') | KeyCode::Char('Q') => return true,
                        KeyCode::Char('e') => self.mode = Mode::Editing,
                        KeyCode::Up => self.logger_state.transition(TuiWidgetEvent::UpKey),
                        KeyCode::Down => self.logger_state.transition(TuiWidgetEvent::DownKey),
                        KeyCode::Left => self.logger_state.transition(TuiWidgetEvent::LeftKey),
                        KeyCode::Right => self.logger_state.transition(TuiWidgetEvent::RightKey),
                        KeyCode::Char('+') => self.logger_state.transition(TuiWidgetEvent::PlusKey),
                        KeyCode::Char('-') => {
                            self.logger_state.transition(TuiWidgetEvent::MinusKey)
                        }
                        KeyCode::Char(' ') => {
                            self.logger_state.transition(TuiWidgetEvent::SpaceKey)
                        }
                        KeyCode::Char('h') => self.logger_state.transition(TuiWidgetEvent::HideKey),
                        KeyCode::Char('f') => {
                            self.logger_state.transition(TuiWidgetEvent::FocusKey)
                        }
                        KeyCode::PageUp => {
                            self.logger_state.transition(TuiWidgetEvent::PrevPageKey)
                        }
                        KeyCode::PageDown => {
                            self.logger_state.transition(TuiWidgetEvent::NextPageKey)
                        }
                        KeyCode::Esc => self.logger_state.transition(TuiWidgetEvent::EscapeKey),
                        _ => {}
                    },
                    Mode::Editing => match key.code {
                        KeyCode::Esc => {
                            self.input.clear();
                            self.mode = Mode::Normal;
                        }
                        KeyCode::Char(c) => self.input.push(c),
                        KeyCode::Backspace => {
                            self.input.pop();
                        }
                        KeyCode::Enter => {
                            let cmd = self.input.trim().to_string();
                            if !cmd.is_empty() {
                                log::info!(target: "command", "> {cmd}");
                                log::warn!(target: "command", "Unknown command: '{cmd}'");
                            }
                            self.input.clear();
                            self.mode = Mode::Normal;
                        }
                        _ => {}
                    },
                }
                false
            }
            _ => false,
        }
    }
}

pub fn start_tui(
    terminal: &mut DefaultTerminal,
    args: StartTuiArgs,
    state: Arc<NodeState>,
) -> std::io::Result<()> {
    tui_logger::init_logger(log::LevelFilter::Trace).ok();
    tui_logger::set_default_level(log::LevelFilter::Info);

    print!("\x1B[?1003h");
    use std::io::Write;
    stdout().flush()?;

    let mut app = App::new(state);
    let result = loop {
        terminal.draw(|frame| ui::render_ui(frame, &app, &args))?;
        if crossterm::event::poll(std::time::Duration::from_millis(500))? {
            let event = crossterm::event::read()?;
            if app.handle_key(event) {
                break Ok(());
            }
        }
    };
    print!("\x1B[?1003l");
    stdout().flush()?;
    result
}
