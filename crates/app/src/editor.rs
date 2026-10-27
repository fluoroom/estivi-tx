//! The channel graphics editor window: a WYSIWYG canvas rendered by the same code
//! that renders the transmission, plus a layer list and an inspector.

use std::cell::RefCell;
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use chrono::Utc;
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};

use estivi_core::compose::data_ctx;
use estivi_core::dynamic::PLACEHOLDERS;
use estivi_core::graphics::{Align, ApiSource, Fit, ImageLayer, Layer, LayerKind, Rgba, TextLayer};

use crate::app::{App, Shared, apply_theme_to, bind_files, hook_os_drops, is_image, paths_model, pick_native, with_app, NativePick};
use crate::i18n;
use crate::view;
use crate::{EditorWindow, LayerItem};

fn placeholder_desc(en: &str) -> String {
    match en {
        "Callsign" => i18n::t("Callsign", "Indicativo"),
        "Operator" => i18n::t("Operator", "Operador"),
        "UTC time (HH:MM)" => i18n::t("UTC time (HH:MM)", "Hora UTC (HH:MM)"),
        "UTC date (YYYY-MM-DD)" => i18n::t("UTC date (YYYY-MM-DD)", "Fecha UTC (AAAA-MM-DD)"),
        "Local time (HH:MM)" => i18n::t("Local time (HH:MM)", "Hora local (HH:MM)"),
        "UTC with a custom format" => i18n::t("UTC with a custom format", "UTC con formato propio"),
        "Transmission number" => i18n::t("Transmission number", "Número de emisión"),
        "Number with zeros (0042)" => i18n::t("Number with zeros (0042)", "Número con ceros (0042)"),
        "SSTV mode" => i18n::t("SSTV mode", "Modo SSTV"),
        "Program name" => i18n::t("Program name", "Nombre del programa"),
        "Frequency (MHz)" => i18n::t("Frequency (MHz)", "Frecuencia (MHz)"),
        "Last received CW text" => i18n::t("Last received CW text", "Último texto CW recibido"),
        "Last callsign heard in CW" => i18n::t("Last callsign heard in CW", "Último indicativo oído en CW"),
        "Last DTMF sequence" => i18n::t("Last DTMF sequence", "Última secuencia DTMF"),
        "This layer's API response" => i18n::t("This layer's API response", "Respuesta de la API de la capa"),
        other => other.to_string(),
    }
}

/// Snap distance, as a fraction of the canvas.
const SNAP: f32 = 0.015;
const MIN_SIZE: f32 = 0.02;

#[derive(Clone, Copy)]
struct Drag {
    id: u64,
    /// Corner being dragged ((0|1, 0|1)), or None when moving.
    corner: Option<(f32, f32)>,
    start: (f32, f32),
    rect: (f32, f32, f32, f32),
    moved: bool,
}

pub struct Editor {
    win: EditorWindow,
    visible: bool,
    selected: Option<u64>,
    /// Layer the inspector was built for.
    inspector_for: Option<Option<u64>>,
    preview: Option<u64>,
    drag: Option<Drag>,
    fonts: Vec<String>,
    layers: Rc<VecModel<LayerItem>>,
    current: Rc<VecModel<LayerItem>>,
    last_render: Instant,
    api_updated: Arc<AtomicBool>,
}

fn edit(w: &Weak<RefCell<App>>, f: impl FnOnce(&mut Editor, &mut App)) {
    with_app(w, |a| {
        if let Some(mut ed) = a.editor.take() {
            f(&mut ed, a);
            a.editor = Some(ed);
        }
    });
}

fn layer_mut(a: &mut App, id: u64) -> Option<&mut Layer> {
    a.project.graphics.layers.iter_mut().find(|l| l.id == id)
}

fn align_of(i: f32) -> Align {
    match i.round() as i32 {
        0 => Align::Start,
        2 => Align::End,
        _ => Align::Center,
    }
}

