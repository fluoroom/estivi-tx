//! SSTV mode table: Martin, Scottie, Robot (color and B/W), PD and YoniQ.
//!
//! Timings follow JL Barber N7CXI, "Proposal for SSTV Mode Specifications"
//! (Dayton SSTV forum, 2000) and Dave Jones KB4YZ line timing notes, cross-checked
//! against slowrx and QSSTV mode tables.

use serde::{Deserialize, Serialize};

/// Duration of the standard VIS header: 300 + 10 + 300 ms leader, start bit,
/// 7 data bits, parity and stop bit (30 ms each).
pub const VIS_HEADER_MS: f64 = 300.0 + 10.0 + 300.0 + 30.0 * 10.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Mode {
    #[serde(rename = "M1")]
    MartinM1,
    #[serde(rename = "M2")]
    MartinM2,
    #[serde(rename = "M3")]
    MartinM3,
    #[serde(rename = "M4")]
    MartinM4,
    #[serde(rename = "S1")]
    ScottieS1,
    #[serde(rename = "S2")]
    ScottieS2,
    #[serde(rename = "SDX")]
    ScottieDx,
    #[serde(rename = "R24")]
    Robot24,
    #[serde(rename = "R36")]
    Robot36,
    #[serde(rename = "R72")]
    Robot72,
    /// Robot 12 color (VIS 0). Not the black-and-white `BW12`.
    #[serde(rename = "R12")]
    Robot12,
    #[serde(rename = "BW8")]
    RobotBw8,
    #[serde(rename = "BW12")]
    RobotBw12,
    #[serde(rename = "BW24")]
    RobotBw24,
    #[serde(rename = "PD50")]
    Pd50,
    #[serde(rename = "PD90")]
    Pd90,
    #[serde(rename = "PD120")]
    Pd120,
    #[serde(rename = "PD160")]
    Pd160,
    #[serde(rename = "PD180")]
    Pd180,
    #[serde(rename = "PD240")]
    Pd240,
    #[serde(rename = "PD290")]
    Pd290,
    /// Analog YoniQ (MMSSTV 1.13.2). A picture mode like the rest: no upload, no side file.
    #[serde(rename = "YQ")]
    Yoniq,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Family {
    Martin,
    Scottie,
    Robot,
    RobotBw,
    Pd,
}

/// Per-family line layout. All durations in milliseconds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Timing {
    /// sync 4.862 · porch 0.572 · G · sep · B · sep · R · sep (sep = 0.572)
    Martin { scan_ms: f64 },
    /// sep 1.5 · G · sep · B · sync 9 · porch 1.5 · R (plus one starting sync)
    Scottie { scan_ms: f64 },
    /// sync · porch · Y · [sep · porch2 · chroma] (one chroma per line when
    /// `alternating`, otherwise R-Y then B-Y on every line)
    Robot {
        sync_ms: f64,
        porch_ms: f64,
        y_ms: f64,
        sep_ms: f64,
        sep_porch_ms: f64,
        c_ms: f64,
        alternating: bool,
    },
    /// sync · Y
    RobotBw { sync_ms: f64, y_ms: f64 },
    /// sync 20 · porch 2.08 · Y(even) · R-Y · B-Y · Y(odd), per line pair.
    /// Chroma is the average of the pair.
    Pd { pixel_ms: f64 },
    /// Same porch as PD. Chroma is the even row only (MMSSTV `LinePD`), then Y of the odd row.
    /// `scan_ms` is one component across the whole width.
    Yoniq { scan_ms: f64 },
}

pub const MARTIN_SYNC_MS: f64 = 4.862;
pub const MARTIN_GAP_MS: f64 = 0.572;
pub const SCOTTIE_SYNC_MS: f64 = 9.0;
pub const SCOTTIE_GAP_MS: f64 = 1.5;
pub const PD_SYNC_MS: f64 = 20.0;
pub const PD_PORCH_MS: f64 = 2.08;
/// One YoniQ component across 320 pixels. Four of these plus the PD porch are one 545.6 ms pair.
pub const YONIQ_SCAN_MS: f64 = 130.88;

