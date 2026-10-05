//! Wake-on-LAN: the "magic packet" (six 0xFF bytes, then the MAC sixteen
//! times) as UDP broadcast into every local network. It reaches the computer
//! only from the same network, and only if waking is on in its firmware and
//! network card.

use std::net::{Ipv4Addr, SocketAddrV4, UdpSocket};

use anyhow::{bail, Context, Result};

/// Sends the packet for each MAC to every local IPv4 network; returns how many went out.
pub fn wake(macs: &[String]) -> Result<usize> {
    let packets: Vec<Vec<u8>> = macs.iter().filter_map(|m| parse_mac(m)).map(|mac| magic_packet(&mac)).collect();
    if packets.is_empty() {
        bail!("Für dieses Gerät ist keine Netzwerkadresse (MAC) bekannt");
    }
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).context("UDP nicht verfügbar")?;
    socket.set_broadcast(true)?;
    let mut targets = vec![Ipv4Addr::BROADCAST];
    targets.extend(directed_broadcasts());
    let mut sent = 0;
    for packet in &packets {
        for target in &targets {
            // Port 9 (discard) is the custom one; some cards listen on 7.
            for port in [9, 7] {
                if socket.send_to(packet, SocketAddrV4::new(*target, port)).is_ok() {
                    sent += 1;
                }
            }
        }
    }
    if sent == 0 {
        bail!("Das Weckpaket ließ sich nicht senden");
    }
    Ok(sent)
}

/// `aa:bb:cc:dd:ee:ff` or with dashes.
pub fn parse_mac(text: &str) -> Option<[u8; 6]> {
    let parts: Vec<u8> = text.split([':', '-']).map(|p| u8::from_str_radix(p, 16).ok()).collect::<Option<_>>()?;
    let mac: [u8; 6] = parts.try_into().ok()?;
    (mac != [0; 6] && mac != [0xff; 6]).then_some(mac)
}

pub fn magic_packet(mac: &[u8; 6]) -> Vec<u8> {
    let mut packet = vec![0xff; 6];
    for _ in 0..16 {
        packet.extend_from_slice(mac);
    }
    packet
}

/// The broadcast address of each local IPv4 network: a plain 255.255.255.255
/// leaves through one interface only.
fn directed_broadcasts() -> Vec<Ipv4Addr> {
    let mut out = Vec::new();
    for (_, data) in sysinfo::Networks::new_with_refreshed_list().iter() {
        for net in data.ip_networks() {
            if let std::net::IpAddr::V4(addr) = net.addr {
                if addr.is_loopback() || addr.is_link_local() || net.prefix == 0 || net.prefix >= 31 {
                    continue;
                }
                let mask = u32::MAX << (32 - net.prefix as u32);
                let broadcast = Ipv4Addr::from(u32::from(addr) | !mask);
                if !out.contains(&broadcast) {
                    out.push(broadcast);
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packet_layout() {
        let mac = parse_mac("01:23:45:67:89:AB").unwrap();
        assert_eq!(parse_mac("01-23-45-67-89-ab"), Some(mac));
        let packet = magic_packet(&mac);
        assert_eq!(packet.len(), 102);
        assert_eq!(&packet[..6], &[0xff; 6]);
        assert_eq!(&packet[6..12], &mac);
        assert_eq!(&packet[96..], &mac);
    }

    #[test]
    fn rejects_bad_macs() {
        assert_eq!(parse_mac("00:00:00:00:00:00"), None);
        assert_eq!(parse_mac("01:23:45:67:89"), None);
        assert_eq!(parse_mac("zz:23:45:67:89:ab"), None);
    }
}
