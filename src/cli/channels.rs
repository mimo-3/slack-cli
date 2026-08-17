//! `slack-cli channels` — チャンネル一覧。
//!
//! チャンネル名 → ID の解決、ターミナルサニタイズ、成功メッセージのチャンネル表記も
//! ここに置き、`channel` サブコマンドから共有する。

use std::io::Write;

use chrono::{DateTime, SecondsFormat, Utc};
use clap::Args;
use serde_json::{json, Value};

use crate::cli::common::is_channel_id;
use crate::cli::{parse_positive_int, GlobalOpts};
use crate::client::pagination::PaginationOpts;
use crate::client::SlackClient;
use crate::error::SlackCliError;
use crate::output::sanitize::sanitize_terminal_text;
use crate::output::{self, OutputFormat};

/// `--type` の既定値。
pub const DEFAULT_CHANNEL_TYPE: &str = "public";

pub const ERR_LIMIT: &str = "--limit must be a positive integer";
pub const MSG_NO_CHANNELS: &str = "No channels found";

/// 表示できない時刻の代替表記（移植方針 A4 / D6）。
pub const INVALID_TIMESTAMP: &str = "(invalid timestamp)";

/// `conversations.list` の 1 リクエストあたり件数。
const DEFAULT_PAGE_SIZE: u32 = 100;
const MAX_PAGE_SIZE: u32 = 1000;

#[derive(Args, Debug)]
pub struct ChannelsCommand {
    /// Channel type: public, private, im, mpim, all
    #[arg(long = "type", default_value = DEFAULT_CHANNEL_TYPE, value_name = "TYPE")]
    pub channel_type: String,

    /// Include archived channels
    #[arg(long)]
    pub include_archived: bool,

    /// Maximum number of channels to list (default: unlimited)
    #[arg(long, value_name = "NUMBER")]
    pub limit: Option<String>,
}

pub async fn run(
    cmd: ChannelsCommand,
    client: &SlackClient,
    global: &GlobalOpts,
) -> Result<(), SlackCliError> {
    let types = channel_types(&cmd.channel_type)?;
    let limit = cmd
        .limit
        .as_deref()
        .map(|raw| parse_positive_int(raw, ERR_LIMIT))
        .transpose()?;
    let exclude_archived = if cmd.include_archived { "false" } else { "true" };

    let opts = PaginationOpts {
        page_size: Some(limit.unwrap_or(DEFAULT_PAGE_SIZE).min(MAX_PAGE_SIZE)),
        cursor: None,
        fetch_all: true,
        limit,
    };

    let channels = client
        .paginate_get(
            "conversations.list",
            &[("types", types), ("exclude_archived", exclude_archived)],
            "channels",
            &opts,
        )
        .await?;

    let mapped: Vec<Value> = channels.iter().map(map_channel).collect();
    write_list(
        &mapped,
        MSG_NO_CHANNELS,
        global.output_format(),
        &mut std::io::stdout(),
    )
}

/// `--type` を `conversations.list` の `types` に変換する。
///
/// TS 版は未知の値を黙って `public_channel` に落としていたが、`--format` の検証と揃えて
/// エラーにする（移植方針 G2 の判断をそのまま `--type` にも適用）。
fn channel_types(value: &str) -> Result<&'static str, SlackCliError> {
    match value {
        "public" => Ok("public_channel"),
        "private" => Ok("private_channel"),
        "im" => Ok("im"),
        "mpim" => Ok("mpim"),
        "all" => Ok("public_channel,private_channel,mpim,im"),
        other => Err(SlackCliError::Validation(format!(
            "Invalid type '{}'. Must be one of: public, private, im, mpim, all",
            sanitize_terminal_text(other)
        ))),
    }
}

/// 0 件のときだけ table 形式で人間向けの一文を出す（移植方針 G14）。
/// 機械可読なフォーマットでは空配列をそのまま出す。
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

fn map_channel(channel: &Value) -> Value {
    let name = channel
        .get("name")
        .and_then(Value::as_str)
        .filter(|n| !n.is_empty())
        .unwrap_or("unnamed");
    let purpose = channel
        .get("purpose")
        .and_then(|p| p.get("value"))
        .and_then(Value::as_str)
        .unwrap_or_default();

    json!({
        "id": sanitize_terminal_text(channel.get("id").and_then(Value::as_str).unwrap_or_default()),
        "name": sanitize_terminal_text(name),
        "type": channel_kind(channel),
        "members": channel.get("num_members").and_then(Value::as_u64).unwrap_or(0),
        "created": format_created(channel.get("created").and_then(Value::as_i64)),
        "purpose": sanitize_terminal_text(purpose),
    })
}

