//! 予約・録画などの、画面にもコマンドの出力にも使うデータ

use crate::epg::Band;
use crate::{json, jst};
use serde::Serialize;
use serde_json::Value;

// ---------------------------------------------------------------- 画質・繰り返し
pub const QUALITY_DR: i64 = 100;
pub const QUALITY_3X: i64 = 101;

pub fn quality_label(q: i64) -> String {
    match q {
        QUALITY_DR => "DR".into(),
        QUALITY_3X => "3倍".into(),
        q => q.to_string(),
    }
}

/// 繰り返し（conditionId）
///   "1" = 1回のみ / "d" = 毎日 / "w1"〜"w7" = 毎週 月〜日 / "w15" = 月〜金 / "w16" = 月〜土
pub const ONCE: &str = "1";
pub const DAILY: &str = "d";
pub const MON_FRI: &str = "w15";
pub const MON_SAT: &str = "w16";

/// 番組の開始時刻の曜日で「毎週」
pub fn weekly(start: i64) -> String {
    let wd = jst::dt(start).wd;
    format!("w{}", if wd == 0 { 7 } else { wd })
}

/// その番組で選べる繰り返し
pub fn repeat_options(start: i64) -> Vec<String> {
    vec![ONCE.into(), DAILY.into(), weekly(start), MON_FRI.into(), MON_SAT.into()]
}

pub fn repeat_label(c: &str) -> String {
    match c {
        ONCE => "1回のみ".into(),
        DAILY => "毎日".into(),
        MON_FRI => "月〜金".into(),
        MON_SAT => "月〜土".into(),
        _ => {
            let b = c.as_bytes();
            if b.len() == 2 && b[0] == b'w' && (b'1'..=b'7').contains(&b[1]) {
                format!("毎週{}", ["月", "火", "水", "木", "金", "土", "日"][(b[1] - b'1') as usize])
            } else {
                c.to_string()
            }
        }
    }
}

// ---------------------------------------------------------------- 表示用の小物
pub fn fmt_size(b: f64) -> String {
    if b >= 1e12 {
        format!("{:.2} TB", b / 1e12)
    } else if b >= 1e9 {
        format!("{:.1} GB", b / 1e9)
    } else {
        format!("{:.0} MB", b / 1e6)
    }
}

/// 全角英数記号 → 半角（番組表の表示と揃える）
pub fn narrow(s: &str) -> String {
    s.chars()
        .map(|c| match c as u32 {
            0xFF01..=0xFF5E => char::from_u32(c as u32 - 0xFEE0).unwrap_or(c),
            _ => c,
        })
        .collect()
}

fn start_of(v: &Value) -> i64 {
    jst::parse_iso(json::s(v, "startDateTime")).unwrap_or(0)
}

// ---------------------------------------------------------------- 本体の状態
#[derive(Clone, Debug, Serialize)]
pub struct Status {
    pub name: String,
    /// HDD の使用量・容量・残り（バイト）
    pub used: f64,
    pub total: f64,
    pub free: f64,
    /// いま録画しているか
    pub recording: bool,
}

// ---------------------------------------------------------------- 予約
/// 時間指定の予約の eventId
pub const NO_EVENT: i64 = 65535;

#[derive(Clone, Debug, Serialize)]
pub struct Reservation {
    pub id: String,
    pub title: String,
    #[serde(serialize_with = "jst::ser_iso")]
    pub start: i64,
    /// 秒
    #[serde(rename = "duration")]
    pub dur: i64,
    #[serde(rename = "channel")]
    pub ch: String,
    #[serde(rename = "serviceId")]
    pub sid: u16,
    /// 時間指定の予約は NO_EVENT
    #[serde(rename = "eventId")]
    pub eid: i64,
    pub quality: i64,
    /// 繰り返し（conditionId）
    #[serde(rename = "repeat")]
    pub cond: String,
    pub recording: bool,
    /// ほかの予約と時間が重なっている
    pub conflict: bool,
}

