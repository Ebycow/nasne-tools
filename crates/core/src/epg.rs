//! nasne 内蔵 EPG / 局ロゴの取得と解析
//!
//!   GET  :64210/status/EPGVersionInfoGet?broadcastingType=N   各局の EIT セグメント版数表（バイナリ）
//!   POST :64210/chEpg/EPGGet  (application/binary)            欲しいセグメントをビットで指定して EIT を取得
//!   GET  :64210/chEpg/channelLogoDataGet?broadcastingType=N   局ロゴ
//!
//!   ・EPGGet / channelLogoDataGet の応答は「全バイト XOR 0x9D → zlib」
//!   ・EPGGet 解凍後 = 4 バイトのヘッダ + 放送波そのままの EIT セクション列
//!   ・ロゴ = zlib ストリームの連結。各エントリに PLTE を抜いた PNG（ARIB 共通固定色を補う）

use crate::client::{Nasne, PORT_BOX, Res};
use crate::{arib, gaiji, json, jst, store};
use serde::Serialize;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::io::Read as _;

/// 放送波
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Band {
    Gr,
    Bs,
    Cs,
}

impl Band {
    pub const ALL: [Band; 3] = [Band::Gr, Band::Bs, Band::Cs];

    /// nasne の broadcastingType
    pub fn code(self) -> u8 {
        match self {
            Band::Gr => 2,
            Band::Bs => 3,
            Band::Cs => 4,
        }
    }

    pub fn from_code(c: i64) -> Option<Band> {
        Band::ALL.into_iter().find(|b| b.code() as i64 == c)
    }

    /// コマンドラインや番組 ID で使う短い名前（gr / bs / cs）
    pub fn key(self) -> &'static str {
        match self {
            Band::Gr => "gr",
            Band::Bs => "bs",
            Band::Cs => "cs",
        }
    }

    pub fn from_key(k: &str) -> Option<Band> {
        Band::ALL.into_iter().find(|b| b.key().eq_ignore_ascii_case(k))
    }

    pub fn name(self) -> &'static str {
        match self {
            Band::Gr => "地デジ",
            Band::Bs => "BS",
            Band::Cs => "CS",
        }
    }

    /// Band::ALL の中の位置
    pub fn index(self) -> usize {
        self as usize
    }
}

pub const GENRES: [&str; 16] = [
    "ニュース", "スポーツ", "情報", "ドラマ", "音楽", "バラエティ", "映画", "アニメ",
    "ドキュメンタリー", "劇場", "趣味", "福祉", "", "", "拡張", "その他",
];

// ---------------------------------------------------------------- 取得
fn unxor(mut d: Vec<u8>) -> Vec<u8> {
    d.iter_mut().for_each(|b| *b ^= 0x9D);
    d
}

fn inflate(d: &[u8]) -> Res<Vec<u8>> {
    let mut out = Vec::with_capacity(d.len() * 6);
    flate2::read::ZlibDecoder::new(d)
        .read_to_end(&mut out)
        .map_err(|e| format!("EPG の解凍に失敗: {e}"))?;
    Ok(out)
}

pub fn fetch_version(n: &Nasne, band: Band) -> Res<Vec<u8>> {
    n.get_bytes(PORT_BOX, "/status/EPGVersionInfoGet", &format!("broadcastingType={}", band.code()))
}

struct VerItem {
    nid: u16,
    tsid: u16,
    sid: u16,
    basic: Option<[u8; 64]>,
    ext: Option<[u8; 64]>,
}

