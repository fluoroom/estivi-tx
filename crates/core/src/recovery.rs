//! YONIQ's DGT YQ upload. The picture on the air is unchanged. A clean 320×256 BMP
//! is stored beside it, and a receiver that heard the name can blit that file over
//! the noisy copy.
//!
//! The remote name is `@` + callsign + `ddmmyyyyhhnnss` + `.BMP`. YONIQ builds the
//! stamp with `DateToStr` (slashes removed) and `hh:nn:ss` (colons removed), date
//! then time, on the machine's local clock. `hh` is 00–23 when the format has no
//! am/pm mark. The day-month-year order is the short date of the YONIQ author's
//! Windows locale, and of this program.

use std::io::{BufRead, BufReader, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream, ToSocketAddrs};
use std::time::Duration;

use chrono::NaiveDateTime;
use image::{DynamicImage, ImageFormat, RgbaImage};

use crate::config::Recovery;

pub const WIDTH: u32 = 320;
pub const HEIGHT: u32 = 256;

/// `ddmmyyyyhhnnss` from a local clock reading.
pub fn stamp(local: NaiveDateTime) -> String {
    local.format("%d%m%Y%H%M%S").to_string()
}

/// `@LU1AQY31102026140509.BMP`. Empty when the callsign or the stamp would not be a
/// single file in `/web/imagenes_ioniq/`.
pub fn remote_name(call: &str, stamp: &str) -> Option<String> {
    let call = call.trim();
    if !call_ok(call) || stamp.len() != 14 || !stamp.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some(format!("@{call}{stamp}.BMP"))
}

fn call_ok(call: &str) -> bool {
    let mut parts = call.split('/');
    let head = parts.next().unwrap_or("");
    let tail = parts.next();
    if parts.next().is_some() {
        return false;
    }
    alnum(head, 1, 12) && tail.is_none_or(|t| alnum(t, 1, 4))
}

fn alnum(s: &str, min: usize, max: usize) -> bool {
    (min..=max).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_alphanumeric())
}

/// 24-bit BMP at the YoniQ size. A frame of another size is scaled with nearest
/// neighbour so the blit matches the mode.
pub fn bmp_320x256(frame: &RgbaImage) -> Vec<u8> {
    let fitted = fit(frame, WIDTH, HEIGHT);
    let rgb = DynamicImage::ImageRgba8(fitted).into_rgb8();
    let mut out = Vec::new();
    DynamicImage::ImageRgb8(rgb).write_to(&mut std::io::Cursor::new(&mut out), ImageFormat::Bmp).expect("bmp");
    out
}

fn fit(src: &RgbaImage, w: u32, h: u32) -> RgbaImage {
    if src.width() == w && src.height() == h {
        return src.clone();
    }
    let mut out = RgbaImage::new(w, h);
    let sw = src.width().max(1);
    let sh = src.height().max(1);
    for y in 0..h {
        for x in 0..w {
            let sx = x * sw / w;
            let sy = y * sh / h;
            out.put_pixel(x, y, *src.get_pixel(sx, sy));
        }
    }
    out
}

