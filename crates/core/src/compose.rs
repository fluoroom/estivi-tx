//! Builds the on-air frame of a program: picture + channel graphics + live data.
//! Shared by the engine (transmission) and the UI (previews), so what you see is
//! exactly what goes on air.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use image::RgbaImage;

use crate::dynamic::{ApiCache, DataCtx, expand};
use crate::graphics::{FontBook, Graphics, ImageCache, QrLayer, RenderInput, paint_layers, render};
use crate::project::{Project, resolve};
use crate::shell;

pub struct Composer {
    pub fonts: FontBook,
    pub cache: ImageCache,
    pub api: ApiCache,
    /// Project fonts folder currently loaded (outer None = nothing loaded yet).
    fonts_dir: Option<Option<PathBuf>>,
    /// Expanded command line → (when, stdout or the error). Failures are remembered so a
    /// preview does not launch the same command on every frame.
    commands: HashMap<String, (Instant, Result<String, String>)>,
}

/// `fuentes/` next to the executable (portable install).
fn exe_fonts_dir() -> Option<PathBuf> {
    std::env::current_exe().ok()?.parent().map(|d| d.join("fuentes"))
}

impl Composer {
    pub fn new(api: ApiCache) -> Composer {
        let mut c = Composer { fonts: FontBook::new(), cache: ImageCache::default(), api, fonts_dir: None, commands: HashMap::new() };
        c.set_project_dir(None);
        c
    }

    /// Reloads fonts when the project moves: embedded + `fuentes/` of the project and
    /// of the executable folder.
    pub fn set_project_dir(&mut self, dir: Option<&Path>) {
        let wanted = dir.map(|d| d.join("fuentes"));
        if self.fonts_dir.as_ref() == Some(&wanted) {
            return;
        }
        let mut fonts = FontBook::new();
        if let Some(d) = &wanted {
            fonts.load_dir(d);
        }
        if let Some(d) = exe_fonts_dir() {
            fonts.load_dir(&d);
        }
        self.fonts = fonts;
        self.fonts_dir = Some(wanted);
    }

    /// `image` is a path stored in the project (relative or absolute).
    /// Channel graphics only. The on-air path uses `frame_chain`.
    pub fn frame_image(&mut self, project: &Project, dir: Option<&Path>, image: Option<&str>, mode: crate::modes::Mode, ctx: &DataCtx) -> RgbaImage {
        self.frame_chain(project, dir, image, None, None, None, mode, ctx)
    }

    /// Picture, then block layers, then program layers, then channel layers.
    /// Each step is painted on the bitmap the previous step left.
    pub fn frame_chain(
        &mut self,
        project: &Project,
        dir: Option<&Path>,
        image: Option<&str>,
        baked: Option<&RgbaImage>,
        block: Option<&Graphics>,
        program: Option<&Graphics>,
        mode: crate::modes::Mode,
        ctx: &DataCtx,
    ) -> RgbaImage {
        let block_g = block.cloned().unwrap_or_default();
        let program_g = program.cloned().unwrap_or_default();
        let channel = project.graphics.clone();
        let mut base = if !block_g.is_plain() {
            block_g.clone()
        } else if !program_g.is_plain() {
            program_g.clone()
        } else {
            channel.clone()
        };
        base.layers.clear();
        let spec = mode.spec();
        let program_path = image.filter(|p| !p.trim().is_empty()).map(|p| resolve(dir, p));
        let levels = [block_g, program_g, channel];
        let maps: Vec<HashMap<u64, String>> = levels.iter().map(|g| self.qr_map(g, ctx)).collect();
        let api = &self.api;
        let mut canvas = render(
            &RenderInput {
                width: spec.width,
                height: spec.height,
                graphics: &base,
                program: program_path.as_deref(),
                resolve: &|stored| resolve(dir, stored),
                text_of: &|tl| expand(&tl.text, ctx, tl.api.as_ref().and_then(|a| api.value(a)).as_deref()),
                qr_of: &|_, _| None,
                fonts: &self.fonts,
                baked,
            },
            &mut self.cache,
        );
        for (g, qr) in levels.iter().zip(maps.iter()) {
            let api = &self.api;
            paint_layers(
                &mut canvas,
                g,
                &|stored| resolve(dir, stored),
                &|tl| expand(&tl.text, ctx, tl.api.as_ref().and_then(|a| api.value(a)).as_deref()),
                &|id, _| qr.get(&id).cloned(),
                &self.fonts,
                &mut self.cache,
            );
        }
        canvas
    }

    fn qr_map(&mut self, graphics: &Graphics, ctx: &DataCtx) -> HashMap<u64, String> {
        let mut out = HashMap::new();
        for layer in &graphics.layers {
            let crate::graphics::LayerKind::Qr(qr) = &layer.kind else { continue };
            if !layer.visible {
                continue;
            }
            match self.qr_text(qr, ctx) {
                Some(text) if !text.trim().is_empty() => {
                    out.insert(layer.id, text);
                }
                Some(_) => log::info!("QR {}: empty payload, the layer is not drawn", layer.name),
                None => {}
            }
        }
        out
    }

    fn qr_text(&mut self, qr: &QrLayer, ctx: &DataCtx) -> Option<String> {
        match qr.source.trim().to_ascii_lowercase().as_str() {
            "command" => {
                let line = expand(&qr.command, ctx, None);
                match self.command(&line) {
                    Ok(s) => Some(s),
                    Err(e) => {
                        log::warn!("QR command: {e}");
                        None
                    }
                }
            }
            "api" => {
                let api = qr.api.as_ref()?;
                if self.api.value(api).is_none() {
                    if let Err(e) = self.api.fetch_now(api) {
                        log::warn!("QR API: {e}");
                    }
                }
                self.api.value(api)
            }
            _ => {
                let api = qr.api.as_ref().and_then(|a| self.api.value(a));
                Some(expand(&qr.text, ctx, api.as_deref()))
            }
        }
    }

    fn command(&mut self, line: &str) -> Result<String, String> {
        if line.trim().is_empty() {
            return Err("empty command".into());
        }
        if let Some((at, cached)) = self.commands.get(line) {
            if at.elapsed() < Duration::from_secs(30) {
                return cached.clone();
            }
        }
        let result = shell::capture(line, Duration::from_secs(10)).map_err(|e| e.to_string());
        self.commands.insert(line.to_string(), (Instant::now(), result.clone()));
        result
    }
}

/// Names the graphics templates can show for the block on air.
#[derive(Clone, Debug, Default)]
pub struct LiveText {
    pub program: String,
    pub block: String,
    pub range: String,
    pub days: String,
    pub daysleft: String,
    pub next: String,
}

/// Live data for `{datos}`.
pub fn data_ctx(project: &Project, now: DateTime<Utc>, serial: u64, mode: crate::modes::Mode, live: &LiveText, freq_hz: u64, cw: &str, dtmf: &str) -> DataCtx {
    let st = &project.station;
    DataCtx {
        call: st.call.clone(),
        operator: st.operator.clone(),
        qth: st.qth.clone(),
        locator: st.locator.clone(),
        now,
        serial,
        mode,
        program: live.program.clone(),
        freq_hz,
        cw: cw.to_string(),
        dtmf: dtmf.to_string(),
        block: live.block.clone(),
        range: live.range.clone(),
        days: live.days.clone(),
        daysleft: live.daysleft.clone(),
        next: live.next.clone(),
    }
}
