//! Turning core data into what the window shows.

use std::path::Path;

use chrono::{DateTime, Datelike, NaiveDate, Utc};
use image::RgbaImage;
use image::imageops::FilterType;

/// Keep UI previews bicubic as well. The previews are pre-scaled to their
/// final pixel size so the window renderer does not resample them again.
const RESAMPLE: FilterType = FilterType::CatmullRom;
use slint::{Image, Rgba8Pixel, SharedPixelBuffer, SharedString};

use estivi_core::graphics::{Layer, LayerKind, Rgba, qr_module_px};
use estivi_core::modes::Mode;
use estivi_core::project::GridTz;

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

/// Durations in a timing table: sub-second pieces keep their milliseconds instead
/// of rounding to "0 s" (PTT lead and tail are often a few hundred).
pub fn span(ms: f64) -> String {
    if ms.abs() < 995.0 { format!("{:.0} ms", ms) } else { duration(ms) }
}

pub fn clock_line(offset_ms: i64, error: &str, tz: GridTz) -> String {
    let now = Utc::now() + chrono::TimeDelta::milliseconds(offset_ms);
    let stamp = match tz {
        GridTz::Utc => now.format("%H:%M:%S UTC").to_string(),
        GridTz::Local => now.with_timezone(&chrono::Local).format("%H:%M:%S").to_string(),
    };
    if error.is_empty() {
        i18n::t(&format!("{stamp}  offset {offset_ms} ms"), &format!("{stamp}  desfase {offset_ms} ms"))
    } else {
        i18n::t(&format!("{stamp}  {error}"), &format!("{stamp}  {error}"))
    }
}

pub fn clock_hm(t: DateTime<Utc>, tz: GridTz) -> String {
    match tz {
        GridTz::Utc => t.format("%H:%M").to_string(),
        GridTz::Local => t.with_timezone(&chrono::Local).format("%H:%M").to_string(),
    }
}

/// Start times carry the project offset in seconds, so the seconds matter here.
pub fn clock_hms(t: DateTime<Utc>, tz: GridTz) -> String {
    match tz {
        GridTz::Utc => t.format("%H:%M:%S").to_string(),
        GridTz::Local => t.with_timezone(&chrono::Local).format("%H:%M:%S").to_string(),
    }
}

pub fn when(t: DateTime<Utc>, offset_ms: i64, tz: GridTz) -> String {
    let now = Utc::now() + chrono::TimeDelta::milliseconds(offset_ms);
    let secs = (t - now).num_seconds();
    if secs >= 0 && secs < 48 * 3600 {
        let h = secs / 3600;
        let m = (secs % 3600) / 60;
        let s = secs % 60;
        if h > 0 { format!("{h}:{m:02}:{s:02}") } else { format!("{m:02}:{s:02}") }
    } else {
        match tz {
            GridTz::Utc => t.format("%Y-%m-%d %H:%M UTC").to_string(),
            GridTz::Local => t.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M").to_string(),
        }
    }
}

/// Keep the lines already sent and black out the rest, so Live shows the scan as it goes out.
pub fn reveal(src: &RgbaImage, progress: f32) -> RgbaImage {
    let mut out = src.clone();
    let h = out.height();
    let w = out.width();
    let cut = (progress.clamp(0.0, 1.0) * h as f32).round() as u32;
    let black = [0u8, 0, 0, 255];
    for chunk in out.as_mut().chunks_exact_mut(4).skip(cut as usize * w as usize) {
        chunk.copy_from_slice(&black);
    }
    out
}

/// The largest bicubic version of `src` that fits `w × h`. Previews are set at
/// their final pixel size, so the window renderer does not resample them again
/// with its own cheaper filter.
pub fn fit_contain(src: &RgbaImage, w: u32, h: u32) -> RgbaImage {
    let (w, h) = (w.max(1), h.max(1));
    let (sw, sh) = (src.width().max(1), src.height().max(1));
    let s = (w as f32 / sw as f32).min(h as f32 / sh as f32);
    let tw = ((sw as f32 * s).round() as u32).clamp(1, w);
    let th = ((sh as f32 * s).round() as u32).clamp(1, h);
    if (tw, th) == (src.width(), src.height()) {
        return src.clone();
    }
    image::imageops::resize(src, tw, th, RESAMPLE)
}

