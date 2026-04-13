use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};
use tui_logger::{TuiLoggerTargetWidget, TuiLoggerWidget};

use crate::tui::app::{App, Mode};

pub fn render_ui(frame: &mut Frame, app: &App) {
    let [top, command, logger, help] = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(3),
        Constraint::Fill(1),
        Constraint::Length(3),
    ])
    .areas(frame.area());

    let [left, right] =
        Layout::horizontal([Constraint::Percentage(25), Constraint::Percentage(75)]).areas(top);

    let [target, log] =
        Layout::horizontal([Constraint::Length(17), Constraint::Fill(1)]).areas(logger);

    render_recent_orders_block(frame, left);
    render_local_agent_status_block(frame, right);
    render_command_block(frame, app, command);
    render_tui_target(frame, app, target);
    render_tui_log(frame, app, log);
    render_help_bar(frame, app, help);
}

fn render_recent_orders_block(frame: &mut Frame, area: Rect) {
    frame.render_widget(
        Paragraph::new("").block(
            Block::bordered()
                .title("Recent Orders")
                .border_style(Style::new().light_cyan()),
        ),
        area,
    );
}

fn render_local_agent_status_block(frame: &mut Frame, area: Rect) {
    frame.render_widget(
        Paragraph::new("").block(
            Block::bordered()
                .title("Local Agent Status")
                .border_style(Style::new().light_cyan()),
        ),
        area,
    );
}

fn render_command_block(frame: &mut Frame, app: &App, area: Rect) {
    let (title, border_style, prefix) = match app.mode {
        Mode::Editing => (
            "Edit Command (use 'help' for help and <Tab> for completion)",
            Style::new().red(),
            Span::styled("> ", Style::new().red().bold()),
        ),
        Mode::Normal => (
            "Command",
            Style::new().white(),
            Span::styled("> ", Style::new().dark_gray()),
        ),
    };

    let input_line = Line::from(vec![prefix, Span::raw(app.input.as_str())]);

    frame.render_widget(
        Paragraph::new(input_line).block(Block::bordered().title(title).border_style(border_style)),
        area,
    );

    if matches!(app.mode, Mode::Editing) {
        frame.set_cursor_position((
            area.x + 3 + app.input.len() as u16, // +1 bordure +2 "> "
            area.y + 1,
        ));
    }
}

fn render_tui_target(frame: &mut Frame, app: &App, area: Rect) {
    let widget = TuiLoggerTargetWidget::default()
        .block(Block::bordered().title("Tui Target Sele"))
        .state(&app.logger_state);
    frame.render_widget(widget, area);
}

fn render_tui_log(frame: &mut Frame, app: &App, area: Rect) {
    let widget = TuiLoggerWidget::default()
        .block(Block::bordered().title("Tui Log"))
        .state(&app.logger_state);
    frame.render_widget(widget, area);
}

fn render_help_bar(frame: &mut Frame, app: &App, area: Rect) {
    let help_paragraph: Paragraph = match app.mode {
        Mode::Editing => Paragraph::new(vec![Line::from(vec![
            Span::styled("Esc", Style::new().bold().light_cyan()),
            Span::raw(": Leave edit mode | "),
            Span::styled("Enter", Style::new().bold().light_cyan()),
            Span::raw(": Valid command"),
        ])]),
        Mode::Normal => Paragraph::new(vec![
            Line::from(vec![
                Span::styled("Q", Style::new().bold().light_cyan()),
                Span::raw(": Quit | "),
                Span::styled("e", Style::new().bold().light_cyan()),
                Span::raw(": edit command | "),
                Span::styled("↑/↓", Style::new().bold().light_cyan()),
                Span::raw(": Select target | "),
                Span::styled("f", Style::new().bold().light_cyan()),
                Span::raw(": Focus target"),
            ]),
            Line::from(vec![
                Span::styled("←/→", Style::new().bold().light_cyan()),
                Span::raw(": Display level | "),
                Span::styled("+/-", Style::new().bold().light_cyan()),
                Span::raw(": Filter level | "),
                Span::styled("Space", Style::new().bold().light_cyan()),
                Span::raw(": Toggle hidden targets"),
            ]),
            Line::from(vec![
                Span::styled("h", Style::new().bold().light_cyan()),
                Span::raw(": Hide target selector | "),
                Span::styled("PageUp/Down", Style::new().bold().light_cyan()),
                Span::raw(": Scroll | "),
                Span::styled("Esc", Style::new().bold().light_cyan()),
                Span::raw(": Cancel scroll"),
            ]),
        ]),
    };

    frame.render_widget(help_paragraph.alignment(Alignment::Center), area);
}
