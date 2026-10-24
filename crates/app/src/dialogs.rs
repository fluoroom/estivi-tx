//! Grid block, project and hardware dialogs. Each opens a fresh window instance, so
//! its fields start from the current values every time.

use std::path::PathBuf;
use std::rc::{Rc, Weak};
use std::cell::RefCell;

use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};

use estivi_core::config::{Config, OutChannel, PttMethod, SerialLine};
use estivi_core::encoder::ColorRange;
use estivi_core::engine::Command;
use estivi_core::modes::Mode;
use estivi_core::project::{GridTz, Slot, Trigger, TriggerAction, TriggerSource, parse_hhmm};
use estivi_core::{audio, ptt};

use crate::app::{App, Shared, with_app};
use crate::view;
use crate::app::{
    apply_picked, apply_theme_to, bind_files, hook_os_drops, is_image, paths_model, pick_native, NativePick, PICK_SAVE,
};
use crate::{
    ConfigDialog, ConfigForm, ImageItem, ModeCheat, PathDialog, ProgramDialog, ProgramForm, ProjectDialog, ProjectForm,
    RenameDialog, SlotDialog, SlotForm, TriggerItem, UtilsDialog,
};

#[derive(Default)]
pub struct Dialogs {
    slot: Option<SlotDialog>,
    project: Option<ProjectDialog>,
    program: Option<ProgramDialog>,
    path: Option<PathDialog>,
    triggers: Vec<Trigger>,
    trigger_model: Option<Rc<VecModel<TriggerItem>>>,
    activations: Vec<Trigger>,
    activation_model: Option<Rc<VecModel<TriggerItem>>>,
    config: Option<ConfigDialog>,
    rename: Option<RenameDialog>,
    utils: Option<UtilsDialog>,
    /// Device ids behind each combo box row, and what was detected.
    outs: Vec<String>,
    ins: Vec<String>,
    outs_found: Vec<audio::DeviceInfo>,
    ins_found: Vec<audio::DeviceInfo>,
    ports: Vec<String>,
}

impl Dialogs {
    pub fn apply_theme(&self, light: bool) {
        if let Some(dlg) = &self.utils {
            apply_theme_to(&dlg.global::<crate::Theme>(), light);
        }
    }
}

fn strings(v: Vec<SharedString>) -> ModelRc<SharedString> {
    ModelRc::new(VecModel::from(v))
}

// ───────────────────────── Grid block ─────────────────────────

pub fn open_slot(app: &Shared, index: Option<usize>) {
    let w: Weak<RefCell<App>> = Rc::downgrade(app);
    let mut a = app.borrow_mut();
    let existing = index.and_then(|i| a.project.grid.slots.get(i).cloned());
    let slot = existing.clone().unwrap_or_default();
    let Ok(dlg) = SlotDialog::new() else { return };
    apply_theme_to(&dlg.global::<crate::Theme>(), a.config.ui.light);
    dlg.set_program_names(strings(view::program_names_with_rotation(&a.project)));
    dlg.set_tz_name(if a.project.grid.timezone == GridTz::Utc { "UTC".into() } else { "hora local".into() });
    let program_index = slot.program.and_then(|id| a.project.programs.iter().position(|p| p.id == id)).map_or(0, |i| i + 1);
    let d = slot.days;
    dlg.set_form(SlotForm {
        name: slot.name.as_str().into(),
        enabled: slot.enabled,
        d0: d[0],
        d1: d[1],
        d2: d[2],
        d3: d[3],
        d4: d[4],
        d5: d[5],
        d6: d[6],
        start: slot.start.as_str().into(),
        end: slot.end.as_str().into(),
        repeat: slot.every_min > 0 && !slot.end.trim().is_empty(),
        every: slot.every_min.max(1) as i32,
        program: program_index as i32,
        freq: if slot.freq_hz > 0 { format!("{:.3}", slot.freq_hz as f64 / 1e6).into() } else { "".into() },
    });
    let id = existing.map(|s| s.id);
    dlg.on_cancel({
        let d = dlg.as_weak();
        move || {
            if let Some(d) = d.upgrade() {
                let _ = d.hide();
            }
        }
    });
    dlg.on_save({
        let d = dlg.as_weak();
        move |f| {
            let Some(dlg) = d.upgrade() else { return };
            with_app(&w, |a| match slot_from_form(a, &f, id) {
                Ok(slot) => {
                    match a.project.grid.slots.iter_mut().find(|s| Some(s.id) == id) {
                        Some(s) => *s = slot,
                        None => a.project.grid.slots.push(slot),
                    }
                    a.changed();
                    let _ = dlg.hide();
                }
                Err(e) => dlg.set_error(e.into()),
            });
        }
    });
    let _ = dlg.show();
    a.dialogs.slot = Some(dlg);
}

