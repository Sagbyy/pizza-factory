use crossterm::event::{Event, KeyEventKind};
use ratatui::DefaultTerminal;

use crate::tui::ui;

pub fn start_tui(terminal: &mut DefaultTerminal) -> std::io::Result<()> {
    loop {
        terminal.draw(ui::render_ui)?;
        if let Event::Key(key) = crossterm::event::read()? {
            if key.kind == KeyEventKind::Press && key.code == crossterm::event::KeyCode::Char('q') {
                break Ok(());
            }
        }
    }
}
