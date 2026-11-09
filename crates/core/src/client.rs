//! nasne との HTTP 通信（GET / 署名付き POST）と LAN 内の nasne の検出
//!
//! nasne は 2 つのポートで API を公開している。
//!   64210 … 本体の状態・設定・番組表（/status /config /chEpg）
//!   64220 … 録画・予約（/schedule /recorded）
//!
//! 放送内容そのもの・著作権保護・限定受信に関わる API には触れない。
//! 扱うのは番組表などのメタデータと録画・予約の管理だけ。

use base64::Engine as _;
use serde_json::Value;
use std::io::Read as _;
use std::sync::OnceLock;
use std::time::Duration;

use crate::gaiji;

/// エラーは利用者にそのまま見せる日本語の文
pub type Res<T> = Result<T, String>;

/// 本体の状態・設定・番組表
pub const PORT_BOX: u16 = 64210;
/// 録画・予約
pub const PORT_REC: u16 = 64220;

pub struct Nasne {
    pub host: String,
    agent: ureq::Agent,
    pin: OnceLock<String>,
}

/// 一覧系 API（reservedListGet / titleListGet）で全件を取るときの引数
pub const LIST_Q: &[(&str, &str)] = &[
    ("searchCriteria", "0"),
    ("filter", "0"),
    ("startingIndex", "0"),
    ("requestedCount", "0"),
    ("sortCriteria", "0"),
    ("withDescriptionLong", "1"),
    ("withUserData", "0"),
];

/// application/x-www-form-urlencoded（Python の urllib.parse.urlencode と同じ）
pub fn urlencode(params: &[(&str, String)]) -> String {
    let mut s = String::new();
    for (i, (k, v)) in params.iter().enumerate() {
        if i > 0 {
            s.push('&');
        }
        enc(&mut s, k);
        s.push('=');
        enc(&mut s, v);
    }
    s
}

