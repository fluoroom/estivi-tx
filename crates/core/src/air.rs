//! What one transmission contains, how long it lasts, and whether it fits.
//!
//! Durations come from the mode table and the AX.25 bit count. No picture is rendered
//! here: the engine renders when the turn actually goes on the air.

use chrono::{DateTime, NaiveDate, TimeDelta, Utc};

use crate::ax25::{self, InfoFields, LineSet};
use crate::dynamic::{DataCtx, expand};
use crate::encoder::{VOX_MS, vox_segments};
use crate::modes::{Family, Mode, SCOTTIE_SYNC_MS, VIS_HEADER_MS};
use crate::project::{BlockImage, ImageRole, Project, Still};
use crate::recovery;
use crate::schedule::{Announcement, DaysLeft, Occurrence, announcements, days_left, weekdays_label};
use crate::synth::{self, Seg};

/// A turn more than this late is dropped. Pausing the agenda does not extend it.
pub const MAX_LATE: TimeDelta = TimeDelta::seconds(90);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Due {
    Wait,
    Go,
    Skip,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PieceKind {
    Vox,
    Cw,
    Ax25,
    Ax25Mode,
    Vis,
    Welcome,
    Bumper,
    Image,
    Bye,
    Dtmf,
}

#[derive(Clone, Debug)]
pub struct Piece {
    pub kind: PieceKind,
    pub ms: f64,
    pub path: String,
    pub mode: Option<Mode>,
    pub image_id: i64,
    pub chain: i64,
    pub text: String,
    /// `ddmmyyyyhhnnss` for a YoniQ picture. Empty for every other piece.
    pub stamp: String,
}

#[derive(Clone, Debug)]
pub struct Turn {
    pub at: DateTime<Utc>,
    pub block_end: DateTime<Utc>,
    pub next_at: Option<DateTime<Utc>>,
    pub pieces: Vec<Piece>,
    pub lead_ms: u32,
    pub tail_ms: u32,
    /// False when the whole transmission does not fit before the next start and the block end.
    pub airs: bool,
    pub image_id: u64,
    pub welcome: bool,
    pub bye: bool,
    pub program_id: Option<u64>,
    pub block_id: Option<u64>,
    pub name: String,
    pub program_name: String,
    pub range: String,
    pub date: NaiveDate,
    pub weekdays: Option<[bool; 7]>,
    pub info: String,
    /// Grid turns are cut at the block end. Instant and activation turns are not.
    pub cutoff: bool,
    pub freq_hz: u64,
    pub mode: Mode,
    pub call: String,
    pub cw_wpm: f32,
    pub cw_tone: f32,
}

/// Where the picture scan sits inside the turn's audio, in milliseconds.
pub fn image_window(pieces: &[Piece]) -> (f64, f64) {
    let mut cursor = 0.0;
    for piece in pieces {
        if piece.kind == PieceKind::Image {
            return (cursor, piece.ms);
        }
        cursor += piece.ms;
    }
    (0.0, 0.0)
}

impl Turn {
    pub fn audio_ms(&self) -> f64 {
        self.pieces.iter().map(|p| p.ms).sum()
    }

    /// PTT lead + audio + PTT tail. All three have to fit.
    pub fn total_ms(&self) -> f64 {
        f64::from(self.lead_ms) + self.audio_ms() + f64::from(self.tail_ms)
    }
}

/// PTT comes up half a millisecond before the written end, never a whole second early.
pub fn release_at(block_end: DateTime<Utc>) -> DateTime<Utc> {
    block_end - TimeDelta::microseconds(500)
}

/// The transmission must finish strictly before the next start (or the block end) and,
/// when there is a next start inside the block, leave `min_pause_s` of silence.
pub fn fits(start: DateTime<Utc>, total_ms: f64, next: Option<DateTime<Utc>>, block_end: DateTime<Utc>, min_pause_s: u32) -> bool {
    let finish = start + TimeDelta::microseconds((total_ms * 1000.0).round() as i64);
    let limit = next.unwrap_or(block_end).min(block_end);
    if finish >= limit {
        return false;
    }
    if let Some(next) = next {
        if next - finish < TimeDelta::seconds(i64::from(min_pause_s)) {
            return false;
        }
    }
    true
}

pub fn due(now: DateTime<Utc>, turn: &Turn, pause_until: Option<DateTime<Utc>>, min_pause_s: u32) -> Due {
    let ready = pause_until.map_or(turn.at, |p| p.max(turn.at));
    if now < ready {
        return Due::Wait;
    }
    if now - ready > MAX_LATE {
        return Due::Skip;
    }
    // A pause can move the start. Re-check the fit at the instant we would actually key down.
    let start = now.max(ready);
    if fits(start, turn.total_ms(), turn.next_at, turn.block_end, min_pause_s) { Due::Go } else { Due::Skip }
}

pub fn plan_occurrence(project: &Project, occ: &Occurrence, today: NaiveDate, lead_ms: u32, tail_ms: u32, serial: u64, now: DateTime<Utc>) -> Vec<Turn> {
    let slots = occ.turns(project.grid.offset_s);
    let normals: Vec<&BlockImage> = occ.images.iter().filter(|i| matches!(i.role, ImageRole::Normal) && i.enabled && !i.path.trim().is_empty()).collect();
    let n = normals.len().min(slots.len());
    let program = occ.program_id.and_then(|id| project.program(id));
    let mut turns = Vec::new();
    for i in 0..n {
        let image = normals[i];
        let at = slots[i];
        let next_at = slots.get(i + 1).copied();
        let welcome = i == 0;
        let bye = i + 1 == n;
        let mut built = build(
            project,
            Build {
                at,
                block_start: occ.start,
                block_end: occ.end,
                next_at,
                lead_ms,
                tail_ms,
                cutoff: true,
                image,
                pres: bumpers(&occ.images, image.id, true),
                posts: bumpers(&occ.images, image.id, false),
                welcome: welcome.then_some(program.and_then(|p| p.welcome.as_ref())).flatten(),
                bye: bye.then_some(program.and_then(|p| p.bye.as_ref())).flatten(),
                cw: project.tx.cw_preamble || project.tx.cw_id,
                ax25: project.ax25.info,
                program_id: occ.program_id,
                block_id: occ.block_id,
                name: &occ.name,
                program_name: program.map(|p| p.name.as_str()).unwrap_or(""),
                range: &occ.range(),
                date: occ.date,
                weekdays: occ.weekdays,
                serial,
                now,
                today,
            },
        );
        built.welcome = welcome && built.pieces.iter().any(|p| p.kind == PieceKind::Welcome);
        built.bye = bye && built.pieces.iter().any(|p| p.kind == PieceKind::Bye);
        built.airs = fits(at, built.total_ms(), next_at, occ.end, project.tx.min_pause_s);
        turns.push(built);
    }
    turns
}

struct Build<'a> {
    at: DateTime<Utc>,
    block_start: DateTime<Utc>,
    block_end: DateTime<Utc>,
    next_at: Option<DateTime<Utc>>,
    lead_ms: u32,
    tail_ms: u32,
    cutoff: bool,
    image: &'a BlockImage,
    pres: Vec<&'a BlockImage>,
    posts: Vec<&'a BlockImage>,
    welcome: Option<&'a Still>,
    bye: Option<&'a Still>,
    cw: bool,
    ax25: bool,
    program_id: Option<u64>,
    block_id: Option<u64>,
    name: &'a str,
    program_name: &'a str,
    range: &'a str,
    date: NaiveDate,
    weekdays: Option<[bool; 7]>,
    serial: u64,
    now: DateTime<Utc>,
    today: NaiveDate,
}

