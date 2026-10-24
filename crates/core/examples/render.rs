//! Renders the on-air frame of a project and optionally encodes it to a WAV file, to
//! check graphics and decode the audio with QSSTV / MMSSTV / Robot36.
//!
//!   cargo run -p estivi-core --example render -- canal.estivi cuadro.png [audio.wav] [programa] [modo]
//!
//! `programa` is the 0-based program index; `modo` a short name (M1, S2, R36, PD120…).

use std::io::Write;
use std::path::PathBuf;

use anyhow::Context;
use chrono::Utc;
use estivi_core::compose::{Composer, data_ctx};
use estivi_core::dynamic::{ApiCache, expand};
use estivi_core::encoder::{ScanImage, TxPlan};
use estivi_core::modes::Mode;
use estivi_core::project::Project;
use estivi_core::synth::SampleSource;

const RATE: u32 = 48_000;

fn write_wav(path: &str, samples: &[f32]) -> anyhow::Result<()> {
    let mut f = std::io::BufWriter::new(std::fs::File::create(path)?);
    let data_len = (samples.len() * 2) as u32;
    f.write_all(b"RIFF")?;
    f.write_all(&(36 + data_len).to_le_bytes())?;
    f.write_all(b"WAVEfmt ")?;
    f.write_all(&16u32.to_le_bytes())?;
    f.write_all(&1u16.to_le_bytes())?; // PCM
    f.write_all(&1u16.to_le_bytes())?; // mono
    f.write_all(&RATE.to_le_bytes())?;
    f.write_all(&(RATE * 2).to_le_bytes())?;
    f.write_all(&2u16.to_le_bytes())?;
    f.write_all(&16u16.to_le_bytes())?;
    f.write_all(b"data")?;
    f.write_all(&data_len.to_le_bytes())?;
    for s in samples {
        f.write_all(&((s.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes())?;
    }
    Ok(())
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    anyhow::ensure!(args.len() >= 3, "uso: render <proyecto.estivi> <cuadro.png> [audio.wav] [programa] [modo]");
    let project_path = PathBuf::from(&args[1]);
    let mut project = Project::load(&project_path)?;
    if let Some(m) = args.get(5) {
        project.tx.mode = Mode::from_short(m).with_context(|| format!("modo desconocido: {m}"))?;
    }
    let dir = project_path.parent();
    let index: usize = args.get(4).map(|s| s.parse()).transpose()?.unwrap_or(0);
    let program = project.programs.get(index);

    let api = ApiCache::default();
    for src in project.graphics.api_sources() {
        if let Err(e) = api.fetch_now(src) {
            eprintln!("API {}: {e}", src.url);
        }
    }
    let mut composer = Composer::new(api);
    composer.set_project_dir(dir);
    let mode = project.mode_of(program, program.and_then(|p| p.enabled_images().next()));
    let ctx = data_ctx(&project, Utc::now(), 42, program.map_or("", |p| p.name.as_str()), 0, "", "", mode);
    let frame = composer.frame(&project, dir, program, &ctx);
    frame.save(&args[2])?;
    println!("{} → {} ({}x{})", mode.name(), args[2], frame.width(), frame.height());

    if let Some(wav) = args.get(3).filter(|w| !w.is_empty()) {
        let rgb = image::DynamicImage::ImageRgba8(frame).to_rgb8();
        let scan = ScanImage::new(mode, &rgb, project.tx.color_range)?;
        let opts = project.tx.options(&project.station.call, |t| expand(t, &ctx, None));
        let plan = TxPlan::new(scan, &opts);
        let mut synth = plan.synth(RATE as f64, 0.8);
        let mut samples = vec![0.0f32; synth.total_samples() as usize];
        let n = synth.fill(&mut samples);
        samples.truncate(n);
        write_wav(wav, &samples)?;
        println!("{wav}: {:.1} s a {RATE} Hz", n as f64 / RATE as f64);
    }
    Ok(())
}
