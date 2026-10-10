//! Lighting: the pad's light configuration and per-key colours.
//!
//! Field layout taken from the vendor's own JavaScript
//! (`sdcx-tech.com`, `_next/static/chunks/app/page-*.js`), which is the
//! authoritative source for this protocol:
//!
//! ```js
//! getLightConfig()      -> sendDeviceData(6, [10]) then .slice(5, 16)
//! setLightConfig(e)     -> sendDeviceData(6, [11, e.length, 0, 0, ...fields])
//! setLightEffectConfig  -> sendDeviceData(6, [22, 0, 0, 0, 1, 0, mode])  (a query)
//! setKeyColorInfo(e)    -> l[1]=3, l[2..3]=3*key_index, l[7..9]=R,G,B
//! ```
//!
//! Two things are easy to get wrong. The light config lives at **report offset
//! 5**, not 8 where payloads normally start, so reading it from offset 8 yields
//! all zeros and looks like "no lighting". And brightness is **byte 3 of the
//! config**, which the device reports as 0 until something raises it.

use crate::protocol::{ProtocolError, REPORT_LEN};

/// Bytes in a light configuration record.
pub const LIGHT_CONFIG_LEN: usize = 11;

/// Lowest effect mode the firmware accepts.
///
/// Modes outside [`MIN_MODE`]..=[`MAX_MODE`] are **silently discarded**: the
/// write is acknowledged and echoed back, but a following read reports an
/// all-zero record. Confirmed on hardware by sweeping 0-8, where only 1-6
/// stuck. Mode 0 in particular is not "solid colour" — it means the record is
/// rejected, so lighting cannot be turned on without also choosing a mode.
pub const MIN_MODE: u8 = 1;

/// Highest effect mode the firmware accepts.
pub const MAX_MODE: u8 = 6;

/// Highest effect speed the firmware documents.
///
/// The vendor's own light panel declares `min: 0, max: 4` on both the speed and
/// brightness sliders. Speeds above 4 are stored verbatim but behave erratically:
/// speed 5 fades over ~7s on mode 3, slower than any supported value.
pub const MAX_SPEED: u8 = 4;

/// Report byte the light configuration starts at, in both directions.
///
/// Matches the device-config (`05`) reply, which also answers at 5 rather
/// than 8.
pub const LIGHT_CONFIG_OFFSET: usize = 5;

/// The pad's lighting state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LightConfig {
    /// Lighting type. The vendor always sends 1.
    pub light_type: u8,
    /// Effect mode.
    pub mode: u8,
    /// Brightness, 0-4 in the vendor UI. **0 means off.**
    pub brightness: u8,
    /// Effect speed.
    pub speed: u8,
    /// Effect direction.
    pub direction: u8,
    /// 0 for a solid colour, 1 to cycle.
    pub cycle: u8,
    /// Single-colour index; the vendor always writes 0.
    pub single_color_index: u8,
    /// Hue, 0-255 (the vendor stores 0-360 divided down by 360/255).
    pub hue: u8,
    /// Saturation, 0-255.
    pub saturation: u8,
    /// Value, 0-255. Equals the brightest RGB channel, so a fully black
    /// colour has `value == 0` and lights nothing however bright the setting.
    pub value: u8,
}

impl LightConfig {
    /// What the device reports on a fresh, unlit pad: lights off.
    pub const OFF: LightConfig = LightConfig {
        light_type: 1,
        mode: 0,
        brightness: 0,
        speed: 0,
        direction: 0,
        cycle: 0,
        single_color_index: 0,
        hue: 0,
        saturation: 0,
        value: 0,
    };

