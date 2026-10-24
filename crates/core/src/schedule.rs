//! The programming grid in time: which transmissions happen when, and which program
//! goes on air next.

use chrono::{DateTime, Datelike, Duration, Local, NaiveDate, NaiveDateTime, TimeZone, Utc};

use crate::project::{Grid, GridTz, Program, ProgramImage, Project, Rotation, Slot, parse_hhmm};

#[derive(Clone, Debug, PartialEq)]
pub struct Occurrence {
    pub at: DateTime<Utc>,
    pub slot_id: u64,
    pub slot_name: String,
    pub program: Option<u64>,
    pub freq_hz: u64,
}

fn to_utc(tz: GridTz, naive: NaiveDateTime) -> Option<DateTime<Utc>> {
    match tz {
        GridTz::Utc => Some(Utc.from_utc_datetime(&naive)),
        // DST gaps have no local time; overlaps take the first.
        GridTz::Local => Local.from_local_datetime(&naive).earliest().map(|t| t.with_timezone(&Utc)),
    }
}

fn day_of(tz: GridTz, t: DateTime<Utc>) -> NaiveDate {
    match tz {
        GridTz::Utc => t.date_naive(),
        GridTz::Local => t.with_timezone(&Local).date_naive(),
    }
}

/// Transmission start times of `slot` for the grid day `day`.
fn slot_times(slot: &Slot, tz: GridTz, day: NaiveDate) -> Vec<DateTime<Utc>> {
    if !slot.enabled || !slot.days[day.weekday().num_days_from_monday() as usize] {
        return Vec::new();
    }
    let Some(start) = parse_hhmm(&slot.start) else { return Vec::new() };
    let midnight = day.and_hms_opt(0, 0, 0).expect("valid midnight");
    let mut minutes = vec![start as i64];
    if let (Some(end), true) = (parse_hhmm(&slot.end), slot.every_min > 0) {
        let mut end = end as i64;
        if end <= start as i64 {
            end += 24 * 60;
        }
        let mut m = start as i64 + slot.every_min as i64;
        while m < end {
            minutes.push(m);
            m += slot.every_min as i64;
        }
    }
    minutes.into_iter().filter_map(|m| to_utc(tz, midnight + Duration::minutes(m))).collect()
}

/// The channel grid: schedule lives here, not on each program.
pub fn airing_slots(project: &Project) -> Vec<Slot> {
    project.grid.slots.clone()
}

fn grid_with(project: &Project) -> Grid {
    project.grid.clone()
}

/// Every transmission of the grid in [from, to), sorted.
pub fn occurrences(grid: &Grid, from: DateTime<Utc>, to: DateTime<Utc>) -> Vec<Occurrence> {
    let mut out = Vec::new();
    let offset = Duration::seconds(grid.offset_s as i64);
    // Start a day early: a block that began yesterday may run past midnight,
    // and a late offset can push yesterday's last start into today.
    let mut day = day_of(grid.timezone, from) - Duration::days(1);
    let last = day_of(grid.timezone, to);
    while day <= last {
        for slot in &grid.slots {
            for at in slot_times(slot, grid.timezone, day) {
                let at = at + offset;
                if at >= from && at < to {
                    out.push(Occurrence { at, slot_id: slot.id, slot_name: slot.name.clone(), program: slot.program, freq_hz: slot.freq_hz });
                }
            }
        }
        day += Duration::days(1);
    }
    out.sort_by_key(|o| o.at);
    out.dedup_by_key(|o| o.at);
    out
}

pub fn project_occurrences(project: &Project, from: DateTime<Utc>, to: DateTime<Utc>) -> Vec<Occurrence> {
    occurrences(&grid_with(project), from, to)
}

/// The first transmission at or after `from`, looking up to 8 days ahead.
pub fn next_occurrence(grid: &Grid, from: DateTime<Utc>) -> Option<Occurrence> {
    occurrences(grid, from, from + Duration::days(8)).into_iter().next()
}

pub fn next_project_occurrence(project: &Project, from: DateTime<Utc>) -> Option<Occurrence> {
    next_occurrence(&grid_with(project), from)
}

