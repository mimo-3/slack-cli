//! `slack-cli bookmark` — 「あとで」に保存したアイテムの管理。
//!
//! 実体は Slack の stars API（save for later）であって `bookmarks.*` ではない。

use std::io::Write;

use clap::{Args, Subcommand};
use serde_json::{json, Value};

use crate::cli::canvas::write_empty;
use crate::cli::common::{is_channel_id, resolve_channel_id, write_success};
use crate::cli::{parse_positive_int, GlobalOpts};
use crate::client::pagination::PaginationOpts;
use crate::client::SlackClient;
use crate::error::SlackCliError;
use crate::output::sanitize::sanitize_terminal_text;
use crate::output::{self};

/// `bookmark list --limit` の既定値。
pub const DEFAULT_LIST_LIMIT: &str = "100";

pub const MSG_NO_ITEMS: &str = "No saved items found";
pub const ERR_LIMIT: &str = "--limit must be a positive integer";

const INVALID_TIMESTAMP: &str = "(invalid timestamp)";

const STARS_ADD: &str = "stars.add";
const STARS_LIST: &str = "stars.list";
const STARS_REMOVE: &str = "stars.remove";

const CHANNEL_NOT_FOUND: &str = "channel_not_found";

#[derive(Args, Debug)]
pub struct BookmarkCommand {
    #[command(subcommand)]
    pub command: BookmarkSubcommand,
}

#[derive(Subcommand, Debug)]
pub enum BookmarkSubcommand {
    /// Save a message for later
    Add(BookmarkTargetArgs),
    /// List saved items
    List {
        /// Number of items to display
        #[arg(long, default_value = DEFAULT_LIST_LIMIT, value_name = "LIMIT")]
        limit: String,
    },
    /// Remove a saved item
    Remove(BookmarkTargetArgs),
}

#[derive(Args, Debug)]
pub struct BookmarkTargetArgs {
    /// Channel name or ID
    #[arg(short, long, required = true, value_name = "CHANNEL")]
    pub channel: String,

    /// Message timestamp
    #[arg(long, required = true, value_name = "TIMESTAMP")]
    pub ts: String,
}

pub async fn run(
    cmd: BookmarkCommand,
    client: &SlackClient,
    global: &GlobalOpts,
) -> Result<(), SlackCliError> {
    run_to(cmd, client, global, &mut std::io::stdout()).await
}

async fn run_to(
    cmd: BookmarkCommand,
    client: &SlackClient,
    global: &GlobalOpts,
    writer: &mut (dyn Write + Send),
) -> Result<(), SlackCliError> {
    let format = global.output_format();

    match cmd.command {
        BookmarkSubcommand::Add(args) => {
            call_stars(client, STARS_ADD, &args).await?;
            write_result(
                &format!(
                    "✓ Saved message {} in {}",
                    sanitize_terminal_text(&args.ts),
                    sanitize_terminal_text(&args.channel)
                ),
                &args,
                global,
                writer,
            )
        }

        BookmarkSubcommand::Remove(args) => {
            call_stars(client, STARS_REMOVE, &args).await?;
            write_result(
                &format!(
                    "✓ Removed saved item {} from {}",
                    sanitize_terminal_text(&args.ts),
                    sanitize_terminal_text(&args.channel)
                ),
                &args,
                global,
                writer,
            )
        }

        BookmarkSubcommand::List { limit } => {
            let limit = parse_positive_int(&limit, ERR_LIMIT)?;
            let count = limit.to_string();

            // stars.list のページサイズは `count`。カーソルは指定件数に達するまで追う（移植方針 G5）。
            let items = client
                .paginate_get(
                    STARS_LIST,
                    &[("count", count.as_str())],
                    "items",
                    &PaginationOpts {
                        limit: Some(limit),
                        ..PaginationOpts::default()
                    },
                )
                .await?;

            if items.is_empty() {
                return write_empty(MSG_NO_ITEMS, format, writer);
            }

            // 表示用の平らな形は TS 版の json 出力のキー構成と同じなので、全フォーマット共通で使う。
            let value = Value::Array(items.iter().map(saved_item).collect());
            output::format_value(&value, format, writer)
        }
    }
}

/// stars.add / stars.remove の呼び出し。チャンネル名は解決せずまず生値で送り、
/// `channel_not_found` が返ったときだけ名前解決して 1 回だけ再試行する（移植方針 G1）。
async fn call_stars(
    client: &SlackClient,
    method: &str,
    args: &BookmarkTargetArgs,
) -> Result<(), SlackCliError> {
    let body = |channel: &str| json!({ "channel": channel, "timestamp": args.ts });

    match client.post_json(method, &body(&args.channel)).await {
        Ok(_) => Ok(()),
        Err(SlackCliError::Api { code, .. })
            if code == CHANNEL_NOT_FOUND && !is_channel_id(&args.channel) =>
        {
            let resolved = resolve_channel_id(client, &args.channel).await?;
            client.post_json(method, &body(&resolved)).await?;
            Ok(())
        }
        Err(other) => Err(other),
    }
}

