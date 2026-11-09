//! nasne の番組表・録画予約・録画管理の共通部分
//!
//! nasne-gui（デスクトップアプリ）・nasne（コマンドライン）・nasne-mcp（MCP サーバ）は、どれもこのクレートを通して nasne を操作する。
//! 画面やコマンドの側は、表示と入力だけを受け持つ。
//!
//! | モジュール | 内容 |
//! |---|---|
//! | [`client`] | HTTP 通信（GET / 署名付き POST）と LAN 内の nasne の検出 |
//! | [`ops`] | 一覧の取得・予約・録画の変更などの操作（[`Nasne`] のメソッド） |
//! | [`model`] | 予約・録画などのデータと、画質・繰り返しの表示名 |
//! | [`info`] | 本体情報（読み取りのみ） |
//! | [`epg`] | 番組表の取得・解析・キャッシュ、局ロゴ |
//! | [`program`] | 番組 ID・番組の検索・局の絞り込み |
//! | [`store`] | ディスクキャッシュと設定（GUI とコマンドで共有） |
//! | [`jst`] | 日本時間の日時計算 |
//! | [`gaiji`] | nasne の外字と [字] などの表示用文字列の変換 |
//!
//! 放送内容そのもの（映像・音声）、著作権保護（DTCP-IP）、限定受信（B-CAS）、
//! 元に戻せない設定の API には触れない。

mod arib;
pub mod client;
pub mod epg;
pub mod gaiji;
pub mod info;
pub mod json;
pub mod jst;
pub mod model;
pub mod ops;
pub mod program;
pub mod store;

pub use client::{Nasne, Res};
pub use epg::{Band, Epg, Event};
