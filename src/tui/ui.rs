use std::collections::HashSet;
use std::time::{SystemTime, UNIX_EPOCH};

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph, Wrap};
use tui_logger::{TuiLoggerTargetWidget, TuiLoggerWidget};

use crate::cli::start_tui::StartTuiArgs;
use crate::node::NodeState;
use crate::recipe::flatten_recipe;
use crate::store::{self, OrderStatus};
use crate::tui::app::{App, Mode};

pub fn render_ui(frame: &mut Frame, app: &App, _args: &StartTuiArgs) {
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
        Layout::horizontal([Constraint::Length(30), Constraint::Fill(1)]).areas(logger);

    render_recent_orders_block(frame, left);
    render_local_agent_status_block(frame, right, app);
    render_command_block(frame, app, command);
    render_tui_target(frame, app, target);
    render_tui_log(frame, app, log);
    render_help_bar(frame, app, help);
}

fn render_recent_orders_block(frame: &mut Frame, area: Rect) {
    let mut orders = store::get_orders();
    orders.sort_by_key(|o| o.timestamp_ms);

    let lines: Vec<Line> = orders
        .iter()
        .map(|order| {
            let elapsed_ms = order.elapsed_ms();
            let id = match order.server_id.as_ref() {
                Some(sid) => sid.clone(),
                None => format!("local-{}", order.id),
            }
            .chars()
            .take(8)
            .collect::<String>();

            let (status_str, status_style) = match &order.status {
                OrderStatus::Sending => ("Sending".to_string(), Style::new().dark_gray()),
                OrderStatus::Receipt => ("Receipt".to_string(), Style::new().yellow()),
                OrderStatus::Delivered => ("Delivered".to_string(), Style::new().green()),
                OrderStatus::Declined(msg) => (format!("Declined: {msg}"), Style::new().red()),
                OrderStatus::Failed(msg) => (format!("Failed: {msg}"), Style::new().red()),
                OrderStatus::Error(msg) => (format!("Error: {msg}"), Style::new().red()),
            };

            Line::from(vec![Span::styled(
                format!(
                    "[{id}] {} ({elapsed_ms}ms ago) - {}",
                    order.recipe_name, status_str
                ),
                status_style,
            )])
        })
        .collect();

    frame.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: false }).block(
            Block::bordered()
                .title("Recent Orders")
                .border_style(Style::new().light_cyan()),
        ),
        area,
    );
}

fn format_recipes(state: &NodeState) -> String {
    if state.identity.recipes.is_empty() {
        return "{}".to_string();
    }

    let capabilities: HashSet<&str> = state
        .identity
        .capabilities
        .iter()
        .map(String::as_str)
        .collect();

    let entries: Vec<String> = state
        .identity
        .recipes
        .iter()
        .map(|recipe| {
            let mut seen: HashSet<String> = HashSet::new();
            let missing: Vec<String> = flatten_recipe(recipe)
                .into_iter()
                .map(|a| a.name)
                .filter(|name| !capabilities.contains(name.as_str()))
                .filter(|name| seen.insert(name.clone()))
                .collect();
            format!(
                "{:?}: Local {{ missing_actions: {:?} }}",
                recipe.name, missing
            )
        })
        .collect();

    format!("{{{}}}", entries.join(", "))
}

fn render_local_agent_status_block(frame: &mut Frame, area: Rect, app: &App) {
    let state = &app.state;
    let gossip = state.gossip.read().unwrap();
    let now_us = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros() as u64;

    let caps = format!("{:?}", state.identity.capabilities);
    let recipes_str = format_recipes(state);
    let version_str = format!("{}#{}", gossip.version.counter, gossip.version.generation);

    let mut lines: Vec<Line> = vec![
        Line::from(vec![
            Span::styled("Capabilities: ", Style::new().bold()),
            Span::raw(caps),
        ]),
        Line::from(vec![
            Span::styled("Recipes: ", Style::new().bold()),
            Span::raw(recipes_str),
        ]),
        Line::from(Span::styled("Known peers:", Style::new().bold())),
    ];

    let mut sorted_peers: Vec<_> = gossip.peers.iter().collect();
    sorted_peers.sort_by_key(|(addr, _)| addr.as_str());

    let my_addr = &state.identity.addr;
    sorted_peers.retain(|(addr, _)| *addr != my_addr);

    if sorted_peers.is_empty() {
        lines.push(Line::from(Span::raw("  (none)")));
    } else {
        for (addr, info) in sorted_peers {
            let elapsed_ms = if info.last_seen_us > 0 {
                now_us.saturating_sub(info.last_seen_us) / 1000
            } else {
                0
            };
            let peer_ver = format!("v{}#{}", info.version.counter, info.version.generation);
            let ping_str = match info.rtt_us {
                Some(rtt) => format!("{:.1} ms", rtt as f64 / 1000.0),
                None => format!("{} ms ago", elapsed_ms),
            };
            lines.push(Line::from(Span::raw(format!(
                "  {addr} ({peer_ver} {ping_str})"
            ))));
        }
    }

    lines.push(Line::from(vec![
        Span::styled("Host: ", Style::new().bold()),
        Span::raw(state.identity.addr.as_str()),
    ]));

    lines.push(Line::from(vec![
        Span::styled("Current version: ", Style::new().bold()),
        Span::raw(version_str),
    ]));

    frame.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: false }).block(
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
        frame.set_cursor_position((area.x + 3 + app.input.len() as u16, area.y + 1));
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
        .style_error(Style::new().red().bold())
        .style_warn(Style::new().yellow())
        .style_info(Style::new().white())
        .style_debug(Style::new().blue())
        .style_trace(Style::new().gray())
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
