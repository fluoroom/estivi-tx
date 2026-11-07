//! Composition of the program picture and the graphics layers at mode resolution.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use image::imageops::FilterType;
use image::{Rgba as Px, RgbaImage};

/// Every scaling step in the graphics pipeline is bicubic (Catmull-Rom):
/// pictures fitted to the mode, cropped, or resized for a layer.
const RESAMPLE: FilterType = FilterType::CatmullRom;

use super::text::{Mask, TextLayout};
use super::{Fit, FontBook, Graphics, LayerKind, QrLayer, Rgba, TextLayer};

/// Decoded pictures and their resized versions, invalidated when the file changes.
#[derive(Default)]
pub struct ImageCache {
    originals: HashMap<PathBuf, (Option<SystemTime>, Arc<RgbaImage>)>,
    fitted: HashMap<(PathBuf, u32, u32, Fit), Arc<RgbaImage>>,
}

impl ImageCache {
    pub fn original(&mut self, path: &Path) -> anyhow::Result<Arc<RgbaImage>> {
        let mtime = std::fs::metadata(path).and_then(|m| m.modified()).ok();
        if let Some((t, img)) = self.originals.get(path) {
            if *t == mtime {
                return Ok(img.clone());
            }
        }
        self.fitted.retain(|k, _| k.0 != path);
        let img = Arc::new(
            image::ImageReader::open(path)?
                .with_guessed_format()?
                .decode()
                .map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?
                .to_rgba8(),
        );
        if self.originals.len() > 64 {
            self.originals.clear();
        }
        self.originals.insert(path.to_path_buf(), (mtime, img.clone()));
        Ok(img)
    }

    /// The picture resized for a `w × h` area. `Contain` returns the largest size that
    /// fits (aspect kept); `Cover` and `Stretch` return exactly `w × h`.
    pub fn fitted(&mut self, path: &Path, w: u32, h: u32, fit: Fit) -> anyhow::Result<Arc<RgbaImage>> {
        let key = (path.to_path_buf(), w.max(1), h.max(1), fit);
        let src = self.original(path)?;
        if let Some(img) = self.fitted.get(&key) {
            return Ok(img.clone());
        }
        let (w, h) = (key.1, key.2);
        let (sw, sh) = (src.width() as f64, src.height() as f64);
        let out = match fit {
            Fit::Stretch => image::imageops::resize(&*src, w, h, RESAMPLE),
            Fit::Contain => {
                let s = (w as f64 / sw).min(h as f64 / sh);
                let (tw, th) = (((sw * s).round() as u32).max(1), ((sh * s).round() as u32).max(1));
                image::imageops::resize(&*src, tw.min(w), th.min(h), RESAMPLE)
            }
            Fit::Cover => {
                let s = (w as f64 / sw).max(h as f64 / sh);
                let (cw, ch) = ((w as f64 / s).round().max(1.0), (h as f64 / s).round().max(1.0));
                let cx = ((sw - cw) / 2.0).max(0.0) as u32;
                let cy = ((sh - ch) / 2.0).max(0.0) as u32;
                let crop = image::imageops::crop_imm(&*src, cx, cy, (cw as u32).min(src.width()), (ch as u32).min(src.height()));
                image::imageops::resize(&*crop, w, h, RESAMPLE)
            }
        };
        if self.fitted.len() > 64 {
            self.fitted.clear();
        }
        let out = Arc::new(out);
        self.fitted.insert(key, out.clone());
        Ok(out)
    }
}

pub struct RenderInput<'a> {
    pub width: u32,
    pub height: u32,
    pub graphics: &'a Graphics,
    /// The program picture (bottom layer), already resolved to an absolute path.
    pub program: Option<&'a Path>,
    /// Resolves a layer image path (relative to the project) to an absolute one.
    pub resolve: &'a dyn Fn(&str) -> PathBuf,
    /// Final text for a text layer, with its `{datos}` expanded.
    pub text_of: &'a dyn Fn(&TextLayer) -> String,
    /// Payload of a QR layer. `None` skips that layer (empty, failed, or too big).
    pub qr_of: &'a dyn Fn(u64, &QrLayer) -> Option<String>,
    pub fonts: &'a FontBook,
    /// When set, this bitmap is the base picture instead of `program`.
    pub baked: Option<&'a RgbaImage>,
}

