//! 画面（番組表 / 予約一覧 / 録画一覧 / nasne 情報）
//!
//! nasne への操作はすべて nasne-core を通し、別スレッドで行って結果をチャネルで受け取る。
//! UI スレッドは描画だけをする。番組表は表示範囲に入っている番組だけを描く（ヘッダーと時刻列は固定表示）。

use crate::theme::{DARK, LIGHT, Pal, bold, genre_color};
use nasne_core::epg::{self, Band, Epg, GENRES};
use nasne_core::info::Info;
use nasne_core::jst::{self, fmt_dur};
use nasne_core::model::{Failed, Recording, RecordingUpdate, ReserveReq, Reservation, Status, fmt_size, quality_label, repeat_label, repeat_options};
use nasne_core::{Nasne, Res, client, model, store};
use eframe::egui::{
    self, Align, Align2, Color32, ComboBox, CornerRadius, CursorIcon, FontFamily, FontId, Frame, Id, Label, Layout,
    Margin, Modal, Rect, RichText, ScrollArea, Sense, Stroke, TextEdit, TextureHandle, TextureOptions,
    pos2, text::LayoutJob, text::TextFormat, vec2,
};
use egui_extras::{Column, TableBuilder};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

const PX: f32 = 3.0; // 1 分あたりの高さ
const HEAD_H: f32 = 44.0;
const TIME_W: f32 = 36.0;
const COL_MIN: f32 = 150.0;
const DAYS: usize = 8;
const EPG_CHECK: Duration = Duration::from_secs(120);
const STATUS_EVERY: Duration = Duration::from_secs(60);
// ---------------------------------------------------------------- データ
#[derive(Clone, Copy, Default)]
struct Refresh {
    reserved: bool,
    recorded: bool,
    status: bool,
    recng: bool,
}

enum Msg {
    Epg { sess: u64, bt: usize, data: Arc<Epg>, ver: Vec<u8> },
    EpgSame { sess: u64, bt: usize },
    EpgErr { sess: u64, bt: usize, err: String },
    Logos { sess: u64, bt: usize, logos: Vec<epg::Logo> },
    Status(u64, Res<Status>),
    Reserved(u64, Res<Vec<Reservation>>),
    RecNg(u64, Res<Vec<Failed>>),
    Recorded(u64, Res<Vec<Recording>>),
    Info(u64, Box<Info>),
    Done(u64, Res<String>, Refresh),
    Conflict(u64, ReserveReq, Vec<String>),
    Discovered(Option<String>),
}

#[derive(Clone)]
struct Tx(mpsc::Sender<Msg>, egui::Context);

impl Tx {
    fn send(&self, m: Msg) {
        let _ = self.0.send(m);
        self.1.request_repaint();
    }
}

#[derive(Default)]
struct EpgState {
    data: Option<Arc<Epg>>,
    ver: Option<Vec<u8>>,
    loading: bool,
    err: Option<String>,
    checked: Option<Instant>,
    logos: HashMap<u16, TextureHandle>,
}

