//! MCP のツール（定義と中身）
//!
//! 権限の確認は MCP クライアントに任せる。クライアントが判断できるよう、各ツールに
//! readOnlyHint / destructiveHint などの注釈を付ける（元に戻せない操作は destructiveHint）。

use nasne_core::epg::{self, Band, Channel, Epg, Event, GENRES, Loaded};
use nasne_core::model::{
    self, NO_EVENT, QUALITY_3X, QUALITY_DR, Recording, RecordingUpdate, ReserveReq, Reservation, repeat_label,
};
use nasne_core::program::{self, Query, channel_matches};
use nasne_core::{Nasne, Res, client, jst, store};
use serde_json::{Map, Value, json};
use std::collections::HashSet;
use std::time::{Duration, Instant};

/// 番組表の版数を確かめ直す間隔（続けて呼ばれたときに毎回確かめない）
const RECHECK: Duration = Duration::from_secs(60);

// ================================================================ 定義
const BANDS: [&str; 3] = ["gr", "bs", "cs"];
const QUALITIES: [&str; 2] = ["dr", "3x"];
const REPEATS: [&str; 5] = ["once", "daily", "weekly", "mon_fri", "mon_sat"];
const SORTS: [&str; 5] = ["date", "title", "channel", "duration", "size"];

/// 注釈（クライアントが権限を判断するためのヒント）
#[derive(Clone, Copy)]
enum Kind {
    /// 読み取りだけ
    Read,
    /// nasne の中身を変えるが、元に戻せる
    Write { idempotent: bool },
    /// 元に戻せない
    Destructive,
}

fn tool(name: &str, title: &str, desc: &str, kind: Kind, props: Value, required: &[&str]) -> Value {
    let (read_only, destructive, idempotent) = match kind {
        Kind::Read => (true, false, true),
        Kind::Write { idempotent } => (false, false, idempotent),
        Kind::Destructive => (false, true, true),
    };
    json!({
        "name": name,
        "title": title,
        "description": desc,
        "inputSchema": { "type": "object", "properties": props, "required": required, "additionalProperties": false },
        "annotations": {
            "title": title,
            "readOnlyHint": read_only,
            "destructiveHint": destructive,
            "idempotentHint": idempotent,
            "openWorldHint": false,
        },
    })
}

fn genres() -> String {
    GENRES.iter().filter(|g| !g.is_empty()).copied().collect::<Vec<_>>().join(" / ")
}

