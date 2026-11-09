//! 本体・予約・録画のコマンド

use crate::table::{Table, kv};
use crate::{Out, RecCmd, ResvCmd, SortArg, confirm, programs, when};
use nasne_core::model::{Recording, RecordingUpdate, Reservation, fmt_size, quality_label, repeat_label};
use nasne_core::{Nasne, Res, jst};

fn yes_no(b: bool) -> String {
    if b { "はい" } else { "いいえ" }.into()
}

fn on_off(b: bool) -> String {
    if b { "オン" } else { "オフ" }.into()
}

// ---------------------------------------------------------------- 本体
pub fn status(n: &Nasne, out: &Out) -> Res<()> {
    let s = n.status()?;
    if out.json(&s) {
        return Ok(());
    }
    kv(&[
        ("名前", s.name.clone()),
        ("接続先", n.host.clone()),
        ("HDD", format!("残り {} / {}（{:.0}% 使用）", fmt_size(s.free), fmt_size(s.total), pct(s.used, s.total))),
        ("録画中", yes_no(s.recording)),
    ]);
    Ok(())
}

fn pct(a: f64, b: f64) -> f64 {
    if b > 0.0 { a / b * 100.0 } else { 0.0 }
}

pub fn info(n: &Nasne, out: &Out) -> Res<()> {
    let d = n.info();
    if out.json(&d) {
        return Ok(());
    }
    let section = |t: &str| println!("\n■ {t}");
    println!("■ 本体");
    kv(&[
        ("名前", d.name.clone()),
        ("型番", d.model.clone()),
        ("ソフトウェア", d.software.clone()),
        ("nasne時刻", d.clock.clone()),
        ("時刻補正(TOT)", if d.tot { "受信済み" } else { "未受信" }.into()),
        ("録画中", yes_no(d.recording)),
        ("配信中のクライアント", format!("{} 台", d.streaming)),
    ]);
    section("ネットワーク");
    let net = &d.network;
    kv(&[
        ("IPアドレス", net.ip.clone()),
        ("取得方法", if net.dhcp { "自動(DHCP)" } else { "手動" }.into()),
        ("サブネット", net.subnet.clone()),
        ("ルーター", net.gateway.clone()),
        ("DNS", net.dns.join(", ")),
        ("MAC", net.mac.clone()),
    ]);
    section("ハードディスク");
    for h in &d.hdd {
        let kind = if h.internal { "内蔵HDD" } else { "外付けHDD" };
        if h.mounted {
            let usage = format!("{} / {}（残り {}）", fmt_size(h.used), fmt_size(h.total), fmt_size(h.free));
            kv(&[(kind, format!("{} {}  {usage}", h.product, h.format))]);
        } else {
            kv(&[(kind, "未接続・未使用".into())]);
        }
    }
    section("設定（変更は nasne HOME で）");
    let st = &d.settings;
    kv(&[
        ("省電力モード", on_off(st.power_saving)),
        ("BSアンテナ給電", on_off(st.bs_antenna_power)),
        ("イベントリレー", on_off(st.event_relay)),
        ("周波数変更の自動追従", on_off(st.frequency_follow)),
        (
            "視聴年齢制限",
            if st.parental_rating != 0 { format!("設定あり（値 {}）", st.parental_rating) } else { "制限なし".into() },
        ),
        ("DLNAクライアント登録", if st.dlna_manual_register { "手動" } else { "自動" }.into()),
        ("nasne HOME", d.home_url.clone()),
    ]);
    section("ファイルサーバー（NAS）");
    let nas = &d.nas;
    let mut rows = vec![("機能", on_off(nas.enabled))];
    if nas.enabled {
        rows.extend([
            ("サーバー名", nas.name.clone()),
            ("ワークグループ", nas.workgroup.clone()),
            ("共有フォルダ", nas.shared.join(", ")),
            ("接続先", nas.url(&net.ip)),
        ]);
    }
    kv(&rows);
    section(&format!("DLNAクライアント（{} 台）", d.clients.len()));
    let mut t = Table::new(&["最終アクセス", "登録", "MACアドレス", "名前"]);
    for c in &d.clients {
        let name = if c.name.is_empty() { "（名前なし）" } else { &c.name };
        t.row(vec![c.last_access.clone(), if c.registered { "登録済み" } else { "未登録" }.into(), c.mac.clone(), name.into()]);
    }
    t.print();
    for e in &d.errors {
        eprintln!("取得できなかった項目: {e}");
    }
    Ok(())
}