fn slot_from_form(a: &App, f: &SlotForm, id: Option<u64>) -> Result<Slot, String> {
    if parse_hhmm(&f.start).is_none() {
        return Err("La hora de comienzo tiene que ser HH:MM (por ejemplo 21:00).".into());
    }
    if f.repeat && parse_hhmm(&f.end).is_none() {
        return Err("La hora «hasta» tiene que ser HH:MM.".into());
    }
    let freq_hz = view::parse_mhz(&f.freq).ok_or("La frecuencia va en MHz, por ejemplo 14.230.")?;
    let days = [f.d0, f.d1, f.d2, f.d3, f.d4, f.d5, f.d6];
    if !days.iter().any(|&d| d) {
        return Err("Elegí al menos un día.".into());
    }
    let program = (f.program > 0).then(|| a.project.programs.get(f.program as usize - 1).map(|p| p.id)).flatten();
    Ok(Slot {
        id: id.unwrap_or_else(|| a.project.next_slot_id()),
        enabled: f.enabled,
        name: if f.name.trim().is_empty() { "Bloque".into() } else { f.name.trim().to_string() },
        days,
        start: f.start.trim().to_string(),
        end: if f.repeat { f.end.trim().to_string() } else { String::new() },
        every_min: if f.repeat { f.every.max(1) as u32 } else { 0 },
        program,
        freq_hz,
    })
}

// ───────────────────────── Program name ─────────────────────────

pub fn rename_program(app: &Shared, index: usize) {
    let w: Weak<RefCell<App>> = Rc::downgrade(app);
    let mut a = app.borrow_mut();
    let Some(program) = a.project.programs.get(index).cloned() else { return };
    let Ok(dlg) = RenameDialog::new() else { return };
    dlg.set_heading("Nombre del programa".into());
    apply_theme_to(&dlg.global::<crate::Theme>(), a.config.ui.light);
    dlg.set_value(program.name.as_str().into());
    dlg.on_cancel({
        let d = dlg.as_weak();
        move || {
            if let Some(d) = d.upgrade() {
                let _ = d.hide();
            }
        }
    });
    dlg.on_save({
        let d = dlg.as_weak();
        move |name| {
            with_app(&w, |a| {
                if let Some(p) = a.project.programs.iter_mut().find(|p| p.id == program.id) {
                    if !name.trim().is_empty() && p.name != name.trim() {
                        p.name = name.trim().to_string();
                        a.changed();
                    }
                }
            });
            if let Some(d) = d.upgrade() {
                let _ = d.hide();
            }
        }
    });
    let _ = dlg.show();
    a.dialogs.rename = Some(dlg);
}

// ───────────────────────── Project ─────────────────────────

fn trigger_item(t: &Trigger, a: &App) -> TriggerItem {
    TriggerItem {
        enabled: t.enabled,
        source: (t.source == TriggerSource::Cw) as i32,
        code: t.code.as_str().into(),
        action: match t.action {
            TriggerAction::Transmit => 0,
            TriggerAction::Pause => 1,
            TriggerAction::Resume => 2,
        },
        program: t.program.and_then(|id| a.project.programs.iter().position(|p| p.id == id)).map_or(0, |i| i as i32 + 1),
    }
}

fn trigger_from_item(item: &TriggerItem, a: &App) -> Trigger {
    Trigger {
        enabled: item.enabled,
        source: if item.source == 1 { TriggerSource::Cw } else { TriggerSource::Dtmf },
        code: item.code.trim().to_string(),
        action: match item.action {
            1 => TriggerAction::Pause,
            2 => TriggerAction::Resume,
            _ => TriggerAction::Transmit,
        },
        program: (item.program > 0).then(|| a.project.programs.get(item.program as usize - 1).map(|p| p.id)).flatten(),
    }
}

