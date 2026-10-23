#![allow(dead_code)]
// 与 Go pvf/path.go 对齐的路径归一化。

pub fn normalize_path(value: &str) -> String {
    let mut value = value.trim().replace('\\', "/").trim().to_string();
    while value.starts_with("./") || value.starts_with('/') {
        if let Some(rest) = value.strip_prefix("./") {
            value = rest.to_string();
        }
        if let Some(rest) = value.strip_prefix('/') {
            value = rest.to_string();
        }
    }
    if value.is_empty() {
        return String::new();
    }
    // Go path.Clean 等价：本工作流的路径均为资源相对路径（无 ".."、无前导 /），
    // 归一化即去空段与 "." 段、折叠多余斜杠。
    let parts: Vec<&str> = value.split('/').filter(|p| !p.is_empty() && *p != ".").collect();
    let mut clean = parts.join("/");
    if clean == ".." || clean.is_empty() {
        return String::new();
    }
    if clean.ends_with('/') && clean.len() > 1 {
        clean.pop();
    }
    clean
}

pub fn path_key(value: &str) -> String {
    normalize_path(value).to_lowercase()
}

pub fn join_archive_path(dir: &str, name: &str) -> String {
    if dir.trim().is_empty() {
        return normalize_path(name);
    }
    if name.trim().is_empty() {
        return normalize_path(dir);
    }
    normalize_path(&format!("{}/{}", dir, name))
}