#[derive(Clone, Copy, Debug)]
pub struct ModeSpec {
    pub mode: Mode,
    pub name: &'static str,
    pub short: &'static str,
    pub family: Family,
    pub vis: u8,
    /// YoniQ sends odd parity. Every other mode sends even parity.
    pub vis_odd: bool,
    /// Size of the picture the mode carries (the canvas the graphics are composed on).
    pub width: u32,
    pub height: u32,
    /// Scan lines actually transmitted (M3/M4 send every other row, PD sends line pairs).
    pub tx_lines: u32,
    pub timing: Timing,
}

const fn robot_color(
    sync_ms: f64,
    porch_ms: f64,
    y_ms: f64,
    sep_ms: f64,
    sep_porch_ms: f64,
    c_ms: f64,
    alternating: bool,
) -> Timing {
    Timing::Robot { sync_ms, porch_ms, y_ms, sep_ms, sep_porch_ms, c_ms, alternating }
}

macro_rules! spec {
    ($mode:ident, $name:literal, $short:literal, $fam:ident, $vis:literal, $w:literal x $h:literal, $lines:literal, $timing:expr) => {
        spec!(@ $mode, $name, $short, $fam, $vis, false, $w, $h, $lines, $timing)
    };
    ($mode:ident, $name:literal, $short:literal, $fam:ident, $vis:literal, odd, $w:literal x $h:literal, $lines:literal, $timing:expr) => {
        spec!(@ $mode, $name, $short, $fam, $vis, true, $w, $h, $lines, $timing)
    };
    (@ $mode:ident, $name:literal, $short:literal, $fam:ident, $vis:literal, $odd:expr, $w:expr, $h:expr, $lines:expr, $timing:expr) => {
        ModeSpec {
            mode: Mode::$mode,
            name: $name,
            short: $short,
            family: Family::$fam,
            vis: $vis,
            vis_odd: $odd,
            width: $w,
            height: $h,
            tx_lines: $lines,
            timing: $timing,
        }
    };
}

