// encoding_rs 侧镜像解码：读 ../inputs.csv -> ../rs_out.csv
use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Write};

fn hex_encode(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{:02X}", b));
    }
    s
}

fn hex_decode(s: &str) -> Option<Vec<u8>> {
    if s.is_empty() {
        return Some(vec![]);
    }
    if s.len() % 2 != 0 {
        return None;
    }
    let mut out = Vec::with_capacity(s.len() / 2);
    let bs = s.as_bytes();
    let mut i = 0;
    while i < bs.len() {
        let hi = (bs[i] as char).to_digit(16)?;
        let lo = (bs[i + 1] as char).to_digit(16)?;
        out.push((hi * 16 + lo) as u8);
        i += 2;
    }
    Some(out)
}

fn main() -> std::io::Result<()> {
    let mode = std::env::args().nth(1).unwrap_or_default();
    let (in_name, out_name) = if mode == "pvf" { ("../pvf_lines.csv", "../pvf_rs.csv") } else { ("../inputs.csv", "../rs_out.csv") };
    let infile = File::open(in_name)?;
    let outfile = File::create(out_name)?;
    let mut w = BufWriter::with_capacity(1 << 20, outfile);
    writeln!(w, "input_hex,rs_gbk_hex,rs_gb18030_hex,rs_gbk_repl,rs_gb18030_repl")?;
    let mut n = 0usize;
    for line in BufReader::new(infile).lines() {
        let line = line?;
        let line = line.trim();
        let Some(bytes) = hex_decode(line) else { continue };
        let (cow_gbk, _, repl_gbk) = encoding_rs::GBK.decode(&bytes);
        let (cow_18, _, repl_18) = encoding_rs::GB18030.decode(&bytes);
        // encoding_rs 对不可映射字节产出 NCR（&#xFFFD; 或 &#x7F; 形态），这里同时保留有损 UTF-8 视图
        writeln!(
            w,
            "{},{},{},{},{}",
            line,
            hex_encode(cow_gbk.as_bytes()),
            hex_encode(cow_18.as_bytes()),
            repl_gbk,
            repl_18
        )?;
        n += 1;
    }
    println!("rs_out.csv：{} 行", n);
    Ok(())
}