/// tools/list の中身
pub fn list() -> Vec<Value> {
    let band = json!({ "type": "string", "enum": BANDS, "description": "放送波（gr = 地デジ / bs / cs）。省略するとすべて" });
    let channel = json!({ "type": "string", "description": "局（名前の一部・チャンネル番号・サービス ID）。例: NHK総合、テレ東、011" });
    let time = |what: &str| {
        json!({ "type": "string", "description": format!(
            "{what}（日本時間。2026-11-12T19:00 の形。日付だけ（2026-11-12）なら、その日の番組表（4:00〜翌 4:00）の{}）",
            if what.starts_with("いつから") { "始まり" } else { "終わり" }
        )})
    };
    let limit = |d: u64| json!({ "type": "integer", "minimum": 1, "description": format!("返す件数の上限（既定 {d}）") });
    let program_id = json!({ "type": "string", "description": "番組 ID（gr:30720:1234 の形。番組表・検索の結果の id）" });
    let resv_id = json!({ "type": "string", "description": "予約 ID（list_reservations の id）" });
    let rec_id = json!({ "type": "string", "description": "録画 ID（list_recordings の id）" });
    let quality = |d: &str| {
        let mut v = json!({ "type": "string", "enum": QUALITIES, "description": "画質（dr = 放送そのまま / 3x = 3倍録画。3x は容量が約 1/3）" });
        if !d.is_empty() {
            v["default"] = d.into();
        }
        v
    };
    let repeat = |d: &str| {
        let mut v = json!({ "type": "string", "enum": REPEATS,
            "description": "繰り返し（once = 1回のみ / daily = 毎日 / weekly = 毎週（番組の曜日） / mon_fri = 月〜金 / mon_sat = 月〜土）" });
        if !d.is_empty() {
            v["default"] = d.into();
        }
        v
    };

    vec![
        tool("status", "nasne の状態", "HDD の容量と残り、いま録画中か、現在時刻。", Kind::Read, json!({}), &[]),
        tool(
            "device_info",
            "nasne の本体情報",
            "型番・ソフトウェアの版・ネットワーク・HDD・設定・DLNA クライアントなど（読み取りのみ。設定の変更は nasne HOME で行う）。",
            Kind::Read,
            json!({}),
            &[],
        ),
        tool(
            "blink",
            "nasne のランプを点灯",
            "本体のランプを点灯させて、どの nasne か見つけられるようにする。",
            Kind::Write { idempotent: true },
            json!({}),
            &[],
        ),
        tool("list_channels", "チャンネル一覧", "受信できる局の一覧（名前・チャンネル番号・サービス ID）。", Kind::Read, json!({ "band": band }), &[]),
        tool(
            "now_on_air",
            "放送中の番組",
            "いま各局で放送している番組。",
            Kind::Read,
            json!({ "band": band, "channel": channel }),
            &[],
        ),
        tool(
            "list_programs",
            "番組表",
            "指定した時間帯の番組表（局ごと・時間順）。番組は最大 8 日先まで。詳しい内容は get_program で。",
            Kind::Read,
            json!({
                "band": band, "channel": channel,
                "from": time("いつから（既定は現在時刻）"), "to": time("いつまで（既定は from の 6 時間後）"),
                "limit": limit(300),
            }),
            &[],
        ),
        tool(
            "search_programs",
            "番組を探す",
            "これから放送する番組を、タイトル・説明・出演者などの語やジャンルで探す（開始時刻順）。",
            Kind::Read,
            json!({
                "query": { "type": "string", "description": "探す語。空白で区切ると、すべてを含むもの。全角・半角や大文字・小文字は区別しない" },
                "genre": { "type": "string", "description": format!("ジャンル（{}）", genres()) },
                "band": band, "channel": channel,
                "from": time("いつから（既定は現在時刻）"), "to": time("いつまで（既定は番組表の最後まで）"),
                "limit": limit(100),
            }),
            &[],
        ),
        tool(
            "get_program",
            "番組の詳細",
            "番組の詳しい内容（出演者・番組内容など）と、予約しているかどうか。",
            Kind::Read,
            json!({ "program_id": program_id }),
            &["program_id"],
        ),
        tool(
            "list_reservations",
            "予約一覧",
            "録画予約の一覧（開始時刻順）。conflict はほかの予約と時間が重なっているもの。",
            Kind::Read,
            json!({}),
            &[],
        ),
        tool(
            "reserve_program",
            "録画予約",
            "番組を録画予約する。ほかの予約と時間が重なるときは予約せずに、重なる予約を返す（allow_conflict で、それでも予約できる）。",
            Kind::Write { idempotent: false },
            json!({
                "program_id": program_id,
                "quality": quality("3x"),
                "repeat": repeat("once"),
                "allow_conflict": { "type": "boolean", "default": false, "description": "ほかの予約と時間が重なっていても予約する（重なった部分はどちらか一方しか録画されない）" },
            }),
            &["program_id"],
        ),
        tool(
            "update_reservation",
            "予約の変更",
            "予約の画質・繰り返しを変える。raise_priority で、時間が重なっている予約のうちこの予約を優先して録画させる。",
            Kind::Write { idempotent: true },
            json!({
                "reservation_id": resv_id, "quality": quality(""), "repeat": repeat(""),
                "raise_priority": { "type": "boolean", "description": "重なっている予約のうち、この予約を優先して録画させる" },
            }),
            &["reservation_id"],
        ),
        tool(
            "cancel_reservation",
            "予約の取り消し",
            "録画予約を取り消す（繰り返しの予約は、以降の回もすべて取り消される）。",
            Kind::Destructive,
            json!({ "reservation_id": resv_id }),
            &["reservation_id"],
        ),
        tool(
            "list_failed_reservations",
            "録画できなかった予約",
            "録画できなかった予約と、その理由。",
            Kind::Read,
            json!({}),
            &[],
        ),
        tool(
            "clear_failed_reservation",
            "録画できなかった予約を消す",
            "録画できなかった予約を一覧から消す。",
            Kind::Destructive,
            json!({ "id": { "type": "string", "description": "list_failed_reservations の id" } }),
            &["id"],
        ),
        tool(
            "list_recordings",
            "録画一覧",
            "録画した番組の一覧。タイトル・説明で絞り込める。件数と合計サイズも返す。",
            Kind::Read,
            json!({
                "query": { "type": "string", "description": "タイトル・説明に含む語" },
                "unwatched_only": { "type": "boolean", "default": false, "description": "未視聴のものだけ" },
                "sort": { "type": "string", "enum": SORTS, "default": "date", "description": "並べ替え（date / size は新しい・大きい順、それ以外は昇順）" },
                "reverse": { "type": "boolean", "default": false, "description": "逆順にする" },
                "limit": limit(100),
            }),
            &[],
        ),
        tool(
            "update_recording",
            "録画の変更",
            "録画のタイトル・未視聴 / 視聴済み・保護を変える。指定しなかった項目はそのまま。",
            Kind::Write { idempotent: true },
            json!({
                "recording_id": rec_id,
                "title": { "type": "string", "description": "新しいタイトル" },
                "unwatched": { "type": "boolean", "description": "true で未視聴、false で視聴済みにする" },
                "protected": { "type": "boolean", "description": "true で保護（削除できなくする）、false で保護を解除" },
            }),
            &["recording_id"],
        ),
        tool(
            "delete_recording",
            "録画の削除",
            "録画を削除する。元に戻せない。保護中・録画中のものは削除できない。",
            Kind::Destructive,
            json!({ "recording_id": rec_id }),
            &["recording_id"],
        ),
    ]
}

