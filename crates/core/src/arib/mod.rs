//! ARIB STD-B24 8 単位符号の文字列 → UTF-8

mod addsym;

mod tables {
    include!("jis_pairs.rs");
}

/// JIS X 0213 第1面・第2面（各 94×94）→ Unicode。0 = 未定義、bit31 = 2 文字（PAIRS の添字）
static JIS: &[u8] = include_bytes!("jis.bin");

#[derive(Clone, Copy, PartialEq, Eq)]
enum G {
    Kanji,
    Jis1,
    Jis2,
    AddSym,
    Alnum,
    Hira,
    Kata,
    PAlnum,
    PHira,
    PKata,
    JisKana,
    Other1,
    Other2,
}

impl G {
    fn two_byte(self) -> bool {
        matches!(self, G::Kanji | G::Jis1 | G::Jis2 | G::AddSym | G::Other2)
    }
}

fn final2(f: u8) -> G {
    match f {
        0x42 => G::Kanji,
        0x39 => G::Jis1,
        0x3A => G::Jis2,
        0x3B => G::AddSym,
        _ => G::Other2,
    }
}

fn final1(f: u8) -> G {
    match f {
        0x4A => G::Alnum,
        0x30 => G::Hira,
        0x31 => G::Kata,
        0x36 => G::PAlnum,
        0x37 => G::PHira,
        0x38 => G::PKata,
        0x49 => G::JisKana,
        _ => G::Other1,
    }
}

const HIRA_TAIL: [&str; 11] = ["", "", "", "ゝ", "ゞ", "ー", "。", "「", "」", "、", "・"];
const KATA_TAIL: [&str; 8] = ["ヽ", "ヾ", "ー", "。", "「", "」", "、", "・"];
const GETA: char = '〓';

fn push_jis(out: &mut String, plane: usize, c1: u8, c2: u8) {
    if !(0x21..=0x7E).contains(&c1) || !(0x21..=0x7E).contains(&c2) {
        out.push(GETA);
        return;
    }
    let idx = plane * 94 * 94 + (c1 as usize - 0x21) * 94 + (c2 as usize - 0x21);
    let v = u32::from_le_bytes(JIS[idx * 4..idx * 4 + 4].try_into().unwrap());
    if v == 0 {
        out.push(GETA);
    } else if v & 0x8000_0000 != 0 {
        out.push_str(tables::PAIRS[(v & 0x7FFF_FFFF) as usize]);
    } else {
        out.push(char::from_u32(v).unwrap_or(GETA));
    }
}

fn push_kanji(out: &mut String, c1: u8, c2: u8) {
    if let Some(s) = addsym::get(c1, c2) {
        out.push_str(s);
    } else if c1 >= 0x75 {
        out.push(GETA);
    } else {
        push_jis(out, 0, c1, c2);
    }
}

pub fn decode(b: &[u8]) -> String {
    let mut g = [G::Kanji, G::Alnum, G::Hira, G::Kata];
    let (mut gl, mut gr) = (0usize, 2usize);
    let mut ss: Option<usize> = None;
    let mut half = false;
    let mut out = String::with_capacity(b.len() * 2);
    let n = b.len();
    let at = |i: usize| -> u8 { if i < n { b[i] } else { 0 } };
    let mut i = 0;
    while i < n {
        let c = b[i];
        i += 1;
        if c == 0x1B {
            if i >= n {
                break;
            }
            let e = b[i];
            i += 1;
            match e {
                0x6E => gl = 2,
                0x6F => gl = 3,
                0x7E => gr = 1,
                0x7D => gr = 2,
                0x7C => gr = 3,
                0x28..=0x2B => {
                    let gi = (e - 0x28) as usize;
                    let f = at(i);
                    i += 1;
                    if f == 0x20 {
                        i += 1;
                        g[gi] = G::Other1;
                    } else {
                        g[gi] = final1(f);
                    }
                }
                0x24 => {
                    let f = at(i);
                    i += 1;
                    if (0x28..=0x2B).contains(&f) {
                        let gi = (f - 0x28) as usize;
                        let f2 = at(i);
                        i += 1;
                        if f2 == 0x20 {
                            i += 1;
                            g[gi] = G::Other2;
                        } else {
                            g[gi] = final2(f2);
                        }
                    } else {
                        g[0] = final2(f);
                    }
                }
                _ => {}
            }
            continue;
        }
        match c {
            0x0E => {
                gl = 1;
                continue;
            }
            0x0F => {
                gl = 0;
                continue;
            }
            0x19 => {
                ss = Some(2);
                continue;
            }
            0x1D => {
                ss = Some(3);
                continue;
            }
            0x0D | 0x0A => {
                out.push('\n');
                continue;
            }
            0x16 => {
                i += 1;
                continue;
            }
            0x1C => {
                i += 2;
                continue;
            }
            0x00..=0x1F => continue,
            0x20 => {
                out.push(if half { ' ' } else { '　' });
                continue;
            }
            0x7F | 0xA0 | 0xFF => continue,
            0x80..=0x9F => {
                match c {
                    0x88 | 0x89 => half = true,
                    0x8A => half = false,
                    0x8B | 0x91 | 0x93 | 0x94 | 0x97 | 0x98 => i += 1,
                    0x90 | 0x92 => i += if at(i) == 0x20 && i < n { 2 } else { 1 },
                    0x9D => i += 2,
                    0x95 => {
                        while i + 1 < n && !(b[i] == 0x95 && b[i + 1] == 0x4F) {
                            i += 1;
                        }
                        i += 2;
                    }
                    0x9B => {
                        while i < n && !(0x40..=0x7E).contains(&b[i]) {
                            i += 1;
                        }
                        i += 1;
                    }
                    _ => {}
                }
                continue;
            }
            _ => {}
        }
        let (gs, code) = if c < 0x80 {
            let gs = g[ss.unwrap_or(gl)];
            ss = None;
            (gs, c)
        } else {
            (g[gr], c & 0x7F)
        };
        if gs.two_byte() {
            let c2 = if i < n { b[i] & 0x7F } else { 0x21 };
            i += 1;
            match gs {
                G::Kanji | G::Jis1 | G::AddSym => push_kanji(&mut out, code, c2),
                G::Other2 => out.push(GETA),
                _ => push_jis(&mut out, 1, code, c2),
            }
            continue;
        }
        let idx = (code - 0x21) as usize;
        match gs {
            // 英数は常に半角で出す（読みやすさ優先）
            G::Alnum | G::PAlnum => out.push(match code {
                0x5C => '¥',
                0x7E => '~',
                _ => code as char,
            }),
            G::Hira | G::PHira => {
                if idx < 0x53 {
                    out.push(char::from_u32(0x3041 + idx as u32).unwrap());
                } else if let Some(s) = HIRA_TAIL.get(idx - 0x53) {
                    out.push_str(s);
                }
            }
            G::Kata | G::PKata => {
                if idx < 0x56 {
                    out.push(char::from_u32(0x30A1 + idx as u32).unwrap());
                } else if let Some(s) = KATA_TAIL.get(idx - 0x56) {
                    out.push_str(s);
                }
            }
            G::JisKana => {
                if idx <= 0x3E {
                    out.push(char::from_u32(0xFF61 + idx as u32).unwrap());
                }
            }
            _ => {}
        }
    }
    out
}