/// Paint the background color and the base picture, without the layers. The
/// cascade in `Composer` paints every level's layers once, so it can use this
/// entry point and avoid cloning each `Graphics` just to clear its layer list.
pub fn render_base(input: &RenderInput, cache: &mut ImageCache) -> RgbaImage {
    let (w, h) = (input.width, input.height);
    let g = input.graphics;
    let [r, gr, b, _] = g.background.0;
    let mut canvas = RgbaImage::from_pixel(w, h, Px([r, gr, b, 255]));

    if let Some(baked) = input.baked {
        let pic = fit_image(baked, w, h, g.fit);
        let x = (w as i64 - pic.width() as i64) / 2;
        let y = (h as i64 - pic.height() as i64) / 2;
        blend_image(&mut canvas, &pic, x, y, 1.0);
    } else if let Some(path) = input.program {
        match cache.fitted(path, w, h, g.fit) {
            Ok(pic) => {
                let x = (w as i64 - pic.width() as i64) / 2;
                let y = (h as i64 - pic.height() as i64) / 2;
                blend_image(&mut canvas, &pic, x, y, 1.0);
            }
            Err(e) => log::warn!("program picture: {e}"),
        }
    }
    canvas
}

pub fn render(input: &RenderInput, cache: &mut ImageCache) -> RgbaImage {
    let mut canvas = render_base(input, cache);
    paint_layers(&mut canvas, input.graphics, input.resolve, input.text_of, input.qr_of, input.fonts, cache);
    canvas
}

pub fn paint_layers(
    canvas: &mut RgbaImage,
    g: &Graphics,
    resolve: &dyn Fn(&str) -> PathBuf,
    text_of: &dyn Fn(&TextLayer) -> String,
    qr_of: &dyn Fn(u64, &QrLayer) -> Option<String>,
    fonts: &FontBook,
    cache: &mut ImageCache,
) {
    let (w, h) = (canvas.width(), canvas.height());
    for layer in g.layers.iter().filter(|l| l.visible && l.opacity > 0.0) {
        let bx = layer.x * w as f32;
        let by = layer.y * h as f32;
        let bw = (layer.w * w as f32).max(1.0);
        let bh = (layer.h * h as f32).max(1.0);
        let opacity = layer.opacity.clamp(0.0, 1.0);
        match &layer.kind {
            LayerKind::Image(il) => {
                if il.path.trim().is_empty() {
                    continue;
                }
                let path = resolve(&il.path);
                let fit = if il.keep_aspect { Fit::Contain } else { Fit::Stretch };
                match cache.fitted(&path, bw.round() as u32, bh.round() as u32, fit) {
                    Ok(pic) => {
                        let x = bx + (bw - pic.width() as f32) / 2.0;
                        let y = by + (bh - pic.height() as f32) / 2.0;
                        blend_image(canvas, &pic, x.round() as i64, y.round() as i64, opacity);
                    }
                    Err(e) => log::warn!("layer {}: {e}", layer.name),
                }
            }
            LayerKind::Text(tl) => {
                let text = text_of(tl);
                draw_text_layer(canvas, fonts, tl, &text, (bx, by, bw, bh), opacity);
            }
            LayerKind::Qr(qr) => {
                let Some(payload) = qr_of(layer.id, qr) else { continue };
                draw_qr(canvas, &payload, &qr.ecc, (bx, by, bw, bh), opacity, &layer.name);
            }
        }
    }
}

fn fit_image(src: &RgbaImage, w: u32, h: u32, fit: Fit) -> RgbaImage {
    let (w, h) = (w.max(1), h.max(1));
    let (sw, sh) = (src.width() as f64, src.height() as f64);
    match fit {
        Fit::Stretch => image::imageops::resize(src, w, h, RESAMPLE),
        Fit::Contain => {
            let s = (w as f64 / sw).min(h as f64 / sh);
            let (tw, th) = (((sw * s).round() as u32).max(1), ((sh * s).round() as u32).max(1));
            image::imageops::resize(src, tw.min(w), th.min(h), RESAMPLE)
        }
        Fit::Cover => {
            let s = (w as f64 / sw).max(h as f64 / sh);
            let (cw, ch) = ((w as f64 / s).round().max(1.0), (h as f64 / s).round().max(1.0));
            let cx = ((sw - cw) / 2.0).max(0.0) as u32;
            let cy = ((sh - ch) / 2.0).max(0.0) as u32;
            let crop = image::imageops::crop_imm(src, cx, cy, (cw as u32).min(src.width()), (ch as u32).min(src.height()));
            image::imageops::resize(&*crop, w, h, RESAMPLE)
        }
    }
}