pub fn open(app: &Shared) {
    let w = Rc::downgrade(app);
    let mut a = app.borrow_mut();
    if a.editor.is_none() {
        match Editor::new(w) {
            Ok(ed) => a.editor = Some(ed),
            Err(e) => {
                log::error!("editor: {e}");
                return;
            }
        }
    }
    let mut ed = a.editor.take().expect("just created");
    apply_theme_to(&ed.win.global::<crate::Theme>(), a.config.ui.light);
    ed.visible = true;
    if ed.preview.is_none() {
        ed.preview = a.project.programs.iter().find(|p| p.enabled).map(|p| p.id);
    }
    ed.refresh(&mut a, true);
    let _ = ed.win.show();
    a.editor = Some(ed);
}

impl Editor {
    pub fn apply_theme(&self, light: bool) {
        apply_theme_to(&self.win.global::<crate::Theme>(), light);
    }

    fn new(w: Weak<RefCell<App>>) -> Result<Editor, slint::PlatformError> {
        let win = EditorWindow::new()?;
        bind_files(&win.global::<crate::Files>());
        {
            let hover = win.as_weak();
            let drop = win.as_weak();
            hook_os_drops(
                win.window(),
                move |on| {
                    if let Some(win) = hover.upgrade() {
                        win.global::<crate::Files>().set_hovering(on);
                    }
                },
                move |paths| {
                    if let Some(win) = drop.upgrade() {
                        win.invoke_drop_image(paths_model(&paths));
                    }
                },
            );
        }
        let layers = Rc::new(VecModel::default());
        let current = Rc::new(VecModel::default());
        win.set_layers(layers.clone().into());
        win.set_current(current.clone().into());
        let labels: Vec<SharedString> = std::iter::once(i18n::ts("Insert field…", "Insertar dato…").into())
            .chain(PLACEHOLDERS.iter().map(|(t, d)| SharedString::from(format!("{t}  —  {}", placeholder_desc(d)))))
            .collect();
        let tokens: Vec<SharedString> = std::iter::once("".into()).chain(PLACEHOLDERS.iter().map(|(t, _)| SharedString::from(*t))).collect();
        win.set_placeholders(ModelRc::new(VecModel::from(labels)));
        win.set_placeholder_tokens(ModelRc::new(VecModel::from(tokens)));

        win.window().on_close_requested({
            let w = w.clone();
            move || {
                edit(&w, |ed, _| ed.visible = false);
                slint::CloseRequestResponse::HideWindow
            }
        });
        win.on_add_text({
            let w = w.clone();
            move || edit(&w, |ed, a| ed.add_text(a))
        });
        win.on_add_image({
            let w = w.clone();
            move || edit(&w, |ed, a| ed.add_image_placeholder(a))
        });
        win.on_pick_image({
            let weak = win.as_weak();
            move || {
                let weak = weak.clone();
                pick_native(NativePick::Images { many: false }, move |paths| {
                    if let Some(win) = weak.upgrade() {
                        win.invoke_drop_image(paths_model(&paths));
                    }
                });
            }
        });
        win.on_drop_image({
            let w = w.clone();
            move |paths| {
                let Some(p) = paths.iter().map(|s| std::path::PathBuf::from(s.as_str())).find(|p| is_image(p)) else { return };
                edit(&w, |ed, a| {
                    if ed.selected.and_then(|id| a.project.graphics.layers.iter().find(|l| l.id == id)).is_some_and(|l| matches!(l.kind, LayerKind::Image(_))) {
                        let stored = a.relativize(&p);
                        if let Some(l) = ed.selected.and_then(|id| layer_mut(a, id)) {
                            if let LayerKind::Image(i) = &mut l.kind {
                                i.path = stored;
                                ed.edited(a, true);
                            }
                        }
                    } else {
                        ed.add_image(a, &p);
                    }
                });
            }
        });
        win.on_select({
            let w = w.clone();
            move |idx| edit(&w, |ed, a| ed.select_index(a, idx))
        });
        win.on_toggle_visible({
            let w = w.clone();
            move |idx| {
                edit(&w, |ed, a| {
                    if let Some(l) = a.project.graphics.layers.get_mut(idx as usize) {
                        l.visible = !l.visible;
                        ed.structural(a);
                    }
                })
            }
        });
        win.on_move_layer({
            let w = w.clone();
            move |dir| edit(&w, |ed, a| ed.move_layer(a, dir))
        });
        win.on_duplicate({
            let w = w.clone();
            move || edit(&w, |ed, a| ed.duplicate(a))
        });
        win.on_remove({
            let w = w.clone();
            move || {
                edit(&w, |ed, a| {
                    if let Some(id) = ed.selected {
                        a.project.graphics.layers.retain(|l| l.id != id);
                        ed.selected = None;
                        ed.structural(a);
                    }
                })
            }
        });
        win.on_preview_selected({
            let w = w.clone();
            move |i| {
                edit(&w, |ed, a| {
                    ed.preview = a.project.programs.get(i as usize).map(|p| p.id);
                    ed.render(a);
                })
            }
        });
        win.on_fit_selected({
            let w = w.clone();
            move |i| {
                edit(&w, |ed, a| {
                    a.project.graphics.fit = match i {
                        1 => Fit::Contain,
                        2 => Fit::Stretch,
                        _ => Fit::Cover,
                    };
                    ed.structural(a);
                })
            }
        });
        win.on_background_changed({
            let w = w.clone();
            move |hex| {
                edit(&w, |ed, a| {
                    if let Some(c) = Rgba::from_hex(&hex) {
                        a.project.graphics.background = c;
                        ed.structural(a);
                    }
                })
            }
        });
        win.on_canvas_press({
            let w = w.clone();
            move |x, y, hx, hy| edit(&w, |ed, a| ed.press(a, x, y, hx, hy))
        });
        win.on_canvas_drag({
            let w = w.clone();
            move |x, y| edit(&w, |ed, a| ed.drag_to(a, x, y))
        });
        win.on_canvas_release({
            let w = w.clone();
            move || edit(&w, |ed, a| ed.release(a))
        });
        win.on_nudge({
            let w = w.clone();
            move |dx, dy| {
                edit(&w, |ed, a| {
                    let spec = a.project.tx.mode.spec();
                    if let Some(l) = ed.selected.and_then(|id| layer_mut(a, id)) {
                        l.x += dx as f32 / spec.width as f32;
                        l.y += dy as f32 / spec.height as f32;
                        ed.edited(a, true);
                        ed.reset_inspector(a);
                    }
                })
            }
        });
        win.on_set_str({
            let w = w.clone();
            move |key, v| edit(&w, |ed, a| ed.set_str(a, &key, &v))
        });
        win.on_set_num({
            let w = w.clone();
            move |key, v| edit(&w, |ed, a| ed.set_num(a, &key, v))
        });
        win.on_set_bool({
            let w = w.clone();
            move |key, v| edit(&w, |ed, a| ed.set_bool(a, &key, v))
        });
        win.on_api_test({
            let w = w.clone();
            move || edit(&w, |ed, a| ed.api_test(a))
        });

        Ok(Editor {
            win,
            visible: false,
            selected: None,
            inspector_for: None,
            preview: None,
            drag: None,
            fonts: Vec::new(),
            layers,
            current,
            last_render: Instant::now(),
            api_updated: Arc::new(AtomicBool::new(false)),
        })
    }

