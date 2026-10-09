//! Drawing.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};

use crate::app::{App, Mode, Status};

pub fn draw(frame: &mut Frame, app: &App) {
    let area = frame.area();
    // The pane takes a fixed slice of the lower area; the slot list keeps the
    // rest. Vertical rather than side-by-side, because the slot list is 28 rows
    // tall and a narrow column would truncate the actions.
    let constraints = if app.show_pane() {
        vec![
            Constraint::Length(3),
            Constraint::Min(4),
            Constraint::Percentage(35),
            Constraint::Length(1),
        ]
    } else {
        vec![
            Constraint::Length(3),
            Constraint::Min(0),
            Constraint::Length(1),
        ]
    };
    let chunks = Layout::vertical(constraints).split(area);

    let header = Block::default().borders(Borders::ALL).title(title(app));
    frame.render_widget(header, chunks[0]);

    // The selected row is marked in the text as well as styled, so the cursor
    // is still visible on a terminal with no colour and to anyone who cannot
    // distinguish the highlight.
    let selected = app.selected();
    let items: Vec<ListItem> = app
        .rows()
        .iter()
        .enumerate()
        .map(|(i, row)| {
            let marker = if i == selected { "> " } else { "  " };
            ListItem::new(Line::from(vec![
                Span::styled(marker, Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(format!("[{:2}] ", row.index())),
                Span::styled(
                    format!("{:<14}", row.label()),
                    Style::default().add_modifier(Modifier::DIM),
                ),
                Span::raw(row.action_text()),
            ]))
        })
        .collect();

    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(" slots "))
        .highlight_style(
            Style::default()
                .add_modifier(Modifier::REVERSED)
                .add_modifier(Modifier::BOLD),
        );
    let mut state = ListState::default();
    // Clamp: the row list can shrink under us if a refresh changes nothing,
    // and an out-of-range index would panic ratatui's stateful widget.
    if !app.rows().is_empty() {
        state.select(Some(app.selected().min(app.rows().len() - 1)));
    }
    frame.render_stateful_widget(list, chunks[1], &mut state);

    let footer_at = chunks.len() - 1;
    if app.show_pane() {
        let pane_at = footer_at - 1;
        frame.render_widget(pane(app), chunks[pane_at]);
    }
    frame.render_widget(footer(app), chunks[footer_at]);
}

/// The live report pane, reusing the shared `Report` rendering.
fn pane(app: &App) -> Paragraph<'static> {
    let mut lines: Vec<Line> = Vec::new();
    for report in app.reports.iter() {
        lines.push(Line::from(report.describe()));
    }
    if lines.is_empty() {
        lines.push(Line::from(Span::styled(
            "no reports yet — press a key or turn the knob",
            Style::default().add_modifier(Modifier::DIM),
        )));
    }
    // Only the tail fits; the newest reports are the point.
    let visible = lines.len().saturating_sub(20);
    let held = app.reports.len();
    let mut title = format!(" reports ({held} kept");
    if app.reports.dropped() > 0 {
        title.push_str(&format!(", {} dropped", app.reports.dropped()));
    }
    title.push_str(") ");

    Paragraph::new(lines.into_iter().skip(visible).collect::<Vec<_>>())
        .block(Block::default().borders(Borders::ALL).title(title))
}

fn title(app: &App) -> String {
    match app.mode() {
        Mode::Normal => " minikbconfig — layer 0 ".to_string(),
        Mode::Editing { input } => format!(" minikbconfig — set: {input}_ "),
        Mode::Verifying { slot, action } => format!(
            " minikbconfig — press key[{slot}] to confirm it emits {} ",
            minikbconfig_config::describe(*action)
        ),
    }
}

fn footer(app: &App) -> Line<'static> {
    let hint = Line::from(vec![
        Span::raw("↑/↓ "),
        Span::styled("move", Style::default().add_modifier(Modifier::DIM)),
        Span::raw("  enter "),
        Span::styled("set", Style::default().add_modifier(Modifier::DIM)),
        Span::raw("  r "),
        Span::styled("refresh", Style::default().add_modifier(Modifier::DIM)),
        Span::raw("  v "),
        Span::styled("reports", Style::default().add_modifier(Modifier::DIM)),
        Span::raw("  q "),
        Span::styled("quit", Style::default().add_modifier(Modifier::DIM)),
    ]);

    match app.status() {
        Status::Ready => hint,
        Status::Warning(msg) => Line::from(vec![
            Span::styled("warning: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(msg.clone()),
        ]),
        Status::Failed(msg) => Line::from(vec![
            Span::styled("error: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(msg.clone()),
        ]),
    }
}