/// その日（04:00〜翌04:00）に表示する局と番組
struct DayCols {
    key: (usize, usize, i64),
    ws: i64,
    we: i64,
    cols: Vec<(usize, Vec<u32>)>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum View {
    Epg,
    Reserved,
    Recorded,
    Info,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SortKey {
    Date,
    Title,
    Ch,
    Dur,
    Size,
}

enum Act {
    DeleteReserve(String),
    DeleteRecorded(String),
    /// 重なりを承知で予約する
    Reserve(ReserveReq),
}

enum Dialog {
    Program { bt: usize, sid: u16, eid: u16, quality: i64, cond: String, busy: bool },
    EditRes { id: String, quality: i64, cond: String },
    Confirm { title: String, body: String, ok: String, act: Act },
    Rename { id: String, text: String },
}

pub struct App {
    tx: Tx,
    rx: mpsc::Receiver<Msg>,
    sess: u64,
    net: Option<Arc<Nasne>>,
    host_edit: String,
    discovering: bool,
    view: View,
    pal: Pal,
    status: Option<Status>,
    status_err: Option<String>,
    status_at: Option<Instant>,
    epg: [EpgState; 3],
    bt: usize,
    day: usize,
    day_cols: Option<Arc<DayCols>>,
    scroll_to: Option<f32>,
    reserved: Vec<Reservation>,
    reserved_err: Option<String>,
    res_by_eid: HashMap<(u16, u16), usize>,
    res_by_start: HashMap<(u16, i64), usize>,
    recng: Option<Res<Vec<Failed>>>,
    recorded: Option<Vec<Recording>>,
    recorded_err: Option<String>,
    rec_search: String,
    rec_unwatched: bool,
    rec_sort: (SortKey, i8),
    info: Option<Info>,
    dialog: Option<Dialog>,
    toast: Option<(String, Instant)>,
    busy: Arc<AtomicU32>,
    started: Instant,
    bench: bool,
    first_epg_shown: bool,
    first_frame_logged: bool,
    dev_open_program: bool,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, started: Instant) -> Self {
        let (s, rx) = mpsc::channel();
        let tx = Tx(s, cc.egui_ctx.clone());
        if std::env::var_os("NASNE_GUI_BENCH").is_some() {
            eprintln!("App::new: {:?}", started.elapsed());
        }
        let mut app = App {
            tx,
            rx,
            sess: 0,
            net: None,
            host_edit: String::new(),
            discovering: false,
            view: View::Epg,
            pal: LIGHT,
            status: None,
            status_err: None,
            status_at: None,
            epg: Default::default(),
            bt: 0,
            day: 0,
            day_cols: None,
            scroll_to: None,
            reserved: Vec::new(),
            reserved_err: None,
            res_by_eid: HashMap::new(),
            res_by_start: HashMap::new(),
            recng: None,
            recorded: None,
            recorded_err: None,
            rec_search: String::new(),
            rec_unwatched: false,
            rec_sort: (SortKey::Date, -1),
            info: None,
            dialog: None,
            toast: None,
            busy: Arc::new(AtomicU32::new(0)),
            started,
            bench: std::env::var_os("NASNE_GUI_BENCH").is_some(),
            first_epg_shown: false,
            first_frame_logged: false,
            dev_open_program: false,
        };
        // 開発用: NASNE_GUI_VIEW=reserved|recorded|info で起動時のタブを選ぶ
        match std::env::var("NASNE_GUI_VIEW").as_deref() {
            Ok("reserved") => app.view = View::Reserved,
            Ok("recorded") => app.view = View::Recorded,
            Ok("info") => app.view = View::Info,
            Ok("program") => app.dev_open_program = true,
            _ => {}
        }
        match store::load_host() {
            Some(h) => app.connect(h),
            None => app.discover(),
        }
        match app.view {
            View::Reserved => app.load_recng(),
            View::Recorded => app.load_recorded(),
            View::Info => app.load_info(),
            View::Epg => {}
        }
        app
    }

    // ------------------------------------------------------------ 非同期処理
    fn spawn(&mut self, f: impl FnOnce(Arc<Nasne>, u64) -> Msg + Send + 'static) {
        let Some(net) = self.net.clone() else { return };
        let (tx, sess, busy) = (self.tx.clone(), self.sess, self.busy.clone());
        busy.fetch_add(1, Ordering::Relaxed);
        std::thread::spawn(move || {
            let m = f(net, sess);
            busy.fetch_sub(1, Ordering::Relaxed);
            tx.send(m);
        });
    }

    fn discover(&mut self) {
        self.discovering = true;
        let tx = self.tx.clone();
        std::thread::spawn(move || tx.send(Msg::Discovered(client::discover())));
    }

    fn connect(&mut self, host: String) {
        let host = host.trim().to_string();
        if host.is_empty() {
            return;
        }
        store::save_host(&host);
        self.host_edit = host.clone();
        self.sess += 1;
        self.net = Some(Arc::new(Nasne::new(&host)));
        self.epg = Default::default();
        self.day_cols = None;
        self.status = None;
        self.status_err = None;
        self.reserved.clear();
        self.recorded = None;
        self.info = None;
        self.recng = None;
        self.scroll_to = Some(f32::NAN); // 番組表が出たら現在時刻へ
        for bt in 0..3 {
            self.load_epg(bt, true);
            self.load_logos(bt);
        }
        self.load_status();
        self.load_reserved();
    }

    fn load_epg(&mut self, bt: usize, from_cache: bool) {
        let st = &mut self.epg[bt];
        if st.loading {
            return;
        }
        st.loading = true;
        st.checked = Some(Instant::now());
        let prev = st.ver.clone();
        let tx = self.tx.clone();
        self.spawn(move |n, sess| {
            let band = Band::ALL[bt];
            let mut prev = prev;
            if from_cache && let Some(c) = epg::load_cached(&n.host, band) {
                tx.send(Msg::Epg { sess, bt, data: Arc::new(c.epg), ver: c.version.clone() });
                prev = Some(c.version);
            }
            match epg::refresh(&n, band, prev.as_deref()) {
                Ok(None) => Msg::EpgSame { sess, bt },
                Ok(Some(l)) => Msg::Epg { sess, bt, data: Arc::new(l.epg), ver: l.version },
                Err(err) => Msg::EpgErr { sess, bt, err },
            }
        });
    }

    fn load_logos(&mut self, bt: usize) {
        let tx = self.tx.clone();
        self.spawn(move |n, sess| {
            let band = Band::ALL[bt];
            let cached = store::load_logos(&n.host, band);
            if let Some(c) = &cached {
                tx.send(Msg::Logos { sess, bt, logos: epg::parse_logos(c) });
            }
            match epg::fetch_logos(&n, band) {
                Ok(d) if cached.as_ref() != Some(&d) => {
                    store::save_logos(&n.host, band, &d);
                    Msg::Logos { sess, bt, logos: epg::parse_logos(&d) }
                }
                _ => Msg::Logos { sess, bt, logos: Vec::new() },
            }
        });
    }

    fn load_status(&mut self) {
        self.status_at = Some(Instant::now());
        self.spawn(|n, sess| Msg::Status(sess, n.status()));
    }

    fn load_reserved(&mut self) {
        self.spawn(|n, sess| Msg::Reserved(sess, n.reservations()));
    }

    fn load_recng(&mut self) {
        self.spawn(|n, sess| Msg::RecNg(sess, n.failed()));
    }

    fn load_recorded(&mut self) {
        self.spawn(|n, sess| Msg::Recorded(sess, n.recordings()));
    }

    fn load_info(&mut self) {
        self.spawn(|n, sess| Msg::Info(sess, Box::new(n.info())));
    }

    /// 変更系の操作。成功したら toast に msg を出して、必要な一覧を取り直す
    fn act(&mut self, refresh: Refresh, msg: &str, f: impl FnOnce(&Nasne) -> Res<()> + Send + 'static) {
        let msg = msg.to_string();
        self.spawn(move |n, sess| Msg::Done(sess, f(&n).map(|_| msg), refresh));
    }

    /// 録画予約。force でなければ、先にほかの予約との重なりを確かめる
    fn reserve(&mut self, req: ReserveReq, force: bool) {
        self.spawn(move |n, sess| {
            if !force {
                match n.conflicts(&req) {
                    Ok(c) if !c.is_empty() => {
                        let names = c.iter().map(|x| format!("・{} {}", jst::dt(x.start).time(), x.title)).collect();
                        return Msg::Conflict(sess, req, names);
                    }
                    Ok(_) => {}
                    Err(e) => return Msg::Done(sess, Err(e), Refresh::default()),
                }
            }
            let r = n.reserve(&req).map(|_| "録画予約しました".to_string());
            Msg::Done(sess, r, Refresh { reserved: true, ..Default::default() })
        });
    }

    fn update_recording(&mut self, id: String, u: RecordingUpdate, msg: &str) {
        // 先に画面へ反映（失敗したら一覧を取り直す）
        if let Some(r) = self.recorded.as_mut().and_then(|v| v.iter_mut().find(|r| r.id == id)) {
            if let Some(x) = u.unwatched {
                r.new = x;
            }
            if let Some(x) = u.protect {
                r.protect = x;
            }
            if let Some(t) = &u.title {
                r.title = t.clone();
            }
        }
        let msg = msg.to_string();
        self.spawn(move |n, sess| {
            let r = n.update_recording(&id, &u).map(|_| msg);
            let refresh = Refresh { recorded: r.is_err(), ..Default::default() };
            Msg::Done(sess, r, refresh)
        });
    }

    fn toast(&mut self, s: impl Into<String>) {
        self.toast = Some((s.into(), Instant::now()));
    }

    fn set_reserved(&mut self, items: Vec<Reservation>) {
        self.reserved = items;
        self.res_by_eid.clear();
        self.res_by_start.clear();
        for (i, r) in self.reserved.iter().enumerate() {
            self.res_by_eid.insert((r.sid, r.eid as u16), i);
            self.res_by_start.insert((r.sid, r.start), i);
        }
    }

    fn find_res(&self, sid: u16, eid: u16, start: i64) -> Option<&Reservation> {
        self.res_by_eid.get(&(sid, eid)).or_else(|| self.res_by_start.get(&(sid, start))).map(|&i| &self.reserved[i])
    }

    fn handle(&mut self, m: Msg) {
        let sess_of = |m: &Msg| match m {
            Msg::Epg { sess, .. } | Msg::EpgSame { sess, .. } | Msg::EpgErr { sess, .. } | Msg::Logos { sess, .. } => *sess,
            Msg::Status(g, _) | Msg::Reserved(g, _) | Msg::RecNg(g, _) | Msg::Recorded(g, _) | Msg::Info(g, _) => *g,
            Msg::Done(g, ..) | Msg::Conflict(g, ..) => *g,
            Msg::Discovered(_) => u64::MAX,
        };
        let g = sess_of(&m);
        if g != u64::MAX && g != self.sess {
            return; // 接続先を変える前の結果
        }
        match m {
            Msg::Epg { bt, data, ver, .. } => {
                let st = &mut self.epg[bt];
                st.data = Some(data);
                st.ver = Some(ver);
                st.err = None;
                if bt == self.bt {
                    self.day_cols = None;
                }
                // キャッシュから出した直後でも、最新版の取得が続いているので loading はそのまま
            }
            Msg::EpgSame { bt, .. } => {
                self.epg[bt].loading = false;
                self.epg[bt].err = None;
            }
            Msg::EpgErr { bt, err, .. } => {
                self.epg[bt].loading = false;
                self.epg[bt].err = Some(err);
            }
            Msg::Logos { bt, logos, .. } => {
                for l in logos {
                    let img = egui::ColorImage::from_rgba_unmultiplied([l.w as usize, l.h as usize], &l.rgba);
                    let t = self.tx.1.load_texture(format!("logo{bt}-{}", l.sid), img, TextureOptions::LINEAR);
                    self.epg[bt].logos.insert(l.sid, t);
                }
            }
            Msg::Status(_, r) => match r {
                Ok(s) => {
                    self.status = Some(s);
                    self.status_err = None;
                }
                Err(e) => self.status_err = Some(e),
            },
            Msg::Reserved(_, r) => match r {
                Ok(v) => {
                    self.set_reserved(v);
                    self.reserved_err = None;
                }
                Err(e) => self.reserved_err = Some(e),
            },
            Msg::RecNg(_, r) => self.recng = Some(r),
            Msg::Recorded(_, r) => match r {
                Ok(v) => {
                    self.recorded = Some(v);
                    self.recorded_err = None;
                }
                Err(e) => self.recorded_err = Some(e),
            },
            Msg::Info(_, i) => self.info = Some(*i),
            Msg::Done(_, r, rf) => {
                match r {
                    Ok(s) => {
                        self.toast(s);
                        if matches!(self.dialog, Some(Dialog::Program { .. } | Dialog::EditRes { .. })) {
                            self.dialog = None;
                        }
                    }
                    Err(e) => {
                        self.toast(format!("失敗しました: {e}"));
                        if let Some(Dialog::Program { busy, .. }) = &mut self.dialog {
                            *busy = false;
                        }
                    }
                }
                if rf.reserved {
                    self.load_reserved();
                }
                if rf.recorded {
                    self.load_recorded();
                }
                if rf.status {
                    self.load_status();
                }
                if rf.recng {
                    self.load_recng();
                }
            }
            Msg::Conflict(_, req, names) => {
                self.dialog = Some(Dialog::Confirm {
                    title: "予約が重なっています".into(),
                    body: format!(
                        "次の予約と時間が重なっています（nasne のチューナーは 1 つです）:\n{}\n\nそれでも予約しますか？（重なった部分はどちらかが録画されません）",
                        names.join("\n")
                    ),
                    ok: "それでも予約する".into(),
                    act: Act::Reserve(req),
                });
            }
            Msg::Discovered(h) => {
                self.discovering = false;
                match h {
                    Some(h) => {
                        self.toast(format!("nasne を見つけました（{h}）"));
                        self.connect(h);
                    }
                    None => self.toast("nasne が見つかりませんでした。IP アドレスを入力してください"),
                }
            }
        }
    }

    fn tick(&mut self) {
        if self.net.is_none() {
            return;
        }
        if self.status_at.is_none_or(|t| t.elapsed() > STATUS_EVERY) {
            self.load_status();
        }
        let st = &self.epg[self.bt];
        if !st.loading && st.checked.is_none_or(|t| t.elapsed() > EPG_CHECK) {
            self.load_epg(self.bt, false);
        }
    }
}

// ---------------------------------------------------------------- 表示用の小物
fn quality_combo(ui: &mut egui::Ui, id: &str, q: &mut i64) {
    ComboBox::from_id_salt(id).selected_text(quality_label(*q)).width(64.0).show_ui(ui, |ui| {
        ui.selectable_value(q, model::QUALITY_DR, "DR");
        ui.selectable_value(q, model::QUALITY_3X, "3倍");
    });
}

fn cond_combo(ui: &mut egui::Ui, id: &str, start: i64, c: &mut String) {
    let mut opts = repeat_options(start);
    if !opts.contains(c) {
        opts.push(c.clone());
    }
    ComboBox::from_id_salt(id).selected_text(repeat_label(c)).width(90.0).show_ui(ui, |ui| {
        for o in opts {
            let l = repeat_label(&o);
            ui.selectable_value(c, o, l);
        }
    });
}

fn badge(ui: &mut egui::Ui, text: &str, fg: Color32, bg: Color32) {
    ui.add(Label::new(RichText::new(format!(" {text} ")).size(11.0).color(fg).background_color(bg).family(bold())));
}

fn danger_button(ui: &mut egui::Ui, p: &Pal, text: &str, enabled: bool) -> egui::Response {
    ui.add_enabled(enabled, egui::Button::new(RichText::new(text).color(p.rec)))
}

fn primary_button(ui: &mut egui::Ui, p: &Pal, text: &str, enabled: bool) -> egui::Response {
    ui.add_enabled(enabled, egui::Button::new(RichText::new(text).color(Color32::WHITE)).fill(p.accent))
}

fn open_url(url: &str) {
    #[cfg(target_os = "macos")]
    let r = std::process::Command::new("open").arg(url).spawn();
    #[cfg(windows)]
    let r = std::process::Command::new("cmd").args(["/C", "start", "", url]).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let r = std::process::Command::new("xdg-open").arg(url).spawn();
    let _ = r;
}

// ---------------------------------------------------------------- 画面
impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        while let Ok(m) = self.rx.try_recv() {
            self.handle(m);
        }
        self.tick();
        self.pal = if ui.visuals().dark_mode { DARK } else { LIGHT };
        let p = self.pal;

