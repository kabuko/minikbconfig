//! Application state and the event loop.

use std::time::Duration;

use minikbconfig_config::describe;
use minikbconfig_core::keymap::{control_name, KeyTable, KeyType, KEY_SLOTS};
use minikbconfig_hid::pad::Pad;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind};

/// One row per addressable slot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub index: u8,
    pub control: String,
    pub action: KeyType,
}

/// What happened on the last device read, shown in the footer.
///
/// There is deliberately no "reading" state: reading the pad is synchronous and
/// blocks the event loop, so the user would never see it. A UI that claims to
/// show progress it cannot actually paint is worse than one that does not
/// pretend.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Status {
    #[default]
    Ready,
    Failed(String),
}

pub struct App {
    pad: Pad,
    rows: Vec<Row>,
    selected: usize,
    status: Status,
}

impl App {
    /// Open the pad and take a first reading of every slot.
    pub fn new() -> anyhow::Result<App> {
        let pad = Pad::open()?;
        Ok(App {
            pad,
            rows: Vec::new(),
            selected: 0,
            status: Status::default(),
        })
    }

    /// Read every slot from the device, replacing what was shown.
    pub fn refresh(&mut self) -> anyhow::Result<()> {
        let table = self.pad.read_keys(0)?;
        self.rows = build_rows(&table);
        self.status = Status::Ready;
        Ok(())
    }

    pub fn run(&mut self, terminal: &mut ratatui::DefaultTerminal) -> anyhow::Result<()> {
        self.refresh()?;
        loop {
            terminal.draw(|frame| super::ui::draw(frame, self))?;
            if event::poll(Duration::from_millis(200))? {
                match event::read()? {
                    Event::Key(key) if key.kind == KeyEventKind::Press => match key.code {
                        KeyCode::Char('q') | KeyCode::Char('Q') | KeyCode::Esc => return Ok(()),
                        KeyCode::Char('r') | KeyCode::Char('R') => self.reload(),
                        KeyCode::Down | KeyCode::Char('j') => self.select_next(),
                        KeyCode::Up | KeyCode::Char('k') => self.select_previous(),
                        _ => {}
                    },
                    Event::Resize(_, _) => {}
                    _ => {}
                }
            }
        }
    }

    /// Re-read the device, reporting failure in the footer rather than exiting,
    /// so a pad that gets unplugged does not throw away the last known state.
    fn reload(&mut self) {
        if let Err(err) = self.refresh() {
            self.status = Status::Failed(format!("{err:#}"));
        }
    }

    fn select_next(&mut self) {
        if !self.rows.is_empty() {
            self.selected = (self.selected + 1).min(self.rows.len() - 1);
        }
    }

    fn select_previous(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    pub fn selected(&self) -> usize {
        self.selected
    }

    pub fn status(&self) -> &Status {
        &self.status
    }
}

impl Row {
    pub fn index(&self) -> u8 {
        self.index
    }

    /// The physical control, or the bare slot number when the index does not
    /// map to one.
    pub fn label(&self) -> String {
        if self.control.is_empty() {
            format!("slot {}", self.index)
        } else {
            self.control.clone()
        }
    }

    pub fn action_text(&self) -> String {
        if self.action == KeyType::Unassigned {
            "—".to_string()
        } else {
            describe(self.action)
        }
    }
}

/// One row per slot, named where the slot maps to a physical control.
fn build_rows(table: &KeyTable) -> Vec<Row> {
    (0..KEY_SLOTS)
        .map(|index| Row {
            index,
            control: control_name(index).unwrap_or_default().to_string(),
            action: table.get(index).unwrap_or(KeyType::Unassigned),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_cover_every_slot_in_order() {
        let rows = build_rows(&KeyTable::new());
        assert_eq!(rows.len(), KEY_SLOTS as usize);
        assert_eq!(rows[0].index, 0);
        assert_eq!(rows[KEY_SLOTS as usize - 1].index, KEY_SLOTS - 1);
    }

    #[test]
    fn a_slot_with_no_control_falls_back_to_its_index() {
        let rows = build_rows(&KeyTable::new());
        // Slot 3 is not a physical control, so it must say so rather than
        // render as an empty cell.
        assert_eq!(rows[3].label(), "slot 3");
        assert_eq!(rows[0].label(), "key 1");
    }

    #[test]
    fn unassigned_slots_render_as_a_dash_not_as_an_action() {
        let rows = build_rows(&KeyTable::new());
        assert_eq!(rows[0].action_text(), "—");
    }
}