// ================================================================ 引数
struct Args<'a>(&'a Map<String, Value>);

impl Args<'_> {
    /// 文字列（空なら None）
    fn str(&self, k: &str) -> Res<Option<&str>> {
        match self.0.get(k) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(s)) => Ok(Some(s.trim()).filter(|s| !s.is_empty())),
            _ => Err(format!("{k} は文字列で指定してください")),
        }
    }

    fn need(&self, k: &str) -> Res<&str> {
        self.str(k)?.ok_or_else(|| format!("{k} を指定してください"))
    }

    fn bool(&self, k: &str) -> Res<Option<bool>> {
        match self.0.get(k) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::Bool(b)) => Ok(Some(*b)),
            _ => Err(format!("{k} は true / false で指定してください")),
        }
    }

    fn limit(&self, default: usize) -> Res<usize> {
        match self.0.get("limit") {
            None | Some(Value::Null) => Ok(default),
            Some(v) => match v.as_u64().or_else(|| v.as_str().and_then(|s| s.trim().parse().ok())) {
                Some(n) if n >= 1 => Ok(n as usize),
                _ => Err("limit は 1 以上の整数で指定してください".into()),
            },
        }
    }

    fn bands(&self) -> Res<Vec<Band>> {
        match self.str("band")? {
            None => Ok(Band::ALL.to_vec()),
            Some(b) => Band::from_key(b).map(|b| vec![b]).ok_or_else(|| format!("band は gr / bs / cs のどれかです（{b}）")),
        }
    }

    fn channel(&self) -> Res<&str> {
        Ok(self.str("channel")?.unwrap_or(""))
    }

    /// 日時。日付だけなら番組表のその日の始まり（4:00）か、終わり（翌 4:00）
    fn time(&self, k: &str, end_of_day: bool) -> Res<Option<i64>> {
        let Some(s) = self.str(k)? else { return Ok(None) };
        let bad = || format!("{k} が読めません: {s}（2026-11-12T19:00 か 2026-11-12 の形）");
        let b = s.as_bytes();
        if b.len() < 10 || b[4] != b'-' || b[7] != b'-' {
            return Err(bad());
        }
        if b.len() == 10 {
            let day = jst::parse_iso(&format!("{s}T04:00:00")).ok_or_else(bad)?;
            return Ok(Some(if end_of_day { day + 86400 } else { day }));
        }
        let s = s.replacen(' ', "T", 1);
        let s = if s.len() == 16 { format!("{s}:00") } else { s };
        jst::parse_iso(&s).map(Some).ok_or_else(bad)
    }

    fn quality(&self) -> Res<Option<i64>> {
        Ok(match self.str("quality")? {
            None => None,
            Some(q) if q.eq_ignore_ascii_case("dr") => Some(QUALITY_DR),
            Some("3x" | "3X" | "3倍") => Some(QUALITY_3X),
            Some(q) => return Err(format!("quality は dr / 3x のどちらかです（{q}）")),
        })
    }

    /// 繰り返し（conditionId）。weekly は start の曜日
    fn repeat(&self, start: i64) -> Res<Option<String>> {
        Ok(match self.str("repeat")? {
            None => None,
            Some("once") => Some(model::ONCE.into()),
            Some("daily") => Some(model::DAILY.into()),
            Some("weekly") => Some(model::weekly(start)),
            Some("mon_fri") => Some(model::MON_FRI.into()),
            Some("mon_sat") => Some(model::MON_SAT.into()),
            Some(r) => return Err(format!("repeat は {} のどれかです（{r}）", REPEATS.join(" / "))),
        })
    }
}