    /// Decode from the 11 bytes starting at [`LIGHT_CONFIG_OFFSET`].
    pub fn parse(report: &[u8]) -> Result<LightConfig, ProtocolError> {
        let end = LIGHT_CONFIG_OFFSET + LIGHT_CONFIG_LEN;
        if report.len() < end {
            return Err(ProtocolError::ResponseTooShort { got: report.len() });
        }
        let f = &report[LIGHT_CONFIG_OFFSET..end];
        Ok(LightConfig {
            light_type: f[0],
            mode: f[2],
            brightness: f[3],
            speed: f[4],
            direction: f[5],
            cycle: f[6],
            single_color_index: f[7],
            hue: f[8],
            saturation: f[9],
            value: f[10],
        })
    }

    /// The 11-byte record, in the order the device stores it.
    pub fn record(self) -> [u8; LIGHT_CONFIG_LEN] {
        [
            self.light_type,
            0, // f[1] is never sent by the vendor
            self.mode,
            self.brightness,
            self.speed,
            self.direction,
            self.cycle,
            self.single_color_index,
            self.hue,
            self.saturation,
            self.value,
        ]
    }

    /// What the *record* implies about whether the pad will show colour.
    ///
    /// **This is a prediction about these bytes, not an observation of the
    /// device.** It cannot be one: the protocol has no lighting read-back beyond
    /// the stored record itself.
    ///
    /// Measured on the 6-key model, `brightness` has **no observable effect** in
    /// any mode - 0 and 4 look identical. Darkness comes from `value == 0`, a
    /// black colour, which is also why `led off` works even though it sets
    /// brightness as well.
    ///
    /// So this returns false only for a genuinely black colour. Brightness is
    /// deliberately *not* consulted: doing so reported "dark" for a saturated
    /// green at brightness 0, which was visibly lit.
    pub fn colour_is_black(self) -> bool {
        self.value == 0
    }

    /// True if the brightness is within the 0-4 range the vendor UI uses.
    pub fn has_valid_brightness(self) -> bool {
        self.brightness <= 4
    }

    /// True if this mode is one the firmware will actually store.
    pub fn has_valid_mode(self) -> bool {
        (MIN_MODE..=MAX_MODE).contains(&self.mode)
    }

    /// The mode to use when lighting should be on but none was chosen.
    pub fn default_mode() -> u8 {
        MIN_MODE
    }

    /// Fields the firmware is expected to store verbatim.
    ///
    /// `light_type` and `single_color_index` are excluded: the device
    /// normalises them (it reports its own type regardless of what was sent),
    /// so requiring an exact match would reject writes that genuinely landed.
    pub fn applied(self, stored: LightConfig) -> bool {
        self.mode == stored.mode
            && self.brightness == stored.brightness
            && self.speed == stored.speed
            && self.direction == stored.direction
            && self.cycle == stored.cycle
            && self.hue == stored.hue
            && self.saturation == stored.saturation
            && self.value == stored.value
    }

    /// Names the fields in which two configurations disagree.
    pub fn differences(self, stored: LightConfig) -> Vec<&'static str> {
        let mut out = Vec::new();
        let mut check = |name: &'static str, a: u8, b: u8| {
            if a != b {
                out.push(name);
            }
        };
        check("mode", self.mode, stored.mode);
        check("brightness", self.brightness, stored.brightness);
        check("speed", self.speed, stored.speed);
        check("direction", self.direction, stored.direction);
        check("cycle", self.cycle, stored.cycle);
        check("hue", self.hue, stored.hue);
        check("saturation", self.saturation, stored.saturation);
        check("value", self.value, stored.value);
        out
    }
}

/// Build the report that reads the light configuration (command `0x0A`).
pub fn read_light_config() -> [u8; REPORT_LEN] {
    let mut buf = [0u8; REPORT_LEN];
    buf[0] = crate::protocol::PREFIX;
    buf[1] = 0x0A;
    buf[2] = 3;
    buf
}