fn bumpers(images: &[BlockImage], of: u64, pre: bool) -> Vec<&BlockImage> {
    images
        .iter()
        .filter(|i| i.enabled && !i.path.trim().is_empty())
        .filter(|i| if pre { matches!(i.role, ImageRole::Pre { of: t } if t == of) } else { matches!(i.role, ImageRole::Post { of: t } if t == of) })
        .collect()
}

fn build(project: &Project, b: Build) -> Turn {
    let mode = project.picture_mode(b.image.mode, b.block_id, b.program_id);
    let (days, daysleft) = program_days(project, b.program_id, b.today);
    let occ = dummy_occ(&b);
    let next = announcements(project, &occ, b.today);
    let next_lines: Vec<String> = next.iter().map(|a| ax25::next_field(&a.name, &a.range, &a.days, a.daysleft)).collect();
    let info = if b.ax25 {
        ax25::format_info(&InfoFields {
            lines: LineSet::ALL,
            freq_hz: project.freq_hz,
            call: &project.station.call,
            name: b.name,
            range: b.range,
            days: days.as_deref(),
            daysleft,
            next: &next_lines,
            comment: "",
        })
    } else {
        String::new()
    };
    let ctx = data_ctx(project, &b, mode, &days, daysleft, &next);
    let cw_text = expand(&project.tx.cw_text, &ctx, None);
    let mut pieces = Vec::new();
    if project.tx.vox_preamble {
        pieces.push(piece(PieceKind::Vox, VOX_MS, "", None, -1, -1, String::new()));
    }
    if b.cw && !cw_text.trim().is_empty() && project.tx.cw_preamble {
        pieces.push(cw_piece(&cw_text, project.tx.cw_wpm, project.tx.cw_tone_hz));
    }
    if !info.is_empty() {
        pieces.push(ax25_piece(&info, &project.station.call));
    }
    if let Some(still) = b.welcome.filter(|s| !s.path.trim().is_empty()) {
        push_sstv(&mut pieces, PieceKind::Welcome, &still.path, project.still_mode(still.mode, b.program_id), -1, i64::try_from(b.image.id).unwrap_or(-1), project.ax25.mode, Some("WELCOME"), &project.station.call, b.at, b.lead_ms);
    }
    let chain = i64::try_from(b.image.id).unwrap_or(-1);
    for pre in &b.pres {
        push_sstv(&mut pieces, PieceKind::Bumper, &pre.path, project.picture_mode(pre.mode, b.block_id, b.program_id), i64::try_from(pre.id).unwrap_or(-1), chain, project.ax25.mode, Some("BUMPER PRE"), &project.station.call, b.at, b.lead_ms);
    }
    push_sstv(&mut pieces, PieceKind::Image, &b.image.path, mode, chain, chain, project.ax25.mode, None, &project.station.call, b.at, b.lead_ms);
    for post in &b.posts {
        push_sstv(&mut pieces, PieceKind::Bumper, &post.path, project.picture_mode(post.mode, b.block_id, b.program_id), i64::try_from(post.id).unwrap_or(-1), chain, project.ax25.mode, Some("BUMPER POST"), &project.station.call, b.at, b.lead_ms);
    }
    if let Some(still) = b.bye.filter(|s| !s.path.trim().is_empty()) {
        push_sstv(&mut pieces, PieceKind::Bye, &still.path, project.still_mode(still.mode, b.program_id), -1, chain, project.ax25.mode, Some("BYE"), &project.station.call, b.at, b.lead_ms);
    }
    if !info.is_empty() {
        pieces.push(ax25_piece(&info, &project.station.call));
    }
    if b.cw && !cw_text.trim().is_empty() && project.tx.cw_id {
        pieces.push(cw_piece(&cw_text, project.tx.cw_wpm, project.tx.cw_tone_hz));
    }
    Turn {
        at: b.at,
        block_end: b.block_end,
        next_at: b.next_at,
        pieces,
        lead_ms: b.lead_ms,
        tail_ms: b.tail_ms,
        airs: true,
        image_id: b.image.id,
        welcome: false,
        bye: false,
        program_id: b.program_id,
        block_id: b.block_id,
        name: b.name.to_string(),
        program_name: b.program_name.to_string(),
        range: b.range.to_string(),
        date: b.date,
        weekdays: b.weekdays,
        info,
        cutoff: b.cutoff,
        freq_hz: project.freq_hz,
        mode,
        call: project.station.call.clone(),
        cw_wpm: project.tx.cw_wpm,
        cw_tone: project.tx.cw_tone_hz,
    }
}

