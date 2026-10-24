//! Lists the sound devices and serial ports Estivi can see. The id (first column) is
//! what goes in `estivi.toml`.
//!
//!   cargo run -p estivi-core --example dispositivos

fn main() {
    println!("Salidas de audio:");
    for d in estivi_core::audio::output_devices() {
        println!("  {:40} {}", d.id, d.name);
    }
    println!("Entradas de audio:");
    for d in estivi_core::audio::input_devices() {
        println!("  {:40} {}", d.id, d.name);
    }
    println!("Puertos serie:");
    for p in estivi_core::ptt::serial_ports() {
        println!("  {p}");
    }
}
