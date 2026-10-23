#![allow(dead_code)]
// 与 Go pvf/archive.go + parse.go + read.go + tokens.go 对齐的只读归档内核。

use std::collections::HashMap;

use crate::crypto::{decrypt, decrypt2, decrypt_guard, decrypt_protected, decrypt_pstring};
use crate::decode::{read_utf16_string, read_utf8_string, zlib_bytes, decode_utf16le};
use crate::path::{join_archive_path, path_key};

pub const HEADER_SIZE: usize = 0x30;
pub const FILE_ITEM_SIZE: usize = 0x18;
pub const GROUP_ITEM_SIZE: usize = 8;
pub const MAGIC_SIGNATURE: u32 = 0x6970_6B6E;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Format {
    Nkpi,
    ProtectedNkpi,
    Dfo20260901,
}

#[derive(Clone, Debug)]
pub struct FileItem {
    pub name_offset: i32,
    pub path_offset: i32,
    pub chunk_index: i32,
    pub data_offset: i32,
    pub data_size: i32,
    pub data_type: i32,
}

pub struct File {
    pub index: usize,
    pub path: String,
    pub name: String,
    pub archive_path: String,
    pub data_type: i32,
    pub size: i32,
}

pub struct HeaderSizes {
    file_count: usize,
    body_size: usize,
    group_count: usize,
    hash_size: usize,
    name_size: usize,
}

pub struct Archive {
    data: Vec<u8>,
    format: Format,
    items: Vec<FileItem>,
    groups: Vec<(i32, i32)>,
    path_idx: HashMap<String, usize>,
    files_cache: Vec<File>,
    body_off: usize,
    str_a: Vec<u8>,
    str_w: Vec<u8>,
    chunks: std::cell::RefCell<HashMap<usize, std::rc::Rc<Vec<u8>>>>,
    pub checksum: String,
}

fn i32at(data: &[u8]) -> i32 {
    i32::from_le_bytes(data[..4].try_into().unwrap())
}
fn u32at(data: &[u8]) -> u32 {
    u32::from_le_bytes(data[..4].try_into().unwrap())
}

impl Archive {
    /// 打开已解密的内层归档字节。
    pub fn open(data: Vec<u8>, checksum: String) -> Result<Archive, String> {
        let mut a = Archive {
            data,
            format: Format::Nkpi,
            items: Vec::new(),
            groups: Vec::new(),
            path_idx: HashMap::new(),
            files_cache: Vec::new(),
            body_off: 0,
            str_a: Vec::new(),
            str_w: Vec::new(),
            chunks: std::cell::RefCell::new(HashMap::new()),
            checksum,
        };
        a.parse()?;
        Ok(a)
    }

    fn parse(&mut self) -> Result<(), String> {
        if self.data.len() < HEADER_SIZE {
            return Err("header too short".into());
        }
        let mut cur = self.data[..HEADER_SIZE].to_vec();
        decrypt_protected("iNfO", &mut cur);
        if u32at(&cur) == MAGIC_SIGNATURE {
            return self.parse_dfo(&cur);
        }
        if self.parse_nkpi().is_ok() {
            return Ok(());
        }
        if self.parse_protected().is_ok() {
            return Ok(());
        }
        Err("unsupported archive format".into())
    }

    fn parse_header_sizes(header: &[u8]) -> HeaderSizes {
        HeaderSizes {
            file_count: i32at(&header[24..28]).max(0) as usize,
            body_size: i32at(&header[32..36]).max(0) as usize,
            group_count: i32at(&header[36..40]).max(0) as usize,
            hash_size: i32at(&header[40..44]).max(0) as usize,
            name_size: i32at(&header[44..48]).max(0) as usize,
        }
    }