pub fn open_project(app: &Shared) {
    let w: Weak<RefCell<App>> = Rc::downgrade(app);
    let mut a = app.borrow_mut();
    let Ok(dlg) = ProjectDialog::new() else { return };
    apply_theme_to(&dlg.global::<crate::Theme>(), a.config.ui.light);
    let p = &a.project;
    dlg.set_form(ProjectForm {
        name: p.name.as_str().into(),
        call: p.station.call.as_str().into(),
        operator: p.station.operator.as_str().into(),
        qth: p.station.qth.as_str().into(),
        locator: p.station.locator.as_str().into(),
        color_range: (p.tx.color_range == ColorRange::Studio) as i32,
        vox: p.tx.vox_preamble,
        fsk: p.tx.fsk_id,
        cw_preamble: p.tx.cw_preamble,
        cw: p.tx.cw_id,
        cw_text: p.tx.cw_text.as_str().into(),
        cw_wpm: p.tx.cw_wpm.round() as i32,
        cw_tone: p.tx.cw_tone_hz.round() as i32,
        pause: p.tx.min_pause_s as i32,
        listen_cw: p.listen.cw,
        listen_wpm: p.listen.cw_wpm.round() as i32,
        listen_tone: p.listen.cw_tone_hz.round() as i32,
        listen_dtmf: p.listen.dtmf,
    });
    let mut names = vec![SharedString::from("Próximo de la rotación")];
    names.extend(p.programs.iter().map(|p| SharedString::from(p.name.as_str())));
    dlg.set_program_names(strings(names));
    let triggers = p.listen.triggers.clone();
    let model = Rc::new(VecModel::from(triggers.iter().map(|t| trigger_item(t, &a)).collect::<Vec<_>>()));
    dlg.set_triggers(model.clone().into());

    dlg.on_trigger_add({
        let w = w.clone();
        move || {
            with_app(&w, |a| {
                let t = Trigger { enabled: true, ..Default::default() };
                let item = trigger_item(&t, a);
                a.dialogs.triggers.push(t);
                if let Some(m) = &a.dialogs.trigger_model {
                    m.push(item);
                }
            })
        }
    });
    dlg.on_trigger_remove({
        let w = w.clone();
        move |i| {
            with_app(&w, |a| {
                let i = i as usize;
                if i < a.dialogs.triggers.len() {
                    a.dialogs.triggers.remove(i);
                    if let Some(m) = &a.dialogs.trigger_model {
                        m.remove(i);
                    }
                }
            })
        }
    });
    dlg.on_trigger_edit({
        let w = w.clone();
        move |i, item| {
            with_app(&w, |a| {
                let t = trigger_from_item(&item, a);
                if let Some(slot) = a.dialogs.triggers.get_mut(i as usize) {
                    *slot = t;
                    if let Some(m) = &a.dialogs.trigger_model {
                        if (i as usize) < m.row_count() {
                            m.set_row_data(i as usize, item);
                        }
                    }
                }
            })
        }
    });
    dlg.on_cancel({
        let d = dlg.as_weak();
        move || {
            if let Some(d) = d.upgrade() {
                let _ = d.hide();
            }
        }
    });
    dlg.on_save({
        let d = dlg.as_weak();
        move |f| {
            let Some(dlg) = d.upgrade() else { return };
            with_app(&w, |a| {
                if f.call.trim().is_empty() && f.cw {
                    dlg.set_error("Para identificarse en CW hace falta el indicativo.".into());
                    return;
                }
                let p = &mut a.project;
                p.name = if f.name.trim().is_empty() { "Canal SSTV".into() } else { f.name.trim().to_string() };
                p.station.call = f.call.trim().to_uppercase();
                p.station.operator = f.operator.trim().to_string();
                p.station.qth = f.qth.trim().to_string();
                p.station.locator = f.locator.trim().to_string();
                p.tx.color_range = if f.color_range == 1 { ColorRange::Studio } else { ColorRange::Full };
                p.tx.vox_preamble = f.vox;
                p.tx.fsk_id = f.fsk;
                p.tx.cw_preamble = f.cw_preamble;
                p.tx.cw_id = f.cw;
                p.tx.cw_text = f.cw_text.trim().to_string();
                p.tx.cw_wpm = f.cw_wpm as f32;
                p.tx.cw_tone_hz = f.cw_tone as f32;
                p.tx.min_pause_s = f.pause.max(0) as u32;
                p.listen.cw = f.listen_cw;
                p.listen.cw_wpm = f.listen_wpm as f32;
                p.listen.cw_tone_hz = f.listen_tone as f32;
                p.listen.dtmf = f.listen_dtmf;
                p.listen.triggers = a.dialogs.triggers.iter().filter(|t| !t.code.is_empty()).cloned().collect();
                a.changed();
                let _ = dlg.hide();
            });
        }
    });
    let _ = dlg.show();
    a.dialogs.triggers = triggers;
    a.dialogs.trigger_model = Some(model);
    a.dialogs.project = Some(dlg);
}

