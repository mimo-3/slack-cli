//! `slack-cli unread` — 未読メッセージの一覧と既読化。
//!
//! チャンネル解決・時刻整形・ユーザー名解決は `history` と共有する。
//! 表示規則も `history` に揃えてある（移植方針 D1 / G13）。

use chrono::Utc;
use clap::Args;
use serde_json::{json, Value};

use crate::cli::common::resolve_channel_id;
use crate::cli::history::{fetch_usernames, format_timestamp, CHANNEL_TYPES, NO_TEXT};
use crate::cli::{parse_positive_int, GlobalOpts};
use crate::client::pagination::PaginationOpts;
use crate::client::SlackClient;
use crate::error::SlackCliError;
use crate::output::{self, OutputFormat};

/// 全チャンネル表示時の既定件数（TypeScript 版 `DEFAULTS.UNREAD_DISPLAY_LIMIT`）。
pub const DEFAULT_DISPLAY_LIMIT: &str = "50";
/// 単一チャンネルで本文をプレビューする件数（TypeScript 版 `UNREAD_MESSAGE_PREVIEW_LIMIT`）。
/// `--limit` はここには効かない（移植方針 G22）。
pub const MESSAGE_PREVIEW_LIMIT: usize = 50;

pub const ERR_LIMIT: &str = "--limit must be a positive integer";
pub const MSG_NO_UNREAD: &str = "✓ No unread messages";

const HISTORY_PAGE_SIZE: u32 = 200;
const SEARCH_PAGE_SIZE: u32 = 100;
const UNREAD_QUERY: &str = "is:unread";

#[derive(Args, Debug)]
pub struct UnreadCommand {
    /// Show unread for a specific channel
    #[arg(short, long, value_name = "CHANNEL")]
    pub channel: Option<String>,

    /// Show only unread counts
    #[arg(long)]
    pub count_only: bool,

    /// Maximum number of channels to display (all-channels view only)
    #[arg(long, default_value = DEFAULT_DISPLAY_LIMIT, value_name = "NUMBER")]
    pub limit: String,

    /// Mark messages as read after fetching (marks every unread channel, ignoring --limit)
    #[arg(long)]
    pub mark_read: bool,
}

/// 未読のあるチャンネル 1 件。
///
/// `last_message_ts` は「最後に既読にした位置」ではなく「最新メッセージの ts」。
/// TypeScript 版は `last_read` に詰めていたが、意味が違うので名前を分ける（移植方針 J7）。
#[derive(Debug, Clone, PartialEq, Eq)]
struct UnreadChannel {
    id: String,
    name: String,
    unread_count: u64,
    last_message_ts: Option<String>,
}

impl UnreadChannel {
    fn to_value(&self) -> Value {
        json!({
            "channel": self.name,
            "channelId": self.id,
            "unreadCount": self.unread_count,
        })
    }
}

pub async fn run(
    cmd: UnreadCommand,
    client: &SlackClient,
    global: &GlobalOpts,
) -> Result<(), SlackCliError> {
    let limit = parse_positive_int(&cmd.limit, ERR_LIMIT)? as usize;
    let format = global.output_format();

    match cmd.channel.as_deref() {
        Some(channel) => run_single_channel(client, &cmd, channel, format).await,
        None => run_all_channels(client, &cmd, limit, format).await,
    }
}

