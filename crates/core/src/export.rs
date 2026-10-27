//! Export a time range as audio files. The sum of the files is the range:
//! silence is part of the recording, and a file's name says when to press play.

use std::fs::File;
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use chrono::{DateTime, TimeDelta, Utc};

use crate::air::{self, PieceKind};
use crate::compose::Composer;
use crate::dynamic::ApiCache;
use crate::encoder::ScanImage;
#[cfg(test)]
use crate::modes::Mode;
use crate::project::{GridTz, Project};
use crate::schedule::{grid_date, occurrences};
use crate::synth::{SampleSource, Seg, Synth};

pub const SAMPLE_RATES: [u32; 4] = [11_025, 22_050, 44_100, 48_000];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudioFormat {
    Flac,
    Mp3,
    Aac,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    Whole,
    TxSilence,
    Pieces,
    /// Slice the finished timeline every this many seconds. The last slice is shorter.
    Every(u32),
    PerBlock,
}

#[derive(Clone, Debug)]
pub struct ExportRequest {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub shape: Shape,
    pub format: AudioFormat,
    pub rate: u32,
    pub lead_ms: u32,
    pub tail_ms: u32,
}

/// Why MP3 or AAC cannot be chosen. `None` for FLAC, which is written by this binary.
pub fn format_block(format: AudioFormat, spanish: bool) -> Option<String> {
    let (en, es) = match format {
        AudioFormat::Flac => return None,
        AudioFormat::Mp3 => (
            "MP3 is not in this build. The usual encoder is LGPL, and linking it here would not match the license of the program.",
            "MP3 no está en esta compilación. El codificador habitual es LGPL y enlazarlo no encaja con la licencia del programa.",
        ),
        AudioFormat::Aac => (
            "AAC is not in this build. There is no encoder we can ship without an external program.",
            "AAC no está en esta compilación. No hay un codificador que podamos incluir sin un programa externo.",
        ),
    };
    Some(if spanish { es } else { en }.to_string())
}

/// Shown every time the user leaves FLAC at 44100. Both languages.
pub fn quality_warning(format: AudioFormat, rate: u32, spanish: bool) -> Option<String> {
    if matches!(format, AudioFormat::Flac) && rate == 44_100 {
        return None;
    }
    Some(if spanish {
        "Cualquier ajuste que no sea FLAC a 44100 introduce cambios visuales leves en la imagen decodificada. No garantiza una transmisión perfecta, pero sigue siendo usable.".into()
    } else {
        "Any setting other than FLAC at 44100 introduces slight visual changes in the decoded image. It does not guarantee a perfect transmission, but it is still usable.".into()
    })
}

#[derive(Clone, Debug)]
struct Tone {
    piece: air::Piece,
    call: String,
    wpm: f32,
    tone: f32,
    skip_us: i64,
}

#[derive(Clone, Debug)]
struct Span {
    start: DateTime<Utc>,
    dur_us: i64,
    tone: Option<Tone>,
    /// Set when this silence sits between two turns of the same block.
    block_key: Option<String>,
    block_name: String,
    block_end: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug)]
pub struct Clip {
    pub name: String,
    pub start: DateTime<Utc>,
    pub dur_us: i64,
    spans: Vec<Span>,
}

pub fn plan(project: &Project, req: &ExportRequest) -> anyhow::Result<Vec<Clip>> {
    anyhow::ensure!(req.end > req.start, "el rango está vacío");
    anyhow::ensure!(SAMPLE_RATES.contains(&req.rate), "frecuencia de muestreo no admitida");
    if let Shape::Every(secs) = req.shape {
        anyhow::ensure!(secs > 0, "el trozo tiene que durar al menos un segundo");
    }
    let spans = audible(project, req);
    let grouped = match req.shape {
        Shape::Whole => vec![group(&spans, "entero")],
        Shape::TxSilence => group_runs(&spans, true),
        Shape::Pieces => piece_runs(&spans),
        Shape::Every(secs) => slice_every(&spans, i64::from(secs) * 1_000_000),
        Shape::PerBlock => block_runs(tag_internal(spans)),
    };
    Ok(name_clips(grouped, project.grid.timezone, ext(req.format), !matches!(req.shape, Shape::Whole)))
}