// ───────────────────────── Hardware ─────────────────────────

const RATES: [u32; 6] = [0, 8000, 11025, 22050, 44100, 48000];

/// Ids for a device combo box: index 0 is the system default (""). A configured device
/// that is not connected right now stays in the list so the setting is not lost.
fn device_list(found: &[audio::DeviceInfo], configured: &str) -> Vec<String> {
    let mut list = vec![String::new()];
    list.extend(found.iter().map(|d| d.id.clone()));
    if !configured.is_empty() && !found.iter().any(|d| d.id == configured || d.name == configured) {
        list.push(configured.to_string());
    }
    list
}

fn device_labels(list: &[String], found: &[audio::DeviceInfo]) -> ModelRc<SharedString> {
    strings(
        list.iter()
            .map(|id| match found.iter().find(|d| d.id == *id) {
                _ if id.is_empty() => "Predeterminada del sistema".into(),
                Some(d) => d.label().into(),
                None => format!("{id} (no conectado)").into(),
            })
            .collect(),
    )
}

fn fill_lists(a: &mut App, dlg: &ConfigDialog) {
    let outs = audio::output_devices();
    let ins = audio::input_devices();
    let ports_found = ptt::serial_ports();
    a.dialogs.outs = device_list(&outs, &a.config.audio.output);
    a.dialogs.ins = device_list(&ins, &a.config.audio.input);
    a.dialogs.ports = std::iter::once(String::new()).chain(ports_found.iter().cloned()).collect();
    dlg.set_out_devices(device_labels(&a.dialogs.outs, &outs));
    dlg.set_in_devices(device_labels(&a.dialogs.ins, &ins));
    a.dialogs.outs_found = outs;
    a.dialogs.ins_found = ins;
    let none = if ports_found.is_empty() { "(no se detectaron puertos)" } else { "(elegir)" };
    dlg.set_ports(strings(a.dialogs.ports.iter().map(|p| if p.is_empty() { none.into() } else { p.as_str().into() }).collect()));
}

/// Position of the configured device (matching an id, or a legacy name) in `list`.
fn device_index(list: &[String], found: &[audio::DeviceInfo], configured: &str) -> usize {
    let id = found.iter().find(|d| d.name == configured).map_or(configured, |d| d.id.as_str());
    list.iter().position(|d| d == configured || d == id).unwrap_or(0)
}

fn form_of(a: &App) -> ConfigForm {
    let c = &a.config;
    let port_index = a.dialogs.ports.iter().position(|p| !p.is_empty() && *p == c.ptt.port);
    ConfigForm {
        out_index: device_index(&a.dialogs.outs, &a.dialogs.outs_found, &c.audio.output) as i32,
        in_index: device_index(&a.dialogs.ins, &a.dialogs.ins_found, &c.audio.input) as i32,
        rate_index: RATES.iter().position(|&r| r == c.audio.sample_rate).unwrap_or(0) as i32,
        channel: match c.audio.channel {
            OutChannel::Both => 0,
            OutChannel::Left => 1,
            OutChannel::Right => 2,
        },
        level: c.audio.level * 100.0,
        ppm: if c.audio.tx_ppm == 0.0 { "0".into() } else { format!("{}", c.audio.tx_ppm).into() },
        ptt_method: match c.ptt.method {
            PttMethod::None => 0,
            PttMethod::Vox => 1,
            PttMethod::Serial => 2,
            PttMethod::Cat => 3,
        },
        port_index: port_index.unwrap_or(0) as i32,
        port_manual: if port_index.is_none() { c.ptt.port.as_str().into() } else { "".into() },
        line: match c.ptt.line {
            SerialLine::Rts => 0,
            SerialLine::Dtr => 1,
            SerialLine::Both => 2,
        },
        invert: c.ptt.invert,
        lead: c.ptt.lead_ms as i32,
        tail: c.ptt.tail_ms as i32,
        timeout: c.ptt.timeout_s as i32,
        cat_host: c.cat.host.as_str().into(),
        cat_port: c.cat.port as i32,
        cat_freq: c.cat.set_freq,
        autostart: c.ui.autostart,
        gpu: c.ui.gpu_render,
        light: c.ui.light,
    }
}