impl Reservation {
    pub fn from(v: &Value) -> Self {
        Reservation {
            id: json::s(v, "id").into(),
            title: narrow(json::s(v, "title")),
            start: start_of(v),
            dur: json::i(v, "duration"),
            ch: narrow(json::s(v, "channelName")),
            sid: json::i(v, "serviceId") as u16,
            eid: json::i(v, "eventId"),
            quality: json::i(v, "quality"),
            cond: json::s(v, "conditionId").into(),
            recording: json::flag(v, "recordingFlag"),
            conflict: json::flag(v, "conflictId"),
        }
    }
}

/// 録画予約の内容
#[derive(Clone, Debug)]
pub struct ReserveReq {
    pub title: String,
    pub start: i64,
    /// 秒
    pub dur: i64,
    pub band: Band,
    pub sid: u16,
    pub eid: u16,
    pub quality: i64,
    /// 繰り返し（conditionId）
    pub cond: String,
}

impl ReserveReq {
    /// 番組表の番組を予約する
    pub fn for_event(band: Band, e: &crate::epg::Event, quality: i64, cond: &str) -> Self {
        ReserveReq {
            title: e.title.clone(),
            start: e.start,
            dur: e.end() - e.start,
            band,
            sid: e.sid,
            eid: e.eid,
            quality,
            cond: cond.into(),
        }
    }
}

/// 録画できなかった予約
#[derive(Clone, Debug, Serialize)]
pub struct Failed {
    pub id: String,
    pub title: String,
    #[serde(serialize_with = "jst::ser_iso")]
    pub start: i64,
    #[serde(rename = "channel")]
    pub ch: String,
    pub reason: String,
}

impl Failed {
    pub fn from(v: &Value) -> Self {
        let reason = ["reason", "errorReason", "ngReason"]
            .iter()
            .find_map(|k| v.get(*k).filter(|v| !v.is_null()))
            .map(|v| v.as_str().map(String::from).unwrap_or_else(|| v.to_string()))
            .unwrap_or_default();
        let title = json::s(v, "title");
        Failed {
            id: json::s(v, "id").into(),
            title: if title.is_empty() { v.to_string() } else { narrow(title) },
            start: start_of(v),
            ch: narrow(json::s(v, "channelName")),
            reason,
        }
    }
}

// ---------------------------------------------------------------- 録画
#[derive(Clone, Debug, Serialize)]
pub struct Recording {
    pub id: String,
    pub title: String,
    #[serde(rename = "description")]
    pub desc: String,
    #[serde(serialize_with = "jst::ser_iso")]
    pub start: i64,
    /// 秒
    #[serde(rename = "duration")]
    pub dur: i64,
    #[serde(rename = "channel")]
    pub ch: String,
    pub quality: i64,
    /// バイト
    pub size: f64,
    /// 未視聴
    #[serde(rename = "unwatched")]
    pub new: bool,
    #[serde(rename = "protected")]
    pub protect: bool,
    pub recording: bool,
    /// 絞り込み用（タイトル・説明・詳しい説明を小文字にしてつなげたもの）
    #[serde(skip)]
    pub key: String,
}

impl Recording {
    pub fn from(v: &Value) -> Self {
        let title = narrow(json::s(v, "title"));
        let desc = narrow(json::s(v, "description"));
        let key = format!("{title} {desc} {}", json::s(v, "descriptionLong")).to_lowercase();
        Recording {
            id: json::s(v, "id").into(),
            title,
            desc,
            start: start_of(v),
            dur: json::i(v, "duration"),
            ch: narrow(json::s(v, "channelName")),
            quality: json::i(v, "quality"),
            size: v.pointer("/containerSize/main").and_then(Value::as_f64).unwrap_or(0.0),
            new: json::flag(v, "newFlag"),
            protect: json::flag(v, "protectFlag"),
            recording: json::flag(v, "recordingFlag"),
            key,
        }
    }

    /// q は小文字にしたもの。空なら常に真
    pub fn matches(&self, q: &str) -> bool {
        q.is_empty() || self.key.contains(q)
    }

    /// 保護中・録画中は削除できない
    pub fn deletable(&self) -> bool {
        !self.protect && !self.recording
    }
}

/// 録画の変更（None の項目は変えない）
#[derive(Clone, Debug, Default)]
pub struct RecordingUpdate {
    pub title: Option<String>,
    pub unwatched: Option<bool>,
    pub protect: Option<bool>,
}
