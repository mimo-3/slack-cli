//! `slack-cli edit` — 送信済みメッセージの編集。
//!
//! タイムスタンプ検証と成功出力のヘルパは `delete` と共有するためここに置いてある。
//! チャンネル名解決と端末サニタイズは `cli::common` / `output::sanitize` にある。

use clap::Args;
use serde_json::{json, Value};

use crate::cli::common::{
    channel_label as display_channel, report_success, resolve_channel_id, ERR_INVALID_MESSAGE_TS,
};
use crate::cli::GlobalOpts;
use crate::client::SlackClient;
use crate::error::SlackCliError;

pub const ERR_INVALID_TS: &str = ERR_INVALID_MESSAGE_TS;
pub const ERR_MESSAGE_OR_FILE: &str = "You must specify either --message or --file";
pub const ERR_BOTH_MESSAGE_AND_FILE: &str = "Cannot use both --message and --file";
pub const ERR_BOTH_BLOCKS: &str = "Cannot use both --blocks and --blocks-file";
pub const ERR_INVALID_BLOCKS_JSON: &str = "Invalid blocks JSON: must be a valid JSON array";
const ERR_BLOCKS_NOT_ARRAY: &str = "blocks must be a JSON array";

#[derive(Args, Debug)]
pub struct EditCommand {
    /// Channel name or ID
    #[arg(short, long, required = true, value_name = "CHANNEL")]
    pub channel: String,

    /// Message timestamp to edit
    #[arg(long, required = true, value_name = "TIMESTAMP")]
    pub ts: String,

    /// New message text
    #[arg(short, long, value_name = "MESSAGE")]
    pub message: Option<String>,

    /// File containing new message text
    #[arg(short, long, value_name = "FILE")]
    pub file: Option<String>,

    /// Block Kit JSON array string
    #[arg(short, long, value_name = "JSON")]
    pub blocks: Option<String>,

    /// File containing Block Kit JSON array
    #[arg(long, value_name = "FILE")]
    pub blocks_file: Option<String>,
}

pub async fn run(
    cmd: EditCommand,
    client: &SlackClient,
    global: &GlobalOpts,
) -> Result<(), SlackCliError> {
    // 検証順は TS 版の preAction と同じ（editTimestamp → messageOrFile → blocksOption）。
    validate_message_ts(&cmd.ts)?;
    validate_message_or_file(
        cmd.message.as_deref(),
        cmd.file.as_deref(),
        cmd.blocks.as_deref(),
        cmd.blocks_file.as_deref(),
    )?;
    let inline_blocks = validate_blocks_options(cmd.blocks.as_deref(), cmd.blocks_file.as_deref())?;

    let text = match cmd.file.as_deref() {
        Some(path) => read_text_file(path)?,
        None => cmd.message.clone().unwrap_or_default(),
    };

    let blocks = match cmd.blocks_file.as_deref() {
        Some(path) => Some(read_blocks_file(path)?),
        None => inline_blocks,
    };

    let channel_id = resolve_channel_id(client, &cmd.channel).await?;

    let mut body = json!({ "channel": channel_id, "ts": cmd.ts, "text": text });
    // blocks は存在するときだけキーを付ける（空配列とキー無しは別物）。
    if let Some(blocks) = blocks {
        body["blocks"] = blocks;
    }

    let response = client.post_json("chat.update", &body).await?;
    report_success(
        global,
        &format!(
            "✓ Message updated successfully in {}",
            display_channel(&cmd.channel)
        ),
        &message_result(&response, &channel_id, &cmd.ts),
    )
}

/// `^\d{10}\.\d{6}$` 相当の厳格な検証。
pub(crate) fn validate_message_ts(ts: &str) -> Result<(), SlackCliError> {
    let is_digits = |s: &str, len: usize| s.len() == len && s.bytes().all(|b| b.is_ascii_digit());
    let valid = match ts.split_once('.') {
        Some((seconds, micros)) => is_digits(seconds, 10) && is_digits(micros, 6),
        None => false,
    };

    if valid {
        Ok(())
    } else {
        Err(SlackCliError::Validation(ERR_INVALID_TS.to_string()))
    }
}

