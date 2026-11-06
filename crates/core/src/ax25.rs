//! AX.25 UI frames at a fixed 1200 baud AFSK (1200 Hz mark, 2200 Hz space).
//! The speed is not a project option: every info frame and every mode frame uses it.

use crate::synth::Seg;

pub const BIT_MS: f64 = 1000.0 / 1200.0;
pub const TAIL_MS: f64 = 100.0;
const MARK_HZ: f32 = 1200.0;
const SPACE_HZ: f32 = 2200.0;
const PREAMBLE_FLAGS: usize = 24;
const TRAILER_FLAGS: usize = 2;

/// Which of the standard lines an activation wants. A grid transmission turns them all on;
/// a line that does not apply to the block (DAYS on a common block) is still omitted.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LineSet {
    pub header: bool,
    pub freq: bool,
    pub call: bool,
    pub name: bool,
    pub range: bool,
    pub days: bool,
    pub daysleft: bool,
    pub next: bool,
}

impl LineSet {
    pub const ALL: LineSet = LineSet {
        header: true,
        freq: true,
        call: true,
        name: true,
        range: true,
        days: true,
        daysleft: true,
        next: true,
    };
}

#[derive(Clone, Debug, Default)]
pub struct InfoFields<'a> {
    pub lines: LineSet,
    pub freq_hz: u64,
    pub call: &'a str,
    pub name: &'a str,
    pub range: &'a str,
    /// `None` on a common block: the DAYS line is omitted so a client does not repeat it.
    pub days: Option<&'a str>,
    pub daysleft: Option<u32>,
    pub next: &'a [String],
    pub comment: &'a str,
}

/// One line per field, `\n` terminated. Version `1` is the text format, not the file format.
pub fn format_info(f: &InfoFields) -> String {
    let mut lines = Vec::new();
    if f.lines.header {
        lines.push("ESTIVI 1".to_string());
    }
    if f.lines.freq {
        lines.push(format!("FREQ {}", f.freq_hz));
    }
    if f.lines.call {
        lines.push(format!("CALL {}", one_line(f.call)));
    }
    if f.lines.name {
        lines.push(format!("NAME {}", one_line(f.name)));
    }
    if f.lines.range {
        lines.push(format!("RANGE {}", one_line(f.range)));
    }
    if f.lines.days {
        if let Some(days) = f.days {
            lines.push(format!("DAYS {days}"));
        }
    }
    if f.lines.daysleft {
        if let Some(n) = f.daysleft {
            lines.push(format!("DAYSLEFT {n}"));
        }
    }
    if f.lines.next {
        for n in f.next {
            // The pipes are the field separators. Only a name is stripped of them, in `next_field`.
            lines.push(format!("NEXT {}", n.replace(['\n', '\r'], " ").trim()));
        }
    }
    if !f.comment.trim().is_empty() {
        lines.push(format!("COMMENT {}", one_line(f.comment)));
    }
    if lines.is_empty() {
        return String::new();
    }
    let mut s = lines.join("\n");
    s.push('\n');
    s
}

/// `Nombre|14:00-14:30|Mon,Tue|12`. A common block passes an empty day list and `0`.
pub fn next_field(name: &str, range: &str, days: &str, daysleft: u32) -> String {
    format!("{}|{range}|{days}|{daysleft}", one_line(name))
}

fn one_line(s: &str) -> String {
    s.replace(['\n', '\r', '|'], " ").trim().to_string()
}

/// `ESTIVI MODE R36`, `ESTIVI MODE R12 BUMPER PRE`, `ESTIVI MODE M1 WELCOME`.
pub fn mode_line(short: &str, mark: Option<&str>) -> String {
    let mut s = format!("ESTIVI MODE {short}");
    if let Some(m) = mark.filter(|m| !m.is_empty()) {
        s.push(' ');
        s.push_str(m);
    }
    s.push('\n');
    s
}

pub fn duration_ms(info: &str, call: &str) -> f64 {
    on_air_bits(info, call).len() as f64 * BIT_MS + TAIL_MS
}

/// AFSK audio for one UI frame, plus 100 ms of silence so a decoder can close the frame.
pub fn segments(info: &str, call: &str) -> Vec<Seg> {
    let mut tone = MARK_HZ;
    let mut out = Vec::new();
    for bit in on_air_bits(info, call) {
        // NRZI: a zero toggles the tone, a one holds it. Phase stays continuous
        // because each segment starts where the previous one ended.
        if !bit {
            tone = if (tone - MARK_HZ).abs() < 1.0 { SPACE_HZ } else { MARK_HZ };
        }
        out.push(Seg::tone(tone, BIT_MS));
    }
    out.push(Seg::silence(TAIL_MS));
    out
}

