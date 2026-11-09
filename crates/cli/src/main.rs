//! nasne: nasne の番組表・録画予約・録画管理のコマンドライン
//!
//! 引数の定義と、接続先の決定。各コマンドの中身は programs.rs（番組表）と manage.rs（予約・録画・本体）。

mod manage;
mod programs;
mod table;
mod when;

use clap::{Args, Parser, Subcommand, ValueEnum};
use nasne_core::model::{self, QUALITY_3X, QUALITY_DR};
use nasne_core::{Band, Nasne, Res, client, store};
use std::io::IsTerminal as _;
use std::process::ExitCode;

#[derive(Parser)]
#[command(
    name = "nasne",
    version,
    about = "nasne の番組表・録画予約・録画管理（視聴機能なし）",
    after_help = "接続先は --host、環境変数 NASNE_HOST、保存済みの接続先（nasne-gui と共通）の順に使います。\n\
                  どれもなければ LAN 内の nasne を探して保存します。"
)]
struct Cli {
    /// 接続先の nasne の IP アドレス
    #[arg(long, global = true, value_name = "IP")]
    host: Option<String>,

    /// 結果を JSON で出力する（一覧・詳細を表示するコマンド）
    #[arg(long, global = true)]
    json: bool,

    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// 本体の状態（HDD の残り・録画中か）
    Status,
    /// 本体情報（型番・ネットワーク・HDD・設定など。読み取りのみ）
    Info,
    /// 本体のランプを点灯させる（どの nasne か見つけるため）
    Blink,
    /// 接続先を表示する。IP アドレスを渡すとそれを保存する
    Host {
        /// 保存する接続先
        ip: Option<String>,
        /// LAN 内の nasne を探して保存する
        #[arg(long, conflicts_with = "ip")]
        discover: bool,
    },
    /// チャンネル一覧
    Channels {
        #[arg(short, long, value_enum)]
        band: Option<BandArg>,
    },
    /// 番組表
    #[command(subcommand)]
    Epg(EpgCmd),
    /// 録画予約
    #[command(subcommand, visible_alias = "reservation")]
    Resv(ResvCmd),
    /// 録画した番組
    #[command(subcommand, visible_alias = "recording")]
    Rec(RecCmd),
}

#[derive(Subcommand)]
pub enum EpgCmd {
    /// 放送中の番組
    Now {
        #[arg(short, long, value_enum)]
        band: Option<BandArg>,
    },
    /// 番組表（局ごと・時間順）
    List {
        #[command(flatten)]
        filter: ChannelFilter,
        /// 日付（today / tomorrow / +2 / 11-12 / 2026-11-12。1 日は 4:00〜翌 4:00）
        #[arg(short, long, default_value = "today")]
        date: String,
        /// この時刻から（HH:MM。0〜3 時は翌日の深夜。今日の既定は現在時刻）
        #[arg(long, value_name = "HH:MM")]
        from: Option<String>,
        /// この時刻まで（HH:MM。既定は翌 4:00）
        #[arg(long, value_name = "HH:MM")]
        to: Option<String>,
    },
    /// 番組を探す（タイトル・説明・詳しい説明。複数の語はすべてを含むもの）
    Search {
        #[arg(required = true)]
        words: Vec<String>,
        #[command(flatten)]
        filter: ChannelFilter,
        /// ジャンル（ニュース / スポーツ / ドラマ / アニメ / 映画 など）
        #[arg(short, long)]
        genre: Option<String>,
    },
    /// 番組の詳細
    Show {
        /// 番組 ID（gr:30720:1234 の形。一覧の左端の列）
        program: String,
    },
}

#[derive(Args)]
pub struct ChannelFilter {
    /// 放送波（省略するとすべて）
    #[arg(short, long, value_enum)]
    band: Option<BandArg>,
    /// 局（名前の一部・チャンネル番号・サービス ID）
    #[arg(short, long, value_name = "CH")]
    channel: Option<String>,
}