    fn parse_dfo(&mut self, plain: &[u8]) -> Result<(), String> {
        self.reset();
        let h = Self::parse_header_sizes(plain);
        let table_size = h.file_count * FILE_ITEM_SIZE;
        let group_size = h.group_count * GROUP_ITEM_SIZE;
        let hash_offset = HEADER_SIZE + table_size;
        let name_offset = hash_offset + h.hash_size;
        let group_offset = name_offset + h.name_size;
        let body_offset = group_offset + group_size;
        if body_offset > self.data.len()
            || h.body_size != self.data.len() - body_offset
            || name_offset < hash_offset
            || group_offset < name_offset
        {
            return Err("current archive section sizes".into());
        }
        let mut hashes = self.data[hash_offset..name_offset].to_vec();
        decrypt_protected("HSrm", &mut hashes);
        if hashes.len() >= 8 {
            let n = i32at(&hashes[..4]).max(0) as usize;
            let p = 4 + n * 8;
            if p + 4 > hashes.len() {
                return Err("hash entries".into());
            }
            let m = i32at(&hashes[p..p + 4]);
            if m < 0 || (hashes.len() - p - 4) % 4 != 0 || m as usize != (hashes.len() - p - 4) / 4 {
                return Err("hash indices".into());
            }
        } else {
            return Err("short hash stream".into());
        }
        let names = &self.data[name_offset..group_offset];
        let mut pos = 8usize;
        self.str_a = decrypt_pstring_buffer(names, &mut pos, "stAs", 0xe7ad_f7ea)?
            .ok_or_else(|| "current strA missing".to_string())?;
        self.str_w = decrypt_pstring_buffer(names, &mut pos, "stWs", 0xb8de_a7ac)?
            .ok_or_else(|| "current strW missing".to_string())?;
        let mut groups = self.data[group_offset..body_offset].to_vec();
        decrypt_protected("Gidx", &mut groups);
        self.parse_groups(&groups, h.group_count)?;
        let mut previous = 0;
        for (i, g) in self.groups.iter().enumerate() {
            if g.0 <= previous || g.0 as usize > h.body_size || g.1 > 256 * 1024 * 1024 {
                return Err(format!("group {} bounds", i));
            }
            previous = g.0;
        }
        if previous as usize != h.body_size && !self.groups.is_empty() {
            return Err("body coverage".into());
        }
        self.format = Format::Dfo20260901;
        self.body_off = body_offset;
        self.parse_files(HEADER_SIZE, h.file_count)
    }

    fn parse_nkpi(&mut self) -> Result<(), String> {
        self.reset();
        let mut header = self.data[..HEADER_SIZE].to_vec();
        decrypt_guard(&mut header);
        decrypt("HeaD", &mut header);
        if u32at(&header) != MAGIC_SIGNATURE {
            return Err("bad signature".into());
        }
        let h = Self::parse_header_sizes(&header);
        self.parse_nkpi_tables(&h, false)
    }

    fn parse_protected(&mut self) -> Result<(), String> {
        self.reset();
        let mut header = self.data[..HEADER_SIZE].to_vec();
        decrypt_protected("hEAd", &mut header);
        if u32at(&header) != MAGIC_SIGNATURE {
            return Err("bad protected signature".into());
        }
        let h = Self::parse_header_sizes(&header);
        self.parse_nkpi_tables(&h, true)
    }

    fn parse_nkpi_tables(&mut self, h: &HeaderSizes, protected: bool) -> Result<(), String> {
        let table_size = h.file_count * FILE_ITEM_SIZE;
        let group_size = h.group_count * GROUP_ITEM_SIZE;
        let mut pos = HEADER_SIZE + table_size + h.hash_size;
        let name_offset = pos;
        pos += h.name_size;
        let group_offset = pos;
        pos += group_size;
        if pos > self.data.len() || h.body_size > self.data.len() - pos {
            return Err("section sizes exceed archive".into());
        }
        self.body_off = pos;
        let name_bytes = self.data[name_offset..name_offset + h.name_size].to_vec();
        if protected {
            let mut idx = 8usize;
            self.str_a = decrypt_pstring_buffer(&name_bytes, &mut idx, "StRa", 0xAA74_472E)?.unwrap_or_default();
            self.str_w = decrypt_pstring_buffer(&name_bytes, &mut idx, "StRw", 0x9A82_F037)?.unwrap_or_default();
            self.format = Format::ProtectedNkpi;
        } else {
            let mut idx = 8usize;
            self.str_a = decrypt_string_buffer(&name_bytes, &mut idx, "sTrA", 0xAA74_472E)?.unwrap_or_default();
            self.str_w = decrypt_string_buffer(&name_bytes, &mut idx, "sTrW", 0x9A82_F037)?.unwrap_or_default();
            self.format = Format::Nkpi;
        }
        let mut group_bytes = self.data[group_offset..group_offset + group_size].to_vec();
        if protected {
            decrypt_protected("grpi", &mut group_bytes);
        } else {
            decrypt("GRPI", &mut group_bytes);
        }
        self.parse_groups(&group_bytes, h.group_count)?;
        self.parse_files(HEADER_SIZE, h.file_count)
    }