fn ext(format: AudioFormat) -> &'static str {
    match format {
        AudioFormat::Flac => "flac",
        AudioFormat::Mp3 => "mp3",
        AudioFormat::Aac => "m4a",
    }
}

fn audible(project: &Project, req: &ExportRequest) -> Vec<Span> {
    let today = grid_date(project.grid.timezone, req.start);
    let from = grid_date(project.grid.timezone, req.start) - TimeDelta::days(1);
    let to = grid_date(project.grid.timezone, req.end) + TimeDelta::days(1);
    let mut raw: Vec<Span> = Vec::new();
    for occ in occurrences(project, from, to, today) {
        if !occ.transmits() {
            continue;
        }
        let turns = air::plan_occurrence(project, &occ, today, req.lead_ms, req.tail_ms, 1, req.start);
        let block_key = format!("{}:{}:{}", occ.program_id.unwrap_or(0), occ.block_id.unwrap_or(0), occ.date);
        for turn in turns.into_iter().filter(|t| t.airs) {
            let mut at = turn.at + TimeDelta::milliseconds(i64::from(req.lead_ms));
            for piece in &turn.pieces {
                let dur_us = (piece.ms * 1000.0).round() as i64;
                let end = at + TimeDelta::microseconds(dur_us.max(0));
                let s = at.max(req.start);
                let e = end.min(req.end);
                if e > s {
                    raw.push(Span {
                        start: s,
                        dur_us: (e - s).num_microseconds().unwrap_or(0),
                        tone: Some(Tone {
                            piece: piece.clone(),
                            call: turn.call.clone(),
                            wpm: turn.cw_wpm,
                            tone: turn.cw_tone,
                            skip_us: (s - at).num_microseconds().unwrap_or(0),
                        }),
                        block_key: Some(block_key.clone()),
                        block_name: occ.name.clone(),
                        block_end: Some(occ.end),
                    });
                }
                at = end;
            }
        }
    }
    raw.sort_by_key(|s| s.start);
    let mut out = Vec::new();
    let mut cursor = req.start;
    for span in raw {
        if span.start > cursor {
            out.push(silence(cursor, span.start));
        }
        if span.start >= cursor && span.dur_us > 0 {
            cursor = span.start + TimeDelta::microseconds(span.dur_us);
            out.push(span);
        }
    }
    if cursor < req.end {
        out.push(silence(cursor, req.end));
    }
    merge_silence(out)
}

fn silence(start: DateTime<Utc>, end: DateTime<Utc>) -> Span {
    Span {
        start,
        dur_us: (end - start).num_microseconds().unwrap_or(0),
        tone: None,
        block_key: None,
        block_name: String::new(),
        block_end: None,
    }
}

fn merge_silence(spans: Vec<Span>) -> Vec<Span> {
    let mut out: Vec<Span> = Vec::new();
    for span in spans {
        if span.dur_us <= 0 {
            continue;
        }
        if span.tone.is_none() {
            if let Some(prev) = out.last_mut() {
                if prev.tone.is_none() {
                    prev.dur_us += span.dur_us;
                    continue;
                }
            }
        }
        out.push(span);
    }
    out
}

