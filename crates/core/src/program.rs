//! 番組 ID・番組の検索・局の絞り込み（nasne コマンドと MCP サーバで共有）

use crate::epg::{self, Band, Channel, Epg, Event};
use crate::{Nasne, Res};
use unicode_normalization::UnicodeNormalization as _;

/// 番組 ID（"gr:30720:1234" = 放送波:サービス ID:イベント ID）
pub fn id(band: Band, e: &Event) -> String {
    format!("{}:{}:{}", band.key(), e.sid, e.eid)
}

/// 番組 ID を読む。形が違えば None
pub fn parse_id(s: &str) -> Option<(Band, u16, u16)> {
    let p: Vec<&str> = s.trim().split(':').collect();
    let [b, sid, eid] = p.as_slice() else { return None };
    Some((Band::from_key(b)?, sid.parse().ok()?, eid.parse().ok()?))
}

/// 比べるための正規化（全角・半角をそろえて小文字に）
pub fn normalize(s: &str) -> String {
    s.nfkc().collect::<String>().to_lowercase()
}

/// 局の指定（名前の一部・チャンネル番号・サービス ID）に合うか。空なら常に真
pub fn channel_matches(c: &Channel, q: &str) -> bool {
    let q = q.trim();
    if q.is_empty() {
        return true;
    }
    if let Ok(x) = q.parse::<i64>() {
        return c.number == x || c.sid as i64 == x;
    }
    normalize(&c.name).contains(&normalize(q))
}

/// その局（番組表にない局はサービス ID から仮の名前を付ける）
pub fn channel(d: &Epg, sid: u16) -> Channel {
    d.channels.iter().find(|c| c.sid == sid).cloned().unwrap_or(Channel { sid, name: format!("サービス {sid}"), number: 0 })
}

/// 番組の検索条件（タイトル・説明・詳しい説明に、すべての語を含むもの）
pub struct Query {
    words: Vec<String>,
    genre: Option<String>,
}

impl Query {
    pub fn new<S: AsRef<str>>(words: &[S], genre: Option<&str>) -> Self {
        let words = words.iter().map(|w| normalize(w.as_ref())).filter(|w| !w.is_empty()).collect();
        let genre = genre.map(normalize).filter(|g| !g.is_empty());
        Query { words, genre }
    }

    pub fn matches(&self, e: &Event) -> bool {
        if let Some(g) = &self.genre
            && (e.genre_name().is_empty() || !normalize(e.genre_name()).contains(g))
        {
            return false;
        }
        if self.words.is_empty() {
            return true;
        }
        let mut hay = format!("{} {}", e.title, e.desc);
        for (k, v) in &e.ext {
            hay.push(' ');
            hay.push_str(k);
            hay.push(' ');
            hay.push_str(v);
        }
        let hay = normalize(&hay);
        self.words.iter().all(|w| hay.contains(w))
    }
}

/// 番組表を放送波ごとに並列で読む（キャッシュが最新ならそれを使う）
pub fn load(n: &Nasne, bands: &[Band]) -> Res<Vec<(Band, Epg)>> {
    std::thread::scope(|s| {
        let hs: Vec<_> = bands.iter().map(|&b| (b, s.spawn(move || epg::load(n, b)))).collect();
        hs.into_iter()
            .map(|(b, h)| {
                let e = h.join().unwrap_or_else(|_| Err("番組表の読み込みに失敗しました".into()))?;
                Ok((b, e))
            })
            .collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids() {
        let mut e = Event::default();
        (e.sid, e.eid) = (30720, 12388);
        assert_eq!(id(Band::Gr, &e), "gr:30720:12388");
        assert_eq!(parse_id(" BS:101:5 "), Some((Band::Bs, 101, 5)));
        assert_eq!(parse_id("gr:30720"), None);
        assert_eq!(parse_id("xx:1:2"), None);
    }

    #[test]
    fn query() {
        let mut e = Event::default();
        e.title = "大河ドラマ　ＡＢＣ".into();
        e.genre = Some(3);
        e.ext = vec![("出演者".into(), "山田太郎".into())];
        assert!(Query::new(&["abc", "山田"], None).matches(&e));
        assert!(Query::new(&["大河"], Some("ドラマ")).matches(&e));
        assert!(!Query::new(&["大河"], Some("アニメ")).matches(&e));
        assert!(!Query::new(&["鈴木"], None).matches(&e));
        assert!(Query::new::<&str>(&[], None).matches(&e));
    }
}