// ---------------------------------------------------------------- 予約
fn find_resv(n: &Nasne, id: &str) -> Res<Reservation> {
    n.reservations()?.into_iter().find(|r| r.id == id).ok_or_else(|| format!("予約 ID {id} は見つかりません（nasne resv list で一覧を見られます）"))
}

pub fn resv(n: &Nasne, out: &Out, cmd: ResvCmd) -> Res<()> {
    match cmd {
        ResvCmd::List => {
            let list = n.reservations()?;
            if out.json(&list) {
                return Ok(());
            }
            if list.is_empty() {
                println!("予約はありません");
                return Ok(());
            }
            let mut t = Table::new(&["予約ID", "放送", "長さ", "局", "画質", "繰り返し", "状態", "番組"]);
            for r in &list {
                let mut st = Vec::new();
                if r.recording {
                    st.push("録画中");
                }
                if r.conflict {
                    st.push("重複");
                }
                if r.eid == nasne_core::model::NO_EVENT {
                    st.push("時間指定");
                }
                t.row(vec![
                    r.id.clone(),
                    when::span(r.start, r.start + r.dur),
                    jst::fmt_dur(r.dur),
                    r.ch.clone(),
                    quality_label(r.quality),
                    repeat_label(&r.cond),
                    st.join("・"),
                    r.title.clone(),
                ]);
            }
            t.print();
            if list.iter().any(|r| r.conflict) {
                println!("\n重複している予約は nasne resv edit <予約ID> --raise-priority で、重なった時間にどちらを録画するかを選べます");
            }
        }
        ResvCmd::Add { program, opts, force } => programs::reserve(n, &program, &opts, force)?,
        ResvCmd::Edit { id, quality, repeat, raise_priority } => {
            if quality.is_none() && repeat.is_none() && !raise_priority {
                return Err("変える内容を --quality / --repeat / --raise-priority で指定してください".into());
            }
            let r = find_resv(n, &id)?;
            let q = quality.map(|q| q.code()).unwrap_or(r.quality);
            let c = repeat.map(|x| x.cond(r.start)).unwrap_or_else(|| r.cond.clone());
            n.update_reservation(&r.id, q, &c, raise_priority)?;
            let what = if raise_priority { "優先度を上げました" } else { "予約を変更しました" };
            println!("{what}: {}（{}・{}）", r.title, quality_label(q), repeat_label(&c));
        }
        ResvCmd::Cancel { id, yes } => {
            let r = find_resv(n, &id)?;
            confirm(&format!("「{}」（{}）の予約を取り消しますか？", r.title, when::span(r.start, r.start + r.dur)), yes)?;
            n.cancel_reservation(&r.id)?;
            println!("予約を取り消しました: {}", r.title);
        }
        ResvCmd::Failed => {
            let list = n.failed()?;
            if out.json(&list) {
                return Ok(());
            }
            if list.is_empty() {
                println!("録画できなかった予約はありません");
                return Ok(());
            }
            let mut t = Table::new(&["ID", "放送", "局", "理由", "番組"]);
            for f in &list {
                let at = if f.start != 0 { when::full(f.start) } else { String::new() };
                t.row(vec![f.id.clone(), at, f.ch.clone(), f.reason.clone(), f.title.clone()]);
            }
            t.print();
        }
        ResvCmd::ClearFailed { id } => {
            n.clear_failed(&id)?;
            println!("消去しました");
        }
    }
    Ok(())
}

