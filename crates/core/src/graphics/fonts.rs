//! Fonts available to text layers: the embedded Noto families plus any TTF/OTF dropped
//! in a `fuentes/` folder next to the project or the executable. System fonts are
//! deliberately not used, so a project renders the same on every station PC.

use std::path::Path;

use ab_glyph::FontArc;

pub const DEFAULT_FAMILY: &str = "Noto Sans";

static NOTO_SANS: &[u8] = include_bytes!("../../assets/fonts/NotoSans-Regular.ttf");
static NOTO_SANS_BOLD: &[u8] = include_bytes!("../../assets/fonts/NotoSans-Bold.ttf");
static NOTO_MONO_BOLD: &[u8] = include_bytes!("../../assets/fonts/NotoSansMono-Bold.ttf");

#[derive(Clone)]
struct Face {
    family: String,
    bold: bool,
    font: FontArc,
}

#[derive(Clone)]
pub struct FontBook {
    faces: Vec<Face>,
}

impl Default for FontBook {
    fn default() -> Self {
        Self::new()
    }
}

impl FontBook {
    pub fn new() -> FontBook {
        let embedded = |family: &str, bold, data: &'static [u8]| Face {
            family: family.into(),
            bold,
            font: FontArc::try_from_slice(data).expect("embedded font is valid"),
        };
        FontBook {
            faces: vec![
                embedded(DEFAULT_FAMILY, false, NOTO_SANS),
                embedded(DEFAULT_FAMILY, true, NOTO_SANS_BOLD),
                embedded("Noto Sans Mono", true, NOTO_MONO_BOLD),
            ],
        }
    }

    /// Adds every .ttf/.otf in `dir`. `Oswald-Bold.ttf` becomes family "Oswald", bold.
    pub fn load_dir(&mut self, dir: &Path) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        let mut paths: Vec<_> = entries.flatten().map(|e| e.path()).collect();
        paths.sort();
        for path in paths {
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
            if ext != "ttf" && ext != "otf" {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else { continue };
            let (family, bold) = split_style(stem);
            if self.faces.iter().any(|f| f.family == family && f.bold == bold) {
                continue;
            }
            match std::fs::read(&path).map_err(anyhow::Error::from).and_then(|d| Ok(FontArc::try_from_vec(d)?)) {
                Ok(font) => self.faces.push(Face { family, bold, font }),
                Err(e) => log::warn!("font {} ignored: {e}", path.display()),
            }
        }
    }

    pub fn families(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for f in &self.faces {
            if !out.contains(&f.family) {
                out.push(f.family.clone());
            }
        }
        out
    }

    pub fn get(&self, family: &str, bold: bool) -> &FontArc {
        let pick = |fam: &str| {
            self.faces
                .iter()
                .find(|f| f.family == fam && f.bold == bold)
                .or_else(|| self.faces.iter().find(|f| f.family == fam))
        };
        &pick(family).or_else(|| pick(DEFAULT_FAMILY)).expect("embedded fonts present").font
    }
}

fn split_style(stem: &str) -> (String, bool) {
    if let Some((family, style)) = stem.rsplit_once(['-', '_', ' ']) {
        let s = style.to_ascii_lowercase();
        if matches!(s.as_str(), "bold" | "black" | "extrabold" | "heavy" | "semibold") {
            return (family.to_string(), true);
        }
        if matches!(s.as_str(), "regular" | "book" | "normal" | "medium") {
            return (family.to_string(), false);
        }
    }
    (stem.to_string(), false)
}

#[cfg(test)]
mod tests {
    #[test]
    fn style_from_file_name() {
        assert_eq!(super::split_style("Oswald-Bold"), ("Oswald".into(), true));
        assert_eq!(super::split_style("Oswald-Regular"), ("Oswald".into(), false));
        assert_eq!(super::split_style("Bebas"), ("Bebas".into(), false));
    }
}
