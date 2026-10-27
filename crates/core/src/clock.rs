//! NTP offset for the grid clock. The operating-system clock is never changed:
//! Estivi adds the saved offset when it decides what time the channel thinks it is.

use std::net::UdpSocket;
use std::time::Duration;

use chrono::{DateTime, TimeDelta, TimeZone, Utc};

/// Seconds between the NTP epoch (1900-01-01) and the Unix epoch.
const NTP_UNIX: i64 = 2_208_988_800;

/// `t1` client send, `t2` server receive, `t3` server transmit, `t4` client receive.
pub fn offset_between(t1: DateTime<Utc>, t2: DateTime<Utc>, t3: DateTime<Utc>, t4: DateTime<Utc>) -> TimeDelta {
    ((t2 - t1) + (t3 - t4)) / 2
}

pub fn ntp_to_utc(secs: u32, frac: u32) -> Option<DateTime<Utc>> {
    let unix = secs as i64 - NTP_UNIX;
    let nanos = ((frac as u64 * 1_000_000_000) >> 32) as u32;
    Utc.timestamp_opt(unix, nanos).single()
}

fn read_ts(buf: &[u8]) -> Option<DateTime<Utc>> {
    let secs = u32::from_be_bytes(buf.get(0..4)?.try_into().ok()?);
    let frac = u32::from_be_bytes(buf.get(4..8)?.try_into().ok()?);
    ntp_to_utc(secs, frac)
}

/// One SNTP query. Returns the offset to add to the system clock, in milliseconds.
pub fn query_offset_ms(server: &str, timeout: Duration) -> anyhow::Result<i64> {
    let server = server.trim();
    anyhow::ensure!(!server.is_empty(), "servidor NTP vacío");
    let addr = if server.contains(':') { server.to_string() } else { format!("{server}:123") };
    let sock = UdpSocket::bind("0.0.0.0:0")?;
    sock.set_read_timeout(Some(timeout))?;
    let mut pkt = [0u8; 48];
    pkt[0] = 0x1B; // LI = 0, VN = 3, mode = client
    let t1 = Utc::now();
    sock.send_to(&pkt, &addr)?;
    let mut buf = [0u8; 48];
    let (n, _) = sock.recv_from(&mut buf)?;
    let t4 = Utc::now();
    anyhow::ensure!(n >= 48, "respuesta NTP corta");
    let t2 = read_ts(&buf[32..40]).ok_or_else(|| anyhow::anyhow!("marca de recepción NTP inválida"))?;
    let t3 = read_ts(&buf[40..48]).ok_or_else(|| anyhow::anyhow!("marca de transmisión NTP inválida"))?;
    Ok(offset_between(t1, t2, t3, t4).num_milliseconds())
}

/// System clock plus the saved offset. A failed query leaves the previous offset in place.
pub fn corrected(offset_ms: i64) -> DateTime<Utc> {
    Utc::now() + TimeDelta::milliseconds(offset_ms)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn symmetric_delay_is_removed() {
        let t0 = Utc.with_ymd_and_hms(2026, 10, 27, 12, 0, 0).unwrap();
        let t1 = t0;
        let t2 = t0 + TimeDelta::milliseconds(150);
        let t3 = t0 + TimeDelta::milliseconds(160);
        let t4 = t0 + TimeDelta::milliseconds(100);
        // ((150) + (60)) / 2 = 105 ms. The 100 ms round trip is not part of the offset.
        assert_eq!(offset_between(t1, t2, t3, t4), TimeDelta::milliseconds(105));
        assert_eq!(offset_between(t0, t0, t0, t0), TimeDelta::zero());
    }

    #[test]
    fn ntp_epoch_converts() {
        // 2_208_988_800 seconds after 1900-01-01 is 1970-01-01.
        let t = ntp_to_utc(NTP_UNIX as u32, 0).unwrap();
        assert_eq!(t, Utc.with_ymd_and_hms(1970, 1, 1, 0, 0, 0).unwrap());
    }
}