// ================================================================ 出力
fn iso(t: i64) -> String {
    jst::dt(t).iso()
}

fn minutes(sec: i64) -> i64 {
    (sec + 30) / 60
}

fn gb(bytes: f64) -> f64 {
    (bytes / 1e8).round() / 10.0
}

fn quality_key(q: i64) -> String {
    match q {
        QUALITY_DR => "dr".into(),
        QUALITY_3X => "3x".into(),
        q => q.to_string(),
    }
}

fn repeat_key(c: &str) -> &str {
    match c {
        model::ONCE => "once",
        model::DAILY => "daily",
        model::MON_FRI => "mon_fri",
        model::MON_SAT => "mon_sat",
        c if c.len() == 2 && c.starts_with('w') => "weekly",
        c => c,
    }
}

fn event_json(band: Band, ch: &Channel, e: &Event, reserved: Option<bool>) -> Value {
    let mut v = json!({
        "id": program::id(band, e),
        "channel": ch.name,
        "start": iso(e.start),
        "end": e.dur.map(|_| iso(e.end())),
        "duration_min": e.dur.map(|d| minutes(d as i64)),
        "title": e.title,
        "description": e.desc,
        "genre": Some(e.genre_name()).filter(|g| !g.is_empty()),
    });
    if e.pay {
        v["pay"] = true.into();
    }
    if let Some(r) = reserved {
        v["reserved"] = r.into();
    }
    v
}

fn resv_json(r: &Reservation) -> Value {
    json!({
        "id": r.id,
        "title": r.title,
        "channel": r.ch,
        "start": iso(r.start),
        "end": iso(r.start + r.dur),
        "duration_min": minutes(r.dur),
        "quality": quality_key(r.quality),
        "repeat": repeat_key(&r.cond),
        "repeat_label": repeat_label(&r.cond),
        "time_specified": r.eid == NO_EVENT,
        "recording": r.recording,
        "conflict": r.conflict,
    })
}

fn rec_json(r: &Recording) -> Value {
    json!({
        "id": r.id,
        "title": r.title,
        "description": r.desc,
        "channel": r.ch,
        "start": iso(r.start),
        "duration_min": minutes(r.dur),
        "quality": quality_key(r.quality),
        "size_gb": gb(r.size),
        "unwatched": r.new,
        "protected": r.protect,
        "recording": r.recording,
    })
}

/// 番組に対応する予約（イベント ID か開始時刻が同じもの）
fn resv_of<'a>(list: &'a [Reservation], e: &Event) -> Option<&'a Reservation> {
    list.iter().find(|r| r.sid == e.sid && (r.eid == e.eid as i64 || r.start == e.start))
}

/// 予約済みの番組をすばやく調べるための表
struct Reserved(HashSet<(u16, i64)>);

impl Reserved {
    fn new(list: &[Reservation]) -> Self {
        Reserved(list.iter().flat_map(|r| [(r.sid, r.eid), (r.sid, -r.start)]).collect())
    }

    fn has(&self, e: &Event) -> bool {
        self.0.contains(&(e.sid, e.eid as i64)) || self.0.contains(&(e.sid, -e.start))
    }
}

// ================================================================ 中身
struct Cached {
    loaded: Loaded,
    checked: Option<Instant>,
}

pub struct Server {
    host: Option<String>,
    nasne: Option<Nasne>,
    epg: [Option<Cached>; 3],
}

impl Server {
    pub fn new(host: Option<String>) -> Self {
        Server { host, nasne: None, epg: [None, None, None] }
    }

    /// 接続先: --host → NASNE_HOST → 保存済み → LAN 内を探して保存
    fn connect(&mut self) -> Res<()> {
        if self.nasne.is_some() {
            return Ok(());
        }
        let host = match self.host.clone().or_else(store::load_host) {
            Some(h) => h,
            None => {
                eprintln!("nasne-mcp: nasne を探しています…");
                let h = client::discover()
                    .ok_or("nasne が見つかりませんでした。nasne-mcp の --host か環境変数 NASNE_HOST で IP アドレスを指定してください")?;
                eprintln!("nasne-mcp: nasne を見つけました（{h}）。次回からはこの接続先を使います");
                store::save_host(&h);
                h
            }
        };
        self.nasne = Some(Nasne::new(&host));
        Ok(())
    }

