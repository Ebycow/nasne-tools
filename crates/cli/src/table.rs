//! 端末向けの表（全角文字の幅を考えて桁を揃える）

use unicode_width::UnicodeWidthStr as _;

pub struct Table {
    head: Vec<&'static str>,
    rows: Vec<Vec<String>>,
}

impl Table {
    pub fn new(head: &[&'static str]) -> Self {
        Table { head: head.to_vec(), rows: Vec::new() }
    }

    pub fn row(&mut self, cells: Vec<String>) {
        self.rows.push(cells);
    }

    /// 最後の列は揃えない（長いタイトルを置く列）
    pub fn print(&self) {
        let n = self.head.len();
        let mut w = vec![0; n];
        for r in std::iter::once(&self.head.iter().map(|s| s.to_string()).collect::<Vec<_>>()).chain(&self.rows) {
            for (i, c) in r.iter().enumerate().take(n) {
                w[i] = w[i].max(c.width());
            }
        }
        let line = |cells: &[String]| {
            let mut s = String::new();
            for (i, c) in cells.iter().enumerate() {
                if i + 1 == cells.len() {
                    s.push_str(c);
                } else {
                    s.push_str(c);
                    s.push_str(&" ".repeat(w[i] - c.width() + 2));
                }
            }
            println!("{}", s.trim_end());
        };
        line(&self.head.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        for r in &self.rows {
            line(r);
        }
    }
}

/// 見出しと値の 2 列（詳細表示用）
pub fn kv(rows: &[(&str, String)]) {
    let w = rows.iter().map(|(k, _)| k.width()).max().unwrap_or(0);
    for (k, v) in rows {
        if v.is_empty() {
            continue;
        }
        let mut lines = v.lines();
        println!("{k}{}  {}", " ".repeat(w - k.width()), lines.next().unwrap_or(""));
        for l in lines {
            println!("{}  {l}", " ".repeat(w));
        }
    }
}
