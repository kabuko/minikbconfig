//! Drawing.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};

use crate::app::{App, LightingField, LightingState, Mode, Status};

pub fn draw(frame: &mut Frame, app: &App) {
    let area = frame.area();

    if let Mode::Lighting(lighting) = app.mode() {
        let constraints = vec![
            Constraint::Length(3),
            Constraint::Min(0),
            Constraint::Length(1),
        ];
        let chunks = Layout::vertical(constraints).split(area);

        let header = Block::default().borders(Borders::ALL).title(title(app));
        frame.render_widget(header, chunks[0]);

        let panel = lighting_panel(lighting, app.pad_keys());
        frame.render_widget(panel, chunks[1]);

        frame.render_widget(footer(app), chunks[2]);
        return;
    }

    // The pane takes a fixed slice of the lower area; the slot list keeps the
    // rest. Vertical rather than side-by-side, because the slot list is 28 rows
    // tall and a narrow column would truncate the actions.
    let is_editing = matches!(app.mode(), Mode::Editing { .. });
    let constraints = if is_editing {
        vec![
            Constraint::Length(3),
            Constraint::Min(4),
            Constraint::Length(8),
            Constraint::Length(1),
        ]
    } else if app.show_pane() {
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
    if let Mode::Editing {
        input,
        selected_suggestion,
    } = app.mode()
    {
        let (list, mut state) = suggestions_pane(input, *selected_suggestion);
        frame.render_stateful_widget(list, chunks[2], &mut state);
    } else if app.show_pane() {
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

fn suggestions_pane(input: &str, selected: usize) -> (List<'static>, ListState) {
    let candidates = minikbconfig_config::complete::suggestions(input);
    let total = candidates.len();
    let items: Vec<ListItem> = candidates
        .into_iter()
        .enumerate()
        .map(|(i, cand)| {
            let marker = if i == selected { "> " } else { "  " };
            let style = if i == selected {
                Style::default().add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            ListItem::new(Line::from(vec![
                Span::styled(marker, style),
                Span::raw(cand),
            ]))
        })
        .collect();

    let title = if total == 0 {
        " completions (none) ".to_string()
    } else {
        format!(" completions ({total}) ")
    };

    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(title))
        .highlight_style(
            Style::default()
                .add_modifier(Modifier::REVERSED)
                .add_modifier(Modifier::BOLD),
        );

    let mut state = ListState::default();
    if total > 0 {
        state.select(Some(selected.min(total - 1)));
    }
    (list, state)
}

fn lighting_panel(state: &LightingState, keys: u8) -> Paragraph<'static> {
    let mut lines = Vec::new();

    let field_line = |f: LightingField, label: &str, val: String| -> Line<'static> {
        let is_selected = state.field == f;
        let marker = if is_selected { "> " } else { "  " };
        let marker_style = if is_selected {
            Style::default().add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        let label_style = if is_selected {
            Style::default().add_modifier(Modifier::BOLD)
        } else {
            Style::default().add_modifier(Modifier::DIM)
        };
        let val_style = if is_selected {
            Style::default()
                .add_modifier(Modifier::BOLD)
                .add_modifier(Modifier::REVERSED)
        } else {
            Style::default()
        };
        Line::from(vec![
            Span::styled(marker, marker_style),
            Span::styled(format!("{:<16}", label), label_style),
            Span::styled(val, val_style),
        ])
    };

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  --- Global Lighting (applied to controller) ---",
        Style::default().add_modifier(Modifier::DIM),
    )));

    // Brightness 0-4
    let b_str = format!(
        "{} / 4 {}",
        state.config.brightness,
        if state.config.brightness == 0 {
            "(off)"
        } else {
            ""
        }
    );
    lines.push(field_line(LightingField::Brightness, "Brightness", b_str));

    // Mode 1-6
    let mode_desc = match state.config.mode {
        1 => "1 (solid / custom)",
        2 => "2 (breathing)",
        3 => "3 (wave)",
        4 => "4 (ripple)",
        5 => "5 (spectrum)",
        6 => "6 (reactive)",
        _other => "invalid",
    };
    let m_str = format!("{}  [{}]", state.config.mode, mode_desc);
    lines.push(field_line(LightingField::Mode, "Effect Mode", m_str));

    // Speed
    lines.push(field_line(
        LightingField::Speed,
        "Speed",
        format!("{}", state.config.speed),
    ));

    // Direction 0-3
    let dir_desc = match state.config.direction {
        0 => "right",
        1 => "left",
        2 => "down",
        3 => "up",
        _ => "unknown",
    };
    lines.push(field_line(
        LightingField::Direction,
        "Direction",
        format!("{} ({})", state.config.direction, dir_desc),
    ));

    // Global Color
    let global_rgb = state.current_global_rgb();
    let global_color_str = if !state.global_hex_input.is_empty() {
        format!("{:<7} (custom hex)", state.global_hex_input)
    } else {
        let preset_name =
            crate::app::COLOR_PRESETS[state.global_preset_idx % crate::app::COLOR_PRESETS.len()].0;
        format!(
            "#{:02x}{:02x}{:02x} ({})",
            global_rgb[0], global_rgb[1], global_rgb[2], preset_name
        )
    };
    lines.push(field_line(
        LightingField::GlobalColor,
        "Global Colour",
        global_color_str,
    ));

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  --- Per-Key Backlight Colour ---",
        Style::default().add_modifier(Modifier::DIM),
    )));

    let target_key_name =
        minikbconfig_core::keymap::control_name(state.target_key, keys).unwrap_or("unknown");
    lines.push(field_line(
        LightingField::TargetKey,
        "Target Key",
        format!("{} ({})", state.target_key, target_key_name),
    ));

    let key_rgb = state.current_key_rgb();
    let key_color_str = if !state.key_hex_input.is_empty() {
        format!("{:<7} (custom hex)", state.key_hex_input)
    } else {
        let preset_name =
            crate::app::COLOR_PRESETS[state.key_preset_idx % crate::app::COLOR_PRESETS.len()].0;
        format!(
            "#{:02x}{:02x}{:02x} ({})",
            key_rgb[0], key_rgb[1], key_rgb[2], preset_name
        )
    };
    lines.push(field_line(
        LightingField::KeyColor,
        "Key Colour",
        key_color_str,
    ));

    lines.push(Line::from(""));
    let status_summary = format!(
        "  Current state: {} (mode {}, brightness {}, hue {}, sat {}, val {})",
        if state.config.is_lit() { "lit" } else { "dark" },
        state.config.mode,
        state.config.brightness,
        state.config.hue,
        state.config.saturation,
        state.config.value,
    );
    lines.push(Line::from(Span::styled(
        status_summary,
        Style::default().add_modifier(Modifier::DIM),
    )));

    Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" lighting configuration "),
    )
}