    fn reset(&mut self) {
        self.items.clear();
        self.groups.clear();
        self.path_idx.clear();
        self.files_cache.clear();
        self.str_a.clear();
        self.str_w.clear();
        self.body_off = 0;
    }

    fn parse_groups(&mut self, data: &[u8], count: usize) -> Result<(), String> {
        for idx in 0..count {
            let off = idx * GROUP_ITEM_SIZE;
            if off + GROUP_ITEM_SIZE > data.len() {
                return Err("negative group size".into());
            }
            let compressed = i32at(&data[off..]);
            let original = i32at(&data[off + 4..]);
            if compressed < 0 || original < 0 {
                return Err("negative group size".into());
            }
            self.groups.push((compressed, original));
        }
        Ok(())
    }

    fn parse_files(&mut self, offset: usize, count: usize) -> Result<(), String> {
        for idx in 0..count {
            let off = offset + idx * FILE_ITEM_SIZE;
            if off + FILE_ITEM_SIZE > self.data.len() {
                return Err("file table exceeds archive".into());
            }
            let item = FileItem {
                name_offset: i32at(&self.data[off..]),
                path_offset: i32at(&self.data[off + 4..]),
                chunk_index: i32at(&self.data[off + 8..]),
                data_offset: i32at(&self.data[off + 12..]),
                data_size: i32at(&self.data[off + 16..]),
                data_type: i32at(&self.data[off + 20..]),
            };
            let name = self.resolve_string(item.name_offset as isize);
            let dir = self.resolve_string(item.path_offset as isize);
            let archive_path = join_archive_path(&dir, &name);
            self.files_cache.push(File {
                index: idx,
                path: dir,
                name,
                archive_path: archive_path.clone(),
                data_type: item.data_type,
                size: item.data_size,
            });
            if !archive_path.is_empty() {
                self.path_idx.entry(path_key(&archive_path)).or_insert(idx);
            }
            self.items.push(item);
        }
        Ok(())
    }

    pub fn find_file_index(&self, relative: &str) -> Option<usize> {
        self.path_idx.get(&path_key(relative)).copied()
    }

    pub fn file_at(&self, idx: usize) -> &File {
        &self.files_cache[idx]
    }

    pub fn all_files(&self) -> &[File] {
        &self.files_cache
    }

    /// 对应 Go `len(a.groups)`：只进产物 `source` 的诊断字段，没有任何判据读它。
    pub fn group_count(&self) -> usize {
        self.groups.len()
    }

    pub fn format(&self) -> Format {
        self.format
    }

    /// 对应 Go ReleaseReadCaches：批量遍历时释放解压块，控制常驻内存。
    pub fn release_read_caches(&self) {
        self.chunks.borrow_mut().clear();
    }

    pub fn resolve_string(&self, magic_offset: isize) -> String {
        if magic_offset < 0 {
            return String::new();
        }
        if magic_offset & 1 != 0 {
            read_utf16_string(&self.str_w, ((magic_offset >> 1) * 2) as usize)
        } else {
            read_utf8_string(&self.str_a, (magic_offset >> 1) as usize)
        }
    }

    pub fn chunk(&self, idx: usize) -> Result<std::rc::Rc<Vec<u8>>, String> {
        if let Some(c) = self.chunks.borrow().get(&idx) {
            return Ok(c.clone());
        }
        if idx >= self.groups.len() {
            return Err(format!("chunk {} out of range", idx));
        }
        let prev = if idx > 0 { self.groups[idx - 1].0 } else { 0 };
        let curr = self.groups[idx].0;
        if curr <= prev {
            return Err(format!("chunk {} compressed range invalid", idx));
        }
        let start = self.body_off + prev as usize;
        let size = (curr - prev) as usize;
        if start + size > self.data.len() {
            return Err(format!("chunk {} exceeds archive", idx));
        }
        let mut encrypted = self.data[start..start + size].to_vec();
        match self.format {
            Format::Dfo20260901 => decrypt_protected("mAIn", &mut encrypted),
            Format::ProtectedNkpi => decrypt_protected("bODy", &mut encrypted),
            Format::Nkpi => decrypt("BodY", &mut encrypted),
        }
        let chunk = zlib_bytes(&encrypted).map_err(|e| format!("chunk {} decompress: {}", idx, e))?;
        if self.groups[idx].1 >= 0 && self.groups[idx].1 as usize != chunk.len() {
            return Err(format!("chunk {} original size mismatch", idx));
        }
        let rc = std::rc::Rc::new(chunk);
        self.chunks.borrow_mut().insert(idx, rc.clone());
        Ok(rc)
    }

