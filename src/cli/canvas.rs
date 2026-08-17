//! `slack-cli canvas` — Canvas の読み取りと一覧。
//!
//! チャンネル名の解決・端末サニタイズ・0 件時の出力は `bookmark` からも使うため、
//! このモジュールに `pub(crate)` で置いてある。

use std::io::Write;

use clap::{Args, Subcommand};
use colored::Colorize;
use serde_json::{json, Value};

use crate::cli::common::resolve_channel_id;
use crate::cli::GlobalOpts;
use crate::client::pagination::PaginationOpts;
use crate::client::SlackClient;
use crate::error::SlackCliError;
use crate::output::sanitize::sanitize_terminal_text;
use crate::output::{self, OutputFormat};

pub const MSG_NO_SECTIONS: &str = "No sections found in canvas";
pub const MSG_NO_CANVASES: &str = "No canvases found in channel";

const NO_ID: &str = "(no id)";
const NO_CONTENT: &str = "(no content)";
const NO_NAME: &str = "(no name)";

const SECTIONS_METHOD: &str = "canvases.sections.lookup";
const FILES_LIST_METHOD: &str = "files.list";


#[derive(Args, Debug)]
pub struct CanvasCommand {
    #[command(subcommand)]
    pub command: CanvasSubcommand,
}

#[derive(Subcommand, Debug)]
pub enum CanvasSubcommand {
    /// Get the sections of a Canvas
    Read {
        /// Canvas ID
        #[arg(short, long, required = true, value_name = "CANVAS_ID")]
        id: String,
    },
    /// List canvases linked to a channel
    List {
        /// Channel name or ID
        #[arg(short, long, required = true, value_name = "CHANNEL")]
        channel: String,
    },
}

pub async fn run(
    cmd: CanvasCommand,
    client: &SlackClient,
    global: &GlobalOpts,
) -> Result<(), SlackCliError> {
    run_to(cmd, client, global, &mut std::io::stdout()).await
}

async fn run_to(
    cmd: CanvasCommand,
    client: &SlackClient,
    global: &GlobalOpts,
    writer: &mut (dyn Write + Send),
) -> Result<(), SlackCliError> {
    let format = global.output_format();

    match cmd.command {
        CanvasSubcommand::Read { id } => {
            let body = json!({
                "canvas_id": id,
                "criteria": { "section_types": ["any_header"] },
            });
            let response = client.post_json(SECTIONS_METHOD, &body).await?;
            let sections = array_field(&response, "sections");

            if sections.is_empty() {
                return write_empty(MSG_NO_SECTIONS, format, writer);
            }

            let value = if keeps_raw_payload(format) {
                Value::Array(sections)
            } else {
                Value::Array(sections.iter().map(section_row).collect())
            };
            output::format_value(&value, format, writer)
        }

        CanvasSubcommand::List { channel } => {
            let channel_id = resolve_channel_id(client, &channel).await?;
            let files = client
                .paginate_get(
                    FILES_LIST_METHOD,
                    &[("channel", channel_id.as_str()), ("types", "spaces")],
                    "files",
                    &PaginationOpts::all(),
                )
                .await?;

            if files.is_empty() {
                return write_empty(MSG_NO_CANVASES, format, writer);
            }

            let value = if keeps_raw_payload(format) {
                Value::Array(files)
            } else {
                Value::Array(files.iter().map(canvas_row).collect())
            };
            output::format_value(&value, format, writer)
        }
    }
}

/// 0 件の出力。人間向けの `table` だけ文言を出し、機械可読なフォーマットでは空配列を返す
/// （移植方針 G14。TS 版は `--format json` でも文言を出していた）。
pub(crate) fn write_empty(
    message: &str,
    format: OutputFormat,
    writer: &mut (dyn Write + Send),
) -> Result<(), SlackCliError> {
    if format == OutputFormat::Table {
        writeln!(writer, "{}", message.yellow())?;
        return Ok(());
    }
    output::format_value(&Value::Array(Vec::new()), format, writer)
}

/// API レスポンスの構造をそのまま渡すフォーマットか。
/// `json` / `yaml` は機械が読む契約なので Slack のオブジェクトをそのまま出し、
/// 表・CSV 系は表示用に平らな行へ畳む。
pub(crate) fn keeps_raw_payload(format: OutputFormat) -> bool {
    matches!(format, OutputFormat::Json | OutputFormat::Yaml)
}

