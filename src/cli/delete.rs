//! `slack-cli delete` — 送信済みメッセージの削除。
//!
//! チャンネル名解決・タイムスタンプ検証・成功出力は `edit` と共通のヘルパを使う。

use clap::Args;
use serde_json::json;

use crate::cli::common::{channel_label as display_channel, report_success, resolve_channel_id};
use crate::cli::edit::{message_result, validate_message_ts};
use crate::cli::GlobalOpts;
use crate::client::SlackClient;
use crate::error::SlackCliError;

#[derive(Args, Debug)]
pub struct DeleteCommand {
    /// Channel name or ID
    #[arg(short, long, required = true, value_name = "CHANNEL")]
    pub channel: String,

    /// Message timestamp to delete
    #[arg(long, required = true, value_name = "TIMESTAMP")]
    pub ts: String,
}

pub async fn run(
    cmd: DeleteCommand,
    client: &SlackClient,
    global: &GlobalOpts,
) -> Result<(), SlackCliError> {
    validate_message_ts(&cmd.ts)?;

    let channel_id = resolve_channel_id(client, &cmd.channel).await?;
    let response = client
        .post_json(
            "chat.delete",
            &json!({ "channel": channel_id, "ts": cmd.ts }),
        )
        .await?;

    report_success(
        global,
        &format!(
            "✓ Message deleted successfully from {}",
            display_channel(&cmd.channel)
        ),
        &message_result(&response, &channel_id, &cmd.ts),
    )
}

#[cfg(test)]
mod tests {
    use clap::error::ErrorKind;
    use clap::Parser;
    use serde_json::Value;
    use url::Url;
    use wiremock::matchers::{body_partial_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;
    use crate::cli::edit::ERR_INVALID_TS;
    use crate::cli::Cli;
    use crate::output::OutputFormat;

    fn client_for(server: &MockServer) -> SlackClient {
        SlackClient::new("test-token-value")
            .unwrap()
            .with_base_url(Url::parse(&format!("{}/", server.uri())).unwrap())
    }

    fn delete_cmd(channel: &str, ts: &str) -> DeleteCommand {
        DeleteCommand {
            channel: channel.to_string(),
            ts: ts.to_string(),
        }
    }

    async fn mount_chat_delete(server: &MockServer) {
        Mock::given(method("POST"))
            .and(path("/chat.delete"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "channel": "C012345678",
                "ts": "1700000000.000100",
            })))
            .mount(server)
            .await;
    }

    #[test]
    fn parses_channel_and_timestamp() {
        let cli = Cli::try_parse_from([
            "slack-cli",
            "delete",
            "-c",
            "C1",
            "--ts",
            "1700000000.000100",
        ])
        .unwrap();
        let crate::cli::Command::Delete(cmd) = cli.command else {
            panic!("expected the delete command");
        };
        assert_eq!(cmd.channel, "C1");
    }

    #[test]
    fn ts_is_required() {
        let err =
            Cli::try_parse_from(["slack-cli", "delete", "-c", "C1"]).expect_err("--ts is required");
        assert_eq!(err.kind(), ErrorKind::MissingRequiredArgument);
    }

    #[tokio::test]
    async fn deletes_by_channel_id_without_resolving() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat.delete"))
            .and(body_partial_json(json!({
                "channel": "C012345678",
                "ts": "1700000000.000100",
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "ok": true })))
            .expect(1)
            .mount(&server)
            .await;

        run(
            delete_cmd("C012345678", "1700000000.000100"),
            &client_for(&server),
            &GlobalOpts::default(),
        )
        .await
        .unwrap();

        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1, "ID 指定では名前解決を挟まない");
    }

    #[tokio::test]
    async fn resolves_the_channel_name_before_deleting() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/conversations.list"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "channels": [{ "id": "C012345678", "name": "general" }],
                "response_metadata": { "next_cursor": "" },
            })))
            .mount(&server)
            .await;
        mount_chat_delete(&server).await;

        run(
            delete_cmd("#general", "1700000000.000100"),
            &client_for(&server),
            &GlobalOpts::default(),
        )
        .await
        .unwrap();

        let requests = server.received_requests().await.unwrap();
        let deleted: Value = serde_json::from_slice(&requests[1].body).unwrap();
        assert_eq!(deleted["channel"], "C012345678");
    }

    #[tokio::test]
    async fn invalid_timestamps_never_reach_the_api() {
        let server = MockServer::start().await;
        let err = run(
            delete_cmd("C012345678", "1700000000"),
            &client_for(&server),
            &GlobalOpts::default(),
        )
        .await
        .unwrap_err();

        assert_eq!(err.to_string(), ERR_INVALID_TS);
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn api_errors_are_propagated() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat.delete"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({ "ok": false, "error": "cant_delete_message" })),
            )
            .mount(&server)
            .await;

        let err = run(
            delete_cmd("C012345678", "1700000000.000100"),
            &client_for(&server),
            &GlobalOpts::default(),
        )
        .await
        .unwrap_err();

        match err {
            SlackCliError::Api { code, .. } => assert_eq!(code, "cant_delete_message"),
            other => panic!("unexpected error: {other}"),
        }
    }

    #[tokio::test]
    async fn json_format_is_honoured() {
        let server = MockServer::start().await;
        mount_chat_delete(&server).await;

        let global = GlobalOpts {
            format: OutputFormat::Json,
            ..GlobalOpts::default()
        };
        run(
            delete_cmd("C012345678", "1700000000.000100"),
            &client_for(&server),
            &global,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn dry_run_never_deletes() {
        let server = MockServer::start().await;
        let client = client_for(&server).with_dry_run(true);

        run(
            delete_cmd("C012345678", "1700000000.000100"),
            &client,
            &GlobalOpts::default(),
        )
        .await
        .unwrap();

        assert!(server.received_requests().await.unwrap().is_empty());
    }
}
