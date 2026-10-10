//! Application state and the event loop.

use std::time::{Duration, Instant};

use minikbconfig_config::{describe, parse_action};
use minikbconfig_core::emitted::{Emitted, Match};
use minikbconfig_core::keymap::{
    KEY_SLOTS, KeyCode as HidKeyCode, KeyTable, KeyType, control_name,
};
use minikbconfig_core::light::{self, LightConfig, MAX_MODE, MIN_MODE};
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
    Editing {
        input: String,
        selected_suggestion: usize,
    },
    /// Written and read back; waiting for the user to press the control to
    /// confirm the pad actually emits it.
    Verifying { slot: u8, action: KeyType },
    /// Viewing and configuring pad lighting.
    Lighting(LightingState),
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
        self.rows = build_rows(&table, self.pad.keys());
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
            match &mut self.mode {
                Mode::Editing {
                    input,
                    selected_suggestion,
                } => match handle_edit_key(input, selected_suggestion, code) {
                    EditOutcome::Cancel => self.cancel(),
                    EditOutcome::Submit => {
                        let text = std::mem::take(input);
                        self.submit(&text);
                    }
                    EditOutcome::Continue => {}
                },
                Mode::Verifying { .. } => {
                    if code == KeyCode::Esc {
                        self.cancel();
                    }
                }
                Mode::Lighting(state) => match handle_lighting_key(state, self.pad.keys(), code) {
                    LightingAction::None => {}
                    LightingAction::WriteGlobal(cfg) => match self.pad.set_light_config(cfg) {
                        Ok(stored) => {
                            state.config = stored;
                            // The device decides what was actually stored, so
                            // the mode can differ from the one requested - and
                            // that can hide the per-key fields.
                            state.clamp_field_to_mode();
                            self.status = Status::Ready;
                        }
                        Err(err) => {
                            self.status = Status::Failed(format!("{err:#}"));
                        }
                    },
                    LightingAction::WriteKeyColor { key, rgb } => {
                        match self.pad.set_key_color(key, rgb) {
                            Ok(()) => {
                                self.status = Status::Ready;
                            }
                            Err(err) => {
                                self.status = Status::Failed(format!("{err:#}"));
                            }
                        }
                    }
                    LightingAction::Refused(msg) => {
                        self.status = Status::Failed(msg);
                    }
                    LightingAction::Exit => {
                        self.mode = Mode::Normal;
                    }
                    LightingAction::Quit => return Ok(true),
                },
                Mode::Normal => {}
            }
            return Ok(false);
        }

        match code {
            KeyCode::Char('q') | KeyCode::Char('Q') | KeyCode::Esc => return Ok(true),
            KeyCode::Char('r') | KeyCode::Char('R') => self.reload(),
            KeyCode::Char('l') | KeyCode::Char('L') => self.open_lighting(),
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
        self.mode = Mode::Editing {
            input: current,
            selected_suggestion: 0,
        };
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
                self.mode = Mode::Editing {
                    input: trimmed.to_string(),
                    selected_suggestion: 0,
                };
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
            self.rows = build_rows(&table, self.pad.keys());
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

    pub fn pad_keys(&self) -> u8 {
        self.pad.keys()
    }

    /// Enter lighting mode. Reads the current light config from the pad.
    fn open_lighting(&mut self) {
        match self.pad.light_config() {
            Ok(config) => {
                self.mode = Mode::Lighting(LightingState::new(config, self.pad.keys()));
                self.status = Status::Ready;
            }
            Err(err) => {
                self.status = Status::Failed(format!("failed to read light config: {err:#}"));
            }
        }
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

/// What the edit loop should do in response to a key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditOutcome {
    Continue,
    Submit,
    Cancel,
}

/// Handle a key event in editing mode.
///
/// Pure so it can be tested without hardware.
pub fn handle_edit_key(
    input: &mut String,
    selected_suggestion: &mut usize,
    code: KeyCode,
) -> EditOutcome {
    match code {
        KeyCode::Esc => EditOutcome::Cancel,
        KeyCode::Enter => EditOutcome::Submit,
        KeyCode::Backspace => {
            input.pop();
            *selected_suggestion = 0;
            EditOutcome::Continue
        }
        KeyCode::Char(c) => {
            input.push(c);
            *selected_suggestion = 0;
            EditOutcome::Continue
        }
        KeyCode::Down => {
            let suggs = minikbconfig_config::complete::suggestions(input);
            if !suggs.is_empty() {
                *selected_suggestion = (*selected_suggestion + 1).min(suggs.len() - 1);
            }
            EditOutcome::Continue
        }
        KeyCode::Up => {
            if *selected_suggestion > 0 {
                *selected_suggestion -= 1;
            }
            EditOutcome::Continue
        }
        KeyCode::Tab => {
            let suggs = minikbconfig_config::complete::suggestions(input);
            if !suggs.is_empty() {
                if input.as_str() == suggs[*selected_suggestion].as_str() {
                    *selected_suggestion = (*selected_suggestion + 1) % suggs.len();
                }
                *input = suggs[*selected_suggestion].clone();
            }
            EditOutcome::Continue
        }
        KeyCode::BackTab => {
            let suggs = minikbconfig_config::complete::suggestions(input);
            if !suggs.is_empty() {
                *selected_suggestion = selected_suggestion
                    .checked_sub(1)
                    .unwrap_or(suggs.len() - 1);
                *input = suggs[*selected_suggestion].clone();
            }
            EditOutcome::Continue
        }
        _ => EditOutcome::Continue,
    }
}

/// Color presets available for quick selection in the lighting panel.
pub const COLOR_PRESETS: [(&str, [u8; 3]); 8] = [
    ("white", [0xFF, 0xFF, 0xFF]),
    ("red", [0xFF, 0x00, 0x00]),
    ("green", [0x00, 0xFF, 0x00]),
    ("blue", [0x00, 0x00, 0xFF]),
    ("cyan", [0x00, 0xFF, 0xFF]),
    ("magenta", [0xFF, 0x00, 0xFF]),
    ("yellow", [0xFF, 0xFF, 0x00]),
    ("black", [0x00, 0x00, 0x00]),
];

/// Parse `#RRGGBB` into `[r, g, b]` bytes.
pub fn parse_rgb_hex(hex: &str) -> Option<[u8; 3]> {
    let s = hex.trim().trim_start_matches('#');
    if s.len() != 6 || !s.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let r = u8::from_str_radix(&s[0..2], 16).ok()?;
    let g = u8::from_str_radix(&s[2..4], 16).ok()?;
    let b = u8::from_str_radix(&s[4..6], 16).ok()?;
    Some([r, g, b])
}

/// Configurable fields in the lighting panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LightingField {
    Brightness,
    Mode,
    Speed,
    Direction,
    GlobalColor,
    TargetKey,
    KeyColor,
}

