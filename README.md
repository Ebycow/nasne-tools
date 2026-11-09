# nasne-tools

nasne の **番組表・録画予約・録画一覧・本体情報** をパソコンから扱うための、非公式のツールです。

- **nasne-gui** … デスクトップアプリ。番組表を見ながら予約や録画の整理ができます。
- **nasne** … コマンドライン。同じことを端末やスクリプトから行えます（`--json` で機械可読な出力）。

どちらも同じ LAN にある nasne に直接つなぎます。インターネットやクラウドのサービスは使いません。
macOS / Windows / Linux で動き、それぞれ単一の実行ファイルです。Rust で書いています（GUI は [egui](https://github.com/emilk/egui)）。

**番組の視聴・ダビング・書き出しはできません。** 映像や音声には一切触れず、番組の情報と録画の管理だけを扱います（[方針](#方針)）。

## できること

**番組表**
- 地デジ / BS / CS の番組表を 8 日分扱えます。GUI では局ロゴ、ジャンル別の色分け、現在時刻の線つきで表示します。
- 番組を選んでそのまま録画予約できます（画質 DR / 3倍、繰り返し: 1回のみ・毎日・毎週・月〜金・月〜土）。
- ほかの予約と時間が重なるときは、予約する前に知らせます（nasne のチューナーは 1 つです）。
- コマンドラインでは、放送中の番組の一覧や、タイトル・説明・ジャンルでの番組検索もできます。

**予約一覧**
- 予約の変更・取り消しができます。重なっている予約は「優先度を上げる」で、重なった時間にどちらを録画するかを選べます。
- 録画できなかった予約を、その理由とあわせて表示します。

**録画一覧**
- タイトルや説明で絞り込めます。未視聴のものだけを表示することもできます。
- 視聴済み / 未視聴の切り替え、保護 / 保護解除、タイトルの変更、削除ができます。

**nasne 情報**
- 型番やソフトウェアのバージョン、HDD の使用量、ネットワーク、DLNA クライアントなどを表示します（読み取りのみ）。
- 本体のランプを点灯させて、どの nasne かを見つけられます。
- 設定の変更は nasne HOME（nasne 本体の設定画面）で行います。GUI からはリンクで開けます。

## 動作環境

- nasne と同じ LAN につながったパソコン
- GUI を使う場合は日本語フォント
  - macOS: ヒラギノ（標準で入っています）
  - Windows: 游ゴシック / メイリオ（標準で入っています）
  - Linux: Noto Sans CJK などの日本語フォントをインストールしてください（見つからない場合は `fc-match` で探します）

## ビルド

[Rust](https://www.rust-lang.org/ja/tools/install) 1.88 以降が必要です。

```sh
git clone <このリポジトリの URL>
cd nasne-tools
cargo build --release
```

`target/release/` に次の 2 つができます（Windows は `.exe` つき）。どちらもほかのファイルを置かなくても動きます。

| 実行ファイル | 内容 | 大きさ（目安） |
|---|---|---|
| `nasne-gui` | デスクトップアプリ | 約 5.5 MB |
| `nasne` | コマンドライン | 約 1.5 MB |

片方だけ作るときは `cargo build --release -p nasne-gui` / `cargo build --release -p nasne-cli` です。
`nasne` をパスの通った場所に置くときは `cargo install --path crates/cli` でも入れられます。

## はじめて使うとき

起動すると、同じネットワーク（/24）の中から nasne を自動で探して接続し、その接続先を保存します。GUI とコマンドラインで接続先は共通です。

自動で見つからないときや、nasne が複数あるときは、次のどれかで接続先を指定してください。

- GUI の「nasne情報」タブの「接続先」に IP アドレスを入力して「接続」を押す（次回からはその接続先を使います）
- `nasne host 192.168.1.20` で保存する
- 環境変数 `NASNE_HOST` に IP アドレスを入れて起動する（保存はしません）
- コマンドラインでは `--host 192.168.1.20` でも指定できます

## コマンドラインの使い方

```sh
nasne status                         # HDD の残り・録画中か
nasne epg now -b gr                  # 地デジで放送中の番組
nasne epg list -c NHK総合 -d tomorrow --from 19:00 --to 23:00
nasne epg search 大河ドラマ -b gr     # 番組を探す（複数の語はすべてを含むもの）
nasne epg show gr:30720:12388        # 番組の詳細
nasne resv add gr:30720:12388 -q dr -r weekly   # 録画予約（毎週・DR）
nasne resv list                      # 予約の一覧
nasne rec list -u -s ニュース         # 未視聴の録画をタイトル・説明で絞り込む
nasne rec protect <録画ID>
```

番組 ID（`gr:30720:12388` = 放送波:サービス ID:イベント ID）は、`nasne epg` の一覧の左端の列に出ます。
予約 ID・録画 ID も、それぞれ `nasne resv list` / `nasne rec list` の左端の列です。

| コマンド | 内容 |
|---|---|
| `status` / `info` | 本体の状態 / 本体情報（読み取りのみ） |
| `blink` | 本体のランプを点灯させる |
| `host [IP]` / `host --discover` | 接続先の表示・保存 / LAN 内を探して保存 |
| `channels` | チャンネル一覧 |
| `epg now` / `list` / `search` / `show` | 放送中 / 番組表 / 番組検索 / 番組の詳細 |
| `resv list` / `add` / `edit` / `cancel` | 予約の一覧・追加・変更（画質・繰り返し・優先度）・取り消し |
| `resv failed` / `clear-failed` | 録画できなかった予約の一覧 / 一覧から消す |
| `rec list` / `rename` / `protect` / `unprotect` / `watched` / `unwatched` / `delete` | 録画の一覧と管理 |

- 詳しくは `nasne --help`、`nasne <コマンド> --help` を見てください。
- `--json` を付けると、一覧や詳細を JSON で出力します（日時は `2026-11-10T20:00:00+09:00` の形）。
- 予約の取り消しと録画の削除は確認を求めます。スクリプトから使うときは `-y` を付けてください。
- `-b` は放送波（`gr` = 地デジ / `bs` / `cs`）、`-c` は局（名前の一部・チャンネル番号・サービス ID）です。
- `-d` の日付は `today` / `tomorrow` / `+2` / `11-12` / `2026-11-12` で、番組表の 1 日は 4:00〜翌 4:00 です。

## 方針

このツールは、nasne が LAN 内に公開している HTTP の仕組みのうち、**番組情報と録画・予約の管理に必要な部分だけ** を使います。

- 放送内容そのもの（映像・音声）、著作権保護（DTCP-IP・暗号化されたサムネイルなど）、限定受信（B-CAS）には触れません。
- 初期化・フォーマット・ファームウェア更新などの、元に戻せない設定の API も使いません。設定の変更は nasne HOME で行ってください。

これらの機能を追加する変更は受け付けません。

## 仕組み

- **番組表**: nasne が受信して内部に持っている EPG を取得し、手元で解析します。外部の番組表サービスは使いません。
- **キャッシュ**: 一度取得した番組表はディスクにキャッシュし、GUI とコマンドラインで共有します。使う前に数 KB の版数情報だけを確認し、番組表が更新されたときだけ取り直します。
- **GUI の軽さ**
  - 2 回目以降の起動では、ウィンドウが開いてから約 0.17 秒で番組表が出ます（Apple Silicon の Mac で計測）。最新かどうかの確認はその裏で行います。
  - 3 つの放送波を並列で取得・解析するので、地デジ / BS / CS の切り替えで待たされません。
  - 起動中は 120 秒ごとに版数情報を確認します。
  - 画面に見えている番組だけを描画します。操作していないときは再描画しないので、CPU をほとんど使いません。

### キャッシュと設定の保存場所

| OS | キャッシュ（番組表・局ロゴ） | 設定（接続先） |
|---|---|---|
| macOS | `~/Library/Caches/nasne-tools/<IP>/` | `~/Library/Application Support/nasne-tools/settings.txt` |
| Windows | `%LOCALAPPDATA%\nasne-tools\<IP>\` | `%APPDATA%\nasne-tools\settings.txt` |
| Linux | `$XDG_CACHE_HOME/nasne-tools/<IP>/` | `$XDG_CONFIG_HOME/nasne-tools/settings.txt` |

キャッシュは消してもかまいません。次に使うときに取り直します。アンインストールするときは、実行ファイルとこの 2 か所を消してください。

## 開発者向け

### 構成

Cargo のワークスペースで、nasne を扱う処理はすべて `nasne-core` にまとめ、GUI とコマンドラインは表示と入力だけを受け持ちます。

```
crates/
  core/  nasne-core  … nasne との通信・番組表の解析・操作・キャッシュ（ライブラリ）
  gui/   nasne-gui   … デスクトップアプリ（実行ファイル nasne-gui）
  cli/   nasne-cli   … コマンドライン（実行ファイル nasne）
tools/
  gen_jis.py         … 文字コード変換表の生成
```

| ファイル | 内容 |
|---|---|
| `crates/core/src/client.rs` | nasne との HTTP 通信（GET / 署名付き POST）、LAN 内の nasne の検出 |
| `crates/core/src/ops.rs` | 一覧の取得・予約・録画の変更などの操作（`Nasne` のメソッド） |
| `crates/core/src/model.rs` | 予約・録画などのデータ、画質・繰り返しの表示名 |
| `crates/core/src/info.rs` | 本体情報（読み取りのみ） |
| `crates/core/src/epg.rs` | 番組表と局ロゴの取得・EIT の解析・キャッシュつきの読み込み、放送波（`Band`） |
| `crates/core/src/arib/` | ARIB STD-B24 の文字列を UTF-8 に変換（追加記号の表、JIS X 0213 の変換表を含む） |
| `crates/core/src/gaiji.rs` | nasne の外字と `[字]` などの表示用文字列の変換 |
| `crates/core/src/jst.rs` | 日本時間の日時計算 |
| `crates/core/src/store.rs` | ディスクキャッシュと設定 |
| `crates/gui/src/main.rs` | 起動、ウィンドウ、日本語フォントの読み込み |
| `crates/gui/src/app.rs` | 画面（番組表 / 予約一覧 / 録画一覧 / nasne 情報）。通信はすべて別スレッドで行う |
| `crates/gui/src/theme.rs` | 配色（ライト / ダーク）とジャンルの色 |
| `crates/cli/src/main.rs` | 引数の定義、接続先の決定 |
| `crates/cli/src/programs.rs` | 番組表のコマンド（チャンネル・放送中・番組表・検索・詳細・録画予約） |
| `crates/cli/src/manage.rs` | 本体・予約・録画のコマンド |

`crates/core/src/arib/jis.bin` と `jis_pairs.rs` は生成済みのものをリポジトリに含めているので、ビルドに Python は要りません。
作り直すときは `python3 tools/gen_jis.py` を実行します（Python 標準ライブラリの codec から作ります）。

### 開発用の環境変数

| 変数 | 内容 |
|---|---|
| `NASNE_HOST` | 接続先の IP アドレス（GUI・コマンドライン共通） |
| `NASNE_GUI_BENCH=1` | GUI の起動にかかった時間を表示し、番組表が出たら終了する |
| `NASNE_GUI_VIEW` | GUI の起動時に開くタブ。`reserved`（予約一覧）/ `recorded`（録画一覧）/ `info`（nasne 情報）/ `program`（放送中の番組の詳細） |

## 免責事項

- このツールは非公式のもので、nasne の製造元・販売元とは関係ありません。
- 予約の取り消しや録画の削除など、nasne の中身を変更する操作は、元に戻せないものがあります。利用は自己責任でお願いします。