fn tag_internal(spans: Vec<Span>) -> Vec<Span> {
    let mut out = Vec::new();
    for (i, span) in spans.iter().enumerate() {
        if span.tone.is_some() {
            out.push(span.clone());
            continue;
        }
        let prev = spans[..i].iter().rev().find(|s| s.tone.is_some());
        let next_key = spans[i + 1..].iter().find(|s| s.tone.is_some()).and_then(|s| s.block_key.clone());
        let prev_key = prev.and_then(|s| s.block_key.clone());
        if prev_key.is_some() && prev_key == next_key {
            let mut owned = span.clone();
            owned.block_key = prev_key;
            owned.block_name = prev.map(|s| s.block_name.clone()).unwrap_or_default();
            out.push(owned);
            continue;
        }
        let Some(end) = prev.and_then(|s| s.block_end) else {
            out.push(span.clone());
            continue;
        };
        if span.start >= end {
            out.push(span.clone());
            continue;
        }
        let inside_us = (end - span.start).num_microseconds().unwrap_or(0).min(span.dur_us);
        let mut inside = span.clone();
        inside.dur_us = inside_us;
        inside.block_key = prev_key;
        inside.block_name = prev.map(|s| s.block_name.clone()).unwrap_or_default();
        out.push(inside);
        if inside_us < span.dur_us {
            let mut rest = span.clone();
            rest.start += TimeDelta::microseconds(inside_us);
            rest.dur_us -= inside_us;
            out.push(rest);
        }
    }
    out
}

struct Draft {
    label: String,
    start: DateTime<Utc>,
    dur_us: i64,
    spans: Vec<Span>,
}

fn group(spans: &[Span], label: &str) -> Draft {
    let start = spans.first().map(|s| s.start).unwrap_or_else(Utc::now);
    Draft { label: label.into(), start, dur_us: spans.iter().map(|s| s.dur_us).sum(), spans: spans.to_vec() }
}

fn group_runs(spans: &[Span], tx: bool) -> Vec<Draft> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < spans.len() {
        let audio = spans[i].tone.is_some();
        let mut j = i + 1;
        if !audio {
            while j < spans.len() && spans[j].tone.is_none() {
                j += 1;
            }
        } else if tx {
            while j < spans.len() && spans[j].tone.is_some() {
                j += 1;
            }
        }
        let label = if audio { "tx" } else { "silencio" };
        out.push(group(&spans[i..j], label));
        i = j;
    }
    out
}

fn piece_runs(spans: &[Span]) -> Vec<Draft> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < spans.len() {
        if spans[i].tone.is_some() {
            let label = spans[i].tone.as_ref().map(|t| piece_label(&t.piece)).unwrap_or("tx");
            out.push(group(&spans[i..i + 1], label));
            i += 1;
        } else {
            let mut j = i + 1;
            while j < spans.len() && spans[j].tone.is_none() {
                j += 1;
            }
            out.push(group(&spans[i..j], "silencio"));
            i = j;
        }
    }
    out
}

fn piece_label(piece: &air::Piece) -> &'static str {
    match piece.kind {
        PieceKind::Vox => "vox",
        PieceKind::Cw => "cw",
        PieceKind::Ax25 | PieceKind::Ax25Mode => "ax25",
        PieceKind::Vis => "vis",
        PieceKind::Welcome => "bienvenida",
        PieceKind::Bumper => "bumper",
        PieceKind::Image => "img",
        PieceKind::Bye => "despedida",
        PieceKind::Dtmf => "dtmf",
    }
}

fn block_runs(spans: Vec<Span>) -> Vec<Draft> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < spans.len() {
        if let Some(key) = spans[i].block_key.clone() {
            let mut j = i + 1;
            while j < spans.len() && spans[j].block_key.as_deref() == Some(key.as_str()) {
                j += 1;
            }
            let name = sanitize(&spans[i].block_name);
            out.push(group(&spans[i..j], &format!("bloque_{name}")));
            i = j;
        } else {
            let mut j = i + 1;
            while j < spans.len() && spans[j].block_key.is_none() {
                j += 1;
            }
            out.push(group(&spans[i..j], "silencio"));
            i = j;
        }
    }
    out
}