/// Unstuffed body: addresses, control `0x03`, PID `0xF0`, info, FCS (low byte first).
pub fn frame_bytes(info: &str, call: &str) -> Vec<u8> {
    let (base, ssid) = split_call(call);
    let mut body = Vec::new();
    body.extend(address("ESTIVI", 0, false, true));
    body.extend(address(&base, ssid, true, false));
    body.push(0x03);
    body.push(0xF0);
    body.extend(info.as_bytes());
    let crc = fcs(&body);
    body.extend(crc.to_le_bytes());
    body
}

fn on_air_bits(info: &str, call: &str) -> Vec<bool> {
    let mut bits = Vec::new();
    for _ in 0..PREAMBLE_FLAGS {
        push_flag(&mut bits);
    }
    let mut ones = 0u8;
    for byte in frame_bytes(info, call) {
        for i in 0..8 {
            let one = (byte >> i) & 1 == 1;
            bits.push(one);
            if one {
                ones += 1;
                if ones == 5 {
                    bits.push(false);
                    ones = 0;
                }
            } else {
                ones = 0;
            }
        }
    }
    push_flag(&mut bits);
    for _ in 0..TRAILER_FLAGS {
        push_flag(&mut bits);
    }
    bits
}

fn push_flag(bits: &mut Vec<bool>) {
    // 0x7E, LSB first, not bit-stuffed.
    for i in 0..8 {
        bits.push((0x7E >> i) & 1 == 1);
    }
}

fn address(call: &str, ssid: u8, last: bool, command: bool) -> [u8; 7] {
    let mut out = [b' ' << 1; 7];
    for (i, c) in call.chars().filter(|c| c.is_ascii_alphanumeric()).take(6).enumerate() {
        out[i] = (c.to_ascii_uppercase() as u8) << 1;
    }
    let mut b = 0x60 | ((ssid & 0x0F) << 1);
    if command {
        b |= 0x80;
    }
    if last {
        b |= 0x01;
    }
    out[6] = b;
    out
}

fn split_call(call: &str) -> (String, u8) {
    let call = call.trim();
    if call.is_empty() {
        return ("NOCALL".into(), 0);
    }
    if let Some((base, ssid)) = call.rsplit_once('-') {
        if let Ok(n) = ssid.parse::<u8>() {
            if n <= 15 && !base.is_empty() {
                return (clean_call(base), n);
            }
        }
    }
    (clean_call(call), 0)
}

fn clean_call(s: &str) -> String {
    let s: String = s.chars().filter(|c| c.is_ascii_alphanumeric()).take(6).collect();
    if s.is_empty() { "NOCALL".into() } else { s.to_ascii_uppercase() }
}

/// CRC-16/CCITT reflected, init 0xFFFF, xorout 0xFFFF. The AX.25 FCS.
pub fn fcs(data: &[u8]) -> u16 {
    !residue_raw(data)
}