    /// Rebuilds everything from the project (`force_inspector` also rebuilds the
    /// property panel, discarding half-typed values).
    pub fn refresh(&mut self, a: &mut App, force_inspector: bool) {
        let spec = a.project.tx.mode.spec();
        self.win.set_canvas_w(spec.width as i32);
        self.win.set_canvas_h(spec.height as i32);
        self.win.set_mode_name(spec.name.into());
        self.fonts = a.composer.fonts.families();
        self.win.set_fonts(ModelRc::new(VecModel::from(self.fonts.iter().map(|f| SharedString::from(f.as_str())).collect::<Vec<_>>())));

        let programs = &a.project.programs;
        if self.preview.is_some_and(|id| !programs.iter().any(|p| p.id == id)) {
            self.preview = programs.first().map(|p| p.id);
        }
        let names: Vec<SharedString> =
            if programs.is_empty() {
                vec![i18n::ts("(no programs: background only)", "(sin programas: solo fondo)").into()]
            } else {
                programs.iter().map(|p| p.name.as_str().into()).collect()
            };
        self.win.set_program_names(ModelRc::new(VecModel::from(names)));
        self.win.set_preview_index(self.preview.and_then(|id| programs.iter().position(|p| p.id == id)).unwrap_or(0) as i32);

        let g = &a.project.graphics;
        self.win.set_fit_index(match g.fit {
            Fit::Cover => 0,
            Fit::Contain => 1,
            Fit::Stretch => 2,
        });
        self.win.set_background_color(view::color(g.background));
        self.win.set_background_hex(g.background.to_hex().into());

        if self.selected.is_some_and(|id| !g.layers.iter().any(|l| l.id == id)) {
            self.selected = None;
        }
        self.layers.set_vec(g.layers.iter().map(|l| view::layer_item(l, &self.fonts)).collect::<Vec<_>>());
        self.update_selection_index(a);
        if force_inspector {
            self.reset_inspector(a);
        } else {
            self.sync_inspector(a);
        }
        self.render(a);
    }