fn title(app: &App) -> String {
    match app.mode() {
        Mode::Normal => " minikbconfig — layer 0 ".to_string(),
        Mode::Editing { input, .. } => {
            let slot = app
                .rows()
                .get(app.selected())
                .map(|r| r.index())
                .unwrap_or(0);
            format!(" minikbconfig — set key[{slot}]: {input}_ ")
        }
        Mode::Verifying { slot, action } => format!(
            " minikbconfig — press key[{slot}] to confirm it emits {} ",
            minikbconfig_config::describe(*action)
        ),
        Mode::Lighting(_) => " minikbconfig — lighting ".to_string(),
    }
}

fn footer(app: &App) -> Line<'static> {
    if let Mode::Editing { input, .. } = app.mode() {
        return editing_footer(input);
    }
    if let Mode::Lighting(_) = app.mode() {
        return lighting_footer(app.status());
    }

    let hint = Line::from(vec![
        Span::raw("↑/↓ "),
        Span::styled("move", Style::default().add_modifier(Modifier::DIM)),
        Span::raw("  enter "),
        Span::styled("set", Style::default().add_modifier(Modifier::DIM)),
        Span::raw("  r "),
        Span::styled("refresh", Style::default().add_modifier(Modifier::DIM)),
        Span::raw("  l "),
        Span::styled("lighting", Style::default().add_modifier(Modifier::DIM)),
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

fn lighting_footer(status: &Status) -> Line<'static> {
    let hint = Line::from(vec![
        Span::styled("↑/↓ ", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw("field  "),
        Span::styled("←/→ ", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw("adjust  "),
        Span::styled("1-6 ", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw("mode  "),
        Span::styled("enter ", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw("apply  "),
        Span::styled("esc ", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw("back  "),
        Span::styled("q ", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw("quit"),
    ]);

    match status {
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

fn editing_footer(input: &str) -> Line<'static> {
    match minikbconfig_config::complete::validate(input) {
        minikbconfig_config::complete::EditValidation::Empty => Line::from(vec![
            Span::styled("tab ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw("complete  "),
            Span::styled("enter ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw("apply  "),
            Span::styled("esc ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw("cancel  "),
            Span::styled(
                "(type 'key:', 'media:' or 'none')",
                Style::default().add_modifier(Modifier::DIM),
            ),
        ]),
        minikbconfig_config::complete::EditValidation::Valid(action) => Line::from(vec![
            Span::styled("valid: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(minikbconfig_config::describe(action)),
            Span::styled(
                "  (enter to apply, tab to cycle)",
                Style::default().add_modifier(Modifier::DIM),
            ),
        ]),
        minikbconfig_config::complete::EditValidation::Incomplete { matches } => Line::from(vec![
            Span::styled("tab ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw("complete  "),
            Span::styled("↑/↓ ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(format!("browse {matches} candidates  ")),
            Span::styled("esc ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw("cancel"),
        ]),
        minikbconfig_config::complete::EditValidation::Invalid(err) => Line::from(vec![
            Span::styled("error: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(err),
        ]),
    }
}
