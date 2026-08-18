//! `slack-cli scheduled` — 予約送信メッセージの一覧と取り消し。
//!
//! チャンネル名解決・端末サニタイズ・一覧出力のヘルパはこのモジュールに置き、
//! `reminder` からも参照する。

use std::io::Write;

use clap::{Args, Subcommand};
use serde_json::{json, Value};

use crate::cli::common::{report_success, resolve_channel_id};
use crate::cli::{parse_positive_int, GlobalOpts};
use crate::client::pagination::PaginationOpts;
use crate::client::SlackClient;
use crate::error::SlackCliError;
use crate::output::sanitize::sanitize_value;
use crate::output::{self, OutputFormat};

/// `scheduled list --limit` の既定値。
pub const DEFAULT_LIST_LIMIT: &str = "50";

pub const ERR_LIMIT: &str = "--limit must be a positive integer";
pub const MSG_NO_SCHEDULED: &str = "No scheduled messages found";

#[derive(Args, Debug)]
pub struct ScheduledCommand {
    #[command(subcommand)]
    pub command: ScheduledSubcommand,
}

#[derive(Subcommand, Debug)]
pub enum ScheduledSubcommand {
    /// List scheduled messages
    List {
        /// Filter by channel name or ID
        #[arg(short, long, value_name = "CHANNEL")]
        channel: Option<String>,

        /// Maximum number of scheduled messages to list
        #[arg(long, default_value = DEFAULT_LIST_LIMIT, value_name = "NUMBER")]
        limit: String,
    },
    /// Cancel a scheduled message
    Cancel {
        /// Channel name or ID
        #[arg(short, long, required = true, value_name = "CHANNEL")]
        channel: String,

        /// Scheduled message ID
        #[arg(long, required = true, value_name = "SCHEDULED_MESSAGE_ID")]
        id: String,
    },
}

pub async fn run(
    cmd: ScheduledCommand,
    client: &SlackClient,
    global: &GlobalOpts,
) -> Result<(), SlackCliError> {
    match cmd.command {
        ScheduledSubcommand::List { channel, limit } => {
            list(client, channel.as_deref(), &limit, global).await
        }
        ScheduledSubcommand::Cancel { channel, id } => cancel(client, &channel, &id, global).await,
    }
}

async fn list(
    client: &SlackClient,
    channel: Option<&str>,
    limit: &str,
    global: &GlobalOpts,
) -> Result<(), SlackCliError> {
    // 移植方針 A1 / G3: `--limit` は厳格にパースし、不正値は API へ送らずエラーにする。
    let limit = parse_positive_int(limit, ERR_LIMIT)?;

    let channel_id = match channel {
        Some(name) => Some(resolve_channel_id(client, name).await?),
        None => None,
    };

    let mut params: Vec<(&str, &str)> = Vec::new();
    if let Some(id) = channel_id.as_deref() {
        params.push(("channel", id));
    }

    // 移植方針 G5: TS 版は 1 ページで打ち切っていた。`--limit` 件に達するまでカーソルを追う。
    let messages = client
        .paginate_get(
            "chat.scheduledMessages.list",
            &params,
            "scheduled_messages",
            &PaginationOpts {
                page_size: Some(limit),
                limit: Some(limit),
                ..PaginationOpts::default()
            },
        )
        .await?;

    let sanitized: Vec<Value> = messages.iter().map(sanitize_value).collect();
    write_list(
        &sanitized,
        MSG_NO_SCHEDULED,
        global.output_format(),
        &mut std::io::stdout(),
    )
}

async fn cancel(
    client: &SlackClient,
    channel: &str,
    id: &str,
    global: &GlobalOpts,
) -> Result<(), SlackCliError> {
    let channel_id = resolve_channel_id(client, channel).await?;
    client
        .post_json(
            "chat.deleteScheduledMessage",
            &json!({ "channel": channel_id, "scheduled_message_id": id }),
        )
        .await?;

    report_success(
        global,
        &format!("✓ Scheduled message {id} cancelled"),
        &json!({ "ok": true, "channel": channel_id, "scheduled_message_id": id }),
    )
}

/// 一覧の出力。0 件でも `--format json` 等では空の JSON を出す（移植方針 G14）。
pub(crate) fn write_list(
    items: &[Value],
    empty_message: &str,
    format: OutputFormat,
    writer: &mut dyn Write,
) -> Result<(), SlackCliError> {
    if items.is_empty() && format == OutputFormat::Table {
        writeln!(writer, "{empty_message}")?;
        return Ok(());
    }
    output::format_value(&Value::Array(items.to_vec()), format, writer)
}