fn parse_version(d: &[u8]) -> Res<Vec<VerItem>> {
    let bad = || "EPGVersionInfo の形式が違います".to_string();
    let u16at = |p: usize| -> Res<u16> { Ok(u16::from_be_bytes(d.get(p..p + 2).ok_or_else(bad)?.try_into().unwrap())) };
    let arr = |p: usize| -> Res<[u8; 64]> { Ok(d.get(p..p + 64).ok_or_else(bad)?.try_into().unwrap()) };
    let count = u16at(8)?;
    let mut p = 12;
    let mut items = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let (nid, tsid, sid) = (u16at(p)?, u16at(p + 2)?, u16at(p + 4)?);
        let tp = *d.get(p + 6).ok_or_else(bad)?;
        p += 8;
        let mut it = VerItem { nid, tsid, sid, basic: None, ext: None };
        // 各局のエントリの並び:
        //   基本EITあり → [4 + 64]、さらに 拡張EITあり → [4 + 64] / 拡張なしで LDTあり → [4]
        if tp & 0x80 != 0 {
            p += 4;
            it.basic = Some(arr(p)?);
            p += 64;
            if tp & 0x40 != 0 {
                p += 4;
                it.ext = Some(arr(p)?);
                p += 64;
            } else if tp & 0x20 != 0 {
                p += 4;
            }
        }
        items.push(it);
    }
    if p != d.len() {
        return Err(format!("EPGVersionInfo の長さが合いません ({p} != {})", d.len()));
    }
    Ok(items)
}

fn bits(ver: &Option<[u8; 64]>) -> [u8; 8] {
    let mut out = [0u8; 8];
    if let Some(v) = ver {
        for (k, &x) in v.iter().enumerate() {
            if x != 255 {
                out[k / 8] |= 0x80 >> (k % 8);
            }
        }
    }
    out
}

/// 版数表に載っている全セグメントの EIT を取得 → 解凍済みの生データ（先頭 4 バイトはヘッダ）
pub fn fetch_eit(n: &Nasne, band: Band, ver: &[u8]) -> Res<Vec<u8>> {
    let items: Vec<VerItem> = parse_version(ver)?.into_iter().filter(|i| i.basic.is_some()).collect();
    if items.is_empty() {
        return Ok(Vec::new());
    }
    let mut b = vec![1, 8, band.code(), 0];
    b.extend_from_slice(&(items.len() as u16).to_be_bytes());
    b.extend_from_slice(&[0, 0]);
    for it in &items {
        for x in [it.nid, it.tsid, it.sid] {
            b.extend_from_slice(&x.to_be_bytes());
        }
        b.extend_from_slice(&[0, 0]);
        b.extend_from_slice(&bits(&it.basic));
        b.extend_from_slice(&bits(&it.ext));
    }
    inflate(&unxor(n.post_binary("/chEpg/EPGGet", &b)?))
}

pub fn fetch_channels(n: &Nasne, band: Band) -> Res<Vec<u8>> {
    n.get_bytes(PORT_BOX, "/status/channelListGet", &format!("broadcastingType={}", band.code()))
}

pub fn fetch_logos(n: &Nasne, band: Band) -> Res<Vec<u8>> {
    n.get_bytes(PORT_BOX, "/chEpg/channelLogoDataGet", &format!("broadcastingType={}", band.code()))
}

// ---------------------------------------------------------------- キャッシュつきの読み込み
/// 番組表と、その元になった版数表
pub struct Loaded {
    pub epg: Epg,
    pub version: Vec<u8>,
}

/// ディスクキャッシュから番組表を組み立てる（通信しない）
pub fn load_cached(host: &str, band: Band) -> Option<Loaded> {
    let c = store::load_epg(host, band)?;
    let epg = Epg::build(&c.eit, &c.channels).ok()?;
    Some(Loaded { epg, version: c.version })
}

/// nasne の版数表を確認し、known と違えば番組表を取り直してキャッシュに保存する。
/// 版数が同じなら（＝手元の番組表が最新なら）None。
pub fn refresh(n: &Nasne, band: Band, known: Option<&[u8]>) -> Res<Option<Loaded>> {
    let version = fetch_version(n, band)?;
    if known == Some(version.as_slice()) {
        return Ok(None);
    }
    let (eit, ch) = std::thread::scope(|s| {
        let ch = s.spawn(|| fetch_channels(n, band));
        let eit = fetch_eit(n, band, &version);
        (eit, ch.join().unwrap_or_else(|_| Err("チャンネル一覧の取得に失敗".into())))
    });
    let (eit, channels) = (eit?, ch?);
    let epg = Epg::build(&eit, &channels)?;
    store::save_epg(&n.host, band, &store::EpgCache { version: version.clone(), eit, channels });
    Ok(Some(Loaded { epg, version }))
}

