const fs = require("fs");
let src = fs.readFileSync("D:/115us-Tauri/gmbuild/src/names.rs", "utf8");
const i = src.indexOf("#[cfg(test)]");

const hex = (arr) => (typeof arr === "string" ? codes(arr) : arr).map((c) => "0x" + c.toString(16).padStart(2, "0")).join(", ");
const codes = (s) => Array.from(s).map((c) => c.codePointAt(0));
const CR = 0x0d, NL = 0x0a, BS = 0x5c, QU = 0x22;

// 测试里不写任何反斜杠转义字面量：期望值一律用字节数组拼出来，
// 避免 shell / 传输层把 \n、\u0026 之类吞成真控制字符。
const textBytes = [...codes(" name_1 >A  B"), NL, ...codes("//comment"), NL, NL, ...codes("name_2>x  "), NL, ...codes("name_3>a"), CR, ...codes("b")];
const secondBytes = [...codes("name_1>second"), NL, ...codes("name_2>other")];
const expectBytes = [...codes("{"), QU, ...codes("name_1"), QU, 0x3a, QU, ...codes("a"), BS, ...codes("u0026b"), QU, 0x2c, QU, ...codes("name_2"), QU, 0x3a, QU, ...codes("b"), BS, ...codes("u003ec"), QU, 0x7d];

const mod = `#[cfg(test)]
mod tests {
    use super::*;

    fn s(bytes: &[u8]) -> String {
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    #[test]
    fn prefix_filter_matches_go() {
        assert!(wanted("name_101001153"));
        assert!(wanted("growtype_name_0"));
        assert!(wanted("common_rarity_4"));
        assert!(!wanted("skill_1"));
        assert!(!wanted("message_2"));
    }

    #[test]
    fn line_parsing_rules() {
        // key 前段 TrimSpace、值原样保留（不 TrimRight 空格）；行尾 CR 被去掉
        let text = s(&[${hex(textBytes)}]);
        let mut names = HashMap::new();
        let n = collect_table_lines(&text, &mut names);
        assert_eq!(n, 3);
        assert_eq!(*names.get("name_1").unwrap(), s(&[${hex("A  B")}]));
        assert_eq!(*names.get("name_2").unwrap(), s(&[${hex("x  ")}]));
        assert_eq!(*names.get("name_3").unwrap(), s(&[${[...codes("a"), CR, ...codes("b")]}]));
        assert!(!names.contains_key("comment"));
    }

    #[test]
    fn first_table_wins() {
        let mut names = HashMap::new();
        collect_table_lines("name_1>first", &mut names);
        let second = s(&[${hex(secondBytes)}]);
        let n = collect_table_lines(&second, &mut names);
        assert_eq!(n, 1);
        assert_eq!(names.get("name_1").unwrap(), "first");
    }

    #[test]
    fn writer_sorts_keys_and_escapes_html() {
        let mut names = HashMap::new();
        names.insert("name_2".to_string(), "b>c".to_string());
        names.insert("name_1".to_string(), "a&b".to_string());
        let mut out = Vec::new();
        let n = write_client_names(&names, &mut out).unwrap();
        assert_eq!(n, 2);
        assert_eq!(s(&out), s(&[${hex(expectBytes)}]));
    }
}
`;
src = src.slice(0, i) + mod;
fs.writeFileSync("D:/115us-Tauri/gmbuild/src/names.rs", src);