static SPECS: [ModeSpec; 22] = [
    spec!(MartinM1, "Martin M1", "M1", Martin, 0x2C, 320 x 256, 256, Timing::Martin { scan_ms: 146.432 }),
    spec!(MartinM2, "Martin M2", "M2", Martin, 0x28, 320 x 256, 256, Timing::Martin { scan_ms: 73.216 }),
    spec!(MartinM3, "Martin M3", "M3", Martin, 0x24, 320 x 256, 128, Timing::Martin { scan_ms: 146.432 }),
    spec!(MartinM4, "Martin M4", "M4", Martin, 0x20, 320 x 256, 128, Timing::Martin { scan_ms: 73.216 }),
    spec!(ScottieS1, "Scottie S1", "S1", Scottie, 0x3C, 320 x 256, 256, Timing::Scottie { scan_ms: 138.240 }),
    spec!(ScottieS2, "Scottie S2", "S2", Scottie, 0x38, 320 x 256, 256, Timing::Scottie { scan_ms: 88.064 }),
    spec!(ScottieDx, "Scottie DX", "SDX", Scottie, 0x4C, 320 x 256, 256, Timing::Scottie { scan_ms: 345.600 }),
    // Robot 24 is the Robot 72 line layout scaled to 2/3 (200 ms lines, 160x120).
    spec!(Robot24, "Robot 24", "R24", Robot, 0x04, 160 x 120, 120, robot_color(6.0, 2.0, 92.0, 3.0, 1.0, 46.0, false)),
    spec!(Robot36, "Robot 36", "R36", Robot, 0x08, 320 x 240, 240, robot_color(9.0, 3.0, 88.0, 4.5, 1.5, 44.0, true)),
    spec!(Robot72, "Robot 72", "R72", Robot, 0x0C, 320 x 240, 240, robot_color(9.0, 3.0, 138.0, 4.5, 1.5, 69.0, false)),
    // Robot 12 color: 100 ms line. Chroma is the Robot 36 package (50 ms) scaled by 30/50.
    spec!(Robot12, "Robot 12", "R12", Robot, 0x00, 160 x 120, 120, robot_color(7.0, 3.0, 60.0, 2.7, 0.9, 26.4, true)),
    spec!(RobotBw8, "Robot 8 B/N", "BW8", RobotBw, 0x02, 160 x 120, 120, Timing::RobotBw { sync_ms: 7.0, y_ms: 59.9 }),
    spec!(RobotBw12, "Robot 12 B/N", "BW12", RobotBw, 0x06, 160 x 120, 120, Timing::RobotBw { sync_ms: 7.0, y_ms: 93.0 }),
    spec!(RobotBw24, "Robot 24 B/N", "BW24", RobotBw, 0x0A, 320 x 240, 240, Timing::RobotBw { sync_ms: 7.0, y_ms: 93.0 }),
    spec!(Pd50, "PD 50", "PD50", Pd, 0x5D, 320 x 256, 128, Timing::Pd { pixel_ms: 0.286 }),
    spec!(Pd90, "PD 90", "PD90", Pd, 0x63, 320 x 256, 128, Timing::Pd { pixel_ms: 0.532 }),
    spec!(Pd120, "PD 120", "PD120", Pd, 0x5F, 640 x 496, 248, Timing::Pd { pixel_ms: 0.190 }),
    spec!(Pd160, "PD 160", "PD160", Pd, 0x62, 512 x 400, 200, Timing::Pd { pixel_ms: 0.382 }),
    spec!(Pd180, "PD 180", "PD180", Pd, 0x60, 640 x 496, 248, Timing::Pd { pixel_ms: 0.286 }),
    spec!(Pd240, "PD 240", "PD240", Pd, 0x61, 640 x 496, 248, Timing::Pd { pixel_ms: 0.382 }),
    spec!(Pd290, "PD 290", "PD290", Pd, 0x5E, 800 x 616, 308, Timing::Pd { pixel_ms: 0.286 }),
    spec!(Yoniq, "YoniQ", "YQ", Pd, 0x73, odd, 320 x 256, 128, Timing::Yoniq { scan_ms: YONIQ_SCAN_MS }),
];

impl Mode {
    pub const ALL: [Mode; 22] = [
        Mode::MartinM1,
        Mode::MartinM2,
        Mode::MartinM3,
        Mode::MartinM4,
        Mode::ScottieS1,
        Mode::ScottieS2,
        Mode::ScottieDx,
        Mode::Robot24,
        Mode::Robot36,
        Mode::Robot72,
        Mode::Robot12,
        Mode::RobotBw8,
        Mode::RobotBw12,
        Mode::RobotBw24,
        Mode::Pd50,
        Mode::Pd90,
        Mode::Pd120,
        Mode::Pd160,
        Mode::Pd180,
        Mode::Pd240,
        Mode::Pd290,
        Mode::Yoniq,
    ];

    pub fn spec(self) -> &'static ModeSpec {
        // SPECS is declared in the same order as the enum.
        &SPECS[self as usize]
    }

    pub fn name(self) -> &'static str {
        self.spec().name
    }

    pub fn from_short(short: &str) -> Option<Mode> {
        SPECS.iter().find(|s| s.short.eq_ignore_ascii_case(short)).map(|s| s.mode)
    }
}

