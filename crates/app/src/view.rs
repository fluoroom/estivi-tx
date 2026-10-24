//! Turning core data into what the UI shows.

use std::path::Path;

use chrono::{DateTime, Datelike, Local, NaiveDate, TimeZone, Utc};
use image::RgbaImage;
use slint::{Image, Rgba8Pixel, SharedPixelBuffer, SharedString};

use estivi_core::graphics::{Layer, LayerKind, Rgba};
use estivi_core::modes::Mode;
use estivi_core::project::{GridTz, Project, Slot};

/// Combo entries: first row inherits, then every SSTV mode.
pub fn mode_choices(inherit: &str) -> Vec<SharedString> {
    std::iter::once(SharedString::from(inherit)).chain(Mode::ALL.iter().map(|m| SharedString::from(m.name()))).collect()
}

pub fn mode_index(mode: Option<Mode>) -> i32 {
    mode.and_then(|m| Mode::ALL.iter().position(|x| *x == m)).map(|i| i as i32 + 1).unwrap_or(0)
}

pub fn mode_at(index: i32) -> Option<Mode> {
    if index <= 0 { None } else { Mode::ALL.get((index as usize) - 1).copied() }
}

pub fn to_image(img: &RgbaImage) -> Image {
    Image::from_rgba8(SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(img.as_raw(), img.width(), img.height()))
}

/// Small preview for the program list; `None` when the file cannot be read.
pub fn thumbnail(path: &Path) -> Option<Image> {
    let img = image::ImageReader::open(path).ok()?.with_guessed_format().ok()?.decode().ok()?;
    Some(to_image(&img.thumbnail(128, 96).to_rgba8()))
}

pub fn color(c: Rgba) -> slint::Color {
    let [r, g, b, a] = c.0;
    slint::Color::from_argb_u8(a, r, g, b)
}

const DAY_LETTERS: [&str; 7] = ["L", "M", "X", "J", "V", "S", "D"];
const WEEKDAYS: [&str; 7] = ["lunes", "martes", "miércoles", "jueves", "viernes", "sábado", "domingo"];

pub fn days_summary(days: &[bool; 7]) -> String {
    match days {
        [true, true, true, true, true, true, true] => "Todos los días".into(),
        [true, true, true, true, true, false, false] => "Lunes a viernes".into(),
        [false, false, false, false, false, true, true] => "Fin de semana".into(),
        [false, false, false, false, false, false, false] => "Ningún día".into(),
        _ => days.iter().zip(DAY_LETTERS).map(|(&on, l)| if on { l } else { "·" }).collect::<Vec<_>>().join(" "),
    }
}

pub fn hours_summary(slot: &Slot) -> String {
    if slot.every_min > 0 && !slot.end.trim().is_empty() {
        format!("{}–{} · cada {} min", slot.start, slot.end, slot.every_min)
    } else {
        format!("{} · una vez", slot.start)
    }
}

pub fn freq(hz: u64) -> String {
    if hz == 0 { "—".into() } else { format!("{:.3} MHz", hz as f64 / 1e6) }
}

pub fn parse_mhz(s: &str) -> Option<u64> {
    let s = s.trim().replace(',', ".");
    if s.is_empty() {
        return Some(0);
    }
    let mhz: f64 = s.parse().ok()?;
    (mhz > 0.0 && mhz < 300_000.0).then(|| (mhz * 1e6).round() as u64)
}

pub fn duration(ms: f64) -> String {
    let s = (ms / 1000.0).round() as u64;
    if s >= 60 { format!("{} min {:02} s", s / 60, s % 60) } else { format!("{s} s") }
}

pub fn clock(ms: f64) -> String {
    let s = (ms.max(0.0) / 1000.0).floor() as u64;
    format!("{:02}:{:02}", s / 60, s % 60)
}

pub fn countdown(secs: i64) -> String {
    let s = secs.max(0);
    if s >= 3600 { format!("{}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60) } else { format!("{:02}:{:02}", s / 60, s % 60) }
}

/// Time of day in the grid's timezone.
pub fn grid_time(t: DateTime<Utc>, tz: GridTz) -> String {
    match tz {
        GridTz::Utc => t.format("%H:%M UTC").to_string(),
        GridTz::Local => t.with_timezone(&Local).format("%H:%M").to_string(),
    }
}

pub fn grid_today(tz: GridTz) -> NaiveDate {
    match tz {
        GridTz::Utc => Utc::now().date_naive(),
        GridTz::Local => Local::now().date_naive(),
    }
}

/// Midnight of `day` in the grid timezone, as UTC.
pub fn grid_midnight(day: NaiveDate, tz: GridTz) -> DateTime<Utc> {
    let naive = day.and_hms_opt(0, 0, 0).expect("midnight");
    match tz {
        GridTz::Utc => Utc.from_utc_datetime(&naive),
        GridTz::Local => Local.from_local_datetime(&naive).earliest().map_or_else(|| Utc.from_utc_datetime(&naive), |t| t.with_timezone(&Utc)),
    }
}

pub fn day_label(day: NaiveDate, tz: GridTz) -> String {
    format!(
        "Hoy, {} {:02}/{:02} · {}",
        WEEKDAYS[day.weekday().num_days_from_monday() as usize],
        day.day(),
        day.month(),
        if tz == GridTz::Utc { "horario UTC" } else { "horario local" }
    )
}

pub fn program_names_with_rotation(project: &Project) -> Vec<slint::SharedString> {
    std::iter::once("Rotación del canal".into()).chain(project.programs.iter().map(|p| p.name.as_str().into())).collect()
}

pub fn layer_item(layer: &Layer, fonts: &[String]) -> crate::LayerItem {
    let mut item = crate::LayerItem {
        id: layer.id as i32,
        is_text: false,
        name: layer.name.as_str().into(),
        visible: layer.visible,
        x: layer.x,
        y: layer.y,
        w: layer.w,
        h: layer.h,
        opacity: layer.opacity,
        ..Default::default()
    };
    match &layer.kind {
        LayerKind::Text(t) => {
            item.is_text = true;
            item.text = t.text.as_str().into();
            item.font = t.font.as_str().into();
            item.font_index = fonts.iter().position(|f| *f == t.font).unwrap_or(0) as i32;
            item.bold = t.bold;
            item.size = t.size;
            item.color = color(t.color);
            item.color_hex = t.color.to_hex().into();
            item.align = t.align as i32;
            item.valign = t.valign as i32;
            item.shrink = t.shrink_to_fit;
            item.outline = t.outline;
            item.outline_color = color(t.outline_color);
            item.outline_hex = t.outline_color.to_hex().into();
            item.shadow = t.shadow;
            item.box_color = color(t.box_color);
            item.box_hex = t.box_color.to_hex().into();
            if let Some(api) = &t.api {
                item.api_on = true;
                item.api_url = api.url.as_str().into();
                item.api_path = api.json_path.as_str().into();
                item.api_refresh = api.refresh_s as i32;
            }
        }
        LayerKind::Image(i) => {
            item.image_path = i.path.as_str().into();
            item.keep_aspect = i.keep_aspect;
        }
    }
    item
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summaries() {
        assert_eq!(days_summary(&[true; 7]), "Todos los días");
        assert_eq!(days_summary(&[true, false, true, false, true, false, false]), "L · X · V · ·");
        assert_eq!(parse_mhz("14,230"), Some(14_230_000));
        assert_eq!(parse_mhz(""), Some(0));
        assert_eq!(parse_mhz("abc"), None);
        assert_eq!(countdown(3725), "1:02:05");
        assert_eq!(countdown(65), "01:05");
    }
}
