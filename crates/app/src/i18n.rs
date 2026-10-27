//! UI language: English source, Spanish translation. First boot follows the OS
//! when it is en/es; anything else is English.

use std::sync::atomic::{AtomicU8, Ordering};

use estivi_core::Lang;

static LANG: AtomicU8 = AtomicU8::new(0);

pub fn set(lang: Lang) {
    LANG.store(match lang {
        Lang::English => 0,
        Lang::Spanish => 1,
    }, Ordering::Relaxed);
}

pub fn get() -> Lang {
    match LANG.load(Ordering::Relaxed) {
        1 => Lang::Spanish,
        _ => Lang::English,
    }
}

pub fn t(en: &str, es: &str) -> String {
    get().t(en, es)
}

pub fn ts(en: &'static str, es: &'static str) -> &'static str {
    get().pick(en, es)
}

/// Apply bundled Slint translations. Must run after the first component exists.
pub fn apply_slint(lang: Lang) {
    set(lang);
    match slint::select_bundled_translation(lang.code()) {
        Ok(()) => {}
        Err(e) => log::warn!("translation {}: {e}", lang.code()),
    }
}
