//! `slack-cli pin` — チャンネルのピン留め操作。
//!
//! タイムスタンプ検証は `reaction` のヘルパを共有する。

use std::io::Write;

use clap::{Args, Subcommand};
use colored::Colorize;
use serde_json::{json, Value};

use crate::cli::common::{channel_label, resolve_channel_id, INVALID_TIMESTAMP};
use crate::cli::reaction::validate_message_timestamp;
use crate::cli::GlobalOpts;
use crate::error::SlackCliError;
use crate::output::sanitize::{sanitize_terminal_text, sanitize_value as sanitize_terminal_value};
use crate::output::{self, OutputFormat};

pub const MSG_NO_PINS: &str = "No pinned items found";
const INVALID_TIMESTAMP_DISPLAY: &str = INVALID_TIMESTAMP;
const UNKNOWN_ITEM_TYPE: &str = "unknown";

#[derive(Args, Debug)]
pub struct PinCommand {
    #[command(subcommand)]
    pub command: PinSubcommand,
}

#[derive(Subcommand, Debug)]
pub enum PinSubcommand {
    /// Pin a message in a channel
    Add(PinTargetArgs),
    /// Unpin a message in a channel
    Remove(PinTargetArgs),
    /// List pinned items in a channel
    List {
        /// Channel name or ID
        #[arg(short, long, required = true, value_name = "CHANNEL")]
        channel: String,
    },
}

#[derive(Args, Debug)]
pub struct PinTargetArgs {
    /// Channel name or ID
    #[arg(short, long, required = true, value_name = "CHANNEL")]
    pub channel: String,

    /// Message timestamp
    #[arg(short, long, required = true, value_name = "TIMESTAMP")]
    pub timestamp: String,
}

pub async fn run(
    cmd: PinCommand,
    client: &crate::client::SlackClient,
    global: &GlobalOpts,
) -> Result<(), SlackCliError> {
    let mut stdout = std::io::stdout();
    execute(cmd, client, global, &mut stdout).await
}

async fn execute(
    cmd: PinCommand,
    client: &crate::client::SlackClient,
    global: &GlobalOpts,
    out: &mut dyn Write,
) -> Result<(), SlackCliError> {
    match cmd.command {
        PinSubcommand::Add(args) => write_pin(client, args, "pins.add", "added to", out).await,
        PinSubcommand::Remove(args) => {
            write_pin(client, args, "pins.remove", "removed from", out).await
        }
        PinSubcommand::List { channel } => list_pins(client, &channel, global, out).await,
    }
}

async fn write_pin(
    client: &crate::client::SlackClient,
    args: PinTargetArgs,
    method: &str,
    verb: &str,
    out: &mut dyn Write,
) -> Result<(), SlackCliError> {
    validate_message_timestamp(&args.timestamp)?;

    let channel_id = resolve_channel_id(client, &args.channel).await?;
    client
        .post_json(
            method,
            &json!({ "channel": channel_id, "timestamp": args.timestamp }),
        )
        .await?;

    let label = channel_label(&args.channel);
    writeln!(
        out,
        "{}",
        format!("✓ Pin {verb} message in {label}").green()
    )?;
    Ok(())
}

async fn list_pins(
    client: &crate::client::SlackClient,
    channel: &str,
    global: &GlobalOpts,
    out: &mut dyn Write,
) -> Result<(), SlackCliError> {
    let channel_id = resolve_channel_id(client, channel).await?;
    // `pins.list` はカーソルを返さないので 1 リクエストで完結する。
    let response = client.get("pins.list", &[("channel", &channel_id)]).await?;
    let items: Vec<Value> = response
        .get("items")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let format = global.output_format();
    if items.is_empty() {
        // 0 件でも機械可読なフォーマットでは空配列を返す（移植方針 G14）。
        if format == OutputFormat::Table {
            writeln!(out, "{MSG_NO_PINS}")?;
            return Ok(());
        }
        return output::format_value(&Value::Array(Vec::new()), format, out);
    }

    let value = match format {
        // JSON / YAML は Slack の生レスポンスを渡す層として扱う。
        OutputFormat::Json | OutputFormat::Yaml => {
            sanitize_terminal_value(&Value::Array(items.clone()))
        }
        _ => Value::Array(items.iter().map(pin_row).collect()),
    };
    output::format_value(&value, format, out)
}

/// 表形式の 1 行。列は TS 版の `type` / `created` / `created_by` / `ts` / `text` に合わせる。
fn pin_row(item: &Value) -> Value {
    let message = item.get("message");
    json!({
        "type": item
            .get("type")
            .and_then(Value::as_str)
            .filter(|t| !t.is_empty())
            .unwrap_or(UNKNOWN_ITEM_TYPE),
        "created": format_created(item.get("created")),
        "created_by": sanitized_field(item.get("created_by")),
        "ts": sanitized_field(message.and_then(|m| m.get("ts"))),
        "text": sanitized_field(message.and_then(|m| m.get("text"))),
    })
}