    fn update_selection_index(&self, a: &App) {
        let idx = self.selected.and_then(|id| a.project.graphics.layers.iter().position(|l| l.id == id)).map_or(-1, |i| i as i32);
        self.win.set_selected(idx);
    }

    /// Recreates the property panel for the selected layer.
    fn reset_inspector(&mut self, a: &App) {
        let item = self.selected.and_then(|id| a.project.graphics.layers.iter().find(|l| l.id == id)).map(|l| view::layer_item(l, &self.fonts));
        self.current.set_vec(item.into_iter().collect::<Vec<_>>());
        self.inspector_for = Some(self.selected);
        self.win.set_api_result("".into());
    }

    /// Updates the property panel in place (keeps focus and cursor in text fields).
    fn sync_inspector(&mut self, a: &App) {
        if self.inspector_for != Some(self.selected) {
            self.reset_inspector(a);
            return;
        }
        if let Some(l) = self.selected.and_then(|id| a.project.graphics.layers.iter().find(|l| l.id == id)) {
            if self.current.row_count() == 1 {
                self.current.set_row_data(0, view::layer_item(l, &self.fonts));
            }
        }
    }

    fn render(&mut self, a: &mut App) {
        let program = self.preview.and_then(|id| a.project.program(id)).cloned();
        let name = program.as_ref().map_or(i18n::ts("Program", "Programa"), |p| p.name.as_str()).to_string();
        let mode = a.project.mode_of(program.as_ref(), program.as_ref().and_then(|p| p.enabled_images().next()));
        let ctx = data_ctx(&a.project, Utc::now(), a.status.serial + 1, &name, 0, &a.status.last_cw, &a.status.last_dtmf, mode);
        let dir = a.dir();
        let frame = a.composer.frame(&a.project, dir.as_deref(), program.as_ref(), &ctx);
        self.win.set_frame(view::to_image(&frame));
        self.last_render = Instant::now();
    }

    pub fn tick(&mut self, a: &mut App) {
        // Keep clock data and fresh API values current while the window is open.
        let api = self.api_updated.swap(false, Ordering::Relaxed);
        if self.visible && self.drag.is_none() && (api || self.last_render.elapsed() > Duration::from_secs(10)) {
            self.render(a);
        }
    }

    /// A property of the selected layer changed.
    fn edited(&mut self, a: &mut App, notify_engine: bool) {
        a.graphics_changed(notify_engine);
        if let Some(i) = self.selected.and_then(|id| a.project.graphics.layers.iter().position(|l| l.id == id)) {
            if i < self.layers.row_count() {
                self.layers.set_row_data(i, view::layer_item(&a.project.graphics.layers[i], &self.fonts));
            }
        }
        self.sync_inspector(a);
        self.render(a);
    }