/// 書き込み系の結果。`table` は TS 版と同じ 1 行の成功メッセージ、
/// それ以外のフォーマットは機械可読なオブジェクトを出す。
fn write_result(
    message: &str,
    args: &BookmarkTargetArgs,
    global: &GlobalOpts,
    writer: &mut (dyn Write + Send),
) -> Result<(), SlackCliError> {
    let value = json!({ "ok": true, "channel": args.channel, "ts": args.ts });
    write_success(writer, global, message, &value)
}

/// `stars.list` の item を表示用のフラットな形へ畳む。
fn saved_item(item: &Value) -> Value {
    let message = item.get("message");
    let date_create = item.get("date_create").and_then(Value::as_i64);

    json!({
        "type": item.get("type").and_then(Value::as_str).unwrap_or_default(),
        "channel": item.get("channel").and_then(Value::as_str).unwrap_or_default(),
        "timestamp": message
            .and_then(|m| m.get("ts"))
            .and_then(Value::as_str)
            .unwrap_or_default(),
        "text": message
            .and_then(|m| m.get("text"))
            .and_then(Value::as_str)
            .map(sanitize_single_line)
            .unwrap_or_default(),
        "date_create": date_create,
        "saved_at": format_saved_at(date_create),
    })
}

/// Unix 秒を `2026-04-05T12:34:38.000Z` 形式にする。範囲外・欠落は文言に落とす（移植方針 A4 / D6）。
fn format_saved_at(seconds: Option<i64>) -> String {
    seconds
        .and_then(|s| chrono::DateTime::from_timestamp(s, 0))
        .map(|dt| dt.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string())
        .unwrap_or_else(|| INVALID_TIMESTAMP.to_string())
}

