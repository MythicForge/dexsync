//! Gen 3 character encoding (Western releases only for now).

/// Decode a Gen 3 string, stopping at the 0xFF terminator.
/// Unmapped bytes become `?`.
pub fn decode_gen3(bytes: &[u8]) -> String {
    bytes
        .iter()
        .take_while(|&&b| b != 0xFF)
        .map(|&b| match b {
            0x00 => ' ',
            0xA1..=0xAA => char::from(b'0' + (b - 0xA1)),
            0xAB => '!',
            0xAC => '?',
            0xAD => '.',
            0xAE => '-',
            0xB4 => '\'',
            0xBA => '/',
            0xBB..=0xD4 => char::from(b'A' + (b - 0xBB)),
            0xD5..=0xEE => char::from(b'a' + (b - 0xD5)),
            _ => '?',
        })
        .collect()
}

/// Inverse of [`decode_gen3`] for test fixtures.
#[cfg(test)]
pub fn encode_gen3(s: &str) -> Vec<u8> {
    s.bytes()
        .map(|c| match c {
            b'A'..=b'Z' => 0xBB + (c - b'A'),
            b'a'..=b'z' => 0xD5 + (c - b'a'),
            b'0'..=b'9' => 0xA1 + (c - b'0'),
            _ => 0x00,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_name_until_terminator() {
        let mut b = encode_gen3("Nate");
        b.extend([0xFF, 0xBB]);
        assert_eq!(decode_gen3(&b), "Nate");
    }
}
