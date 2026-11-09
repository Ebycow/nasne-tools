//! 番組表のコマンド（チャンネル一覧・放送中・番組表・検索・詳細・録画予約）

use crate::table::{Table, kv};
use crate::{BandArg, ChannelFilter, EpgCmd, Out, ResvOpts, bands, when};
use nasne_core::epg::{self, Channel};
use nasne_core::model::{self, ReserveReq};
use nasne_core::program::{self, Query, channel_matches, load};
use nasne_core::{Band, Event, Nasne, Res, jst};
use serde::Serialize;

// ---------------------------------------------------------------- 番組 ID
fn parse_program_id(s: &str) -> Res<(Band, u16, u16)> {
    program::parse_id(s).ok_or_else(|| format!("番組 ID が読めません: {s}（gr:30720:1234 の形。nasne epg の一覧の左端の列）"))
}

impl ChannelFilter {
    fn bands(&self) -> Vec<Band> {
        bands(self.band)
    }

    /// 名前の一部・チャンネル番号・サービス ID
    fn matches(&self, c: &Channel) -> bool {
        channel_matches(c, self.channel.as_deref().unwrap_or(""))
    }
}

/// 一覧・JSON に出す番組
#[derive(Serialize)]
struct Program<'a> {
    id: String,
    band: Band,
    channel: &'a str,
    #[serde(flatten)]
    event: &'a Event,
}

impl<'a> Program<'a> {
    fn new(band: Band, ch: &'a Channel, e: &'a Event) -> Self {
        Program { id: program::id(band, e), band, channel: &ch.name, event: e }
    }
}

fn dur(e: &Event) -> String {
    e.dur.map(|d| jst::fmt_dur(d as i64)).unwrap_or_else(|| "未定".into())
}

fn title(e: &Event) -> String {
    if e.pay { format!("【有料】{}", e.title) } else { e.title.clone() }
}

fn print_programs(out: &Out, list: &[Program], with_date: bool) {
    if out.json(list) {
        return;
    }
    if list.is_empty() {
        println!("番組はありません");
        return;
    }
    let mut t = Table::new(&["番組ID", "局", "放送", "長さ", "番組"]);
    for p in list {
        let e = p.event;
        let when = if with_date { when::span(e.start, e.end()) } else { format!("{}〜{}", jst::dt(e.start).time(), jst::dt(e.end()).time()) };
        t.row(vec![p.id.clone(), p.channel.to_string(), when, dur(e), title(e)]);
    }
    t.print();
}

// ---------------------------------------------------------------- コマンド
pub fn channels(n: &Nasne, out: &Out, band: Option<BandArg>) -> Res<()> {
    #[derive(Serialize)]
    struct Row<'a> {
        band: Band,
        #[serde(flatten)]
        ch: &'a Channel,
    }
    let epgs = load(n, &bands(band))?;
    let rows: Vec<Row> = epgs.iter().flat_map(|(b, e)| e.channels.iter().map(|ch| Row { band: *b, ch })).collect();
    if out.json(&rows) {
        return Ok(());
    }
    let mut t = Table::new(&["放送波", "番号", "サービスID", "局"]);
    for r in &rows {
        t.row(vec![r.band.name().into(), format!("{:03}", r.ch.number), r.ch.sid.to_string(), r.ch.name.clone()]);
    }
    t.print();
    Ok(())
}