#[derive(Subcommand)]
pub enum ResvCmd {
    /// 予約の一覧
    #[command(visible_alias = "ls")]
    List,
    /// 番組を録画予約する
    Add {
        /// 番組 ID（nasne epg の一覧の左端の列）
        program: String,
        #[command(flatten)]
        opts: ResvOpts,
        /// ほかの予約と時間が重なっていても予約する
        #[arg(long)]
        force: bool,
    },
    /// 予約の画質・繰り返しを変える
    Edit {
        /// 予約 ID（nasne resv list の左端の列）
        id: String,
        /// 画質
        #[arg(short, long, value_enum)]
        quality: Option<QualityArg>,
        /// 繰り返し
        #[arg(short, long, value_enum)]
        repeat: Option<RepeatArg>,
        /// 重なっている予約のうち、この予約を優先して録画させる
        #[arg(long)]
        raise_priority: bool,
    },
    /// 予約を取り消す
    #[command(visible_alias = "rm")]
    Cancel {
        id: String,
        /// 確認しない
        #[arg(short, long)]
        yes: bool,
    },
    /// 録画できなかった予約の一覧
    Failed,
    /// 録画できなかった予約を一覧から消す
    ClearFailed { id: String },
}

#[derive(Args)]
pub struct ResvOpts {
    /// 画質
    #[arg(short, long, value_enum, default_value = "3x")]
    quality: QualityArg,
    /// 繰り返し（weekly は番組の曜日で毎週）
    #[arg(short, long, value_enum, default_value = "once")]
    repeat: RepeatArg,
}

#[derive(Subcommand)]
pub enum RecCmd {
    /// 録画の一覧
    #[command(visible_alias = "ls")]
    List {
        /// タイトル・説明で絞り込む
        #[arg(short, long, value_name = "WORDS")]
        search: Option<String>,
        /// 未視聴のものだけ
        #[arg(short, long)]
        unwatched: bool,
        /// 並べ替え
        #[arg(long, value_enum, default_value = "date")]
        sort: SortArg,
        /// 逆順にする（既定は日付・サイズが新しい / 大きい順、それ以外は昇順）
        #[arg(short, long)]
        reverse: bool,
    },
    /// タイトルを変える
    Rename { id: String, title: String },
    /// 保護する（削除できなくする）
    Protect { id: String },
    /// 保護を解除する
    Unprotect { id: String },
    /// 視聴済みにする
    Watched { id: String },
    /// 未視聴にする
    Unwatched { id: String },
    /// 録画を削除する（元に戻せません）
    #[command(visible_alias = "rm")]
    Delete {
        id: String,
        /// 確認しない
        #[arg(short, long)]
        yes: bool,
    },
}

#[derive(Clone, Copy, ValueEnum)]
pub enum BandArg {
    /// 地デジ
    Gr,
    Bs,
    Cs,
}

impl BandArg {
    pub fn band(self) -> Band {
        match self {
            BandArg::Gr => Band::Gr,
            BandArg::Bs => Band::Bs,
            BandArg::Cs => Band::Cs,
        }
    }
}

/// 指定がなければすべての放送波
pub fn bands(b: Option<BandArg>) -> Vec<Band> {
    b.map(|b| vec![b.band()]).unwrap_or_else(|| Band::ALL.to_vec())
}

#[derive(Clone, Copy, ValueEnum)]
pub enum QualityArg {
    Dr,
    #[value(name = "3x")]
    X3,
}