fn validate_message_or_file(
    message: Option<&str>,
    file: Option<&str>,
    blocks: Option<&str>,
    blocks_file: Option<&str>,
) -> Result<(), SlackCliError> {
    if message.is_some() && file.is_some() {
        return Err(SlackCliError::Validation(
            ERR_BOTH_MESSAGE_AND_FILE.to_string(),
        ));
    }
    if message.is_none() && file.is_none() && blocks.is_none() && blocks_file.is_none() {
        return Err(SlackCliError::Validation(ERR_MESSAGE_OR_FILE.to_string()));
    }
    Ok(())
}

/// `--blocks` / `--blocks-file` の排他と、`--blocks` の JSON 配列としての妥当性を見る。
/// 検証時にパースした値をそのまま返し、二重パースを避ける。
fn validate_blocks_options(
    blocks: Option<&str>,
    blocks_file: Option<&str>,
) -> Result<Option<Value>, SlackCliError> {
    if blocks.is_some() && blocks_file.is_some() {
        return Err(SlackCliError::Validation(ERR_BOTH_BLOCKS.to_string()));
    }

    let Some(raw) = blocks else {
        return Ok(None);
    };

    let parsed: Value = serde_json::from_str(raw)
        .map_err(|_| SlackCliError::Validation(ERR_INVALID_BLOCKS_JSON.to_string()))?;
    if !parsed.is_array() {
        return Err(SlackCliError::Validation(
            ERR_INVALID_BLOCKS_JSON.to_string(),
        ));
    }
    Ok(Some(parsed))
}