fn dummy_occ(b: &Build) -> Occurrence {
    Occurrence {
        program_id: b.program_id,
        block_id: b.block_id,
        name: b.name.to_string(),
        date: b.date,
        start: b.block_start,
        end: b.block_end,
        start_hm: String::new(),
        end_hm: String::new(),
        weekdays: b.weekdays,
        hue: None,
        block_enabled: true,
        program_enabled: true,
        empty: false,
        images: Vec::new(),
        interval_min: 6,
    }
}

fn program_days(project: &Project, id: Option<u64>, today: NaiveDate) -> (Option<String>, Option<u32>) {
    let Some(program) = id.and_then(|id| project.program(id)) else { return (None, None) };
    let days = weekdays_label(&program.weekdays);
    let left = match days_left(program, project.grid.timezone, today, project.grid.offset_s) {
        DaysLeft::Value(n) => Some(n),
        DaysLeft::Expired => None,
    };
    (Some(days), left)
}

fn data_ctx(project: &Project, b: &Build, mode: Mode, days: &Option<String>, daysleft: Option<u32>, next: &[Announcement]) -> DataCtx {
    DataCtx {
        call: project.station.call.clone(),
        operator: project.station.operator.clone(),
        qth: project.station.qth.clone(),
        locator: project.station.locator.clone(),
        now: b.now,
        serial: b.serial,
        mode,
        program: b.program_name.to_string(),
        freq_hz: project.freq_hz,
        cw: String::new(),
        dtmf: String::new(),
        block: b.name.to_string(),
        range: b.range.to_string(),
        days: days.clone().unwrap_or_default(),
        daysleft: daysleft.map(|n| n.to_string()).unwrap_or_default(),
        next: next.iter().map(|a| ax25::next_field(&a.name, &a.range, &a.days, a.daysleft)).collect::<Vec<_>>().join("\n"),
    }
}