/// Passive binary `STOR` into `cfg.path`. The control conversation never includes
/// the password in an error string.
pub fn upload(cfg: &Recovery, name: &str, bmp: &[u8]) -> Result<(), String> {
    if !cfg.enabled {
        return Ok(());
    }
    if cfg.password.is_empty() {
        return Err("recovery FTP password is missing (YONIQ_FTP_PASSWORD)".into());
    }
    let mut ctrl = TcpStream::connect_timeout(&resolve(&cfg.host, cfg.port)?, Duration::from_secs(15)).map_err(|e| format!("recovery FTP connect: {e}"))?;
    ctrl.set_read_timeout(Some(Duration::from_secs(20))).ok();
    ctrl.set_write_timeout(Some(Duration::from_secs(20))).ok();
    let mut reader = BufReader::new(ctrl.try_clone().map_err(|e| e.to_string())?);
    expect(&mut reader, &[220])?;
    write_cmd(&mut ctrl, &format!("USER {}", cfg.user))?;
    let (code, _) = read_reply(&mut reader)?;
    if code == 331 {
        write_cmd(&mut ctrl, &format!("PASS {}", cfg.password))?;
        expect(&mut reader, &[230])?;
    } else if code != 230 {
        return Err(format!("recovery FTP login refused ({code})"));
    }
    write_cmd(&mut ctrl, "TYPE I")?;
    expect(&mut reader, &[200])?;
    write_cmd(&mut ctrl, &format!("CWD {}", cfg.path))?;
    expect(&mut reader, &[250, 200])?;
    write_cmd(&mut ctrl, "PASV")?;
    let (_, pasv_line) = read_reply(&mut reader)?;
    if !pasv_line.starts_with("227") {
        return Err("recovery FTP has no passive port".into());
    }
    let data_addr = pasv_addr(&pasv_line)?;
    let mut data = match TcpStream::connect_timeout(&data_addr, Duration::from_secs(15)) {
        Ok(stream) => stream,
        Err(_) => {
            let peer = ctrl.peer_addr().map_err(|e| e.to_string())?;
            TcpStream::connect_timeout(&SocketAddr::new(peer.ip(), data_addr.port()), Duration::from_secs(15)).map_err(|e| format!("recovery FTP data: {e}"))?
        }
    };
    write_cmd(&mut ctrl, &format!("STOR {name}"))?;
    expect(&mut reader, &[150, 125])?;
    data.write_all(bmp).map_err(|e| format!("recovery FTP send: {e}"))?;
    data.shutdown(std::net::Shutdown::Write).ok();
    drop(data);
    expect(&mut reader, &[226, 250])?;
    write_cmd(&mut ctrl, "QUIT").ok();
    Ok(())
}

fn resolve(host: &str, port: u16) -> Result<SocketAddr, String> {
    (host, port).to_socket_addrs().map_err(|e| e.to_string())?.next().ok_or_else(|| format!("cannot resolve {host}"))
}

fn write_cmd(stream: &mut TcpStream, text: &str) -> Result<(), String> {
    stream.write_all(format!("{text}\r\n").as_bytes()).map_err(|e| e.to_string())?;
    stream.flush().map_err(|e| e.to_string())
}

fn expect(reader: &mut BufReader<TcpStream>, ok: &[u16]) -> Result<(), String> {
    let (code, _) = read_reply(reader)?;
    if ok.contains(&code) { Ok(()) } else { Err(format!("recovery FTP {code}")) }
}

fn read_reply(reader: &mut BufReader<TcpStream>) -> Result<(u16, String), String> {
    let mut line = String::new();
    reader.read_line(&mut line).map_err(|e| e.to_string())?;
    if line.len() < 3 {
        return Err("recovery FTP closed".into());
    }
    let code: u16 = line[..3].parse().map_err(|_| "recovery FTP reply".to_string())?;
    if line.as_bytes().get(3) == Some(&b'-') {
        let end = format!("{code} ");
        loop {
            line.clear();
            let n = reader.read_line(&mut line).map_err(|e| e.to_string())?;
            if n == 0 {
                return Err("recovery FTP closed".into());
            }
            if line.starts_with(&end) || line.starts_with(&format!("{code}\r")) {
                break;
            }
        }
    }
    Ok((code, line))
}