fn slice_every(spans: &[Span], slice_us: i64) -> Vec<Draft> {
    let mut rest = spans.to_vec();
    let mut out = Vec::new();
    while !rest.is_empty() {
        let mut acc = Vec::new();
        let mut need = slice_us;
        while need > 0 && !rest.is_empty() {
            if rest[0].dur_us <= need {
                need -= rest[0].dur_us;
                acc.push(rest.remove(0));
            } else {
                let (head, tail) = cut(&rest[0], need);
                acc.push(head);
                rest[0] = tail;
                need = 0;
            }
        }
        out.push(group(&acc, "trozo"));
    }
    out
}

fn cut(span: &Span, us: i64) -> (Span, Span) {
    let mut head = span.clone();
    head.dur_us = us;
    let mut tail = span.clone();
    tail.start += TimeDelta::microseconds(us);
    tail.dur_us -= us;
    if let Some(tone) = tail.tone.as_mut() {
        tone.skip_us += us;
    }
    (head, tail)
}

fn sanitize(name: &str) -> String {
    let s: String = name.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect();
    let s = s.trim_matches('_');
    let s = if s.is_empty() { "bloque" } else { s };
    s.chars().take(40).collect()
}

fn stamp(t: DateTime<Utc>, tz: GridTz) -> String {
    match tz {
        GridTz::Utc => t.format("%Y%m%dT%H%M%SZ").to_string(),
        GridTz::Local => {
            let local = t.with_timezone(&chrono::Local);
            let secs = local.offset().local_minus_utc();
            let sign = if secs < 0 { '-' } else { '+' };
            let secs = secs.abs();
            format!("{}{sign}{:02}{:02}", local.format("%Y%m%dT%H%M%S"), secs / 3600, (secs % 3600) / 60)
        }
    }
}

fn name_clips(drafts: Vec<Draft>, tz: GridTz, ext: &str, indexed: bool) -> Vec<Clip> {
    let width = if drafts.len() >= 1000 { 4 } else { 3 };
    drafts
        .into_iter()
        .enumerate()
        .map(|(i, d)| {
            let when = stamp(d.start, tz);
            let name = if indexed {
                format!("{idx:0width$}_{when}_{label}.{ext}", idx = i + 1, label = d.label)
            } else {
                format!("{when}_{label}.{ext}", label = d.label)
            };
            Clip { name, start: d.start, dur_us: d.dur_us, spans: d.spans }
        })
        .collect()
}

/// Writes one FLAC file, pulling samples in small blocks so a long range is not held in memory.
pub fn write_flac(path: &Path, rate: u32, mut pull: impl FnMut(&mut [f32]) -> usize) -> anyhow::Result<()> {
    use flacenc::component::Stream;
    use flacenc::config::Encoder;
    use flacenc::error::Verify;
    use flacenc::source::{Context, Fill, FrameBuf};

    const BLOCK: usize = 4096;
    let config = Encoder::default().into_verified().map_err(|e| anyhow::anyhow!("flac: {e:?}"))?;
    let mut stream = Stream::new(rate as usize, 1, 16).map_err(|e| anyhow::anyhow!("flac: {e}"))?;
    let mut file = File::create(path)?;
    let header = bytes_of(&stream)?;
    file.write_all(&header)?;

    let mut ctx = Context::new(16, 1);
    let mut frame_no = 0usize;
    let mut pending: Vec<f32> = Vec::new();
    let mut real_samples = 0usize;
    loop {
        let mut buf = vec![0.0f32; BLOCK];
        let n = pull(&mut buf);
        if n == 0 && pending.is_empty() {
            break;
        }
        pending.extend_from_slice(&buf[..n]);
        if n == 0 || pending.len() >= BLOCK {
            let take = if n == 0 { pending.len() } else { BLOCK.min(pending.len()) };
            if take == 0 {
                break;
            }
            let enc_len = take.max(32);
            let mut pcm = vec![0i32; enc_len];
            for (i, s) in pending.drain(..take).enumerate() {
                pcm[i] = (s.clamp(-1.0, 1.0) * 32767.0) as i32;
            }
            ctx.fill_interleaved(&pcm[..take]).map_err(|e| anyhow::anyhow!("flac: {e}"))?;
            let mut fb = FrameBuf::with_size(1, enc_len).map_err(|e| anyhow::anyhow!("flac: {e}"))?;
            fb.fill_interleaved(&pcm).map_err(|e| anyhow::anyhow!("flac: {e}"))?;
            let frame = {
                let info = stream.stream_info();
                flacenc::encode_fixed_size_frame(&config, &fb, frame_no, info).map_err(|e| anyhow::anyhow!("flac: {e}"))?
            };
            stream.stream_info_mut().update_frame_info(&frame);
            file.write_all(&bytes_of(&frame)?)?;
            real_samples += take;
            frame_no += 1;
            if n == 0 {
                break;
            }
        }
    }
    stream.stream_info_mut().set_total_samples(real_samples);
    stream.stream_info_mut().set_md5_digest(&ctx.md5_digest());
    file.seek(SeekFrom::Start(0))?;
    file.write_all(&bytes_of(&stream)?)?;
    Ok(())
}