        egui::Panel::top("top")
            .frame(Frame::new().fill(p.panel).inner_margin(Margin::symmetric(16, 8)))
            .show(ui, |ui| self.top_bar(ui));

        egui::CentralPanel::default().frame(Frame::new().fill(p.bg)).show(ui, |ui| match self.view {
            View::Epg => self.epg_view(ui),
            View::Reserved => self.reserved_view(ui),
            View::Recorded => self.recorded_view(ui),
            View::Info => self.info_view(ui),
        });

        if self.dev_open_program
            && let Some(d) = &self.epg[self.bt].data
        {
            let now = jst::now();
            if let Some(e) = d.channels.first().and_then(|c| d.by_service.get(&c.sid)).and_then(|v| {
                v.iter().map(|&i| &d.events[i as usize]).find(|e| e.start <= now && e.end() > now)
            }) {
                self.dialog = Some(Dialog::Program { bt: self.bt, sid: e.sid, eid: e.eid, quality: model::QUALITY_3X, cond: model::ONCE.into(), busy: false });
            }
            self.dev_open_program = false;
        }
        self.dialogs(&ctx);
        self.paint_toast(&ctx);

        // 時刻表示・定期更新のための再描画（操作がなければ CPU は使わない）
        ctx.request_repaint_after(Duration::from_secs(30));

        if self.bench && !self.first_frame_logged {
            self.first_frame_logged = true;
            eprintln!("最初のフレーム: {:?}", self.started.elapsed());
        }
        if self.bench {
            let shown = self.epg[self.bt].data.is_some();
            if shown && !self.first_epg_shown {
                self.first_epg_shown = true;
                eprintln!("番組表を表示: {:?}", self.started.elapsed());
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
    }
}

impl App {
    fn top_bar(&mut self, ui: &mut egui::Ui) {
        let p = self.pal;
        ui.horizontal(|ui| {
            let name = self.status.as_ref().map(|s| s.name.as_str()).unwrap_or("nasne");
            ui.label(RichText::new(name).size(16.0).family(bold()).color(p.text));
            ui.add_space(12.0);
            for (v, label) in
                [(View::Epg, "番組表"), (View::Reserved, "予約一覧"), (View::Recorded, "録画一覧"), (View::Info, "nasne情報")]
            {
                let on = self.view == v;
                let t = RichText::new(label).color(if on { p.accent } else { p.muted });
                let t = if on { t.family(bold()) } else { t };
                if ui.add(egui::Button::new(t).fill(if on { p.accent_soft } else { Color32::TRANSPARENT }).frame(on))
                    .clicked()
                    && !on
                {
                    self.view = v;
                    match v {
                        View::Reserved => {
                            self.load_reserved();
                            self.load_recng();
                        }
                        View::Recorded if self.recorded.is_none() => self.load_recorded(),
                        View::Info => self.load_info(),
                        _ => {}
                    }
                }
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if self.net.is_none() || self.status_err.is_some() {
                    let t = if self.discovering { "nasne を探しています…" } else { "nasne に接続できません" };
                    ui.label(RichText::new(t).color(p.rec).size(13.0));
                } else if let Some(s) = &self.status {
                    {
                        ui.label(
                            RichText::new(format!("HDD 残り {} / {}", fmt_size(s.free), fmt_size(s.total)))
                                .size(13.0)
                                .color(p.muted),
                        );
                        let (r, _) = ui.allocate_exact_size(vec2(120.0, 6.0), Sense::hover());
                        ui.painter().rect_filled(r, 3.0, p.line);
                        let f = if s.total > 0.0 { (s.used / s.total) as f32 } else { 0.0 };
                        ui.painter().rect_filled(Rect::from_min_size(r.min, vec2(120.0 * f, 6.0)), 3.0, p.accent);
                        if s.recording {
                            ui.label(RichText::new("録画中").size(13.0).color(p.rec));
                            let t = ui.input(|i| i.time);
                            let a = if (t % 1.4) < 0.7 { 255 } else { 80 };
                            let (r, _) = ui.allocate_exact_size(vec2(8.0, 8.0), Sense::hover());
                            ui.painter().circle_filled(r.center(), 4.0, p.rec.gamma_multiply(a as f32 / 255.0));
                            ui.ctx().request_repaint_after(Duration::from_millis(700));
                        }
                    }
                } else {
                    ui.label(RichText::new("HDD –").size(13.0).color(p.muted));
                }
                if self.busy.load(Ordering::Relaxed) > 0 || self.discovering {
                    ui.add(egui::Spinner::new().size(14.0).color(p.muted));
                }
            });
        });
    }

    fn host_editor(&mut self, ui: &mut egui::Ui) {
        ui.label("接続先");
        let r = ui.add(TextEdit::singleline(&mut self.host_edit).hint_text("192.168.x.x").desired_width(150.0));
        let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        if ui.button("接続").clicked() || enter {
            let h = self.host_edit.clone();
            self.connect(h);
        }
        if ui.add_enabled(!self.discovering, egui::Button::new("自動検出")).clicked() {
            self.discover();
        }
    }

    // ------------------------------------------------------------ 番組表
    /// その日の 04:00（暦日基準。深夜 0〜4 時でも「今日」は今日の日付）
    fn day_base(day: usize) -> i64 {
        jst::midnight(jst::now()) + 4 * 3600 + day as i64 * 86400
    }

    /// 表示範囲 [ws, we)。通常は 04:00〜翌04:00 だが、深夜 0〜4 時の「今日」は
    /// 現在時刻の正時から始めて、放送中の番組と今日 1 日分の番組を続けて見せる。
    fn window(day: usize) -> (i64, i64) {
        let base = Self::day_base(day);
        let now = jst::now();
        let ws = if day == 0 { base.min(now - now.rem_euclid(3600)) } else { base };
        (ws, base + 86400)
    }

    fn day_cols(&mut self) -> Option<Arc<DayCols>> {
        let data = self.epg[self.bt].data.clone()?;
        let (ws, we) = Self::window(self.day);
        let key = (self.bt, Arc::as_ptr(&data) as usize, ws);
        if let Some(dc) = &self.day_cols
            && dc.key == key
        {
            return Some(dc.clone());
        }
        let cols = data
            .channels
            .iter()
            .enumerate()
            .filter_map(|(ci, ch)| {
                let evs: Vec<u32> = data
                    .by_service
                    .get(&ch.sid)?
                    .iter()
                    .copied()
                    .filter(|&i| {
                        let e = &data.events[i as usize];
                        e.end() > ws && e.start < we
                    })
                    .collect();
                (!evs.is_empty()).then_some((ci, evs))
            })
            .collect();
        let dc = Arc::new(DayCols { key, ws, we, cols });
        self.day_cols = Some(dc.clone());
        Some(dc)
    }

