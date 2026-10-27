//! Turning core data into what the window shows.

use std::path::Path;

use chrono::{DateTime, Datelike, NaiveDate, Utc};
use image::RgbaImage;
use slint::{Image, Rgba8Pixel, SharedPixelBuffer, SharedString};

use estivi_core::air::{Row, RowKind};
use estivi_core::graphics::{Layer, LayerKind, Rgba, qr_module_px};
use estivi_core::modes::Mode;
use estivi_core::project::{BlockImage, ImageRole};

use crate::i18n;

pub fn mode_names() -> Vec<SharedString> {
    Mode::ALL.iter().map(|m| SharedString::from(m.name())).collect()
}

pub fn mode_index(mode: Mode) -> i32 {
    Mode::ALL.iter().position(|m| *m == mode).unwrap_or(0) as i32
}

pub fn mode_at(index: i32) -> Mode {
    Mode::ALL.get(index.max(0) as usize).copied().unwrap_or(Mode::MartinM1)
}

pub fn to_image(img: &RgbaImage) -> Image {
    Image::from_rgba8(SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(img.as_raw(), img.width(), img.height()))
}

pub fn color(c: Rgba) -> slint::Color {
    let [r, g, b, a] = c.0;
    slint::Color::from_argb_u8(a, r, g, b)
}

pub fn freq(hz: u64) -> String {
    if hz == 0 { "0".into() } else { format!("{:.3}", hz as f64 / 1e6) }
}

pub fn parse_mhz(s: &str) -> Option<u64> {
    let s = s.trim().replace(',', ".");
    if s.is_empty() || s == "0" {
        return Some(0);
    }
    let mhz: f64 = s.parse().ok()?;
    (mhz > 0.0 && mhz < 300_000.0).then(|| (mhz * 1e6).round() as u64)
}

pub fn duration(ms: f64) -> String {
    let sign = if ms < 0.0 { "−" } else { "" };
    let s = (ms.abs() / 1000.0).round() as u64;
    if s >= 60 { format!("{sign}{}:{:02}", s / 60, s % 60) } else { format!("{sign}{s} s") }
}

pub fn clock_line(offset_ms: i64, error: &str) -> String {
    let now = Utc::now() + chrono::TimeDelta::milliseconds(offset_ms);
    let stamp = now.format("%H:%M:%S");
    if error.is_empty() {
        i18n::t(&format!("{stamp}  offset {offset_ms} ms"), &format!("{stamp}  desfase {offset_ms} ms"))
    } else {
        i18n::t(&format!("{stamp}  {error}"), &format!("{stamp}  {error}"))
    }
}

pub fn when(t: DateTime<Utc>, offset_ms: i64) -> String {
    let now = Utc::now() + chrono::TimeDelta::milliseconds(offset_ms);
    let secs = (t - now).num_seconds();
    if secs >= 0 && secs < 48 * 3600 {
        let h = secs / 3600;
        let m = (secs % 3600) / 60;
        let s = secs % 60;
        if h > 0 { format!("{h}:{m:02}:{s:02}") } else { format!("{m:02}:{s:02}") }
    } else {
        t.format("%Y-%m-%d %H:%M").to_string()
    }
}

pub fn row_label(row: &Row) -> String {
    let name = match row.kind {
        RowKind::Vox => i18n::ts("VOX", "VOX"),
        RowKind::Cw => i18n::ts("CW", "CW"),
        RowKind::Ax25 => "AX.25",
        RowKind::Ax25Mode => i18n::ts("AX.25 mode", "AX.25 modo"),
        RowKind::Vis => "VIS",
        RowKind::Welcome => i18n::ts("Welcome", "Bienvenida"),
        RowKind::Bumper => i18n::ts("Bumper", "Cortina"),
        RowKind::Image => i18n::ts("Image", "Imagen"),
        RowKind::Bye => i18n::ts("Bye", "Despedida"),
        RowKind::Silence => i18n::ts("Silence", "Silencio"),
        RowKind::Total => i18n::ts("Total", "Total"),
    };
    if row.airs { name.into() } else { i18n::t(&format!("{name} (skipped)"), &format!("{name} (no sale)")) }
}

pub fn role_name(image: &BlockImage) -> String {
    match image.role {
        ImageRole::Normal => i18n::ts("Image", "Imagen").to_string(),
        ImageRole::Pre { .. } => i18n::ts("Pre", "Antes").to_string(),
        ImageRole::Post { .. } => i18n::ts("Post", "Después").to_string(),
    }
}

pub fn file_name(path: &str) -> String {
    Path::new(path).file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| path.to_string())
}

pub fn day_heading(date: NaiveDate) -> String {
    let name = match date.weekday().num_days_from_monday() {
        0 => i18n::ts("Mon", "Lun"),
        1 => i18n::ts("Tue", "Mar"),
        2 => i18n::ts("Wed", "Mié"),
        3 => i18n::ts("Thu", "Jue"),
        4 => i18n::ts("Fri", "Vie"),
        5 => i18n::ts("Sat", "Sáb"),
        _ => i18n::ts("Sun", "Dom"),
    };
    format!("{name} {}", date.format("%d"))
}

pub fn layer_item(layer: &Layer, fonts: &[String], canvas: (u32, u32)) -> crate::LayerItem {
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
        LayerKind::Qr(q) => {
            item.is_qr = true;
            item.qr_source = match q.source.as_str() {
                "command" => 1,
                "api" => 2,
                _ => 0,
            };
            item.qr_ecc = match q.ecc.as_str() {
                "L" => 0,
                "Q" => 2,
                "H" => 3,
                _ => 1,
            };
            item.qr_text = q.text.as_str().into();
            item.qr_command = q.command.as_str().into();
            if let Some(api) = &q.api {
                item.api_on = true;
                item.api_url = api.url.as_str().into();
                item.api_path = api.json_path.as_str().into();
                item.api_refresh = api.refresh_s as i32;
            }
            let px = qr_module_px(layer.w * canvas.0 as f32, layer.h * canvas.1 as f32, 177);
            if px < 2 {
                item.qr_warn = i18n::t(
                    "At the largest QR this box is under 2 px per module in the current mode. It still draws.",
                    "En el QR más grande este recuadro queda bajo 2 px por módulo en el modo actual. Igual se dibuja.",
                ).into();
            }
        }
    }
    item
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn megahertz_and_duration() {
        assert_eq!(parse_mhz("14,230"), Some(14_230_000));
        assert_eq!(parse_mhz(""), Some(0));
        assert_eq!(parse_mhz("abc"), None);
        assert_eq!(duration(65_000.0), "1:05");
        assert_eq!(duration(-1500.0), "−2 s");
    }
}
