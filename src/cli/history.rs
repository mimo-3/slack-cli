//! `slack-cli history` — チャンネル / スレッドのメッセージ履歴。
//!
//! チャンネル名の解決・時刻整形・ユーザー名解決は `unread` からも使うため、
//! このモジュールに `pub(crate)` で置いてある。

use std::collections::HashMap;

use chrono::{DateTime, Local, NaiveDate, NaiveDateTime, TimeZone, Utc};
use clap::Args;
use serde_json::{json, Map, Value};

use crate::cli::common::{is_message_ts, resolve_channel_id};
use crate::cli::{parse_positive_int, GlobalOpts};
use crate::client::pagination::PaginationOpts;
use crate::client::SlackClient;
use crate::error::SlackCliError;
use crate::output::{self, OutputFormat};

/// `--number` の既定値と許容範囲。範囲は 1 箇所にまとめる（移植方針 A5）。
pub const DEFAULT_MESSAGE_COUNT: u32 = 10;
pub const MAX_MESSAGE_COUNT: u32 = 1000;

/// チャンネル一覧を引くときの種別。
pub(crate) const CHANNEL_TYPES: &str = "public_channel,private_channel,im,mpim";
/// 解決できない Slack ts の表示（移植方針 A4 / D6）。
pub(crate) const INVALID_TIMESTAMP: &str = "(invalid timestamp)";
/// 本文が空のメッセージの表示。
pub(crate) const NO_TEXT: &str = "(no text)";

pub const ERR_NUMBER_NOT_INT: &str = "--number must be a positive integer";
pub const ERR_NUMBER_RANGE: &str = "Message count must be between 1 and 1000";
pub const ERR_THREAD_TS: &str = "Invalid thread timestamp format";
pub const ERR_SINCE: &str = "Invalid date format. Use YYYY-MM-DD HH:MM:SS";
pub const WARN_NUMBER_IGNORED: &str = "Warning: --number is ignored when --thread is specified.";
pub const WARN_SINCE_IGNORED: &str = "Warning: --since is ignored when --thread is specified.";


#[derive(Args, Debug)]
pub struct HistoryCommand {
    /// Target channel name or ID
    #[arg(short, long, required = true, value_name = "CHANNEL")]
    pub channel: String,

    /// Number of messages to retrieve
    #[arg(short, long, value_name = "NUMBER")]
    pub number: Option<String>,

    /// Get messages since specific date (YYYY-MM-DD HH:MM:SS, interpreted in local time)
    #[arg(long, value_name = "DATE")]
    pub since: Option<String>,

    /// Thread timestamp to retrieve complete thread conversation
    #[arg(short, long, value_name = "THREAD")]
    pub thread: Option<String>,

    /// Include permalink URL for each message
    #[arg(long)]
    pub with_link: bool,
}

/// 検証を通したあとの実効オプション。
#[derive(Debug, PartialEq, Eq)]
struct HistoryOptions {
    thread: Option<String>,
    limit: u32,
    oldest: Option<String>,
}

impl HistoryOptions {
    /// 引数を検証して実効値に畳む。`--thread` 指定時は `--number` / `--since` を
    /// 検証ごと読み飛ばし、無視した旨を stderr に出す。
    fn from_command(cmd: &HistoryCommand) -> Result<Self, SlackCliError> {
        if let Some(thread) = cmd.thread.as_deref() {
            if !is_message_ts(thread) {
                return Err(SlackCliError::Validation(ERR_THREAD_TS.to_string()));
            }
            if cmd.number.is_some() {
                eprintln!("{WARN_NUMBER_IGNORED}");
            }
            if cmd.since.is_some() {
                eprintln!("{WARN_SINCE_IGNORED}");
            }
            return Ok(Self {
                thread: Some(thread.to_string()),
                limit: DEFAULT_MESSAGE_COUNT,
                oldest: None,
            });
        }

        let limit = match cmd.number.as_deref() {
            None => DEFAULT_MESSAGE_COUNT,
            Some(raw) => {
                let parsed = parse_positive_int(raw, ERR_NUMBER_NOT_INT)?;
                if parsed > MAX_MESSAGE_COUNT {
                    return Err(SlackCliError::Validation(ERR_NUMBER_RANGE.to_string()));
                }
                parsed
            }
        };

        let oldest = cmd
            .since
            .as_deref()
            .map(|raw| parse_local_datetime(raw, ERR_SINCE).map(|secs| secs.to_string()))
            .transpose()?;

        Ok(Self {
            thread: None,
            limit,
            oldest,
        })
    }
}