fn ecc_level(s: &str) -> qrcode::EcLevel {
    match s.trim().to_ascii_uppercase().as_str() {
        "L" => qrcode::EcLevel::L,
        "Q" => qrcode::EcLevel::Q,
        "H" => qrcode::EcLevel::H,
        _ => qrcode::EcLevel::M,
    }
}

/// Module size in whole pixels, including the 4-module quiet zone, for a layer box.
pub fn qr_module_px(box_w: f32, box_h: f32, modules: usize) -> u32 {
    let side = box_w.min(box_h).max(0.0);
    let span = modules.saturating_add(8).max(1) as f32;
    (side / span).floor().max(0.0) as u32
}

fn draw_qr(canvas: &mut RgbaImage, payload: &str, ecc: &str, bx: (f32, f32, f32, f32), opacity: f32, name: &str) {
    let (x, y, w, h) = bx;
    let code = match qrcode::QrCode::with_error_correction_level(payload.as_bytes(), ecc_level(ecc)) {
        Ok(c) => c,
        Err(e) => {
            log::warn!("QR {name}: {e}");
            return;
        }
    };
    let n = code.width();
    let px = qr_module_px(w, h, n);
    if px == 0 {
        log::warn!("QR {name}: does not fit in the layer");
        return;
    }
    let span = (n + 8) as f32 * px as f32;
    let x0 = (x + (w - span) / 2.0).round() as i32;
    let y0 = (y + (h - span) / 2.0).round() as i32;
    for my in 0..(n + 8) {
        for mx in 0..(n + 8) {
            let inside = mx >= 4 && my >= 4 && mx < n + 4 && my < n + 4;
            let dark = inside && code[(mx - 4, my - 4)] == qrcode::Color::Dark;
            let rgb = if dark { [0, 0, 0] } else { [255, 255, 255] };
            let px0 = x0 + (mx * px as usize) as i32;
            let py0 = y0 + (my * px as usize) as i32;
            for dy in 0..px as i32 {
                for dx in 0..px as i32 {
                    let (pxi, pyi) = (px0 + dx, py0 + dy);
                    if pxi < 0 || pyi < 0 || pxi >= canvas.width() as i32 || pyi >= canvas.height() as i32 {
                        continue;
                    }
                    mix(canvas.get_pixel_mut(pxi as u32, pyi as u32), rgb, opacity);
                }
            }
        }
    }
}

fn draw_text_layer(canvas: &mut RgbaImage, fonts: &FontBook, tl: &TextLayer, text: &str, bx: (f32, f32, f32, f32), opacity: f32) {
    let (x, y, w, h) = bx;
    if tl.box_color.0[3] > 0 {
        fill_rect(canvas, (x, y, w, h), tl.box_color, opacity);
    }
    if text.trim().is_empty() {
        return;
    }
    let font = fonts.get(&tl.font, tl.bold);
    let mut px = (tl.size * canvas.height() as f32).max(4.0);
    let pad = |px: f32| (tl.outline * px).max(0.0) + px * 0.08;
    let mut layout = TextLayout::new(font, px, text);
    if tl.shrink_to_fit {
        let avail_w = (w - 2.0 * pad(px)).max(1.0);
        let avail_h = (h - pad(px)).max(1.0);
        let k = (avail_w / layout.width().max(1.0)).min(avail_h / layout.height().max(1.0));
        if k < 1.0 {
            px = (px * k).max(4.0);
            layout = TextLayout::new(font, px, text);
        }
    }
    let p = pad(px);
    let inner = (x + p, y, (w - 2.0 * p).max(1.0), h);
    let mut mask = Mask::new(canvas.width(), canvas.height());
    layout.draw(font, &mut mask, inner, tl.align, tl.valign);

    let outline = (tl.outline > 0.0).then(|| mask.dilate(tl.outline * px));
    if tl.shadow {
        let off = (px * 0.06).max(1.0).round() as i64;
        blend_mask(canvas, outline.as_ref().unwrap_or(&mask), (off, off), Rgba([0, 0, 0, 170]), opacity);
    }
    if let Some(outline) = &outline {
        blend_mask(canvas, outline, (0, 0), tl.outline_color, opacity);
    }
    blend_mask(canvas, &mask, (0, 0), tl.color, opacity);
}

fn mix(dst: &mut Px<u8>, src: [u8; 3], a: f32) {
    if a <= 0.0 {
        return;
    }
    for (d, s) in dst.0.iter_mut().zip(src) {
        *d = (s as f32 * a + *d as f32 * (1.0 - a)).round() as u8;
    }
}