/// Whether the current mode displays stored per-key colours.
///
/// Only mode 5 does. Measured on the 6-key model: in modes 1 to 4 every key is
/// painted with the *global* colour and a per-key write is invisible, which
/// looks exactly like the write having failed. Mode 0 is off. So the per-key
/// controls are only meaningful in mode 5, and showing them elsewhere invites
/// the user to write a colour that provably does nothing.
///
/// The vendor calls mode 5 "custom"; it is the per-key view.
pub const PER_KEY_MODE: u8 = 5;

impl LightingField {
    /// Fields available in every mode.
    pub const GLOBAL: [LightingField; 5] = [
        LightingField::Brightness,
        LightingField::Mode,
        LightingField::Speed,
        LightingField::Direction,
        LightingField::GlobalColor,
    ];

    /// Fields that only mean something when per-key colour is displayed.
    pub const PER_KEY: [LightingField; 2] = [LightingField::TargetKey, LightingField::KeyColor];

    /// The fields to cycle through, given the current mode.
    pub fn for_mode(mode: u8) -> Vec<LightingField> {
        let mut out = Self::GLOBAL.to_vec();
        if mode == PER_KEY_MODE {
            out.extend_from_slice(&Self::PER_KEY);
        }
        out
    }
}

/// State for the lighting configuration panel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LightingState {
    pub config: LightConfig,
    pub field: LightingField,
    pub global_preset_idx: usize,
    pub global_hex_input: String,
    pub target_key: u8,
    pub key_preset_idx: usize,
    pub key_hex_input: String,
}