pub fn run(n: &Nasne, out: &Out, cmd: EpgCmd) -> Res<()> {
    let now = jst::now();
    match cmd {
        EpgCmd::Now { band } => {
            let epgs = load(n, &bands(band))?;
            let list: Vec<Program> = epgs
                .iter()
                .flat_map(|(b, d)| d.channels.iter().filter_map(move |c| d.on_air(c.sid, now).map(|e| Program::new(*b, c, e))))
                .collect();
            print_programs(out, &list, false);
        }
        EpgCmd::List { filter, date, from, to } => {
            let day = when::date(&date, now)?;
            let from = match from {
                Some(f) => when::time_on(day, &f)?,
                None if day == jst::midnight(now) => now,
                None => day + when::DAY_START,
            };
            let to = match to {
                Some(t) => when::time_on(day, &t)?,
                None => day + when::DAY_START + 86400,
            };
            if to <= from {
                return Err("--to は --from より後にしてください".into());
            }
            let epgs = load(n, &filter.bands())?;
            let mut list = Vec::new();
            let mut found = false;
            for (b, d) in &epgs {
                for c in d.channels.iter().filter(|c| filter.matches(c)) {
                    found = true;
                    list.extend(d.schedule(c.sid).filter(|e| e.end() > from && e.start < to).map(|e| Program::new(*b, c, e)));
                }
            }
            if !found {
                return Err("その局は見つかりませんでした（nasne channels で一覧を見られます）".into());
            }
            print_programs(out, &list, true);
        }
        EpgCmd::Search { words, filter, genre } => {
            let q = Query::new(&words, genre.as_deref());
            let epgs = load(n, &filter.bands())?;
            let hit = |e: &Event| e.end() > now && q.matches(e);
            let mut list = Vec::new();
            for (b, d) in &epgs {
                for c in d.channels.iter().filter(|c| filter.matches(c)) {
                    list.extend(d.schedule(c.sid).filter(|e| hit(e)).map(|e| Program::new(*b, c, e)));
                }
            }
            list.sort_by_key(|p| p.event.start);
            print_programs(out, &list, true);
        }
        EpgCmd::Show { program } => show(n, out, &program)?,
    }
    Ok(())
}

/// 番組表から番組を探す
fn find(n: &Nasne, id: &str) -> Res<(Band, Channel, Event)> {
    let (band, sid, eid) = parse_program_id(id)?;
    let d = epg::load(n, band)?;
    let e = d.get(sid, eid).ok_or("番組表にその番組がありません（終わった番組は消えます）")?;
    Ok((band, program::channel(&d, sid), e.clone()))
}

fn show(n: &Nasne, out: &Out, id: &str) -> Res<()> {
    let (band, ch, e) = find(n, id)?;
    let resv = n.reservations()?.into_iter().find(|r| r.sid == e.sid && (r.eid == e.eid as i64 || r.start == e.start));
    if out.json {
        #[derive(Serialize)]
        struct Detail<'a> {
            #[serde(flatten)]
            program: Program<'a>,
            reservation: Option<nasne_core::model::Reservation>,
        }
        out.json(&Detail { program: Program::new(band, &ch, &e), reservation: resv });
        return Ok(());
    }
    let mut rows = vec![
        ("番組", title(&e)),
        ("局", format!("{} {}（{:03}）", band.name(), ch.name, ch.number)),
        ("放送", when::span(e.start, e.end())),
        ("長さ", dur(&e)),
        ("ジャンル", e.genre_name().to_string()),
        ("番組ID", program::id(band, &e)),
    ];
    if let Some(r) = &resv {
        rows.push((
            "予約",
            format!("予約済み（{}・{}・予約ID {}）", model::quality_label(r.quality), model::repeat_label(&r.cond), r.id),
        ));
    }
    kv(&rows);
    if !e.desc.is_empty() {
        println!("\n{}", e.desc);
    }
    for (k, v) in &e.ext {
        println!("\n【{k}】\n{v}");
    }
    Ok(())
}

pub fn reserve(n: &Nasne, program: &str, opts: &ResvOpts, force: bool) -> Res<()> {
    let (band, ch, e) = find(n, program)?;
    if e.end() <= jst::now() {
        return Err("この番組の放送は終わっています".into());
    }
    if let Some(r) = n.reservations()?.iter().find(|r| r.sid == e.sid && r.eid == e.eid as i64) {
        return Err(format!("すでに予約しています（予約ID {}）", r.id));
    }
    let req = ReserveReq::for_event(band, &e, opts.quality.code(), &opts.repeat.cond(e.start));
    if !force {
        let c = n.conflicts(&req)?;
        if !c.is_empty() {
            let names: Vec<String> = c.iter().map(|x| format!("  {} {}", when::span(x.start, x.start + x.dur), x.title)).collect();
            return Err(format!(
                "次の予約と時間が重なっています（nasne のチューナーは 1 つです）:\n{}\nそれでも予約するときは --force を付けてください（重なった部分はどちらかが録画されません）",
                names.join("\n")
            ));
        }
    }
    n.reserve(&req)?;
    println!(
        "録画予約しました: {}（{} {}・{}・{}）",
        e.title,
        when::span(e.start, e.end()),
        ch.name,
        model::quality_label(req.quality),
        model::repeat_label(&req.cond)
    );
    Ok(())
}