    /// Layers were added, removed, reordered or the base changed.
    fn structural(&mut self, a: &mut App) {
        a.graphics_changed(true);
        self.refresh(a, false);
    }

    fn select_index(&mut self, a: &mut App, idx: i32) {
        self.selected = (idx >= 0).then(|| a.project.graphics.layers.get(idx as usize).map(|l| l.id)).flatten();
        self.update_selection_index(a);
        self.reset_inspector(a);
    }

    fn select_new(&mut self, a: &mut App, layer: Layer) {
        self.selected = Some(layer.id);
        a.project.graphics.layers.push(layer);
        self.structural(a);
        self.reset_inspector(a);
    }

    fn add_text(&mut self, a: &mut App) {
        let id = a.project.graphics.next_layer_id();
        let n = a.project.graphics.layers.iter().filter(|l| matches!(l.kind, LayerKind::Text(_))).count() + 1;
        let layer = Layer {
            id,
            name: format!("{} {n}", i18n::ts("Text", "Texto")),
            x: 0.04,
            y: 0.80,
            w: 0.92,
            h: 0.15,
            kind: LayerKind::Text(TextLayer { text: if n == 1 { "{call}  {utc}Z".into() } else { "Texto".into() }, ..Default::default() }),
            ..Default::default()
        };
        self.select_new(a, layer);
    }

    fn add_image_placeholder(&mut self, a: &mut App) {
        let id = a.project.graphics.next_layer_id();
        let layer = Layer {
            id,
            name: i18n::t("Image", "Imagen"),
            x: 0.66,
            y: 0.04,
            w: 0.3,
            h: 0.25,
            kind: LayerKind::Image(ImageLayer { path: String::new(), keep_aspect: true }),
            ..Default::default()
        };
        self.select_new(a, layer);
    }