    fn n(&self) -> &Nasne {
        self.nasne.as_ref().expect("connect してから使う")
    }

    /// 番組表を用意する。手元の番組表は RECHECK ごとに版数を確かめ、更新されていれば取り直す
    fn load_epg(&mut self, bands: &[Band]) -> Res<()> {
        let n = self.nasne.as_ref().expect("connect してから使う");
        let stale: Vec<Band> = bands
            .iter()
            .copied()
            .filter(|b| self.epg[b.index()].as_ref().and_then(|c| c.checked).is_none_or(|t| t.elapsed() >= RECHECK))
            .collect();
        for &b in &stale {
            if self.epg[b.index()].is_none() {
                self.epg[b.index()] = epg::load_cached(&n.host, b).map(|loaded| Cached { loaded, checked: None });
            }
        }
        let cache = &self.epg;
        let results: Vec<(Band, Res<Option<Loaded>>)> = std::thread::scope(|s| {
            let hs: Vec<_> = stale
                .iter()
                .map(|&b| {
                    let known = cache[b.index()].as_ref().map(|c| c.loaded.version.as_slice());
                    (b, s.spawn(move || epg::refresh(n, b, known)))
                })
                .collect();
            hs.into_iter().map(|(b, h)| (b, h.join().unwrap_or_else(|_| Err("番組表の読み込みに失敗しました".into())))).collect()
        });
        let now = Some(Instant::now());
        for (b, r) in results {
            let slot = &mut self.epg[b.index()];
            match (r, slot.as_mut()) {
                (Ok(Some(loaded)), _) => *slot = Some(Cached { loaded, checked: now }),
                (Ok(None), Some(c)) => c.checked = now,
                (Ok(None), None) => return Err("番組表のキャッシュを読めません".into()),
                // 確かめられなくても、手元に番組表があればそれを使う
                (Err(e), Some(_)) => eprintln!("nasne-mcp: 番組表の更新を確かめられませんでした: {e}"),
                (Err(e), None) => return Err(e),
            }
        }
        Ok(())
    }

    fn epg(&self, b: Band) -> &Epg {
        &self.epg[b.index()].as_ref().expect("load_epg してから使う").loaded.epg
    }

    /// 番組 ID → 番組表・局・番組
    fn find(&mut self, id: &str) -> Res<(Band, Channel, Event)> {
        let (band, sid, eid) =
            program::parse_id(id).ok_or_else(|| format!("番組 ID が読めません: {id}（gr:30720:1234 の形。番組表・検索の結果の id）"))?;
        self.load_epg(&[band])?;
        let d = self.epg(band);
        let e = d.get(sid, eid).ok_or("番組表にその番組がありません（放送が終わった番組は消えます）")?;
        Ok((band, program::channel(d, sid), e.clone()))
    }

    /// ツールを実行する。知らないツールなら None
    pub fn call(&mut self, name: &str, args: &Map<String, Value>) -> Option<Res<Value>> {
        let a = Args(args);
        let f: fn(&mut Server, &Args) -> Res<Value> = match name {
            "status" => Server::status,
            "device_info" => Server::device_info,
            "blink" => Server::blink,
            "list_channels" => Server::list_channels,
            "now_on_air" => Server::now_on_air,
            "list_programs" => Server::list_programs,
            "search_programs" => Server::search_programs,
            "get_program" => Server::get_program,
            "list_reservations" => Server::list_reservations,
            "reserve_program" => Server::reserve_program,
            "update_reservation" => Server::update_reservation,
            "cancel_reservation" => Server::cancel_reservation,
            "list_failed_reservations" => Server::list_failed_reservations,
            "clear_failed_reservation" => Server::clear_failed_reservation,
            "list_recordings" => Server::list_recordings,
            "update_recording" => Server::update_recording,
            "delete_recording" => Server::delete_recording,
            _ => return None,
        };
        Some(self.connect().and_then(|()| f(self, &a)))
    }

    // ------------------------------------------------------------ 本体
    fn status(&mut self, _: &Args) -> Res<Value> {
        let s = self.n().status()?;
        Ok(json!({
            "now": iso(jst::now()),
            "host": self.n().host,
            "name": s.name,
            "hdd": {
                "total_gb": gb(s.total),
                "used_gb": gb(s.used),
                "free_gb": gb(s.free),
                "used_percent": if s.total > 0.0 { (s.used / s.total * 1000.0).round() / 10.0 } else { 0.0 },
            },
            "recording": s.recording,
        }))
    }

