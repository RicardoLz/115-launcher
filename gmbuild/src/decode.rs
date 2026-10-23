#![allow(dead_code)]
// 与 Go pvf/decode.go 对齐的字符串池解码。

pub fn zlib_bytes(data: &[u8]) -> std::io::Result<Vec<u8>> {
    use flate2::read::ZlibDecoder;
    use std::io::Read;
    let mut out = Vec::new();
    ZlibDecoder::new(data).read_to_end(&mut out)?;
    Ok(out)
}

pub fn read_utf8_string(buf: &[u8], start: usize) -> String {
    if start >= buf.len() {
        return String::new();
    }
    let Some(end) = buf[start..].iter().position(|b| *b == 0) else {
        return String::new();
    };
    let raw = &buf[start..start + end];
    match std::str::from_utf8(raw) {
        Ok(s) => s.to_string(),
        Err(_) => {
            // x/text GB18030 NewDecoder().String 永不返回 err（FFFD 置换），与 Go 一致。
            let (cow, _, _) = encoding_rs::GB18030.decode(raw);
            cow.into_owned()
        }
    }
}

pub fn decode_utf16le(data: &[u8]) -> String {
    if data.len() < 2 {
        return String::new();
    }
    let units: Vec<u16> = data
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    utf16_decode(&units)
}

// Go unicode/utf16.Decode 语义：合法代理对合成码点，落单代理替换为 U+FFFD。
fn utf16_decode(units: &[u16]) -> String {
    let mut out = String::with_capacity(units.len());
    let mut i = 0;
    while i < units.len() {
        let u = units[i];
        i += 1;
        if !(0xD800..0xDC00).contains(&u) {
            push_unit(&mut out, u);
            continue;
        }
        if i < units.len() && (0xDC00..0xE000).contains(&units[i]) {
            let c = u as u32;
            let d = units[i] as u32;
            let cp = 0x1_0000 + ((c - 0xD800) << 10) + (d - 0xDC00);
            out.push(char::from_u32(cp).unwrap_or(char::REPLACEMENT_CHARACTER));
            i += 1;
        } else {
            push_unit(&mut out, u);
        }
    }
    out
}

fn push_unit(out: &mut String, u: u16) {
    match char::from_u32(u as u32) {
        Some(c) => out.push(c),
        None => out.push(char::REPLACEMENT_CHARACTER),
    }
}

pub fn read_utf16_string(buf: &[u8], start: usize) -> String {
    if start >= buf.len() {
        return String::new();
    }
    let mut end = start;
    while end + 1 < buf.len() {
        if buf[end] == 0 && buf[end + 1] == 0 {
            break;
        }
        end += 2;
    }
    if end <= start {
        return String::new();
    }
    decode_utf16le(&buf[start..end])
}