/// 最新の番組表（キャッシュが最新ならそれを使い、古ければ取り直す）
pub fn load(n: &Nasne, band: Band) -> Res<Epg> {
    let cached = load_cached(&n.host, band);
    match refresh(n, band, cached.as_ref().map(|c| c.version.as_slice()))? {
        Some(l) => Ok(l.epg),
        None => cached.map(|c| c.epg).ok_or_else(|| "番組表のキャッシュを読めません".into()),
    }
}

// ---------------------------------------------------------------- EIT
#[derive(Clone, Debug, Default, Serialize)]
pub struct Event {
    #[serde(rename = "serviceId")]
    pub sid: u16,
    #[serde(rename = "eventId")]
    pub eid: u16,
    #[serde(serialize_with = "jst::ser_iso")]
    pub start: i64,
    /// 秒。未定（番組の延長がありうるとき）は None
    #[serde(rename = "duration")]
    pub dur: Option<u32>,
    pub title: String,
    #[serde(rename = "description")]
    pub desc: String,
    #[serde(serialize_with = "ser_genre")]
    pub genre: Option<u8>,
    /// スクランブル（free_CA_mode = 1）。番組表では【有料】と表示する
    pub pay: bool,
    /// 拡張形式イベント記述子（「出演者」「番組内容」などの見出しと本文）
    #[serde(rename = "extended", serialize_with = "ser_ext")]
    pub ext: Vec<(String, String)>,
    #[serde(skip)]
    share: Vec<(u16, u16)>,
}

impl Event {
    pub fn end(&self) -> i64 {
        self.start + self.dur.unwrap_or(0) as i64
    }

    /// ジャンル名（不明なら ""）
    pub fn genre_name(&self) -> &'static str {
        self.genre.map(|g| GENRES[g as usize & 15]).unwrap_or("")
    }
}

fn ser_ext<S: serde::Serializer>(ext: &[(String, String)], s: S) -> Result<S::Ok, S::Error> {
    #[derive(Serialize)]
    struct Item<'a> {
        heading: &'a str,
        text: &'a str,
    }
    s.collect_seq(ext.iter().map(|(h, t)| Item { heading: h, text: t }))
}

fn ser_genre<S: serde::Serializer>(g: &Option<u8>, s: S) -> Result<S::Ok, S::Error> {
    match g.map(|g| GENRES[g as usize & 15]).filter(|n| !n.is_empty()) {
        Some(n) => s.serialize_str(n),
        None => s.serialize_none(),
    }
}

fn bcd(x: u8) -> u32 {
    (x >> 4) as u32 * 10 + (x & 0x0F) as u32
}

fn mjd_time(b: &[u8]) -> Option<i64> {
    if b.len() < 5 || b == [0xFF; 5] {
        return None;
    }
    let mjd = ((b[0] as i64) << 8) | b[1] as i64;
    Some((mjd - 40587) * 86400 + (bcd(b[2]) * 3600 + bcd(b[3]) * 60 + bcd(b[4])) as i64 - jst::OFFSET)
}

/// 解凍済みの EPGGet 応答 → セクション列
fn sections(raw: &[u8]) -> impl Iterator<Item = &[u8]> {
    let mut p = 4;
    std::iter::from_fn(move || {
        if p + 3 > raw.len() {
            return None;
        }
        let len = (((raw[p + 1] & 0x0F) as usize) << 8) | raw[p + 2] as usize;
        let s = &raw[p..(p + 3 + len).min(raw.len())];
        p += 3 + len;
        Some(s)
    })
}

fn sl(b: &[u8], from: usize, to: usize) -> &[u8] {
    let to = to.min(b.len());
    if from >= to { &[] } else { &b[from..to] }
}