/// 単一チャンネルモード。未読の総数を数えるため履歴は全ページ辿る。
async fn run_single_channel(
    client: &SlackClient,
    cmd: &UnreadCommand,
    channel: &str,
    format: OutputFormat,
) -> Result<(), SlackCliError> {
    let channel_id = resolve_channel_id(client, channel).await?;
    let info = client
        .get("conversations.info", &[("channel", channel_id.as_str())])
        .await?;
    let info_channel = info.get("channel").cloned().unwrap_or(Value::Null);
    let display_name = display_name_for(client, &info_channel).await;

    let last_read = info_channel
        .get("last_read")
        .and_then(Value::as_str)
        .map(str::to_string);
    let mut params: Vec<(&str, &str)> = vec![("channel", channel_id.as_str())];
    if let Some(oldest) = &last_read {
        params.push(("oldest", oldest));
    }

    let unread = client
        .paginate_get(
            "conversations.history",
            &params,
            "messages",
            &PaginationOpts {
                page_size: Some(HISTORY_PAGE_SIZE),
                fetch_all: true,
                ..PaginationOpts::default()
            },
        )
        .await?;

    let total = unread.len();
    let preview = &unread[..total.min(MESSAGE_PREVIEW_LIMIT)];
    let users = fetch_usernames(client, &author_ids(preview)).await;
    let messages: Vec<Value> = preview
        .iter()
        .map(|message| {
            json!({
                "timestamp": format_timestamp(message.get("ts").and_then(Value::as_str).unwrap_or_default()),
                "author": crate::cli::history::resolve_username(message, &users),
                "text": message
                    .get("text")
                    .and_then(Value::as_str)
                    .filter(|text| !text.is_empty())
                    .unwrap_or(NO_TEXT),
            })
        })
        .collect();

    if !matches!(format, OutputFormat::Json | OutputFormat::Yaml) {
        eprintln!("{display_name}: {total} unread messages");
    }
    let value = build_single_output(
        &display_name,
        &channel_id,
        total,
        &messages,
        cmd.count_only,
        format,
    );
    output::format_value(&value, format, &mut std::io::stdout())?;

    if cmd.mark_read {
        mark_read(client, &channel_id).await?;
        eprintln!("✓ Marked messages in {display_name} as read");
    }
    Ok(())
}

/// 単一チャンネルの出力値。
///
/// `--count-only` は「本文を出さない」フラグとして扱い、`--format` の選択は尊重する
/// （移植方針 G15）。
fn build_single_output(
    display_name: &str,
    channel_id: &str,
    total: usize,
    messages: &[Value],
    count_only: bool,
    format: OutputFormat,
) -> Value {
    let summary = json!({
        "channel": display_name,
        "channelId": channel_id,
        "unreadCount": total,
    });

    match format {
        OutputFormat::Json | OutputFormat::Yaml => {
            let mut value = summary;
            if !count_only {
                value["messages"] = Value::Array(messages.to_vec());
                value["displayedMessageCount"] = json!(messages.len());
                value["isTruncated"] = json!(messages.len() < total);
            }
            value
        }
        // 表形式は 1 メッセージ 1 行。件数だけのときは要約 1 行を出す。
        _ if count_only => summary,
        _ => Value::Array(messages.to_vec()),
    }
}

/// 全チャンネルモード。`search.messages` 経路を試し、失敗したら
/// `users.conversations` 経路に丸ごと落とす。
async fn run_all_channels(
    client: &SlackClient,
    cmd: &UnreadCommand,
    limit: usize,
    format: OutputFormat,
) -> Result<(), SlackCliError> {
    let channels = match search_unread_channels(client).await {
        Ok(channels) => channels,
        Err(_) => list_unread_conversations(client).await?,
    };

    if channels.is_empty() {
        eprintln!("{MSG_NO_UNREAD}");
    }

    let displayed: Vec<Value> = channels
        .iter()
        .take(limit)
        .map(UnreadChannel::to_value)
        .collect();
    let total: u64 = channels
        .iter()
        .take(limit)
        .map(|channel| channel.unread_count)
        .sum();

    let value = match (cmd.count_only, format) {
        (true, OutputFormat::Json | OutputFormat::Yaml) => json!({
            "channels": displayed,
            "total": total,
        }),
        (true, _) => {
            eprintln!("Total: {total} unread messages");
            Value::Array(displayed)
        }
        (false, _) => Value::Array(displayed),
    };
    output::format_value(&value, format, &mut std::io::stdout())?;

    if cmd.mark_read {
        // --limit で表示を絞っても既読化は全チャンネルが対象（移植方針 G22）
        for channel in &channels {
            mark_read(client, &channel.id).await?;
        }
        if !channels.is_empty() {
            eprintln!("✓ Marked all messages as read");
        }
    }
    Ok(())
}