pub async fn run(
    cmd: HistoryCommand,
    client: &SlackClient,
    global: &GlobalOpts,
) -> Result<(), SlackCliError> {
    let options = HistoryOptions::from_command(&cmd)?;
    let channel_id = resolve_channel_id(client, &cmd.channel).await?;

    let raw = fetch_messages(client, &channel_id, &options).await?;
    let users = fetch_usernames(client, &collect_user_ids(&raw)).await;
    let permalinks = fetch_permalinks(client, &channel_id, &raw, cmd.with_link).await;

    let messages: Vec<Value> = raw
        .iter()
        .map(|message| message_value(message, &users, &permalinks))
        .collect();

    let format = global.output_format();
    let value = build_output(&cmd.channel, &messages, format);
    output::format_value(&value, format, &mut std::io::stdout())?;
    Ok(())
}

/// 履歴本体の取得。スレッド指定時のみ全ページ辿る。
///
/// 通常モードは 1 リクエストのみ（移植方針 G5 の `history` 例外。
/// `--number` の上限 1000 が `conversations.history` の `limit` 上限と一致する）。
async fn fetch_messages(
    client: &SlackClient,
    channel_id: &str,
    options: &HistoryOptions,
) -> Result<Vec<Value>, SlackCliError> {
    if let Some(thread) = &options.thread {
        // スレッドは API の返り順（古い順）をそのまま使う
        return client
            .paginate_get(
                "conversations.replies",
                &[("channel", channel_id), ("ts", thread)],
                "messages",
                &PaginationOpts::all(),
            )
            .await;
    }

    let limit = options.limit.to_string();
    let mut params: Vec<(&str, &str)> = vec![("channel", channel_id), ("limit", &limit)];
    if let Some(oldest) = &options.oldest {
        params.push(("oldest", oldest));
    }

    let response = client.get("conversations.history", &params).await?;
    let mut messages = response
        .get("messages")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    // conversations.history は新しい順に返すので、表示は古い順に直す
    messages.reverse();
    Ok(messages)
}

/// permalink の取得。1 件でも失敗したら諦めるのではなく、取れたものだけ使う。
async fn fetch_permalinks(
    client: &SlackClient,
    channel_id: &str,
    messages: &[Value],
    with_link: bool,
) -> HashMap<String, String> {
    let mut permalinks = HashMap::new();
    if !with_link {
        return permalinks;
    }

    for message in messages {
        let Some(ts) = message.get("ts").and_then(Value::as_str) else {
            continue;
        };
        let response = client
            .get(
                "chat.getPermalink",
                &[("channel", channel_id), ("message_ts", ts)],
            )
            .await;
        if let Ok(body) = response {
            if let Some(link) = body.get("permalink").and_then(Value::as_str) {
                permalinks.insert(ts.to_string(), link.to_string());
            }
        }
    }
    permalinks
}

/// 出力値の組み立て。
///
/// JSON / YAML は TypeScript 版の契約（`{channel, messages, total}`）をそのまま出す。
/// 表形式は 1 メッセージ 1 行でないと読めないため、平坦化した配列を渡す。
fn build_output(channel: &str, messages: &[Value], format: OutputFormat) -> Value {
    match format {
        OutputFormat::Json | OutputFormat::Yaml => json!({
            "channel": channel,
            "messages": messages,
            "total": messages.len(),
        }),
        _ => Value::Array(messages.iter().map(tabular_row).collect()),
    }
}