/// Build the report that writes the light configuration (command `0x0B`).
///
/// The record starts at [`LIGHT_CONFIG_OFFSET`] rather than the usual payload
/// offset, so this cannot be expressed with the generic request builder.
///
/// Rejects a mode the firmware would discard, rather than sending a write that
/// is acknowledged and then silently dropped.
pub fn write_light_config(config: LightConfig) -> Result<[u8; REPORT_LEN], ProtocolError> {
    if !config.has_valid_mode() {
        return Err(ProtocolError::InvalidLightMode {
            mode: config.mode,
            min: MIN_MODE,
            max: MAX_MODE,
        });
    }
    let mut buf = [0u8; REPORT_LEN];
    buf[0] = crate::protocol::PREFIX;
    buf[1] = 0x0B;
    buf[2] = LIGHT_CONFIG_LEN as u8;
    buf[LIGHT_CONFIG_OFFSET..LIGHT_CONFIG_OFFSET + LIGHT_CONFIG_LEN]
        .copy_from_slice(&config.record());
    Ok(buf)
}

/// Build the report that sets one key's colour (command `0x14`).
///
/// The offset is `3 * index`, not the index, and the length byte is 3. The
/// vendor's `setKeyColorInfo` uses that scaling because colours are stored one
/// RGB triple per key; a plain index offset writes to the wrong key.
pub fn set_key_color(index: u8, rgb: [u8; 3]) -> Result<[u8; REPORT_LEN], ProtocolError> {
    if index >= crate::keymap::KEY_SLOTS {
        return Err(ProtocolError::KeyIndexOutOfRange {
            index,
            max: crate::keymap::KEY_SLOTS - 1,
        });
    }
    let offset = u16::from(index) * 3;
    let mut buf = [0u8; REPORT_LEN];
    buf[0] = crate::protocol::PREFIX;
    buf[1] = 0x14;
    buf[2] = 3;
    buf[3] = offset as u8;
    buf[4] = (offset >> 8) as u8;
    buf[8] = rgb[0];
    buf[9] = rgb[1];
    buf[10] = rgb[2];
    Ok(buf)
}

/// Build the report that reads every key colour (command `0x12`).
///
/// **Do not send this.** It is not a read, and there is no correct framing for
/// it: the vendor app contains no per-key colour read at all — its
/// `getKeyRGBBytes` is the generic read primitive, never called for colours.
///
/// Measured on the 6-key model: a `14` write to offset 0 followed by this
/// command leaves the key dark, while the same write with no `12` afterwards
/// shows the colour immediately. Since the framing carries no offset, the
/// firmware applies it at offset 0, where it lands on the colour-write path and
/// zeroes key 1.
///
/// So it silently erases key 1's colour, which is why per-key colour writes to
/// index 0 appeared to fail. The function is kept only so the probe that
/// established this can be reproduced; nothing in the CLI or TUI may call it.
pub fn read_key_colors() -> [u8; REPORT_LEN] {
    let mut buf = [0u8; REPORT_LEN];
    buf[0] = crate::protocol::PREFIX;
    buf[1] = 0x12;
    buf[2] = 3;
    buf
}

/// Decode the reply to [`read_key_colors`] into one RGB triple per key.
///
/// Keys beyond `keys` are dropped: the device fills `KEY_SLOTS` triples
/// regardless of how many keys are fitted, so reading a 6-key pad with the
/// 3-key geometry would report knobs as lit keys.
pub fn parse_key_colors(reply: &[u8], keys: u8) -> Vec<(u8, [u8; 3])> {
    let count = keys.min(crate::keymap::KEY_SLOTS);
    let end = 8 + usize::from(count) * 3;
    if reply.len() < end {
        return Vec::new();
    }
    (0..count)
        .map(|i| {
            let at = 8 + usize::from(i) * 3;
            (i, [reply[at], reply[at + 1], reply[at + 2]])
        })
        .collect()
}

