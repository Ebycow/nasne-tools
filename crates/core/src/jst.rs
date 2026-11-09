//! 日本時間（UTC+9 固定）の日時計算。nasne は国内専用なので常に JST で表示する。

pub const OFFSET: i64 = 9 * 3600;

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Dt {
    pub y: i32,
    pub mo: u32,
    pub d: u32,
    pub h: u32,
    pub mi: u32,
    pub s: u32,
    /// 0 = 日曜
    pub wd: u32,
}

const WD: [&str; 7] = ["日", "月", "火", "水", "木", "金", "土"];

/// 1970-01-01 からの日数 → (年, 月, 日)
fn civil(z: i64) -> (i32, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = (yoe + era * 400 + if m <= 2 { 1 } else { 0 }) as i32;
    (y, m, d)
}

/// (年, 月, 日) → 1970-01-01 からの日数
pub fn days_from_civil(y: i32, m: u32, d: u32) -> i64 {
    let y = y as i64 - if m <= 2 { 1 } else { 0 };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let m = m as i64;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

pub fn dt(t: i64) -> Dt {
    let l = t + OFFSET;
    let days = l.div_euclid(86400);
    let sec = l.rem_euclid(86400) as u32;
    let (y, mo, d) = civil(days);
    Dt { y, mo, d, h: sec / 3600, mi: sec / 60 % 60, s: sec % 60, wd: (days + 4).rem_euclid(7) as u32 }
}

/// JST のその日 0:00 の UNIX 時刻
pub fn midnight(t: i64) -> i64 {
    (t + OFFSET).div_euclid(86400) * 86400 - OFFSET
}

impl Dt {
    pub fn date(&self) -> String {
        format!("{}/{}({})", self.mo, self.d, WD[self.wd as usize])
    }
    pub fn time(&self) -> String {
        format!("{:02}:{:02}", self.h, self.mi)
    }
    pub fn iso(&self) -> String {
        format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}+09:00", self.y, self.mo, self.d, self.h, self.mi, self.s)
    }
}

/// "2026-11-09T20:00:00+09:00" / "...Z" / オフセットなし（JST とみなす）→ UNIX 時刻
pub fn parse_iso(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() < 19 {
        return None;
    }
    let num = |r: std::ops::Range<usize>| -> Option<i64> { s.get(r)?.parse().ok() };
    let (y, mo, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (h, mi, sec) = (num(11..13)?, num(14..16)?, num(17..19)?);
    let mut p = 19;
    if b.get(p) == Some(&b'.') {
        p += 1;
        while p < b.len() && b[p].is_ascii_digit() {
            p += 1;
        }
    }
    let off = match b.get(p) {
        Some(b'Z') => 0,
        Some(&c @ (b'+' | b'-')) => {
            let oh = num(p + 1..p + 3)?;
            let om = if b.get(p + 3) == Some(&b':') { num(p + 4..p + 6)? } else { num(p + 3..p + 5).unwrap_or(0) };
            (oh * 3600 + om * 60) * if c == b'-' { -1 } else { 1 }
        }
        _ => OFFSET,
    };
    Some(days_from_civil(y as i32, mo as u32, d as u32) * 86400 + h * 3600 + mi * 60 + sec - off)
}

/// serde 用: UNIX 時刻を "2026-11-09T20:00:00+09:00" で書き出す
pub fn ser_iso<S: serde::Serializer>(t: &i64, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&dt(*t).iso())
}

/// 長さの表示（"30分" "1時間" "1時間30分"）
pub fn fmt_dur(sec: i64) -> String {
    let m = (sec as f64 / 60.0).round() as i64;
    if m >= 60 {
        if m % 60 != 0 { format!("{}時間{}分", m / 60, m % 60) } else { format!("{}時間", m / 60) }
    } else {
        format!("{m}分")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn roundtrip() {
        let t = parse_iso("2026-11-09T20:05:07+09:00").unwrap();
        let d = dt(t);
        assert_eq!((d.y, d.mo, d.d, d.h, d.mi, d.s, d.wd), (2026, 11, 9, 20, 5, 7, 1));
        assert_eq!(d.iso(), "2026-11-09T20:05:07+09:00");
        assert_eq!(parse_iso("2026-11-09T11:05:07Z"), Some(t));
        assert_eq!(parse_iso("2026-11-09T20:05:07"), Some(t));
        assert_eq!(dt(midnight(t)).h, 0);
    }
}