/// 表・CSV 向けの平坦な行。ファイルは名前だけをカンマで繋ぐ。
fn tabular_row(message: &Value) -> Value {
    let mut row = Map::new();
    insert_str(&mut row, "timestamp", message.get("timestamp"));
    insert_str(&mut row, "user", message.get("user"));
    insert_str(&mut row, "text", message.get("text"));
    insert_str(&mut row, "ts", message.get("ts"));

    if let Some(files) = message.get("files").and_then(Value::as_array) {
        let names: Vec<&str> = files
            .iter()
            .filter_map(|f| f.get("name").and_then(Value::as_str))
            .collect();
        row.insert("files".to_string(), json!(names.join(", ")));
    }
    if let Some(link) = message.get("permalink") {
        row.insert("permalink".to_string(), link.clone());
    }
    Value::Object(row)
}

fn insert_str(row: &mut Map<String, Value>, key: &str, value: Option<&Value>) {
    row.insert(
        key.to_string(),
        value
            .cloned()
            .unwrap_or_else(|| Value::String(String::new())),
    );
}

/// 1 メッセージ分の JSON。キーの出現規則は TypeScript 版に合わせる。
/// `text` はメンション未置換の生テキスト（移植方針 G21）。
fn message_value(
    message: &Value,
    users: &HashMap<String, String>,
    permalinks: &HashMap<String, String>,
) -> Value {
    let ts = message
        .get("ts")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let mut out = Map::new();
    out.insert("ts".to_string(), json!(ts));
    out.insert("timestamp".to_string(), json!(format_timestamp(ts)));
    out.insert("user".to_string(), json!(resolve_username(message, users)));

    if let Some(user_id) = message.get("user").and_then(Value::as_str) {
        out.insert("user_id".to_string(), json!(user_id));
    }

    let text = message
        .get("text")
        .and_then(Value::as_str)
        .filter(|t| !t.is_empty())
        .unwrap_or(NO_TEXT);
    out.insert("text".to_string(), json!(text));

    for key in ["thread_ts", "reply_count"] {
        if let Some(value) = message.get(key).filter(|v| !v.is_null()) {
            out.insert(key.to_string(), value.clone());
        }
    }

    let files = file_values(message);
    if !files.is_empty() {
        out.insert("files".to_string(), Value::Array(files));
    }
    if let Some(link) = permalinks.get(ts) {
        out.insert("permalink".to_string(), json!(link));
    }

    Value::Object(out)
}

fn file_values(message: &Value) -> Vec<Value> {
    let Some(files) = message.get("files").and_then(Value::as_array) else {
        return Vec::new();
    };

    files
        .iter()
        .map(|file| {
            let url = ["url_private_download", "url_private", "permalink"]
                .iter()
                .find_map(|key| file.get(*key).and_then(Value::as_str));
            json!({
                "id": file.get("id").cloned().unwrap_or(Value::Null),
                "name": file.get("name").cloned().unwrap_or(Value::Null),
                "mimetype": file.get("mimetype").cloned().unwrap_or(Value::Null),
                "filetype": file.get("filetype").cloned().unwrap_or(Value::Null),
                "size": file.get("size").cloned().unwrap_or(Value::Null),
                "url": url,
            })
        })
        .collect()
}

/// 表示ユーザー名。`unread` もこの規則に統一する（移植方針 G13）。
pub(crate) fn resolve_username(message: &Value, users: &HashMap<String, String>) -> String {
    if let Some(user_id) = message.get("user").and_then(Value::as_str) {
        return users
            .get(user_id)
            .cloned()
            .unwrap_or_else(|| "Unknown User".to_string());
    }
    if message.get("bot_id").and_then(Value::as_str).is_some() {
        return "Bot".to_string();
    }
    "Unknown".to_string()
}

