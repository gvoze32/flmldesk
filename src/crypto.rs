//! PES 2021 / Football Life save encryption (format documented by pesXdecrypter).
//!
//! A save is: 320-byte encryption header, 208-byte file header, then the
//! description, logo, data and serial blocks. Every block is XORed with an
//! MT19937-derived keystream. `enc_header[0..256]` holds the SHA-512 digests of
//! the four blocks, so editing `data` changes the rolling key of every block.

use sha2::{Digest, Sha512};

use crate::Error;

pub const MASTER_KEY: [u8; 64] = [
    0x90, 0x61, 0xD8, 0x66, 0x43, 0x77, 0x24, 0xF8, 0x92, 0xBA, 0xB8, 0x71, 0x21, 0xC7, 0x60, 0x63,
    0xF0, 0x91, 0x9A, 0x7D, 0xED, 0x47, 0x80, 0xDE, 0x51, 0xF5, 0xDD, 0xD1, 0x08, 0xFE, 0x32, 0x84,
    0xF5, 0x09, 0x92, 0x00, 0xB2, 0x3E, 0x88, 0x9F, 0xEB, 0x24, 0x43, 0x05, 0x58, 0x76, 0x00, 0x22,
    0x9B, 0xFE, 0xEC, 0xF6, 0x50, 0x00, 0x29, 0xD3, 0x42, 0x75, 0x50, 0xB9, 0xEC, 0xD2, 0xF6, 0x75,
];
pub const ENC_HEADER_SIZE: usize = 320;
pub const FILE_HEADER_SIZE: usize = 208;

/// MT19937 seeded with `init_by_array`.
struct Mt {
    s: [u32; 624],
    i: usize,
}

impl Mt {
    fn new(key: &[u32]) -> Self {
        let mut s = [0u32; 624];
        s[0] = 19_650_218;
        for j in 1..624 {
            s[j] = 1_812_433_253u32
                .wrapping_mul(s[j - 1] ^ (s[j - 1] >> 30))
                .wrapping_add(j as u32);
        }
        let (mut i, mut j) = (1usize, 0usize);
        for _ in 0..624.max(key.len()) {
            s[i] = (s[i] ^ (s[i - 1] ^ (s[i - 1] >> 30)).wrapping_mul(1_664_525))
                .wrapping_add(key[j])
                .wrapping_add(j as u32);
            i += 1;
            j += 1;
            if i >= 624 {
                s[0] = s[623];
                i = 1;
            }
            if j >= key.len() {
                j = 0;
            }
        }
        for _ in 0..623 {
            s[i] = (s[i] ^ (s[i - 1] ^ (s[i - 1] >> 30)).wrapping_mul(1_566_083_941))
                .wrapping_sub(i as u32);
            i += 1;
            if i >= 624 {
                s[0] = s[623];
                i = 1;
            }
        }
        s[0] = 0x8000_0000;
        Mt { s, i: 624 }
    }

    fn next(&mut self) -> u32 {
        if self.i >= 624 {
            for k in 0..624 {
                let y = (self.s[k] & 0x8000_0000) | (self.s[(k + 1) % 624] & 0x7FFF_FFFF);
                self.s[k] = self.s[(k + 397) % 624] ^ (y >> 1) ^ if y & 1 != 0 { 0x9908_B0DF } else { 0 };
            }
            self.i = 0;
        }
        let mut y = self.s[self.i];
        self.i += 1;
        y ^= y >> 11;
        y ^= (y << 7) & 0x9D2C_5680;
        y ^= (y << 15) & 0xEFC6_0000;
        y ^ (y >> 18)
    }
}

/// Encrypt or decrypt `data` in place (the cipher is symmetric).
pub fn crypt_in_place(key: &[u8; 64], data: &mut [u8]) {
    let words: Vec<u32> = key.chunks_exact(4).map(|c| u32::from_le_bytes(c.try_into().unwrap())).collect();
    let mut mt = Mt::new(&words);
    let (mut c0, mut c1, mut c2, mut c3) = (mt.next(), mt.next(), mt.next(), mt.next());
    let mut chunks = data.chunks_exact_mut(4);
    for chunk in &mut chunks {
        let c4 = mt.next();
        let v = u32::from_le_bytes((&*chunk).try_into().unwrap()) ^ c4 ^ c3 ^ c2 ^ c1 ^ c0;
        chunk.copy_from_slice(&v.to_le_bytes());
        c0 = c1.rotate_left(17);
        c1 = c2.rotate_left(11);
        c2 = c3.rotate_left(7);
        c3 = c4.rotate_right(13);
    }
    let rest = chunks.into_remainder();
    if !rest.is_empty() {
        let mut buf = [0u8; 4];
        buf[..rest.len()].copy_from_slice(rest);
        let v = u32::from_le_bytes(buf) ^ mt.next() ^ c3 ^ c2 ^ c1 ^ c0;
        let n = rest.len();
        rest.copy_from_slice(&v.to_le_bytes()[..n]);
    }
}

pub fn crypt(key: &[u8; 64], data: &[u8]) -> Vec<u8> {
    let mut out = data.to_vec();
    crypt_in_place(key, &mut out);
    out
}

fn fold64(base: &[u8], extra: &[u8]) -> [u8; 64] {
    let mut out: [u8; 64] = base[..64].try_into().unwrap();
    for (i, v) in extra.iter().enumerate() {
        out[i % 64] ^= v;
    }
    out
}

