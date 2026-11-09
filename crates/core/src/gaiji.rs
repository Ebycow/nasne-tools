//! nasne の JSON が返す ARIB 外字（Unicode 私用領域）と表示用文字列（[字] [再] など）の変換
//!
//! 表示には変換した文字列を使い、nasne へ送り返すときは元の外字コードに戻す
//! （変換したまま送ると nasne 上のタイトルが壊れる）。

use serde_json::Value;

/// 外字コード → 表示用文字列
pub const GAIJI: &[(u32, &str)] = &[
    (0xE0F8, "[HV]"), (0xE0F9, "[SD]"), (0xE0FA, "[P]"), (0xE0FB, "[W]"), (0xE0FC, "[MV]"),
    (0xE0FD, "[手]"), (0xE0FE, "[字]"), (0xE0FF, "[双]"), (0xE180, "[デ]"), (0xE181, "[S]"),
    (0xE182, "[二]"), (0xE183, "[多]"), (0xE184, "[解]"), (0xE185, "[SS]"), (0xE186, "[B]"),
    (0xE187, "[N]"), (0xE188, "■"), (0xE189, "●"), (0xE18A, "[天]"), (0xE18B, "[交]"),
    (0xE18C, "[映]"), (0xE18D, "[無]"), (0xE18E, "[料]"), (0xE18F, "[年齢制限]"), (0xE190, "[前]"),
    (0xE191, "[後]"), (0xE192, "[再]"), (0xE193, "[新]"), (0xE194, "[初]"), (0xE195, "[終]"),
    (0xE196, "[生]"), (0xE197, "[販]"), (0xE198, "[声]"), (0xE199, "[吹]"), (0xE19A, "[PPV]"),
];

/// 外字を含まなければ None
pub fn to_text(s: &str) -> Option<String> {
    if !s.chars().any(|c| ('\u{E000}'..='\u{F8FF}').contains(&c)) {
        return None;
    }
    let mut o = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match GAIJI.iter().find(|g| g.0 == c as u32) {
            Some(g) => o.push_str(g.1),
            None => o.push(c),
        }
    }
    Some(o)
}

/// JSON の中のすべての文字列の外字を変換する
pub fn fix(v: &mut Value) {
    match v {
        Value::String(s) => {
            if let Some(n) = to_text(s) {
                *s = n;
            }
        }
        Value::Array(a) => a.iter_mut().for_each(fix),
        Value::Object(m) => m.values_mut().for_each(fix),
        _ => {}
    }
}

/// [字] などを nasne の外字コードに戻す（長いものから）
pub fn from_text(s: &str) -> String {
    let mut v: Vec<&(u32, &str)> = GAIJI.iter().collect();
    v.sort_by_key(|g| std::cmp::Reverse(g.1.len()));
    let mut s = s.to_string();
    for (code, text) in v {
        if s.contains(text) {
            s = s.replace(text, &char::from_u32(*code).unwrap().to_string());
        }
    }
    s
}

#[cfg(test)]
mod tests {
    #[test]
    fn roundtrip() {
        let raw = "ニュース\u{E0FE}\u{E192}";
        let shown = super::to_text(raw).unwrap();
        assert_eq!(shown, "ニュース[字][再]");
        assert_eq!(super::from_text(&shown), raw);
        assert_eq!(super::to_text("外字なし"), None);
    }
}