fn config_from_form(a: &App, f: &ConfigForm) -> Result<Config, String> {
    let mut c = a.config.clone();
    let pick = |list: &[String], i: i32| list.get(i.max(0) as usize).cloned().unwrap_or_default();
    c.audio.output = pick(&a.dialogs.outs, f.out_index);
    c.audio.input = pick(&a.dialogs.ins, f.in_index);
    c.audio.sample_rate = RATES.get(f.rate_index.max(0) as usize).copied().unwrap_or(0);
    c.audio.channel = match f.channel {
        1 => OutChannel::Left,
        2 => OutChannel::Right,
        _ => OutChannel::Both,
    };
    c.audio.level = (f.level / 100.0).clamp(0.0, 1.0);
    let ppm = f.ppm.trim().replace(',', ".");
    c.audio.tx_ppm = if ppm.is_empty() { 0.0 } else { ppm.parse::<f64>().map_err(|_| "La corrección del reloj tiene que ser un número (ppm).")? };
    if c.audio.tx_ppm.abs() > 5000.0 {
        return Err("La corrección del reloj parece demasiado grande (más de 5000 ppm).".into());
    }
    c.ptt.method = match f.ptt_method {
        0 => PttMethod::None,
        2 => PttMethod::Serial,
        3 => PttMethod::Cat,
        _ => PttMethod::Vox,
    };
    c.ptt.port = if !f.port_manual.trim().is_empty() { f.port_manual.trim().to_string() } else { pick(&a.dialogs.ports, f.port_index) };
    if c.ptt.method == PttMethod::Serial && c.ptt.port.is_empty() {
        return Err("Elegí el puerto serie del PTT.".into());
    }
    c.ptt.line = match f.line {
        1 => SerialLine::Dtr,
        2 => SerialLine::Both,
        _ => SerialLine::Rts,
    };
    c.ptt.invert = f.invert;
    c.ptt.lead_ms = f.lead.max(0) as u32;
    c.ptt.tail_ms = f.tail.max(0) as u32;
    c.ptt.timeout_s = f.timeout.max(30) as u32;
    c.cat.host = if f.cat_host.trim().is_empty() { "127.0.0.1".into() } else { f.cat_host.trim().to_string() };
    c.cat.port = f.cat_port.clamp(1, 65535) as u16;
    c.cat.set_freq = f.cat_freq;
    c.ui.autostart = f.autostart;
    c.ui.gpu_render = f.gpu;
    c.ui.light = f.light;
    Ok(c)
}

pub fn open_config(app: &Shared) {
    let w: Weak<RefCell<App>> = Rc::downgrade(app);
    let mut a = app.borrow_mut();
    let Ok(dlg) = ConfigDialog::new() else { return };
    apply_theme_to(&dlg.global::<crate::Theme>(), a.config.ui.light);
    fill_lists(&mut a, &dlg);
    dlg.set_form(form_of(&a));
    dlg.set_config_path(a.config_path.to_string_lossy().as_ref().into());

    let test = |w: Weak<RefCell<App>>, d: slint::Weak<ConfigDialog>, cmd: fn() -> Command, msg: &'static str| {
        move |f: ConfigForm| {
            let Some(dlg) = d.upgrade() else { return };
            with_app(&w, |a| match config_from_form(a, &f) {
                Ok(c) => {
                    a.engine.send(Command::SetConfig(Box::new(c)));
                    a.engine.send(cmd());
                    dlg.set_error("".into());
                    dlg.set_test_state(msg.into());
                }
                Err(e) => dlg.set_error(e.into()),
            });
        }
    };
    dlg.on_test_tone(test(w.clone(), dlg.as_weak(), || Command::TestTone, "Tono de 1900 Hz durante 5 s. Si algo falla, el error aparece en la ventana principal."));
    dlg.on_test_ptt(test(w.clone(), dlg.as_weak(), || Command::TestPtt, "PTT activado 2 s. Si algo falla, el error aparece en la ventana principal."));
    dlg.on_refresh({
        let (w, d) = (w.clone(), dlg.as_weak());
        move || {
            let Some(dlg) = d.upgrade() else { return };
            with_app(&w, |a| {
                fill_lists(a, &dlg);
                dlg.set_form(form_of(a));
            });
        }
    });
    dlg.on_cancel({
        let (w, d) = (w.clone(), dlg.as_weak());
        move || {
            // Undo whatever a test applied.
            with_app(&w, |a| a.engine.send(Command::SetConfig(Box::new(a.config.clone()))));
            if let Some(d) = d.upgrade() {
                let _ = d.hide();
            }
        }
    });
    dlg.on_save({
        let (w, d) = (w.clone(), dlg.as_weak());
        move |f| {
            let Some(dlg) = d.upgrade() else { return };
            with_app(&w, |a| match config_from_form(a, &f) {
                Ok(c) => {
                    a.config = c;
                    apply_theme_to(&a.ui().global::<crate::Theme>(), a.config.ui.light);
                    a.engine.send(Command::SetConfig(Box::new(a.config.clone())));
                    match a.config.save(&a.config_path) {
                        Ok(()) => {
                            let _ = dlg.hide();
                        }
                        Err(e) => dlg.set_error(format!("No se pudo guardar: {e:#}").into()),
                    }
                }
                Err(e) => dlg.set_error(e.into()),
            });
        }
    });
    let _ = dlg.show();
    a.dialogs.config = Some(dlg);
}

