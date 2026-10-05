//! The server's UDP reflector: tells a client the public address its UDP
//! packets come from, so two clients behind NAT can punch a direct path to
//! each other (see `docs/DIRECT.md`).
//!
//! It listens on the server's port, over UDP. Requests are padded to
//! [`REQUEST_LEN`] and answers are shorter, so the reflector cannot amplify
//! traffic towards a forged sender.

use std::net::SocketAddr;

const MAGIC: &[u8; 16] = b"ctxremote/refl1\0";
/// Size of every request; anything else is ignored.
pub const REQUEST_LEN: usize = 64;

/// A request carrying `id`, to match the answer.
pub fn request(id: [u8; 16]) -> [u8; REQUEST_LEN] {
    let mut packet = [0u8; REQUEST_LEN];
    packet[..16].copy_from_slice(MAGIC);
    packet[16..32].copy_from_slice(&id);
    packet
}

/// Server side: the answer to `packet` from `from`, or `None` if it is no request.
pub fn answer(packet: &[u8], from: SocketAddr) -> Option<Vec<u8>> {
    if packet.len() != REQUEST_LEN || &packet[..16] != MAGIC {
        return None;
    }
    let mut reply = packet[..32].to_vec();
    reply.extend_from_slice(&postcard::to_stdvec(&from).ok()?);
    // At most 16 + 16 + 19 bytes, always below the request size.
    (reply.len() < REQUEST_LEN).then_some(reply)
}

/// Client side: the address in the answer to request `id`.
pub fn parse_answer(packet: &[u8], id: [u8; 16]) -> Option<SocketAddr> {
    if packet.len() < 32 || &packet[..16] != MAGIC || packet[16..32] != id {
        return None;
    }
    postcard::from_bytes(&packet[32..]).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_without_amplification() {
        for from in ["203.0.113.7:40000", "[2001:db8::1]:65535"] {
            let from: SocketAddr = from.parse().unwrap();
            let request = request([5; 16]);
            let reply = answer(&request, from).unwrap();
            assert!(reply.len() < request.len());
            assert_eq!(parse_answer(&reply, [5; 16]), Some(from));
            assert_eq!(parse_answer(&reply, [6; 16]), None);
        }
        assert!(answer(&request([1; 16])[..63], "1.2.3.4:5".parse().unwrap()).is_none());
        assert!(answer(&[0; REQUEST_LEN], "1.2.3.4:5".parse().unwrap()).is_none());
    }
}