/// 本文ファイルの読み込み。不正な UTF-8 は Node と同じく U+FFFD に置換して読む。
fn read_text_file(path: &str) -> Result<String, SlackCliError> {
    let bytes = std::fs::read(path)
        .map_err(|e| SlackCliError::File(format!("Error reading file {path}: {e}")))?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn read_blocks_file(path: &str) -> Result<Value, SlackCliError> {
    let bytes = std::fs::read(path)
        .map_err(|e| SlackCliError::File(format!("Error reading blocks file {path}: {e}")))?;

    let parsed: Value = serde_json::from_slice(&bytes)
        .map_err(|_| SlackCliError::File(ERR_INVALID_BLOCKS_JSON.to_string()))?;
    if !parsed.is_array() {
        return Err(SlackCliError::File(format!(
            "Error reading blocks file {path}: {ERR_BLOCKS_NOT_ARRAY}"
        )));
    }
    Ok(parsed)
}

/// 書き込み系の結果。API が返した値を優先し、dry-run のように欠けていれば入力値で埋める。
pub(crate) fn message_result(response: &Value, channel_id: &str, ts: &str) -> Value {
    json!({
        "channel": response.get("channel").and_then(Value::as_str).unwrap_or(channel_id),
        "ts": response.get("ts").and_then(Value::as_str).unwrap_or(ts),
    })
}

#[cfg(test)]
mod tests {
    use crate::output::{self, OutputFormat};
    use clap::error::ErrorKind;
    use clap::Parser;
    use serde_json::json;
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

    fn edit_cmd(channel: &str, ts: &str, message: Option<&str>) -> EditCommand {
        EditCommand {
            channel: channel.to_string(),
            ts: ts.to_string(),
            message: message.map(str::to_string),
            file: None,
            blocks: None,
            blocks_file: None,
        }
    }

    async fn mount_chat_update(server: &MockServer) {
        Mock::given(method("POST"))
            .and(path("/chat.update"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "channel": "C012345678",
                "ts": "1700000000.000100",
                "text": "fixed",
            })))
            .mount(server)
            .await;
    }

    async fn mount_channel_list(server: &MockServer) {
        Mock::given(method("GET"))
            .and(path("/conversations.list"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "channels": [
                    { "id": "C012345678", "name": "general", "name_normalized": "general" },
                    { "id": "C087654321", "name": "general-random" },
                ],
                "response_metadata": { "next_cursor": "" },
            })))
            .mount(server)
            .await;
    }

    #[test]
    fn parses_the_full_invocation() {
        let cli = Cli::try_parse_from([
            "slack-cli",
            "edit",
            "-c",
            "C1",
            "--ts",
            "1700000000.000100",
            "-m",
            "fixed",
            "--blocks-file",
            "b.json",
        ])
        .unwrap();

        let crate::cli::Command::Edit(cmd) = cli.command else {
            panic!("expected the edit command");
        };
        assert_eq!(cmd.ts, "1700000000.000100");
        assert_eq!(cmd.blocks_file.as_deref(), Some("b.json"));
    }

    #[test]
    fn channel_and_ts_are_required() {
        let err =
            Cli::try_parse_from(["slack-cli", "edit", "-c", "C1"]).expect_err("--ts is required");
        assert_eq!(err.kind(), ErrorKind::MissingRequiredArgument);
    }

    #[test]
    fn message_timestamps_must_be_ten_dot_six_digits() {
        validate_message_ts("1700000000.000100").unwrap();
        for bad in [
            "1700000000",
            "1700000000.0001",
            "170000000.000100",
            "1700000000.0001000",
            "17000000000.000100",
            "abcdefghij.000100",
            " 1700000000.000100",
            "1700000000.000100 ",
            "1700000000.00010a",
            "",
        ] {
            assert_eq!(
                validate_message_ts(bad).unwrap_err().to_string(),
                ERR_INVALID_TS,
                "{bad:?} should have been rejected"
            );
        }
    }
    #[test]
    fn message_or_file_rules_match_the_typescript_validators() {
        assert_eq!(
            validate_message_or_file(Some("m"), Some("f"), None, None)
                .unwrap_err()
                .to_string(),
            ERR_BOTH_MESSAGE_AND_FILE
        );
        assert_eq!(
            validate_message_or_file(None, None, None, None)
                .unwrap_err()
                .to_string(),
            ERR_MESSAGE_OR_FILE
        );
        // blocks だけの指定は許される
        validate_message_or_file(None, None, Some("[]"), None).unwrap();
        validate_message_or_file(None, None, None, Some("b.json")).unwrap();
    }

    #[test]
    fn blocks_options_are_exclusive_and_must_be_a_json_array() {
        assert_eq!(
            validate_blocks_options(Some("[]"), Some("b.json"))
                .unwrap_err()
                .to_string(),
            ERR_BOTH_BLOCKS
        );
        for bad in ["{}", "not json", "\"text\"", "3"] {
            assert_eq!(
                validate_blocks_options(Some(bad), None)
                    .unwrap_err()
                    .to_string(),
                ERR_INVALID_BLOCKS_JSON,
                "{bad:?} should have been rejected"
            );
        }
        assert_eq!(
            validate_blocks_options(Some("[{\"type\":\"divider\"}]"), None).unwrap(),
            Some(json!([{ "type": "divider" }]))
        );
        assert_eq!(validate_blocks_options(None, None).unwrap(), None);
    }

    #[test]
    fn blocks_file_errors_distinguish_syntax_from_shape() {
        let dir = tempfile::tempdir().unwrap();

        let syntax = dir.path().join("syntax.json");
        std::fs::write(&syntax, "[").unwrap();
        assert_eq!(
            read_blocks_file(syntax.to_str().unwrap())
                .unwrap_err()
                .to_string(),
            ERR_INVALID_BLOCKS_JSON
        );

        let object = dir.path().join("object.json");
        std::fs::write(&object, "{}").unwrap();
        let path = object.to_str().unwrap().to_string();
        assert_eq!(
            read_blocks_file(&path).unwrap_err().to_string(),
            format!("Error reading blocks file {path}: {ERR_BLOCKS_NOT_ARRAY}")
        );

        let missing = dir.path().join("missing.json");
        let missing = missing.to_str().unwrap().to_string();
        assert!(read_blocks_file(&missing)
            .unwrap_err()
            .to_string()
            .starts_with(&format!("Error reading blocks file {missing}: ")));
    }

    #[test]
    fn text_files_are_read_leniently() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("body.txt");
        std::fs::write(&file, [b'h', b'i', 0xff]).unwrap();
        assert_eq!(
            read_text_file(file.to_str().unwrap()).unwrap(),
            "hi\u{fffd}"
        );

        let missing = dir.path().join("nope.txt");
        let missing = missing.to_str().unwrap().to_string();
        assert!(read_text_file(&missing)
            .unwrap_err()
            .to_string()
            .starts_with(&format!("Error reading file {missing}: ")));
    }

    #[tokio::test]
    async fn channel_ids_are_used_without_calling_the_api() {
        let server = MockServer::start().await;
        let resolved = resolve_channel_id(&client_for(&server), "C012345678")
            .await
            .unwrap();

        assert_eq!(resolved, "C012345678");
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn channel_names_are_resolved_through_conversations_list() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/conversations.list"))
            .and(query_param(
                "types",
                "public_channel,private_channel,im,mpim",
            ))
            .and(query_param("exclude_archived", "true"))
            .and(query_param("limit", "1000"))
            .and(query_param_is_missing("cursor"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "channels": [{ "id": "C011111111", "name": "random" }],
                "response_metadata": { "next_cursor": "page2" },
            })))
            .mount(&server)
            .await;
        Mock::given(query_param("cursor", "page2"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "channels": [{ "id": "C012345678", "name": "general" }],
                "response_metadata": { "next_cursor": "" },
            })))
            .mount(&server)
            .await;

        let client = client_for(&server);
        // 2 ページ目まで辿ってから一致させる
        assert_eq!(
            resolve_channel_id(&client, "general").await.unwrap(),
            "C012345678"
        );
        // `#` 付き・大文字小文字違いも同じチャンネルに解決される
        assert_eq!(
            resolve_channel_id(&client, "#general").await.unwrap(),
            "C012345678"
        );
        assert_eq!(
            resolve_channel_id(&client, "GENERAL").await.unwrap(),
            "C012345678"
        );
    }

    #[tokio::test]
    async fn unknown_channels_report_similar_names() {
        let server = MockServer::start().await;
        mount_channel_list(&server).await;

        let err = resolve_channel_id(&client_for(&server), "genera")
            .await
            .unwrap_err();
        // 人間向けの文章はそのまま出す。`API Error:` は Slack のエラーコード用の前置き
        assert_eq!(
            err.to_string(),
            "Channel 'genera' not found. Did you mean one of these? general, general-random"
        );
        assert_eq!(err.code(), Some(crate::error::CODE_API));
    }

    #[tokio::test]
    async fn unknown_channels_without_candidates_suggest_membership() {
        let server = MockServer::start().await;
        mount_channel_list(&server).await;

        let err = resolve_channel_id(&client_for(&server), "nowhere")
            .await
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "Channel 'nowhere' not found. Make sure you are a member of this channel."
        );
    }

    #[tokio::test]
    async fn missing_scope_retries_with_the_readable_channel_types() {
        let server = MockServer::start().await;
        Mock::given(query_param(
            "types",
            "public_channel,private_channel,im,mpim",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "ok": false,
            "error": "missing_scope",
            "needed": "im:read,mpim:read",
        })))
        .mount(&server)
        .await;
        Mock::given(query_param("types", "public_channel,private_channel"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "channels": [{ "id": "C012345678", "name": "general" }],
                "response_metadata": { "next_cursor": "" },
            })))
            .expect(1)
            .mount(&server)
            .await;

        assert_eq!(
            resolve_channel_id(&client_for(&server), "general")
                .await
                .unwrap(),
            "C012345678"
        );
    }

    #[tokio::test]
    async fn edit_updates_the_message_with_the_resolved_channel() {
        let server = MockServer::start().await;
        mount_channel_list(&server).await;
        Mock::given(method("POST"))
            .and(path("/chat.update"))
            .and(body_partial_json(json!({
                "channel": "C012345678",
                "ts": "1700000000.000100",
                "text": "fixed",
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "channel": "C012345678",
                "ts": "1700000000.000100",
            })))
            .expect(1)
            .mount(&server)
            .await;

        run(
            edit_cmd("general", "1700000000.000100", Some("fixed")),
            &client_for(&server),
            &GlobalOpts::default(),
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn edit_sends_blocks_only_when_they_were_given() {
        let server = MockServer::start().await;
        mount_chat_update(&server).await;
        let client = client_for(&server);

        let mut cmd = edit_cmd("C012345678", "1700000000.000100", Some("fixed"));
        cmd.blocks = Some("[{\"type\":\"divider\"}]".to_string());
        run(cmd, &client, &GlobalOpts::default()).await.unwrap();

        run(
            edit_cmd("C012345678", "1700000000.000100", Some("fixed")),
            &client,
            &GlobalOpts::default(),
        )
        .await
        .unwrap();

        let requests = server.received_requests().await.unwrap();
        let with_blocks: Value = serde_json::from_slice(&requests[0].body).unwrap();
        let without_blocks: Value = serde_json::from_slice(&requests[1].body).unwrap();
        assert_eq!(with_blocks["blocks"], json!([{ "type": "divider" }]));
        assert!(
            without_blocks.get("blocks").is_none(),
            "blocks キーは指定が無いとき送ってはいけない"
        );
    }

    #[tokio::test]
    async fn edit_with_blocks_only_sends_an_empty_text() {
        let server = MockServer::start().await;
        mount_chat_update(&server).await;

        let mut cmd = edit_cmd("C012345678", "1700000000.000100", None);
        cmd.blocks = Some("[]".to_string());
        run(cmd, &client_for(&server), &GlobalOpts::default())
            .await
            .unwrap();

        let requests = server.received_requests().await.unwrap();
        let body: Value = serde_json::from_slice(&requests[0].body).unwrap();
        assert_eq!(body["text"], "");
    }

    #[tokio::test]
    async fn edit_rejects_bad_arguments_before_calling_the_api() {
        let server = MockServer::start().await;
        let client = client_for(&server);
        let global = GlobalOpts::default();

        let cases: Vec<(EditCommand, &str)> = vec![
            (edit_cmd("C012345678", "nope", Some("m")), ERR_INVALID_TS),
            (
                edit_cmd("C012345678", "1700000000.000100", None),
                ERR_MESSAGE_OR_FILE,
            ),
            (
                EditCommand {
                    file: Some("body.txt".into()),
                    ..edit_cmd("C012345678", "1700000000.000100", Some("m"))
                },
                ERR_BOTH_MESSAGE_AND_FILE,
            ),
            (
                EditCommand {
                    blocks: Some("[]".into()),
                    blocks_file: Some("b.json".into()),
                    ..edit_cmd("C012345678", "1700000000.000100", Some("m"))
                },
                ERR_BOTH_BLOCKS,
            ),
            (
                EditCommand {
                    blocks: Some("{}".into()),
                    ..edit_cmd("C012345678", "1700000000.000100", Some("m"))
                },
                ERR_INVALID_BLOCKS_JSON,
            ),
        ];

        for (cmd, expected) in cases {
            let err = run(cmd, &client, &global).await.unwrap_err();
            assert_eq!(err.to_string(), expected);
        }
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn edit_propagates_api_errors() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat.update"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({ "ok": false, "error": "message_not_found" })),
            )
            .mount(&server)
            .await;

        let err = run(
            edit_cmd("C012345678", "1700000000.000100", Some("fixed")),
            &client_for(&server),
            &GlobalOpts::default(),
        )
        .await
        .unwrap_err();

        match err {
            SlackCliError::Api { code, .. } => assert_eq!(code, "message_not_found"),
            other => panic!("unexpected error: {other}"),
        }
    }

    #[tokio::test]
    async fn dry_run_resolves_the_channel_but_never_updates() {
        let server = MockServer::start().await;
        mount_channel_list(&server).await;

        let client = client_for(&server).with_dry_run(true);
        run(
            edit_cmd("general", "1700000000.000100", Some("fixed")),
            &client,
            &GlobalOpts::default(),
        )
        .await
        .unwrap();

        let requests = server.received_requests().await.unwrap();
        assert!(requests.iter().all(|r| r.method.as_str() == "GET"));
    }

    #[test]
    fn non_table_formats_emit_the_structured_result() {
        let response = json!({ "ok": true, "channel": "C012345678", "ts": "1700000000.000100" });
        let value = message_result(&response, "C012345678", "1700000000.000100");

        let mut buf = Vec::new();
        output::format_value(&value, OutputFormat::Json, &mut buf).unwrap();
        let rendered: Value = serde_json::from_slice(&buf).unwrap();
        assert_eq!(rendered["channel"], "C012345678");
        assert_eq!(rendered["ts"], "1700000000.000100");

        // dry-run のように API 側が値を返さない場合は入力値で埋める
        let fallback = message_result(&json!({ "ok": true, "dry_run": true }), "C1", "1.2");
        assert_eq!(fallback["channel"], "C1");
        assert_eq!(fallback["ts"], "1.2");
    }
}