// ───────────────────────── Program ─────────────────────────

fn image_item(a: &mut App, img: &estivi_core::project::ProgramImage) -> ImageItem {
    let abs = a.resolve(&img.path);
    let thumb = a.thumb(&abs);
    ImageItem {
        path: img.path.as_str().into(),
        name: abs.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default().into(),
        enabled: img.enabled,
        missing: thumb.is_none(),
        thumb: thumb.unwrap_or_default(),
        mode_index: view::mode_index(img.mode),
    }
}

pub fn open_program(app: &Shared, index: Option<usize>) {
    let w: Weak<RefCell<App>> = Rc::downgrade(app);
    let mut a = app.borrow_mut();
    let existing = index.and_then(|i| a.project.programs.get(i).cloned());
    let program = existing.clone().unwrap_or_else(|| {
        let id = a.project.next_program_id();
        estivi_core::project::Program { id, name: "Programa".into(), ..Default::default() }
    });
    let Ok(dlg) = ProgramDialog::new() else { return };
    bind_files(&dlg.global::<crate::Files>());
    {
        let hover = dlg.as_weak();
        let drop = dlg.as_weak();
        hook_os_drops(
            dlg.window(),
            move |on| {
                if let Some(dlg) = hover.upgrade() {
                    dlg.global::<crate::Files>().set_hovering(on);
                }
            },
            move |paths| {
                if let Some(dlg) = drop.upgrade() {
                    dlg.invoke_images_dropped(paths_model(&paths));
                }
            },
        );
    }
    apply_theme_to(&dlg.global::<crate::Theme>(), a.config.ui.light);
    dlg.set_form(ProgramForm {
        name: program.name.as_str().into(),
        enabled: program.enabled,
        rotation: match program.rotation {
            estivi_core::project::Rotation::Sequential => 0,
            estivi_core::project::Rotation::Shuffle => 1,
        },
        inherit_cw: program.cw_preamble.is_none() && program.cw_id.is_none(),
        cw_preamble: program.cw_preamble.unwrap_or(a.project.tx.cw_preamble),
        cw: program.cw_id.unwrap_or(a.project.tx.cw_id),
        cw_text: program.cw_text.as_str().into(),
        mode: view::mode_index(program.mode),
    });
    dlg.set_mode_names(ModelRc::new(VecModel::from(view::mode_choices("Del proyecto"))));
    dlg.set_image_modes(ModelRc::new(VecModel::from(view::mode_choices("Del programa"))));
    let images: Vec<ImageItem> = program.images.iter().map(|img| image_item(&mut a, img)).collect();
    let image_model = Rc::new(VecModel::from(images));
    dlg.set_images(image_model.clone().into());
    let activations = program.activations.clone();
    let act_model = Rc::new(VecModel::from(activations.iter().map(|t| trigger_item(t, &a)).collect::<Vec<_>>()));
    dlg.set_activations(act_model.clone().into());

    let creating = existing.is_none();
    let id = existing.as_ref().map(|p| p.id);
    if creating {
        a.project.programs.push(program.clone());
        a.changed();
    }
    let program_id = program.id;

    dlg.on_pick_images({
        let d = dlg.as_weak();
        move || {
            let d = d.clone();
            pick_native(NativePick::Images { many: true }, move |paths| {
                if let Some(dlg) = d.upgrade() {
                    dlg.invoke_images_dropped(paths_model(&paths));
                }
            });
        }
    });
    dlg.on_images_dropped({
        let w = w.clone();
        move |paths| {
            with_app(&w, |a| {
                let added: Vec<_> = paths
                    .iter()
                    .map(|p| PathBuf::from(p.as_str()))
                    .filter(|p| p.exists() && is_image(p))
                    .collect();
                let stored: Vec<String> = added.iter().map(|f| a.relativize(f)).collect();
                if let Some(p) = a.project.programs.iter_mut().find(|p| p.id == program_id) {
                    p.images.extend(stored.into_iter().map(estivi_core::project::ProgramImage::new));
                }
                a.changed();
                refresh_program_images(a, program_id);
            })
        }
    });
    dlg.on_image_remove({
        let w = w.clone();
        move |i| {
            with_app(&w, |a| {
                if let Some(p) = a.project.programs.iter_mut().find(|p| p.id == program_id) {
                    if (i as usize) < p.images.len() {
                        p.images.remove(i as usize);
                    }
                }
                a.changed();
                refresh_program_images(a, program_id);
            })
        }
    });
    dlg.on_image_move({
        let w = w.clone();
        move |i, delta| {
            with_app(&w, |a| {
                if let Some(p) = a.project.programs.iter_mut().find(|p| p.id == program_id) {
                    let (i, j) = (i as usize, (i + delta) as usize);
                    if i < p.images.len() && j < p.images.len() {
                        p.images.swap(i, j);
                    }
                }
                a.changed();
                refresh_program_images(a, program_id);
            })
        }
    });
    dlg.on_image_mode({
        let w = w.clone();
        move |i, mode| {
            with_app(&w, |a| {
                if let Some(p) = a.project.programs.iter_mut().find(|p| p.id == program_id) {
                    if let Some(img) = p.images.get_mut(i as usize) {
                        let next = view::mode_at(mode);
                        if img.mode != next {
                            img.mode = next;
                            a.changed();
                        }
                    }
                }
            })
        }
    });
    dlg.on_image_toggle({
        let w = w.clone();
        move |i, on| {
            with_app(&w, |a| {
                if let Some(p) = a.project.programs.iter_mut().find(|p| p.id == program_id) {
                    if let Some(img) = p.images.get_mut(i as usize) {
                        img.enabled = on;
                    }
                }
                a.changed();
            })
        }
    });
    dlg.on_activation_add({
        let w = w.clone();
        move || {
            with_app(&w, |a| {
                let t = Trigger { enabled: true, action: TriggerAction::Transmit, program: Some(program_id), ..Default::default() };
                let item = trigger_item(&t, a);
                a.dialogs.activations.push(t);
                if let Some(m) = &a.dialogs.activation_model {
                    m.push(item);
                }
            })
        }
    });
    dlg.on_activation_remove({
        let w = w.clone();
        move |i| {
            with_app(&w, |a| {
                let i = i as usize;
                if i < a.dialogs.activations.len() {
                    a.dialogs.activations.remove(i);
                    if let Some(m) = &a.dialogs.activation_model {
                        m.remove(i);
                    }
                }
            })
        }
    });
    dlg.on_activation_edit({
        let w = w.clone();
        move |i, item| {
            with_app(&w, |a| {
                let mut t = trigger_from_item(&item, a);
                t.program = Some(program_id);
                if let Some(slot) = a.dialogs.activations.get_mut(i as usize) {
                    *slot = t;
                    if let Some(m) = &a.dialogs.activation_model {
                        if (i as usize) < m.row_count() {
                            m.set_row_data(i as usize, item);
                        }
                    }
                }
            })
        }
    });
    dlg.on_cancel({
        let (w, d) = (w.clone(), dlg.as_weak());
        move || {
            if creating {
                with_app(&w, |a| {
                    if let Some(i) = a.project.programs.iter().position(|p| p.id == program_id) {
                        if a.project.programs[i].images.is_empty() && a.project.programs[i].name == "Programa" {
                            a.project.programs.remove(i);
                            a.changed();
                        }
                    }
                });
            }
            if let Some(d) = d.upgrade() {
                let _ = d.hide();
            }
        }
    });
    dlg.on_save({
        let (w, d) = (w.clone(), dlg.as_weak());
        move |f| {
            let Some(dlg) = d.upgrade() else { return };
            with_app(&w, |a| match program_from_form(a, &f, program_id) {
                Ok(()) => {
                    a.changed();
                    let _ = dlg.hide();
                }
                Err(e) => dlg.set_error(e.into()),
            });
        }
    });
    let _ = dlg.show();
    a.dialogs.activations = activations;
    a.dialogs.activation_model = Some(act_model);
    a.dialogs.program = Some(dlg);
    let _ = id;
}

