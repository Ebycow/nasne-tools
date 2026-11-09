//! 日付・時刻の引数（日本時間。番組表の 1 日は 4:00〜翌 4:00）

use nasne_core::{Res, jst};

/// 番組表の 1 日の始まり（4:00）
pub const DAY_START: i64 = 4 * 3600;

/// "today" / "tomorrow" / "+2" / "11-12" / "11/12" / "2026-11-12" → その日の 0:00（UNIX 時刻）
pub fn date(s: &str, now: i64) -> Res<i64> {
    let bad = || format!("日付が読めません: {s}（today / tomorrow / +2 / 11-12 / 2026-11-12）");
    // 深夜 0〜4 時でも「今日」は暦の今日（nasne-gui の番組表と同じ）
    let today = jst::midnight(now);
    let s = s.trim();
    match s {
        "today" | "今日" => return Ok(today),
        "tomorrow" | "明日" => return Ok(today + 86400),
        _ => {}
    }
    if let Some(n) = s.strip_prefix('+') {
        let n: i64 = n.parse().map_err(|_| bad())?;
        return Ok(today + n * 86400);
    }
    let parts: Vec<&str> = s.split(['-', '/']).collect();
    let num = |x: &str| x.parse::<u32>().map_err(|_| bad());
    let cur = jst::dt(now);
    let (y, m, d) = match parts.as_slice() {
        [y, m, d] => (num(y)? as i32, num(m)?, num(d)?),
        [m, d] => {
            let (m, d) = (num(m)?, num(d)?);
            // 年をまたぐ指定（12 月に 1-5 など）は来年
            let y = if m < cur.mo && cur.mo - m > 6 { cur.y + 1 } else { cur.y };
            (y, m, d)
        }
        _ => return Err(bad()),
    };
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return Err(bad());
    }
    Ok(jst::days_from_civil(y, m, d) * 86400 - jst::OFFSET)
}

/// "HH:MM"（その日の中で。0〜3 時は翌日の深夜。"25:30" のような書き方も可）→ UNIX 時刻
pub fn time_on(day: i64, s: &str) -> Res<i64> {
    let bad = || format!("時刻が読めません: {s}（HH:MM）");
    let (h, m) = s.split_once(':').unwrap_or((s, "0"));
    let (h, m): (i64, i64) = (h.trim().parse().map_err(|_| bad())?, m.trim().parse().map_err(|_| bad())?);
    if !(0..=28).contains(&h) || !(0..60).contains(&m) {
        return Err(bad());
    }
    let sec = h * 3600 + m * 60;
    Ok(day + if sec < DAY_START { sec + 86400 } else { sec })
}

/// "11/10(火) 20:00〜20:45"
pub fn span(start: i64, end: i64) -> String {
    let (s, e) = (jst::dt(start), jst::dt(end));
    format!("{} {}〜{}", s.date(), s.time(), e.time())
}

/// "2026/11/10(火) 20:00"
pub fn full(t: i64) -> String {
    let d = jst::dt(t);
    format!("{}/{} {}", d.y, d.date(), d.time())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_and_times() {
        let now = jst::parse_iso("2026-11-10T01:30:00+09:00").unwrap();
        let today = jst::parse_iso("2026-11-10T00:00:00+09:00").unwrap();
        assert_eq!(date("today", now), Ok(today));
        assert_eq!(date("+1", now), Ok(today + 86400));
        assert_eq!(date("11-12", now), Ok(today + 2 * 86400));
        assert_eq!(date("2026/11/12", now), Ok(today + 2 * 86400));
        assert_eq!(date("1-5", jst::parse_iso("2026-12-20T12:00:00+09:00").unwrap()), Ok(jst::parse_iso("2027-01-05T00:00:00+09:00").unwrap()));
        assert!(date("x", now).is_err());
        assert_eq!(time_on(today, "20:00"), Ok(today + 20 * 3600));
        assert_eq!(time_on(today, "1:30"), Ok(today + 25 * 3600 + 1800));
        assert_eq!(time_on(today, "25:30"), Ok(today + 25 * 3600 + 1800));
    }
}
