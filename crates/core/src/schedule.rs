//! Calendar: occurrences, turns, overlap, days-left, and the cards the window draws.
//!
//! The offset moves every start. It does not move the end of a block. A start that lands
//! on the end (14:30 in a block that ends at 14:30) is not a turn.

use chrono::{DateTime, Datelike, NaiveDate, NaiveDateTime, TimeDelta, TimeZone, Utc};

use crate::project::{Block, BlockImage, GridTz, Program, Project, format_hhmm, hue_rgb, parse_hhmm, range_label};

pub use crate::project::GridTz as Tz;

/// 0 = open-ended, unset, or more than 99 days. 1 = today is the last day. 2..=99 count today.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DaysLeft {
    Expired,
    Value(u32),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Conflict {
    pub name: String,
    pub date: NaiveDate,
    pub range: String,
}

impl Conflict {
    pub fn text(&self, spanish: bool) -> String {
        let date = self.date.format("%Y-%m-%d");
        if spanish {
            format!("«{}» el {date} ({}) está en el camino", self.name, self.range)
        } else {
            format!("“{}” on {date} ({}) is in the way", self.name, self.range)
        }
    }
}

#[derive(Clone, Debug)]
pub enum EditError {
    Conflict(Conflict),
    /// Unchecking a weekday or deleting a program waits until the user picks what happens to the blocks.
    NeedChoice,
    Missing,
}

/// What to do with occurrences that would otherwise disappear.
#[derive(Clone, Copy, Debug)]
pub enum Keep {
    /// Drop the stored blocks this edit is allowed to touch.
    Discard,
    /// Turn the stored (non-empty) blocks into common blocks.
    KeepFilled,
    /// Also materialise empty occurrences through this date, capped at three years from today.
    KeepThrough(NaiveDate),
}

#[derive(Clone, Debug)]
pub struct Occurrence {
    pub program_id: Option<u64>,
    pub block_id: Option<u64>,
    pub name: String,
    pub date: NaiveDate,
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub start_hm: String,
    pub end_hm: String,
    pub weekdays: Option<[bool; 7]>,
    pub hue: Option<u16>,
    pub block_enabled: bool,
    pub program_enabled: bool,
    pub empty: bool,
    pub images: Vec<BlockImage>,
}

impl Occurrence {
    pub fn transmits(&self) -> bool {
        self.block_enabled && self.program_enabled && !self.empty
    }

    pub fn range(&self) -> String {
        range_label(&self.start_hm, &self.end_hm)
    }

    pub fn turns(&self, interval_min: u32, offset_s: u32) -> Vec<DateTime<Utc>> {
        turn_times(self.start, self.end, interval_min, offset_s)
    }