fn pasv_addr(line: &str) -> Result<SocketAddr, String> {
    let start = line.rfind('(').ok_or("recovery FTP passive reply")?;
    let end = line[start..].find(')').ok_or("recovery FTP passive reply")? + start;
    let nums: Vec<u8> = line[start + 1..end].split(',').map(|s| s.trim().parse::<u8>()).collect::<Result<_, _>>().map_err(|_| "recovery FTP passive reply".to_string())?;
    if nums.len() != 6 {
        return Err("recovery FTP passive reply".into());
    }
    let ip = Ipv4Addr::new(nums[0], nums[1], nums[2], nums[3]);
    let port = (u16::from(nums[4]) << 8) | u16::from(nums[5]);
    Ok(SocketAddr::from((ip, port)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;
    use image::Rgba;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::sync::mpsc;
    use std::thread;

    #[test]
    fn stamp_is_day_month_year_then_time() {
        let local = NaiveDate::from_ymd_opt(2026, 10, 31).unwrap().and_hms_opt(14, 5, 9).unwrap();
        assert_eq!(stamp(local), "31102026140509");
        assert_eq!(remote_name("LU1AQY", "31102026140509").as_deref(), Some("@LU1AQY31102026140509.BMP"));
        assert_eq!(remote_name("W1AW/P", "31102026140509").as_deref(), Some("@W1AW/P31102026140509.BMP"));
        assert!(remote_name("", "31102026140509").is_none());
        assert!(remote_name("LU1AQY", "31102026").is_none());
        assert!(remote_name("../x", "31102026140509").is_none());
    }

    #[test]
    fn bmp_is_24_bit_and_the_yoniq_size() {
        let mut img = RgbaImage::new(4, 2);
        img.put_pixel(0, 0, Rgba([10, 20, 30, 255]));
        let bmp = bmp_320x256(&img);
        assert_eq!(&bmp[0..2], b"BM");
        let w = u32::from_le_bytes(bmp[18..22].try_into().unwrap());
        let h = u32::from_le_bytes(bmp[22..26].try_into().unwrap());
        let bits = u16::from_le_bytes(bmp[28..30].try_into().unwrap());
        assert_eq!((w, h, bits), (320, 256, 24));
    }

    #[test]
    fn stor_sends_the_bitmap_on_the_passive_port() {
        let ctrl = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = ctrl.local_addr().unwrap().port();
        let data = TcpListener::bind("127.0.0.1:0").unwrap();
        let data_port = data.local_addr().unwrap().port();
        let got = thread::spawn(move || {
            let (mut c, _) = ctrl.accept().unwrap();
            let mut r = BufReader::new(c.try_clone().unwrap());
            let mut line = String::new();
            let (tx, rx) = mpsc::channel();
            let mut data_slot = Some(data);
            c.write_all(b"220 ready\r\n").unwrap();
            let mut stored = Vec::new();
            loop {
                line.clear();
                if r.read_line(&mut line).unwrap() == 0 {
                    break;
                }
                let cmd = line.trim().to_string();
                if cmd.starts_with("USER ") {
                    c.write_all(b"331 need password\r\n").unwrap();
                } else if cmd.starts_with("PASS ") {
                    c.write_all(b"230 in\r\n").unwrap();
                } else if cmd == "TYPE I" {
                    c.write_all(b"200 binary\r\n").unwrap();
                } else if cmd.starts_with("CWD ") {
                    c.write_all(b"250 ok\r\n").unwrap();
                } else if cmd == "PASV" {
                    let p1 = data_port >> 8;
                    let p2 = data_port & 0xff;
                    c.write_all(format!("227 Entering Passive Mode (127,0,0,1,{p1},{p2})\r\n").as_bytes()).unwrap();
                    let tx = tx.clone();
                    let listener = data_slot.take().unwrap();
                    thread::spawn(move || {
                        let (mut d, _) = listener.accept().unwrap();
                        let mut body = Vec::new();
                        d.read_to_end(&mut body).unwrap();
                        tx.send(body).unwrap();
                    });
                } else if cmd.starts_with("STOR ") {
                    c.write_all(b"150 go\r\n").unwrap();
                    stored = rx.recv().unwrap();
                    c.write_all(b"226 done\r\n").unwrap();
                } else if cmd == "QUIT" {
                    c.write_all(b"221 bye\r\n").unwrap();
                    break;
                } else {
                    c.write_all(b"500\r\n").unwrap();
                }
            }
            stored
        });
        let cfg = Recovery { enabled: true, host: "127.0.0.1".into(), port, user: "u".into(), password: "p".into(), path: "/web/imagenes_ioniq".into() };
        upload(&cfg, "@LU1AQY31102026140509.BMP", b"BMPDATA").unwrap();
        assert_eq!(got.join().unwrap(), b"BMPDATA");
    }
}