fn refresh_program_images(a: &mut App, program_id: u64) {
    let imgs = a.project.program(program_id).map(|p| p.images.clone()).unwrap_or_default();
    let items: Vec<ImageItem> = imgs.iter().map(|img| image_item(a, img)).collect();
    if let Some(dlg) = a.dialogs.program.as_ref() {
        dlg.set_images(Rc::new(VecModel::from(items)).into());
    }
}

fn program_from_form(a: &mut App, f: &ProgramForm, id: u64) -> Result<(), String> {
    let activations = a.dialogs.activations.iter().filter(|t| !t.code.is_empty()).cloned().collect();
    let Some(p) = a.project.programs.iter_mut().find(|p| p.id == id) else {
        return Err("El programa ya no existe.".into());
    };
    p.name = if f.name.trim().is_empty() { "Programa".into() } else { f.name.trim().to_string() };
    p.enabled = f.enabled;
    p.rotation = if f.rotation == 1 { estivi_core::project::Rotation::Shuffle } else { estivi_core::project::Rotation::Sequential };
    if f.inherit_cw {
        p.cw_preamble = None;
        p.cw_id = None;
        p.cw_text.clear();
    } else {
        p.cw_preamble = Some(f.cw_preamble);
        p.cw_id = Some(f.cw);
        p.cw_text = f.cw_text.trim().to_string();
    }
    p.mode = view::mode_at(f.mode);
    p.slots.clear();
    p.activations = activations;
    Ok(())
}

