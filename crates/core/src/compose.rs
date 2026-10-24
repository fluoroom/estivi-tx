//! Builds the on-air frame of a program: picture + channel graphics + live data.
//! Shared by the engine (transmission) and the UI (previews), so what you see is
//! exactly what goes on air.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use image::RgbaImage;

use crate::dynamic::{ApiCache, DataCtx, expand};
use crate::graphics::{FontBook, ImageCache, RenderInput, render};
use crate::project::{Program, Project, resolve};

pub struct Composer {
    pub fonts: FontBook,
    pub cache: ImageCache,
    pub api: ApiCache,
    /// Project fonts folder currently loaded (outer None = nothing loaded yet).
    fonts_dir: Option<Option<PathBuf>>,
}

/// `fuentes/` next to the executable (portable install).
fn exe_fonts_dir() -> Option<PathBuf> {
    std::env::current_exe().ok()?.parent().map(|d| d.join("fuentes"))
}

impl Composer {
    pub fn new(api: ApiCache) -> Composer {
        let mut c = Composer { fonts: FontBook::new(), cache: ImageCache::default(), api, fonts_dir: None };
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

    pub fn frame(&mut self, project: &Project, dir: Option<&Path>, program: Option<&Program>, ctx: &DataCtx) -> RgbaImage {
        let image = program.and_then(|p| p.enabled_images().next());
        let mode = project.mode_of(program, image);
        self.frame_image(project, dir, image.map(|i| i.path.as_str()), mode, ctx)
    }

    /// `image` is a path stored in the project (relative or absolute).
    pub fn frame_image(
        &mut self,
        project: &Project,
        dir: Option<&Path>,
        image: Option<&str>,
        mode: crate::modes::Mode,
        ctx: &DataCtx,
    ) -> RgbaImage {
        let spec = mode.spec();
        let program_path = image.filter(|p| !p.trim().is_empty()).map(|p| resolve(dir, p));
        let api = &self.api;
        render(
            &RenderInput {
                width: spec.width,
                height: spec.height,
                graphics: &project.graphics,
                program: program_path.as_deref(),
                resolve: &|stored| resolve(dir, stored),
                text_of: &|tl| expand(&tl.text, ctx, tl.api.as_ref().and_then(|a| api.value(a)).as_deref()),
                fonts: &self.fonts,
            },
            &mut self.cache,
        )
    }
}

/// Live data for `{datos}`.
pub fn data_ctx(
    project: &Project,
    now: DateTime<Utc>,
    serial: u64,
    program: &str,
    freq_hz: u64,
    cw: &str,
    dtmf: &str,
    mode: crate::modes::Mode,
) -> DataCtx {
    let st = &project.station;
    DataCtx {
        call: st.call.clone(),
        operator: st.operator.clone(),
        qth: st.qth.clone(),
        locator: st.locator.clone(),
        now,
        serial,
        mode,
        program: program.to_string(),
        freq_hz,
        cw: cw.to_string(),
        dtmf: dtmf.to_string(),
    }
}
