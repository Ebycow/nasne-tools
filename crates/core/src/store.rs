//! ディスク上のキャッシュ（番組表の生データ・ロゴ）と設定。nasne-gui と nasne コマンドで共有する。
//!
//!   macOS   ~/Library/Caches/nasne-tools/<host>/      ~/Library/Application Support/nasne-tools/settings.txt
//!   Windows %LOCALAPPDATA%\nasne-tools\<host>\        %APPDATA%\nasne-tools\settings.txt
//!   Linux   $XDG_CACHE_HOME/nasne-tools/<host>/       $XDG_CONFIG_HOME/nasne-tools/settings.txt

use crate::epg::Band;
use std::path::PathBuf;

const DIR: &str = "nasne-tools";

fn env(k: &str) -> Option<PathBuf> {
    std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from)
}

fn base(cache: bool) -> Option<PathBuf> {
    let home = env("HOME");
    let p = if cfg!(target_os = "macos") {
        home?.join("Library").join(if cache { "Caches" } else { "Application Support" })
    } else if cfg!(windows) {
        if cache { env("LOCALAPPDATA")? } else { env("APPDATA")? }
    } else if cache {
        env("XDG_CACHE_HOME").or_else(|| home.map(|h| h.join(".cache")))?
    } else {
        env("XDG_CONFIG_HOME").or_else(|| home.map(|h| h.join(".config")))?
    };
    Some(p.join(DIR))
}

fn cache_dir(host: &str) -> Option<PathBuf> {
    let safe: String = host.chars().map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' { c } else { '_' }).collect();
    Some(base(true)?.join(safe))
}

fn write_atomic(path: PathBuf, data: &[u8]) {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    // GUI と CLI が同時に書いても混ざらないよう、一時ファイルはプロセスごとに分ける
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    if std::fs::write(&tmp, data).is_ok() {
        let _ = std::fs::rename(&tmp, &path);
    }
}

/// 長さ付きで複数のバイト列を 1 ファイルにまとめる
fn pack(parts: &[&[u8]]) -> Vec<u8> {
    let mut v = Vec::with_capacity(parts.iter().map(|p| p.len() + 4).sum());
    for p in parts {
        v.extend_from_slice(&(p.len() as u32).to_le_bytes());
        v.extend_from_slice(p);
    }
    v
}

fn unpack(mut d: &[u8]) -> Option<Vec<Vec<u8>>> {
    let mut out = Vec::new();
    while !d.is_empty() {
        let n = u32::from_le_bytes(d.get(..4)?.try_into().ok()?) as usize;
        out.push(d.get(4..4 + n)?.to_vec());
        d = &d[4 + n..];
    }
    Some(out)
}

pub struct EpgCache {
    pub version: Vec<u8>,
    pub eit: Vec<u8>,
    pub channels: Vec<u8>,
}

pub fn load_epg(host: &str, band: Band) -> Option<EpgCache> {
    let d = std::fs::read(cache_dir(host)?.join(format!("epg{}.bin", band.code()))).ok()?;
    let mut p = unpack(&d)?.into_iter();
    Some(EpgCache { version: p.next()?, eit: p.next()?, channels: p.next()? })
}

pub fn save_epg(host: &str, band: Band, c: &EpgCache) {
    if let Some(dir) = cache_dir(host) {
        write_atomic(dir.join(format!("epg{}.bin", band.code())), &pack(&[&c.version, &c.eit, &c.channels]));
    }
}

pub fn load_logos(host: &str, band: Band) -> Option<Vec<u8>> {
    std::fs::read(cache_dir(host)?.join(format!("logo{}.bin", band.code()))).ok()
}

pub fn save_logos(host: &str, band: Band, d: &[u8]) {
    if let Some(dir) = cache_dir(host) {
        write_atomic(dir.join(format!("logo{}.bin", band.code())), d);
    }
}

// ---------------------------------------------------------------- 設定
/// 接続先。環境変数 NASNE_HOST があればそれを、なければ保存済みの接続先を使う
pub fn load_host() -> Option<String> {
    if let Some(h) = std::env::var("NASNE_HOST").ok().filter(|h| !h.trim().is_empty()) {
        return Some(h.trim().to_string());
    }
    saved_host()
}

/// 保存済みの接続先だけ（NASNE_HOST は見ない）
pub fn saved_host() -> Option<String> {
    let s = std::fs::read_to_string(base(false)?.join("settings.txt")).ok()?;
    s.lines().find_map(|l| l.strip_prefix("host=")).map(|h| h.trim().to_string()).filter(|h| !h.is_empty())
}

pub fn save_host(host: &str) {
    if let Some(dir) = base(false) {
        write_atomic(dir.join("settings.txt"), format!("host={}\n", host.trim()).as_bytes());
    }
}