/// Convert `#RRGGBB` into the packed HSV bytes the device stores.
///
/// Mirrors the vendor's `HW()` helper: `#RRGGBB` is converted to HSV with hue
/// in degrees and s/v as percentages, then each is scaled into a byte.
pub fn hex_to_hsv(hex: &str) -> Result<[u8; 3], ProtocolError> {
    let s = hex.trim().trim_start_matches('#');
    if s.len() != 6 || !s.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(ProtocolError::BadColour(hex.to_string()));
    }
    let r = u8::from_str_radix(&s[0..2], 16).unwrap();
    let g = u8::from_str_radix(&s[2..4], 16).unwrap();
    let b = u8::from_str_radix(&s[4..6], 16).unwrap();
    Ok(rgb_to_hsv([r, g, b]))
}

/// Convert RGB into the packed HSV bytes the device stores.
pub fn rgb_to_hsv([r, g, b]: [u8; 3]) -> [u8; 3] {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let delta = max - min;

    // Hue in degrees, 0 when the colour is grey.
    let hue_deg = if delta == 0 {
        0.0
    } else if max == r {
        60.0 * (g as f32 - b as f32) / delta as f32
    } else if max == g {
        60.0 * (2.0 + (b as f32 - r as f32) / delta as f32)
    } else {
        60.0 * (4.0 + (r as f32 - g as f32) / delta as f32)
    };
    let hue_deg = if hue_deg < 0.0 {
        hue_deg + 360.0
    } else {
        hue_deg
    };

    let sat = if max == 0 {
        0.0
    } else {
        delta as f32 / max as f32 * 100.0
    };

    // The vendor scales 0-360 / 0-100 / 0-100 down into bytes.
    [
        (hue_deg / 360.0 * 255.0).floor() as u8,
        (sat / 100.0 * 255.0).floor() as u8,
        (max as f32 / 255.0 * 100.0 / 100.0 * 255.0).floor() as u8,
    ]
}

#[cfg(test)]
mod tests {

    #[test]
    fn a_key_colour_reply_is_decoded_one_triple_per_key() {
        let mut reply = [0u8; 64];
        // Red on key 0, green on key 1, rest black.
        reply[8] = 0xFF; // key 0, byte 0
        reply[12] = 0xFF; // key 1 spans 11,12,13 = R,G,B; this is green
        let got = parse_key_colors(&reply, 3);
        assert_eq!(got.len(), 3, "only the fitted keys, not all 28 slots");
        assert_eq!(got[0], (0, [0xFF, 0, 0]));
        assert_eq!(got[1], (1, [0, 0xFF, 0]));
        assert_eq!(got[2], (2, [0, 0, 0]));
    }

    #[test]
    fn a_short_reply_decodes_to_nothing_rather_than_panicking() {
        assert!(parse_key_colors(&[0u8; 4], 6).is_empty());
        assert!(parse_key_colors(&[0u8; 16], 6).is_empty());
    }

    #[test]
    fn a_six_key_pad_is_not_read_as_if_it_were_three() {
        // The device fills KEY_SLOTS triples whatever is fitted, so using the
        // wrong count would report knob slots as lit keys.
        let mut reply = [0u8; 64];
        reply[8 + 15] = 0xFF;
        let got = parse_key_colors(&reply, 6);
        assert_eq!(got.len(), 6);
        assert!(
            !got.iter().any(|(i, _)| *i == 16),
            "keys beyond the fitted count"
        );
        let three = parse_key_colors(&reply, 3);
        assert_eq!(three.len(), 3);
    }

    #[test]
    fn the_read_request_matches_the_write_framing() {
        // Kept as a characterisation test, not an endorsement: this request is
        // destructive. It is pinned here so that anyone changing the framing
        // has to look at the warning, and so the probe can be reproduced.
        let r = read_key_colors();
        assert_eq!(r[0], crate::protocol::PREFIX);
        assert_eq!(r[1], 0x12);
        assert_eq!(r[2], 3, "three bytes per colour, as `14` writes");
        // No offset field, which is precisely why the firmware applies it at
        // offset 0 and wipes key 1's colour. If a real read framing is ever
        // found, this assertion should fail.
        assert_eq!(r[3], 0, "no offset is sent - see the warning above");
        assert_eq!(r[4], 0);
    }