    fn device_info(&mut self, _: &Args) -> Res<Value> {
        serde_json::to_value(self.n().info()).map_err(|e| e.to_string())
    }

    fn blink(&mut self, _: &Args) -> Res<Value> {
        self.n().blink()?;
        Ok(json!({ "message": "nasne のランプを点灯させました" }))
    }

    // ------------------------------------------------------------ 番組表
    fn list_channels(&mut self, a: &Args) -> Res<Value> {
        let bands = a.bands()?;
        self.load_epg(&bands)?;
        let list: Vec<Value> = bands
            .iter()
            .flat_map(|&b| {
                self.epg(b).channels.iter().map(move |c| json!({ "band": b.key(), "name": c.name, "number": c.number, "service_id": c.sid }))
            })
            .collect();
        Ok(json!({ "channels": list }))
    }

    /// 予約一覧（取れなければ None。番組表の reserved の印は付けない）
    fn reserved(&self) -> Option<Reserved> {
        self.n().reservations().ok().map(|l| Reserved::new(&l))
    }

    fn now_on_air(&mut self, a: &Args) -> Res<Value> {
        let (bands, ch) = (a.bands()?, a.channel()?);
        self.load_epg(&bands)?;
        let now = jst::now();
        let rs = self.reserved();
        let mut list = Vec::new();
        for &b in &bands {
            let d = self.epg(b);
            for c in d.channels.iter().filter(|c| channel_matches(c, ch)) {
                if let Some(e) = d.on_air(c.sid, now) {
                    list.push(event_json(b, c, e, rs.as_ref().map(|r| r.has(e))));
                }
            }
        }
        Ok(json!({ "now": iso(now), "programs": list }))
    }

    fn list_programs(&mut self, a: &Args) -> Res<Value> {
        let (bands, ch, limit) = (a.bands()?, a.channel()?, a.limit(300)?);
        let now = jst::now();
        let from = a.time("from", false)?.unwrap_or(now);
        let to = a.time("to", true)?.unwrap_or(from + 6 * 3600);
        if to <= from {
            return Err("to は from より後にしてください".into());
        }
        self.load_epg(&bands)?;
        let rs = self.reserved();
        let (mut list, mut total, mut found) = (Vec::new(), 0, false);
        for &b in &bands {
            let d = self.epg(b);
            for c in d.channels.iter().filter(|c| channel_matches(c, ch)) {
                found = true;
                for e in d.schedule(c.sid).filter(|e| e.end() > from && e.start < to) {
                    total += 1;
                    if list.len() < limit {
                        list.push(event_json(b, c, e, rs.as_ref().map(|r| r.has(e))));
                    }
                }
            }
        }
        if !found {
            return Err(format!("局「{ch}」は見つかりませんでした（list_channels で一覧を見られます）"));
        }
        Ok(json!({
            "now": iso(now), "from": iso(from), "to": iso(to),
            "total": total, "truncated": total > list.len(), "programs": list,
        }))
    }

    fn search_programs(&mut self, a: &Args) -> Res<Value> {
        let words: Vec<&str> = a.str("query")?.unwrap_or("").split_whitespace().collect();
        let genre = a.str("genre")?;
        if words.is_empty() && genre.is_none() {
            return Err("query か genre を指定してください".into());
        }
        let (bands, ch, limit) = (a.bands()?, a.channel()?, a.limit(100)?);
        let now = jst::now();
        let from = a.time("from", false)?.unwrap_or(now);
        let to = a.time("to", true)?.unwrap_or(i64::MAX);
        let q = Query::new(&words, genre);
        self.load_epg(&bands)?;
        let rs = self.reserved();
        let mut hits: Vec<(Band, &Channel, &Event)> = Vec::new();
        for &b in &bands {
            let d = self.epg(b);
            for c in d.channels.iter().filter(|c| channel_matches(c, ch)) {
                hits.extend(d.schedule(c.sid).filter(|e| e.end() > from.max(now) && e.start < to && q.matches(e)).map(|e| (b, c, e)));
            }
        }
        hits.sort_by_key(|(_, _, e)| e.start);
        let total = hits.len();
        let list: Vec<Value> = hits.iter().take(limit).map(|(b, c, e)| event_json(*b, c, e, rs.as_ref().map(|r| r.has(e)))).collect();
        Ok(json!({ "now": iso(now), "total": total, "truncated": total > list.len(), "programs": list }))
    }