fn sanitized_field(value: Option<&Value>) -> String {
    value
        .and_then(Value::as_str)
        .map(sanitize_terminal_text)
        .unwrap_or_default()
}

/// `created`（Unix 秒）を ISO 8601 に整形する。未設定・0 は空文字、範囲外は明示的に印を出す。
fn format_created(value: Option<&Value>) -> String {
    let Some(seconds) = value.and_then(Value::as_i64) else {
        return String::new();
    };
    if seconds == 0 {
        return String::new();
    }

    match chrono::DateTime::from_timestamp(seconds, 0) {
        Some(datetime) => datetime.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string(),
        None => INVALID_TIMESTAMP_DISPLAY.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::reaction::ERR_INVALID_TIMESTAMP;
    use crate::cli::Cli;
    use crate::client::SlackClient;
    use clap::error::ErrorKind;
    use clap::Parser;
    use url::Url;
    use wiremock::matchers::{body_partial_json, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn client_for(server: &MockServer) -> SlackClient {
        SlackClient::new("test-token-value")
            .unwrap()
            .with_base_url(Url::parse(&format!("{}/", server.uri())).unwrap())
    }

    fn target(channel: &str) -> PinTargetArgs {
        PinTargetArgs {
            channel: channel.to_string(),
            timestamp: "1700000000.000100".to_string(),
        }
    }

    fn global_with(format: OutputFormat) -> GlobalOpts {
        GlobalOpts {
            format,
            ..GlobalOpts::default()
        }
    }

    async fn run_capture(
        cmd: PinCommand,
        client: &SlackClient,
        global: &GlobalOpts,
    ) -> Result<String, SlackCliError> {
        let mut buf = Vec::new();
        execute(cmd, client, global, &mut buf).await?;
        Ok(String::from_utf8(buf).unwrap())
    }

    async fn mount_pins_list(server: &MockServer, items: Value) {
        Mock::given(method("GET"))
            .and(path("/pins.list"))
            .and(query_param("channel", "C0123456789"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({ "ok": true, "items": items })),
            )
            .mount(server)
            .await;
    }

    #[test]
    fn add_requires_channel_and_timestamp() {
        let cli = Cli::try_parse_from([
            "slack-cli",
            "pin",
            "add",
            "-c",
            "C1",
            "-t",
            "1700000000.000100",
        ])
        .unwrap();
        let crate::cli::Command::Pin(cmd) = cli.command else {
            panic!("expected the pin command");
        };
        let PinSubcommand::Add(args) = cmd.command else {
            panic!("expected pin add");
        };
        assert_eq!(args.channel, "C1");
    }

    #[test]
    fn list_takes_only_a_channel() {
        let cli = Cli::try_parse_from(["slack-cli", "pin", "list", "-c", "C1"]).unwrap();
        let crate::cli::Command::Pin(cmd) = cli.command else {
            panic!("expected the pin command");
        };
        assert!(matches!(cmd.command, PinSubcommand::List { .. }));

        let err =
            Cli::try_parse_from(["slack-cli", "pin", "list"]).expect_err("--channel required");
        assert_eq!(err.kind(), ErrorKind::MissingRequiredArgument);
    }

    #[test]
    fn created_is_iso8601_with_milliseconds() {
        assert_eq!(
            format_created(Some(&json!(1_755_403_953_i64))),
            "2025-08-17T04:12:33.000Z"
        );
        assert_eq!(format_created(Some(&json!(0))), "");
        assert_eq!(format_created(None), "");
        assert_eq!(
            format_created(Some(&json!(i64::MAX))),
            INVALID_TIMESTAMP_DISPLAY
        );
    }

    #[test]
    fn rows_fall_back_to_unknown_and_empty_strings() {
        let row = pin_row(&json!({ "created": 1_755_403_953_i64 }));
        assert_eq!(row["type"], UNKNOWN_ITEM_TYPE);
        assert_eq!(row["ts"], "");
        assert_eq!(row["text"], "");

        let row = pin_row(&json!({
            "type": "message",
            "created_by": "U1",
            "message": { "ts": "1.1", "text": "\u{1b}[31mデプロイ完了" },
        }));
        assert_eq!(row["type"], "message");
        assert_eq!(row["created_by"], "U1");
        assert_eq!(row["text"], "デプロイ完了");
    }

    #[tokio::test]
    async fn add_and_remove_call_their_endpoints_with_the_resolved_channel() {
        for (subcommand, api, phrase) in [
            ("add", "pins.add", "Pin added to message in C0123456789"),
            (
                "remove",
                "pins.remove",
                "Pin removed from message in C0123456789",
            ),
        ] {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path(format!("/{api}")))
                .and(body_partial_json(json!({
                    "channel": "C0123456789",
                    "timestamp": "1700000000.000100",
                })))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "ok": true })))
                .mount(&server)
                .await;

            let command = if subcommand == "add" {
                PinSubcommand::Add(target("C0123456789"))
            } else {
                PinSubcommand::Remove(target("C0123456789"))
            };
            let out = run_capture(
                PinCommand { command },
                &client_for(&server),
                &GlobalOpts::default(),
            )
            .await
            .unwrap();

            assert!(out.contains(phrase), "output was: {out}");
        }
    }

    #[tokio::test]
    async fn a_malformed_timestamp_is_rejected_before_any_request() {
        let server = MockServer::start().await;
        let mut args = target("C0123456789");
        args.timestamp = "1755400000.1".to_string();

        let err = run_capture(
            PinCommand {
                command: PinSubcommand::Add(args),
            },
            &client_for(&server),
            &GlobalOpts::default(),
        )
        .await
        .unwrap_err();

        assert_eq!(err.to_string(), ERR_INVALID_TIMESTAMP);
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn an_empty_list_prints_text_for_table_and_json_for_json() {
        let server = MockServer::start().await;
        mount_pins_list(&server, json!([])).await;
        let client = client_for(&server);

        let out = run_capture(
            PinCommand {
                command: PinSubcommand::List {
                    channel: "C0123456789".to_string(),
                },
            },
            &client,
            &global_with(OutputFormat::Table),
        )
        .await
        .unwrap();
        assert_eq!(out.trim(), MSG_NO_PINS);

        let out = run_capture(
            PinCommand {
                command: PinSubcommand::List {
                    channel: "C0123456789".to_string(),
                },
            },
            &client,
            &global_with(OutputFormat::Json),
        )
        .await
        .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&out).unwrap(),
            Value::Array(Vec::new())
        );
    }

    #[tokio::test]
    async fn json_keeps_the_raw_items_and_table_projects_columns() {
        let server = MockServer::start().await;
        mount_pins_list(
            &server,
            json!([{
                "type": "message",
                "created": 1_755_403_953_i64,
                "created_by": "U1",
                "channel": "C0123456789",
                "message": { "ts": "1755403953.123456", "text": "デプロイ完了" },
            }]),
        )
        .await;
        let client = client_for(&server);

        let out = run_capture(
            PinCommand {
                command: PinSubcommand::List {
                    channel: "C0123456789".to_string(),
                },
            },
            &client,
            &global_with(OutputFormat::Json),
        )
        .await
        .unwrap();
        let parsed: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed[0]["created"], 1_755_403_953_i64);
        assert_eq!(parsed[0]["message"]["text"], "デプロイ完了");

        let out = run_capture(
            PinCommand {
                command: PinSubcommand::List {
                    channel: "C0123456789".to_string(),
                },
            },
            &client,
            &global_with(OutputFormat::Table),
        )
        .await
        .unwrap();
        assert!(out.contains("created_by"), "table was: {out}");
        assert!(out.contains("2025-08-17T04:12:33.000Z"), "table was: {out}");
        assert!(out.contains("デプロイ完了"), "table was: {out}");
    }

    #[tokio::test]
    async fn list_resolves_channel_names_before_calling_the_api() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/conversations.list"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "channels": [{ "id": "C0123456789", "name": "dev-acejob" }],
                "response_metadata": { "next_cursor": "" },
            })))
            .mount(&server)
            .await;
        mount_pins_list(&server, json!([])).await;

        let out = run_capture(
            PinCommand {
                command: PinSubcommand::List {
                    channel: "dev-acejob".to_string(),
                },
            },
            &client_for(&server),
            &global_with(OutputFormat::Table),
        )
        .await
        .unwrap();

        assert_eq!(out.trim(), MSG_NO_PINS);
    }

    #[tokio::test]
    async fn slack_errors_from_pins_list_are_propagated() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/pins.list"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": false,
                "error": "channel_not_found",
            })))
            .mount(&server)
            .await;

        let err = run_capture(
            PinCommand {
                command: PinSubcommand::List {
                    channel: "C0123456789".to_string(),
                },
            },
            &client_for(&server),
            &global_with(OutputFormat::Json),
        )
        .await
        .unwrap_err();

        assert!(err.to_string().contains("channel_not_found"), "{err}");
    }
}