    #[test]
    fn every_key_colour_is_an_offset_triple_not_an_index_offset() {
        // `14` writes at 3 * index. If `12` were read at a plain index offset the
        // two would disagree, which is the bug this guards.
        for index in [0u8, 1, 5, 13] {
            let write = set_key_color(index, [1, 2, 3]).unwrap();
            assert_eq!(write[3], index * 3, "write offset is 3 * index");
        }
    }
    use super::*;

    /// Real reply captured from the pad for command `0x0A` on 2026-10-09:
    /// `aa 0a 0b 00 00 01 00 00 00 00 00 00 00 00 00 00 ...`
    ///
    /// Read from offset 5 this is type 1 with brightness 0 — lights off.
    /// Read from offset 8, as an earlier probe did, every field is zero.
    const REAL_LIGHT_REPLY: [u8; REPORT_LEN] = {
        let mut r = [0u8; REPORT_LEN];
        r[0] = 0xAA;
        r[1] = 0x0A;
        r[2] = 0x0B;
        r[5] = 0x01; // light type
        r
    };

    #[test]
    fn light_config_is_read_from_offset_five_not_eight() {
        let config = LightConfig::parse(&REAL_LIGHT_REPLY).unwrap();
        assert_eq!(config.light_type, 1, "type lives at offset 5");
        assert_eq!(config.brightness, 0, "this pad is unlit");
        assert!(config.colour_is_black(), "an unlit pad has a black colour");
        // The tempting wrong offset yields a featureless config.
        let mut wrong = REAL_LIGHT_REPLY;
        wrong[5] = 0;
        assert_eq!(LightConfig::parse(&wrong).unwrap().light_type, 0);
    }

    #[test]
    fn the_write_report_matches_the_vendor_layout() {
        let config = LightConfig {
            mode: 1,
            brightness: 3,
            value: 255,
            hue: 0,
            saturation: 255,
            ..LightConfig::OFF
        };
        let report = write_light_config(config).unwrap();
        assert_eq!(report[0], 0x06);
        assert_eq!(report[1], 0x0B, "command");
        assert_eq!(report[2], 11, "length is the 11-byte record");
        // Fields start at offset 5, not 8.
        assert_eq!(report[5], 1, "type at byte 5");
        assert_eq!(report[7], 1, "mode at byte 7");
        assert_eq!(report[8], 3, "brightness at byte 8");
        assert_eq!(report[13], 0, "hue at byte 13");
        assert_eq!(report[15], 255, "value at byte 15");
    }

    #[test]
    fn only_modes_one_to_six_are_accepted() {
        // Verified on hardware: sweeping modes 0-8, only 1-6 stuck. Mode 0 is
        // not "solid colour", it makes the firmware drop the whole record.
        for mode in 0..=8u8 {
            let config = LightConfig {
                mode,
                brightness: 3,
                value: 255,
                ..LightConfig::OFF
            };
            let expected_ok = (1..=6).contains(&mode);
            assert_eq!(config.has_valid_mode(), expected_ok, "mode {mode}");
            let result = write_light_config(config);
            assert_eq!(result.is_ok(), expected_ok, "write for mode {mode}");
        }
    }

    #[test]
    fn an_unusable_mode_explains_why_it_would_be_dropped() {
        let config = LightConfig {
            mode: 0,
            brightness: 3,
            value: 255,
            ..LightConfig::OFF
        };
        let err = write_light_config(config).unwrap_err().to_string();
        assert!(err.contains('0'), "names the mode: {err}");
        assert!(err.contains("silently"), "warns it would be dropped: {err}");
    }

    #[test]
    fn config_records_round_trip() {
        let config = LightConfig {
            light_type: 1,
            mode: 2,
            brightness: 4,
            speed: 30,
            direction: 1,
            cycle: 1,
            single_color_index: 0,
            hue: 200,
            saturation: 100,
            value: 180,
        };
        let mut report = [0u8; REPORT_LEN];
        report[0] = 0xAA;
        report[LIGHT_CONFIG_OFFSET..LIGHT_CONFIG_OFFSET + LIGHT_CONFIG_LEN]
            .copy_from_slice(&config.record());
        assert_eq!(LightConfig::parse(&report).unwrap(), config);
    }

