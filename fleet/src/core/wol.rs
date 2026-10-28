//! Wake-on-LAN: the magic packet is 6 bytes of 0xFF and the machine's MAC 16 times, sent
//! as a UDP broadcast on its LAN (a layer-2 broadcast: VPNs such as ZeroTier do not carry it).

/// The magic packet for `mac`.
pub fn magic_packet(mac: [u8; 6]) -> [u8; 102] {
    let mut p = [0xFFu8; 102];
    for i in 0..16 {
        p[6 + i * 6..12 + i * 6].copy_from_slice(&mac);
    }
    p
}

/// Port 9 ("discard") is the usual one; some cards listen on 7.
pub const PORT: u16 = 9;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packet() {
        let mac = [0x1c, 0x69, 0x7a, 0xaa, 0xbb, 0xcc];
        let p = magic_packet(mac);
        assert_eq!(&p[..6], &[0xFF; 6]);
        for i in 0..16 {
            assert_eq!(&p[6 + i * 6..12 + i * 6], &mac);
        }
    }
}