impl QualityArg {
    pub fn code(self) -> i64 {
        match self {
            QualityArg::Dr => QUALITY_DR,
            QualityArg::X3 => QUALITY_3X,
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
pub enum RepeatArg {
    /// 1回のみ
    Once,
    /// 毎日
    Daily,
    /// 毎週（番組の曜日）
    Weekly,
    /// 月〜金
    MonFri,
    /// 月〜土
    MonSat,
}

impl RepeatArg {
    /// conditionId。weekly は番組の開始時刻の曜日で決まる
    pub fn cond(self, start: i64) -> String {
        match self {
            RepeatArg::Once => model::ONCE.into(),
            RepeatArg::Daily => model::DAILY.into(),
            RepeatArg::Weekly => model::weekly(start),
            RepeatArg::MonFri => model::MON_FRI.into(),
            RepeatArg::MonSat => model::MON_SAT.into(),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum SortArg {
    Date,
    Title,
    Channel,
    Duration,
    Size,
}

/// 結果の出し方
pub struct Out {
    pub json: bool,
}

impl Out {
    /// --json なら JSON を出して true
    pub fn json<T: serde::Serialize + ?Sized>(&self, v: &T) -> bool {
        if self.json {
            match serde_json::to_string_pretty(v) {
                Ok(s) => println!("{s}"),
                Err(e) => eprintln!("JSON にできません: {e}"),
            }
        }
        self.json
    }
}

/// 元に戻せない操作の前の確認。端末でなければ -y が必要
pub fn confirm(question: &str, yes: bool) -> Res<()> {
    if yes {
        return Ok(());
    }
    if !std::io::stdin().is_terminal() {
        return Err("確認できません。実行するには -y を付けてください".into());
    }
    eprint!("{question} [y/N] ");
    let mut s = String::new();
    std::io::stdin().read_line(&mut s).map_err(|e| e.to_string())?;
    if matches!(s.trim(), "y" | "Y" | "yes") { Ok(()) } else { Err("中止しました".into()) }
}

/// 接続先: --host → NASNE_HOST → 保存済み → LAN 内を探して保存
fn connect(host: Option<String>) -> Res<Nasne> {
    if let Some(h) = host.filter(|h| !h.trim().is_empty()).or_else(store::load_host) {
        return Ok(Nasne::new(&h));
    }
    eprintln!("nasne を探しています…");
    let h = client::discover().ok_or("nasne が見つかりませんでした。--host で IP アドレスを指定してください")?;
    eprintln!("nasne を見つけました（{h}）。次回からはこの接続先を使います");
    store::save_host(&h);
    Ok(Nasne::new(&h))
}

fn host_cmd(flag: Option<String>, ip: Option<String>, discover: bool) -> Res<()> {
    let save = |h: &str| -> Res<()> {
        let n = Nasne::new(h);
        let st = n.status().map_err(|e| format!("{h} に接続できません: {e}"))?;
        store::save_host(&n.host);
        println!("{}（{}）を接続先にしました", n.host, st.name);
        Ok(())
    };
    if discover {
        eprintln!("nasne を探しています…");
        let h = client::discover().ok_or("nasne が見つかりませんでした")?;
        return save(&h);
    }
    if let Some(ip) = ip {
        return save(&ip);
    }
    let (h, from) = if let Some(h) = flag {
        (h, "--host")
    } else if let Some(h) = std::env::var("NASNE_HOST").ok().filter(|h| !h.trim().is_empty()) {
        (h, "環境変数 NASNE_HOST")
    } else if let Some(h) = store::saved_host() {
        (h, "保存済み")
    } else {
        println!("接続先は未設定です（nasne host --discover で探せます）");
        return Ok(());
    };
    println!("{}（{from}）", h.trim());
    Ok(())
}

fn run(cli: Cli) -> Res<()> {
    let out = Out { json: cli.json };
    if let Cmd::Host { ip, discover } = cli.cmd {
        return host_cmd(cli.host, ip, discover);
    }
    let n = connect(cli.host)?;
    match cli.cmd {
        Cmd::Host { .. } => unreachable!(),
        Cmd::Status => manage::status(&n, &out),
        Cmd::Info => manage::info(&n, &out),
        Cmd::Blink => {
            n.blink()?;
            println!("nasne のランプを点灯させました");
            Ok(())
        }
        Cmd::Channels { band } => programs::channels(&n, &out, band),
        Cmd::Epg(c) => programs::run(&n, &out, c),
        Cmd::Resv(c) => manage::resv(&n, &out, c),
        Cmd::Rec(c) => manage::rec(&n, &out, c),
    }
}

/// `nasne rec list | head` などで出力先が先に閉じたとき、panic せず静かに終わる（Rust は SIGPIPE を無視する設定で起動するため戻す）
#[cfg(unix)]
fn default_sigpipe() {
    const SIGPIPE: i32 = 13; // Linux / macOS 共通
    const SIG_DFL: usize = 0;
    unsafe extern "C" {
        fn signal(sig: i32, handler: usize) -> usize;
    }
    // SAFETY: 起動直後でほかのスレッドがないうちに、標準の動作へ戻すだけ
    unsafe {
        signal(SIGPIPE, SIG_DFL);
    }
}

fn main() -> ExitCode {
    #[cfg(unix)]
    default_sigpipe();
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("エラー: {e}");
            ExitCode::FAILURE
        }
    }
}