/// 経路 1: `search.messages` の `is:unread` をチャンネルごとに集計する。
async fn search_unread_channels(client: &SlackClient) -> Result<Vec<UnreadChannel>, SlackCliError> {
    let matches = client
        .paginate_get(
            "search.messages",
            &[
                ("query", UNREAD_QUERY),
                ("sort", "timestamp"),
                ("sort_dir", "desc"),
            ],
            "messages.matches",
            &PaginationOpts {
                page_size: Some(SEARCH_PAGE_SIZE),
                fetch_all: true,
                ..PaginationOpts::default()
            },
        )
        .await?;

    let mut channels: Vec<UnreadChannel> = Vec::new();
    for hit in &matches {
        let channel = hit.get("channel");
        let Some(id) = channel.and_then(|c| c.get("id")).and_then(Value::as_str) else {
            continue;
        };
        let ts = hit.get("ts").and_then(Value::as_str).map(str::to_string);

        match channels.iter_mut().find(|known| known.id == id) {
            Some(known) => {
                known.unread_count += 1;
                if is_newer(&ts, &known.last_message_ts) {
                    known.last_message_ts = ts;
                }
            }
            None => channels.push(UnreadChannel {
                id: id.to_string(),
                name: channel.map(plain_display_name).unwrap_or_default(),
                unread_count: 1,
                last_message_ts: ts,
            }),
        }
    }

    // 名前が取れなかったチャンネルだけ補完する
    for channel in &mut channels {
        if channel.name.is_empty() {
            channel.name = fetch_display_name(client, &channel.id).await;
        }
    }

    channels.sort_by(|a, b| {
        timestamp_key(&b.last_message_ts).total_cmp(&timestamp_key(&a.last_message_ts))
    });
    Ok(channels)
}

/// 経路 2: `users.conversations` から未読数を拾う。
async fn list_unread_conversations(
    client: &SlackClient,
) -> Result<Vec<UnreadChannel>, SlackCliError> {
    let conversations = client
        .paginate_get(
            "users.conversations",
            &[("types", CHANNEL_TYPES), ("exclude_archived", "true")],
            "channels",
            &PaginationOpts {
                page_size: Some(HISTORY_PAGE_SIZE),
                fetch_all: true,
                ..PaginationOpts::default()
            },
        )
        .await?;

    let mut channels = Vec::new();
    for conversation in &conversations {
        let Some(id) = conversation.get("id").and_then(Value::as_str) else {
            continue;
        };

        // 未読数を返さないチャンネルだけ conversations.info で補う
        let mut detail = conversation.clone();
        if detail.get("unread_count").is_none() {
            if let Ok(info) = client.get("conversations.info", &[("channel", id)]).await {
                if let Some(channel) = info.get("channel") {
                    detail = channel.clone();
                }
            }
        }

        let unread_count = detail
            .get("unread_count")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        if unread_count == 0 {
            continue;
        }

        channels.push(UnreadChannel {
            id: id.to_string(),
            name: display_name_for(client, &detail).await,
            unread_count,
            last_message_ts: None,
        });
    }
    Ok(channels)
}

async fn mark_read(client: &SlackClient, channel_id: &str) -> Result<(), SlackCliError> {
    let now = Utc::now();
    let ts = format!("{}.{:06}", now.timestamp(), now.timestamp_subsec_micros());
    client
        .post_json(
            "conversations.mark",
            &json!({ "channel": channel_id, "ts": ts }),
        )
        .await?;
    Ok(())
}