    fn get_program(&mut self, a: &Args) -> Res<Value> {
        let (band, ch, e) = self.find(a.need("program_id")?)?;
        let list = self.n().reservations()?;
        let mut v = event_json(band, &ch, &e, None);
        v["band"] = band.key().into();
        v["channel_number"] = ch.number.into();
        v["extended"] = e.ext.iter().map(|(h, t)| json!({ "heading": h, "text": t })).collect();
        v["reservation"] = resv_of(&list, &e).map(resv_json).unwrap_or(Value::Null);
        Ok(v)
    }

    // ------------------------------------------------------------ 予約
    fn find_resv(&self, id: &str) -> Res<Reservation> {
        self.n()
            .reservations()?
            .into_iter()
            .find(|r| r.id == id)
            .ok_or_else(|| format!("予約 ID {id} は見つかりません（list_reservations で一覧を見られます）"))
    }

    fn list_reservations(&mut self, _: &Args) -> Res<Value> {
        let list = self.n().reservations()?;
        Ok(json!({ "now": iso(jst::now()), "reservations": list.iter().map(resv_json).collect::<Vec<_>>() }))
    }

    fn reserve_program(&mut self, a: &Args) -> Res<Value> {
        let (band, ch, e) = self.find(a.need("program_id")?)?;
        if e.end() <= jst::now() {
            return Err("この番組の放送は終わっています".into());
        }
        let n = self.n();
        if let Some(r) = resv_of(&n.reservations()?, &e) {
            return Err(format!("すでに予約しています（予約 ID {}）", r.id));
        }
        let quality = a.quality()?.unwrap_or(QUALITY_3X);
        let cond = a.repeat(e.start)?.unwrap_or_else(|| model::ONCE.into());
        let req = ReserveReq::for_event(band, &e, quality, &cond);
        if !a.bool("allow_conflict")?.unwrap_or(false) {
            let c = n.conflicts(&req)?;
            if !c.is_empty() {
                let list: Vec<String> =
                    c.iter().map(|r| format!("{}（{}〜{}・予約 ID {}）", r.title, iso(r.start), iso(r.start + r.dur), r.id)).collect();
                return Err(format!(
                    "予約していません。次の予約と時間が重なっています（nasne のチューナーは 1 つです）: {}。\
                     それでも予約するときは allow_conflict を true にしてください（重なった部分はどちらか一方しか録画されません）",
                    list.join("、")
                ));
            }
        }
        n.reserve(&req)?;
        let made = n.reservations().ok().and_then(|l| resv_of(&l, &e).map(resv_json));
        Ok(json!({
            "message": format!("録画予約しました: {}（{}）", e.title, ch.name),
            "reservation": made,
        }))
    }

    fn update_reservation(&mut self, a: &Args) -> Res<Value> {
        let r = self.find_resv(a.need("reservation_id")?)?;
        let (quality, cond, raise) = (a.quality()?, a.repeat(r.start)?, a.bool("raise_priority")?.unwrap_or(false));
        if quality.is_none() && cond.is_none() && !raise {
            return Err("変える内容を quality / repeat / raise_priority で指定してください".into());
        }
        let (q, c) = (quality.unwrap_or(r.quality), cond.unwrap_or_else(|| r.cond.clone()));
        self.n().update_reservation(&r.id, q, &c, raise)?;
        let now = self.find_resv(&r.id).ok().map(|r| resv_json(&r));
        let what = if raise { "優先度を上げました" } else { "予約を変更しました" };
        Ok(json!({ "message": format!("{what}: {}", r.title), "reservation": now }))
    }

    fn cancel_reservation(&mut self, a: &Args) -> Res<Value> {
        let r = self.find_resv(a.need("reservation_id")?)?;
        self.n().cancel_reservation(&r.id)?;
        Ok(json!({ "message": format!("予約を取り消しました: {}（{}）", r.title, iso(r.start)) }))
    }

    fn list_failed_reservations(&mut self, _: &Args) -> Res<Value> {
        let list: Vec<Value> = self
            .n()
            .failed()?
            .iter()
            .map(|f| {
                json!({
                    "id": f.id, "title": f.title, "channel": f.ch,
                    "start": (f.start != 0).then(|| iso(f.start)), "reason": f.reason,
                })
            })
            .collect();
        Ok(json!({ "failed": list }))
    }

    fn clear_failed_reservation(&mut self, a: &Args) -> Res<Value> {
        self.n().clear_failed(a.need("id")?)?;
        Ok(json!({ "message": "録画できなかった予約を一覧から消しました" }))
    }

