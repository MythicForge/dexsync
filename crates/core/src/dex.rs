//! Pokédex bit-field helpers shared by every generation.
//!
//! Gens 1–3 store one bit per species: bit `n - 1` is National Dex #n,
//! least significant bit first within each byte.

/// List the dex numbers whose flag is set, ignoring bits past `max`.
pub fn flags_to_numbers(bits: &[u8], max: u16) -> Vec<u16> {
    (1..=max)
        .filter(|&n| {
            let i = usize::from(n - 1);
            bits.get(i / 8).is_some_and(|b| b & (1 << (i % 8)) != 0)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lsb_first() {
        // #1 and #8 in byte 0, #9 in byte 1.
        assert_eq!(flags_to_numbers(&[0b1000_0001, 0b0000_0001], 16), vec![1, 8, 9]);
    }

    #[test]
    fn ignores_bits_past_max() {
        assert_eq!(flags_to_numbers(&[0xFF], 3), vec![1, 2, 3]);
    }
}