    fn epg_view(&mut self, ui: &mut egui::Ui) {
        let p = self.pal;
        Frame::new().inner_margin(Margin::symmetric(16, 8)).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                for (i, band) in Band::ALL.iter().enumerate() {
                    let on = self.bt == i;
                    let t = RichText::new(band.name()).color(if on { Color32::WHITE } else { p.text });
                    let cr = match i {
                        0 => CornerRadius { nw: 6, sw: 6, ne: 0, se: 0 },
                        2 => CornerRadius { nw: 0, sw: 0, ne: 6, se: 6 },
                        _ => CornerRadius::ZERO,
                    };
                    if ui
                        .add(egui::Button::new(t).fill(if on { p.accent } else { p.panel }).corner_radius(cr))
                        .clicked()
                        && !on
                    {
                        self.bt = i;
                        self.day_cols = None;
                        let st = &self.epg[i];
                        if !st.loading && st.checked.is_none_or(|t| t.elapsed() > EPG_CHECK) {
                            self.load_epg(i, st.data.is_none());
                        }
                    }
                }
                ui.spacing_mut().item_spacing.x = 8.0;
                ui.add_space(8.0);
                let base = Self::day_base(0);
                let label = |d: usize| {
                    let t = jst::dt(base + d as i64 * 86400);
                    format!("{}{}", ["今日 ", "明日 "].get(d).copied().unwrap_or(""), t.date())
                };
                ComboBox::from_id_salt("day").selected_text(label(self.day)).width(110.0).show_ui(ui, |ui| {
                    for d in 0..DAYS {
                        ui.selectable_value(&mut self.day, d, label(d));
                    }
                });
                if ui.button("現在時刻へ").clicked() {
                    self.day = 0;
                    self.scroll_to = Some(f32::NAN);
                }
                let st = &self.epg[self.bt];
                if st.loading && st.data.is_some() {
                    ui.label(RichText::new("更新を確認中…").size(12.0).color(p.muted));
                }
                if let (Some(e), Some(_)) = (&st.err, &st.data) {
                    ui.label(RichText::new(format!("更新できません: {e}")).size(12.0).color(p.rec));
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    for g in [8usize, 7, 6, 5, 3, 1, 0] {
                        ui.label(RichText::new(GENRES[g]).size(11.0).color(p.muted));
                        let (r, _) = ui.allocate_exact_size(vec2(8.0, 8.0), Sense::hover());
                        ui.painter().rect_filled(r, 2.0, genre_color(Some(g as u8)).unwrap_or(p.muted));
                        ui.add_space(4.0);
                    }
                });
            });
        });
        let st = &self.epg[self.bt];
        if st.data.is_none() {
            let (err, loading) = (st.err.clone(), st.loading);
            ui.add_space(24.0);
            ui.horizontal(|ui| {
                ui.add_space(16.0);
                if self.net.is_none() {
                    if self.discovering {
                        ui.label(RichText::new("LAN 内の nasne を探しています…").color(p.muted));
                    } else {
                        self.host_editor(ui);
                    }
                } else if let Some(e) = err {
                    ui.label(RichText::new(format!("番組表を取得できませんでした: {e}")).color(p.rec));
                    if ui.button("再試行").clicked() {
                        self.load_epg(self.bt, false);
                    }
                    ui.add_space(16.0);
                    self.host_editor(ui);
                } else if loading {
                    ui.label(RichText::new("読み込み中…").color(p.muted));
                }
            });
            return;
        }
        let Some(dc) = self.day_cols() else { return };
        if dc.cols.is_empty() {
            ui.add_space(24.0);
            ui.horizontal(|ui| {
                ui.add_space(16.0);
                ui.label(RichText::new("この放送波の番組情報がありません。").color(p.muted));
            });
            return;
        }
        self.epg_grid(ui, &dc);
    }

    fn epg_grid(&mut self, ui: &mut egui::Ui, dc: &DayCols) {
        let p = self.pal;
        let data = self.epg[self.bt].data.clone().unwrap();
        let n = dc.cols.len();
        let total_min = ((dc.we - dc.ws) / 60) as f32;
        let col_w = ((ui.available_width() - TIME_W) / n as f32).max(COL_MIN).floor();
        let size = vec2(TIME_W + col_w * n as f32, HEAD_H + total_min * PX);
        let now = jst::now();

        let mut sa = ScrollArea::both()
            .id_salt("epg")
            .auto_shrink(false)
            .scroll_source(egui::scroll_area::ScrollSource::ALL);
        if let Some(y) = self.scroll_to.take() {
            let y = if y.is_nan() { ((now - dc.ws) as f32 / 60.0 * PX - 60.0).clamp(0.0, size.y) } else { y };
            sa = sa.vertical_scroll_offset(y);
        }
        let mut clicked: Option<(u16, u16)> = None;
        sa.show_viewport(ui, |ui, vp| {
            let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
            let o = rect.min;
            let clip = ui.clip_rect();
            let painter = ui.painter().clone();
            let lanes = Rect::from_min_max(pos2(clip.min.x + TIME_W, clip.min.y + HEAD_H), clip.max);
            let lp = painter.with_clip_rect(lanes);
            let hover = resp.hover_pos().filter(|h| lanes.contains(*h));
            let line = Stroke::new(1.0, p.line);

            let c0 = (((vp.min.x - TIME_W) / col_w).floor().max(0.0)) as usize;
            let c1 = ((((vp.max.x - TIME_W) / col_w).ceil()).max(0.0) as usize).min(n);
            let t0 = dc.ws + (((vp.min.y - HEAD_H) / PX).max(0.0) * 60.0) as i64;
            let t1 = dc.ws + (((vp.max.y - HEAD_H) / PX).max(0.0) * 60.0) as i64 + 60;
            let mut hovered: Option<(Rect, u32, bool, bool)> = None;

            for c in c0..c1 {
                let x = o.x + TIME_W + c as f32 * col_w;
                let sid = data.channels[dc.cols[c].0].sid;
                for &ei in &dc.cols[c].1 {
                    let e = &data.events[ei as usize];
                    let end = e.end();
                    if end <= t0 || e.start >= t1 {
                        continue;
                    }
                    let top = ((e.start - dc.ws) as f32 / 60.0).max(0.0) * PX;
                    let bot = ((end - dc.ws) as f32 / 60.0).min(total_min) * PX;
                    let r = Rect::from_min_max(pos2(x, o.y + HEAD_H + top), pos2(x + col_w, o.y + HEAD_H + bot));
                    let past = end < now;
                    let resv = self.find_res(sid, e.eid, e.start).is_some();
                    if hover.is_some_and(|h| r.contains(h)) {
                        hovered = Some((r, ei, past, resv));
                        if resp.clicked() {
                            clicked = Some((sid, e.eid));
                        }
                        continue;
                    }
                    paint_prog(&lp, &p, r, e, past, resv, false);
                }
                lp.vline(x, lanes.y_range(), line);
            }
            if let Some((r, ei, past, resv)) = hovered {
                let e = &data.events[ei as usize];
                paint_prog(&lp, &p, r, e, past, resv, true);
                ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
            }
            // 現在時刻の線
            if now > dc.ws && now < dc.we {
                let y = o.y + HEAD_H + (now - dc.ws) as f32 / 60.0 * PX;
                lp.hline(lanes.x_range(), y, Stroke::new(2.0, p.now));
            }
            // 時刻列（左に固定）
            let tc = Rect::from_min_max(pos2(clip.min.x, clip.min.y + HEAD_H), pos2(clip.min.x + TIME_W, clip.max.y));
            let tp = painter.with_clip_rect(tc);
            tp.rect_filled(tc, 0.0, p.panel);
            let h0 = jst::dt(dc.ws).h as i64;
            for m in (0..(total_min as i64)).step_by(60) {
                let y = o.y + HEAD_H + m as f32 * PX;
                if y + 60.0 * PX < tc.min.y || y > tc.max.y {
                    continue;
                }
                tp.hline(tc.x_range(), y, line);
                tp.text(
                    pos2(tc.center().x, y + 3.0),
                    Align2::CENTER_TOP,
                    ((h0 + m / 60) % 24).to_string(),
                    FontId::new(12.0, bold()),
                    p.muted,
                );
            }
            tp.vline(tc.max.x - 0.5, tc.y_range(), line);
            if now > dc.ws && now < dc.we {
                let y = o.y + HEAD_H + (now - dc.ws) as f32 / 60.0 * PX;
                tp.hline(pos2(tc.max.x - 6.0, 0.0).x..=tc.max.x, y, Stroke::new(2.0, p.now));
            }
            // 局名ヘッダー（上に固定）
            let hc = Rect::from_min_max(pos2(clip.min.x + TIME_W, clip.min.y), pos2(clip.max.x, clip.min.y + HEAD_H));
            let hp = painter.with_clip_rect(hc);
            hp.rect_filled(hc, 0.0, p.panel);
            for c in c0..c1 {
                let x = o.x + TIME_W + c as f32 * col_w;
                let ch = &data.channels[dc.cols[c].0];
                let r = Rect::from_min_size(pos2(x, hc.min.y), vec2(col_w, HEAD_H));
                hp.vline(x, r.y_range(), line);
                let mut tx = x + 8.0;
                if let Some(t) = self.epg[self.bt].logos.get(&ch.sid) {
                    let ir = Rect::from_center_size(pos2(x + 8.0 + 16.0, r.center().y), vec2(32.0, 18.0));
                    hp.rect_filled(ir.expand(1.0), 3.0, Color32::WHITE);
                    hp.image(t.id(), ir, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
                    tx += 38.0;
                }
                let num = format!("{:03}", ch.number);
                let ng = hp.layout_no_wrap(num, FontId::proportional(11.0), p.muted);
                let nx = r.max.x - 6.0 - ng.size().x;
                let name_w = (nx - tx - 4.0).max(10.0);
                let mut job = LayoutJob::single_section(
                    ch.name.clone(),
                    TextFormat { font_id: FontId::new(12.0, bold()), color: p.text, ..Default::default() },
                );
                job.wrap.max_width = name_w;
                job.wrap.max_rows = 1;
                job.wrap.break_anywhere = true;
                let g = hp.layout_job(job);
                hp.galley(pos2(tx, r.center().y - g.size().y / 2.0), g, p.text);
                hp.galley(pos2(nx, r.center().y - ng.size().y / 2.0), ng, p.muted);
            }
            hp.hline(hc.x_range(), hc.max.y - 0.5, line);
            // 左上の角
            let corner = Rect::from_min_size(clip.min, vec2(TIME_W, HEAD_H));
            painter.rect_filled(corner, 0.0, p.panel);
            painter.hline(corner.x_range(), corner.max.y - 0.5, line);
            painter.vline(corner.max.x - 0.5, corner.y_range(), line);
        });
        if let Some((sid, eid)) = clicked {
            self.dialog = Some(Dialog::Program { bt: self.bt, sid, eid, quality: model::QUALITY_3X, cond: model::ONCE.into(), busy: false });
        }
    }

    // ------------------------------------------------------------ 予約一覧
    fn reserved_view(&mut self, ui: &mut egui::Ui) {
        let p = self.pal;
        Frame::new().inner_margin(Margin::symmetric(16, 8)).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("{} 件", self.reserved.len())).family(bold()));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.button("更新").clicked() {
                        self.load_reserved();
                        self.load_recng();
                    }
                });
            });
        });
        let mut edit: Option<usize> = None;
        let mut del: Option<usize> = None;
        let mut ng_del: Option<String> = None;
        ScrollArea::vertical().id_salt("resv").auto_shrink(false).show(ui, |ui| {
            Frame::new().inner_margin(Margin { left: 16, right: 16, top: 0, bottom: 24 }).show(ui, |ui| {
                if let Some(e) = &self.reserved_err {
                    ui.label(RichText::new(e).color(p.rec));
                } else if self.reserved.is_empty() {
                    ui.label(RichText::new("予約はありません。番組表から番組をクリックして予約できます。").color(p.muted));
                } else {
                    card(ui, &p, |ui| {
                        TableBuilder::new(ui)
                            .id_salt("rt")
                            .vscroll(false)
                            .striped(false)
                            .cell_layout(Layout::left_to_right(Align::Center))
                            .column(Column::exact(150.0))
                            .column(Column::exact(130.0))
                            .column(Column::remainder().clip(true))
                            .column(Column::exact(70.0))
                            .column(Column::exact(40.0))
                            .column(Column::exact(70.0))
                            .column(Column::exact(100.0))
                            .header(26.0, |mut h| {
                                for t in ["日時", "放送局", "番組", "長さ", "画質", "繰り返し", ""] {
                                    h.col(|ui| {
                                        ui.label(RichText::new(t).size(12.0).color(p.muted).family(bold()));
                                    });
                                }
                            })
                            .body(|b| {
                                b.rows(30.0, self.reserved.len(), |mut row| {
                                    let i = row.index();
                                    let r = &self.reserved[i];
                                    let s = jst::dt(r.start);
                                    let e = jst::dt(r.start + r.dur);
                                    row.col(|ui| {
                                        ui.label(format!("{} {}〜{}", s.date(), s.time(), e.time()));
                                    });
                                    row.col(|ui| {
                                        ui.add(Label::new(RichText::new(&r.ch).color(p.muted)).truncate());
                                    });
                                    row.col(|ui| {
                                        if r.recording {
                                            badge(ui, "録画中", p.rec, p.rec_soft);
                                        }
                                        if r.conflict {
                                            badge(ui, "重複", p.warn, p.warn_soft);
                                        }
                                        if r.eid == 65535 {
                                            badge(ui, "時間指定", p.muted, p.line);
                                        }
                                        ui.add(Label::new(&r.title).truncate());
                                    });
                                    row.col(|ui| {
                                        ui.label(fmt_dur(r.dur));
                                    });
                                    row.col(|ui| {
                                        ui.label(quality_label(r.quality));
                                    });
                                    row.col(|ui| {
                                        ui.label(repeat_label(&r.cond));
                                    });
                                    row.col(|ui| {
                                        if ui.small_button("変更").clicked() {
                                            edit = Some(i);
                                        }
                                        if ui.add(egui::Button::new(RichText::new("取消").color(p.rec)).small()).clicked() {
                                            del = Some(i);
                                        }
                                    });
                                });
                            });
                    });
                }
                ui.add_space(20.0);
                ui.label(RichText::new("録画できなかった予約").family(bold()));
                ui.add_space(6.0);
                match &self.recng {
                    None => {
                        ui.label(RichText::new("読み込み中…").color(p.muted));
                    }
                    Some(Err(e)) => {
                        ui.label(RichText::new(e).color(p.rec));
                    }
                    Some(Ok(v)) if v.is_empty() => {
                        ui.label(RichText::new("ありません。").color(p.muted));
                    }
                    Some(Ok(v)) => {
                        card(ui, &p, |ui| {
                            TableBuilder::new(ui)
                                .id_salt("ng")
                                .vscroll(false)
                                .cell_layout(Layout::left_to_right(Align::Center))
                                .column(Column::exact(110.0))
                                .column(Column::exact(130.0))
                                .column(Column::remainder().clip(true))
                                .column(Column::exact(120.0))
                                .column(Column::exact(60.0))
                                .header(26.0, |mut h| {
                                    for t in ["日時", "放送局", "番組", "理由", ""] {
                                        h.col(|ui| {
                                            ui.label(RichText::new(t).size(12.0).color(p.muted).family(bold()));
                                        });
                                    }
                                })
                                .body(|b| {
                                    b.rows(30.0, v.len(), |mut row| {
                                        let r = &v[row.index()];
                                        row.col(|ui| {
                                            if r.start != 0 {
                                                let d = jst::dt(r.start);
                                                ui.label(format!("{} {}", d.date(), d.time()));
                                            }
                                        });
                                        row.col(|ui| {
                                            ui.add(Label::new(RichText::new(&r.ch).color(p.muted)).truncate());
                                        });
                                        row.col(|ui| {
                                            ui.add(Label::new(&r.title).truncate());
                                        });
                                        row.col(|ui| {
                                            ui.add(Label::new(&r.reason).truncate());
                                        });
                                        row.col(|ui| {
                                            if ui.add(egui::Button::new(RichText::new("消去").color(p.rec)).small()).clicked() {
                                                ng_del = Some(r.id.clone());
                                            }
                                        });
                                    });
                                });
                        });
                    }
                }
            });
        });
        if let Some(i) = edit {
            let r = &self.reserved[i];
            self.dialog = Some(Dialog::EditRes { id: r.id.clone(), quality: r.quality, cond: r.cond.clone() });
        }
        if let Some(i) = del {
            let r = &self.reserved[i];
            self.dialog = Some(Dialog::Confirm {
                title: "予約の取り消し".into(),
                body: format!("「{}」の予約を取り消しますか？", r.title),
                ok: "取り消す".into(),
                act: Act::DeleteReserve(r.id.clone()),
            });
        }
        if let Some(id) = ng_del {
            self.act(Refresh { recng: true, ..Default::default() }, "消去しました", move |n| n.clear_failed(&id));
        }
    }

    // ------------------------------------------------------------ 録画一覧
    fn recorded_view(&mut self, ui: &mut egui::Ui) {
        let p = self.pal;
        let q = self.rec_search.trim().to_lowercase();
        let mut items: Vec<usize> = self
            .recorded
            .as_ref()
            .map(|v| {
                (0..v.len()).filter(|&i| (!self.rec_unwatched || v[i].new) && v[i].matches(&q)).collect()
            })
            .unwrap_or_default();
        if let Some(v) = &self.recorded {
            let (k, d) = self.rec_sort;
            items.sort_by(|&a, &b| {
                let (a, b) = (&v[a], &v[b]);
                let o = match k {
                    SortKey::Date => a.start.cmp(&b.start),
                    SortKey::Title => a.title.cmp(&b.title),
                    SortKey::Ch => a.ch.cmp(&b.ch),
                    SortKey::Dur => a.dur.cmp(&b.dur),
                    SortKey::Size => a.size.total_cmp(&b.size),
                };
                if d < 0 { o.reverse() } else { o }
            });
        }
        Frame::new().inner_margin(Margin::symmetric(16, 8)).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.add(TextEdit::singleline(&mut self.rec_search).hint_text("タイトル・説明で絞り込み").desired_width(260.0));
                ui.checkbox(&mut self.rec_unwatched, "未視聴のみ");
                if let Some(v) = &self.recorded {
                    let total: f64 = items.iter().map(|&i| v[i].size).sum();
                    ui.label(RichText::new(format!("{} 件・{}", items.len(), fmt_size(total))).family(bold()));
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.button("更新").clicked() {
                        self.load_recorded();
                    }
                });
            });
        });
        let Some(v) = self.recorded.clone() else {
            ui.horizontal(|ui| {
                ui.add_space(16.0);
                match &self.recorded_err {
                    Some(e) => ui.label(RichText::new(e).color(p.rec)),
                    None => ui.label(RichText::new("読み込み中…").color(p.muted)),
                };
            });
            return;
        };
        let mut sort_click: Option<SortKey> = None;
        let mut action: Option<(usize, &str)> = None;
        Frame::new().inner_margin(Margin { left: 16, right: 16, top: 0, bottom: 16 }).show(ui, |ui| {
            card(ui, &p, |ui| {
                let arrow = |k: SortKey| {
                    if self.rec_sort.0 == k { if self.rec_sort.1 > 0 { " ▲" } else { " ▼" } } else { "" }
                };
                TableBuilder::new(ui)
                    .id_salt("rec")
                    .auto_shrink(false)
                    .cell_layout(Layout::left_to_right(Align::Center))
                    .column(Column::exact(178.0))
                    .column(Column::exact(130.0))
                    .column(Column::remainder().at_least(200.0).clip(true))
                    .column(Column::exact(76.0))
                    .column(Column::exact(40.0))
                    .column(Column::exact(76.0))
                    .column(Column::exact(300.0))
                    .header(26.0, |mut h| {
                        for (t, k) in [
                            ("放送日時", Some(SortKey::Date)),
                            ("放送局", Some(SortKey::Ch)),
                            ("番組", Some(SortKey::Title)),
                            ("長さ", Some(SortKey::Dur)),
                            ("画質", None),
                            ("サイズ", Some(SortKey::Size)),
                            ("", None),
                        ] {
                            h.col(|ui| {
                                let txt = format!("{t}{}", k.map(arrow).unwrap_or(""));
                                let l = Label::new(RichText::new(txt).size(12.0).color(p.muted).family(bold()));
                                if let Some(k) = k {
                                    if ui.add(l.sense(Sense::click())).on_hover_cursor(CursorIcon::PointingHand).clicked() {
                                        sort_click = Some(k);
                                    }
                                } else {
                                    ui.add(l);
                                }
                            });
                        }
                    })
                    .body(|b| {
                        b.rows(44.0, items.len(), |mut row| {
                            let i = items[row.index()];
                            let r = &v[i];
                            let s = jst::dt(r.start);
                            row.col(|ui| {
                                ui.label(format!("{}/{} {}", s.y, s.date(), s.time()));
                            });
                            row.col(|ui| {
                                ui.add(Label::new(RichText::new(&r.ch).color(p.muted)).truncate());
                            });
                            row.col(|ui| {
                                ui.vertical(|ui| {
                                    ui.spacing_mut().item_spacing.y = 1.0;
                                    ui.horizontal(|ui| {
                                        // バッジを先に置く（長いタイトルで押し出されないように）
                                        if r.new {
                                            badge(ui, "未視聴", p.accent, p.accent_soft);
                                        }
                                        if r.recording {
                                            badge(ui, "録画中", p.rec, p.rec_soft);
                                        }
                                        if r.protect {
                                            badge(ui, "保護", p.muted, p.line);
                                        }
                                        ui.add(Label::new(&r.title).truncate());
                                    });
                                    ui.add(Label::new(RichText::new(&r.desc).size(12.0).color(p.muted)).truncate());
                                });
                            });
                            row.col(|ui| {
                                ui.label(fmt_dur(r.dur));
                            });
                            row.col(|ui| {
                                ui.label(quality_label(r.quality));
                            });
                            row.col(|ui| {
                                ui.label(fmt_size(r.size));
                            });
                            row.col(|ui| {
                                if ui.small_button(if r.new { "視聴済みに" } else { "未視聴に" }).clicked() {
                                    action = Some((i, "new"));
                                }
                                if ui.small_button(if r.protect { "保護解除" } else { "保護" }).clicked() {
                                    action = Some((i, "prot"));
                                }
                                if ui.small_button("名前").clicked() {
                                    action = Some((i, "ren"));
                                }
                                let can = r.deletable();
                                let b = ui
                                    .add_enabled(can, egui::Button::new(RichText::new("削除").color(p.rec)).small())
                                    .on_disabled_hover_text("保護中・録画中は削除できません");
                                if b.clicked() {
                                    action = Some((i, "del"));
                                }
                            });
                        });
                    });
            });
        });
        if let Some(k) = sort_click {
            self.rec_sort = if self.rec_sort.0 == k {
                (k, -self.rec_sort.1)
            } else {
                (k, if matches!(k, SortKey::Date | SortKey::Size) { -1 } else { 1 })
            };
        }
        if let Some((i, a)) = action {
            let r = v[i].clone();
            match a {
                "new" => self.update_recording(
                    r.id,
                    RecordingUpdate { unwatched: Some(!r.new), ..Default::default() },
                    if r.new { "視聴済みにしました" } else { "未視聴にしました" },
                ),
                "prot" => self.update_recording(
                    r.id,
                    RecordingUpdate { protect: Some(!r.protect), ..Default::default() },
                    if r.protect { "保護を解除しました" } else { "保護しました" },
                ),
                "ren" => self.dialog = Some(Dialog::Rename { id: r.id, text: r.title }),
                _ => {
                    self.dialog = Some(Dialog::Confirm {
                        title: "録画の削除".into(),
                        body: format!("「{}」を削除しますか？\n（{}・元に戻せません）", r.title, fmt_size(r.size)),
                        ok: "削除する".into(),
                        act: Act::DeleteRecorded(r.id),
                    })
                }
            }
        }
    }

    // ------------------------------------------------------------ nasne 情報
    fn info_view(&mut self, ui: &mut egui::Ui) {
        let p = self.pal;
        let mut home: Option<String> = None;
        Frame::new().inner_margin(Margin::symmetric(16, 8)).show(ui, |ui| {
            ui.horizontal(|ui| {
                if ui.button("nasneを光らせて見つける").clicked() {
                    self.act(Refresh::default(), "nasneのランプを点灯させました", |n| n.blink());
                }
                if let Some(d) = &self.info
                    && ui.link("nasne HOME（設定の変更はこちら）").clicked()
                {
                    home = Some(d.home_url.clone());
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.button("更新").clicked() {
                        self.load_info();
                    }
                    ui.add_space(12.0);
                    ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                        ui.add_space(ui.available_width() - 350.0);
                        self.host_editor(ui);
                    });
                });
            });
        });
        if let Some(u) = home {
            open_url(&u);
        }
        let Some(d) = self.info.clone() else {
            ui.horizontal(|ui| {
                ui.add_space(16.0);
                ui.label(RichText::new("読み込み中…").color(p.muted));
            });
            return;
        };
        ScrollArea::vertical().id_salt("info").auto_shrink(false).show(ui, |ui| {
            Frame::new().inner_margin(Margin { left: 16, right: 16, top: 0, bottom: 24 }).show(ui, |ui| {
                info_cards(ui, &p, &d);
            });
        });
    }

    // ------------------------------------------------------------ ダイアログ
    fn dialogs(&mut self, ctx: &egui::Context) {
        let Some(mut dlg) = self.dialog.take() else { return };
        let p = self.pal;
        let mut keep = true;
        let modal = Modal::new(Id::unique("dlg"))
            .frame(Frame::new().fill(p.panel).corner_radius(10).inner_margin(Margin::symmetric(20, 18)).stroke(Stroke::new(1.0, p.line)));
        let resp = modal.show(ctx, |ui| {
            ui.set_width(520.0f32.min(ctx.content_rect().width() - 72.0));
            match &mut dlg {
                Dialog::Program { bt, sid, eid, quality, cond, busy } => {
                    let Some(data) = self.epg[*bt].data.clone() else {
                        keep = false;
                        return;
                    };
                    let Some(e) = data.get(*sid, *eid) else {
                        keep = false;
                        return;
                    };
                    let ch = data.channels.iter().find(|c| c.sid == *sid).map(|c| c.name.clone()).unwrap_or_default();
                    let (s, en) = (jst::dt(e.start), jst::dt(e.end()));
                    let resv = self.find_res(*sid, *eid, e.start).cloned();
                    let ended = e.end() < jst::now();
                    ui.horizontal_wrapped(|ui| {
                        if e.pay {
                            badge(ui, "有料", p.warn, p.warn_soft);
                        }
                        ui.label(RichText::new(&e.title).size(17.0).family(bold()).color(p.text));
                    });
                    ui.horizontal_wrapped(|ui| {
                        ui.label(
                            RichText::new(format!(
                                "{} {}〜{}（{}）・{}",
                                s.date(),
                                s.time(),
                                en.time(),
                                fmt_dur(e.end() - e.start),
                                ch
                            ))
                            .size(13.0)
                            .color(p.muted),
                        );
                        if let (Some(g), Some(c)) = (e.genre, genre_color(e.genre))
                            && !GENRES[g as usize].is_empty()
                        {
                            badge(ui, GENRES[g as usize], Color32::WHITE, c);
                        }
                        if let Some(r) = &resv {
                            badge(ui, &format!("予約済み・{}", repeat_label(&r.cond)), p.rec, p.rec_soft);
                        }
                    });
                    ui.add_space(8.0);
                    ScrollArea::vertical().max_height(ctx.content_rect().height() * 0.55).show(ui, |ui| {
                        if e.desc.is_empty() {
                            ui.label(RichText::new("説明なし").color(p.muted));
                        } else {
                            ui.add(Label::new(RichText::new(&e.desc).size(13.0)).selectable(true));
                        }
                        for (k, v) in &e.ext {
                            ui.add_space(8.0);
                            ui.label(RichText::new(k).size(12.0).color(p.muted).family(bold()));
                            ui.add(Label::new(RichText::new(v).size(13.0)).selectable(true));
                        }
                    });
                    ui.add_space(14.0);
                    ui.horizontal(|ui| {
                        if resv.is_none() && !ended {
                            ui.label("画質");
                            quality_combo(ui, "q", quality);
                            ui.label("繰り返し");
                            cond_combo(ui, "cond", e.start, cond);
                        }
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if !ended {
                                if let Some(r) = &resv {
                                    if danger_button(ui, &p, "予約を取り消す", true).clicked() {
                                        let id = r.id.clone();
                                        self.act(Refresh { reserved: true, ..Default::default() }, "予約を取り消しました", move |n| {
                                            n.cancel_reservation(&id)
                                        });
                                    }
                                    if ui.button("予約を変更").clicked() {
                                        self.dialog = Some(Dialog::EditRes {
                                            id: r.id.clone(),
                                            quality: r.quality,
                                            cond: r.cond.clone(),
                                        });
                                        keep = false;
                                    }
                                } else if primary_button(ui, &p, "録画予約", !*busy).clicked() {
                                    *busy = true;
                                    self.reserve(ReserveReq::for_event(Band::ALL[*bt], e, *quality, cond), false);
                                }
                            }
                            if ui.button("閉じる").clicked() {
                                keep = false;
                            }
                        });
                    });
                }
                Dialog::EditRes { id, quality, cond } => {
                    let Some(r) = self.reserved.iter().find(|r| &r.id == id).cloned() else {
                        keep = false;
                        return;
                    };
                    let (s, e) = (jst::dt(r.start), jst::dt(r.start + r.dur));
                    ui.label(RichText::new("予約を変更").size(17.0).family(bold()));
                    ui.label(
                        RichText::new(format!("{}\n{} {}〜{}・{}", r.title, s.date(), s.time(), e.time(), r.ch))
                            .size(13.0)
                            .color(p.muted),
                    );
                    ui.add_space(12.0);
                    ui.horizontal(|ui| {
                        ui.label("画質");
                        quality_combo(ui, "eq", quality);
                        ui.label("繰り返し");
                        cond_combo(ui, "ec", r.start, cond);
                    });
                    if r.conflict {
                        ui.add_space(8.0);
                        ui.label(
                            RichText::new("この予約は他の予約と重なっています。「優先度を上げる」で、重なった時間にこちらを録画させられます。")
                                .size(13.0)
                                .color(p.rec),
                        );
                    }
                    ui.add_space(14.0);
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let save = |prio: bool, this: &mut App| {
                            let (id, q, c) = (id.clone(), *quality, cond.clone());
                            let msg = if prio { "優先度を上げました" } else { "予約を変更しました" };
                            this.act(Refresh { reserved: true, ..Default::default() }, msg, move |n| {
                                n.update_reservation(&id, q, &c, prio)
                            });
                        };
                        if primary_button(ui, &p, "保存", true).clicked() {
                            save(false, self);
                        }
                        if r.conflict && ui.button("優先度を上げる").clicked() {
                            save(true, self);
                        }
                        if ui.button("キャンセル").clicked() {
                            keep = false;
                        }
                    });
                }
                Dialog::Confirm { title, body, ok, act } => {
                    ui.label(RichText::new(title.as_str()).size(17.0).family(bold()));
                    ui.add_space(8.0);
                    ui.label(body.as_str());
                    ui.add_space(14.0);
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if danger_button(ui, &p, ok, true).clicked() {
                            keep = false;
                            match std::mem::replace(act, Act::DeleteReserve(String::new())) {
                                Act::DeleteReserve(id) => {
                                    self.act(Refresh { reserved: true, ..Default::default() }, "予約を取り消しました", move |n| {
                                        n.cancel_reservation(&id)
                                    })
                                }
                                Act::DeleteRecorded(id) => {
                                    if let Some(v) = &mut self.recorded {
                                        v.retain(|r| r.id != id);
                                    }
                                    self.act(Refresh { status: true, ..Default::default() }, "削除しました", move |n| {
                                        n.delete_recording(&id)
                                    })
                                }
                                Act::Reserve(req) => self.reserve(req, true),
                            }
                        }
                        if ui.button("キャンセル").clicked() {
                            keep = false;
                        }
                    });
                }
                Dialog::Rename { id, text } => {
                    ui.label(RichText::new("タイトルを変更").size(17.0).family(bold()));
                    ui.add_space(8.0);
                    let r = ui.add(TextEdit::singleline(text).desired_width(f32::INFINITY));
                    if !r.has_focus() && ui.memory(|m| m.focused().is_none()) {
                        r.request_focus();
                    }
                    let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    ui.add_space(14.0);
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if primary_button(ui, &p, "変更", !text.trim().is_empty()).clicked() || (enter && !text.trim().is_empty()) {
                            keep = false;
                            let t = text.trim().to_string();
                            let cur = self.recorded.as_ref().and_then(|v| v.iter().find(|r| &r.id == id)).map(|r| r.title.clone());
                            if cur.as_deref() != Some(t.as_str()) {
                                self.update_recording(
                                    id.clone(),
                                    RecordingUpdate { title: Some(t), ..Default::default() },
                                    "タイトルを変更しました",
                                );
                            }
                        }
                        if ui.button("キャンセル").clicked() {
                            keep = false;
                        }
                    });
                }
            }
        });
        if resp.should_close() {
            keep = false;
        }
        // ダイアログの中で別のダイアログに切り替えた場合はそちらを優先
        if keep && self.dialog.is_none() {
            self.dialog = Some(dlg);
        }
    }

    fn paint_toast(&mut self, ctx: &egui::Context) {
        let Some((msg, t)) = &self.toast else { return };
        let el = t.elapsed().as_secs_f32();
        if el > 2.8 {
            self.toast = None;
            return;
        }
        let p = self.pal;
        let alpha = ((2.8 - el) / 0.2).clamp(0.0, 1.0);
        let r = ctx.content_rect();
        egui::Area::new(Id::unique("toast"))
            .order(egui::Order::Tooltip)
            .anchor(Align2::CENTER_BOTTOM, vec2(0.0, -20.0))
            .interactable(false)
            .show(ctx, |ui| {
                ui.set_max_width(r.width() - 40.0);
                Frame::new()
                    .fill(p.text.gamma_multiply(alpha))
                    .corner_radius(8)
                    .inner_margin(Margin::symmetric(16, 8))
                    .show(ui, |ui| {
                        ui.label(RichText::new(msg.as_str()).size(13.0).color(p.bg.gamma_multiply(alpha)));
                    });
            });
        ctx.request_repaint_after(Duration::from_millis(if el > 2.5 { 16 } else { 200 }));
    }
}

