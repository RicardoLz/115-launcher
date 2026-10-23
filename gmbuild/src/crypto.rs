// 与 Go pvf/crypto.go 逐位对齐的解密核心。
#![allow(dead_code)]

pub fn decrypt_guard(buf: &mut [u8]) {
    if buf.len() < 28 {
        return;
    }
    for idx in 24..28 {
        buf[idx] ^= 0x55;
    }
}

pub fn decrypt(key: &str, buf: &mut [u8]) {
    decrypt_core(key, buf, 0x269EC3);
}

pub fn decrypt2(key: &str, buf: &mut [u8]) {
    decrypt_core(key, buf, 0x269EC9);
}

pub fn decrypt_protected(key: &str, buf: &mut [u8]) {
    decrypt_protected_core(key, buf, 0x269EC3);
}

pub fn decrypt_pstring(key: &str, buf: &mut [u8]) {
    decrypt_protected_core(key, buf, 0x269EC9);
}

fn decrypt_core(key: &str, buf: &mut [u8], magic: u32) {
    if key.is_empty() || buf.is_empty() {
        return;
    }
    let k = key.as_bytes();
    if k.len() < 4 {
        return;
    }
    let size = buf.len();
    let mut tail = size;
    let mut seed: u32 = 0x76826701u32
        .wrapping_mul(k[0] as u32)
        .wrapping_add(0x1C1u32.wrapping_mul(
            k[3] as u32 + 0x1C1u32.wrapping_mul(k[2] as u32 + 0x1C1u32.wrapping_mul(k[1] as u32)),
        ));
    if size >= 4 {
        let quad_count = size >> 2;
        tail = size - (quad_count << 2);
        for idx in 0..quad_count {
            let t1 = 0x343FDu32.wrapping_mul(seed).wrapping_add(magic);
            seed = 0x343FDu32.wrapping_mul(t1).wrapping_add(magic);
            let xor_key = ((seed >> 16) & 0xFFFF) + (t1 & 0xFFFF_0000);
            let off = idx << 2;
            let value = u32::from_le_bytes(buf[off..off + 4].try_into().unwrap()) ^ xor_key;
            buf[off..off + 4].copy_from_slice(&value.to_le_bytes());
        }
    }
    if tail > 0 {
        let t1 = 0x343FDu32.wrapping_mul(seed).wrapping_add(magic);
        let t2 = 0x343FDu32.wrapping_mul(t1).wrapping_add(magic);
        let final_key = (t1 & 0xFFFF_0000) + ((t2 >> 16) & 0xFFFF);
        let key_bytes = final_key.to_le_bytes();
        let start = size - tail;
        for idx in 0..tail {
            buf[start + idx] ^= key_bytes[idx];
        }
    }
}

fn utf16_encode_rune(c: char) -> Vec<u16> {
    let cp = c as u32;
    if cp < 0x1_0000 {
        vec![cp as u16]
    } else {
        let cp = cp - 0x1_0000;
        vec![(0xD800 + (cp >> 10)) as u16, (0xDC00 + (cp & 0x3FF)) as u16]
    }
}

fn decrypt_protected_core(key: &str, buf: &mut [u8], magic: u32) {
    if key.is_empty() || buf.is_empty() {
        return;
    }
    let words: Vec<u16> = key.chars().flat_map(utf16_encode_rune).collect();
    if words.len() < 4 {
        return;
    }
    let size = buf.len();
    let mut tail = size;
    let mut seed: u32 = 0x339E_9711u32
        .wrapping_mul(words[0] as u32)
        .wrapping_add(0x393u32.wrapping_mul(
            words[3] as u32 + 0x393u32.wrapping_mul(words[2] as u32 + 0x393u32.wrapping_mul(words[1] as u32)),
        ));
    if size >= 4 {
        let quad_count = size >> 2;
        tail = size - (quad_count << 2);
        for idx in 0..quad_count {
            let t1 = 0x343FDu32.wrapping_mul(seed).wrapping_add(magic);
            seed = 0x343FDu32.wrapping_mul(t1).wrapping_add(magic);
            let xor_key = (t1 & 0xFFFF_0000) + (seed >> 16);
            let off = idx << 2;
            let value = u32::from_le_bytes(buf[off..off + 4].try_into().unwrap()) ^ xor_key;
            buf[off..off + 4].copy_from_slice(&value.to_le_bytes());
        }
    }
    if tail > 0 {
        let t1 = 0x343FDu32.wrapping_mul(seed).wrapping_add(magic);
        let t2 = 0x343FDu32.wrapping_mul(t1).wrapping_add(magic);
        let final_key = (t1 & 0xFFFF_0000) + (t2 >> 16);
        let key_bytes = final_key.to_le_bytes();
        let start = size - tail;
        for idx in 0..tail {
            buf[start + idx] ^= key_bytes[idx];
        }
    }
}
