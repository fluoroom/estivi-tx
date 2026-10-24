//! Sound card access through cpal (WASAPI on Windows, ALSA/PipeWire on Linux).
//!
//! cpal streams are not `Send` on every platform, so a stream must be created and
//! dropped on the same thread; the engine and the listener each own theirs.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use anyhow::{Context, anyhow};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, FromSample, SampleFormat, SizedSample, StreamConfig, SupportedStreamConfig};

use crate::config::{AudioConfig, OutChannel};
use crate::synth::SampleSource;

/// A sound device as shown to the user and stored in the config.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceInfo {
    /// Stable identifier (`alsa:front:CARD=Device,DEV=0`, a WASAPI id…). This is
    /// what the config stores: names repeat across variants of the same card.
    pub id: String,
    pub name: String,
}

impl DeviceInfo {
    pub fn label(&self) -> String {
        let short = self.id.split_once(':').map_or(self.id.as_str(), |(_, rest)| rest);
        if short.is_empty() || short == self.name { self.name.clone() } else { format!("{} ({short})", self.name) }
    }
}

fn info(d: &Device) -> Option<DeviceInfo> {
    let id = d.id().ok()?.to_string();
    let name = d.description().map(|desc| desc.name().to_string()).unwrap_or_else(|_| id.clone());
    Some(DeviceInfo { id, name })
}

fn unique(devices: impl Iterator<Item = Device>) -> Vec<DeviceInfo> {
    let mut out: Vec<DeviceInfo> = Vec::new();
    for i in devices.filter_map(|d| info(&d)) {
        // Some backends report the same device more than once.
        if !out.iter().any(|o| o.id == i.id) {
            out.push(i);
        }
    }
    out
}

pub fn output_devices() -> Vec<DeviceInfo> {
    cpal::default_host().output_devices().map(unique).unwrap_or_default()
}

pub fn input_devices() -> Vec<DeviceInfo> {
    cpal::default_host().input_devices().map(unique).unwrap_or_default()
}

/// `key` is a device id, or a name for hand-written configs; empty = system default.
fn find_device(key: &str, output: bool) -> anyhow::Result<Device> {
    let host = cpal::default_host();
    let key = key.trim();
    if key.is_empty() {
        let d = if output { host.default_output_device() } else { host.default_input_device() };
        return d.ok_or_else(|| anyhow!("no hay dispositivo de audio predeterminado"));
    }
    let devices: Vec<Device> = if output { host.output_devices()?.collect() } else { host.input_devices()?.collect() };
    let infos: Vec<Option<DeviceInfo>> = devices.iter().map(info).collect();
    let pos = infos
        .iter()
        .position(|i| i.as_ref().is_some_and(|i| i.id == key))
        .or_else(|| infos.iter().position(|i| i.as_ref().is_some_and(|i| i.name == key)))
        .ok_or_else(|| anyhow!("no se encuentra el dispositivo de audio «{key}» (¿está conectado?)"))?;
    Ok(devices.into_iter().nth(pos).expect("position within list"))
}

fn pick_config(device: &Device, rate: u32, output: bool) -> anyhow::Result<SupportedStreamConfig> {
    let default = if output { device.default_output_config()? } else { device.default_input_config()? };
    if rate == 0 || rate == default.sample_rate() {
        return Ok(default);
    }
    let ranges: Vec<_> = if output {
        device.supported_output_configs()?.collect()
    } else {
        device.supported_input_configs()?.collect()
    };
    ranges
        .into_iter()
        .filter(|r| r.sample_format() == default.sample_format())
        .find_map(|r| r.try_with_sample_rate(rate))
        .ok_or_else(|| anyhow!("el dispositivo no admite {rate} Hz"))
}

/// A running output. Dropping it stops the sound.
pub struct Playback {
    _stream: cpal::Stream,
    pub sample_rate: u32,
    played: Arc<AtomicU64>,
    finished: Arc<AtomicBool>,
}

impl Playback {
    pub fn played_samples(&self) -> u64 {
        self.played.load(Ordering::Relaxed)
    }

    /// True once the source ran out (the device may still be playing its buffer).
    pub fn finished(&self) -> bool {
        self.finished.load(Ordering::Relaxed)
    }
}