impl ModeSpec {
    /// Time from the start of one transmitted line to the next.
    pub fn line_ms(&self) -> f64 {
        match self.timing {
            Timing::Martin { scan_ms } => MARTIN_SYNC_MS + 4.0 * MARTIN_GAP_MS + 3.0 * scan_ms,
            Timing::Scottie { scan_ms } => SCOTTIE_SYNC_MS + 3.0 * SCOTTIE_GAP_MS + 3.0 * scan_ms,
            Timing::Robot { sync_ms, porch_ms, y_ms, sep_ms, sep_porch_ms, c_ms, alternating } => {
                let chroma = sep_ms + sep_porch_ms + c_ms;
                sync_ms + porch_ms + y_ms + if alternating { chroma } else { 2.0 * chroma }
            }
            Timing::RobotBw { sync_ms, y_ms } => sync_ms + y_ms,
            Timing::Pd { pixel_ms } => PD_SYNC_MS + PD_PORCH_MS + 4.0 * pixel_ms * self.width as f64,
            Timing::Yoniq { scan_ms } => PD_SYNC_MS + PD_PORCH_MS + 4.0 * scan_ms,
        }
    }

    /// Duration of the picture itself (after the VIS header).
    pub fn image_ms(&self) -> f64 {
        let lead = if self.family == Family::Scottie { SCOTTIE_SYNC_MS } else { 0.0 };
        lead + self.tx_lines as f64 * self.line_ms()
    }

    /// VIS header + picture, without preamble or identification.
    pub fn total_ms(&self) -> f64 {
        VIS_HEADER_MS + self.image_ms()
    }

    pub fn is_color(&self) -> bool {
        self.family != Family::RobotBw
    }