fn piece(kind: PieceKind, ms: f64, path: &str, mode: Option<Mode>, image_id: i64, chain: i64, text: String) -> Piece {
    Piece { kind, ms, path: path.to_string(), mode, image_id, chain, text, stamp: String::new() }
}

fn cw_piece(text: &str, wpm: f32, tone: f32) -> Piece {
    let ms = synth::cw_segments(text, wpm, tone).iter().map(|s| s.ms).sum();
    piece(PieceKind::Cw, ms, "", None, -1, -1, text.to_string())
}

fn ax25_piece(text: &str, call: &str) -> Piece {
    piece(PieceKind::Ax25, ax25::duration_ms(text, call), "", None, -1, -1, text.to_string())
}

/// Local `ddmmyyyyhhnnss` at the moment this picture's scan is planned to start.
fn yoniq_stamp(at: DateTime<Utc>, lead_ms: u32, before_ms: f64) -> String {
    let start = at + TimeDelta::milliseconds(i64::from(lead_ms) + before_ms.round() as i64);
    recovery::stamp(start.with_timezone(&chrono::Local).naive_local())
}

fn push_sstv(pieces: &mut Vec<Piece>, kind: PieceKind, path: &str, mode: Mode, image_id: i64, chain: i64, mode_ax25: bool, mark: Option<&str>, call: &str, at: DateTime<Utc>, lead_ms: u32) {
    let stamp = (mode == Mode::Yoniq).then(|| yoniq_stamp(at, lead_ms, pieces.iter().map(|p| p.ms).sum()));
    if mode_ax25 {
        let text = ax25::mode_line(mode.spec().short, mark, stamp.as_deref());
        pieces.push(piece(PieceKind::Ax25Mode, ax25::duration_ms(&text, call), "", Some(mode), image_id, chain, text));
    }
    let spec = mode.spec();
    let vis = VIS_HEADER_MS + if spec.family == Family::Scottie { SCOTTIE_SYNC_MS } else { 0.0 };
    pieces.push(piece(PieceKind::Vis, vis, "", Some(mode), image_id, chain, String::new()));
    let scan = spec.tx_lines as f64 * spec.line_ms();
    let mut picture = piece(kind, scan, path, Some(mode), image_id, chain, String::new());
    if let Some(stamp) = stamp {
        picture.stamp = stamp;
    }
    pieces.push(picture);
}