/// 端末サニタイズに加えて空白の連続を 1 つに畳む。
/// JS の `\s` に合わせるため U+FEFF も空白として扱う（移植方針 J5）。
fn sanitize_single_line(input: &str) -> String {
    sanitize_terminal_text(input)
        .split(|c: char| c.is_whitespace() || c == '\u{feff}')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::output::OutputFormat;
    use clap::error::ErrorKind;
    use clap::Parser;
    use url::Url;
    use wiremock::matchers::{body_json, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use crate::cli::Cli;

    fn client_for(server: &MockServer) -> SlackClient {
        SlackClient::new("test-token-value")
            .unwrap()
            .with_base_url(Url::parse(&format!("{}/", server.uri())).unwrap())
    }

    fn target(channel: &str, ts: &str) -> BookmarkTargetArgs {
        BookmarkTargetArgs {
            channel: channel.into(),
            ts: ts.into(),
        }
    }

    async fn execute(
        command: BookmarkSubcommand,
        client: &SlackClient,
        format: OutputFormat,
    ) -> Result<String, SlackCliError> {
        let global = GlobalOpts {
            format,
            ..GlobalOpts::default()
        };
        let mut buf: Vec<u8> = Vec::new();
        run_to(BookmarkCommand { command }, client, &global, &mut buf).await?;
        Ok(String::from_utf8(buf).unwrap())
    }

    async fn mount_json(server: &MockServer, endpoint: &str, body: Value) {
        Mock::given(path(format!("/{endpoint}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .mount(server)
            .await;
    }

    #[test]
    fn list_limit_defaults_to_100() {
        let cli = Cli::try_parse_from(["slack-cli", "bookmark", "list"]).unwrap();
        let crate::cli::Command::Bookmark(cmd) = cli.command else {
            panic!("expected the bookmark command");
        };
        let BookmarkSubcommand::List { limit } = cmd.command else {
            panic!("expected bookmark list");
        };
        assert_eq!(limit, "100");
    }

    #[test]
    fn add_and_remove_require_channel_and_ts() {
        for sub in ["add", "remove"] {
            Cli::try_parse_from(["slack-cli", "bookmark", sub, "-c", "C1", "--ts", "1.1"]).unwrap();
            let err = Cli::try_parse_from(["slack-cli", "bookmark", sub, "-c", "C1"])
                .expect_err("--ts is required");
            assert_eq!(err.kind(), ErrorKind::MissingRequiredArgument);
        }
    }

    #[tokio::test]
    async fn add_posts_the_channel_and_timestamp() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/stars.add"))
            .and(body_json(
                json!({ "channel": "C0123456789", "timestamp": "1712345678.123456" }),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "ok": true })))
            .expect(1)
            .mount(&server)
            .await;

        let out = execute(
            BookmarkSubcommand::Add(target("C0123456789", "1712345678.123456")),
            &client_for(&server),
            OutputFormat::Table,
        )
        .await
        .unwrap();

        assert!(
            out.contains("✓ Saved message 1712345678.123456 in C0123456789"),
            "{out}"
        );
    }

    #[tokio::test]
    async fn remove_reports_the_item_it_removed() {
        let server = MockServer::start().await;
        mount_json(&server, "stars.remove", json!({ "ok": true })).await;

        let out = execute(
            BookmarkSubcommand::Remove(target("C0123456789", "1.1")),
            &client_for(&server),
            OutputFormat::Table,
        )
        .await
        .unwrap();

        assert!(
            out.contains("✓ Removed saved item 1.1 from C0123456789"),
            "{out}"
        );
    }

    #[tokio::test]
    async fn success_messages_are_stripped_of_escape_sequences() {
        let server = MockServer::start().await;
        mount_json(&server, "stars.add", json!({ "ok": true })).await;

        let out = execute(
            BookmarkSubcommand::Add(target("C0123456789", "\u{1b}[31m1.1")),
            &client_for(&server),
            OutputFormat::Table,
        )
        .await
        .unwrap();

        assert!(!out.contains("\u{1b}[31m"), "{out:?}");
        assert!(out.contains("✓ Saved message 1.1 in C0123456789"), "{out}");
    }

    #[tokio::test]
    async fn add_resolves_a_channel_name_after_channel_not_found() {
        let server = MockServer::start().await;
        Mock::given(path("/stars.add"))
            .and(body_json(
                json!({ "channel": "general", "timestamp": "1.1" }),
            ))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({ "ok": false, "error": "channel_not_found" })),
            )
            .expect(1)
            .mount(&server)
            .await;
        mount_json(
            &server,
            "conversations.list",
            json!({
                "ok": true,
                "channels": [{ "id": "C0123456789", "name": "general" }],
                "response_metadata": { "next_cursor": "" },
            }),
        )
        .await;
        Mock::given(path("/stars.add"))
            .and(body_json(
                json!({ "channel": "C0123456789", "timestamp": "1.1" }),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "ok": true })))
            .expect(1)
            .mount(&server)
            .await;

        let out = execute(
            BookmarkSubcommand::Add(target("general", "1.1")),
            &client_for(&server),
            OutputFormat::Table,
        )
        .await
        .unwrap();

        assert!(out.contains("✓ Saved message 1.1 in general"), "{out}");
    }

    #[tokio::test]
    async fn channel_ids_are_never_retried_after_channel_not_found() {
        let server = MockServer::start().await;
        Mock::given(path("/stars.add"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({ "ok": false, "error": "channel_not_found" })),
            )
            .expect(1)
            .mount(&server)
            .await;

        let err = execute(
            BookmarkSubcommand::Add(target("C0123456789", "1.1")),
            &client_for(&server),
            OutputFormat::Table,
        )
        .await
        .unwrap_err();

        assert!(
            matches!(&err, SlackCliError::Api { code, .. } if code == CHANNEL_NOT_FOUND),
            "{err}"
        );
    }

    #[tokio::test]
    async fn list_sends_the_limit_as_count_and_flattens_items() {
        let server = MockServer::start().await;
        Mock::given(path("/stars.list"))
            .and(query_param("count", "2"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "items": [{
                    "type": "message",
                    "channel": "C0123456789",
                    "date_create": 1712345678,
                    "message": { "ts": "1712345678.123456", "text": "お疲れさま\nです" },
                }],
                "response_metadata": { "next_cursor": "" },
            })))
            .expect(1)
            .mount(&server)
            .await;

        let out = execute(
            BookmarkSubcommand::List { limit: "2".into() },
            &client_for(&server),
            OutputFormat::Json,
        )
        .await
        .unwrap();

        let parsed: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed[0]["timestamp"], "1712345678.123456");
        assert_eq!(parsed[0]["text"], "お疲れさま です");
        assert_eq!(parsed[0]["saved_at"], "2024-04-05T19:34:38.000Z");
        assert_eq!(parsed[0]["channel"], "C0123456789");
    }

    #[tokio::test]
    async fn list_rejects_a_non_numeric_limit_before_calling_the_api() {
        let server = MockServer::start().await;
        let err = execute(
            BookmarkSubcommand::List {
                limit: "12abc".into(),
            },
            &client_for(&server),
            OutputFormat::Json,
        )
        .await
        .unwrap_err();

        assert_eq!(err.to_string(), ERR_LIMIT);
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn empty_lists_respect_the_output_format() {
        let server = MockServer::start().await;
        mount_json(
            &server,
            "stars.list",
            json!({ "ok": true, "items": [], "response_metadata": { "next_cursor": "" } }),
        )
        .await;
        let client = client_for(&server);

        let json_out = execute(
            BookmarkSubcommand::List { limit: "10".into() },
            &client,
            OutputFormat::Json,
        )
        .await
        .unwrap();
        assert_eq!(serde_json::from_str::<Value>(&json_out).unwrap(), json!([]));

        let table_out = execute(
            BookmarkSubcommand::List { limit: "10".into() },
            &client,
            OutputFormat::Table,
        )
        .await
        .unwrap();
        assert!(table_out.contains(MSG_NO_ITEMS), "{table_out}");
    }

    #[test]
    fn timestamps_out_of_range_do_not_panic() {
        assert_eq!(format_saved_at(Some(0)), "1970-01-01T00:00:00.000Z");
        assert_eq!(format_saved_at(None), INVALID_TIMESTAMP);
        assert_eq!(format_saved_at(Some(i64::MAX)), INVALID_TIMESTAMP);
    }

    #[test]
    fn single_line_sanitize_collapses_whitespace_including_the_bom() {
        assert_eq!(sanitize_single_line(" a\t\n b\u{feff}c "), "a b c");
    }
}
