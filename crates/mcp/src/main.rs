//! nasne-mcp: nasne の番組表・録画予約・録画管理を AI アシスタントから扱う MCP サーバ
//!
//! 標準入出力で、改行区切りの JSON-RPC 2.0 を受け付ける（MCP の stdio トランスポート）。
//! 標準出力にはプロトコルのメッセージだけを書き、ログは標準エラーに出す。
//! ツールの中身は tools.rs。

mod tools;

use serde_json::{Value, json};
use std::io::{BufRead as _, Write as _};
use std::process::ExitCode;

/// 対応している MCP のプロトコル版（新しい順）。クライアントの版がこの中にあればそれに合わせる
const PROTOCOL_VERSIONS: [&str; 4] = ["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

const INSTRUCTIONS: &str = "\
nasne（ソニーのネットワークレコーダー）の番組表・録画予約・録画を扱います。映像の視聴やダビングはできません。
- 日時はすべて日本時間（+09:00）です。現在時刻は一覧の結果の now にあります。
- 番組 ID は gr:30720:1234（放送波:サービスID:イベントID）の形で、番組表・検索の結果の id にあります。
- nasne のチューナーは 1 つです。時間が重なる予約は、重なった部分をどちらか一方しか録画できません。
- 番組表の 1 日は 4:00〜翌 4:00 です。";

const HELP: &str = "\
nasne-mcp: nasne の番組表・録画予約・録画管理を AI アシスタントから扱う MCP サーバ（stdio）

使い方:
  nasne-mcp [--host IP]

オプション:
  --host IP     接続先の nasne の IP アドレス
  -h, --help    この説明を表示する
  -V, --version 版を表示する

接続先は --host、環境変数 NASNE_HOST、保存済みの接続先（nasne コマンド・nasne-gui と共通）の順に使います。
どれもなければ、最初にツールが呼ばれたときに LAN 内の nasne を探して保存します。

登録の例（Claude Code）:
  claude mcp add nasne -- /path/to/nasne-mcp
";

fn ok(id: &Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn error(id: &Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

/// メッセージ 1 つを処理する。通知と応答には何も返さない
fn handle(server: &mut tools::Server, msg: &Value) -> Option<Value> {
    let Some(obj) = msg.as_object() else { return Some(error(&Value::Null, -32600, "Invalid Request")) };
    let method = obj.get("method").and_then(Value::as_str);
    let id = obj.get("id")?; // id がなければ通知（notifications/initialized・cancelled など）
    let method = method?; // method がなければクライアントからの応答（このサーバは要求を出さない）
    let params = obj.get("params").cloned().unwrap_or(Value::Null);
    Some(match method {
        "initialize" => {
            let asked = params.get("protocolVersion").and_then(Value::as_str).unwrap_or("");
            let version = PROTOCOL_VERSIONS.iter().find(|v| **v == asked).unwrap_or(&PROTOCOL_VERSIONS[0]);
            ok(
                id,
                json!({
                    "protocolVersion": version,
                    "capabilities": { "tools": { "listChanged": false } },
                    "serverInfo": { "name": "nasne", "title": "nasne", "version": env!("CARGO_PKG_VERSION") },
                    "instructions": INSTRUCTIONS,
                }),
            )
        }
        "ping" => ok(id, json!({})),
        "tools/list" => ok(id, json!({ "tools": tools::list() })),
        "tools/call" => {
            let Some(name) = params.get("name").and_then(Value::as_str) else {
                return Some(error(id, -32602, "name がありません"));
            };
            let empty = serde_json::Map::new();
            let args = params.get("arguments").and_then(Value::as_object).unwrap_or(&empty);
            match server.call(name, args) {
                None => error(id, -32602, &format!("Unknown tool: {name}")),
                Some(Ok(v)) => ok(id, json!({ "content": [{ "type": "text", "text": v.to_string() }], "isError": false })),
                Some(Err(e)) => ok(id, json!({ "content": [{ "type": "text", "text": e }], "isError": true })),
            }
        }
        _ => error(id, -32601, &format!("Method not found: {method}")),
    })
}

fn serve(host: Option<String>) -> std::io::Result<()> {
    let mut server = tools::Server::new(host);
    let stdin = std::io::stdin().lock();
    let mut stdout = std::io::stdout().lock();
    for line in stdin.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Value>(&line) {
            Err(_) => Some(error(&Value::Null, -32700, "Parse error")),
            // 古い版（2025-03-26）のバッチにも応じる
            Ok(Value::Array(batch)) => {
                let r: Vec<Value> = batch.iter().filter_map(|m| handle(&mut server, m)).collect();
                (!r.is_empty()).then_some(Value::Array(r))
            }
            Ok(msg) => handle(&mut server, &msg),
        };
        if let Some(r) = reply {
            writeln!(stdout, "{r}")?;
            stdout.flush()?;
        }
    }
    Ok(())
}

fn main() -> ExitCode {
    let mut host = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "-h" | "--help" => {
                print!("{HELP}");
                return ExitCode::SUCCESS;
            }
            "-V" | "--version" => {
                println!("nasne-mcp {}", env!("CARGO_PKG_VERSION"));
                return ExitCode::SUCCESS;
            }
            "--host" => match args.next() {
                Some(h) => host = Some(h),
                None => {
                    eprintln!("エラー: --host には IP アドレスを指定してください");
                    return ExitCode::from(2);
                }
            },
            _ if a.starts_with("--host=") => host = Some(a["--host=".len()..].to_string()),
            _ => {
                eprintln!("エラー: 知らない引数です: {a}（nasne-mcp --help で使い方を見られます）");
                return ExitCode::from(2);
            }
        }
    }
    match serve(host.filter(|h| !h.trim().is_empty())) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("エラー: {e}");
            ExitCode::FAILURE
        }
    }
}
