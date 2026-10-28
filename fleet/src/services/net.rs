//! Reaching machines: their kimo (with the fleet key), a ping when kimo does not answer,
//! the Wake-on-LAN magic packet, and notifications to the owner (ntfy).

use crate::core::wol;
use anyhow::{anyhow, Context, Result};
use kimo::client::Client;
use std::net::{IpAddr, SocketAddr, UdpSocket};
use std::process::{Command, Stdio};
use std::time::Duration;

/// Seconds a machine's kimo has to answer one call.
pub const CALL_SECS: u64 = 4;

pub fn kimo(url: &str, key: Option<&str>) -> Client {
    Client::new(url, key.map(str::to_string))
}

/// Does the host answer one ping within a second?
pub fn ping(host: &str) -> bool {
    let mut cmd = Command::new("ping");
    // The timeout flag differs: seconds on Linux, milliseconds on macOS.
    if cfg!(target_os = "macos") {
        cmd.args(["-c", "1", "-t", "1", host]);
    } else {
        cmd.args(["-c", "1", "-W", "1", host]);
    }
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// Where the magic packet goes: `192.168.1.255`, `192.168.1.255:7`, or the LAN broadcast.
pub fn wol_target(broadcast: Option<&str>) -> Result<SocketAddr> {
    match broadcast {
        None => Ok(SocketAddr::from(([255, 255, 255, 255], wol::PORT))),
        Some(b) => b
            .parse::<SocketAddr>()
            .or_else(|_| b.parse::<IpAddr>().map(|ip| SocketAddr::new(ip, wol::PORT)))
            .map_err(|_| anyhow!("{b} is not an address")),
    }
}

/// Send the magic packet for `mac` (three times: UDP may drop one).
pub fn send_wol(mac: [u8; 6], to: SocketAddr) -> Result<()> {
    let socket = UdpSocket::bind(if to.is_ipv4() { "0.0.0.0:0" } else { "[::]:0" })
        .context("opening a socket for Wake-on-LAN")?;
    socket.set_broadcast(true)?;
    let packet = wol::magic_packet(mac);
    for _ in 0..3 {
        socket
            .send_to(&packet, to)
            .with_context(|| format!("sending the magic packet to {to}"))?;
        std::thread::sleep(Duration::from_millis(30));
    }
    Ok(())
}

/// Send a notification to an ntfy topic URL (HTTPS with checked certificates, or HTTP).
pub fn notify(url: &str, title: &str, body: &str) -> Result<()> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(10)))
        .http_status_as_error(false)
        .build()
        .into();
    let res = agent
        .post(url)
        .header("Title", title)
        .send(body.as_bytes())
        .map_err(|e| anyhow!("{e}"))?;
    if !res.status().is_success() {
        anyhow::bail!("{url} answered {}", res.status());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn magic_packets_reach_the_broadcast_address() {
        let listener = UdpSocket::bind("127.0.0.1:0").unwrap();
        listener
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let to = wol_target(Some(&listener.local_addr().unwrap().to_string())).unwrap();
        let mac = [1, 2, 3, 4, 5, 6];
        send_wol(mac, to).unwrap();
        let mut buf = [0u8; 200];
        let (n, _) = listener.recv_from(&mut buf).unwrap();
        assert_eq!(&buf[..n], &wol::magic_packet(mac)[..]);
        assert_eq!(
            wol_target(Some("192.168.1.255")).unwrap(),
            "192.168.1.255:9".parse().unwrap()
        );
        assert_eq!(
            wol_target(None).unwrap(),
            "255.255.255.255:9".parse().unwrap()
        );
        assert!(wol_target(Some("lan")).is_err());
    }
}