    /// Color model as sent on the air.
    pub fn color_space(&self) -> &'static str {
        match self.timing {
            Timing::Robot { alternating: true, .. } | Timing::Pd { .. } | Timing::Yoniq { .. } => "YCrCb 4:2:0",
            Timing::Robot { alternating: false, .. } => "YCrCb 4:2:2",
            Timing::RobotBw { .. } => "Y",
            Timing::Martin { .. } | Timing::Scottie { .. } => "RGB",
        }
    }

    /// Canvas size. M2/M4/S2 scan in half the time of M1/S1, so they are often
    /// written 160(320)×H even though the picture is still 320 pixels wide.
    pub fn size_label(&self) -> String {
        match self.mode {
            Mode::MartinM2 | Mode::MartinM4 | Mode::ScottieS2 => {
                format!("160(320)×{}", self.height)
            }
            _ => format!("{}×{}", self.width, self.height),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_is_in_enum_order() {
        for m in Mode::ALL {
            assert_eq!(m.spec().mode, m);
        }
    }

    #[test]
    fn line_times_match_published_values() {
        let cases = [
            (Mode::MartinM1, 446.446),
            (Mode::MartinM2, 226.798),
            (Mode::ScottieS1, 428.22),
            (Mode::ScottieS2, 277.692),
            (Mode::ScottieDx, 1050.3),
            (Mode::Robot36, 150.0),
            (Mode::Robot72, 300.0),
            (Mode::Robot24, 200.0),
            (Mode::RobotBw8, 66.9),
            (Mode::RobotBw12, 100.0),
            (Mode::Pd50, 388.16),
            (Mode::Pd90, 703.04),
            (Mode::Pd120, 508.48),
            (Mode::Pd160, 804.416),
            (Mode::Pd180, 754.24),
            (Mode::Pd240, 1000.0),
            (Mode::Pd290, 937.28),
            (Mode::Yoniq, 545.6),
        ];
        for (mode, expected) in cases {
            let got = mode.spec().line_ms();
            assert!((got - expected).abs() < 1e-3, "{mode:?}: {got} != {expected}");
        }
    }

    #[test]
    fn image_durations_are_the_nominal_ones() {
        // Nominal names are the picture duration in seconds (rounded).
        let cases = [
            (Mode::MartinM1, 114.0),
            (Mode::MartinM2, 58.0),
            (Mode::ScottieS1, 110.0),
            (Mode::Robot36, 36.0),
            (Mode::Robot72, 72.0),
            (Mode::Robot24, 24.0),
            (Mode::RobotBw8, 8.0),
            (Mode::Pd50, 50.0),
            (Mode::Pd90, 90.0),
            (Mode::Pd120, 126.0),
            (Mode::Pd160, 161.0),
            (Mode::Pd180, 187.0),
            (Mode::Pd240, 248.0),
            (Mode::Pd290, 289.0),
        ];
        for (mode, secs) in cases {
            let got = mode.spec().image_ms() / 1000.0;
            assert!((got - secs).abs() < 1.0, "{mode:?}: {got} s");
        }
    }

    #[test]
    fn robot12_color_is_twelve_seconds_of_picture() {
        let spec = Mode::Robot12.spec();
        assert_eq!(spec.short, "R12");
        assert_eq!(spec.vis, 0x00);
        assert_eq!((spec.width, spec.height, spec.tx_lines), (160, 120, 120));
        match spec.timing {
            Timing::Robot { sync_ms, porch_ms, y_ms, sep_ms, sep_porch_ms, c_ms, alternating: true } => {
                assert_eq!(sync_ms, 7.0);
                assert_eq!(porch_ms, 3.0);
                assert_eq!(y_ms, 60.0);
                assert!((sep_ms + sep_porch_ms + c_ms - 30.0).abs() < 1e-9);
                assert_eq!(sep_ms, 2.7);
                assert_eq!(sep_porch_ms, 0.9);
                assert_eq!(c_ms, 26.4);
            }
            other => panic!("Robot 12 must be alternating color, got {other:?}"),
        }
        assert!((spec.line_ms() - 100.0).abs() < 1e-9, "{}", spec.line_ms());
        assert!((spec.image_ms() - 12_000.0).abs() < 1e-6, "{}", spec.image_ms());
        // The published 12 s are the picture. The VIS header is extra, as with Robot 36.
        assert!((spec.total_ms() - spec.image_ms() - VIS_HEADER_MS).abs() < 1e-6);
        assert!(spec.is_color());
    }

    #[test]
    fn yoniq_is_the_mmsstv_analog_mode() {
        let spec = Mode::Yoniq.spec();
        assert_eq!(spec.short, "YQ");
        assert_eq!(spec.name, "YoniQ");
        assert_eq!(spec.vis, 0x73);
        assert!(spec.vis_odd);
        assert_eq!((spec.width, spec.height, spec.tx_lines), (320, 256, 128));
        assert!((spec.line_ms() - 545.6).abs() < 1e-6, "{}", spec.line_ms());
        // 128 pairs. The VIS header is extra.
        assert!((spec.image_ms() - 69_836.8).abs() < 1e-3, "{}", spec.image_ms());
        assert!((spec.total_ms() - spec.image_ms() - VIS_HEADER_MS).abs() < 1e-6);
        assert_eq!(Mode::from_short("yq"), Some(Mode::Yoniq));
    }

    #[test]
    fn vis_codes_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for m in Mode::ALL {
            assert!(seen.insert(m.spec().vis), "duplicate VIS for {m:?}");
        }
    }

    #[test]
    fn cheat_sheet_matches_the_usual_definitions() {
        assert_eq!(Mode::Robot72.spec().color_space(), "YCrCb 4:2:2");
        assert_eq!(Mode::Robot72.spec().size_label(), "320×240");
        assert_eq!(Mode::Robot12.spec().color_space(), "YCrCb 4:2:0");
        assert_eq!(Mode::Robot36.spec().color_space(), "YCrCb 4:2:0");
        assert_eq!(Mode::MartinM1.spec().color_space(), "RGB");
        assert_eq!(Mode::MartinM1.spec().size_label(), "320×256");
        assert_eq!(Mode::MartinM2.spec().color_space(), "RGB");
        assert_eq!(Mode::MartinM2.spec().size_label(), "160(320)×256");
    }
}
