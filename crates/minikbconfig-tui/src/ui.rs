//! Drawing.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState};

use crate::app::{App, Mode, Status};

pub fn draw(frame: &mut Frame, app: &App) {
    let area = frame.area();
    let chunks = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .split(area);

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

    frame.render_widget(footer(app), chunks[2]);
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