/// メッセージ本文の `<@Uxxxx>` は表示上も置換しないため、著者 ID だけを集める。
fn collect_user_ids(messages: &[Value]) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    for message in messages {
        if let Some(id) = message.get("user").and_then(Value::as_str) {
            if !ids.iter().any(|known| known == id) {
                ids.push(id.to_string());
            }
        }
    }
    ids
}

/// `users.info` を 1 件ずつ引く。失敗した ID は単に解決できなかったものとして扱う。
pub(crate) async fn fetch_usernames(
    client: &SlackClient,
    ids: &[String],
) -> HashMap<String, String> {
    let mut users = HashMap::new();
    for id in ids {
        if let Ok(response) = client.get("users.info", &[("user", id.as_str())]).await {
            if let Some(name) = user_display_name(response.get("user").unwrap_or(&Value::Null)) {
                users.insert(id.clone(), name);
            }
        }
    }
    users
}

fn user_display_name(user: &Value) -> Option<String> {
    let profile = user.get("profile");
    [
        user.get("name"),
        profile.and_then(|p| p.get("display_name")),
        profile.and_then(|p| p.get("real_name")),
    ]
    .into_iter()
    .flatten()
    .filter_map(Value::as_str)
    .find(|name| !name.is_empty())
    .map(str::to_string)
}

/// Slack の ts を UTC の `YYYY-MM-DD HH:MM:SS` にする（移植方針 D1）。
pub(crate) fn format_timestamp(ts: &str) -> String {
    let Ok(seconds) = ts.trim().parse::<f64>() else {
        return INVALID_TIMESTAMP.to_string();
    };
    if !seconds.is_finite() {
        return INVALID_TIMESTAMP.to_string();
    }

    match Utc.timestamp_opt(seconds.trunc() as i64, 0).single() {
        Some(datetime) => datetime.format("%Y-%m-%d %H:%M:%S").to_string(),
        None => INVALID_TIMESTAMP.to_string(),
    }
}

/// 日時入力のパース（移植方針 D3 / D4）。
/// 受理する形式を明示列挙し、タイムゾーンを書かない入力はローカル時刻として解釈する。
pub(crate) fn parse_local_datetime(raw: &str, message: &str) -> Result<i64, SlackCliError> {
    let value = raw.trim();
    let invalid = || SlackCliError::Validation(message.to_string());

    if !value.is_empty() && value.chars().all(|c| c.is_ascii_digit()) {
        return value.parse::<i64>().map_err(|_| invalid());
    }

    if let Ok(datetime) = DateTime::parse_from_rfc3339(value) {
        return Ok(datetime.timestamp());
    }

    for format in [
        "%Y-%m-%dT%H:%M:%S",
        "%Y-%m-%dT%H:%M",
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%d %H:%M",
    ] {
        if let Ok(naive) = NaiveDateTime::parse_from_str(value, format) {
            return to_local_timestamp(naive, message);
        }
    }

    if let Ok(date) = NaiveDate::parse_from_str(value, "%Y-%m-%d") {
        let naive = date.and_hms_opt(0, 0, 0).ok_or_else(invalid)?;
        return to_local_timestamp(naive, message);
    }

    Err(invalid())
}

fn to_local_timestamp(naive: NaiveDateTime, message: &str) -> Result<i64, SlackCliError> {
    Local
        .from_local_datetime(&naive)
        .earliest()
        .map(|datetime| datetime.timestamp())
        .ok_or_else(|| SlackCliError::Validation(message.to_string()))
}

#[cfg(test)]
mod tests {
    use clap::error::ErrorKind;
    use clap::Parser;
    use serde_json::json;
    use url::Url;
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;
    use crate::cli::Cli;

    fn client_for(server: &MockServer) -> SlackClient {
        SlackClient::new("test-token-value")
            .unwrap()
            .with_base_url(Url::parse(&format!("{}/", server.uri())).unwrap())
    }