fn channel_kind(channel: &Value) -> &'static str {
    let flag = |key: &str| channel.get(key).and_then(Value::as_bool).unwrap_or(false);

    if flag("is_channel") && !flag("is_private") {
        "public"
    } else if flag("is_group") || (flag("is_channel") && flag("is_private")) {
        "private"
    } else if flag("is_im") {
        "im"
    } else if flag("is_mpim") {
        "mpim"
    } else {
        "unknown"
    }
}

/// 作成時刻を RFC3339（UTC・ミリ秒付き）にする（移植方針 D5）。
/// TS 版は日付だけ残して `T00:00:00Z` を連結していたため時分秒が失われていた。
fn format_created(created: Option<i64>) -> String {
    format_timestamp(created, |dt| dt.to_rfc3339_opts(SecondsFormat::Millis, true))
}

/// Unix 秒を UTC の日付（`YYYY-MM-DD`）にする（移植方針 D2）。
pub(crate) fn format_created_date(created: Option<i64>) -> String {
    format_timestamp(created, |dt| dt.format("%Y-%m-%d").to_string())
}

fn format_timestamp(seconds: Option<i64>, render: impl Fn(DateTime<Utc>) -> String) -> String {
    seconds
        .and_then(|s| DateTime::<Utc>::from_timestamp(s, 0))
        .map(render)
        .unwrap_or_else(|| INVALID_TIMESTAMP.to_string())
}