    // ------------------------------------------------------------ 録画
    fn find_rec(&self, id: &str) -> Res<Recording> {
        self.n()
            .recordings()?
            .into_iter()
            .find(|r| r.id == id)
            .ok_or_else(|| format!("録画 ID {id} は見つかりません（list_recordings で一覧を見られます）"))
    }

    fn list_recordings(&mut self, a: &Args) -> Res<Value> {
        let q = a.str("query")?.unwrap_or("").to_lowercase();
        let unwatched = a.bool("unwatched_only")?.unwrap_or(false);
        let limit = a.limit(100)?;
        let sort = a.str("sort")?.unwrap_or("date");
        if !SORTS.contains(&sort) {
            return Err(format!("sort は {} のどれかです（{sort}）", SORTS.join(" / ")));
        }
        let mut list: Vec<Recording> = self.n().recordings()?.into_iter().filter(|r| (!unwatched || r.new) && r.matches(&q)).collect();
        list.sort_by(|a, b| match sort {
            "size" => b.size.total_cmp(&a.size),
            "title" => a.title.cmp(&b.title),
            "channel" => a.ch.cmp(&b.ch).then(b.start.cmp(&a.start)),
            "duration" => a.dur.cmp(&b.dur),
            _ => b.start.cmp(&a.start),
        });
        if a.bool("reverse")?.unwrap_or(false) {
            list.reverse();
        }
        Ok(json!({
            "total": list.len(),
            "total_size_gb": gb(list.iter().map(|r| r.size).sum()),
            "truncated": list.len() > limit,
            "recordings": list.iter().take(limit).map(rec_json).collect::<Vec<_>>(),
        }))
    }

    fn update_recording(&mut self, a: &Args) -> Res<Value> {
        let r = self.find_rec(a.need("recording_id")?)?;
        let u = RecordingUpdate { title: a.str("title")?.map(String::from), unwatched: a.bool("unwatched")?, protect: a.bool("protected")? };
        if u.title.is_none() && u.unwatched.is_none() && u.protect.is_none() {
            return Err("変える内容を title / unwatched / protected で指定してください".into());
        }
        self.n().update_recording(&r.id, &u)?;
        let now = self.find_rec(&r.id).ok().map(|r| rec_json(&r));
        Ok(json!({ "message": format!("録画を変更しました: {}", r.title), "recording": now }))
    }

    fn delete_recording(&mut self, a: &Args) -> Res<Value> {
        let r = self.find_rec(a.need("recording_id")?)?;
        if !r.deletable() {
            return Err(format!("「{}」は{}なので削除できません", r.title, if r.recording { "録画中" } else { "保護中" }));
        }
        self.n().delete_recording(&r.id)?;
        Ok(json!({ "message": format!("削除しました: {}（{}・{} GB）", r.title, iso(r.start), gb(r.size)) }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_names_are_unique_and_dispatchable() {
        let tools = list();
        let names: HashSet<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(names.len(), tools.len());
        for t in &tools {
            let req = t["inputSchema"]["required"].as_array().unwrap();
            let props = t["inputSchema"]["properties"].as_object().unwrap();
            for r in req {
                assert!(props.contains_key(r.as_str().unwrap()), "{}: {r}", t["name"]);
            }
        }
        // 接続しなくても、知らないツールは None になる
        let mut s = Server::new(Some("192.0.2.1".into()));
        assert!(s.call("nope", &Map::new()).is_none());
    }

    #[test]
    fn time_args() {
        let m: Map<String, Value> = serde_json::from_value(json!({
            "a": "2026-11-12", "b": "2026-11-12T19:00", "c": "2026-11-12 19:00:00+09:00", "d": "11/12",
        }))
        .unwrap();
        let a = Args(&m);
        let day = jst::parse_iso("2026-11-12T04:00:00+09:00").unwrap();
        assert_eq!(a.time("a", false), Ok(Some(day)));
        assert_eq!(a.time("a", true), Ok(Some(day + 86400)));
        assert_eq!(a.time("b", false), Ok(Some(day + 15 * 3600)));
        assert_eq!(a.time("c", false), Ok(Some(day + 15 * 3600)));
        assert!(a.time("d", false).is_err());
        assert_eq!(a.time("none", false), Ok(None));
    }

    #[test]
    fn repeat_keys() {
        assert_eq!(repeat_key("w3"), "weekly");
        assert_eq!(repeat_key("w15"), "mon_fri");
        assert_eq!(repeat_key("1"), "once");
    }
}
