//! Channel graphics: the program picture at the bottom, then text, image and QR layers.
//!
//! Geometry is stored normalized (0..1 of the canvas) so the same graphics fit every
//! mode, from 160x120 (Robot 24) to 800x616 (PD 290).

mod fonts;
mod render;
mod text;

pub use fonts::FontBook;
pub use render::{ImageCache, RenderInput, qr_module_px, render};

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rgba(pub [u8; 4]);

impl Rgba {
    pub const BLACK: Rgba = Rgba([0, 0, 0, 255]);
    pub const WHITE: Rgba = Rgba([255, 255, 255, 255]);

    pub fn to_hex(self) -> String {
        let [r, g, b, a] = self.0;
        if a == 255 { format!("#{r:02X}{g:02X}{b:02X}") } else { format!("#{r:02X}{g:02X}{b:02X}{a:02X}") }
    }

    pub fn from_hex(s: &str) -> Option<Rgba> {
        let h = s.trim().trim_start_matches('#');
        let byte = |i: usize| u8::from_str_radix(h.get(i..i + 2)?, 16).ok();
        match h.len() {
            6 => Some(Rgba([byte(0)?, byte(2)?, byte(4)?, 255])),
            8 => Some(Rgba([byte(0)?, byte(2)?, byte(4)?, byte(6)?])),
            _ => None,
        }
    }
}

impl Serialize for Rgba {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for Rgba {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Rgba, D::Error> {
        let s = String::deserialize(d)?;
        Rgba::from_hex(&s).ok_or_else(|| serde::de::Error::custom(format!("color inválido: {s}")))
    }
}

/// How the program picture fills the canvas.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Fit {
    /// Whole picture visible, bars filled with the background color.
    Contain,
    /// Fills the canvas, cropping what does not fit.
    #[default]
    Cover,
    /// Fills the canvas, distorting the aspect ratio.
    Stretch,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Align {
    Start,
    #[default]
    Center,
    End,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Graphics {
    pub background: Rgba,
    pub fit: Fit,
    /// Bottom to top.
    pub layers: Vec<Layer>,
}

impl Default for Graphics {
    fn default() -> Self {
        Graphics { background: Rgba::BLACK, fit: Fit::Cover, layers: Vec::new() }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Layer {
    pub id: u64,
    pub name: String,
    pub visible: bool,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub opacity: f32,
    #[serde(flatten)]
    pub kind: LayerKind,
}

impl Default for Layer {
    fn default() -> Self {
        Layer {
            id: 0,
            name: String::new(),
            visible: true,
            x: 0.05,
            y: 0.05,
            w: 0.5,
            h: 0.15,
            opacity: 1.0,
            kind: LayerKind::Text(TextLayer::default()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "tipo", rename_all = "lowercase")]
pub enum LayerKind {
    Text(TextLayer),
    Image(ImageLayer),
    Qr(QrLayer),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TextLayer {
    /// Text with `{datos}` (see `dynamic::expand`).
    pub text: String,
    /// When set, the response of this URL is available as `{api}`.
    pub api: Option<ApiSource>,
    pub font: String,
    pub bold: bool,
    /// Font size as a fraction of the canvas height.
    pub size: f32,
    pub color: Rgba,
    pub align: Align,
    pub valign: Align,
    /// Shrink the font so the text always fits the box (useful for dynamic text).
    pub shrink_to_fit: bool,
    /// Outline thickness as a fraction of the font size (0 = none).
    pub outline: f32,
    pub outline_color: Rgba,
    pub shadow: bool,
    /// Box fill behind the text (alpha 0 = no box).
    pub box_color: Rgba,
}

impl Default for TextLayer {
    fn default() -> Self {
        TextLayer {
            text: "{call}".into(),
            api: None,
            font: fonts::DEFAULT_FAMILY.into(),
            bold: true,
            size: 0.09,
            color: Rgba::WHITE,
            align: Align::Start,
            valign: Align::Center,
            shrink_to_fit: true,
            outline: 0.10,
            outline_color: Rgba::BLACK,
            shadow: false,
            box_color: Rgba([0, 0, 0, 0]),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ApiSource {
    pub url: String,
    /// Dotted path into a JSON response (`current.temperature_2m`, `items.0.name`).
    /// Empty = use the raw response text.
    pub json_path: String,
    pub refresh_s: u32,
}

impl Default for ApiSource {
    fn default() -> Self {
        ApiSource { url: String::new(), json_path: String::new(), refresh_s: 300 }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ImageLayer {
    /// Relative to the project file when possible.
    pub path: String,
    pub keep_aspect: bool,
}

impl Default for ImageLayer {
    fn default() -> Self {
        ImageLayer { path: String::new(), keep_aspect: true }
    }
}

/// A QR drawn on every frame that already carries the channel graphics.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct QrLayer {
    /// `L`, `M`, `Q` or `H`. Anything else is M.
    pub ecc: String,
    /// `text`, `command` or `api`.
    pub source: String,
    pub text: String,
    pub command: String,
    pub api: Option<ApiSource>,
}

impl Default for QrLayer {
    fn default() -> Self {
        QrLayer { ecc: "M".into(), source: "text".into(), text: "{call}".into(), command: String::new(), api: None }
    }
}

impl Graphics {
    pub fn next_layer_id(&self) -> u64 {
        self.layers.iter().map(|l| l.id).max().unwrap_or(0) + 1
    }

    pub fn api_sources(&self) -> impl Iterator<Item = &ApiSource> {
        self.layers.iter().filter_map(|l| match &l.kind {
            LayerKind::Text(t) => t.api.as_ref().filter(|a| !a.url.trim().is_empty()),
            LayerKind::Image(_) => None,
            LayerKind::Qr(q) => q.api.as_ref().filter(|a| !a.url.trim().is_empty()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_roundtrip() {
        for s in ["#FF8800", "#00000080"] {
            assert_eq!(Rgba::from_hex(s).unwrap().to_hex(), s);
        }
        assert!(Rgba::from_hex("#12").is_none());
    }

    #[test]
    fn layers_serialize_to_readable_toml() {
        let g = Graphics {
            layers: vec![
                Layer { id: 1, name: "Indicativo".into(), ..Default::default() },
                Layer { id: 2, name: "Logo".into(), kind: LayerKind::Image(ImageLayer { path: "logo.png".into(), keep_aspect: true }), ..Default::default() },
            ],
            ..Default::default()
        };
        let s = toml::to_string(&g).unwrap();
        assert!(s.contains("tipo = \"text\""), "{s}");
        let back: Graphics = toml::from_str(&s).unwrap();
        assert_eq!(back, g);
    }
}