/// 成功メッセージに埋めるチャンネル表記（移植方針 E1 / G16）。
/// ID には `#` を付けず、既に `#` で始まる入力には足さない。サニタイズは必ず通す。
pub(crate) fn format_channel_display(input: &str) -> String {
    let sanitized = sanitize_terminal_text(input);
    if is_channel_id(&sanitized) || sanitized.starts_with('#') {
        sanitized
    } else {
        format!("#{sanitized}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::common::resolve_channel_id;
    use crate::cli::Cli;
    use clap::Parser;
    use url::Url;
    use wiremock::matchers::{any, method, path, query_param, query_param_is_missing};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn parse(argv: &[&str]) -> super::ChannelsCommand {
        let cli = Cli::try_parse_from(argv).unwrap();
        let crate::cli::Command::Channels(cmd) = cli.command else {
            panic!("expected the channels command");
        };
        cmd
    }

    fn client_for(server: &MockServer) -> SlackClient {
        SlackClient::new("test-token-value")
            .unwrap()
            .with_base_url(Url::parse(&format!("{}/", server.uri())).unwrap())
    }

    fn command(channel_type: &str, limit: Option<&str>) -> ChannelsCommand {
        ChannelsCommand {
            channel_type: channel_type.to_string(),
            include_archived: false,
            limit: limit.map(str::to_string),
        }
    }

    fn render(items: &[Value], format: OutputFormat) -> String {
        let mut buf = Vec::new();
        write_list(items, MSG_NO_CHANNELS, format, &mut buf).unwrap();
        String::from_utf8(buf).unwrap()
    }

    #[test]
    fn type_defaults_to_public_and_limit_is_unlimited() {
        // 移植方針 G6: --limit の既定値だけ「無制限」に変えている
        let cmd = parse(&["slack-cli", "channels"]);
        assert_eq!(cmd.channel_type, "public");
        assert!(cmd.limit.is_none());
        assert!(!cmd.include_archived);
    }

    #[test]
    fn parses_every_flag() {
        let cmd = parse(&[
            "slack-cli",
            "channels",
            "--type",
            "all",
            "--include-archived",
            "--limit",
            "25",
        ]);
        assert_eq!(cmd.channel_type, "all");
        assert_eq!(cmd.limit.as_deref(), Some("25"));
        assert!(cmd.include_archived);
    }

    #[test]
    fn channel_types_are_mapped_and_unknown_values_rejected() {
        assert_eq!(channel_types("public").unwrap(), "public_channel");
        assert_eq!(channel_types("private").unwrap(), "private_channel");
        assert_eq!(channel_types("im").unwrap(), "im");
        assert_eq!(channel_types("mpim").unwrap(), "mpim");
        assert_eq!(
            channel_types("all").unwrap(),
            "public_channel,private_channel,mpim,im"
        );

        let err = channel_types("foo").unwrap_err();
        assert_eq!(
            err.to_string(),
            "Invalid type 'foo'. Must be one of: public, private, im, mpim, all"
        );
    }

    #[test]
    fn maps_channel_fields_and_kinds() {
        let mapped = map_channel(&json!({
            "id": "C0123456789",
            "name": "general",
            "is_channel": true,
            "is_private": false,
            "num_members": 128,
            "created": 1_554_076_800_i64,
            "purpose": { "value": "会社全体のお知らせ" },
        }));

        assert_eq!(mapped["id"], "C0123456789");
        assert_eq!(mapped["type"], "public");
        assert_eq!(mapped["members"], 128);
        assert_eq!(mapped["purpose"], "会社全体のお知らせ");
        // 移植方針 D5: 時分秒を捨てて T00:00:00Z を連結する変換をやめた
        assert_eq!(mapped["created"], "2019-04-01T00:00:00.000Z");

        let private = map_channel(&json!({ "is_group": true }));
        assert_eq!(private["type"], "private");
        let im = map_channel(&json!({ "is_im": true }));
        assert_eq!(im["type"], "im");
        let mpim = map_channel(&json!({ "is_mpim": true }));
        assert_eq!(mpim["type"], "mpim");
        let unknown = map_channel(&json!({}));
        assert_eq!(unknown["type"], "unknown");
        assert_eq!(unknown["name"], "unnamed");
        assert_eq!(unknown["members"], 0);
    }

    #[test]
    fn created_keeps_the_time_of_day_and_survives_out_of_range_values() {
        assert_eq!(format_created(Some(1_554_076_845)), "2019-04-01T00:00:45.000Z");
        // 移植方針 D6 / A4: 範囲外でもパニックせず代替表記にする
        assert_eq!(format_created(Some(i64::MAX)), INVALID_TIMESTAMP);
        assert_eq!(format_created(None), INVALID_TIMESTAMP);
        assert_eq!(format_created_date(Some(1_554_076_845)), "2019-04-01");
    }

    #[test]
    fn terminal_escapes_are_stripped_from_values() {
        assert_eq!(
            sanitize_terminal_text("\u{1b}]0;pwned\u{7}general"),
            "general"
        );
        assert_eq!(sanitize_terminal_text("\u{1b}[31mred\u{1b}[0m"), "red");
        assert_eq!(sanitize_terminal_text("a\u{7}b\u{7f}c\u{9b}d"), "abcd");
        assert_eq!(sanitize_terminal_text("keep\tthis\nline"), "keep\tthis\nline");
        // 絵文字を割らない
        assert_eq!(sanitize_terminal_text("🎉 done"), "🎉 done");
    }
    #[test]
    fn success_message_channel_display_never_doubles_the_hash() {
        assert_eq!(format_channel_display("general"), "#general");
        assert_eq!(format_channel_display("#general"), "#general");
        // 移植方針 G16: ID には # を付けない
        assert_eq!(format_channel_display("C0123456789"), "C0123456789");
        assert_eq!(format_channel_display("\u{1b}[31mgeneral"), "#general");
    }

    #[test]
    fn empty_result_is_text_for_table_and_json_for_machines() {
        assert_eq!(render(&[], OutputFormat::Table), "No channels found\n");
        // 移植方針 G14: JSON では人間向けの文言ではなく空配列
        let parsed: Value = serde_json::from_str(&render(&[], OutputFormat::Json)).unwrap();
        assert_eq!(parsed, json!([]));
    }

    #[test]
    fn non_empty_result_is_rendered_in_the_requested_format() {
        let items = vec![map_channel(&json!({ "id": "C0123456789", "name": "general" }))];
        let parsed: Value = serde_json::from_str(&render(&items, OutputFormat::Json)).unwrap();
        assert_eq!(parsed[0]["name"], "general");
        assert!(render(&items, OutputFormat::Table).contains("general"));
    }

    #[tokio::test]
    async fn lists_channels_across_pages_and_sends_the_expected_parameters() {
        let server = MockServer::start().await;
        Mock::given(query_param_is_missing("cursor"))
            .and(query_param("types", "public_channel"))
            .and(query_param("exclude_archived", "true"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "channels": [{ "id": "C000000001", "name": "general", "is_channel": true }],
                "response_metadata": { "next_cursor": "page2" },
            })))
            .mount(&server)
            .await;
        Mock::given(query_param("cursor", "page2"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "channels": [{ "id": "C000000002", "name": "random", "is_channel": true }],
                "response_metadata": { "next_cursor": "" },
            })))
            .mount(&server)
            .await;

        run(
            command("public", None),
            &client_for(&server),
            &GlobalOpts {
                json: true,
                ..GlobalOpts::default()
            },
        )
        .await
        .unwrap();

        assert_eq!(server.received_requests().await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn limit_caps_the_total_number_of_channels() {
        let server = MockServer::start().await;
        Mock::given(any())
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "channels": [
                    { "id": "C000000001", "name": "a" },
                    { "id": "C000000002", "name": "b" },
                    { "id": "C000000003", "name": "c" },
                ],
                "response_metadata": { "next_cursor": "more" },
            })))
            .mount(&server)
            .await;

        // 移植方針 G6: --limit はページサイズではなく総件数の上限
        run(
            command("public", Some("2")),
            &client_for(&server),
            &GlobalOpts {
                json: true,
                ..GlobalOpts::default()
            },
        )
        .await
        .unwrap();

        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn non_numeric_limit_fails_before_any_request() {
        let server = MockServer::start().await;
        Mock::given(any())
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "ok": true })))
            .expect(0)
            .mount(&server)
            .await;

        // 移植方針 A1 / A2: NaN を API に送らずエラーで止める
        for raw in ["abc", "12abc", "0", "-1"] {
            let err = run(
                command("public", Some(raw)),
                &client_for(&server),
                &GlobalOpts::default(),
            )
            .await
            .unwrap_err();
            assert_eq!(err.to_string(), ERR_LIMIT, "{raw:?} should have been rejected");
        }
    }

    #[tokio::test]
    async fn api_errors_are_propagated() {
        let server = MockServer::start().await;
        Mock::given(any())
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": false,
                "error": "missing_scope",
                "needed": "channels:read",
            })))
            .mount(&server)
            .await;

        let err = run(
            command("public", None),
            &client_for(&server),
            &GlobalOpts::default(),
        )
        .await
        .unwrap_err();
        assert_eq!(
            err.to_string(),
            "API Error: missing_scope (needed: channels:read)"
        );
    }

    #[tokio::test]
    async fn channel_ids_skip_the_lookup_request() {
        let server = MockServer::start().await;
        Mock::given(any())
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "ok": true })))
            .expect(0)
            .mount(&server)
            .await;

        let resolved = resolve_channel_id(&client_for(&server), "C0123456789")
            .await
            .unwrap();
        assert_eq!(resolved, "C0123456789");
    }

    #[tokio::test]
    async fn resolves_names_by_the_four_matching_rules() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/conversations.list"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "channels": [
                    { "id": "C000000001", "name": "general" },
                    { "id": "C000000002", "name": "Dev-AceJob" },
                    { "id": "C000000003", "name": "x", "name_normalized": "設計相談" },
                ],
                "response_metadata": { "next_cursor": "" },
            })))
            .mount(&server)
            .await;

        for (input, expected) in [
            ("general", "C000000001"),
            ("#general", "C000000001"),
            ("dev-acejob", "C000000002"),
            ("設計相談", "C000000003"),
        ] {
            let resolved = resolve_channel_id(&client_for(&server), input).await.unwrap();
            assert_eq!(resolved, expected, "{input} resolved to the wrong channel");
        }
    }

    #[tokio::test]
    async fn unknown_names_suggest_similar_channels() {
        let server = MockServer::start().await;
        Mock::given(any())
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "channels": [{ "id": "C000000001", "name": "general" }],
                "response_metadata": { "next_cursor": "" },
            })))
            .mount(&server)
            .await;

        let err = resolve_channel_id(&client_for(&server), "gener").await.unwrap_err();
        assert_eq!(
            err.to_string(),
            "Channel 'gener' not found. Did you mean one of these? general"
        );

        let err = resolve_channel_id(&client_for(&server), "nope").await.unwrap_err();
        assert_eq!(
            err.to_string(),
            "Channel 'nope' not found. Make sure you are a member of this channel."
        );
    }

    #[tokio::test]
    async fn missing_scope_retries_without_the_unavailable_channel_types() {
        let server = MockServer::start().await;
        Mock::given(query_param(
            "types",
            "public_channel,private_channel,im,mpim",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "ok": false,
            "error": "missing_scope",
            "needed": "im:read, mpim:read",
        })))
        .mount(&server)
        .await;
        Mock::given(query_param("types", "public_channel,private_channel"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "channels": [{ "id": "C000000001", "name": "general" }],
                "response_metadata": { "next_cursor": "" },
            })))
            .mount(&server)
            .await;

        let resolved = resolve_channel_id(&client_for(&server), "general").await.unwrap();
        assert_eq!(resolved, "C000000001");
    }

    #[tokio::test]
    async fn missing_scope_that_removes_nothing_is_rethrown() {
        let server = MockServer::start().await;
        Mock::given(any())
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": false,
                "error": "missing_scope",
                "needed": "chat:write",
            })))
            .mount(&server)
            .await;

        let err = resolve_channel_id(&client_for(&server), "general").await.unwrap_err();
        assert_eq!(err.to_string(), "API Error: missing_scope (needed: chat:write)");
    }
}
