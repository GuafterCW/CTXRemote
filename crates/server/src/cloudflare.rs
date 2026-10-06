//! Cloudflare's proxy addresses (https://www.cloudflare.com/ips/). The web
//! interface sits behind Cloudflare; only requests that come from these
//! addresses may name the client's address in `CF-Connecting-IP`.

use std::net::IpAddr;

const V4: &[(u32, u8)] = &[
    (u32::from_be_bytes([173, 245, 48, 0]), 20),
    (u32::from_be_bytes([103, 21, 244, 0]), 22),
    (u32::from_be_bytes([103, 22, 200, 0]), 22),
    (u32::from_be_bytes([103, 31, 4, 0]), 22),
    (u32::from_be_bytes([141, 101, 64, 0]), 18),
    (u32::from_be_bytes([108, 162, 192, 0]), 18),
    (u32::from_be_bytes([190, 93, 240, 0]), 20),
    (u32::from_be_bytes([188, 114, 96, 0]), 20),
    (u32::from_be_bytes([197, 234, 240, 0]), 22),
    (u32::from_be_bytes([198, 41, 128, 0]), 17),
    (u32::from_be_bytes([162, 158, 0, 0]), 15),
    (u32::from_be_bytes([104, 16, 0, 0]), 13),
    (u32::from_be_bytes([104, 24, 0, 0]), 14),
    (u32::from_be_bytes([172, 64, 0, 0]), 13),
    (u32::from_be_bytes([131, 0, 72, 0]), 22),
];

/// The first 32 bits and the prefix length.
const V6: &[(u32, u8)] = &[
    (0x2400_cb00, 32),
    (0x2606_4700, 32),
    (0x2803_f800, 32),
    (0x2405_b500, 32),
    (0x2405_8100, 32),
    (0x2a06_98c0, 29),
    (0x2c0f_f248, 32),
];

fn within(addr: u32, (net, bits): (u32, u8)) -> bool {
    let mask = u32::MAX << (32 - u32::from(bits));
    addr & mask == net & mask
}

pub fn contains(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => V4.iter().any(|&range| within(u32::from(v4), range)),
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return contains(IpAddr::V4(v4));
            }
            let top = (u128::from(v6) >> 96) as u32;
            V6.iter().any(|&range| within(top, range))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn knows_cloudflare_and_nobody_else() {
        for ip in ["172.70.1.2", "104.23.255.255", "162.159.0.1", "2a06:98c7::1", "2606:4700::6810:84e5"] {
            assert!(contains(ip.parse().unwrap()), "{ip}");
        }
        for ip in ["8.8.8.8", "172.16.0.1", "104.32.0.1", "127.0.0.1", "2a06:98c8::1", "2001:db8::1"] {
            assert!(!contains(ip.parse().unwrap()), "{ip}");
        }
    }
}