    fn same_as(&self, other: &Occurrence) -> bool {
        self.date == other.date && self.program_id == other.program_id && self.block_id == other.block_id && self.start == other.start
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Announcement {
    pub name: String,
    pub range: String,
    pub days: String,
    pub daysleft: u32,
    pub at: DateTime<Utc>,
    pub program_id: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Zoom {
    Month,
    Week,
    Day,
}

#[derive(Clone, Debug)]
pub struct Card {
    pub name: String,
    pub date: NaiveDate,
    pub start_hm: String,
    pub end_hm: String,
    pub start_min: i32,
    pub end_min: i32,
    /// 0 = Monday … 6 = Sunday, inside the visible window.
    pub col: i32,
    pub stack: i32,
    /// Top of the card, 0..1 of the day. Month view leaves this at 0.
    pub y: f32,
    pub h: f32,
    pub rgb: [u8; 3],
    pub opacity: f32,
    pub gray: bool,
    pub empty: bool,
    pub program_id: i64,
    pub block_id: i64,
}

const COMMON_RGB: [u8; 3] = [108, 114, 122];

pub fn future_limit(today: NaiveDate) -> NaiveDate {
    today + TimeDelta::days(365 * 3)
}

pub fn grid_date(tz: GridTz, t: DateTime<Utc>) -> NaiveDate {
    match tz {
        GridTz::Utc => t.date_naive(),
        GridTz::Local => t.with_timezone(&chrono::Local).date_naive(),
    }
}

pub fn to_utc(tz: GridTz, naive: NaiveDateTime) -> Option<DateTime<Utc>> {
    match tz {
        GridTz::Utc => Some(Utc.from_utc_datetime(&naive)),
        GridTz::Local => map_local(&chrono::Local, naive),
    }
}

fn map_local<Tz: TimeZone>(tz: &Tz, naive: NaiveDateTime) -> Option<DateTime<Utc>>
where
    Tz::Offset: std::fmt::Display,
{
    tz.from_local_datetime(&naive).earliest().map(|t| t.with_timezone(&Utc))
}

/// `(start, end)` in minutes. `end` is greater than `start`; a wrap adds a day.
pub fn window_minutes(start: &str, end: &str) -> Option<(u32, u32)> {
    let s = parse_hhmm(start)?;
    let mut e = parse_hhmm(end)?;
    if e <= s {
        e += 24 * 60;
    }
    Some((s, e))
}

pub fn ranges_overlap(a: (u32, u32), b: (u32, u32)) -> bool {
    fn parts(r: (u32, u32)) -> Vec<(u32, u32)> {
        let (s, e) = r;
        if e <= 24 * 60 {
            vec![(s, e)]
        } else {
            let mut v = vec![(s, 24 * 60)];
            if e > 24 * 60 {
                v.push((0, e - 24 * 60));
            }
            v
        }
    }
    for (a0, a1) in parts(a) {
        for (b0, b1) in parts(b) {
            if a0 < b1 && b0 < a1 {
                return true;
            }
        }
    }
    false
}

fn at_minutes(tz: GridTz, day: NaiveDate, minutes: u32) -> Option<DateTime<Utc>> {
    let extra = i64::from(minutes / (24 * 60));
    let m = minutes % (24 * 60);
    let day = day + TimeDelta::days(extra);
    let naive = day.and_hms_opt(m / 60, m % 60, 0)?;
    to_utc(tz, naive)
}

pub fn bounds(tz: GridTz, day: NaiveDate, start_min: u32, end_min: u32) -> Option<(DateTime<Utc>, DateTime<Utc>)> {
    Some((at_minutes(tz, day, start_min)?, at_minutes(tz, day, end_min)?))
}

/// Starts in `[start + offset, end)`. The end itself is not a start.
pub fn turn_times(start: DateTime<Utc>, end: DateTime<Utc>, interval_min: u32, offset_s: u32) -> Vec<DateTime<Utc>> {
    let mut t = start + TimeDelta::seconds(i64::from(offset_s));
    if interval_min == 0 {
        return if t < end { vec![t] } else { Vec::new() };
    }
    let step = TimeDelta::minutes(i64::from(interval_min));
    let mut out = Vec::new();
    while t < end && out.len() < 24 * 60 {
        out.push(t);
        t += step;
    }
    out
}

pub fn days_left(program: &Program, tz: GridTz, today: NaiveDate, offset_s: u32) -> DaysLeft {
    if !program.scheduled() {
        return DaysLeft::Value(0);
    }
    let Some(ends) = program.ends else {
        return DaysLeft::Value(0);
    };
    let Some(last) = last_air_day(program, tz, ends, offset_s) else {
        return DaysLeft::Expired;
    };
    if last < today {
        return DaysLeft::Expired;
    }
    let diff = (last - today).num_days();
    if diff > 98 { DaysLeft::Value(0) } else { DaysLeft::Value((diff + 1) as u32) }
}

fn last_air_day(program: &Program, tz: GridTz, ends: DateTime<Utc>, offset_s: u32) -> Option<NaiveDate> {
    let end_day = grid_date(tz, ends);
    let (s, e) = window_minutes(&program.start, &program.end)?;
    for back in 0..8 {
        let day = end_day - TimeDelta::days(back);
        let wd = day.weekday().num_days_from_monday() as usize;
        if !program.weekdays.get(wd).copied().unwrap_or(false) {
            continue;
        }
        let Some((start, _)) = bounds(tz, day, s, e) else { continue };
        if start + TimeDelta::seconds(i64::from(offset_s)) < ends {
            return Some(day);
        }
    }
    None
}

fn hours_of(program: &Program) -> Option<(u32, u32)> {
    program.scheduled().then(|| window_minutes(&program.start, &program.end)).flatten()
}

/// The weekday is selected, the window exists, and the keyed start is still before `ends`.
fn on_mask(program: &Program, tz: GridTz, day: NaiveDate, offset_s: u32) -> bool {
    let wd = day.weekday().num_days_from_monday() as usize;
    if !program.weekdays.get(wd).copied().unwrap_or(false) {
        return false;
    }
    let Some((s, e)) = window_minutes(&program.start, &program.end) else { return false };
    let Some((start, _)) = bounds(tz, day, s, e) else { return false };
    match program.ends {
        Some(ends) => start + TimeDelta::seconds(i64::from(offset_s)) < ends,
        None => true,
    }
}

fn alive(program: &Program, tz: GridTz, today: NaiveDate, offset_s: u32) -> bool {
    !matches!(days_left(program, tz, today, offset_s), DaysLeft::Expired)
}

pub fn occurrences(project: &Project, from: NaiveDate, to: NaiveDate, today: NaiveDate) -> Vec<Occurrence> {
    let mut out = Vec::new();
    let mut day = from;
    while day < to {
        for program in &project.programs {
            if let Some(occ) = program_occurrence(project, program, day, today) {
                out.push(occ);
            }
        }
        for block in &project.blocks {
            if block.program.is_none() && block.date == day {
                if let Some(occ) = common_occurrence(project, block) {
                    out.push(occ);
                }
            }
        }
        day += TimeDelta::days(1);
    }
    out.sort_by_key(|o| (o.start, o.name.clone()));
    out
}

fn program_occurrence(project: &Project, program: &Program, day: NaiveDate, today: NaiveDate) -> Option<Occurrence> {
    let tz = project.grid.timezone;
    let offset = project.grid.offset_s;
    if !alive(program, tz, today, offset) {
        return None;
    }
    let stored = project.program_block(program.id, day);
    let masked = on_mask(program, tz, day, offset);
    if !masked && stored.is_none() {
        return None;
    }
    let (s, e) = window_minutes(&program.start, &program.end)?;
    let (start, end) = bounds(tz, day, s, e)?;
    if !masked {
        // A stored block whose weekday was unchecked. It still inherits the program's hours,
        // but only while that start is inside the program's end date.
        let keyed = start + TimeDelta::seconds(i64::from(offset));
        if program.ends.is_some_and(|ends| keyed >= ends) {
            return None;
        }
    }
    let images = stored.map(|b| b.images.clone()).unwrap_or_default();
    Some(Occurrence {
        program_id: Some(program.id),
        block_id: stored.map(|b| b.id),
        name: program.name.clone(),
        date: day,
        start,
        end,
        start_hm: program.start.clone(),
        end_hm: program.end.clone(),
        weekdays: Some(program.weekdays),
        hue: Some(program.hue),
        block_enabled: stored.map(|b| b.enabled).unwrap_or(true),
        program_enabled: program.enabled,
        empty: stored.map(|b| !b.has_pictures()).unwrap_or(true),
        images,
    })
}

fn common_occurrence(project: &Project, block: &Block) -> Option<Occurrence> {
    let (s, e) = window_minutes(&block.start, &block.end)?;
    let (start, end) = bounds(project.grid.timezone, block.date, s, e)?;
    Some(Occurrence {
        program_id: None,
        block_id: Some(block.id),
        name: if block.name.trim().is_empty() { "Block".into() } else { block.name.clone() },
        date: block.date,
        start,
        end,
        start_hm: block.start.clone(),
        end_hm: block.end.clone(),
        weekdays: None,
        hue: None,
        block_enabled: block.enabled,
        program_enabled: true,
        empty: !block.has_pictures(),
        images: block.images.clone(),
    })
}

pub fn program_conflict(project: &Project, candidate: &Program, ignore: Option<u64>, today: NaiveDate) -> Option<Conflict> {
    let tz = project.grid.timezone;
    let offset = project.grid.offset_s;
    if !candidate.scheduled() || !alive(candidate, tz, today, offset) {
        return None;
    }
    let mine = hours_of(candidate)?;
    for other in &project.programs {
        if Some(other.id) == ignore || !other.scheduled() || !alive(other, tz, today, offset) {
            continue;
        }
        let Some(theirs) = hours_of(other) else { continue };
        if !ranges_overlap(mine, theirs) {
            continue;
        }
        if let Some(date) = shared_day(candidate, other, tz, today, offset) {
            return Some(Conflict { name: other.name.clone(), date, range: range_label(&other.start, &other.end) });
        }
    }
    for block in &project.blocks {
        if block.program.is_some() {
            continue;
        }
        if !on_mask(candidate, tz, block.date, offset) {
            continue;
        }
        let Some(theirs) = window_minutes(&block.start, &block.end) else { continue };
        if ranges_overlap(mine, theirs) {
            return Some(conflict_common(block));
        }
    }
    for block in &project.blocks {
        let Some(pid) = block.program else { continue };
        if Some(pid) == ignore {
            continue;
        }
        let Some(other) = project.program(pid) else { continue };
        if on_mask(other, tz, block.date, offset) || !alive(other, tz, today, offset) {
            continue;
        }
        if !on_mask(candidate, tz, block.date, offset) {
            continue;
        }
        let Some(theirs) = hours_of(other) else { continue };
        if ranges_overlap(mine, theirs) {
            return Some(Conflict { name: other.name.clone(), date: block.date, range: range_label(&other.start, &other.end) });
        }
    }
    None
}

fn shared_day(a: &Program, b: &Program, tz: GridTz, today: NaiveDate, offset: u32) -> Option<NaiveDate> {
    let limit = match (witness_limit(a, tz, today, offset), witness_limit(b, tz, today, offset)) {
        (Some(x), Some(y)) => x.min(y),
        _ => return None,
    };
    for back in 0..8 {
        let day = limit - TimeDelta::days(back);
        if on_mask(a, tz, day, offset) && on_mask(b, tz, day, offset) {
            return Some(day);
        }
    }
    None
}

/// A date that is still inside the program, used only to hunt for a shared weekday.
fn witness_limit(program: &Program, tz: GridTz, today: NaiveDate, offset: u32) -> Option<NaiveDate> {
    if !alive(program, tz, today, offset) {
        return None;
    }
    match program.ends {
        Some(ends) => last_air_day(program, tz, ends, offset),
        None => Some(today + TimeDelta::days(6)),
    }
}

pub fn span_conflict(
    project: &Project,
    date: NaiveDate,
    start_hm: &str,
    end_hm: &str,
    ignore_block: Option<u64>,
    ignore_program: Option<u64>,
    today: NaiveDate,
) -> Option<Conflict> {
    let mine = window_minutes(start_hm, end_hm)?;
    let tz = project.grid.timezone;
    let offset = project.grid.offset_s;
    for program in &project.programs {
        if Some(program.id) == ignore_program || !alive(program, tz, today, offset) {
            continue;
        }
        if !on_mask(program, tz, date, offset) {
            continue;
        }
        let Some(theirs) = hours_of(program) else { continue };
        if ranges_overlap(mine, theirs) {
            return Some(Conflict { name: program.name.clone(), date, range: range_label(&program.start, &program.end) });
        }
    }
    for block in &project.blocks {
        if Some(block.id) == ignore_block {
            continue;
        }
        if let Some(pid) = block.program {
            if Some(pid) == ignore_program {
                continue;
            }
            let Some(program) = project.program(pid) else { continue };
            if on_mask(program, tz, block.date, offset) || !alive(program, tz, today, offset) || block.date != date {
                continue;
            }
            let Some(theirs) = hours_of(program) else { continue };
            if ranges_overlap(mine, theirs) {
                return Some(Conflict { name: program.name.clone(), date, range: range_label(&program.start, &program.end) });
            }
        } else if block.date == date {
            let Some(theirs) = window_minutes(&block.start, &block.end) else { continue };
            if ranges_overlap(mine, theirs) {
                return Some(conflict_common(block));
            }
        }
    }
    None
}

fn conflict_common(block: &Block) -> Conflict {
    let name = if block.name.trim().is_empty() { "Block".into() } else { block.name.clone() };
    Conflict { name, date: block.date, range: range_label(&block.start, &block.end) }
}

pub fn can_place_common(project: &Project, date: NaiveDate, start_hm: &str, end_hm: &str, today: NaiveDate) -> Result<(), Conflict> {
    match span_conflict(project, date, start_hm, end_hm, None, None, today) {
        Some(c) => Err(c),
        None => Ok(()),
    }
}

pub fn can_place_program(project: &Project, date: NaiveDate, start_hm: &str, end_hm: &str, today: NaiveDate) -> Result<(), Conflict> {
    let mut weekdays = [false; 7];
    weekdays[date.weekday().num_days_from_monday() as usize] = true;
    let candidate = Program {
        id: 0,
        name: String::new(),
        enabled: true,
        weekdays,
        start: start_hm.into(),
        end: end_hm.into(),
        ..Program::default()
    };
    match program_conflict(project, &candidate, None, today) {
        Some(c) => Err(c),
        None => Ok(()),
    }
}

pub fn proposed_window(start_min: u32, interval_min: u32) -> (String, String) {
    let end = start_min + interval_min.max(1);
    let end_hm = if end >= 24 * 60 { format_hhmm(end - 24 * 60) } else { format_hhmm(end) };
    // A wrap that lands on the same minute is a full day; interval.max(1) avoids a zero-length window.
    (format_hhmm(start_min), end_hm)
}

pub fn set_weekday(
    project: &mut Project,
    id: u64,
    day: usize,
    on: bool,
    choice: Option<Keep>,
    now: DateTime<Utc>,
    today: NaiveDate,
) -> Result<(), EditError> {
    if day > 6 || project.program(id).is_none() {
        return Err(EditError::Missing);
    }
    if project.program(id).unwrap().weekdays[day] == on {
        return Ok(());
    }
    if on {
        let mut next = project.program(id).unwrap().clone();
        next.weekdays[day] = true;
        if let Some(c) = program_conflict(project, &next, Some(id), today) {
            return Err(EditError::Conflict(c));
        }
        project.program_mut(id).unwrap().weekdays[day] = true;
        return Ok(());
    }
    let Some(choice) = choice else { return Err(EditError::NeedChoice) };
    release_weekday(project, id, day, choice, now, today);
    if let Some(p) = project.program_mut(id) {
        p.weekdays[day] = false;
    }
    Ok(())
}

pub fn delete_program(project: &mut Project, id: u64, choice: Keep, now: DateTime<Utc>, today: NaiveDate) -> Result<(), EditError> {
    if project.program(id).is_none() {
        return Err(EditError::Missing);
    }
    match choice {
        Keep::Discard => {
            project.blocks.retain(|b| b.program != Some(id));
        }
        Keep::KeepFilled => {
            let ids: Vec<u64> = project.blocks.iter().filter(|b| b.program == Some(id)).map(|b| b.id).collect();
            convert_ids(project, id, &ids);
        }
        Keep::KeepThrough(until) => {
            let ids: Vec<u64> = project.blocks.iter().filter(|b| b.program == Some(id)).map(|b| b.id).collect();
            convert_ids(project, id, &ids);
            materialise_empty(project, id, None, now, today, until);
        }
    }
    project.programs.retain(|p| p.id != id);
    project.blocks.retain(|b| b.program != Some(id));
    Ok(())
}

fn release_weekday(project: &mut Project, id: u64, day: usize, choice: Keep, now: DateTime<Utc>, today: NaiveDate) {
    let ids = stored_ids(project, id, Some(day), now);
    match choice {
        Keep::Discard => project.blocks.retain(|b| !ids.contains(&b.id)),
        Keep::KeepFilled => convert_ids(project, id, &ids),
        Keep::KeepThrough(until) => {
            convert_ids(project, id, &ids);
            materialise_empty(project, id, Some(day), now, today, until);
        }
    }
}

/// Stored blocks of `id` whose written start is still at or after `now`.
fn stored_ids(project: &Project, id: u64, day: Option<usize>, now: DateTime<Utc>) -> Vec<u64> {
    let Some(program) = project.program(id).cloned() else { return Vec::new() };
    let tz = project.grid.timezone;
    project
        .blocks
        .iter()
        .filter(|b| b.program == Some(id))
        .filter(|b| day.is_none_or(|d| b.date.weekday().num_days_from_monday() as usize == d))
        .filter(|b| start_of(tz, &program, b.date).is_some_and(|s| s >= now))
        .map(|b| b.id)
        .collect()
}

fn start_of(tz: GridTz, program: &Program, date: NaiveDate) -> Option<DateTime<Utc>> {
    let (s, e) = window_minutes(&program.start, &program.end)?;
    bounds(tz, date, s, e).map(|(start, _)| start)
}

fn convert_ids(project: &mut Project, id: u64, ids: &[u64]) {
    let Some(program) = project.program(id).cloned() else { return };
    for block in &mut project.blocks {
        if block.program != Some(id) || !ids.contains(&block.id) {
            continue;
        }
        block.program = None;
        block.name = program.name.clone();
        block.start = program.start.clone();
        block.end = program.end.clone();
        block.enabled = block.enabled && program.enabled;
    }
}

fn materialise_empty(project: &mut Project, id: u64, day: Option<usize>, now: DateTime<Utc>, today: NaiveDate, until: NaiveDate) {
    let cap = future_limit(today);
    let until = until.min(cap);
    let Some(program) = project.program(id).cloned() else { return };
    let tz = project.grid.timezone;
    let start_day = grid_date(tz, now);
    let mut date = start_day;
    while date <= until {
        let wd = date.weekday().num_days_from_monday() as usize;
        let wanted = day.is_none_or(|d| d == wd) && program.weekdays.get(wd).copied().unwrap_or(false);
        let starts = start_of(tz, &program, date).is_some_and(|s| s >= now);
        let already = project.blocks.iter().any(|b| b.date == date && b.program.is_none() && b.start == program.start && b.end == program.end);
        let open = wanted && starts && !already && project.program_block(id, date).is_none();
        if open {
            let id_new = project.next_id();
            project.blocks.push(Block {
                id: id_new,
                program: None,
                date,
                name: program.name.clone(),
                start: program.start.clone(),
                end: program.end.clone(),
                enabled: program.enabled,
                images: Vec::new(),
            });
        }
        date += TimeDelta::days(1);
    }
}

/// Occurrence starts at or after `now`, in order, including empty ones. Stops at the program end.
pub fn upcoming_dates(project: &Project, program_id: u64, now: DateTime<Utc>) -> Vec<NaiveDate> {
    let Some(program) = project.program(program_id).cloned() else { return Vec::new() };
    if !program.scheduled() {
        return Vec::new();
    }
    let today = grid_date(project.grid.timezone, now);
    let mut out = Vec::new();
    let mut date = today - TimeDelta::days(1);
    let horizon = program.ends.map(|e| grid_date(project.grid.timezone, e) + TimeDelta::days(1)).unwrap_or_else(|| today + TimeDelta::days(365 * 20));
    while date <= horizon && out.len() < 20_000 {
        if let Some(occ) = program_occurrence(project, &program, date, today) {
            if occ.start >= now {
                out.push(date);
            }
        }
        date += TimeDelta::days(1);
    }
    out
}

pub fn announcements(project: &Project, current: &Occurrence, today: NaiveDate) -> Vec<Announcement> {
    let after = current.start;
    let end = if project.ax25.by_count {
        after + TimeDelta::days(400)
    } else {
        after + TimeDelta::hours(i64::from(project.ax25.next_hours.max(1)))
    };
    let from = grid_date(project.grid.timezone, after) - TimeDelta::days(1);
    let to = grid_date(project.grid.timezone, end) + TimeDelta::days(2);
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for occ in occurrences(project, from, to, today) {
        if occ.same_as(current) || occ.start <= after {
            continue;
        }
        if !project.ax25.by_count && occ.start >= end {
            continue;
        }
        if project.ax25.dedupe {
            if occ.program_id.is_some() && occ.program_id == current.program_id {
                continue;
            }
            if let Some(pid) = occ.program_id {
                if !seen.insert(pid) {
                    continue;
                }
            }
        }
        let (days, daysleft) = match occ.program_id.and_then(|id| project.program(id)) {
            Some(p) => {
                let n = match days_left(p, project.grid.timezone, today, project.grid.offset_s) {
                    DaysLeft::Value(n) => n,
                    DaysLeft::Expired => 0,
                };
                (weekdays_label(&p.weekdays), n)
            }
            None => (String::new(), 0),
        };
        out.push(Announcement {
            name: occ.name.clone(),
            range: occ.range(),
            days,
            daysleft,
            at: occ.start,
            program_id: occ.program_id,
        });
        if project.ax25.by_count && out.len() >= project.ax25.next_count.max(0) as usize {
            break;
        }
    }
    out
}

pub fn weekdays_label(days: &[bool; 7]) -> String {
    const NAMES: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
    days.iter().zip(NAMES).filter(|(on, _)| **on).map(|(_, n)| n).collect::<Vec<_>>().join(",")
}

pub fn cards(project: &Project, zoom: Zoom, anchor: NaiveDate, today: NaiveDate) -> Result<Vec<Card>, &'static str> {
    let (from, to) = match zoom {
        Zoom::Month => month_span(anchor),
        Zoom::Week => week_span(anchor),
        Zoom::Day => (anchor, anchor + TimeDelta::days(1)),
    };
    if from > future_limit(today) {
        return Err("future");
    }
    let occ = occurrences(project, from, to, today);
    let mut cards = Vec::new();
    let mut stacks: std::collections::HashMap<NaiveDate, i32> = std::collections::HashMap::new();
    for occ in occ {
        let (opacity, gray) = look(&occ);
        let rgb = match occ.hue {
            Some(h) if !gray => hue_rgb(h),
            _ => COMMON_RGB,
        };
        let base = Card {
            name: occ.name.clone(),
            date: occ.date,
            start_hm: occ.start_hm.clone(),
            end_hm: occ.end_hm.clone(),
            start_min: 0,
            end_min: 0,
            col: 0,
            stack: 0,
            y: 0.0,
            h: 0.0,
            rgb,
            opacity,
            gray,
            empty: occ.empty,
            program_id: occ.program_id.map(|id| id as i64).unwrap_or(-1),
            block_id: occ.block_id.map(|id| id as i64).unwrap_or(-1),
        };
        match zoom {
            Zoom::Month => {
                let stack = stacks.entry(occ.date).or_insert(0);
                let mut card = base;
                card.col = occ.date.weekday().num_days_from_monday() as i32;
                card.stack = *stack;
                *stack += 1;
                cards.push(card);
            }
            Zoom::Week | Zoom::Day => {
                push_timed(&mut cards, base, &occ, from, zoom);
            }
        }
    }
    Ok(cards)
}

fn look(occ: &Occurrence) -> (f32, bool) {
    let gray = !occ.program_enabled;
    let off = !occ.block_enabled || !occ.program_enabled;
    let opacity = if occ.empty { 0.32 } else if off { 0.58 } else { 1.0 };
    (opacity, gray)
}

fn push_timed(cards: &mut Vec<Card>, base: Card, occ: &Occurrence, window_start: NaiveDate, zoom: Zoom) {
    let Some((s, e)) = window_minutes(&occ.start_hm, &occ.end_hm) else { return };
    let pieces = if e <= 24 * 60 { vec![(occ.date, s, e)] } else { vec![(occ.date, s, 24 * 60), (occ.date + TimeDelta::days(1), 0, e - 24 * 60)] };
    for (date, start, end) in pieces {
        if date < window_start {
            continue;
        }
        let col = match zoom {
            Zoom::Day => 0,
            _ => (date - window_start).num_days() as i32,
        };
        if !(0..7).contains(&col) && zoom == Zoom::Week {
            continue;
        }
        let mut card = base.clone();
        card.date = date;
        card.col = col;
        card.start_min = start as i32;
        card.end_min = end as i32;
        card.y = start as f32 / (24.0 * 60.0);
        card.h = (end.saturating_sub(start)) as f32 / (24.0 * 60.0);
        cards.push(card);
    }
}

fn month_span(anchor: NaiveDate) -> (NaiveDate, NaiveDate) {
    let first = anchor.with_day(1).unwrap_or(anchor);
    let next = if first.month() == 12 {
        NaiveDate::from_ymd_opt(first.year() + 1, 1, 1)
    } else {
        NaiveDate::from_ymd_opt(first.year(), first.month() + 1, 1)
    };
    (first, next.unwrap_or(first + TimeDelta::days(31)))
}

fn week_span(anchor: NaiveDate) -> (NaiveDate, NaiveDate) {
    let start = anchor - TimeDelta::days(i64::from(anchor.weekday().num_days_from_monday()));
    (start, start + TimeDelta::days(7))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::{BlockImage, ImageRole};

    fn utc(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }
    fn day(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }
    fn program(id: u64, name: &str, days: [bool; 7], start: &str, end: &str) -> Program {
        Program { id, name: name.into(), weekdays: days, start: start.into(), end: end.into(), ..Program::default() }
    }

    #[test]
    fn five_turns_stop_before_the_end_and_a_sixth_needs_room() {
        let start = utc("2026-10-27T14:00:00Z");
        let end = utc("2026-10-27T14:30:00Z");
        let times: Vec<_> = turn_times(start, end, 6, 0).into_iter().map(|t| t.format("%H:%M").to_string()).collect();
        assert_eq!(times, ["14:00", "14:06", "14:12", "14:18", "14:24"]);
        let wider = utc("2026-10-27T14:33:00Z");
        let times: Vec<_> = turn_times(start, wider, 6, 0).into_iter().map(|t| t.format("%H:%M").to_string()).collect();
        assert_eq!(times, ["14:00", "14:06", "14:12", "14:18", "14:24", "14:30"]);
    }

    #[test]
    fn offset_moves_starts_only() {
        let start = utc("2026-10-27T14:00:00Z");
        let end = utc("2026-10-27T14:30:00Z");
        let times: Vec<_> = turn_times(start, end, 6, 90).into_iter().map(|t| t.format("%H:%M:%S").to_string()).collect();
        assert_eq!(times[0], "14:01:30");
        assert!(times.iter().all(|t| t.as_str() < "14:30:00"));
        assert_ne!(*times.last().unwrap(), "14:30:00");
    }

    #[test]
    fn midnight_block_includes_the_turn_after_midnight() {
        let start = utc("2026-10-27T23:50:00Z");
        let end = utc("2026-10-28T00:10:00Z");
        let times: Vec<_> = turn_times(start, end, 10, 0).into_iter().map(|t| t.to_rfc3339()).collect();
        assert_eq!(times, ["2026-10-27T23:50:00+00:00", "2026-10-28T00:00:00+00:00"]);
    }

    #[test]
    fn dst_gap_is_skipped_and_the_overlap_uses_the_earlier_instant() {
        use chrono_tz::America::New_York;
        let gap = NaiveDate::from_ymd_opt(2026, 3, 8).unwrap().and_hms_opt(2, 30, 0).unwrap();
        assert!(map_local(&New_York, gap).is_none());
        let fold = NaiveDate::from_ymd_opt(2026, 11, 1).unwrap().and_hms_opt(1, 30, 0).unwrap();
        let t = map_local(&New_York, fold).unwrap();
        // 01:30 EDT (UTC−4) is the earlier of the two 01:30s.
        assert_eq!(t, utc("2026-11-01T05:30:00Z"));
    }

    #[test]
    fn touching_edges_do_not_overlap() {
        let a = window_minutes("14:00", "14:30").unwrap();
        let b = window_minutes("14:30", "15:00").unwrap();
        assert!(!ranges_overlap(a, b));
        let c = window_minutes("14:00", "14:31").unwrap();
        assert!(ranges_overlap(c, b));
        let night = window_minutes("23:00", "01:00").unwrap();
        let morning = window_minutes("00:30", "02:00").unwrap();
        assert!(ranges_overlap(night, morning));
        let before = window_minutes("22:00", "23:00").unwrap();
        assert!(!ranges_overlap(before, night));
    }

    fn channel(programs: Vec<Program>, blocks: Vec<Block>) -> Project {
        Project { programs, blocks, ..Project::default() }
    }

    #[test]
    fn overlap_names_the_card_and_an_edit_is_all_or_nothing() {
        let mut days = [false; 7];
        days[0] = true;
        let mut p = channel(vec![program(1, "Noticias", days, "14:00", "14:30")], vec![]);
        let today = day(2026, 10, 26); // Monday
        let err = can_place_program(&p, today, "14:15", "14:45", today).unwrap_err();
        assert!(err.text(false).contains("Noticias"));
        assert!(err.text(true).contains("Noticias"));
        // Turning Tuesday on does not touch Monday, so it is free.
        assert!(set_weekday(&mut p, 1, 1, true, None, utc("2026-10-26T00:00:00Z"), today).is_ok());
        // A second Monday program is refused and the new program is not inserted by the check.
        assert!(can_place_common(&p, today, "14:00", "14:10", today).is_err());
        p.blocks.push(Block {
            id: 9,
            name: "Vacio".into(),
            date: today,
            start: "18:00".into(),
            end: "18:30".into(),
            enabled: false,
            ..Block::default()
        });
        assert!(can_place_common(&p, today, "18:15", "18:40", today).is_err(), "disabled empty still occupies");
    }

    #[test]
    fn an_expired_program_does_not_occupy() {
        let mut old = program(1, "Viejo", [true; 7], "14:00", "14:30");
        old.ends = Some(utc("2026-10-01T23:00:00Z"));
        let p = channel(vec![old], vec![]);
        let today = day(2026, 10, 26);
        assert!(can_place_program(&p, today, "14:00", "14:30", today).is_ok());
        assert!(matches!(days_left(&p.programs[0], GridTz::Utc, today, 0), DaysLeft::Expired));
    }

    #[test]
    fn days_left_counts_today_and_saturates() {
        let mut p = program(1, "N", [true; 7], "14:00", "15:00");
        let today = day(2026, 10, 27);
        assert_eq!(days_left(&p, GridTz::Utc, today, 0), DaysLeft::Value(0));
        p.ends = Some(utc("2026-10-27T23:00:00Z"));
        assert_eq!(days_left(&p, GridTz::Utc, today, 0), DaysLeft::Value(1));
        p.ends = Some(utc("2026-10-28T23:00:00Z"));
        assert_eq!(days_left(&p, GridTz::Utc, today, 0), DaysLeft::Value(2));
        p.ends = Some(utc("2026-10-26T23:00:00Z"));
        assert_eq!(days_left(&p, GridTz::Utc, today, 0), DaysLeft::Expired);
        p.ends = Some(utc("2027-02-02T23:00:00Z")); // 2026-10-27 + 98 days
        assert_eq!(days_left(&p, GridTz::Utc, today, 0), DaysLeft::Value(99));
        p.ends = Some(utc("2027-02-03T23:00:00Z")); // +99 days → more than 99
        assert_eq!(days_left(&p, GridTz::Utc, today, 0), DaysLeft::Value(0));
        p.ends = Some(utc("2026-10-27T14:00:00Z")); // the turn would start exactly at the end
        assert_eq!(days_left(&p, GridTz::Utc, today, 0), DaysLeft::Expired);
    }

    #[test]
    fn deduped_announcements_skip_the_program_on_air() {
        let mut days = [false; 7];
        days[0] = true;
        days[1] = true;
        let news = program(1, "Noticias", days, "14:00", "14:30");
        let mut weekend = [false; 7];
        weekend[5] = true;
        weekend[6] = true;
        let weather = program(2, "Clima", weekend, "18:00", "18:15");
        let mut p = channel(vec![news, weather], vec![]);
        p.ax25.by_count = true;
        p.ax25.next_count = 8;
        p.ax25.dedupe = true;
        let today = day(2026, 10, 26); // Monday
        let occ = occurrences(&p, today, today + TimeDelta::days(1), today);
        let current = occ.iter().find(|o| o.program_id == Some(1)).unwrap();
        let next = announcements(&p, current, today);
        assert!(next.iter().all(|a| a.program_id != Some(1)));
        assert_eq!(next.iter().filter(|a| a.program_id == Some(2)).count(), 1);
        p.ax25.dedupe = false;
        let next = announcements(&p, current, today);
        assert!(next.iter().any(|a| a.program_id == Some(1)), "without dedupe the next block of this program stays");
    }

    #[test]
    fn unchecking_a_day_waits_for_a_choice_and_keeps_the_past() {
        let mut days = [false; 7];
        days[0] = true;
        let mut p = channel(vec![program(1, "Noticias", days, "14:00", "14:30")], vec![]);
        let today = day(2026, 10, 26);
        let now = utc("2026-10-26T12:00:00Z");
        p.blocks.push(Block {
            id: 4,
            program: Some(1),
            date: day(2026, 10, 19), // previous Monday, already started
            images: vec![BlockImage::normal(5, "vieja.png")],
            ..Block::default()
        });
        p.blocks.push(Block {
            id: 6,
            program: Some(1),
            date: today,
            images: vec![BlockImage::normal(7, "hoy.png")],
            ..Block::default()
        });
        assert!(matches!(set_weekday(&mut p, 1, 0, false, None, now, today), Err(EditError::NeedChoice)));
        assert!(p.programs[0].weekdays[0]);
        set_weekday(&mut p, 1, 0, false, Some(Keep::KeepFilled), now, today).unwrap();
        assert!(!p.programs[0].weekdays[0]);
        let past = p.blocks.iter().find(|b| b.id == 4).unwrap();
        assert_eq!(past.program, Some(1), "past blocks stay on the program");
        let converted = p.blocks.iter().find(|b| b.date == today).unwrap();
        assert!(converted.program.is_none());
        assert_eq!(converted.start, "14:00");
        assert!(converted.images.iter().any(|i| matches!(i.role, ImageRole::Normal)));
    }

    #[test]
    fn navigation_stops_three_years_ahead_and_not_behind() {
        let p = Project::default();
        let today = day(2026, 10, 27);
        assert!(cards(&p, Zoom::Month, today - TimeDelta::days(800), today).is_ok());
        assert!(cards(&p, Zoom::Month, future_limit(today) + TimeDelta::days(40), today).is_err());
    }

    #[test]
    fn empty_program_cards_are_the_most_transparent() {
        let mut days = [false; 7];
        days[1] = true; // Tuesday
        let mut prog = program(1, "Hueco", days, "10:00", "11:00");
        prog.enabled = false;
        let p = channel(vec![prog], vec![]);
        let today = day(2026, 10, 27);
        let cards = cards(&p, Zoom::Day, today, today).unwrap();
        assert_eq!(cards.len(), 1);
        assert!(cards[0].empty);
        assert!(cards[0].gray);
        assert!(cards[0].opacity < 0.5);
    }
}