    #[test]
    fn only_a_black_colour_counts_as_unlit() {
        // Guards the trap that made an earlier probe light nothing: a black
        // colour lights nothing at any brightness setting.
        let black = LightConfig {
            brightness: 4,
            hue: 0,
            saturation: 255,
            value: 0,
            ..LightConfig::OFF
        };
        assert!(black.colour_is_black(), "value 0 means nothing to light");
        let red = LightConfig {
            brightness: 4,
            value: 255,
            ..LightConfig::OFF
        };
        assert!(!red.colour_is_black());

        // brightness 0 with a real colour is NOT dark. Measured on the 6-key
        // model: brightness has no observable effect at all, and the tool used
        // to report this as dark while the pad was plainly lit.
        let dim_brightness = LightConfig {
            brightness: 0,
            value: 255,
            ..LightConfig::OFF
        };
        assert!(
            !dim_brightness.colour_is_black(),
            "brightness 0 must not be read as darkness"
        );
    }

    #[test]
    fn key_colour_offsets_are_three_times_the_index() {
        // The vendor scales by 3 because colours are stored one RGB triple per
        // key; using the bare index writes to the wrong slot.
        let report = set_key_color(0, [0xFF, 0, 0]).unwrap();
        assert_eq!(&report[..5], &[0x06, 0x14, 0x03, 0x00, 0x00]);
        assert_eq!(&report[8..11], &[0xFF, 0x00, 0x00]);

        let report = set_key_color(5, [0, 0xFF, 0]).unwrap();
        assert_eq!(report[3], 15, "key 5 is at offset 15, not 5");
        assert_eq!(&report[8..11], &[0x00, 0xFF, 0x00]);
    }

    #[test]
    fn an_out_of_range_key_colour_is_rejected() {
        assert_eq!(
            set_key_color(crate::keymap::KEY_SLOTS, [1, 2, 3]),
            Err(ProtocolError::KeyIndexOutOfRange {
                index: crate::keymap::KEY_SLOTS,
                max: crate::keymap::KEY_SLOTS - 1
            })
        );
    }

    #[test]
    fn pure_red_maps_to_hue_zero_and_full_saturation() {
        let [h, s, v] = hex_to_hsv("#ff0000").unwrap();
        assert_eq!(h, 0, "red is hue 0");
        assert_eq!(s, 255, "fully saturated");
        assert_eq!(v, 255, "full value");
    }

    #[test]
    fn primary_colours_land_on_the_expected_hues() {
        assert_eq!(hex_to_hsv("#00ff00").unwrap()[0], 85, "green is hue 120deg");
        assert_eq!(hex_to_hsv("#0000ff").unwrap()[0], 170, "blue is hue 240deg");
    }

    #[test]
    fn a_grey_colour_has_no_hue() {
        let [h, s, v] = hex_to_hsv("#808080").unwrap();
        assert_eq!(h, 0);
        assert_eq!(s, 0, "grey is unsaturated");
        assert_eq!(v, 128);
    }

    #[test]
    fn white_is_full_value_and_no_hue() {
        let [h, s, v] = hex_to_hsv("#ffffff").unwrap();
        assert_eq!((h, s, v), (0, 0, 255));
    }

    #[test]
    fn a_malformed_colour_is_an_error() {
        assert!(hex_to_hsv("#fff").is_err());
        assert!(hex_to_hsv("nothex").is_err());
        assert!(hex_to_hsv("#12345g").is_err());
    }

    #[test]
    fn hex_parsing_accepts_both_spellings() {
        assert_eq!(
            hex_to_hsv("#ff0000").unwrap(),
            hex_to_hsv("ff0000").unwrap()
        );
    }
}
