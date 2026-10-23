//! 方案 C 的 Rust 侧：PVF 内核 + 离线索引构建器（`build` feature）+ 纯 JSON 读取器。

// 纯运行时读取侧（任何 feature 都编译）：只依赖 serde。
pub mod gmindex;
pub mod gojson;
pub mod growth;
pub mod path;
pub mod quest;

// 离线构建侧（PVF 内核 + 索引构建器），只有 `build` feature 下才编译。
#[cfg(feature = "build")]
pub mod archive;
#[cfg(feature = "build")]
pub mod client;
#[cfg(feature = "build")]
pub mod crypto;
#[cfg(feature = "build")]
pub mod decode;
#[cfg(feature = "build")]
pub mod equipmentexport;
#[cfg(feature = "build")]
pub mod growthbuild;
#[cfg(feature = "build")]
pub mod itemindex;
#[cfg(feature = "build")]
pub mod names;
#[cfg(feature = "build")]
pub mod questbuild;
#[cfg(feature = "build")]
pub mod slotmap;

#[cfg(feature = "build")]
use archive::Archive;

/// 等价于 Go 的 openGMArchive：读入整个 Script.pvf，必要时用 DFO.exe+sk.dat 解外层。
#[cfg(feature = "build")]
pub fn open_gm_archive(pvf: &std::path::Path) -> Result<Archive, String> {
    let mut data = std::fs::read(pvf).map_err(|e| format!("读取 PVF：{}", e))?;
    let plain = {
        let mut h = data[..archive::HEADER_SIZE].to_vec();
        crypto::decrypt_protected("iNfO", &mut h);
        u32::from_le_bytes(h[..4].try_into().unwrap()) == archive::MAGIC_SIGNATURE
    } || {
        let mut h = data[..archive::HEADER_SIZE].to_vec();
        crypto::decrypt_guard(&mut h);
        crypto::decrypt("HeaD", &mut h);
        u32::from_le_bytes(h[..4].try_into().unwrap()) == archive::MAGIC_SIGNATURE
    } || {
        let mut h = data[..archive::HEADER_SIZE].to_vec();
        crypto::decrypt_protected("hEAd", &mut h);
        u32::from_le_bytes(h[..4].try_into().unwrap()) == archive::MAGIC_SIGNATURE
    };
    if !plain {
        let dir = pvf.parent().ok_or("PVF 路径无父目录")?;
        client::unwrap_outer(&mut data, &dir.join("DFO.exe"), &dir.join("sk.dat"))?;
    }
    let checksum = sha256_hex(&data);
    Archive::open(data, checksum)
}

/// 与 Go 一致的 sha256 十六进制（sha2 crate）。
#[cfg(feature = "build")]
pub(crate) fn sha256_hex(data: &[u8]) -> String {
    use sha2::Digest;
    let mut h = sha2::Sha256::new();
    // 分块，避免一次性借用整个 866MB
    for chunk in data.chunks(8 << 20) {
        h.update(chunk);
    }
    let out = h.finalize();
    out.iter().map(|b| format!("{:02x}", b)).collect()
}