/// Audio for one piece. SSTV picture pieces are not included: the engine scans those from the composed frame.
pub fn piece_segments(piece: &Piece, call: &str, wpm: f32, tone: f32) -> Option<Vec<Seg>> {
    match piece.kind {
        PieceKind::Vox => Some(vox_segments()),
        PieceKind::Cw => Some(synth::cw_segments(&piece.text, wpm, tone)),
        PieceKind::Ax25 | PieceKind::Ax25Mode => Some(ax25::segments(&piece.text, call)),
        PieceKind::Dtmf => Some(synth::dtmf_segments(&piece.text)),
        PieceKind::Vis => piece.mode.map(|m| crate::encoder::sstv_header(m)),
        PieceKind::Welcome | PieceKind::Bumper | PieceKind::Image | PieceKind::Bye => None,
    }
}

#[derive(Clone, Debug)]
pub struct Manual {
    pub cw: bool,
    pub ax25: bool,
    pub welcome: Option<Still>,
    pub pre: Vec<Still>,
    pub image: Still,
    pub post: Vec<Still>,
    pub bye: Option<Still>,
    pub program_id: Option<u64>,
    pub block_id: Option<u64>,
    pub name: String,
    pub program_name: String,
    pub range: String,
    pub date: NaiveDate,
    pub weekdays: Option<[bool; 7]>,
}

/// The instant tab has no schedule and is not cut when a block ends.
pub fn plan_manual(project: &Project, manual: &Manual, lead_ms: u32, tail_ms: u32, serial: u64, now: DateTime<Utc>, today: NaiveDate) -> Turn {
    let image = BlockImage { id: 0, path: manual.image.path.clone(), enabled: true, mode: manual.image.mode, role: ImageRole::Normal };
    let pres: Vec<BlockImage> = manual.pre.iter().map(|s| BlockImage { id: 0, path: s.path.clone(), enabled: true, mode: s.mode, role: ImageRole::Pre { of: 0 } }).collect();
    let posts: Vec<BlockImage> = manual.post.iter().map(|s| BlockImage { id: 0, path: s.path.clone(), enabled: true, mode: s.mode, role: ImageRole::Post { of: 0 } }).collect();
    let mut turn = build(
        project,
        Build {
            at: now,
            block_start: now,
            block_end: now + TimeDelta::days(1),
            next_at: None,
            lead_ms,
            tail_ms,
            cutoff: false,
            image: &image,
            pres: pres.iter().collect(),
            posts: posts.iter().collect(),
            welcome: manual.welcome.as_ref(),
            bye: manual.bye.as_ref(),
            cw: manual.cw,
            ax25: manual.ax25,
            program_id: manual.program_id,
            block_id: manual.block_id,
            name: &manual.name,
            program_name: &manual.program_name,
            range: &manual.range,
            date: manual.date,
            weekdays: manual.weekdays,
            serial,
            now,
            today,
        },
    );
    turn.airs = true;
    turn.cutoff = false;
    turn.welcome = turn.pieces.iter().any(|p| p.kind == PieceKind::Welcome);
    turn.bye = turn.pieces.iter().any(|p| p.kind == PieceKind::Bye);
    turn
}