/// Picks programs according to the rotation, never repeating one until all have aired.
#[derive(Debug, Default)]
pub struct Rotator {
    bag: Vec<u64>,
    last: Option<u64>,
    seed: u64,
}

impl Rotator {
    pub fn new() -> Rotator {
        let seed = Utc::now().timestamp_nanos_opt().unwrap_or(1) as u64 | 1;
        Rotator { bag: Vec::new(), last: None, seed }
    }

    fn rand(&mut self) -> u64 {
        // xorshift64*: plenty for shuffling a playlist.
        self.seed ^= self.seed >> 12;
        self.seed ^= self.seed << 25;
        self.seed ^= self.seed >> 27;
        self.seed.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    pub fn next(&mut self, programs: &[Program], rotation: Rotation) -> Option<u64> {
        let ids: Vec<u64> = programs.iter().filter(|p| p.enabled && p.image_count() > 0).map(|p| p.id).collect();
        if ids.is_empty() {
            return None;
        }
        let pick = match rotation {
            Rotation::Sequential => {
                let i = self.last.and_then(|l| ids.iter().position(|&id| id == l)).map_or(0, |i| (i + 1) % ids.len());
                ids[i]
            }
            Rotation::Shuffle => {
                self.bag.retain(|id| ids.contains(id));
                if self.bag.is_empty() {
                    self.bag = ids.clone();
                    for i in (1..self.bag.len()).rev() {
                        let j = (self.rand() % (i as u64 + 1)) as usize;
                        self.bag.swap(i, j);
                    }
                    // Avoid airing the same program twice in a row across bags.
                    let n = self.bag.len();
                    if n > 1 && self.bag.last() == self.last.as_ref() {
                        self.bag.swap(0, n - 1);
                    }
                }
                self.bag.pop().expect("bag refilled")
            }
        };
        self.last = Some(pick);
        Some(pick)
    }

    /// The program `next` would return, without consuming it.
    pub fn peek(&mut self, programs: &[Program], rotation: Rotation) -> Option<u64> {
        let (bag, last) = (self.bag.clone(), self.last);
        let pick = self.next(programs, rotation);
        if let Some(id) = pick {
            // Keep a freshly shuffled bag (so the peeked program really is next), but
            // put the pick back.
            if rotation == Rotation::Shuffle && bag.is_empty() {
                self.bag.push(id);
            } else {
                self.bag = bag;
            }
        }
        self.last = last;
        pick
    }

    /// Records a program aired out of rotation (triggers, manual) so sequential
    /// rotation continues after it.
    pub fn aired(&mut self, id: u64) {
        self.last = Some(id);
        self.bag.retain(|&b| b != id);
    }
}

/// Picks the next picture of a program, same rules as the channel rotator.
#[derive(Debug, Default)]
pub struct ImageRotator {
    inner: Rotator,
}

impl ImageRotator {
    pub fn next(&mut self, images: &[ProgramImage], rotation: Rotation) -> Option<usize> {
        let ids: Vec<u64> = images.iter().enumerate().filter(|(_, i)| i.enabled && !i.path.trim().is_empty()).map(|(i, _)| i as u64).collect();
        if ids.is_empty() {
            return None;
        }
        let fake: Vec<Program> = ids.iter().map(|&id| Program { id, enabled: true, images: vec![ProgramImage::new("x")], ..Default::default() }).collect();
        self.inner.next(&fake, rotation).map(|id| id as usize)
    }

