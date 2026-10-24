//! Text layout and rasterization into a coverage mask.

use ab_glyph::{Font, FontArc, PxScale, ScaleFont, point};

use super::Align;

/// Canvas-sized coverage buffer (0..1) with a tracked dirty rectangle.
pub struct Mask {
    pub w: usize,
    pub h: usize,
    pub data: Vec<f32>,
    /// x0, y0, x1, y1 (exclusive) of the touched area.
    pub bbox: Option<(usize, usize, usize, usize)>,
}

impl Mask {
    pub fn new(w: u32, h: u32) -> Mask {
        Mask { w: w as usize, h: h as usize, data: vec![0.0; w as usize * h as usize], bbox: None }
    }

    fn touch(&mut self, x: usize, y: usize) {
        self.bbox = Some(match self.bbox {
            None => (x, y, x + 1, y + 1),
            Some((x0, y0, x1, y1)) => (x0.min(x), y0.min(y), x1.max(x + 1), y1.max(y + 1)),
        });
    }

    pub fn get(&self, x: i64, y: i64) -> f32 {
        if x < 0 || y < 0 || x >= self.w as i64 || y >= self.h as i64 {
            0.0
        } else {
            self.data[y as usize * self.w + x as usize]
        }
    }

    /// Morphological dilation with a disc of radius `r` px (for outlines).
    pub fn dilate(&self, r: f32) -> Mask {
        let mut out = Mask::new(self.w as u32, self.h as u32);
        let Some((x0, y0, x1, y1)) = self.bbox else { return out };
        let ri = r.ceil() as i64;
        let offsets: Vec<(i64, i64, f32)> = (-ri..=ri)
            .flat_map(|dy| (-ri..=ri).map(move |dx| (dx, dy)))
            .filter_map(|(dx, dy)| {
                let d = ((dx * dx + dy * dy) as f32).sqrt();
                // Soft edge on the last pixel ring keeps the outline anti-aliased.
                let weight = (r + 0.5 - d).clamp(0.0, 1.0);
                (weight > 0.0).then_some((dx, dy, weight))
            })
            .collect();
        let ex0 = x0.saturating_sub(ri as usize);
        let ey0 = y0.saturating_sub(ri as usize);
        let ex1 = (x1 + ri as usize).min(self.w);
        let ey1 = (y1 + ri as usize).min(self.h);
        for y in ey0..ey1 {
            for x in ex0..ex1 {
                let mut m = 0.0f32;
                for &(dx, dy, wgt) in &offsets {
                    let v = self.get(x as i64 + dx, y as i64 + dy) * wgt;
                    if v > m {
                        m = v;
                        if m >= 1.0 {
                            break;
                        }
                    }
                }
                if m > 0.0 {
                    out.data[y * self.w + x] = m;
                    out.touch(x, y);
                }
            }
        }
        out
    }
}

struct LineLayout {
    width: f32,
    glyphs: Vec<(ab_glyph::GlyphId, f32)>,
}

pub struct TextLayout {
    lines: Vec<LineLayout>,
    px: f32,
    ascent: f32,
    descent: f32,
    line_h: f32,
}

impl TextLayout {
    pub fn new(font: &FontArc, px: f32, text: &str) -> TextLayout {
        let scaled = font.as_scaled(PxScale::from(px));
        let lines = text
            .lines()
            .map(|line| {
                let mut x = 0.0;
                let mut prev = None;
                let mut glyphs = Vec::new();
                for c in line.chars() {
                    let id = scaled.glyph_id(c);
                    if let Some(p) = prev {
                        x += scaled.kern(p, id);
                    }
                    glyphs.push((id, x));
                    x += scaled.h_advance(id);
                    prev = Some(id);
                }
                LineLayout { width: x, glyphs }
            })
            .collect();
        TextLayout {
            lines,
            px,
            ascent: scaled.ascent(),
            descent: scaled.descent(),
            line_h: scaled.height() + scaled.line_gap(),
        }
    }

    pub fn width(&self) -> f32 {
        self.lines.iter().map(|l| l.width).fold(0.0, f32::max)
    }

    pub fn height(&self) -> f32 {
        // Last line without its line gap.
        let n = self.lines.len().max(1) as f32;
        (n - 1.0) * self.line_h + self.ascent - self.descent
    }

    /// Rasterizes into `mask`, aligned inside the box (x, y, w, h) in pixels.
    pub fn draw(&self, font: &FontArc, mask: &mut Mask, (bx, by, bw, bh): (f32, f32, f32, f32), align: Align, valign: Align) {
        let top = match valign {
            Align::Start => by,
            Align::Center => by + (bh - self.height()) / 2.0,
            Align::End => by + bh - self.height(),
        };
        for (i, line) in self.lines.iter().enumerate() {
            let left = match align {
                Align::Start => bx,
                Align::Center => bx + (bw - line.width) / 2.0,
                Align::End => bx + bw - line.width,
            };
            let baseline = top + self.ascent + i as f32 * self.line_h;
            for &(id, gx) in &line.glyphs {
                let glyph = id.with_scale_and_position(self.px, point(left + gx, baseline));
                let Some(outlined) = font.outline_glyph(glyph) else { continue };
                let b = outlined.px_bounds();
                outlined.draw(|x, y, cov| {
                    let px = b.min.x as i64 + x as i64;
                    let py = b.min.y as i64 + y as i64;
                    if px < 0 || py < 0 || px >= mask.w as i64 || py >= mask.h as i64 {
                        return;
                    }
                    let (px, py) = (px as usize, py as usize);
                    let cell = &mut mask.data[py * mask.w + px];
                    if cov > *cell {
                        *cell = cov.min(1.0);
                    }
                    mask.touch(px, py);
                });
            }
        }
    }
}
