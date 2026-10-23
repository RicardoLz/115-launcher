// 与 Go encoding/json 逐字节一致的字符串转义（含 HTML 转义 <、>、&）。

pub fn escape_string(out: &mut Vec<u8>, s: &str) {
    out.push(b'"');
    let mut rest = s;
    while !rest.is_empty() {
        let ch = rest.chars().next().unwrap();
        let width = ch.len_utf8();
        rest = &rest[width..];
        match ch {
            '"' => out.extend_from_slice(b"\\\""),
            '\\' => out.extend_from_slice(b"\\\\"),
            '\n' => out.extend_from_slice(b"\\n"),
            '\r' => out.extend_from_slice(b"\\r"),
            '\t' => out.extend_from_slice(b"\\t"),
            c if (c as u32) < 0x20 => {
                out.extend_from_slice(format!("\\u{:04x}", c as u32).as_bytes());
            }
            '<' => out.extend_from_slice(b"\\u003c"),
            '>' => out.extend_from_slice(b"\\u003e"),
            '&' => out.extend_from_slice(b"\\u0026"),
            '\u{7f}' => out.extend_from_slice(b"\\u007f"),
            '\u{2028}' | '\u{2029}' => {
                out.extend_from_slice(format!("\\u{:04x}", ch as u32).as_bytes());
            }
            c => out.extend_from_slice(c.encode_utf8(&mut [0u8; 4]).as_bytes()),
        }
    }
    out.push(b'"');
}

#[cfg(test)]
mod tests {
    use super::escape_string;

    fn esc(s: &str) -> String {
        let mut v = Vec::new();
        escape_string(&mut v, s);
        String::from_utf8(v).unwrap()
    }

    #[test]
    fn html_and_control() {
        assert_eq!(esc("a"), String::from_utf8(vec![0x22, 0x61, 0x22]).unwrap());
        assert_eq!(
            esc(&[char::from_u32(0x7f).unwrap(), char::from_u32(0x2028).unwrap()].iter().collect::<String>()),
            String::from_utf8(
                vec![0x22, 0x5c, 0x75, 0x30, 0x30, 0x37, 0x66, 0x5c, 0x75, 0x32, 0x30, 0x32, 0x38, 0x22]
            )
            .unwrap()
        );
        assert_eq!(
            esc("a<b>c&d"),
            String::from_utf8(
                vec![
                    0x22, 0x61, 0x5c, 0x75, 0x30, 0x30, 0x33, 0x63, 0x62, 0x5c, 0x75, 0x30, 0x30, 0x33, 0x65, 0x63,
                    0x5c, 0x75, 0x30, 0x30, 0x32, 0x36, 0x64, 0x22
                ]
            )
            .unwrap()
        );
    }
}
