//! Application state and the event loop.

use std::time::{Duration, Instant};

use minikbconfig_config::{describe, parse_action};
use minikbconfig_core::emitted::{Emitted, Match};
use minikbconfig_core::keymap::{
    KEY_SLOTS, KeyCode as HidKeyCode, KeyTable, KeyType, control_name,
};
use minikbconfig_hid::pad::{Pad, Report, Watcher};
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
    /// The write worked, but something else is wrong — e.g. a stuck key.
    Warning(String),
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
    /// Live reports from the pad, shown in the bottom pane.
    pub reports: RecentReports,
    show_pane: bool,
    /// Opened on first use: holding the input interfaces is not free, and there
    /// is no reason to before the user asks to see them.
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
            reports: RecentReports::new(),
            show_pane: false,
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
            self.collect_reports();
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
            KeyCode::Char('v') | KeyCode::Char('V') => self.toggle_pane(),
            KeyCode::Char('c') | KeyCode::Char('C') if self.show_pane() => self.reports.clear(),
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

        match self.ensure_watcher() {
            Ok(()) => {
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
                // A stuck extra key is not the same fault as a wrong action:
                // the write landed, but the pad believes something else is
                // still held. Say which, rather than reporting a good write as
                // a failure.
                self.status = match event.compare(action) {
                    Match::Exact => Status::Ready,
                    Match::ExtraKeys(extra) => {
                        let names: Vec<String> =
                            extra.iter().map(|c| HidKeyCode(*c).name()).collect();
                        Status::Warning(format!(
                            "key[{slot}] confirmed, but the pad is also emitting {} — \
                             that looks like a key it never released; unplug it to clear",
                            names.join("+")
                        ))
                    }
                    Match::Mismatch => Status::Failed(format!(
                        "key[{slot}] should emit {} but emitted {}",
                        describe(action),
                        event.describe()
                    )),
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
        self.verify_deadline = None;
    }

    /// Open the input interfaces once and keep them.
    fn ensure_watcher(&mut self) -> anyhow::Result<()> {
        if self.watcher.is_none() {
            self.watcher = Some(Watcher::open()?);
        }
        Ok(())
    }

    /// Toggle the live report pane.
    pub fn toggle_pane(&mut self) {
        match self.show_pane {
            true => {
                self.show_pane = false;
                self.reports.clear();
            }
            false => {
                if let Err(err) = self.ensure_watcher() {
                    self.status = Status::Failed(format!("cannot watch the pad: {err:#}"));
                    return;
                }
                self.show_pane = true;
            }
        }
    }

    /// Collect whatever has arrived since the last frame.
    pub fn collect_reports(&mut self) {
        if !self.show_pane {
            return;
        }
        let Some(watcher) = &self.watcher else {
            return;
        };
        let mut batch = Vec::new();
        if watcher.drain_reports(&mut batch, PANE_PER_FRAME).is_ok() {
            for report in batch {
                self.reports.push(report);
            }
        }
    }

    pub fn show_pane(&self) -> bool {
        self.show_pane
    }

    /// Re-read after a write so the list shows the new value immediately.
    fn refresh_rows(&mut self) {
        if let Ok(table) = self.pad.read_keys(0) {
            self.rows = build_rows(&table);
            self.clamp_selection();
        }
    }

    fn clamp_selection(&mut self) {
        self.selected = move_selection(self.selected, 0, self.rows.len());
    }

    /// Re-read the device, reporting failure in the footer rather than exiting,
    /// so a pad that gets unplugged does not throw away the last known state.
    fn reload(&mut self) {
        if let Err(err) = self.refresh() {
            self.status = Status::Failed(format!("{err:#}"));
        }
    }

    fn select_next(&mut self) {
        self.selected = move_selection(self.selected, 1, self.rows.len());
    }

    fn select_previous(&mut self) {
        self.selected = move_selection(self.selected, -1, self.rows.len());
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

/// Move a selection index within `len` items, clamping at both ends.
///
/// Pure so it can be tested: selection drifting off the end of a list is easy to
/// miss by eye and easy to pin down here.
pub fn move_selection(current: usize, delta: isize, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    (current as isize + delta).clamp(0, len as isize - 1) as usize
}

/// How many reports the pane keeps.
const PANE_CAPACITY: usize = 200;

/// How many reports a single frame may collect.
///
/// A burst must not stall a frame; the rest are picked up next time.
const PANE_PER_FRAME: usize = 64;

/// A bounded, newest-last log of reports.
///
/// A plain `Vec` with a cap: the oldest are dropped, because the interesting
/// end when watching something is always the most recent.
#[derive(Debug, Default)]
pub struct RecentReports {
    items: Vec<Report>,
    dropped: usize,
}

impl RecentReports {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, report: Report) {
        if self.items.len() >= PANE_CAPACITY {
            self.items.remove(0);
            self.dropped += 1;
        }
        self.items.push(report);
    }

    pub fn clear(&mut self) {
        self.items.clear();
        self.dropped = 0;
    }

    /// How many reports have been dropped because the buffer was full.
    pub fn dropped(&self) -> usize {
        self.dropped
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn iter(&self) -> impl Iterator<Item = &Report> {
        self.items.iter()
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

    #[test]
    fn selection_moves_and_clamps_at_both_ends() {
        assert_eq!(move_selection(0, 1, 5), 1);
        assert_eq!(move_selection(3, -1, 5), 2);
        // Clamped, never wrapping.
        assert_eq!(move_selection(0, -1, 5), 0);
        assert_eq!(move_selection(4, 1, 5), 4);
        assert_eq!(move_selection(4, 99, 5), 4);
    }

    #[test]
    fn selection_into_an_empty_list_is_harmless() {
        assert_eq!(move_selection(0, 1, 0), 0);
        assert_eq!(move_selection(0, -1, 0), 0);
        // And an index left over from a longer list is pulled back in range.
        assert_eq!(move_selection(7, 0, 3), 2);
    }
}

#[cfg(test)]
mod pane_tests {
    use super::*;
    use minikbconfig_hid::InputKind;
    use minikbconfig_hid::pad::Report;

    fn report(byte: u8) -> Report {
        let raw = [byte; 8];
        Report {
            kind: InputKind::Boot,
            bytes: raw.to_vec(),
            event: Emitted::from_boot(&raw),
        }
    }

    #[test]
    fn the_pane_keeps_the_newest_reports_and_counts_what_it_dropped() {
        // A knob turn produces reports faster than a frame redraws, so the
        // buffer has to be bounded. Silently forgetting the oldest would make
        // the pane lie about what it saw, so the drop count is reported.
        let mut pane = RecentReports::new();
        for i in 0..(PANE_CAPACITY + 10) {
            pane.push(report(i as u8));
        }
        assert_eq!(pane.len(), PANE_CAPACITY);
        assert_eq!(pane.dropped(), 10);
        // The newest is the one that survived.
        let last = pane.iter().last().unwrap();
        assert_eq!(last.bytes[0], (PANE_CAPACITY + 9) as u8);
    }

    #[test]
    fn clearing_resets_the_dropped_count() {
        let mut pane = RecentReports::new();
        for i in 0..(PANE_CAPACITY + 3) {
            pane.push(report(i as u8));
        }
        assert!(pane.dropped() > 0);
        pane.clear();
        assert_eq!(pane.dropped(), 0);
        assert_eq!(pane.iter().count(), 0);
    }

    #[test]
    fn an_empty_pane_is_empty_rather_than_unbounded() {
        let pane = RecentReports::new();
        assert_eq!(pane.iter().count(), 0);
        assert_eq!(pane.dropped(), 0);
    }
}