fn bytes_of(value: &impl flacenc::component::BitRepr) -> anyhow::Result<Vec<u8>> {
    let mut sink = flacenc::bitsink::ByteSink::new();
    value.write(&mut sink).map_err(|e| anyhow::anyhow!("flac: {e:?}"))?;
    Ok(sink.as_slice().to_vec())
}

pub fn write_dir(project: &Project, base: Option<&Path>, dir: &Path, req: &ExportRequest) -> anyhow::Result<Vec<PathBuf>> {
    if let Some(why) = format_block(req.format, false) {
        anyhow::bail!(why);
    }
    let clips = plan(project, req)?;
    std::fs::create_dir_all(dir)?;
    let mut composer = Composer::new(ApiCache::default());
    composer.set_project_dir(base);
    let mut written = Vec::new();
    for clip in &clips {
        if clip.dur_us <= 0 {
            continue;
        }
        let path = dir.join(&clip.name);
        let mut src = ClipSource { project, base, composer: &mut composer, spans: &clip.spans, idx: 0, synth: None, left: 0, skip: 0, silence: true, rate: req.rate };
        write_flac(&path, req.rate, |buf| src.fill(buf))?;
        written.push(path);
    }
    Ok(written)
}

struct ClipSource<'a> {
    project: &'a Project,
    base: Option<&'a Path>,
    composer: &'a mut Composer,
    spans: &'a [Span],
    idx: usize,
    synth: Option<Synth<std::vec::IntoIter<Seg>>>,
    left: u64,
    skip: u64,
    silence: bool,
    rate: u32,
}

impl ClipSource<'_> {
    fn fill(&mut self, buf: &mut [f32]) -> usize {
        let mut wrote = 0;
        while wrote < buf.len() {
            if self.left == 0 && self.skip == 0 {
                if !self.enter() {
                    break;
                }
            }
            if self.skip > 0 {
                let n = (buf.len() - wrote).min(self.skip as usize);
                let mut trash = vec![0.0f32; n];
                let got = self.read_raw(&mut trash);
                self.skip -= got as u64;
                if got == 0 {
                    self.skip = 0;
                    self.left = 0;
                }
                continue;
            }
            let n = ((buf.len() - wrote) as u64).min(self.left) as usize;
            let got = self.read_raw(&mut buf[wrote..wrote + n]);
            if got == 0 {
                self.left = 0;
                continue;
            }
            wrote += got;
            self.left -= got as u64;
        }
        wrote
    }

    fn enter(&mut self) -> bool {
        let Some(span) = self.spans.get(self.idx) else { return false };
        self.idx += 1;
        let samples = (span.dur_us as u128 * u128::from(self.rate) / 1_000_000) as u64;
        let skip = span.tone.as_ref().map(|t| (t.skip_us.max(0) as u128 * u128::from(self.rate) / 1_000_000) as u64).unwrap_or(0);
        self.left = samples;
        self.skip = skip;
        self.silence = span.tone.is_none();
        self.synth = None;
        if let Some(tone) = &span.tone {
            if let Ok(segs) = segments(self.project, self.base, self.composer, tone) {
                let ms = segs.iter().map(|s| s.ms).sum();
                self.synth = Some(Synth::new(segs.into_iter(), f64::from(self.rate), 0.8, ms));
            }
        }
        true
    }

    fn read_raw(&mut self, buf: &mut [f32]) -> usize {
        if self.silence || self.synth.is_none() {
            buf.fill(0.0);
            return buf.len();
        }
        self.synth.as_mut().map(|s| s.fill(buf)).unwrap_or(0)
    }
}

