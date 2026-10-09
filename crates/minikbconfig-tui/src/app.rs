//! Application state and the event loop.

use std::time::{Duration, Instant};

use minikbconfig_config::{describe, parse_action};
use minikbconfig_core::emitted::Emitted;
use minikbconfig_core::keymap::{KEY_SLOTS, KeyTable, KeyType, control_name};
use minikbconfig_hid::pad::{Pad, Watcher};
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind};

/// How long to wait for the user to press a control before giving up on
/// confirming a write.
const VERIFY_TIMEOUT: Duration = Duration::from_secs(10);

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

/// What the app is currently doing.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Mode {
    /// Browsing slots.
    #[default]
    Normal,
    /// Typing an action string.
    Editing { input: String },
    /// Written and read back; waiting for the user to press the control to
    /// confirm the pad actually emits it.
    Verifying { slot: u8, action: KeyType },
}

pub struct App {
    pad: Pad,
    rows: Vec<Row>,
    selected: usize,
    status: Status,
    mode: Mode,
    /// Held only while verifying, since opening the input interfaces is not
    /// free and there is no reason to hold them otherwise.
    watcher: Option<Watcher>,
    verify_deadline: Option<Instant>,
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
            mode: Mode::default(),
            watcher: None,
            verify_deadline: None,
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
            self.poll_verification();
            terminal.draw(|frame| super::ui::draw(frame, self))?;
            if event::poll(Duration::from_millis(200))? {
                if let Event::Key(key) = event::read()? {
                    if key.kind == KeyEventKind::Press && self.on_key(key.code)? {
                        return Ok(());
                    }
                }
            }
        }
    }

    /// Handle a key press. Returns true when the app should exit.
    fn on_key(&mut self, code: KeyCode) -> anyhow::Result<bool> {
        // Editing and verifying own every key while they are active, so a
        // stray 'q' while typing an action does not quit the app.
        if !matches!(self.mode, Mode::Normal) {
            match (&mut self.mode, code) {
                (Mode::Editing { .. }, KeyCode::Esc) => self.cancel(),
                (Mode::Editing { input }, KeyCode::Enter) => {
                    let text = std::mem::take(input);
                    self.submit(&text);
                }
                (Mode::Editing { input }, KeyCode::Backspace) => {
                    input.pop();
                }
                (Mode::Editing { input }, KeyCode::Char(c)) => input.push(c),
                (Mode::Verifying { .. }, KeyCode::Esc) => self.cancel(),
                _ => {}
            }
            return Ok(false);
        }

        match code {
            KeyCode::Char('q') | KeyCode::Char('Q') | KeyCode::Esc => return Ok(true),
            KeyCode::Char('r') | KeyCode::Char('R') => self.reload(),
            KeyCode::Down | KeyCode::Char('j') => self.select_next(),
            KeyCode::Up | KeyCode::Char('k') => self.select_previous(),
            KeyCode::Enter => self.begin_edit(),
            _ => {}
        }
        Ok(false)
    }

    fn begin_edit(&mut self) {
        let Some(row) = self.rows.get(self.selected) else {
            return;
        };
        // Seed the input with the current value so Enter re-applies it.
        let current = if row.action == KeyType::Unassigned {
            String::new()
        } else {
            describe(row.action)
        };
        self.mode = Mode::Editing { input: current };
    }

    /// Apply the typed action, then verify it by observation.
    fn submit(&mut self, input: &str) {
        let trimmed = input.trim();
        if trimmed.is_empty() {
            self.cancel();
            return;
        }
        let Some(row) = self.rows.get(self.selected) else {
            self.cancel();
            return;
        };
        let slot = row.index;

        let action = match parse_action(trimmed) {
            Ok(action) => action,
            Err(err) => {
                // Stay in edit mode with the text, so it can be corrected
                // rather than retyped.
                self.status = Status::Failed(format!("{err}"));
                return;
            }
        };

        if let Err(err) = self.pad.write_key(slot, action, 0) {
            self.status = Status::Failed(format!("{err:#}"));
            self.cancel();
            return;
        }

        // Step 2 of the loop: confirm the device stored it. Necessary, not
        // sufficient -- see the step 3 below.
        match self.pad.read_key(slot, 0) {
            Ok(stored) if stored == action => {}
            Ok(stored) => {
                self.status = Status::Failed(format!(
                    "key[{slot}] did not take effect: wanted {}, device returned {}",
                    describe(action),
                    describe(stored)
                ));
                self.cancel();
                return;
            }
            Err(err) => {
                self.status = Status::Failed(format!("{err:#}"));
                self.cancel();
                return;
            }
        }

        if !Emitted::is_observable_for(action) {
            self.status = Status::Failed(format!(
                "key[{slot}] stored {}, but nothing observable confirms it",
                describe(action)
            ));
            self.cancel();
            return;
        }

        match Watcher::open() {
            Ok(watcher) => {
                self.watcher = Some(watcher);
                self.verify_deadline = Some(Instant::now() + VERIFY_TIMEOUT);
                self.mode = Mode::Verifying { slot, action };
                self.status = Status::Ready;
            }
            Err(err) => {
                self.status = Status::Failed(format!("stored, but cannot verify: {err:#}"));
                self.cancel();
            }
        }
    }

    /// Step 3: wait for the pad to actually emit what was written.
    fn poll_verification(&mut self) {
        let deadline = match self.verify_deadline {
            Some(d) => d,
            None => return,
        };
        let Mode::Verifying { slot, action } = self.mode.clone() else {
            return;
        };

        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            self.status = Status::Failed(format!(
                "key[{slot}] stored but nothing was pressed; not confirmed"
            ));
            self.cancel();
            return;
        }

        let Some(watcher) = &self.watcher else {
            self.cancel();
            return;
        };
        match watcher.next_press(remaining.min(Duration::from_millis(100))) {
            Ok(Some(event)) => {
                self.status = if event.matches(action) {
                    Status::Ready
                } else {
                    Status::Failed(format!(
                        "key[{slot}] should emit {} but emitted {}",
                        describe(action),
                        event.describe()
                    ))
                };
                self.refresh_rows();
                self.cancel();
            }
            Ok(None) => {}
            Err(err) => {
                self.status = Status::Failed(format!("{err:#}"));
                self.cancel();
            }
        }
    }

    fn cancel(&mut self) {
        self.mode = Mode::Normal;
        self.watcher = None;
        self.verify_deadline = None;
    }

    /// Re-read after a write so the list shows the new value immediately.
    fn refresh_rows(&mut self) {
        if let Ok(table) = self.pad.read_keys(0) {
            self.rows = build_rows(&table);
            self.clamp_selection();
        }
    }

    fn clamp_selection(&mut self) {
        if !self.rows.is_empty() {
            self.selected = self.selected.min(self.rows.len() - 1);
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

    pub fn mode(&self) -> &Mode {
        &self.mode
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