    pub fn read_raw_idx(&self, idx: usize) -> Result<std::rc::Rc<Vec<u8>>, String> {
        let item = self.items.get(idx).ok_or_else(|| "file index".to_string())?;
        let chunk = self.chunk(item.chunk_index.max(0) as usize)?;
        let start = item.data_offset.max(0) as usize;
        let end = start + item.data_size.max(0) as usize;
        if end > chunk.len() {
            return Err(format!("file {} data range invalid", idx));
        }
        Ok(std::rc::Rc::new(chunk[start..end].to_vec()))
    }

    pub fn read_raw(&self, path: &str) -> Result<std::rc::Rc<Vec<u8>>, String> {
        let idx = self.find_file_index(path).ok_or_else(|| format!("pvf file not found: {}", path))?;
        self.read_raw_idx(idx)
    }

    pub fn read_text_idx(&self, idx: usize) -> Result<String, String> {
        let raw = self.read_raw_idx(idx)?;
        Ok(match self.items[idx].data_type {
            1 => self.decode_script(&raw),
            3 => decode_utf16le(&raw),
            _ => String::new(),
        })
    }

    pub fn read_text(&self, path: &str) -> Result<String, String> {
        let idx = self.find_file_index(path).ok_or_else(|| format!("pvf file not found: {}", path))?;
        self.read_text_idx(idx)
    }

    fn decode_script(&self, raw: &[u8]) -> String {
        let line_count = raw.len() / 5;
        let mut b = String::with_capacity(raw.len() * 2);
        for idx in 0..line_count {
            let off = idx * 5;
            let token_type = raw[off];
            let value = i32at(&raw[off + 1..off + 5]);
            match token_type {
                0 => b.push_str(&format!("{} ", value)),
                2 => b.push_str(&format!("{:.2} ", f32::from_bits(value as u32))),
                3 => {
                    b.push('\n');
                    b.push_str(&self.resolve_string(value as isize));
                    b.push('\n');
                }
                5 => b.push_str("\n{5=``}"),
                6 => b.push_str(&format!("`{}` ", self.resolve_string(value as isize))),
                7 => b.push_str("\n{7=``}"),
                _ => {}
            }
        }
        b
    }
}

fn decrypt_string_buffer(data: &[u8], idx: &mut usize, key: &str, xor_const: u32) -> Result<Option<Vec<u8>>, String> {
    if *idx + 8 > data.len() {
        return Ok(None);
    }
    let cnt1 = u32at(&data[*idx..]);
    *idx += 8;
    let enc_size = (cnt1 ^ xor_const) as i64;
    if enc_size <= 0 || *idx + enc_size as usize > data.len() {
        return Ok(None);
    }
    let mut encrypted = data[*idx..*idx + enc_size as usize].to_vec();
    *idx += enc_size as usize;
    decrypt2(key, &mut encrypted);
    Ok(zlib_bytes(&encrypted).ok())
}

fn decrypt_pstring_buffer(data: &[u8], idx: &mut usize, key: &str, xor_const: u32) -> Result<Option<Vec<u8>>, String> {
    if *idx + 8 > data.len() {
        return Ok(None);
    }
    let encoded_size = u32at(&data[*idx..]);
    let encoded_original_size = u32at(&data[*idx + 4..]);
    *idx += 8;
    let enc_size = (encoded_size ^ xor_const) as i64;
    if enc_size <= 0 || *idx + enc_size as usize > data.len() {
        return Err("invalid protected string buffer size".into());
    }
    let mut encrypted = data[*idx..*idx + enc_size as usize].to_vec();
    *idx += enc_size as usize;
    decrypt_pstring(key, &mut encrypted);
    let out = match zlib_bytes(&encrypted) {
        Ok(v) => v,
        Err(_) => return Ok(None),
    };
    let want = (encoded_original_size ^ enc_size as u32) as i64;
    if want >= 0 && want as usize != out.len() {
        return Err(format!("protected string buffer size mismatch: want {} got {}", want, out.len()));
    }
    Ok(Some(out))
}