#[cfg(test)]
mod tests {
    use clap::error::ErrorKind;
    use clap::Parser;
    use url::Url;
    use wiremock::matchers::{
        body_partial_json, method, path, query_param, query_param_is_missing,
    };
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;
    use crate::cli::Cli;

    fn client_for(server: &MockServer) -> SlackClient {
        SlackClient::new("test-token-value")
            .unwrap()
            .with_base_url(Url::parse(&format!("{}/", server.uri())).unwrap())
    }

    fn parse(argv: &[&str]) -> ScheduledSubcommand {
        let cli = Cli::try_parse_from(argv).unwrap();
        let crate::cli::Command::Scheduled(cmd) = cli.command else {
            panic!("expected the scheduled command");
        };
        cmd.command
    }

    fn json_global() -> GlobalOpts {
        GlobalOpts {
            json: true,
            ..GlobalOpts::default()
        }
    }

    async fn mount_channel_lookup(server: &MockServer) {
        Mock::given(method("GET"))
            .and(path("/conversations.list"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "channels": [{ "id": "C012345678", "name": "general" }],
                "response_metadata": { "next_cursor": "" },
            })))
            .mount(server)
            .await;
    }

    async fn mount_scheduled_list(server: &MockServer, messages: Value) {
        Mock::given(method("GET"))
            .and(path("/chat.scheduledMessages.list"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "scheduled_messages": messages,
                "response_metadata": { "next_cursor": "" },
            })))
            .mount(server)
            .await;
    }

    #[test]
    fn list_limit_defaults_to_50_and_channel_is_optional() {
        let ScheduledSubcommand::List { channel, limit } =
            parse(&["slack-cli", "scheduled", "list"])
        else {
            panic!("expected scheduled list");
        };
        assert!(channel.is_none());
        assert_eq!(limit, "50");
    }

    #[test]
    fn cancel_requires_channel_and_id() {
        let ScheduledSubcommand::Cancel { channel, id } =
            parse(&["slack-cli", "scheduled", "cancel", "-c", "C1", "--id", "Q1"])
        else {
            panic!("expected scheduled cancel");
        };
        assert_eq!(channel, "C1");
        assert_eq!(id, "Q1");

        let err = Cli::try_parse_from(["slack-cli", "scheduled", "cancel", "-c", "C1"])
            .expect_err("--id is required");
        assert_eq!(err.kind(), ErrorKind::MissingRequiredArgument);
    }

    #[tokio::test]
    async fn list_sends_the_limit_and_omits_the_channel_filter() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/chat.scheduledMessages.list"))
            .and(query_param("limit", "50"))
            .and(query_param_is_missing("channel"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "scheduled_messages": [{
                    "id": "Q1234ABCD",
                    "channel_id": "C012345678",
                    "post_at": 1769936400,
                    "date_created": 1769850000,
                    "text": "明日の会議の件",
                }],
                "response_metadata": { "next_cursor": "" },
            })))
            .expect(1)
            .mount(&server)
            .await;

        list(&client_for(&server), None, "50", &json_global())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn list_resolves_a_channel_name_before_filtering() {
        let server = MockServer::start().await;
        mount_channel_lookup(&server).await;
        mount_scheduled_list(&server, json!([])).await;

        list(&client_for(&server), Some("#general"), "50", &json_global())
            .await
            .unwrap();

        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 2);
        assert!(
            requests[1]
                .url
                .query()
                .unwrap()
                .contains("channel=C012345678"),
            "解決済み ID がフィルタに乗っていない: {:?}",
            requests[1].url
        );
    }

    #[tokio::test]
    async fn list_skips_the_lookup_for_channel_ids() {
        let server = MockServer::start().await;
        mount_scheduled_list(&server, json!([])).await;

        list(
            &client_for(&server),
            Some("C012345678"),
            "50",
            &json_global(),
        )
        .await
        .unwrap();

        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn list_follows_the_cursor_until_the_limit_is_reached() {
        let server = MockServer::start().await;
        Mock::given(query_param_is_missing("cursor"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "scheduled_messages": [{ "id": "Q1" }],
                "response_metadata": { "next_cursor": "page2" },
            })))
            .mount(&server)
            .await;
        Mock::given(query_param("cursor", "page2"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "scheduled_messages": [{ "id": "Q2" }],
                "response_metadata": { "next_cursor": "" },
            })))
            .mount(&server)
            .await;

        list(&client_for(&server), None, "5", &json_global())
            .await
            .unwrap();
        assert_eq!(server.received_requests().await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn a_non_numeric_limit_never_reaches_the_api() {
        let server = MockServer::start().await;
        for raw in ["abc", "12abc", "0", "-1"] {
            let err = list(&client_for(&server), None, raw, &json_global())
                .await
                .unwrap_err();
            assert_eq!(err.to_string(), ERR_LIMIT, "{raw:?} が素通りした");
        }
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn api_errors_are_propagated() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/chat.scheduledMessages.list"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({ "ok": false, "error": "invalid_auth" })),
            )
            .mount(&server)
            .await;

        let err = list(&client_for(&server), None, "50", &json_global())
            .await
            .unwrap_err();
        match err {
            SlackCliError::Api { code, .. } => assert_eq!(code, "invalid_auth"),
            other => panic!("unexpected error: {other}"),
        }
    }

    #[tokio::test]
    async fn an_unknown_channel_name_reports_the_closest_candidates() {
        let server = MockServer::start().await;
        mount_channel_lookup(&server).await;

        let err = list(&client_for(&server), Some("gene"), "50", &json_global())
            .await
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "Channel 'gene' not found. Did you mean one of these? general"
        );
    }

    #[tokio::test]
    async fn cancel_posts_the_resolved_channel_id() {
        let server = MockServer::start().await;
        mount_channel_lookup(&server).await;
        Mock::given(method("POST"))
            .and(path("/chat.deleteScheduledMessage"))
            .and(body_partial_json(json!({
                "channel": "C012345678",
                "scheduled_message_id": "Q1234ABCD",
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "ok": true })))
            .expect(1)
            .mount(&server)
            .await;

        cancel(
            &client_for(&server),
            "#general",
            "Q1234ABCD",
            &GlobalOpts::default(),
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn cancel_sends_nothing_in_dry_run() {
        let server = MockServer::start().await;
        let client = client_for(&server).with_dry_run(true);

        cancel(&client, "C012345678", "Q1", &GlobalOpts::default())
            .await
            .unwrap();
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn cancel_propagates_api_errors() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat.deleteScheduledMessage"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({ "ok": false, "error": "invalid_scheduled_message_id" })),
            )
            .mount(&server)
            .await;

        let err = cancel(
            &client_for(&server),
            "C012345678",
            "Q1",
            &GlobalOpts::default(),
        )
        .await
        .unwrap_err();
        match err {
            SlackCliError::Api { code, .. } => assert_eq!(code, "invalid_scheduled_message_id"),
            other => panic!("unexpected error: {other}"),
        }
    }

    #[test]
    fn empty_results_stay_json_but_keep_the_table_message() {
        let mut table = Vec::new();
        write_list(&[], MSG_NO_SCHEDULED, OutputFormat::Table, &mut table).unwrap();
        assert_eq!(
            String::from_utf8(table).unwrap(),
            "No scheduled messages found\n"
        );

        let mut as_json = Vec::new();
        write_list(&[], MSG_NO_SCHEDULED, OutputFormat::Json, &mut as_json).unwrap();
        let parsed: Value = serde_json::from_slice(&as_json).unwrap();
        assert_eq!(parsed, json!([]));
    }

    #[test]
    fn json_output_keeps_the_raw_api_shape() {
        let message = json!({
            "id": "Q1234ABCD",
            "channel_id": "C012345678",
            "post_at": 1769936400,
            "date_created": 1769850000,
            "text": "hello",
        });
        let mut buf = Vec::new();
        write_list(
            &[sanitize_value(&message)],
            MSG_NO_SCHEDULED,
            OutputFormat::Json,
            &mut buf,
        )
        .unwrap();

        let parsed: Value = serde_json::from_slice(&buf).unwrap();
        // post_at は epoch 秒の数値のまま（TS 版の JSON 契約）
        assert_eq!(parsed[0]["post_at"], 1769936400);
        assert_eq!(parsed[0]["id"], "Q1234ABCD");
    }
    #[tokio::test]
    async fn run_dispatches_both_subcommands() {
        let server = MockServer::start().await;
        mount_scheduled_list(&server, json!([])).await;
        Mock::given(method("POST"))
            .and(path("/chat.deleteScheduledMessage"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "ok": true })))
            .mount(&server)
            .await;

        let client = client_for(&server);
        let global = json_global();
        run(
            ScheduledCommand {
                command: ScheduledSubcommand::List {
                    channel: None,
                    limit: DEFAULT_LIST_LIMIT.to_string(),
                },
            },
            &client,
            &global,
        )
        .await
        .unwrap();
        run(
            ScheduledCommand {
                command: ScheduledSubcommand::Cancel {
                    channel: "C012345678".to_string(),
                    id: "Q1".to_string(),
                },
            },
            &client,
            &global,
        )
        .await
        .unwrap();

        assert_eq!(server.received_requests().await.unwrap().len(), 2);
    }
}