/// Opens the output and starts playing. `make` receives the device sample rate and
/// builds the source for it.
pub fn play(cfg: &AudioConfig, make: impl FnOnce(u32) -> Box<dyn SampleSource>) -> anyhow::Result<Playback> {
    let device = find_device(&cfg.output, true)?;
    let supported = pick_config(&device, cfg.sample_rate, true)?;
    let rate = supported.sample_rate();
    log::info!(
        "salida de audio: {} · {} Hz · {} canales · {:?}",
        info(&device).map_or_else(|| "?".into(), |i| i.label()),
        rate,
        supported.channels(),
        supported.sample_format()
    );
    let source = make(rate);
    let played = Arc::new(AtomicU64::new(0));
    let finished = Arc::new(AtomicBool::new(false));
    let ctx = OutCtx { source, level: cfg.level.clamp(0.0, 1.0), channel: cfg.channel, played: played.clone(), finished: finished.clone() };
    let config = supported.config();
    let stream = match supported.sample_format() {
        SampleFormat::F32 => build_output::<f32>(&device, config, ctx),
        SampleFormat::F64 => build_output::<f64>(&device, config, ctx),
        SampleFormat::I16 => build_output::<i16>(&device, config, ctx),
        SampleFormat::U16 => build_output::<u16>(&device, config, ctx),
        SampleFormat::I32 => build_output::<i32>(&device, config, ctx),
        SampleFormat::U32 => build_output::<u32>(&device, config, ctx),
        SampleFormat::I8 => build_output::<i8>(&device, config, ctx),
        SampleFormat::U8 => build_output::<u8>(&device, config, ctx),
        other => Err(anyhow!("formato de muestra no soportado: {other:?}")),
    }?;
    stream.play().context("no se pudo iniciar la salida de audio")?;
    Ok(Playback { _stream: stream, sample_rate: rate, played, finished })
}

struct OutCtx {
    source: Box<dyn SampleSource>,
    level: f32,
    channel: OutChannel,
    played: Arc<AtomicU64>,
    finished: Arc<AtomicBool>,
}

fn build_output<T: SizedSample + FromSample<f32>>(device: &Device, config: StreamConfig, mut ctx: OutCtx) -> anyhow::Result<cpal::Stream> {
    let channels = config.channels.max(1) as usize;
    let mut mono: Vec<f32> = Vec::new();
    let stream = device.build_output_stream::<T, _, _>(
        config,
        move |data: &mut [T], _| {
            let frames = data.len() / channels;
            mono.resize(frames, 0.0);
            let n = if ctx.finished.load(Ordering::Relaxed) { 0 } else { ctx.source.fill(&mut mono) };
            if n < frames {
                mono[n..].fill(0.0);
                ctx.finished.store(true, Ordering::Relaxed);
            }
            ctx.played.fetch_add(n as u64, Ordering::Relaxed);
            for (frame, &v) in data.chunks_mut(channels).zip(&mono) {
                let v = v * ctx.level;
                for (c, s) in frame.iter_mut().enumerate() {
                    let on = match ctx.channel {
                        OutChannel::Both => true,
                        OutChannel::Left => c == 0,
                        OutChannel::Right => c == 1 || channels == 1,
                    };
                    *s = T::from_sample(if on { v } else { 0.0 });
                }
            }
        },
        |e| log::error!("salida de audio: {e}"),
        None,
    )?;
    Ok(stream)
}

/// A running input; dropping it stops the capture.
pub struct Capture {
    _stream: cpal::Stream,
    pub sample_rate: u32,
}

/// Starts capturing; `on_block` receives mono samples (first channel) on the audio thread.
pub fn capture(cfg: &AudioConfig, on_block: impl FnMut(&[f32]) + Send + 'static) -> anyhow::Result<Capture> {
    let device = find_device(&cfg.input, false)?;
    let supported = pick_config(&device, cfg.sample_rate, false)?;
    let rate = supported.sample_rate();
    log::info!(
        "entrada de audio: {} · {} Hz · {} canales · {:?}",
        info(&device).map_or_else(|| "?".into(), |i| i.label()),
        rate,
        supported.channels(),
        supported.sample_format()
    );
    let config = supported.config();
    let stream = match supported.sample_format() {
        SampleFormat::F32 => build_input::<f32>(&device, config, on_block),
        SampleFormat::F64 => build_input::<f64>(&device, config, on_block),
        SampleFormat::I16 => build_input::<i16>(&device, config, on_block),
        SampleFormat::U16 => build_input::<u16>(&device, config, on_block),
        SampleFormat::I32 => build_input::<i32>(&device, config, on_block),
        SampleFormat::U32 => build_input::<u32>(&device, config, on_block),
        SampleFormat::I8 => build_input::<i8>(&device, config, on_block),
        SampleFormat::U8 => build_input::<u8>(&device, config, on_block),
        other => Err(anyhow!("formato de muestra no soportado: {other:?}")),
    }?;
    stream.play().context("no se pudo iniciar la entrada de audio")?;
    Ok(Capture { _stream: stream, sample_rate: rate })
}

fn build_input<T: SizedSample>(device: &Device, config: StreamConfig, mut on_block: impl FnMut(&[f32]) + Send + 'static) -> anyhow::Result<cpal::Stream>
where
    f32: FromSample<T>,
{
    let channels = config.channels.max(1) as usize;
    let mut mono: Vec<f32> = Vec::new();
    let stream = device.build_input_stream::<T, _, _>(
        config,
        move |data: &[T], _| {
            mono.clear();
            mono.extend(data.chunks(channels).map(|f| f[0].to_sample::<f32>()));
            on_block(&mono);
        },
        |e| log::error!("entrada de audio: {e}"),
        None,
    )?;
    Ok(stream)
}