/// Rows under one block: silence, each piece, the total, and the gap until the next start.
#[derive(Clone, Debug)]
pub struct Row {
    pub kind: RowKind,
    pub ms: f64,
    pub image_id: i64,
    pub chain: i64,
    pub editable: bool,
    pub airs: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowKind {
    Vox,
    Cw,
    Ax25,
    Ax25Mode,
    Vis,
    Welcome,
    Bumper,
    Image,
    Bye,
    Silence,
    Total,
}

pub fn timeline(turns: &[Turn], block_start: DateTime<Utc>, block_end: DateTime<Utc>) -> Vec<Row> {
    let mut rows = Vec::new();
    if let Some(first) = turns.first() {
        let gap = (first.at - block_start).num_milliseconds() as f64;
        if gap > 0.5 {
            rows.push(Row { kind: RowKind::Silence, ms: gap, image_id: -1, chain: -1, editable: false, airs: true });
        }
    }
    for (i, turn) in turns.iter().enumerate() {
        for p in &turn.pieces {
            let editable = matches!(p.kind, PieceKind::Bumper | PieceKind::Image);
            rows.push(Row {
                kind: match p.kind {
                    PieceKind::Vox => RowKind::Vox,
                    PieceKind::Cw => RowKind::Cw,
                    PieceKind::Ax25 => RowKind::Ax25,
                    PieceKind::Ax25Mode => RowKind::Ax25Mode,
                    PieceKind::Vis => RowKind::Vis,
                    PieceKind::Welcome => RowKind::Welcome,
                    PieceKind::Bumper => RowKind::Bumper,
                    PieceKind::Image => RowKind::Image,
                    PieceKind::Bye => RowKind::Bye,
                    PieceKind::Dtmf => RowKind::Ax25,
                },
                ms: p.ms,
                image_id: p.image_id,
                chain: p.chain,
                editable,
                airs: turn.airs,
            });
        }
        rows.push(Row { kind: RowKind::Total, ms: turn.total_ms(), image_id: i64::try_from(turn.image_id).unwrap_or(-1), chain: -1, editable: false, airs: turn.airs });
        let until = turns.get(i + 1).map(|n| n.at).unwrap_or(block_end);
        let gap = (until - turn.at).num_milliseconds() as f64 - turn.total_ms();
        rows.push(Row { kind: RowKind::Silence, ms: gap, image_id: -1, chain: -1, editable: false, airs: true });
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::{Block, ImageRole, Program, Still};
    use chrono::TimeZone;

    fn at(s: &str) -> DateTime<Utc> {
        Utc.from_utc_datetime(&chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S").unwrap())
    }

    fn channel(end: &str, images: Vec<BlockImage>) -> (Project, Occurrence) {
        let mut p = Project::default();
        p.tx.min_pause_s = 60;
        p.tx.cw_id = false;
        p.tx.mode = Mode::Robot12;
        p.station.call = "LU1ABC".into();
        let mut days = [false; 7];
        days[0] = true;
        let mut news = Program {
            id: 1,
            name: "Noticias".into(),
            weekdays: days,
            welcome: Some(Still { path: "hola.png".into(), mode: Some(Mode::Robot12) }),
            bye: Some(Still { path: "chau.png".into(), mode: Some(Mode::Robot12) }),
            ..Program::default()
        };
        news.every_day("14:00", end, 6);
        p.programs.push(news);
        p.blocks.push(Block {
            id: 7,
            program: Some(1),
            date: NaiveDate::from_ymd_opt(2026, 10, 26).unwrap(),
            images,
            ..Block::default()
        });
        let today = NaiveDate::from_ymd_opt(2026, 10, 26).unwrap();
        let occ = crate::schedule::occurrences(&p, today, today + TimeDelta::days(1), today).into_iter().next().unwrap();
        (p, occ)
    }

    fn pics(n: usize, mode: Mode) -> Vec<BlockImage> {
        (1..=n).map(|i| BlockImage { id: i as u64, path: format!("{i}.png"), enabled: true, mode: Some(mode), role: ImageRole::Normal }).collect()
    }

    #[test]
    fn five_robot12_fit_and_the_sixth_only_when_three_minutes_remain() {
        let (p, occ) = channel("14:30", pics(6, Mode::Robot12));
        let today = NaiveDate::from_ymd_opt(2026, 10, 26).unwrap();
        let turns = plan_occurrence(&p, &occ, today, 0, 0, 1, occ.start);
        assert_eq!(turns.len(), 5, "a sixth picture has no turn");
        assert!(turns.iter().all(|t| t.airs));
        assert!(turns[0].welcome && !turns[0].bye);
        assert!(turns[4].bye && !turns[4].welcome);
        assert!((turns[0].audio_ms() - Mode::Robot12.spec().total_ms() * 2.0).abs() < 1.0, "welcome plus the picture");
        let vis = turns[0].pieces.iter().filter(|p| p.kind == PieceKind::Vis).count();
        assert_eq!(vis, 2, "welcome and the picture each carry VIS; bye is on the last turn");
        assert_eq!(turns[4].pieces.iter().filter(|p| p.kind == PieceKind::Vis).count(), 2);

        let (p, occ) = channel("14:33", pics(6, Mode::Robot12));
        let turns = plan_occurrence(&p, &occ, today, 0, 0, 1, occ.start);
        assert_eq!(turns.len(), 6);
        assert!(turns[5].airs, "Robot 12 fits in the last three minutes");
        assert!(turns[5].bye);

        let (mut p, occ) = channel("14:33", pics(6, Mode::Pd290));
        p.tx.min_pause_s = 0;
        let turns = plan_occurrence(&p, &occ, today, 0, 0, 1, occ.start);
        assert!(!turns[5].airs, "PD 290 does not fit in three minutes");
        assert!(turns[5].bye, "bye stays on the last assigned turn");
        assert!(!turns[4].bye, "bye does not move backward");
    }

    #[test]
    fn a_dropped_first_turn_keeps_the_welcome() {
        let (mut p, occ) = channel("14:30", pics(2, Mode::Pd290));
        p.tx.min_pause_s = 120;
        let today = NaiveDate::from_ymd_opt(2026, 10, 26).unwrap();
        let turns = plan_occurrence(&p, &occ, today, 0, 0, 1, occ.start);
        assert!(!turns[0].airs);
        assert!(turns[0].welcome);
        assert!(!turns[1].welcome);
    }

    #[test]
    fn pause_and_lead_can_push_a_turn_out() {
        let (mut p, occ) = channel("14:30", pics(1, Mode::Robot12));
        p.tx.min_pause_s = 0;
        let today = NaiveDate::from_ymd_opt(2026, 10, 26).unwrap();
        let turns = plan_occurrence(&p, &occ, today, 0, 0, 1, occ.start);
        assert!(turns[0].airs);
        let late = plan_occurrence(&p, &occ, today, 500, 500, 1, occ.start);
        // 12.9 s plus a second of lead and tail still fits in six minutes, and there is no next start.
        assert!(late[0].airs);
        assert!((late[0].total_ms() - late[0].audio_ms() - 1000.0).abs() < 0.1);

        let start = at("2026-10-26 14:00:00");
        let end = at("2026-10-26 14:00:13");
        let audio = Mode::Robot12.spec().total_ms();
        assert!(fits(start, audio, None, end, 0));
        assert!(!fits(start, audio + 200.0, None, end, 0), "lead and tail count");
        assert!(!fits(start, audio, Some(start + TimeDelta::seconds(20)), end + TimeDelta::minutes(10), 8));
    }

    #[test]
    fn ninety_seconds_late_skips_and_the_release_is_under_a_millisecond() {
        let (p, occ) = channel("14:30", pics(1, Mode::Robot12));
        let today = NaiveDate::from_ymd_opt(2026, 10, 26).unwrap();
        let turns = plan_occurrence(&p, &occ, today, 0, 0, 1, occ.start);
        let at = turns[0].at;
        assert_eq!(due(at - TimeDelta::seconds(1), &turns[0], None, 60), Due::Wait);
        assert_eq!(due(at + TimeDelta::seconds(89), &turns[0], None, 60), Due::Go);
        assert_eq!(due(at + TimeDelta::seconds(91), &turns[0], None, 60), Due::Skip);
        let end = at + TimeDelta::minutes(6);
        let released = release_at(end);
        assert_eq!((end - released).num_microseconds(), Some(500));
        let manual = plan_manual(
            &p,
            &Manual {
                cw: false,
                ax25: true,
                welcome: None,
                pre: vec![],
                image: Still { path: "x.png".into(), mode: Some(Mode::Robot12) },
                post: vec![],
                bye: None,
                program_id: None,
                block_id: None,
                name: "ahora".into(),
                program_name: String::new(),
                range: String::new(),
                date: today,
                weekdays: None,
            },
            0,
            0,
            1,
            at,
            today,
        );
        assert!(!manual.cutoff);
        assert!(manual.info.contains("NAME ahora\n"));
        assert!(!manual.info.contains("DAYS"));
    }

    #[test]
    fn vox_and_ax25_sit_outside_the_pictures() {
        let (mut p, occ) = channel("14:30", pics(1, Mode::Robot12));
        p.tx.vox_preamble = true;
        p.tx.cw_preamble = false;
        p.tx.cw_id = false;
        p.ax25.info = true;
        p.ax25.mode = true;
        let today = NaiveDate::from_ymd_opt(2026, 10, 26).unwrap();
        let turns = plan_occurrence(&p, &occ, today, 0, 0, 1, occ.start);
        let kinds: Vec<_> = turns[0].pieces.iter().map(|p| p.kind).collect();
        assert_eq!(kinds[0], PieceKind::Vox);
        assert_eq!(kinds[1], PieceKind::Ax25);
        assert!(kinds.contains(&PieceKind::Ax25Mode));
        assert!(turns[0].info.lines().any(|l| l.starts_with("DAYS Mon")));
        assert!(turns[0].info.lines().any(|l| l.starts_with("DAYSLEFT ")));
        let rows = timeline(&turns, occ.start, occ.end);
        assert!(rows.iter().any(|r| r.kind == RowKind::Total && r.airs));
        assert!(rows.last().unwrap().kind == RowKind::Silence);
    }

    #[test]
    fn yoniq_names_the_recovery_file_in_the_mode_frame() {
        let (mut p, _) = channel("14:30", pics(1, Mode::Yoniq));
        p.ax25.mode = true;
        p.programs[0].welcome = None;
        p.programs[0].bye = None;
        let today = NaiveDate::from_ymd_opt(2026, 10, 26).unwrap();
        let occ = crate::schedule::occurrences(&p, today, today + TimeDelta::days(1), today).into_iter().next().unwrap();
        let turns = plan_occurrence(&p, &occ, today, 0, 0, 1, occ.start);
        let picture = turns[0].pieces.iter().find(|p| p.kind == PieceKind::Image).unwrap();
        assert_eq!(picture.stamp.len(), 14);
        assert!(picture.stamp.bytes().all(|b| b.is_ascii_digit()));
        let mode = turns[0].pieces.iter().find(|p| p.kind == PieceKind::Ax25Mode).unwrap();
        assert!(mode.text.contains(&picture.stamp), "{}", mode.text);
        assert!(mode.text.starts_with("ESTIVI MODE YQ "));
    }
}