    fn command(channel: &str) -> HistoryCommand {
        HistoryCommand {
            channel: channel.to_string(),
            number: None,
            since: None,
            thread: None,
            with_link: false,
        }
    }

    async fn mount_history(server: &MockServer, body: Value) {
        Mock::given(method("GET"))
            .and(path("/conversations.history"))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .mount(server)
            .await;
    }

    async fn mount_user(server: &MockServer, id: &str, name: &str) {
        Mock::given(method("GET"))
            .and(path("/users.info"))
            .and(query_param("user", id))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "user": { "id": id, "name": name },
            })))
            .mount(server)
            .await;
    }

    #[test]
    fn parses_the_full_invocation() {
        let cli = Cli::try_parse_from([
            "slack-cli",
            "history",
            "-c",
            "general",
            "-n",
            "20",
            "--since",
            "2026-08-01",
            "-t",
            "1700000000.000100",
            "--with-link",
        ])
        .unwrap();

        let crate::cli::Command::History(cmd) = cli.command else {
            panic!("expected the history command");
        };
        assert_eq!(cmd.number.as_deref(), Some("20"));
        assert!(cmd.with_link);
    }

    #[test]
    fn with_link_defaults_to_false() {
        let cli = Cli::try_parse_from(["slack-cli", "history", "-c", "general"]).unwrap();
        let crate::cli::Command::History(cmd) = cli.command else {
            panic!("expected the history command");
        };
        assert!(!cmd.with_link);
        assert!(cmd.number.is_none());
    }

    #[test]
    fn channel_is_required() {
        let err = Cli::try_parse_from(["slack-cli", "history"]).expect_err("--channel is required");
        assert_eq!(err.kind(), ErrorKind::MissingRequiredArgument);
    }

    #[test]
    fn number_is_parsed_strictly_and_range_checked() {
        let mut cmd = command("C0123ABCD");
        cmd.number = Some("12abc".into());
        assert_eq!(
            HistoryOptions::from_command(&cmd).unwrap_err().to_string(),
            ERR_NUMBER_NOT_INT
        );

        cmd.number = Some("1001".into());
        assert_eq!(
            HistoryOptions::from_command(&cmd).unwrap_err().to_string(),
            ERR_NUMBER_RANGE
        );

        cmd.number = Some(" 25 ".into());
        assert_eq!(HistoryOptions::from_command(&cmd).unwrap().limit, 25);

        cmd.number = None;
        assert_eq!(
            HistoryOptions::from_command(&cmd).unwrap().limit,
            DEFAULT_MESSAGE_COUNT
        );
    }

    #[test]
    fn thread_timestamp_format_is_validated_and_wins_over_number_and_since() {
        let mut cmd = command("C0123ABCD");
        cmd.thread = Some("17000000.1".into());
        assert_eq!(
            HistoryOptions::from_command(&cmd).unwrap_err().to_string(),
            ERR_THREAD_TS
        );

        cmd.thread = Some("1700000000.000100".into());
        cmd.number = Some("not a number".into());
        cmd.since = Some("nonsense".into());
        let options = HistoryOptions::from_command(&cmd).unwrap();
        assert_eq!(options.thread.as_deref(), Some("1700000000.000100"));
        assert!(options.oldest.is_none());
    }

    #[test]
    fn since_accepts_the_documented_formats_only() {
        // 全桁数字は Unix 秒としてそのまま通す
        assert_eq!(
            parse_local_datetime("1700000000", ERR_SINCE).unwrap(),
            1_700_000_000
        );
        // オフセット付きは書かれたとおりに解釈する
        assert_eq!(
            parse_local_datetime("2024-01-01T00:00:00+09:00", ERR_SINCE).unwrap(),
            1_704_034_800
        );
        // TZ なしはローカル解釈。日付のみと日時で同じ規則になっていること（D4）
        let date_only = parse_local_datetime("2024-01-01", ERR_SINCE).unwrap();
        let with_time = parse_local_datetime("2024-01-01 00:00:00", ERR_SINCE).unwrap();
        assert_eq!(date_only, with_time);

        for bad in ["Jan 1, 2024", "2024/01/01", "", "abc"] {
            assert_eq!(
                parse_local_datetime(bad, ERR_SINCE)
                    .unwrap_err()
                    .to_string(),
                ERR_SINCE,
                "{bad:?} should have been rejected"
            );
        }
    }

    #[test]
    fn timestamps_are_utc_and_never_render_nan() {
        assert_eq!(format_timestamp("1700000000.000100"), "2023-11-14 22:13:20");
        for bad in ["abc", "", "12abc", "1e999"] {
            assert_eq!(format_timestamp(bad), INVALID_TIMESTAMP, "{bad:?}");
        }
    }
    #[test]
    fn unknown_authors_follow_the_history_rules() {
        let users = HashMap::from([("U1".to_string(), "alice".to_string())]);
        assert_eq!(resolve_username(&json!({ "user": "U1" }), &users), "alice");
        assert_eq!(
            resolve_username(&json!({ "user": "U9" }), &users),
            "Unknown User"
        );
        assert_eq!(resolve_username(&json!({ "bot_id": "B1" }), &users), "Bot");
        assert_eq!(resolve_username(&json!({}), &users), "Unknown");
    }
    #[test]
    fn json_output_keeps_the_typescript_contract_and_tables_get_flat_rows() {
        let messages = vec![message_value(
            &json!({ "ts": "1700000000.000100", "user": "U1", "text": "hi" }),
            &HashMap::from([("U1".to_string(), "alice".to_string())]),
            &HashMap::new(),
        )];

        let json_value = build_output("general", &messages, OutputFormat::Json);
        assert_eq!(json_value["channel"], "general");
        assert_eq!(json_value["total"], 1);
        assert_eq!(json_value["messages"][0]["user_id"], "U1");

        let table_value = build_output("general", &messages, OutputFormat::Table);
        let rows = table_value.as_array().expect("tables need a row array");
        assert_eq!(rows[0]["user"], "alice");
        assert_eq!(rows[0]["timestamp"], "2023-11-14 22:13:20");
        assert!(rows[0].get("user_id").is_none());
    }

    #[test]
    fn empty_text_and_optional_keys_follow_the_spec() {
        let value = message_value(
            &json!({
                "ts": "1700000000.000100",
                "bot_id": "B1",
                "thread_ts": "1700000000.000100",
                "reply_count": 3,
                "files": [{ "id": "F1", "name": "report.pdf", "url_private": "https://files/x" }],
            }),
            &HashMap::new(),
            &HashMap::new(),
        );

        assert_eq!(value["text"], NO_TEXT);
        assert_eq!(value["user"], "Bot");
        assert!(value.get("user_id").is_none());
        assert_eq!(value["reply_count"], 3);
        assert_eq!(value["files"][0]["url"], "https://files/x");
    }

    #[tokio::test]
    async fn history_is_fetched_once_and_reversed_into_chronological_order() {
        let server = MockServer::start().await;
        mount_history(
            &server,
            json!({
                "ok": true,
                "messages": [
                    { "ts": "1700000002.000000", "user": "U1", "text": "second" },
                    { "ts": "1700000001.000000", "user": "U1", "text": "first" },
                ],
            }),
        )
        .await;

        let options = HistoryOptions {
            thread: None,
            limit: 10,
            oldest: None,
        };
        let messages = fetch_messages(&client_for(&server), "C0123ABCD", &options)
            .await
            .unwrap();

        assert_eq!(messages[0]["text"], "first");
        assert_eq!(messages[1]["text"], "second");
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn thread_mode_walks_every_page_of_replies() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/conversations.replies"))
            .and(query_param("cursor", "page2"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "messages": [{ "ts": "1700000002.000000", "text": "reply" }],
                "response_metadata": { "next_cursor": "" },
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/conversations.replies"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "messages": [{ "ts": "1700000001.000000", "text": "root" }],
                "response_metadata": { "next_cursor": "page2" },
            })))
            .mount(&server)
            .await;

        let options = HistoryOptions {
            thread: Some("1700000001.000000".into()),
            limit: 10,
            oldest: None,
        };
        let messages = fetch_messages(&client_for(&server), "C0123ABCD", &options)
            .await
            .unwrap();

        // スレッドは API の順序のまま
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0]["text"], "root");
    }

    #[tokio::test]
    async fn since_is_sent_as_the_oldest_parameter() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/conversations.history"))
            .and(query_param("oldest", "1700000000"))
            .and(query_param("limit", "5"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({ "ok": true, "messages": [] })),
            )
            .expect(1)
            .mount(&server)
            .await;

        let options = HistoryOptions {
            thread: None,
            limit: 5,
            oldest: Some("1700000000".into()),
        };
        fetch_messages(&client_for(&server), "C0123ABCD", &options)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn channel_names_are_resolved_and_unknown_names_suggest_candidates() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/conversations.list"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "channels": [
                    { "id": "C0123ABCD", "name": "general" },
                    { "id": "C0999ZZZZ", "name": "general-random" },
                ],
                "response_metadata": { "next_cursor": "" },
            })))
            .mount(&server)
            .await;

        let client = client_for(&server);
        assert_eq!(
            resolve_channel_id(&client, "#general").await.unwrap(),
            "C0123ABCD"
        );
        // ID 形式は API を呼ばずに素通し
        assert_eq!(
            resolve_channel_id(&client, "C0123ABCD").await.unwrap(),
            "C0123ABCD"
        );

        let err = resolve_channel_id(&client, "genera").await.unwrap_err();
        assert_eq!(
            err.to_string(),
            "Channel 'genera' not found. Did you mean one of these? general, general-random"
        );
    }

    #[tokio::test]
    async fn api_errors_are_propagated_with_the_slack_error_code() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/conversations.history"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": false,
                "error": "channel_not_found",
            })))
            .mount(&server)
            .await;

        let global = GlobalOpts {
            json: true,
            ..GlobalOpts::default()
        };
        let err = run(command("C0123ABCD"), &client_for(&server), &global)
            .await
            .unwrap_err();
        assert!(matches!(err, SlackCliError::Api { .. }), "{err}");
        assert_eq!(err.to_string(), "API Error: channel_not_found");
    }

    #[tokio::test]
    async fn permalink_failures_degrade_instead_of_failing_the_command() {
        let server = MockServer::start().await;
        mount_history(
            &server,
            json!({
                "ok": true,
                "messages": [
                    { "ts": "1700000002.000000", "user": "U1", "text": "second" },
                    { "ts": "1700000001.000000", "user": "U1", "text": "first" },
                ],
            }),
        )
        .await;
        mount_user(&server, "U1", "alice").await;
        Mock::given(method("GET"))
            .and(path("/chat.getPermalink"))
            .and(query_param("message_ts", "1700000001.000000"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "permalink": "https://acme.slack.com/archives/C1/p1700000001000000",
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/chat.getPermalink"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({ "ok": false, "error": "message_not_found" })),
            )
            .mount(&server)
            .await;

        let mut cmd = command("C0123ABCD");
        cmd.with_link = true;
        let global = GlobalOpts {
            json: true,
            ..GlobalOpts::default()
        };
        run(cmd, &client_for(&server), &global).await.unwrap();

        // users.info は著者 1 人分だけ。permalink は 2 件呼んで 1 件成功
        let calls = server.received_requests().await.unwrap();
        let user_calls = calls
            .iter()
            .filter(|r| r.url.path().ends_with("users.info"))
            .count();
        assert_eq!(user_calls, 1);
    }
}