fn enc(out: &mut String, v: &str) {
    for &b in v.as_bytes() {
        match b {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_' | b'.' | b'-' | b'~' => out.push(b as char),
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
}

fn token(pin: &str, path: &str, body: &[u8]) -> String {
    let mut h = sha1_smol::Sha1::new();
    h.update(pin.as_bytes());
    h.update(path.as_bytes());
    h.update(&body[..body.len().min(256)]);
    base64::engine::general_purpose::STANDARD.encode(h.digest().bytes())
}

fn err(e: ureq::Error) -> String {
    match e {
        ureq::Error::StatusCode(c) => format!("nasne HTTP {c}"),
        ureq::Error::Timeout(_) => "nasne が応答しません（タイムアウト）".into(),
        ureq::Error::Io(e) => format!("nasne に接続できません: {e}"),
        ureq::Error::HostNotFound => "nasne が見つかりません".into(),
        ureq::Error::ConnectionFailed => "nasne に接続できません".into(),
        e => format!("通信エラー: {e}"),
    }
}

fn read_all(mut r: ureq::http::Response<ureq::Body>) -> Res<Vec<u8>> {
    let mut v = Vec::new();
    r.body_mut()
        .as_reader()
        .read_to_end(&mut v)
        .map_err(|e| format!("受信エラー: {e}"))?;
    Ok(v)
}

impl Nasne {
    pub fn new(host: &str) -> Self {
        let agent = ureq::Agent::config_builder()
            .timeout_connect(Some(Duration::from_secs(3)))
            .timeout_global(Some(Duration::from_secs(60)))
            .build()
            .into();
        Self { host: host.trim().to_string(), agent, pin: OnceLock::new() }
    }

    fn url(&self, port: u16, path: &str, q: &str) -> String {
        if q.is_empty() {
            format!("http://{}:{port}{path}", self.host)
        } else {
            format!("http://{}:{port}{path}?{q}", self.host)
        }
    }

    pub fn get_bytes(&self, port: u16, path: &str, q: &str) -> Res<Vec<u8>> {
        read_all(self.agent.get(&self.url(port, path, q)).call().map_err(err)?)
    }

    /// GET → JSON（外字は表示用文字列に変換しない生の値）
    pub fn get_raw(&self, port: u16, path: &str, q: &[(&str, String)]) -> Res<Value> {
        let b = self.get_bytes(port, path, &urlencode(q))?;
        serde_json::from_slice(&b).map_err(|e| format!("nasne の応答を読めません: {e}"))
    }

    /// GET → JSON（外字を [字] などに変換済み）
    pub fn get(&self, port: u16, path: &str, q: &[(&str, String)]) -> Res<Value> {
        let mut v = self.get_raw(port, path, q)?;
        gaiji::fix(&mut v);
        Ok(v)
    }

    pub fn get_list(&self, port: u16, path: &str) -> Res<Value> {
        let q: Vec<(&str, String)> = LIST_Q.iter().map(|&(k, v)| (k, v.to_string())).collect();
        self.get(port, path, &q)
    }

    /// POST 署名用 PIN = MAC アドレス下位 3 バイト
    pub fn pin(&self) -> Res<&str> {
        if let Some(p) = self.pin.get() {
            return Ok(p);
        }
        let v = self.get_raw(PORT_BOX, "/status/networkIfInfoGet", &[])?;
        let mac = v["macAddr"].as_str().ok_or("MAC アドレスを取得できません")?.replace(':', "");
        let p = mac.get(6..).unwrap_or("").to_lowercase();
        Ok(self.pin.get_or_init(|| p))
    }

    /// フォーム POST（変更系）。nasne の errorcode が 0 以外ならエラーにする
    pub fn post(&self, port: u16, path: &str, params: &[(&str, String)]) -> Res<Value> {
        let body = urlencode(params);
        let tok = token(self.pin()?, path, body.as_bytes());
        let r = self
            .agent
            .post(&self.url(port, path, ""))
            .header("Content-Type", "application/x-www-form-urlencoded")
            .header("X-DTVToken", &tok)
            .send(body.as_bytes())
            .map_err(err)?;
        let v: Value = serde_json::from_slice(&read_all(r)?).map_err(|e| format!("nasne の応答を読めません: {e}"))?;
        check(v)
    }

    pub fn post_binary(&self, path: &str, body: &[u8]) -> Res<Vec<u8>> {
        let tok = token(self.pin()?, path, body);
        let r = self
            .agent
            .post(&self.url(PORT_BOX, path, ""))
            .header("Content-Type", "application/binary")
            .header("X-DTVToken", &tok)
            .send(body)
            .map_err(err)?;
        read_all(r)
    }
}

/// nasne の応答の errorcode が 0 以外ならエラーにする
pub fn check(v: Value) -> Res<Value> {
    match v.get("errorcode").and_then(Value::as_i64) {
        Some(0) | None => Ok(v),
        Some(c) => Err(format!("nasne errorcode {c}")),
    }
}

/// LAN 内の nasne を探す（自分の /24 を 64210 番で総当たり → boxNameGet で確認）
pub fn discover() -> Option<String> {
    use std::net::{IpAddr, SocketAddr, TcpStream, UdpSocket};
    let sock = UdpSocket::bind("0.0.0.0:0").ok()?;
    sock.connect("192.0.2.1:9").ok()?; // パケットは送られない（経路選択だけ）
    let IpAddr::V4(me) = sock.local_addr().ok()?.ip() else { return None };
    let o = me.octets();
    let (tx, rx) = std::sync::mpsc::channel();
    for chunk in (1u8..=254).collect::<Vec<_>>().chunks(16) {
        let chunk = chunk.to_vec();
        let tx = tx.clone();
        std::thread::spawn(move || {
            for last in chunk {
                let ip = std::net::Ipv4Addr::new(o[0], o[1], o[2], last);
                let addr = SocketAddr::new(IpAddr::V4(ip), PORT_BOX);
                if TcpStream::connect_timeout(&addr, Duration::from_millis(300)).is_ok() {
                    let n = Nasne::new(&ip.to_string());
                    if n.get_raw(PORT_BOX, "/status/boxNameGet", &[]).is_ok_and(|v| v.get("name").is_some()) {
                        let _ = tx.send(ip.to_string());
                    }
                }
            }
        });
    }
    drop(tx);
    rx.recv_timeout(Duration::from_secs(8)).ok()
}
