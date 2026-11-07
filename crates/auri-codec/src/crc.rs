//! CRC-16/CCITT-FALSE, the checksum the firmware validates before applying a
//! payload.

/// CRC-16/CCITT-FALSE: polynomial `0x1021`, init `0xFFFF`, no reflection, no
/// final XOR.
///
/// The chip rejects a payload whose CRC does not match, silently — the write
/// simply never happens.
pub fn crc16_ccitt(data: &[u8]) -> u16 {
    let mut register: u16 = 0xffff;
    for &byte in data {
        register ^= u16::from(byte) << 8;
        for _ in 0..8 {
            register = if register & 0x8000 != 0 {
                (register << 1) ^ 0x1021
            } else {
                register << 1
            };
        }
    }
    register
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_is_init() {
        assert_eq!(crc16_ccitt(&[]), 0xffff);
    }

    #[test]
    fn known_vector_123456789() {
        assert_eq!(crc16_ccitt(b"123456789"), 0x29b1);
    }
}