fn residue_raw(data: &[u8]) -> u16 {
    let mut crc = 0xFFFFu16;
    for &b in data {
        crc ^= u16::from(b);
        for _ in 0..8 {
            crc = if crc & 1 == 1 { (crc >> 1) ^ 0x8408 } else { crc >> 1 };
        }
    }
    crc
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> String {
        format_info(&InfoFields {
            lines: LineSet::ALL,
            freq_hz: 145_825_000,
            call: "LU1ABC",
            name: "Noticias",
            range: "14:00-14:30",
            days: Some("Mon,Tue,Wed,Thu,Fri"),
            daysleft: Some(12),
            next: &[
                "Noticias|14:00-14:30|Mon,Tue,Wed,Thu,Fri|12".into(),
                "Clima|18:00-18:15|Sat,Sun|0".into(),
            ],
            comment: "",
        })
    }

    #[test]
    fn info_roundtrip_matches_the_published_layout() {
        let text = sample();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(
            lines,
            [
                "ESTIVI 1",
                "FREQ 145825000",
                "CALL LU1ABC",
                "NAME Noticias",
                "RANGE 14:00-14:30",
                "DAYS Mon,Tue,Wed,Thu,Fri",
                "DAYSLEFT 12",
                "NEXT Noticias|14:00-14:30|Mon,Tue,Wed,Thu,Fri|12",
                "NEXT Clima|18:00-18:15|Sat,Sun|0",
            ]
        );
        assert!(text.ends_with('\n'));
    }

    #[test]
    fn a_common_block_omits_days_and_daysleft() {
        let text = format_info(&InfoFields {
            lines: LineSet::ALL,
            freq_hz: 145_825_000,
            call: "LU1ABC",
            name: "Clima",
            range: "18:00-18:15",
            days: None,
            daysleft: None,
            next: &["Clima|18:00-18:15||0".into()],
            comment: "",
        });
        assert!(text.lines().all(|l| !l.starts_with("DAYS")));
        assert!(text.contains("NEXT Clima|18:00-18:15||0\n"));
    }

    #[test]
    fn activation_lines_are_optional_and_comment_is_one_line() {
        let text = format_info(&InfoFields {
            lines: LineSet { header: true, call: true, ..LineSet::default() },
            call: "LU1ABC",
            comment: "hola\nmundo",
            ..InfoFields::default()
        });
        assert_eq!(text.lines().collect::<Vec<_>>(), ["ESTIVI 1", "CALL LU1ABC", "COMMENT hola mundo"]);
    }

    #[test]
    fn mode_preamble_marks() {
        assert_eq!(mode_line("R36", None), "ESTIVI MODE R36\n");
        assert_eq!(mode_line("R12", Some("BUMPER PRE")), "ESTIVI MODE R12 BUMPER PRE\n");
        assert_eq!(mode_line("BW8", Some("BUMPER POST")), "ESTIVI MODE BW8 BUMPER POST\n");
        assert_eq!(mode_line("M1", Some("WELCOME")), "ESTIVI MODE M1 WELCOME\n");
        assert_eq!(mode_line("R24", Some("BYE")), "ESTIVI MODE R24 BYE\n");
    }

    #[test]
    fn frame_is_a_ui_to_estivi_and_the_fcs_checks() {
        let info = sample();
        let frame = frame_bytes(&info, "LU1ABC-7");
        let dest: String = frame[..6].iter().map(|b| (b >> 1) as char).collect();
        assert_eq!(dest.trim(), "ESTIVI");
        let src: String = frame[7..13].iter().map(|b| (b >> 1) as char).collect();
        assert_eq!(src.trim(), "LU1ABC");
        assert_eq!(frame[6] & 0x81, 0x80, "destination is the command address and not the last");
        assert_eq!(frame[13] & 0x1F, (7 << 1) | 0x01, "ssid 7 and the last-address bit");
        assert_eq!(frame[14], 0x03);
        assert_eq!(frame[15], 0xF0);
        let info_end = frame.len() - 2;
        assert_eq!(&frame[16..info_end], info.as_bytes());
        // Good-frame residue of this CRC, over the body including the FCS.
        assert_eq!(residue_raw(&frame), 0xF0B8);
    }

    #[test]
    fn duration_is_the_stuffed_audio_plus_the_tail() {
        let info = "ESTIVI 1\n";
        let frame = frame_bytes(info, "N0CALL");
        let bits = on_air_bits(info, "N0CALL");
        let mut stuffed = 0usize;
        let mut ones = 0u8;
        for b in &frame {
            for i in 0..8 {
                stuffed += 1;
                if (b >> i) & 1 == 1 {
                    ones += 1;
                    if ones == 5 {
                        stuffed += 1;
                        ones = 0;
                    }
                } else {
                    ones = 0;
                }
            }
        }
        let flags = PREAMBLE_FLAGS + 1 + TRAILER_FLAGS;
        assert_eq!(bits.len(), flags * 8 + stuffed);
        let sum: f64 = segments(info, "N0CALL").iter().map(|s| s.ms).sum();
        let expect = bits.len() as f64 * BIT_MS + TAIL_MS;
        assert!((sum - expect).abs() < 1e-6, "{sum} vs {expect}");
        assert!((duration_ms(info, "N0CALL") - expect).abs() < 1e-6);
        // The 0.25 s in the plan is an illustration, not this duration.
        assert!(expect > 200.0, "{expect}");
    }

    #[test]
    fn five_ones_are_stuffed() {
        let bits = on_air_bits(&"\u{00ff}".repeat(4), "N0CALL");
        // Somewhere in the info (0xFF bytes) five ones are followed by a stuffed zero.
        let mut ones = 0;
        let mut stuffed = false;
        for b in bits {
            if b {
                ones += 1;
            } else {
                if ones == 5 {
                    stuffed = true;
                }
                ones = 0;
            }
            assert!(ones < 7, "six ones would be a flag inside the body");
        }
        assert!(stuffed);
    }
}
