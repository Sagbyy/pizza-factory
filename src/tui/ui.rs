use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::widgets::{Block, Paragraph};
use ratatui::Frame;

pub fn render_ui(frame: &mut Frame) {
    let [left, right] = Layout::horizontal([
        Constraint::Percentage(25),
        Constraint::Percentage(75),
    ])
    .areas(frame.area());

    render_recent_orders_block(frame, left);
    render_local_agent_status_block(frame, right);
}

fn render_recent_orders_block(frame: &mut Frame, area: Rect) {
    frame.render_widget(
        Paragraph::new("contenu gauche").block(
          Block::bordered()
          .title("Recent Orders")
          .border_style(Style::new().light_magenta()).style(Style::new().light_magenta())
        ),
        area,
    );
}

fn render_local_agent_status_block(frame: &mut Frame, area: Rect) {
    frame.render_widget(
        Paragraph::new("contenu droite").block(
          Block::bordered()
          .title("Agent Local Status")
          .border_style(Style::new().light_magenta()).style(Style::new().light_magenta())
        ),
        area,
    );
}