/// A stand-in picture so graphics can be judged on something other than black.
/// `kind` follows the editor's base menu: 0 is the transparency checkerboard,
/// 1 color bars, 2 a grid and 3 a gray ramp.
pub fn test_picture(kind: i32, w: u32, h: u32) -> RgbaImage {
    let mut img = RgbaImage::new(w.max(1), h.max(1));
    let (w, h) = (img.width(), img.height());
    match kind {
        0 => {
            // Photoshop's default transparency grid: 8 px squares in white and #cccccc.
            const CELL: u32 = 8;
            for (x, y, p) in img.enumerate_pixels_mut() {
                let light = (x / CELL + y / CELL) % 2 == 0;
                *p = if light { image::Rgba([255, 255, 255, 255]) } else { image::Rgba([204, 204, 204, 255]) };
            }
        }
        1 => {
            let bars: [[u8; 3]; 8] = [
                [192, 192, 192], [192, 192, 0], [0, 192, 192], [0, 192, 0], [192, 0, 192], [192, 0, 0], [0, 0, 192], [0, 0, 0],
            ];
            for (x, y, p) in img.enumerate_pixels_mut() {
                let i = (x as usize * bars.len() / w as usize).min(bars.len() - 1);
                let c = bars[i];
                *p = image::Rgba([c[0], c[1], c[2], 255]);
                let _ = y;
            }
        }
        2 => {
            for (x, y, p) in img.enumerate_pixels_mut() {
                let line = x % 16 == 0 || y % 16 == 0 || x + 1 == w || y + 1 == h || x == w / 2 || y == h / 2;
                *p = if line { image::Rgba([255, 255, 255, 255]) } else { image::Rgba([24, 24, 24, 255]) };
            }
        }
        _ => {
            for (x, y, p) in img.enumerate_pixels_mut() {
                let v = (x as u32 * 255 / w.max(1)) as u8;
                *p = image::Rgba([v, v, v, 255]);
                let _ = y;
            }
        }
    }
    img
}

/// The rectangle that should hug a keep-aspect image, in canvas fractions.
pub fn selector(layer: &Layer, canvas: (u32, u32), px: Option<(u32, u32)>) -> (f32, f32, f32, f32) {
    let (x, y, w, h) = (layer.x, layer.y, layer.w.max(0.0), layer.h.max(0.0));
    let Some((iw, ih)) = px.filter(|(iw, ih)| *iw > 0 && *ih > 0) else { return (x, y, w, h) };
    let LayerKind::Image(img) = &layer.kind else { return (x, y, w, h) };
    if !img.keep_aspect || canvas.0 == 0 || canvas.1 == 0 || w == 0.0 || h == 0.0 {
        return (x, y, w, h);
    }
    let box_w = w * canvas.0 as f32;
    let box_h = h * canvas.1 as f32;
    let scale = (box_w / iw as f32).min(box_h / ih as f32);
    let dw = iw as f32 * scale;
    let dh = ih as f32 * scale;
    (x + (box_w - dw) * 0.5 / canvas.0 as f32, y + (box_h - dh) * 0.5 / canvas.1 as f32, dw / canvas.0 as f32, dh / canvas.1 as f32)
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
    let (sx, sy, sw, sh) = selector(layer, canvas, None);
    item.sel_x = sx;
    item.sel_y = sy;
    item.sel_w = sw;
    item.sel_h = sh;
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

    fn solid(w: u32, h: u32) -> RgbaImage {
        RgbaImage::from_pixel(w, h, image::Rgba([200, 100, 50, 255]))
    }

    #[test]
    fn preview_fit_keeps_aspect_inside_the_box() {
        let a = fit_contain(&solid(640, 480), 200, 190);
        assert_eq!((a.width(), a.height()), (200, 150));
        let b = fit_contain(&solid(100, 200), 200, 190);
        assert_eq!((b.width(), b.height()), (95, 190));
        let c = fit_contain(&solid(100, 100), 200, 190);
        assert_eq!((c.width(), c.height()), (190, 190));
    }

    #[test]
    fn reveal_blacks_only_the_unscanned_part() {
        let out = reveal(&solid(4, 4), 0.5);
        assert_eq!(out.get_pixel(0, 0).0, [200, 100, 50, 255]);
        assert_eq!(out.get_pixel(0, 1).0, [200, 100, 50, 255]);
        assert_eq!(out.get_pixel(0, 2).0, [0, 0, 0, 255]);
        assert_eq!(out.get_pixel(3, 3).0, [0, 0, 0, 255]);
    }

    #[test]
    fn transparency_picture_is_a_photoshop_checkerboard() {
        let img = test_picture(0, 20, 10);
        assert_eq!(img.get_pixel(0, 0).0, [255, 255, 255, 255]);
        assert_eq!(img.get_pixel(7, 7).0, [255, 255, 255, 255]);
        assert_eq!(img.get_pixel(8, 0).0, [204, 204, 204, 255]);
        assert_eq!(img.get_pixel(0, 8).0, [204, 204, 204, 255]);
        assert_eq!(img.get_pixel(8, 8).0, [255, 255, 255, 255]);
    }
}
