#![allow(dead_code)]
// 与 Go pvf/client.go 对齐：从 DFO.exe + sk.dat 取区段密钥，AES-256-CBC 解外层前缀。

use cipher::{BlockDecryptMut, KeyIvInit};
use rsa::{pkcs1::DecodeRsaPrivateKey, pkcs8::DecodePrivateKey, Pkcs1v15Encrypt, RsaPrivateKey};
use rsa::traits::PublicKeyParts;

type Aes256CbcDec = cbc::Decryptor<aes::Aes256>;

const SEGMENT_SIZE: usize = 0xa0_0000;
const PREFIX_SIZE: usize = 0x2800;

pub fn client_segment_keys(exe: &std::path::Path, sk: &std::path::Path) -> Result<Vec<Vec<u8>>, String> {
    use std::io::{Read, Seek};
    let mut f = std::fs::File::open(exe).map_err(|e| format!("读取客户端程序：{}", e))?;
    let mut dos = [0u8; 64];
    f.seek(std::io::SeekFrom::Start(0)).unwrap();
    f.read_exact(&mut dos).map_err(|e| e.to_string())?;
    if &dos[0..2] != b"MZ" {
        return Err("客户端程序不是有效 PE 文件".into());
    }
    let pe_off = u32::from_le_bytes(dos[60..64].try_into().unwrap()) as u64;
    f.seek(std::io::SeekFrom::Start(pe_off)).map_err(|e| e.to_string())?;
    let mut sig = [0u8; 4];
    f.read_exact(&mut sig).map_err(|e| e.to_string())?;
    if &sig[0..2] != b"PE" {
        return Err("客户端程序不是有效 PE 文件".into());
    }
    let mut coff = [0u8; 20];
    f.read_exact(&mut coff).map_err(|e| e.to_string())?;
    let machine = u16::from_le_bytes([coff[0], coff[1]]);
    let optional_size = u16::from_le_bytes([coff[16], coff[17]]) as usize;
    let mut optional = vec![0u8; optional_size];
    f.read_exact(&mut optional).map_err(|e| e.to_string())?;
    if machine != 0x8664 {
        return Err("此客户端程序版本尚不支持".into());
    }
    let image_base = u64::from_le_bytes(optional[24..32].try_into().unwrap());
    let mut sections = Vec::new();
    loop {
        let mut sec = [0u8; 40];
        match f.read_exact(&mut sec) {
            Ok(()) => {}
            Err(_) => break,
        }
        let vsize = u32::from_le_bytes(sec[8..12].try_into().unwrap()) as u64;
        let va = u32::from_le_bytes(sec[12..16].try_into().unwrap()) as u64;
        let raw_size = u32::from_le_bytes(sec[16..20].try_into().unwrap()) as u64;
        let raw_off = u32::from_le_bytes(sec[20..24].try_into().unwrap()) as u64;
        sections.push((va, vsize.max(raw_size), raw_off, raw_size));
    }
    let read_va = |f: &mut std::fs::File, rva: u64, n: usize| -> Result<Vec<u8>, String> {
        for (va, vs, off, _rs) in sections.iter() {
            if rva >= *va && rva + n as u64 <= *va + *vs {
                let mut data = vec![0u8; n];
                f.seek(std::io::SeekFrom::Start(off + (rva - va))).map_err(|e| e.to_string())?;
                f.read_exact(&mut data).map_err(|e| e.to_string())?;
                return Ok(data);
            }
        }
        Err("此客户端版本的资源加载布局尚不支持".into())
    };
    let literal = |f: &mut std::fs::File, rva: u64| -> Result<Vec<u8>, String> {
        let pointer = read_va(f, rva, 8)?;
        let va = u64::from_le_bytes(pointer.try_into().unwrap());
        if va < image_base {
            return Err("客户端资源地址无效".into());
        }
        let data = read_va(f, va - image_base, 4096)?;
        let i = data.iter().position(|b| *b == 0).ok_or("客户端资源参数缺少结束标记".to_string())?;
        Ok(data[..i].to_vec())
    };
    let private_data = literal(&mut f, 0x0dc9_8110)?;
    let s = String::from_utf8_lossy(&private_data);
    let block = pem::parse(s.trim()).map_err(|_| "此客户端版本的资源密钥格式尚不支持".to_string())?;
    let private = if block.tag() == "RSA PRIVATE KEY" {
        RsaPrivateKey::from_pkcs1_der(block.contents()).map_err(|_| "客户端资源密钥格式无效".to_string())?
    } else {
        RsaPrivateKey::from_pkcs8_der(block.contents()).map_err(|_| "客户端资源密钥格式无效".to_string())?
    };
    let key_text = literal(&mut f, 0x0dc9_8118)?;
    let key = hex_decode(&String::from_utf8_lossy(&key_text)).ok_or_else(|| "客户端资源参数格式无效".to_string())?;
    let data = std::fs::read(sk).map_err(|e| format!("读取 sk.dat：{}", e))?;
    let size = private.size();
    if data.is_empty() || data.len() >= 1 << 20 || data.len() % size != 0 {
        return Err("sk.dat 长度无效".into());
    }
    let mut plain = Vec::new();
    for chunk in data.chunks(size) {
        let part = private
            .decrypt(Pkcs1v15Encrypt, chunk)
            .map_err(|_| "sk.dat 与客户端程序不匹配".to_string())?;
        plain.extend_from_slice(&part);
    }
    let head = plain.len() / 256 * 256;
    client_cbc(&key, &mut plain[..head])?;
    if plain.is_empty() || plain.len() % 32 != 0 {
        return Err("sk.dat 区段密钥表无效".into());
    }
    Ok(plain.chunks(32).map(<[u8]>::to_vec).collect())
}

pub fn client_cbc(key: &[u8], data: &mut [u8]) -> Result<(), String> {
    if key.len() != 32 || data.len() % 16 != 0 {
        return Err("客户端归档密钥或分组长度无效".into());
    }
    let mut dec = Aes256CbcDec::new_from_slices(key, &[0u8; 16]).map_err(|_| "密钥长度无效".to_string())?;
    for chunk in data.chunks_mut(16) {
        dec.decrypt_block_mut(chunk.into());
    }
    Ok(())
}

/// 若归档头不是直接可解的内层格式，就地解外层区段前缀。
pub fn unwrap_outer(data: &mut [u8], exe: &std::path::Path, sk: &std::path::Path) -> Result<(), String> {
    let keys = client_segment_keys(exe, sk)?;
    for (i, key) in keys.iter().enumerate() {
        let start = i * SEGMENT_SIZE;
        if start >= data.len() {
            break;
        }
        if start + PREFIX_SIZE > data.len() {
            return Err("客户端归档加密区段不完整".into());
        }
        client_cbc(key, &mut data[start..start + PREFIX_SIZE])?;
    }
    let mut header = data[..crate::archive::HEADER_SIZE].to_vec();
    crate::crypto::decrypt_protected("iNfO", &mut header);
    if u32::from_le_bytes(header[..4].try_into().unwrap()) != crate::archive::MAGIC_SIGNATURE {
        return Err("客户端资源解密校验失败，请检查 DFO.exe、sk.dat 与 Script.pvf 是否配套".into());
    }
    Ok(())
}

fn hex_decode(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 {
        return None;
    }
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(s.len() / 2);
    let mut i = 0;
    while i < bytes.len() {
        let hi = (bytes[i] as char).to_digit(16)?;
        let lo = (bytes[i + 1] as char).to_digit(16)?;
        out.push((hi * 16 + lo) as u8);
        i += 2;
    }
    Some(out)
}