impl LightingState {
    /// Move the selection off a field the current mode does not show.
    ///
    /// Needed because the mode can change without the user navigating to the
    /// mode field: `r` re-reads the pad, and so does any write. Leaving the
    /// cursor on a hidden per-key field would strand it, since the field list
    /// is mode-dependent.
    pub fn clamp_field_to_mode(&mut self) {
        if !LightingField::for_mode(self.config.mode).contains(&self.field) {
            self.field = LightingField::GlobalColor;
        }
    }

    pub fn new(config: LightConfig, _keys: u8) -> Self {
        LightingState {
            config,
            field: LightingField::Brightness,
            global_preset_idx: 0,
            global_hex_input: String::new(),
            target_key: 0,
            key_preset_idx: 0,
            key_hex_input: String::new(),
        }
    }

    pub fn current_global_rgb(&self) -> [u8; 3] {
        if let Some(rgb) = parse_rgb_hex(&self.global_hex_input) {
            rgb
        } else {
            COLOR_PRESETS[self.global_preset_idx % COLOR_PRESETS.len()].1
        }
    }

    pub fn current_key_rgb(&self) -> [u8; 3] {
        if let Some(rgb) = parse_rgb_hex(&self.key_hex_input) {
            rgb
        } else {
            COLOR_PRESETS[self.key_preset_idx % COLOR_PRESETS.len()].1
        }
    }
}

/// Action produced by `handle_lighting_key`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LightingAction {
    None,
    WriteGlobal(LightConfig),
    WriteKeyColor { key: u8, rgb: [u8; 3] },
    Refused(String),
    Exit,
    Quit,
}