fn blend_image(canvas: &mut RgbaImage, src: &RgbaImage, x0: i64, y0: i64, opacity: f32) {
    let (cw, ch) = (canvas.width() as i64, canvas.height() as i64);
    for (sx, sy, p) in src.enumerate_pixels() {
        let (x, y) = (x0 + sx as i64, y0 + sy as i64);
        if x < 0 || y < 0 || x >= cw || y >= ch {
            continue;
        }
        let a = p.0[3] as f32 / 255.0 * opacity;
        mix(canvas.get_pixel_mut(x as u32, y as u32), [p.0[0], p.0[1], p.0[2]], a);
    }
}

fn blend_mask(canvas: &mut RgbaImage, mask: &Mask, (ox, oy): (i64, i64), color: Rgba, opacity: f32) {
    let Some((x0, y0, x1, y1)) = mask.bbox else { return };
    let ca = color.0[3] as f32 / 255.0 * opacity;
    let rgb = [color.0[0], color.0[1], color.0[2]];
    for my in y0..y1 {
        for mx in x0..x1 {
            let cov = mask.data[my * mask.w + mx];
            if cov <= 0.0 {
                continue;
            }
            let (x, y) = (mx as i64 + ox, my as i64 + oy);
            if x < 0 || y < 0 || x >= canvas.width() as i64 || y >= canvas.height() as i64 {
                continue;
            }
            mix(canvas.get_pixel_mut(x as u32, y as u32), rgb, cov * ca);
        }
    }
}

fn fill_rect(canvas: &mut RgbaImage, (x, y, w, h): (f32, f32, f32, f32), color: Rgba, opacity: f32) {
    let a = color.0[3] as f32 / 255.0 * opacity;
    let x0 = x.round().max(0.0) as u32;
    let y0 = y.round().max(0.0) as u32;
    let x1 = ((x + w).round().max(0.0) as u32).min(canvas.width());
    let y1 = ((y + h).round().max(0.0) as u32).min(canvas.height());
    for py in y0..y1 {
        for px in x0..x1 {
            mix(canvas.get_pixel_mut(px, py), [color.0[0], color.0[1], color.0[2]], a);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graphics::{Layer, TextLayer};

    #[test]
    fn renders_text_inside_its_box() {
        let g = Graphics {
            layers: vec![Layer {
                x: 0.0,
                y: 0.0,
                w: 0.5,
                h: 0.25,
                kind: LayerKind::Text(TextLayer { text: "LU1ABC".into(), outline: 0.0, ..Default::default() }),
                ..Default::default()
            }],
            ..Default::default()
        };
        let fonts = FontBook::new();
        let out = render(
            &RenderInput {
                width: 320,
                height: 256,
                graphics: &g,
                program: None,
                resolve: &|p| PathBuf::from(p),
                text_of: &|t| t.text.clone(),
                qr_of: &|_, _| None,
                fonts: &fonts,
                baked: None,
            },
            &mut ImageCache::default(),
        );
        let lit = |x0, y0, x1, y1| (y0..y1).flat_map(|y| (x0..x1).map(move |x| (x, y))).filter(|&(x, y)| out.get_pixel(x, y).0[0] > 128).count();
        assert!(lit(0, 0, 160, 64) > 200, "text should be drawn in its box");
        assert_eq!(lit(160, 64, 320, 256), 0, "nothing outside the box");
    }

    #[test]
    fn qr_is_black_modules_on_white() {
        let g = Graphics {
            layers: vec![Layer {
                id: 3,
                x: 0.0,
                y: 0.0,
                w: 1.0,
                h: 1.0,
                kind: LayerKind::Qr(crate::graphics::QrLayer { text: "HI".into(), ..Default::default() }),
                ..Default::default()
            }],
            ..Default::default()
        };
        let fonts = FontBook::new();
        let out = render(
            &RenderInput {
                width: 160,
                height: 160,
                graphics: &g,
                program: None,
                resolve: &|p| PathBuf::from(p),
                text_of: &|t| t.text.clone(),
                qr_of: &|_, q| Some(q.text.clone()),
                fonts: &fonts,
                baked: None,
            },
            &mut ImageCache::default(),
        );
        let black = out.pixels().filter(|p| p.0[0] < 16 && p.0[1] < 16 && p.0[2] < 16).count();
        let white = out.pixels().filter(|p| p.0[0] > 240).count();
        assert!(black > 20, "modules");
        assert!(white > 20, "quiet zone");
        assert_eq!(qr_module_px(20.0, 20.0, 21), 0);
        assert!(qr_module_px(200.0, 200.0, 21) >= 2);
    }
}