// ---------------------------------------------------------------- 録画
fn find_rec(n: &Nasne, id: &str) -> Res<Recording> {
    n.recordings()?.into_iter().find(|r| r.id == id).ok_or_else(|| format!("録画 ID {id} は見つかりません（nasne rec list で一覧を見られます）"))
}

fn update(n: &Nasne, id: &str, u: RecordingUpdate, done: &str) -> Res<()> {
    let r = find_rec(n, id)?;
    n.update_recording(&r.id, &u)?;
    println!("{done}: {}", u.title.as_deref().unwrap_or(&r.title));
    Ok(())
}

pub fn rec(n: &Nasne, out: &Out, cmd: RecCmd) -> Res<()> {
    match cmd {
        RecCmd::List { search, unwatched, sort, reverse } => {
            let q = search.unwrap_or_default().trim().to_lowercase();
            let mut list: Vec<Recording> = n.recordings()?.into_iter().filter(|r| (!unwatched || r.new) && r.matches(&q)).collect();
            list.sort_by(|a, b| match sort {
                SortArg::Date => b.start.cmp(&a.start),
                SortArg::Size => b.size.total_cmp(&a.size),
                SortArg::Title => a.title.cmp(&b.title),
                SortArg::Channel => a.ch.cmp(&b.ch).then(b.start.cmp(&a.start)),
                SortArg::Duration => a.dur.cmp(&b.dur),
            });
            if reverse {
                list.reverse();
            }
            if out.json(&list) {
                return Ok(());
            }
            if list.is_empty() {
                println!("録画はありません");
                return Ok(());
            }
            let mut t = Table::new(&["録画ID", "放送", "長さ", "局", "画質", "サイズ", "状態", "番組"]);
            for r in &list {
                let mut st = Vec::new();
                if r.new {
                    st.push("未視聴");
                }
                if r.recording {
                    st.push("録画中");
                }
                if r.protect {
                    st.push("保護");
                }
                t.row(vec![
                    r.id.clone(),
                    when::full(r.start),
                    jst::fmt_dur(r.dur),
                    r.ch.clone(),
                    quality_label(r.quality),
                    fmt_size(r.size),
                    st.join("・"),
                    r.title.clone(),
                ]);
            }
            t.print();
            println!("\n{} 件・{}", list.len(), fmt_size(list.iter().map(|r| r.size).sum()));
        }
        RecCmd::Rename { id, title } => {
            let title = title.trim().to_string();
            if title.is_empty() {
                return Err("タイトルが空です".into());
            }
            update(n, &id, RecordingUpdate { title: Some(title), ..Default::default() }, "タイトルを変更しました")?
        }
        RecCmd::Protect { id } => update(n, &id, RecordingUpdate { protect: Some(true), ..Default::default() }, "保護しました")?,
        RecCmd::Unprotect { id } => {
            update(n, &id, RecordingUpdate { protect: Some(false), ..Default::default() }, "保護を解除しました")?
        }
        RecCmd::Watched { id } => {
            update(n, &id, RecordingUpdate { unwatched: Some(false), ..Default::default() }, "視聴済みにしました")?
        }
        RecCmd::Unwatched { id } => {
            update(n, &id, RecordingUpdate { unwatched: Some(true), ..Default::default() }, "未視聴にしました")?
        }
        RecCmd::Delete { id, yes } => {
            let r = find_rec(n, &id)?;
            if !r.deletable() {
                return Err(format!("「{}」は{}なので削除できません", r.title, if r.recording { "録画中" } else { "保護中" }));
            }
            confirm(&format!("「{}」（{}・{}）を削除しますか？元に戻せません。", r.title, when::full(r.start), fmt_size(r.size)), yes)?;
            n.delete_recording(&r.id)?;
            println!("削除しました: {}", r.title);
        }
    }
    Ok(())
}