fn header_key_mask() -> [u8; 64] {
    let mut out = [0u8; 64];
    for (i, chunk) in MASTER_KEY.chunks_exact(8).enumerate() {
        for (j, b) in chunk.iter().rev().enumerate() {
            out[i * 8 + j] = *b;
        }
    }
    out
}

fn block_key(rolling: &[u8; 64], param: u64) -> [u8; 64] {
    let mut out = *rolling;
    for (i, b) in out.iter_mut().enumerate() {
        *b ^= ((param >> (8 * (i % 8))) & 0xFF) as u8;
    }
    out
}

fn sha512(data: &[u8]) -> [u8; 64] {
    Sha512::digest(data).into()
}

#[derive(Clone, Debug)]
pub struct SaveFile {
    /// Decrypted 320-byte header (digests + salt).
    pub enc_header: Vec<u8>,
    /// Decrypted 208-byte header.
    pub file_header: Vec<u8>,
    pub desc: Vec<u8>,
    pub logo: Vec<u8>,
    pub data: Vec<u8>,
    pub serial: Vec<u8>,
}

fn header_text(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}

impl SaveFile {
    pub fn file_type(&self) -> String {
        header_text(&self.file_header[144..176])
    }

    pub fn game_version(&self) -> String {
        header_text(&self.file_header[176..208])
    }

    fn blocks(&self) -> [&Vec<u8>; 4] {
        [&self.desc, &self.logo, &self.data, &self.serial]
    }
}

/// Decrypt a save. `verify = false` skips the SHA-512 check (read-only use).
pub fn unpack(blob: &[u8], verify: bool) -> Result<SaveFile, Error> {
    if blob.len() < ENC_HEADER_SIZE + FILE_HEADER_SIZE {
        return Err(Error::new("File terlalu kecil untuk save PES/FL."));
    }
    let salt = &blob[256..320];
    let mut enc = crypt(&fold64(salt, &header_key_mask()), &blob[..ENC_HEADER_SIZE]);
    enc[256..320].copy_from_slice(salt);
    let rolling = fold64(&enc[..64], &enc[64..320]);
    let mut pos = ENC_HEADER_SIZE;
    let fh = crypt(&block_key(&rolling, 208), &blob[pos..pos + FILE_HEADER_SIZE]);
    pos += FILE_HEADER_SIZE;
    let field = |i: usize| u32::from_le_bytes(fh[64 + 4 * i..68 + 4 * i].try_into().unwrap()) as usize;
    let (data_size, logo_size, desc_size, serial_len) = (field(0), field(1), field(2), field(3));
    let sizes = [desc_size, logo_size, data_size, serial_len * 2];
    if pos + sizes.iter().sum::<usize>() != blob.len() {
        return Err(Error::new(
            "Ukuran blok tidak cocok dengan ukuran file; bukan save PES 2021/FL atau file rusak.",
        ));
    }
    let mut blocks: Vec<Vec<u8>> = Vec::with_capacity(4);
    for (index, size) in sizes.iter().enumerate() {
        blocks.push(crypt(&block_key(&rolling, index as u64), &blob[pos..pos + size]));
        pos += size;
    }
    if verify {
        for (index, name) in ["desc", "logo", "data", "serial"].iter().enumerate() {
            if enc[index * 64..(index + 1) * 64] != sha512(&blocks[index]) {
                return Err(Error::new(format!(
                    "Hash SHA-512 blok {name} tidak cocok; file rusak atau bukan save PES 2021/FL."
                )));
            }
        }
    }
    let serial = blocks.pop().unwrap();
    let data = blocks.pop().unwrap();
    let logo = blocks.pop().unwrap();
    let desc = blocks.pop().unwrap();
    Ok(SaveFile { enc_header: enc, file_header: fh, desc, logo, data, serial })
}

pub fn pack(save: &SaveFile) -> Vec<u8> {
    let mut enc = save.enc_header.clone();
    for (index, block) in save.blocks().iter().enumerate() {
        enc[index * 64..(index + 1) * 64].copy_from_slice(&sha512(block));
    }
    let salt: Vec<u8> = enc[256..320].to_vec();
    let mut out = crypt(&fold64(&salt, &header_key_mask()), &enc);
    out[256..320].copy_from_slice(&salt);
    let rolling = fold64(&enc[..64], &enc[64..320]);
    out.extend(crypt(&block_key(&rolling, 208), &save.file_header));
    for (index, block) in save.blocks().iter().enumerate() {
        out.extend(crypt(&block_key(&rolling, index as u64), block));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::Sha256;

    fn sha256_hex(data: &[u8]) -> String {
        Sha256::digest(data).iter().map(|b| format!("{b:02x}")).collect()
    }

    /// Vectors produced by the pesXdecrypter-compatible Python implementation.
    #[test]
    fn cipher_matches_reference_vectors() {
        let payload: Vec<u8> = (0..1027u32).map(|i| (i * 7 % 256) as u8).collect();
        let key: [u8; 64] = core::array::from_fn(|i| i as u8);
        let out = crypt(&key, &payload);
        assert_eq!(out[..16].iter().map(|b| format!("{b:02x}")).collect::<String>(), "4910382ec79ae867489a51caef47d988");
        assert_eq!(sha256_hex(&out), "c561564e89fb24ab15efbc58644dae88b761da64ffb2fb0851a30e14d610c203");
        let mut key2 = key;
        key2[60..].fill(0);
        assert_eq!(sha256_hex(&crypt(&key2, &payload)), "2f5d73bbbe66ef51fd89c6b7c33b3b7cc92afaefe67c490c9e8c6ff2a87fce86");
    }
}