fn author_ids(messages: &[Value]) -> Vec<String> {
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

/// API 呼び出しなしで決まる表示名。IM で名前が無い場合は空文字を返す。
fn plain_display_name(channel: &Value) -> String {
    channel
        .get("name")
        .and_then(Value::as_str)
        .filter(|name| !name.is_empty())
        .map(format_channel_name)
        .unwrap_or_default()
}

/// 表示名。IM は相手のユーザー名を引いて `@name` にする。
async fn display_name_for(client: &SlackClient, channel: &Value) -> String {
    let name = plain_display_name(channel);
    if !name.is_empty() {
        return name;
    }

    if let Some(user_id) = channel.get("user").and_then(Value::as_str) {
        let users = fetch_usernames(client, &[user_id.to_string()]).await;
        return match users.get(user_id) {
            Some(name) => format!("@{name}"),
            None => format!("@{user_id}"),
        };
    }
    "#unknown".to_string()
}

async fn fetch_display_name(client: &SlackClient, channel_id: &str) -> String {
    match client
        .get("conversations.info", &[("channel", channel_id)])
        .await
    {
        Ok(info) => {
            let channel = info.get("channel").cloned().unwrap_or(Value::Null);
            display_name_for(client, &channel).await
        }
        Err(_) => "#unknown".to_string(),
    }
}

/// 先頭が `#` / `@` なら足さない（移植方針 G16 と同じ規則）。
fn format_channel_name(name: &str) -> String {
    if name.starts_with('#') || name.starts_with('@') {
        name.to_string()
    } else {
        format!("#{name}")
    }
}

fn timestamp_key(ts: &Option<String>) -> f64 {
    ts.as_deref()
        .and_then(|value| value.parse::<f64>().ok())
        .filter(|value| value.is_finite())
        .unwrap_or(f64::MIN)
}

fn is_newer(candidate: &Option<String>, current: &Option<String>) -> bool {
    timestamp_key(candidate) > timestamp_key(current)
}

#[cfg(test)]
mod tests {
    use clap::Parser;
    use url::Url;
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;
    use crate::cli::Cli;

    fn parse(argv: &[&str]) -> UnreadCommand {
        let cli = Cli::try_parse_from(argv).unwrap();
        let crate::cli::Command::Unread(cmd) = cli.command else {
            panic!("expected the unread command");
        };
        cmd
    }

    fn client_for(server: &MockServer) -> SlackClient {
        SlackClient::new("test-token-value")
            .unwrap()
            .with_base_url(Url::parse(&format!("{}/", server.uri())).unwrap())
    }

    fn command() -> UnreadCommand {
        UnreadCommand {
            channel: None,
            count_only: false,
            limit: DEFAULT_DISPLAY_LIMIT.to_string(),
            mark_read: false,
        }
    }

    fn json_opts() -> GlobalOpts {
        GlobalOpts {
            json: true,
            ..GlobalOpts::default()
        }
    }

    async fn mount_search(server: &MockServer, body: Value) {
        Mock::given(method("GET"))
            .and(path("/search.messages"))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .mount(server)
            .await;
    }

    #[test]
    fn defaults_match_the_typescript_version() {
        let cmd = parse(&["slack-cli", "unread"]);
        assert_eq!(cmd.limit, "50");
        assert!(!cmd.count_only);
        assert!(!cmd.mark_read);
        assert!(cmd.channel.is_none());
    }

    #[test]
    fn parses_every_flag() {
        let cmd = parse(&[
            "slack-cli",
            "unread",
            "-c",
            "general",
            "--count-only",
            "--limit",
            "10",
            "--mark-read",
        ]);
        assert_eq!(cmd.channel.as_deref(), Some("general"));
        assert_eq!(cmd.limit, "10");
        assert!(cmd.count_only);
        assert!(cmd.mark_read);
    }

    #[tokio::test]
    async fn limit_is_parsed_strictly() {
        let server = MockServer::start().await;
        let client = client_for(&server);

        for bad in ["abc", "0", "12abc"] {
            let cmd = UnreadCommand {
                limit: bad.to_string(),
                ..command()
            };
            let err = run(cmd, &client, &json_opts()).await.unwrap_err();
            assert_eq!(err.to_string(), ERR_LIMIT, "{bad:?} should be rejected");
        }
        // 引数が不正なら API は 1 度も呼ばない
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    #[test]
    fn count_only_never_overrides_the_requested_format() {
        let messages =
            vec![json!({ "timestamp": "2023-11-14 22:13:20", "author": "alice", "text": "hi" })];

        let counted =
            build_single_output("#general", "C1", 12, &messages, true, OutputFormat::Json);
        assert_eq!(counted["unreadCount"], 12);
        assert!(counted.get("messages").is_none());
        assert!(counted.is_object(), "json output must stay JSON");

        let full = build_single_output("#general", "C1", 12, &messages, false, OutputFormat::Json);
        assert_eq!(full["displayedMessageCount"], 1);
        assert_eq!(full["isTruncated"], true);
        assert_eq!(full["messages"][0]["author"], "alice");

        // 表形式はメッセージ 1 行ずつ、件数だけなら要約 1 行
        let table =
            build_single_output("#general", "C1", 12, &messages, false, OutputFormat::Table);
        assert!(table.is_array());
        let table_counted =
            build_single_output("#general", "C1", 12, &messages, true, OutputFormat::Table);
        assert_eq!(table_counted["unreadCount"], 12);
    }

    #[test]
    fn channel_names_never_get_a_double_prefix() {
        assert_eq!(format_channel_name("general"), "#general");
        assert_eq!(format_channel_name("#general"), "#general");
        assert_eq!(format_channel_name("@alice"), "@alice");
        assert_eq!(plain_display_name(&json!({ "id": "D1" })), "");
    }

    #[tokio::test]
    async fn single_channel_counts_every_page_but_previews_at_most_fifty() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/conversations.info"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "channel": { "id": "C0123ABCD", "name": "general", "last_read": "1700000000.000000" },
            })))
            .mount(&server)
            .await;

        let page: Vec<Value> = (0..40)
            .map(
                |i| json!({ "ts": format!("17000000{:02}.000000", i), "user": "U1", "text": "hi" }),
            )
            .collect();
        Mock::given(method("GET"))
            .and(path("/conversations.history"))
            .and(query_param("cursor", "page2"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "messages": page,
                "response_metadata": { "next_cursor": "" },
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/conversations.history"))
            .and(query_param("oldest", "1700000000.000000"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "messages": page,
                "response_metadata": { "next_cursor": "page2" },
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/users.info"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "user": { "id": "U1", "name": "alice" },
            })))
            .mount(&server)
            .await;

        let cmd = UnreadCommand {
            channel: Some("C0123ABCD".into()),
            ..command()
        };
        run(cmd, &client_for(&server), &json_opts()).await.unwrap();

        let requests = server.received_requests().await.unwrap();
        let history_calls = requests
            .iter()
            .filter(|r| r.url.path().ends_with("conversations.history"))
            .count();
        assert_eq!(history_calls, 2, "unread counts require every page");
    }

    #[tokio::test]
    async fn search_results_are_aggregated_per_channel_and_sorted_by_recency() {
        let server = MockServer::start().await;
        mount_search(
            &server,
            json!({
                "ok": true,
                "messages": {
                    "matches": [
                        { "ts": "1700000001.000000", "channel": { "id": "C1", "name": "general" } },
                        { "ts": "1700000009.000000", "channel": { "id": "C2", "name": "random" } },
                        { "ts": "1700000005.000000", "channel": { "id": "C1", "name": "general" } },
                    ],
                },
                "response_metadata": { "next_cursor": "" },
            }),
        )
        .await;

        let channels = search_unread_channels(&client_for(&server)).await.unwrap();
        assert_eq!(channels.len(), 2);
        assert_eq!(channels[0].id, "C2");
        assert_eq!(channels[1].unread_count, 2);
        assert_eq!(channels[1].name, "#general");
        assert_eq!(
            channels[1].to_value(),
            json!({ "channel": "#general", "channelId": "C1", "unreadCount": 2 })
        );
    }

    #[tokio::test]
    async fn a_failing_search_falls_back_to_users_conversations() {
        let server = MockServer::start().await;
        mount_search(
            &server,
            json!({ "ok": false, "error": "not_allowed_token_type" }),
        )
        .await;
        Mock::given(method("GET"))
            .and(path("/users.conversations"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "channels": [
                    { "id": "C1", "name": "general", "unread_count": 3 },
                    { "id": "C2", "name": "quiet", "unread_count": 0 },
                ],
                "response_metadata": { "next_cursor": "" },
            })))
            .mount(&server)
            .await;

        let channels = list_unread_conversations(&client_for(&server))
            .await
            .unwrap();
        assert_eq!(channels.len(), 1, "channels without unread must be dropped");
        assert_eq!(channels[0].name, "#general");

        // run() 側でも経路が切り替わることを確認する
        run(command(), &client_for(&server), &json_opts())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn mark_read_covers_every_unread_channel_even_beyond_the_limit() {
        let server = MockServer::start().await;
        mount_search(
            &server,
            json!({
                "ok": true,
                "messages": {
                    "matches": [
                        { "ts": "1700000001.000000", "channel": { "id": "C1", "name": "general" } },
                        { "ts": "1700000009.000000", "channel": { "id": "C2", "name": "random" } },
                    ],
                },
                "response_metadata": { "next_cursor": "" },
            }),
        )
        .await;
        Mock::given(method("POST"))
            .and(path("/conversations.mark"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "ok": true })))
            .expect(2)
            .mount(&server)
            .await;

        let cmd = UnreadCommand {
            limit: "1".to_string(),
            mark_read: true,
            ..command()
        };
        run(cmd, &client_for(&server), &json_opts()).await.unwrap();
    }

    #[tokio::test]
    async fn no_unread_channels_still_produce_valid_json() {
        let server = MockServer::start().await;
        mount_search(
            &server,
            json!({
                "ok": true,
                "messages": { "matches": [] },
                "response_metadata": { "next_cursor": "" },
            }),
        )
        .await;

        let channels = search_unread_channels(&client_for(&server)).await.unwrap();
        assert!(channels.is_empty());

        let mut buffer = Vec::new();
        output::format_value(&json!([]), OutputFormat::Json, &mut buffer).unwrap();
        let rendered: Value = serde_json::from_slice(&buffer).unwrap();
        assert_eq!(rendered, json!([]));

        run(command(), &client_for(&server), &json_opts())
            .await
            .unwrap();
    }
}