pub fn open_utils(app: &Shared) {
    let mut a = app.borrow_mut();
    if let Some(dlg) = &a.dialogs.utils {
        let _ = dlg.show();
        return;
    }
    let Ok(dlg) = UtilsDialog::new() else { return };
    apply_theme_to(&dlg.global::<crate::Theme>(), a.config.ui.light);
    let rows: Vec<ModeCheat> = Mode::ALL
        .iter()
        .map(|m| {
            let s = m.spec();
            ModeCheat {
                name: s.name.into(),
                size: s.size_label().into(),
                color: s.color_space().into(),
                time: view::duration(s.image_ms()).into(),
                vis: format!("{:02X}", s.vis).into(),
            }
        })
        .collect();
    dlg.set_modes(Rc::new(VecModel::from(rows)).into());
    let d = dlg.as_weak();
    dlg.on_dismiss(move || {
        if let Some(d) = d.upgrade() {
            let _ = d.hide();
        }
    });
    let _ = dlg.show();
    a.dialogs.utils = Some(dlg);
}

pub fn ask_path(app: &Shared, heading: &str, hint: &str, accept: &str, purpose: &'static str) {
    let w: Weak<RefCell<App>> = Rc::downgrade(app);
    let mut a = app.borrow_mut();
    let Ok(dlg) = PathDialog::new() else { return };
    bind_files(&dlg.global::<crate::Files>());
    {
        let hover = dlg.as_weak();
        let drop = dlg.as_weak();
        hook_os_drops(
            dlg.window(),
            move |on| {
                if let Some(dlg) = hover.upgrade() {
                    dlg.global::<crate::Files>().set_hovering(on);
                }
            },
            move |paths| {
                if let (Some(dlg), Some(p)) = (drop.upgrade(), paths.first()) {
                    dlg.set_path(p.to_string_lossy().into_owned().into());
                }
            },
        );
    }
    apply_theme_to(&dlg.global::<crate::Theme>(), a.config.ui.light);
    dlg.set_heading(heading.into());
    dlg.set_hint(hint.into());
    dlg.set_accept(accept.into());
    dlg.set_path(a.path.as_ref().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default().into());
    dlg.on_cancel({
        let d = dlg.as_weak();
        move || {
            if let Some(d) = d.upgrade() {
                let _ = d.hide();
            }
        }
    });
    dlg.on_browse({
        let d = dlg.as_weak();
        move || {
            let d = d.clone();
            let kind = if purpose == PICK_SAVE { NativePick::ProjectSave } else { NativePick::ProjectOpen };
            pick_native(kind, move |paths| {
                if let (Some(dlg), Some(p)) = (d.upgrade(), paths.first()) {
                    dlg.set_path(p.to_string_lossy().into_owned().into());
                }
            });
        }
    });
    dlg.on_save({
        let d = dlg.as_weak();
        move |path| {
            let Some(dlg) = d.upgrade() else { return };
            if path.trim().is_empty() {
                dlg.set_error("Soltá un archivo, elegilo o escribí la ruta.".into());
                return;
            }
            with_app(&w, |a| {
                apply_picked(a, purpose, vec![PathBuf::from(path.trim())]);
                let _ = dlg.hide();
            });
        }
    });
    let _ = dlg.show();
    a.dialogs.path = Some(dlg);
}