pub fn parse_events(raw: &[u8]) -> Vec<Event> {
    let mut ev: Vec<Event> = Vec::new();
    let mut index: HashMap<(u16, u16), usize> = HashMap::new();
    let mut ext_raw: HashMap<usize, Vec<(Vec<u8>, Vec<u8>)>> = HashMap::new();
    for s in sections(raw) {
        let tid = s[0];
        if !(0x50..=0x6F).contains(&tid) || s.len() < 18 {
            continue;
        }
        let sid = u16::from_be_bytes([s[3], s[4]]);
        let end = (3 + ((((s[1] & 0x0F) as usize) << 8) | s[2] as usize)).saturating_sub(4).min(s.len());
        let mut p = 14;
        while p + 12 <= end {
            let eid = u16::from_be_bytes([s[p], s[p + 1]]);
            let start = mjd_time(&s[p + 2..p + 7]);
            let dur = &s[p + 7..p + 10];
            let free_ca = (s[p + 10] >> 4) & 1;
            let dl = (((s[p + 10] & 0x0F) as usize) << 8) | s[p + 11] as usize;
            let (mut q, qe) = (p + 12, (p + 12 + dl).min(s.len()));
            p += 12 + dl;
            let Some(start) = start else { continue };
            let k = *index.entry((sid, eid)).or_insert_with(|| {
                ev.push(Event { sid, eid, start, ..Default::default() });
                ev.len() - 1
            });
            let e = &mut ev[k];
            // 基本 EIT（自TS 0x50-0x57 / 他TS 0x60-0x67。どちらも 0x08 のビットが 0）の値を採用
            if tid & 0x08 == 0 {
                e.pay = free_ca != 0;
            }
            if dur != [0xFF; 3] {
                e.dur = Some(bcd(dur[0]) * 3600 + bcd(dur[1]) * 60 + bcd(dur[2]));
            }
            while q + 2 <= qe {
                let (tag, ln) = (s[q], s[q + 1] as usize);
                let body = sl(s, q + 2, q + 2 + ln);
                q += 2 + ln;
                match tag {
                    0x4D if body.len() >= 4 => {
                        // 短形式イベント記述子
                        let nl = body[3] as usize;
                        e.title = arib::decode(sl(body, 4, 4 + nl));
                        let tl = body.get(4 + nl).copied().unwrap_or(0) as usize;
                        e.desc = arib::decode(sl(body, 5 + nl, 5 + nl + tl));
                    }
                    0x54 if !body.is_empty() => e.genre = Some(body[0] >> 4),
                    0xD6 if !body.is_empty() && body[0] >> 4 == 1 => {
                        // イベントグループ記述子: イベント共有
                        let cnt = (body[0] & 0x0F) as usize;
                        e.share = (0..cnt)
                            .filter(|k| 5 + 4 * k <= body.len())
                            .map(|k| {
                                (
                                    u16::from_be_bytes([body[1 + 4 * k], body[2 + 4 * k]]),
                                    u16::from_be_bytes([body[3 + 4 * k], body[4 + 4 * k]]),
                                )
                            })
                            .collect();
                    }
                    0x4E if body.len() >= 5 => {
                        // 拡張形式イベント記述子
                        let items = ext_raw.entry(k).or_default();
                        let il = body[4] as usize;
                        let (mut r, re) = (5, 5 + il);
                        while r < re && r < body.len() {
                            let dlen = body[r] as usize;
                            let d = sl(body, r + 1, r + 1 + dlen);
                            r += 1 + dlen;
                            let Some(&tlen) = body.get(r) else { break };
                            let t = sl(body, r + 1, r + 1 + tlen as usize);
                            r += 1 + tlen as usize;
                            match items.last_mut() {
                                Some(last) if dlen == 0 => last.1.extend_from_slice(t),
                                _ => items.push((d.to_vec(), t.to_vec())),
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    for (k, items) in ext_raw {
        ev[k].ext = items.iter().map(|(d, t)| (arib::decode(d), arib::decode(t))).collect();
    }
    // イベント共有: 中身が空のサブチャンネル側に、参照先の番組情報を写す
    let copies: Vec<(usize, usize)> = ev
        .iter()
        .enumerate()
        .filter(|(_, e)| e.title.is_empty() && !e.share.is_empty())
        .filter_map(|(i, e)| {
            e.share.iter().find_map(|r| index.get(r).copied().filter(|&j| !ev[j].title.is_empty())).map(|j| (i, j))
        })
        .collect();
    for (i, j) in copies {
        let src = ev[j].clone();
        let e = &mut ev[i];
        e.title = src.title;
        e.desc = src.desc;
        if src.genre.is_some() {
            e.genre = src.genre;
        }
        if !src.ext.is_empty() {
            e.ext = src.ext;
        }
    }
    ev
}

// ---------------------------------------------------------------- チャンネル・番組表
#[derive(Clone, Debug, Serialize)]
pub struct Channel {
    #[serde(rename = "serviceId")]
    pub sid: u16,
    pub name: String,
    pub number: i64,
}

pub struct Epg {
    pub events: Vec<Event>,
    pub index: HashMap<(u16, u16), u32>,
    /// sid → 開始時刻順の events 添字
    pub by_service: HashMap<u16, Vec<u32>>,
    pub channels: Vec<Channel>,
}

impl Epg {
    pub fn build(raw_eit: &[u8], channel_json: &[u8]) -> Res<Epg> {
        let events = parse_events(raw_eit);
        let mut by_service: HashMap<u16, Vec<u32>> = HashMap::new();
        let mut index = HashMap::with_capacity(events.len());
        for (i, e) in events.iter().enumerate() {
            by_service.entry(e.sid).or_default().push(i as u32);
            index.insert((e.sid, e.eid), i as u32);
        }
        for v in by_service.values_mut() {
            v.sort_by_key(|&i| events[i as usize].start);
        }
        let mut chj: Value = serde_json::from_slice(channel_json).map_err(|e| format!("チャンネル一覧を読めません: {e}"))?;
        gaiji::fix(&mut chj);
        let mut channels = Vec::new();
        let mut kept: HashMap<(i64, i64), Vec<HashSet<(i64, &str)>>> = HashMap::new();
        let list = chj.get("channel").and_then(Value::as_array).cloned().unwrap_or_default();
        for c in &list {
            let sid = json::i(c, "serviceId") as u16;
            let Some(evs) = by_service.get(&sid) else { continue };
            if json::i(c, "serviceType") != 1 || evs.is_empty() {
                continue;
            }
            // 同じ TS の別サービス（NHK総合2 など）で番組がほぼ同じなら隠す
            let sig: HashSet<(i64, &str)> =
                evs.iter().take(200).map(|&i| (events[i as usize].start, events[i as usize].title.as_str())).collect();
            let ts = (json::i(c, "networkId"), json::i(c, "transportStreamId"));
            let ks = kept.entry(ts).or_default();
            if ks.iter().any(|s| sig.intersection(s).count() as f64 >= 0.8 * sig.len().min(s.len()) as f64) {
                continue;
            }
            ks.push(sig);
            use unicode_normalization::UnicodeNormalization as _;
            channels.push(Channel {
                sid,
                name: json::s(c, "serviceName").nfkc().collect(),
                number: json::i(c, "channelNumber"),
            });
        }
        drop(kept);
        Ok(Epg { events, index, by_service, channels })
    }

    pub fn get(&self, sid: u16, eid: u16) -> Option<&Event> {
        self.index.get(&(sid, eid)).map(|&i| &self.events[i as usize])
    }

    /// その局の番組（開始時刻順）
    pub fn schedule(&self, sid: u16) -> impl Iterator<Item = &Event> {
        self.by_service.get(&sid).into_iter().flatten().map(|&i| &self.events[i as usize])
    }

    /// その局で時刻 t に放送している番組
    pub fn on_air(&self, sid: u16, t: i64) -> Option<&Event> {
        self.schedule(sid).find(|e| e.start <= t && e.end() > t)
    }
}

// ---------------------------------------------------------------- ロゴ
fn png_chunk(t: &[u8], b: &[u8]) -> Vec<u8> {
    let mut v = (b.len() as u32).to_be_bytes().to_vec();
    v.extend_from_slice(t);
    v.extend_from_slice(b);
    let mut h = flate2::Crc::new();
    h.update(t);
    h.update(b);
    v.extend_from_slice(&h.sum().to_be_bytes());
    v
}

/// ARIB 共通固定色（128 色）の PLTE + tRNS
fn arib_palette() -> Vec<u8> {
    let base: [Option<[u8; 3]>; 16] = [
        Some([0, 0, 0]), Some([255, 0, 0]), Some([0, 255, 0]), Some([255, 255, 0]),
        Some([0, 0, 255]), Some([255, 0, 255]), Some([0, 255, 255]), Some([255, 255, 255]),
        None, Some([170, 0, 0]), Some([0, 170, 0]), Some([170, 170, 0]),
        Some([0, 0, 170]), Some([170, 0, 170]), Some([0, 170, 170]), Some([170, 170, 170]),
    ];
    let lv = [0u8, 85, 170, 255];
    let mut rest = Vec::new();
    for r in lv {
        for g in lv {
            for b in lv {
                let c = [r, g, b];
                if !base.contains(&Some(c)) {
                    rest.push(c);
                }
            }
        }
    }
    let mut op: Vec<[u8; 3]> = base.iter().map(|c| c.unwrap_or([0, 0, 0])).collect();
    op.extend(rest.iter().copied());
    let mut alpha: Vec<u8> = base.iter().map(|c| if c.is_none() { 0 } else { 255 }).collect();
    alpha.extend(std::iter::repeat_n(255, rest.len()));
    let mut pal = op.clone();
    pal.extend_from_slice(&op[1..64]);
    alpha.extend(std::iter::repeat_n(128, 63));
    let mut v = png_chunk(b"PLTE", &pal.concat());
    v.extend(png_chunk(b"tRNS", &alpha));
    v
}

pub struct Logo {
    pub sid: u16,
    pub w: u32,
    pub h: u32,
    pub rgba: Vec<u8>,
}

fn decode_png(png: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    let mut dec = png::Decoder::new(std::io::Cursor::new(png));
    dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut r = dec.read_info().ok()?;
    let mut buf = vec![0; r.output_buffer_size()?];
    let info = r.next_frame(&mut buf).ok()?;
    let px = &buf[..info.buffer_size()];
    let rgba = match info.color_type {
        png::ColorType::Rgba => px.to_vec(),
        png::ColorType::Rgb => px.chunks_exact(3).flat_map(|c| [c[0], c[1], c[2], 255]).collect(),
        png::ColorType::GrayscaleAlpha => px.chunks_exact(2).flat_map(|c| [c[0], c[0], c[0], c[1]]).collect(),
        png::ColorType::Grayscale => px.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        _ => return None,
    };
    Some((info.width, info.height, rgba))
}

/// channelLogoDataGet の応答 → 各局で最も大きいロゴ
pub fn parse_logos(resp: &[u8]) -> Vec<Logo> {
    let data = unxor(resp.to_vec());
    let plte = arib_palette();
    let mut best: HashMap<u16, Vec<u8>> = HashMap::new();
    let mut rest: &[u8] = &data;
    let mut first = true;
    while !rest.is_empty() {
        let mut out = Vec::new();
        let mut z = flate2::bufread::ZlibDecoder::new(rest);
        if z.read_to_end(&mut out).is_err() {
            break;
        }
        let consumed = z.total_in() as usize;
        if consumed == 0 {
            break;
        }
        rest = &rest[consumed.min(rest.len())..];
        if std::mem::take(&mut first) {
            continue;
        }
        let Some(i) = out.windows(4).position(|w| w == b"\x89PNG") else { continue };
        if out.len() < 16 || out.len() < i + 33 {
            continue;
        }
        let sid = u16::from_be_bytes([out[14], out[15]]);
        let src = &out[i..];
        let mut png = src[..33].to_vec();
        png.extend_from_slice(&plte);
        png.extend_from_slice(&src[33..]);
        if best.get(&sid).is_none_or(|b| png.len() > b.len()) {
            best.insert(sid, png);
        }
    }
    best.into_iter()
        .filter_map(|(sid, png)| decode_png(&png).map(|(w, h, rgba)| Logo { sid, w, h, rgba }))
        .collect()
}