    pub fn peek(&mut self, images: &[ProgramImage], rotation: Rotation) -> Option<usize> {
        let ids: Vec<u64> = images.iter().enumerate().filter(|(_, i)| i.enabled && !i.path.trim().is_empty()).map(|(i, _)| i as u64).collect();
        if ids.is_empty() {
            return None;
        }
        let fake: Vec<Program> = ids.iter().map(|&id| Program { id, enabled: true, images: vec![ProgramImage::new("x")], ..Default::default() }).collect();
        self.inner.peek(&fake, rotation).map(|id| id as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utc(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    fn grid(slots: Vec<Slot>) -> Grid {
        Grid { timezone: GridTz::Utc, slots, ..Default::default() }
    }

    #[test]
    fn repeating_block() {
        let g = grid(vec![Slot { id: 1, start: "21:00".into(), end: "21:20".into(), every_min: 5, ..Default::default() }]);
        let occ = occurrences(&g, utc("2026-10-23T00:00:00Z"), utc("2026-10-24T00:00:00Z"));
        let times: Vec<String> = occ.iter().map(|o| o.at.format("%H:%M").to_string()).collect();
        assert_eq!(times, ["21:00", "21:05", "21:10", "21:15"]);
    }

    #[test]
    fn block_crossing_midnight() {
        // 23:50 → 00:10 every 10 min airs at 23:50 and 00:00 (00:10 is the end).
        let g = grid(vec![Slot { id: 1, start: "23:50".into(), end: "00:10".into(), every_min: 10, ..Default::default() }]);
        assert_eq!(next_occurrence(&g, utc("2026-10-24T00:00:00Z")).unwrap().at, utc("2026-10-24T00:00:00Z"));
        assert_eq!(next_occurrence(&g, utc("2026-10-24T00:00:01Z")).unwrap().at, utc("2026-10-24T23:50:00Z"));
    }

    #[test]
    fn weekdays_and_single_shot() {
        let mut days = [false; 7];
        days[4] = true; // Friday
        let g = grid(vec![Slot { id: 1, days, start: "12:00".into(), end: String::new(), ..Default::default() }]);
        // 2026-10-23 is a Friday.
        assert_eq!(next_occurrence(&g, utc("2026-10-22T13:00:00Z")).unwrap().at, utc("2026-10-23T12:00:00Z"));
        assert_eq!(next_occurrence(&g, utc("2026-10-23T12:00:01Z")).unwrap().at, utc("2026-10-30T12:00:00Z"));
    }

    #[test]
    fn shuffle_covers_everything_before_repeating() {
        let programs: Vec<Program> = (1..=5)
            .map(|id| Program { id, images: vec![ProgramImage::new(format!("{id}.png"))], ..Default::default() })
            .collect();
        let mut r = Rotator::new();
        for _ in 0..4 {
            let mut round: Vec<u64> = (0..5).map(|_| r.next(&programs, Rotation::Shuffle).unwrap()).collect();
            round.sort();
            assert_eq!(round, [1, 2, 3, 4, 5]);
        }
        let peeked = r.peek(&programs, Rotation::Shuffle);
        assert_eq!(peeked, r.peek(&programs, Rotation::Shuffle));
        assert_eq!(peeked, r.next(&programs, Rotation::Shuffle));
        let seq: Vec<u64> = (0..6).map(|_| r.next(&programs, Rotation::Sequential).unwrap()).collect();
        assert_eq!(seq.len(), 6);
        assert_eq!(seq[1], seq[0] % 5 + 1);
    }

    #[test]
    fn project_offset_shifts_every_start() {
        // Shared frequency: the other channel occupies 14:00–14:03, this one
        // keeps the 14:00 block and keys 3 minutes later.
        let mut g = grid(vec![Slot { id: 1, start: "14:00".into(), end: String::new(), ..Default::default() }]);
        g.offset_s = 3 * 60;
        assert_eq!(next_occurrence(&g, utc("2026-10-24T00:00:00Z")).unwrap().at, utc("2026-10-24T14:03:00Z"));
        g.offset_s = 3 * 60 + 15;
        assert_eq!(next_occurrence(&g, utc("2026-10-24T00:00:00Z")).unwrap().at, utc("2026-10-24T14:03:15Z"));
        // Yesterday 23:59 + 90 s is today 00:00:30.
        g.offset_s = 90;
        g.slots[0].start = "23:59".into();
        assert_eq!(next_occurrence(&g, utc("2026-10-24T00:00:00Z")).unwrap().at, utc("2026-10-24T00:00:30Z"));
        assert_eq!(next_occurrence(&g, utc("2026-10-24T00:00:31Z")).unwrap().at, utc("2026-10-25T00:00:30Z"));
    }
}