    fn add_image(&mut self, a: &mut App, path: &std::path::Path) {
        let spec = a.project.tx.mode.spec();
        let (iw, ih) = image::image_dimensions(path).unwrap_or((1, 1));
        let w = 0.3f32;
        let h = (w * ih as f32 / iw.max(1) as f32 * spec.width as f32 / spec.height as f32).clamp(MIN_SIZE, 0.9);
        let id = a.project.graphics.next_layer_id();
        let layer = Layer {
            id,
            name: path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| i18n::t("Image", "Imagen")),
            x: 0.96 - w,
            y: 0.04,
            w,
            h,
            kind: LayerKind::Image(ImageLayer { path: a.relativize(path), keep_aspect: true }),
            ..Default::default()
        };
        self.select_new(a, layer);
    }

    fn move_layer(&mut self, a: &mut App, dir: i32) {
        let layers = &mut a.project.graphics.layers;
        let Some(i) = self.selected.and_then(|id| layers.iter().position(|l| l.id == id)) else { return };
        let j = i as i32 + dir;
        if j >= 0 && (j as usize) < layers.len() {
            layers.swap(i, j as usize);
            self.structural(a);
        }
    }

    fn duplicate(&mut self, a: &mut App) {
        let Some(src) = self.selected.and_then(|id| a.project.graphics.layers.iter().find(|l| l.id == id)).cloned() else { return };
        let layer = Layer {
            id: a.project.graphics.next_layer_id(),
            name: format!("{} (copia)", src.name),
            x: (src.x + 0.03).min(1.0 - MIN_SIZE),
            y: (src.y + 0.03).min(1.0 - MIN_SIZE),
            ..src
        };
        self.select_new(a, layer);
    }

    fn press(&mut self, a: &mut App, x: f32, y: f32, hx: f32, hy: f32) {
        let layers = &a.project.graphics.layers;
        if let Some(sel) = self.selected.and_then(|id| layers.iter().find(|l| l.id == id)) {
            for corner in [(0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)] {
                let (cx, cy) = (sel.x + corner.0 * sel.w, sel.y + corner.1 * sel.h);
                if (x - cx).abs() <= hx && (y - cy).abs() <= hy {
                    self.drag = Some(Drag { id: sel.id, corner: Some(corner), start: (x, y), rect: (sel.x, sel.y, sel.w, sel.h), moved: false });
                    return;
                }
            }
        }
        let hit = layers.iter().rev().find(|l| l.visible && x >= l.x && x <= l.x + l.w && y >= l.y && y <= l.y + l.h).cloned();
        match hit {
            Some(l) => {
                if self.selected != Some(l.id) {
                    self.selected = Some(l.id);
                    self.update_selection_index(a);
                    self.reset_inspector(a);
                }
                self.drag = Some(Drag { id: l.id, corner: None, start: (x, y), rect: (l.x, l.y, l.w, l.h), moved: false });
            }
            None => {
                if self.selected.is_some() {
                    self.selected = None;
                    self.update_selection_index(a);
                    self.reset_inspector(a);
                }
            }
        }
    }

    fn drag_to(&mut self, a: &mut App, x: f32, y: f32) {
        let Some(mut d) = self.drag else { return };
        let (dx, dy) = (x - d.start.0, y - d.start.1);
        if !d.moved && dx.abs() < 0.002 && dy.abs() < 0.002 {
            return;
        }
        d.moved = true;
        self.drag = Some(d);
        let (sx, sy, sw, sh) = d.rect;
        let (mut gx, mut gy) = (-1.0f32, -1.0f32);
        let Some(layer) = layer_mut(a, d.id) else { return };
        let keep = matches!(&layer.kind, LayerKind::Image(i) if i.keep_aspect);
        match d.corner {
            None => {
                let snap = |v: f32, size: f32, guide: &mut f32| {
                    for (pos, line) in [(0.0, 0.0), (0.5 - size / 2.0, 0.5), (1.0 - size, 1.0)] {
                        if (v - pos).abs() < SNAP {
                            *guide = line;
                            return pos;
                        }
                    }
                    v
                };
                layer.x = snap(sx + dx, sw, &mut gx);
                layer.y = snap(sy + dy, sh, &mut gy);
            }
            Some((cx, cy)) => {
                let (mut x0, mut y0, mut x1, mut y1) = (sx, sy, sx + sw, sy + sh);
                if cx == 0.0 {
                    x0 = (x0 + dx).min(x1 - MIN_SIZE);
                } else {
                    x1 = (x1 + dx).max(x0 + MIN_SIZE);
                }
                if cy == 0.0 {
                    y0 = (y0 + dy).min(y1 - MIN_SIZE);
                } else {
                    y1 = (y1 + dy).max(y0 + MIN_SIZE);
                }
                if keep {
                    let h = (x1 - x0) * sh / sw;
                    if cy == 0.0 {
                        y0 = y1 - h;
                    } else {
                        y1 = y0 + h;
                    }
                }
                layer.x = x0;
                layer.y = y0;
                layer.w = x1 - x0;
                layer.h = y1 - y0;
            }
        }
        self.win.set_guide_x(gx);
        self.win.set_guide_y(gy);
        // The engine gets the result on release; while dragging only the editor redraws.
        self.edited(a, false);
    }

    fn release(&mut self, a: &mut App) {
        let Some(d) = self.drag.take() else { return };
        self.win.set_guide_x(-1.0);
        self.win.set_guide_y(-1.0);
        if d.moved {
            a.graphics_changed(true);
            self.reset_inspector(a);
        }
    }

    fn set_str(&mut self, a: &mut App, key: &str, v: &str) {
        let Some(layer) = self.selected.and_then(|id| layer_mut(a, id)) else { return };
        let color = Rgba::from_hex(v);
        match (key, &mut layer.kind) {
            ("name", _) => layer.name = v.to_string(),
            ("text", LayerKind::Text(t)) => t.text = v.to_string(),
            ("font", LayerKind::Text(t)) => t.font = v.to_string(),
            ("color", LayerKind::Text(t)) if color.is_some() => t.color = color.unwrap(),
            ("outline-color", LayerKind::Text(t)) if color.is_some() => t.outline_color = color.unwrap(),
            ("box-color", LayerKind::Text(t)) if color.is_some() => t.box_color = color.unwrap(),
            ("api-url", LayerKind::Text(t)) => t.api.get_or_insert_with(ApiSource::default).url = v.to_string(),
            ("api-path", LayerKind::Text(t)) => t.api.get_or_insert_with(ApiSource::default).json_path = v.to_string(),
            _ => return,
        }
        self.edited(a, true);
    }

    fn set_num(&mut self, a: &mut App, key: &str, v: f32) {
        let spec = a.project.tx.mode.spec();
        let (cw, ch) = (spec.width as f32, spec.height as f32);
        let Some(layer) = self.selected.and_then(|id| layer_mut(a, id)) else { return };
        match (key, &mut layer.kind) {
            ("x", _) => layer.x = v / cw,
            ("y", _) => layer.y = v / ch,
            ("w", _) => layer.w = (v / cw).max(MIN_SIZE),
            ("h", _) => layer.h = (v / ch).max(MIN_SIZE),
            ("opacity", _) => layer.opacity = v.clamp(0.0, 1.0),
            ("size", LayerKind::Text(t)) => t.size = v.clamp(0.02, 0.6),
            ("outline", LayerKind::Text(t)) => t.outline = v.clamp(0.0, 0.5),
            ("align", LayerKind::Text(t)) => t.align = align_of(v),
            ("valign", LayerKind::Text(t)) => t.valign = align_of(v),
            ("api-refresh", LayerKind::Text(t)) => t.api.get_or_insert_with(ApiSource::default).refresh_s = v.max(10.0) as u32,
            _ => return,
        }
        self.edited(a, true);
    }

    fn set_bool(&mut self, a: &mut App, key: &str, v: bool) {
        let Some(layer) = self.selected.and_then(|id| layer_mut(a, id)) else { return };
        match (key, &mut layer.kind) {
            ("bold", LayerKind::Text(t)) => t.bold = v,
            ("shrink", LayerKind::Text(t)) => t.shrink_to_fit = v,
            ("shadow", LayerKind::Text(t)) => t.shadow = v,
            ("api", LayerKind::Text(t)) => {
                t.api = v.then(ApiSource::default);
                if v && !t.text.contains("{api}") {
                    t.text.push_str(" {api}");
                }
            }
            ("keep-aspect", LayerKind::Image(i)) => i.keep_aspect = v,
            _ => return,
        }
        self.edited(a, true);
    }

    fn api_test(&mut self, a: &mut App) {
        let src = self.selected.and_then(|id| a.project.graphics.layers.iter().find(|l| l.id == id)).and_then(|l| match &l.kind {
            LayerKind::Text(t) => t.api.clone(),
            LayerKind::Image(_) => None,
        });
        let Some(src) = src.filter(|s| !s.url.trim().is_empty()) else {
            self.win.set_api_result(i18n::ts("URL is missing.", "Falta la URL.").into());
            return;
        };
        self.win.set_api_result(i18n::ts("Fetching…", "Consultando…").into());
        let cache = a.composer.api.clone();
        let win = self.win.as_weak();
        let flag = self.api_updated.clone();
        std::thread::spawn(move || {
            let text = match cache.fetch_now(&src) {
                Ok(v) => format!("{}: {v}", i18n::ts("Response", "Respuesta")),
                Err(e) => format!("{}: {e}", i18n::ts("Error", "Error")),
            };
            flag.store(true, Ordering::Relaxed);
            let _ = win.upgrade_in_event_loop(move |w| w.set_api_result(text.into()));
        });
    }
}

pub fn image_picked_mut(a: &mut App, new_layer: bool, path: &std::path::Path) {
    if let Some(mut ed) = a.editor.take() {
        if new_layer {
            ed.add_image(a, path);
        } else {
            let stored = a.relativize(path);
            if let Some(l) = ed.selected.and_then(|id| layer_mut(a, id)) {
                if let LayerKind::Image(i) = &mut l.kind {
                    i.path = stored;
                    ed.edited(a, true);
                }
            }
        }
        a.editor = Some(ed);
    }
}
