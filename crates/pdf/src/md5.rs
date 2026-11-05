//! MD5 (RFC 1321), for the `/CheckSum` of an embedded file's `/Params` (which the PDF specification defines as MD5) and for making a new
//! file identifier. Not used for anything that must resist an attacker.

const S: [u32; 64] = [
    7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23,
    4, 11, 16, 23, 4, 11, 16, 23, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
];

/// The 16 bytes of the MD5 of `data`.
pub fn md5(data: &[u8]) -> [u8; 16] {
    // K[i] = floor(2^32 * abs(sin(i + 1))).
    let k: Vec<u32> = (0..64).map(|i| ((f64::from(i + 1)).sin().abs() * 4_294_967_296.0) as u32).collect();
    let mut state = [0x6745_2301u32, 0xefcd_ab89u32, 0x98ba_dcfeu32, 0x1032_5476u32];
    // The whole blocks of the data are hashed where they are; only the end (at most two blocks, with the padding) is copied.
    let (blocks, rest) = data.as_chunks::<64>();
    for block in blocks {
        compress(&mut state, block, &k);
    }
    let mut tail = rest.to_vec();
    tail.push(0x80);
    while tail.len() % 64 != 56 {
        tail.push(0);
    }
    tail.extend_from_slice(&(u64::try_from(data.len()).unwrap_or(u64::MAX).wrapping_mul(8)).to_le_bytes());
    for block in tail.as_chunks::<64>().0 {
        compress(&mut state, block, &k);
    }
    let mut out = [0u8; 16];
    for (i, word) in state.iter().enumerate() {
        if let Some(slot) = out.get_mut(i * 4..i * 4 + 4) {
            slot.copy_from_slice(&word.to_le_bytes());
        }
    }
    out
}

/// One block of 64 bytes into the state.
fn compress(state: &mut [u32; 4], block: &[u8; 64], k: &[u32]) {
    let mut words = [0u32; 16];
    for (word, bytes) in words.iter_mut().zip(block.as_chunks::<4>().0) {
        *word = u32::from_le_bytes(*bytes);
    }
    let [mut a, mut b, mut c, mut d] = *state;
    for i in 0..64usize {
        let (f, g) = match i / 16 {
            0 => ((b & c) | (!b & d), i),
            1 => ((d & b) | (!d & c), (5 * i + 1) % 16),
            2 => (b ^ c ^ d, (3 * i + 5) % 16),
            _ => (c ^ (b | !d), (7 * i) % 16),
        };
        let word = words.get(g).copied().unwrap_or(0);
        let rotated = a.wrapping_add(f).wrapping_add(k.get(i).copied().unwrap_or(0)).wrapping_add(word).rotate_left(S.get(i).copied().unwrap_or(0));
        let next = b.wrapping_add(rotated);
        a = d;
        d = c;
        c = b;
        b = next;
    }
    for (slot, value) in state.iter_mut().zip([a, b, c, d]) {
        *slot = slot.wrapping_add(value);
    }
}

/// The 32 lower-case hexadecimal digits of the MD5 of `data`.
pub fn md5_hex(data: &[u8]) -> String {
    md5(data).iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_test_vectors_of_rfc_1321() {
        for (text, digest) in [
            ("", "d41d8cd98f00b204e9800998ecf8427e"),
            ("a", "0cc175b9c0f1b6a831c399e269772661"),
            ("abc", "900150983cd24fb0d6963f7d28e17f72"),
            ("message digest", "f96b697d7cb7938d525a2f31aaf161d0"),
            ("abcdefghijklmnopqrstuvwxyz", "c3fcd3d76192e4007dfb496cca67e13b"),
            ("ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789", "d174ab98d277d9f5a5611c2c9f419d9f"),
            ("12345678901234567890123456789012345678901234567890123456789012345678901234567890", "57edf4a22be3c955ac49da2e2107b67a"),
        ] {
            assert_eq!(md5_hex(text.as_bytes()), digest, "{text:?}");
        }
        // Lengths around the block boundary.
        assert_eq!(md5_hex(&[b'a'; 55]), "ef1772b6dff9a122358552954ad0df65");
        assert_eq!(md5_hex(&[b'a'; 56]), "3b0c8ac703f828b04c6c197006d17218");
        assert_eq!(md5_hex(&[b'a'; 64]), "014842d480b571495a4a0363793f7367");
    }
}