/// Pure handler for key events in lighting mode.
pub fn handle_lighting_key(
    state: &mut LightingState,
    pad_keys: u8,
    code: KeyCode,
) -> LightingAction {
    let max_key = pad_keys.saturating_sub(1);
    match code {
        KeyCode::Esc => LightingAction::Exit,
        KeyCode::Char('q') | KeyCode::Char('Q') => LightingAction::Quit,
        KeyCode::Down | KeyCode::Char('j') => {
            let fields = LightingField::for_mode(state.config.mode);
            let at = fields.iter().position(|f| *f == state.field).unwrap_or(0);
            state.field = fields[(at + 1) % fields.len()];
            LightingAction::None
        }
        KeyCode::Up | KeyCode::Char('k') => {
            let fields = LightingField::for_mode(state.config.mode);
            let at = fields.iter().position(|f| *f == state.field).unwrap_or(0);
            state.field = fields[(at + fields.len() - 1) % fields.len()];
            LightingAction::None
        }
        KeyCode::Char(c @ '0'..='9') => {
            let digit = c as u8 - b'0';
            match state.field {
                LightingField::Mode => {
                    if (MIN_MODE..=MAX_MODE).contains(&digit) {
                        state.config.mode = digit;
                        state.clamp_field_to_mode();
                        LightingAction::WriteGlobal(state.config)
                    } else {
                        LightingAction::Refused(format!(
                            "mode must be {MIN_MODE}..={MAX_MODE} (got '{c}'); modes outside 1-6 are silently discarded by firmware"
                        ))
                    }
                }
                LightingField::Brightness => {
                    if digit <= 4 {
                        state.config.brightness = digit;
                        LightingAction::WriteGlobal(state.config)
                    } else {
                        LightingAction::Refused(format!("brightness must be 0..=4 (got '{c}')"))
                    }
                }
                LightingField::TargetKey => {
                    if digit <= max_key {
                        state.target_key = digit;
                        LightingAction::None
                    } else {
                        LightingAction::Refused(format!(
                            "key index must be 0..={max_key} (got '{c}')"
                        ))
                    }
                }
                LightingField::GlobalColor => {
                    if state.global_hex_input.len() < 7 {
                        state.global_hex_input.push(c);
                    }
                    LightingAction::None
                }
                LightingField::KeyColor => {
                    if state.key_hex_input.len() < 7 {
                        state.key_hex_input.push(c);
                    }
                    LightingAction::None
                }
                _ => LightingAction::None,
            }
        }
        KeyCode::Char(c) if c.is_ascii_hexdigit() || c == '#' => match state.field {
            LightingField::GlobalColor => {
                if state.global_hex_input.len() < 7 {
                    state.global_hex_input.push(c);
                }
                LightingAction::None
            }
            LightingField::KeyColor => {
                if state.key_hex_input.len() < 7 {
                    state.key_hex_input.push(c);
                }
                LightingAction::None
            }
            _ => LightingAction::None,
        },
        KeyCode::Backspace => match state.field {
            LightingField::GlobalColor => {
                state.global_hex_input.pop();
                LightingAction::None
            }
            LightingField::KeyColor => {
                state.key_hex_input.pop();
                LightingAction::None
            }
            _ => LightingAction::None,
        },
        KeyCode::Left | KeyCode::Char('h') => match state.field {
            LightingField::Brightness => {
                state.config.brightness = state.config.brightness.saturating_sub(1);
                LightingAction::WriteGlobal(state.config)
            }
            LightingField::Mode => {
                let m = state.config.mode;
                if m > MIN_MODE && m <= MAX_MODE {
                    state.config.mode = m - 1;
                } else {
                    state.config.mode = MIN_MODE;
                }
                LightingAction::WriteGlobal(state.config)
            }
            LightingField::Speed => {
                state.config.speed = state.config.speed.saturating_sub(1);
                LightingAction::WriteGlobal(state.config)
            }
            LightingField::Direction => {
                state.config.direction = if state.config.direction == 0 {
                    3
                } else {
                    state.config.direction - 1
                };
                LightingAction::WriteGlobal(state.config)
            }
            LightingField::GlobalColor => {
                state.global_hex_input.clear();
                state.global_preset_idx = if state.global_preset_idx == 0 {
                    COLOR_PRESETS.len() - 1
                } else {
                    state.global_preset_idx - 1
                };
                let rgb = COLOR_PRESETS[state.global_preset_idx].1;
                let [h, s, v] = light::rgb_to_hsv(rgb);
                state.config.hue = h;
                state.config.saturation = s;
                state.config.value = v;
                state.config.cycle = 1;
                LightingAction::WriteGlobal(state.config)
            }
            LightingField::TargetKey => {
                state.target_key = state.target_key.saturating_sub(1);
                LightingAction::None
            }
            LightingField::KeyColor => {
                state.key_hex_input.clear();
                state.key_preset_idx = if state.key_preset_idx == 0 {
                    COLOR_PRESETS.len() - 1
                } else {
                    state.key_preset_idx - 1
                };
                let rgb = COLOR_PRESETS[state.key_preset_idx].1;
                LightingAction::WriteKeyColor {
                    key: state.target_key,
                    rgb,
                }
            }
        },
        KeyCode::Right | KeyCode::Char('l') => match state.field {
            LightingField::Brightness => {
                if state.config.brightness < 4 {
                    state.config.brightness += 1;
                }
                LightingAction::WriteGlobal(state.config)
            }
            LightingField::Mode => {
                let m = state.config.mode;
                if (MIN_MODE..MAX_MODE).contains(&m) {
                    state.config.mode = m + 1;
                } else {
                    state.config.mode = MAX_MODE;
                }
                LightingAction::WriteGlobal(state.config)
            }
            LightingField::Speed => {
                state.config.speed = state.config.speed.saturating_add(1);
                LightingAction::WriteGlobal(state.config)
            }
            LightingField::Direction => {
                state.config.direction = (state.config.direction + 1) % 4;
                LightingAction::WriteGlobal(state.config)
            }
            LightingField::GlobalColor => {
                state.global_hex_input.clear();
                state.global_preset_idx = (state.global_preset_idx + 1) % COLOR_PRESETS.len();
                let rgb = COLOR_PRESETS[state.global_preset_idx].1;
                let [h, s, v] = light::rgb_to_hsv(rgb);
                state.config.hue = h;
                state.config.saturation = s;
                state.config.value = v;
                state.config.cycle = 1;
                LightingAction::WriteGlobal(state.config)
            }
            LightingField::TargetKey => {
                if state.target_key < max_key {
                    state.target_key += 1;
                }
                LightingAction::None
            }
            LightingField::KeyColor => {
                state.key_hex_input.clear();
                state.key_preset_idx = (state.key_preset_idx + 1) % COLOR_PRESETS.len();
                let rgb = COLOR_PRESETS[state.key_preset_idx].1;
                LightingAction::WriteKeyColor {
                    key: state.target_key,
                    rgb,
                }
            }
        },
        KeyCode::Enter => match state.field {
            LightingField::GlobalColor => {
                if !state.global_hex_input.is_empty() {
                    match parse_rgb_hex(&state.global_hex_input) {
                        Some(rgb) => {
                            let [h, s, v] = light::rgb_to_hsv(rgb);
                            state.config.hue = h;
                            state.config.saturation = s;
                            state.config.value = v;
                            state.config.cycle = 1;
                            LightingAction::WriteGlobal(state.config)
                        }
                        None => LightingAction::Refused(format!(
                            "'{}' is not a valid 6-hex-digit colour (e.g. #ff00ff)",
                            state.global_hex_input
                        )),
                    }
                } else {
                    let rgb = COLOR_PRESETS[state.global_preset_idx % COLOR_PRESETS.len()].1;
                    let [h, s, v] = light::rgb_to_hsv(rgb);
                    state.config.hue = h;
                    state.config.saturation = s;
                    state.config.value = v;
                    state.config.cycle = 1;
                    LightingAction::WriteGlobal(state.config)
                }
            }
            LightingField::KeyColor => {
                if !state.key_hex_input.is_empty() {
                    match parse_rgb_hex(&state.key_hex_input) {
                        Some(rgb) => LightingAction::WriteKeyColor {
                            key: state.target_key,
                            rgb,
                        },
                        None => LightingAction::Refused(format!(
                            "'{}' is not a valid 6-hex-digit colour (e.g. #ff00ff)",
                            state.key_hex_input
                        )),
                    }
                } else {
                    let rgb = COLOR_PRESETS[state.key_preset_idx % COLOR_PRESETS.len()].1;
                    LightingAction::WriteKeyColor {
                        key: state.target_key,
                        rgb,
                    }
                }
            }
            _ => {
                if !state.config.has_valid_mode() {
                    state.config.mode = MIN_MODE;
                }
                LightingAction::WriteGlobal(state.config)
            }
        },
        _ => LightingAction::None,
    }
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
fn build_rows(table: &KeyTable, keys: u8) -> Vec<Row> {
    (0..KEY_SLOTS)
        .map(|index| Row {
            index,
            control: control_name(index, keys).unwrap_or_default().to_string(),
            action: table.get(index).unwrap_or(KeyType::Unassigned),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_cover_every_slot_in_order() {
        let rows = build_rows(&KeyTable::new(), 6);
        assert_eq!(rows.len(), KEY_SLOTS as usize);
        assert_eq!(rows[0].index, 0);
        assert_eq!(rows[KEY_SLOTS as usize - 1].index, KEY_SLOTS - 1);
    }

    #[test]
    fn labels_follow_the_model_rather_than_the_slot_index() {
        // The two boards share knob slots and differ only in key count, so the
        // same index means different things on each.
        let six = build_rows(&KeyTable::new(), 6);
        assert_eq!(six[0].label(), "key 1");
        assert_eq!(six[3].label(), "key 4");
        assert_eq!(six[6].label(), "slot 6", "past the last key");
        assert_eq!(six[16].label(), "knob 1 press");

        let three = build_rows(&KeyTable::new(), 3);
        assert_eq!(three[2].label(), "key 3");
        assert_eq!(
            three[3].label(),
            "slot 3",
            "a 3-key board must not claim a key 4"
        );
        assert_eq!(three[16].label(), "knob 1 press", "knobs are shared");
    }

    #[test]
    fn unassigned_slots_render_as_a_dash_not_as_an_action() {
        let rows = build_rows(&KeyTable::new(), 6);
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

    #[test]
    fn typing_and_tab_completion_replaces_field_wholesale() {
        let mut input = String::from("key:ctr");
        let mut selected = 0;
        let outcome = handle_edit_key(&mut input, &mut selected, KeyCode::Tab);
        assert_eq!(outcome, EditOutcome::Continue);
        assert_eq!(input, "key:ctrl-");

        // Type 'sp'
        let _ = handle_edit_key(&mut input, &mut selected, KeyCode::Char('s'));
        let _ = handle_edit_key(&mut input, &mut selected, KeyCode::Char('p'));
        assert_eq!(input, "key:ctrl-sp");

        // Tab completes to space
        let _ = handle_edit_key(&mut input, &mut selected, KeyCode::Tab);
        assert_eq!(input, "key:ctrl-space");

        // Enter submits
        let outcome = handle_edit_key(&mut input, &mut selected, KeyCode::Enter);
        assert_eq!(outcome, EditOutcome::Submit);
    }

    #[test]
    fn arrow_keys_navigate_suggestions_and_esc_cancels() {
        let mut input = String::from("key:ctrl-");
        let mut selected = 0;

        let _ = handle_edit_key(&mut input, &mut selected, KeyCode::Down);
        assert_eq!(selected, 1);

        let _ = handle_edit_key(&mut input, &mut selected, KeyCode::Up);
        assert_eq!(selected, 0);

        let outcome = handle_edit_key(&mut input, &mut selected, KeyCode::Esc);
        assert_eq!(outcome, EditOutcome::Cancel);
    }

    #[test]
    fn backspace_resets_selected_suggestion() {
        let mut input = String::from("key:ctrl-");
        let mut selected = 5;

        let _ = handle_edit_key(&mut input, &mut selected, KeyCode::Backspace);
        assert_eq!(input, "key:ctrl");
        assert_eq!(selected, 0);
    }

    #[test]
    fn lighting_mode_clamping_and_refusal() {
        let mut state = LightingState::new(LightConfig::OFF, 6);
        state.field = LightingField::Mode;

        // Digit 1-6 is accepted and writes config
        match handle_lighting_key(&mut state, 6, KeyCode::Char('3')) {
            LightingAction::WriteGlobal(cfg) => assert_eq!(cfg.mode, 3),
            other => panic!("expected WriteGlobal, got {:?}", other),
        }

        // Mode 0 must be refused with explanation (firmware discards 0)
        match handle_lighting_key(&mut state, 6, KeyCode::Char('0')) {
            LightingAction::Refused(msg) => {
                assert!(msg.contains("mode must be 1..=6"), "got: {msg}");
                assert!(msg.contains("silently discarded"), "got: {msg}");
            }
            other => panic!("expected Refused, got {:?}", other),
        }

        // Mode 7+ must be refused with explanation (firmware discards 7+)
        match handle_lighting_key(&mut state, 6, KeyCode::Char('7')) {
            LightingAction::Refused(msg) => {
                assert!(msg.contains("mode must be 1..=6"), "got: {msg}");
            }
            other => panic!("expected Refused, got {:?}", other),
        }
    }

    #[test]
    fn lighting_brightness_bounds_and_refusal() {
        let mut state = LightingState::new(LightConfig::OFF, 6);
        state.field = LightingField::Brightness;

        // Digits 0-4 are valid
        match handle_lighting_key(&mut state, 6, KeyCode::Char('4')) {
            LightingAction::WriteGlobal(cfg) => assert_eq!(cfg.brightness, 4),
            other => panic!("expected WriteGlobal, got {:?}", other),
        }

        // Digit 5+ is refused
        match handle_lighting_key(&mut state, 6, KeyCode::Char('5')) {
            LightingAction::Refused(msg) => {
                assert!(msg.contains("brightness must be 0..=4"), "got: {msg}");
            }
            other => panic!("expected Refused, got {:?}", other),
        }

        // Right arrow clamps at 4
        match handle_lighting_key(&mut state, 6, KeyCode::Right) {
            LightingAction::WriteGlobal(cfg) => assert_eq!(cfg.brightness, 4),
            other => panic!("expected WriteGlobal, got {:?}", other),
        }

        // Left arrow decrements
        match handle_lighting_key(&mut state, 6, KeyCode::Left) {
            LightingAction::WriteGlobal(cfg) => assert_eq!(cfg.brightness, 3),
            other => panic!("expected WriteGlobal, got {:?}", other),
        }
    }

    #[test]
    fn lighting_per_key_color_write() {
        let mut state = LightingState::new(LightConfig::OFF, 6);
        state.field = LightingField::TargetKey;

        // Set target key to 2
        let _ = handle_lighting_key(&mut state, 6, KeyCode::Char('2'));
        assert_eq!(state.target_key, 2);

        // Move to KeyColor
        state.field = LightingField::KeyColor;

        // Change color with right arrow: selects next preset and writes key color
        match handle_lighting_key(&mut state, 6, KeyCode::Right) {
            LightingAction::WriteKeyColor { key, rgb } => {
                assert_eq!(key, 2);
                assert_eq!(rgb, COLOR_PRESETS[1].1);
            }
            other => panic!("expected WriteKeyColor, got {:?}", other),
        }

        // Custom hex input and Enter
        state.key_hex_input = "ff00aa".to_string();
        match handle_lighting_key(&mut state, 6, KeyCode::Enter) {
            LightingAction::WriteKeyColor { key, rgb } => {
                assert_eq!(key, 2);
                assert_eq!(rgb, [0xFF, 0x00, 0xAA]);
            }
            other => panic!("expected WriteKeyColor, got {:?}", other),
        }
    }

    #[test]
    fn per_key_fields_are_only_reachable_in_the_mode_that_shows_them() {
        // Modes 1-4 paint every key with the global colour, so a per-key write
        // is invisible there - it reads as a failed write. The controls must not
        // be offered, or navigable, in those modes.
        for mode in 1..=4 {
            let fields = LightingField::for_mode(mode);
            assert!(
                !fields.contains(&LightingField::TargetKey),
                "mode {mode} does not show per-key colour"
            );
            assert!(!fields.contains(&LightingField::KeyColor), "mode {mode}");
        }
        let custom = LightingField::for_mode(PER_KEY_MODE);
        assert!(custom.contains(&LightingField::TargetKey));
        assert!(custom.contains(&LightingField::KeyColor));
        // Every mode still gets the global controls.
        for mode in 1..=6 {
            for f in LightingField::GLOBAL {
                assert!(LightingField::for_mode(mode).contains(&f), "mode {mode}");
            }
        }
    }

    #[test]
    fn navigation_never_lands_on_a_hidden_per_key_field() {
        let mut state = LightingState::new(LightConfig::OFF, 6);
        state.config.mode = 2;
        state.field = LightingField::Brightness;
        for _ in 0..12 {
            handle_lighting_key(&mut state, 6, KeyCode::Down);
            assert!(!LightingField::PER_KEY.contains(&state.field));
        }
        // In mode 5 the same walk reaches them.
        let mut custom = LightingState::new(LightConfig::OFF, 6);
        custom.config.mode = PER_KEY_MODE;
        custom.field = LightingField::GlobalColor;
        let mut seen = false;
        for _ in 0..6 {
            handle_lighting_key(&mut custom, 6, KeyCode::Down);
            if LightingField::PER_KEY.contains(&custom.field) {
                seen = true;
            }
        }
        assert!(seen, "mode 5 should make the per-key fields reachable");
    }

    #[test]
    fn a_refresh_that_changes_mode_clamps_the_selection() {
        // The mode can change without the user visiting the mode field, since
        // `r` re-reads the pad. The cursor must not be left on a hidden field.
        let mut state = LightingState::new(LightConfig::OFF, 6);
        state.config.mode = PER_KEY_MODE;
        state.field = LightingField::KeyColor;
        assert!(LightingField::for_mode(state.config.mode).contains(&state.field));

        state.config.mode = 2; // as if the pad were changed elsewhere
        state.clamp_field_to_mode();
        assert!(!LightingField::PER_KEY.contains(&state.field));
        assert!(LightingField::for_mode(state.config.mode).contains(&state.field));
    }

    #[test]
    fn changing_mode_from_the_mode_field_keeps_the_selection_visible() {
        let mut state = LightingState::new(LightConfig::OFF, 6);
        state.config.mode = 1;
        state.field = LightingField::Mode;
        handle_lighting_key(&mut state, 6, KeyCode::Char('5'));
        assert_eq!(state.config.mode, PER_KEY_MODE);
        assert_eq!(state.field, LightingField::Mode);
    }

    #[test]
    fn lighting_navigation_and_exit() {
        let mut state = LightingState::new(LightConfig::OFF, 6);
        assert_eq!(state.field, LightingField::Brightness);

        let _ = handle_lighting_key(&mut state, 6, KeyCode::Down);
        assert_eq!(state.field, LightingField::Mode);

        let _ = handle_lighting_key(&mut state, 6, KeyCode::Up);
        assert_eq!(state.field, LightingField::Brightness);

        assert_eq!(
            handle_lighting_key(&mut state, 6, KeyCode::Esc),
            LightingAction::Exit
        );
        assert_eq!(
            handle_lighting_key(&mut state, 6, KeyCode::Char('q')),
            LightingAction::Quit
        );
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