fn array_field(value: &Value, key: &str) -> Vec<Value> {
    value
        .get(key)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

fn section_row(section: &Value) -> Value {
    let content = section
        .get("elements")
        .and_then(Value::as_array)
        .map(|elements| elements.iter().map(element_text).collect::<String>())
        .unwrap_or_default();

    json!({
        "id": text_or(section.get("id"), NO_ID),
        "content": if content.is_empty() { NO_CONTENT.to_string() } else { content },
    })
}

fn canvas_row(file: &Value) -> Value {
    json!({
        "id": text_or(file.get("id"), NO_ID),
        "name": text_or(file.get("name"), NO_NAME),
    })
}

/// Canvas のセクション要素からテキストを取り出す。`text` があればそれ、
/// 無ければ子 `elements` を再帰し、区切りなしで連結する。
fn element_text(element: &Value) -> String {
    if let Some(text) = element.get("text").and_then(Value::as_str) {
        return sanitize_terminal_text(text);
    }
    element
        .get("elements")
        .and_then(Value::as_array)
        .map(|children| children.iter().map(element_text).collect())
        .unwrap_or_default()
}

fn text_or(value: Option<&Value>, fallback: &str) -> String {
    value
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(sanitize_terminal_text)
        .unwrap_or_else(|| fallback.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::error::ErrorKind;
    use clap::Parser;
    use url::Url;
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use crate::cli::Cli;

    fn client_for(server: &MockServer) -> SlackClient {
        SlackClient::new("test-token-value")
            .unwrap()
            .with_base_url(Url::parse(&format!("{}/", server.uri())).unwrap())
    }

    fn opts(format: OutputFormat) -> GlobalOpts {
        GlobalOpts {
            format,
            ..GlobalOpts::default()
        }
    }

    async fn execute(
        command: CanvasSubcommand,
        client: &SlackClient,
        format: OutputFormat,
    ) -> Result<String, SlackCliError> {
        let mut buf: Vec<u8> = Vec::new();
        run_to(CanvasCommand { command }, client, &opts(format), &mut buf).await?;
        Ok(String::from_utf8(buf).unwrap())
    }

    async fn mount_json(server: &MockServer, endpoint: &str, body: Value) {
        Mock::given(path(format!("/{endpoint}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .mount(server)
            .await;
    }

    #[test]
    fn read_takes_a_canvas_id() {
        let cli = Cli::try_parse_from(["slack-cli", "canvas", "read", "-i", "F123"]).unwrap();
        let crate::cli::Command::Canvas(cmd) = cli.command else {
            panic!("expected the canvas command");
        };
        let CanvasSubcommand::Read { id } = cmd.command else {
            panic!("expected canvas read");
        };
        assert_eq!(id, "F123");
    }

    #[test]
    fn both_subcommands_require_their_key() {
        for argv in [
            vec!["slack-cli", "canvas", "read"],
            vec!["slack-cli", "canvas", "list"],
        ] {
            let err = Cli::try_parse_from(&argv).expect_err("a required flag is missing");
            assert_eq!(err.kind(), ErrorKind::MissingRequiredArgument);
        }
    }

    #[tokio::test]
    async fn read_sends_the_any_header_criteria() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/canvases.sections.lookup"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "sections": [{ "id": "temp:C:abc", "elements": [{ "type": "text", "text": "概要" }] }],
            })))
            .expect(1)
            .mount(&server)
            .await;

        let client = client_for(&server);
        execute(
            CanvasSubcommand::Read { id: "F1".into() },
            &client,
            OutputFormat::Table,
        )
        .await
        .unwrap();

        let requests = server.received_requests().await.unwrap();
        let body: Value = serde_json::from_slice(&requests[0].body).unwrap();
        assert_eq!(body["canvas_id"], "F1");
        assert_eq!(body["criteria"]["section_types"], json!(["any_header"]));
    }

    #[tokio::test]
    async fn read_flattens_sections_for_the_table_format() {
        let server = MockServer::start().await;
        mount_json(
            &server,
            "canvases.sections.lookup",
            json!({
                "ok": true,
                "sections": [
                    { "id": "temp:C:abc", "elements": [
                        { "type": "text", "text": "プロジェクト" },
                        { "type": "rich", "elements": [{ "text": "概要" }] },
                    ]},
                    { "elements": [] },
                ],
            }),
        )
        .await;

        let out = execute(
            CanvasSubcommand::Read { id: "F1".into() },
            &client_for(&server),
            OutputFormat::Table,
        )
        .await
        .unwrap();

        assert!(out.contains("プロジェクト概要"), "{out}");
        assert!(out.contains(NO_ID), "{out}");
        assert!(out.contains(NO_CONTENT), "{out}");
    }

    #[tokio::test]
    async fn read_keeps_the_raw_sections_for_json() {
        let server = MockServer::start().await;
        mount_json(
            &server,
            "canvases.sections.lookup",
            json!({
                "ok": true,
                "sections": [{ "id": "s1", "elements": [{ "type": "text", "text": "hi" }] }],
            }),
        )
        .await;

        let out = execute(
            CanvasSubcommand::Read { id: "F1".into() },
            &client_for(&server),
            OutputFormat::Json,
        )
        .await
        .unwrap();

        let parsed: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed[0]["elements"][0]["type"], "text");
    }

    #[tokio::test]
    async fn empty_results_stay_machine_readable_in_json() {
        let server = MockServer::start().await;
        mount_json(
            &server,
            "canvases.sections.lookup",
            json!({ "ok": true, "sections": [] }),
        )
        .await;
        let client = client_for(&server);

        let json_out = execute(
            CanvasSubcommand::Read { id: "F1".into() },
            &client,
            OutputFormat::Json,
        )
        .await
        .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&json_out).unwrap(),
            json!([]),
            "{json_out}"
        );

        let table_out = execute(
            CanvasSubcommand::Read { id: "F1".into() },
            &client,
            OutputFormat::Table,
        )
        .await
        .unwrap();
        assert!(table_out.contains(MSG_NO_SECTIONS), "{table_out}");
    }

    #[tokio::test]
    async fn api_errors_are_propagated() {
        let server = MockServer::start().await;
        mount_json(
            &server,
            "canvases.sections.lookup",
            json!({ "ok": false, "error": "canvas_not_found" }),
        )
        .await;

        let err = execute(
            CanvasSubcommand::Read { id: "F1".into() },
            &client_for(&server),
            OutputFormat::Json,
        )
        .await
        .unwrap_err();

        assert!(
            matches!(&err, SlackCliError::Api { code, .. } if code == "canvas_not_found"),
            "{err}"
        );
    }

    #[tokio::test]
    async fn list_skips_resolution_when_given_a_channel_id() {
        let server = MockServer::start().await;
        Mock::given(path("/files.list"))
            .and(query_param("channel", "C0123456789"))
            .and(query_param("types", "spaces"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "files": [{ "id": "F1", "name": "週次メモ" }],
            })))
            .expect(1)
            .mount(&server)
            .await;

        let out = execute(
            CanvasSubcommand::List {
                channel: "C0123456789".into(),
            },
            &client_for(&server),
            OutputFormat::Table,
        )
        .await
        .unwrap();

        assert!(out.contains("週次メモ"), "{out}");
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn list_resolves_a_channel_name_before_calling_files_list() {
        let server = MockServer::start().await;
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
        Mock::given(path("/files.list"))
            .and(query_param("channel", "C0123456789"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "files": [{ "id": "F1", "name": "memo" }],
                "response_metadata": { "next_cursor": "" },
            })))
            .expect(1)
            .mount(&server)
            .await;

        let out = execute(
            CanvasSubcommand::List {
                channel: "#general".into(),
            },
            &client_for(&server),
            OutputFormat::Json,
        )
        .await
        .unwrap();

        let parsed: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed[0]["id"], "F1");
    }

    #[tokio::test]
    async fn unknown_channel_names_suggest_close_matches() {
        let server = MockServer::start().await;
        mount_json(
            &server,
            "conversations.list",
            json!({
                "ok": true,
                "channels": [
                    { "id": "C0123456789", "name": "dev-acejob" },
                    { "id": "C0123456780", "name": "random" },
                ],
                "response_metadata": { "next_cursor": "" },
            }),
        )
        .await;

        let err = execute(
            CanvasSubcommand::List {
                channel: "dev".into(),
            },
            &client_for(&server),
            OutputFormat::Table,
        )
        .await
        .unwrap_err();

        assert_eq!(
            err.to_string(),
            "Channel 'dev' not found. Did you mean one of these? dev-acejob"
        );
    }

    #[tokio::test]
    async fn missing_scope_retries_without_the_unreadable_types() {
        let server = MockServer::start().await;
        Mock::given(path("/conversations.list"))
            .and(query_param(
                "types",
                "public_channel,private_channel,im,mpim",
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": false,
                "error": "missing_scope",
                "needed": "groups:read, mpim:read",
            })))
            .mount(&server)
            .await;
        Mock::given(path("/conversations.list"))
            .and(query_param("types", "public_channel,im"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "channels": [{ "id": "C0123456789", "name": "general" }],
                "response_metadata": { "next_cursor": "" },
            })))
            .expect(1)
            .mount(&server)
            .await;

        let resolved = resolve_channel_id(&client_for(&server), "general")
            .await
            .unwrap();
        assert_eq!(resolved, "C0123456789");
    }

    #[tokio::test]
    async fn missing_scope_is_rethrown_when_no_type_can_be_dropped() {
        let server = MockServer::start().await;
        mount_json(
            &server,
            "conversations.list",
            json!({ "ok": false, "error": "missing_scope", "needed": "chat:write" }),
        )
        .await;

        let err = resolve_channel_id(&client_for(&server), "general")
            .await
            .unwrap_err();
        assert!(
            matches!(&err, SlackCliError::Api { code, .. } if code == crate::cli::common::MISSING_SCOPE_CODE),
            "{err}"
        );
    }
    #[test]
    fn sanitize_drops_escape_sequences_but_keeps_tabs_and_newlines() {
        assert_eq!(sanitize_terminal_text("\u{1b}[31mred\u{1b}[0m"), "red");
        assert_eq!(
            sanitize_terminal_text("\u{1b}]0;title\u{7}general"),
            "general"
        );
        assert_eq!(sanitize_terminal_text("a\tb\nc"), "a\tb\nc");
        assert_eq!(sanitize_terminal_text("a\u{7f}\u{9b}b"), "ab");
        assert_eq!(sanitize_terminal_text("絵文字🎉"), "絵文字🎉");
    }
    #[test]
    fn section_text_is_concatenated_without_separators() {
        let section = json!({
            "id": "s1",
            "elements": [
                { "text": "前半" },
                { "elements": [{ "text": "後半" }] },
                { "type": "image" },
            ],
        });
        assert_eq!(section_row(&section)["content"], "前半後半");
    }
}