fn segments(project: &Project, dir: Option<&Path>, composer: &mut Composer, tone: &Tone) -> anyhow::Result<Vec<Seg>> {
    if let Some(ready) = air::piece_segments(&tone.piece, &tone.call, tone.wpm, tone.tone) {
        return Ok(ready);
    }
    let mode = tone.piece.mode.unwrap_or(project.tx.mode);
    let live = crate::compose::LiveText::default();
    let ctx = crate::compose::data_ctx(project, Utc::now(), 1, mode, &live, project.freq_hz, "", "");
    let frame = composer.frame_image(project, dir, Some(&tone.piece.path), mode, &ctx);
    let scan = ScanImage::new(mode, &image::DynamicImage::ImageRgba8(frame).to_rgb8(), project.tx.color_range)?;
    let mut segs = Vec::new();
    for line in 0..mode.spec().tx_lines {
        scan.line(line, &mut segs);
    }
    Ok(segs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::{Block, BlockImage, Program};
    use chrono::{NaiveDate, TimeZone};

    fn at(s: &str) -> DateTime<Utc> {
        Utc.from_utc_datetime(&chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S").unwrap())
    }

    fn project() -> Project {
        let mut p = Project::default();
        p.interval_min = 6;
        p.tx.min_pause_s = 0;
        p.tx.cw_id = false;
        p.tx.cw_preamble = false;
        p.tx.mode = Mode::Robot12;
        let mut days = [false; 7];
        days[0] = true;
        p.programs.push(Program {
            id: 1,
            name: "Noticias".into(),
            weekdays: days,
            start: "14:00".into(),
            end: "14:30".into(),
            ..Program::default()
        });
        p.blocks.push(Block {
            id: 4,
            program: Some(1),
            date: NaiveDate::from_ymd_opt(2026, 10, 26).unwrap(),
            images: vec![BlockImage::normal(8, "a.png")],
            ..Block::default()
        });
        p
    }

    fn req(shape: Shape, start: &str, end: &str) -> ExportRequest {
        ExportRequest { start: at(start), end: at(end), shape, format: AudioFormat::Flac, rate: 44_100, lead_ms: 0, tail_ms: 0 }
    }

    #[test]
    fn durations_sum_to_the_range_and_names_sort_in_play_order() {
        let p = project();
        let range = (at("2026-10-26 14:30:00") - at("2026-10-26 14:00:00")).num_microseconds().unwrap();
        for shape in [Shape::Whole, Shape::TxSilence, Shape::Pieces, Shape::Every(600), Shape::PerBlock] {
            let clips = plan(&p, &req(shape, "2026-10-26 14:00:00", "2026-10-26 14:30:00")).unwrap();
            let sum: i64 = clips.iter().map(|c| c.dur_us).sum();
            assert_eq!(sum, range, "{shape:?}");
            let mut names: Vec<_> = clips.iter().map(|c| c.name.clone()).collect();
            let original = names.clone();
            names.sort();
            assert_eq!(names, original, "{shape:?}");
        }
        let whole = plan(&p, &req(Shape::Whole, "2026-10-26 14:00:00", "2026-10-26 14:30:00")).unwrap();
        assert_eq!(whole.len(), 1);
        assert!(whole[0].name.contains("entero.flac"), "{}", whole[0].name);
        assert!(whole[0].name.contains('Z'), "{}", whole[0].name);

        let split = plan(&p, &req(Shape::TxSilence, "2026-10-26 14:00:00", "2026-10-26 14:30:00")).unwrap();
        assert!(split[0].name.contains("_tx.flac"), "{}", split[0].name);
        assert!(split[1].name.contains("_silencio.flac"));
        assert!(split[0].dur_us < split[1].dur_us);

        let pieces = plan(&p, &req(Shape::Pieces, "2026-10-26 14:00:00", "2026-10-26 14:30:00")).unwrap();
        assert!(pieces.iter().any(|c| c.name.contains("_vis.flac")));
        assert!(pieces.iter().any(|c| c.name.contains("_img.flac")));
        assert!(pieces.last().unwrap().name.contains("_silencio.flac"));

        let slices = plan(&p, &req(Shape::Every(7 * 60), "2026-10-26 14:00:00", "2026-10-26 14:30:00")).unwrap();
        assert!(slices.len() >= 2);
        assert!(slices.last().unwrap().dur_us < slices[0].dur_us);
        assert!(slices[0].name.contains("_trozo.flac"));
        let blocks = plan(&p, &req(Shape::PerBlock, "2026-10-26 14:00:00", "2026-10-26 14:30:00")).unwrap();
        assert_eq!(blocks.len(), 1, "the silence until the block end stays in the block file");
        assert!(blocks[0].name.contains("bloque_Noticias"));
    }

    #[test]
    fn a_range_that_starts_mid_transmission_keeps_only_the_rest() {
        let p = project();
        let full = plan(&p, &req(Shape::TxSilence, "2026-10-26 14:00:00", "2026-10-26 14:30:00")).unwrap();
        let mid = plan(&p, &req(Shape::TxSilence, "2026-10-26 14:00:05", "2026-10-26 14:30:00")).unwrap();
        let full_us = (at("2026-10-26 14:30:00") - at("2026-10-26 14:00:00")).num_microseconds().unwrap();
        let mid_us = (at("2026-10-26 14:30:00") - at("2026-10-26 14:00:05")).num_microseconds().unwrap();
        assert_eq!(mid.iter().map(|c| c.dur_us).sum::<i64>(), mid_us);
        assert!(mid[0].dur_us < full[0].dur_us);
        assert_eq!(full.iter().map(|c| c.dur_us).sum::<i64>(), full_us);
    }

    #[test]
    fn offset_is_leading_silence_and_the_warning_is_in_both_languages() {
        let mut p = project();
        p.grid.offset_s = 30;
        let clips = plan(&p, &req(Shape::TxSilence, "2026-10-26 14:00:00", "2026-10-26 14:30:00")).unwrap();
        assert!(clips[0].name.contains("_silencio.flac"), "{}", clips[0].name);
        assert!(clips[0].dur_us >= 30_000_000);
        assert!(quality_warning(AudioFormat::Flac, 44_100, false).is_none());
        assert!(quality_warning(AudioFormat::Flac, 22_050, true).unwrap().contains("44100"));
        assert!(quality_warning(AudioFormat::Mp3, 44_100, false).unwrap().contains("44100"));
        assert!(format_block(AudioFormat::Aac, true).unwrap().contains("AAC"));
        assert!(format_block(AudioFormat::Flac, false).is_none());
    }

    #[test]
    fn a_short_silence_becomes_a_flac_file() {
        let p = Project::default();
        let dir = std::env::temp_dir().join(format!("estivi-flac-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let req = ExportRequest {
            start: at("2026-10-26 14:00:00"),
            end: at("2026-10-26 14:00:01"),
            shape: Shape::Whole,
            format: AudioFormat::Flac,
            rate: 44_100,
            lead_ms: 0,
            tail_ms: 0,
        };
        let files = write_dir(&p, None, &dir, &req).unwrap();
        let bytes = std::fs::read(&files[0]).unwrap();
        assert_eq!(&bytes[..4], b"fLaC");
        assert!(bytes.len() > 100);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