fn card(ui: &mut egui::Ui, p: &Pal, add: impl FnOnce(&mut egui::Ui)) {
    Frame::new()
        .fill(p.panel)
        .stroke(Stroke::new(1.0, p.line))
        .corner_radius(8)
        .inner_margin(Margin::symmetric(10, 6))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        });
}

fn paint_prog(pt: &egui::Painter, p: &Pal, r: Rect, e: &epg::Event, past: bool, resv: bool, hovered: bool) {
    let inner_w = r.width() - 10.0;
    let mut job = LayoutJob::default();
    job.wrap.max_width = inner_w.max(10.0);
    let lh = |size: f32| Some(size * 1.35);
    let fmt = |size: f32, color: Color32, fam: FontFamily| TextFormat {
        font_id: FontId::new(size, fam),
        color,
        line_height: lh(size),
        ..Default::default()
    };
    let minute = jst::dt(e.start).mi;
    job.append(&format!("{minute:02} "), 0.0, fmt(12.0, p.muted, FontFamily::Proportional));
    if e.pay {
        let mut f = fmt(10.0, p.warn, bold());
        f.background = p.warn_soft;
        job.append("有料", 0.0, f);
        job.append(" ", 0.0, fmt(12.0, p.text, FontFamily::Proportional));
    }
    job.append(&e.title, 0.0, fmt(12.0, if past { p.muted } else { p.text }, bold()));
    if (hovered || r.height() > 44.0) && !e.desc.is_empty() {
        job.append("\n", 0.0, fmt(11.0, p.muted, FontFamily::Proportional));
        job.append(&e.desc, 0.0, fmt(11.0, p.muted, FontFamily::Proportional));
    }
    let g = pt.layout_job(job);
    let r = if hovered { Rect::from_min_size(r.min, vec2(r.width(), r.height().max(g.size().y + 6.0))) } else { r };
    let fill = if hovered {
        p.accent_soft
    } else if resv {
        p.rec_soft
    } else if past {
        p.past
    } else {
        p.panel
    };
    if hovered {
        pt.rect_filled(r.translate(vec2(0.0, 2.0)).expand(2.0), 4.0, Color32::from_black_alpha(40));
    }
    pt.rect_filled(r, 0.0, fill);
    pt.hline(r.x_range(), r.max.y - 0.5, Stroke::new(1.0, p.line));
    let mut x = r.min.x;
    if let Some(c) = genre_color(e.genre) {
        pt.rect_filled(Rect::from_min_size(r.min, vec2(3.0, r.height())), 0.0, c);
        x += 3.0;
    }
    if resv {
        pt.rect_filled(Rect::from_min_size(pos2(x, r.min.y), vec2(3.0, r.height())), 0.0, p.rec);
    }
    pt.with_clip_rect(r.intersect(pt.clip_rect())).galley(pos2(r.min.x + 6.0, r.min.y + 2.0), g, p.text);
}

fn kv_grid(ui: &mut egui::Ui, p: &Pal, id: &str, rows: Vec<(&str, RichText)>) {
    egui::Grid::new(id).num_columns(2).spacing(vec2(14.0, 4.0)).show(ui, |ui| {
        for (k, v) in rows {
            ui.label(RichText::new(k).size(13.0).color(p.muted));
            ui.add(Label::new(v.size(13.0)).wrap());
            ui.end_row();
        }
    });
}

fn info_cards(ui: &mut egui::Ui, p: &Pal, d: &Info) {
    let onoff = |b: bool| if b { RichText::new("オン").color(p.ok).family(bold()) } else { RichText::new("オフ").color(p.muted) };
    let t = |s: &str| RichText::new(s);
    let n = &d.network;
    type Body<'a> = Box<dyn FnOnce(&mut egui::Ui) + 'a>;
    let mut cards: Vec<(&str, Body)> = Vec::new();
    {
        cards.push(("本体", Box::new(|ui| {
            kv_grid(ui, p, "k1", vec![
                ("名前", t(&d.name)),
                ("型番", t(&d.model)),
                ("ソフトウェア", t(&d.software)),
                ("nasne時刻", t(&d.clock)),
                ("時刻補正(TOT)", if d.tot { RichText::new("受信済み").color(p.ok) } else { RichText::new("未受信").color(p.muted) }),
                ("録画中", t(if d.recording { "はい" } else { "いいえ" })),
                ("配信中のクライアント", t(&format!("{} 台", d.streaming))),
            ]);
        })));
        cards.push(("ネットワーク", Box::new(|ui| {
            kv_grid(ui, p, "k2", vec![
                ("IPアドレス", t(&n.ip)),
                ("取得方法", t(if n.dhcp { "自動(DHCP)" } else { "手動" })),
                ("サブネット", t(&n.subnet)),
                ("ルーター", t(&n.gateway)),
                ("DNS", t(&n.dns.join(", "))),
                ("MAC", t(&n.mac)),
            ]);
        })));
        cards.push(("ハードディスク", Box::new(|ui| {
            for (i, h) in d.hdd.iter().enumerate() {
                if i > 0 {
                    ui.separator();
                }
                let kind = t(if h.internal { "内蔵HDD" } else { "外付けHDD" });
                if h.mounted {
                    kv_grid(ui, p, &format!("hdd{i}"), vec![
                        ("種類", kind),
                        ("機種", t(&h.product)),
                        ("形式", t(&h.format)),
                        ("使用量", t(&format!("{} / {}（残り {}）", fmt_size(h.used), fmt_size(h.total), fmt_size(h.free)))),
                    ]);
                    let bw = ui.available_width();
                    let (r, _) = ui.allocate_exact_size(vec2(bw, 6.0), Sense::hover());
                    ui.painter().rect_filled(r, 3.0, p.line);
                    let fr = if h.total > 0.0 { (h.used / h.total) as f32 } else { 0.0 };
                    ui.painter().rect_filled(Rect::from_min_size(r.min, vec2(bw * fr, 6.0)), 3.0, p.accent);
                } else {
                    kv_grid(ui, p, &format!("hdd{i}"), vec![("種類", kind), ("状態", RichText::new("未接続・未使用").color(p.muted))]);
                }
            }
        })));
        cards.push(("設定（変更は nasne HOME で）", Box::new(|ui| {
            let st = &d.settings;
            let pr = st.parental_rating;
            kv_grid(ui, p, "k4", vec![
                ("省電力モード", onoff(st.power_saving)),
                ("BSアンテナ給電", onoff(st.bs_antenna_power)),
                ("イベントリレー", onoff(st.event_relay)),
                ("周波数変更の自動追従", onoff(st.frequency_follow)),
                ("視聴年齢制限", t(&if pr != 0 { format!("設定あり（値 {pr}）") } else { "制限なし".into() })),
                ("DLNAクライアント登録", t(if st.dlna_manual_register { "手動" } else { "自動" })),
            ]);
        })));
        cards.push(("ファイルサーバー（NAS）", Box::new(|ui| {
            let nas = &d.nas;
            let mut rows = vec![
                ("機能", onoff(nas.enabled)),
                ("サーバー名", t(&nas.name)),
                ("ワークグループ", t(&nas.workgroup)),
                ("共有フォルダ", t(&nas.shared.join(", "))),
            ];
            if nas.enabled {
                rows.push(("接続先", RichText::new(nas.url(&n.ip)).monospace()));
            }
            kv_grid(ui, p, "k5", rows);
        })));
    }
    // 幅に収まる列数で行を組む（horizontal_wrapped は Frame を折り返さないため自前で割り付け）
    const GAP: f32 = 12.0;
    const MIN_W: f32 = 300.0;
    const PAD: f32 = 14.0 * 2.0 + 2.0; // inner_margin + stroke
    let avail = ui.available_width();
    let ncol = (((avail + GAP) / (MIN_W + PAD + GAP)).floor() as usize).clamp(1, cards.len());
    let w = ((avail - GAP * (ncol - 1) as f32) / ncol as f32 - PAD).floor();
    let mut it = cards.into_iter().peekable();
    while it.peek().is_some() {
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = GAP;
            for (title, f) in it.by_ref().take(ncol) {
                Frame::new()
                    .fill(p.panel)
                    .stroke(Stroke::new(1.0, p.line))
                    .corner_radius(8)
                    .inner_margin(Margin::symmetric(14, 12))
                    .show(ui, |ui| {
                        ui.set_width(w);
                        ui.set_min_height(150.0);
                        ui.vertical(|ui| {
                            ui.label(RichText::new(title).size(13.0).family(bold()));
                            ui.add_space(6.0);
                            f(ui);
                        });
                    });
            }
        });
        ui.add_space(GAP);
    }
    let dmp = &d.clients;
    ui.add_space(20.0);
    ui.label(RichText::new(format!("DLNAクライアント（{} 台）", dmp.len())).family(bold()));
    ui.add_space(6.0);
    card(ui, p, |ui| {
        TableBuilder::new(ui)
            .id_salt("dmp")
            .vscroll(false)
            .cell_layout(Layout::left_to_right(Align::Center))
            .column(Column::exact(200.0).clip(true))
            .column(Column::exact(150.0))
            .column(Column::exact(80.0))
            .column(Column::exact(140.0))
            .column(Column::remainder().clip(true))
            .header(26.0, |mut h| {
                for x in ["名前", "MACアドレス", "登録", "最終アクセス", "User-Agent"] {
                    h.col(|ui| {
                        ui.label(RichText::new(x).size(12.0).color(p.muted).family(bold()));
                    });
                }
            })
            .body(|b| {
                b.rows(28.0, dmp.len(), |mut row| {
                    let c = &dmp[row.index()];
                    row.col(|ui| {
                        ui.add(Label::new(if c.name.is_empty() { "（名前なし）" } else { &c.name }).truncate());
                    });
                    row.col(|ui| {
                        ui.label(RichText::new(&c.mac).color(p.muted));
                    });
                    row.col(|ui| {
                        if c.registered {
                            ui.label(RichText::new("登録済み").color(p.ok));
                        } else {
                            ui.label(RichText::new("未登録").color(p.muted));
                        }
                    });
                    row.col(|ui| {
                        ui.label(&c.last_access);
                    });
                    row.col(|ui| {
                        ui.add(Label::new(RichText::new(&c.user_agent).size(12.0).color(p.muted)).truncate());
                    });
                });
            });
    });
}
