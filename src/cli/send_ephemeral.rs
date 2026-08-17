//! `slack-cli send-ephemeral` — 特定ユーザーにだけ見えるメッセージの送信。
//!
//! `--user` は Slack のユーザー ID をそのまま渡す（TypeScript 版と同じ。`send --user` の
//! ような名前解決は行わない）。チャンネルは移植方針 G1 のフォールバック解決を通す。

use clap::Args;
use serde_json::{json, Value};

use crate::cli::send::{
    finish, post_with_channel_fallback, validate_thread_ts,
};
use crate::cli::common::channel_label as format_channel_label;
use crate::cli::GlobalOpts;
use crate::client::SlackClient;
use crate::error::SlackCliError;

const API_METHOD: &str = "chat.postEphemeral";

#[derive(Args, Debug)]
pub struct SendEphemeralCommand {
    /// Target channel name or ID
    #[arg(short, long, required = true, value_name = "CHANNEL")]
    pub channel: String,

    /// User ID who will see the ephemeral message
    #[arg(short, long, required = true, value_name = "USER")]
    pub user: String,

    /// Message to send
    #[arg(short, long, required = true, value_name = "MESSAGE")]
    pub message: String,

    /// Thread timestamp to reply to
    #[arg(short, long, value_name = "THREAD")]
    pub thread: Option<String>,
}

pub async fn run(
    cmd: SendEphemeralCommand,
    client: &SlackClient,
    global: &GlobalOpts,
) -> Result<(), SlackCliError> {
    validate_thread_ts(cmd.thread.as_deref())?;

    let mut body = json!({
        "channel": cmd.channel,
        "user": cmd.user,
        "text": cmd.message,
    });
    if let Some(thread) = &cmd.thread {
        if let Some(object) = body.as_object_mut() {
            object.insert("thread_ts".to_string(), Value::String(thread.clone()));
        }
    }

    let response = post_with_channel_fallback(client, API_METHOD, &body, &cmd.channel).await?;

    let message = format!(
        "✓ Ephemeral message sent to {}",
        format_channel_label(&cmd.channel)
    );
    finish(global, &message, &response)
}

#[cfg(test)]
mod tests {
    use clap::error::ErrorKind;
    use clap::Parser;
    use url::Url;
    use wiremock::matchers::{body_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;
    use crate::cli::send::ERR_INVALID_THREAD_TS;
    use crate::cli::Cli;

    fn client_for(server: &MockServer) -> SlackClient {
        SlackClient::new("test-token-value")
            .unwrap()
            .with_base_url(Url::parse(&format!("{}/", server.uri())).unwrap())
    }

    fn parse(argv: &[&str]) -> SendEphemeralCommand {
        let cli = Cli::try_parse_from(argv).expect("arguments should parse");
        match cli.command {
            crate::cli::Command::SendEphemeral(cmd) => cmd,
            _ => panic!("expected the send-ephemeral command"),
        }
    }

    #[test]
    fn parses_the_full_invocation() {
        let cmd = parse(&[
            "slack-cli",
            "send-ephemeral",
            "-c",
            "C1",
            "-u",
            "U1",
            "-m",
            "hi",
            "-t",
            "1700000000.000100",
        ]);
        assert_eq!(cmd.user, "U1");
        assert_eq!(cmd.thread.as_deref(), Some("1700000000.000100"));
    }

    #[test]
    fn channel_user_and_message_are_required() {
        // TS 版は手書きバリデータで必須にしていたが、Rust 版は clap に寄せる（移植方針 G11）
        for argv in [
            vec!["slack-cli", "send-ephemeral", "-u", "U1", "-m", "hi"],
            vec!["slack-cli", "send-ephemeral", "-c", "C1", "-m", "hi"],
            vec!["slack-cli", "send-ephemeral", "-c", "C1", "-u", "U1"],
        ] {
            let err = Cli::try_parse_from(&argv).expect_err("a required flag is missing");
            assert_eq!(err.kind(), ErrorKind::MissingRequiredArgument);
        }
    }

    #[tokio::test]
    async fn sends_the_raw_channel_and_user_values() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat.postEphemeral"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "message_ts": "1700000000.000100",
            })))
            .mount(&server)
            .await;

        let cmd = parse(&[
            "slack-cli",
            "send-ephemeral",
            "-c",
            "general",
            "-u",
            "U123",
            "-m",
            "hi",
        ]);
        run(cmd, &client_for(&server), &GlobalOpts::default())
            .await
            .unwrap();

        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1);
        let body: Value = serde_json::from_slice(&requests[0].body).unwrap();
        assert_eq!(body["channel"], "general");
        assert_eq!(body["user"], "U123");
        assert_eq!(body["text"], "hi");
        assert!(body.get("thread_ts").is_none());
    }

    #[tokio::test]
    async fn invalid_thread_timestamps_are_rejected_before_any_request() {
        let server = MockServer::start().await;
        let cmd = parse(&[
            "slack-cli",
            "send-ephemeral",
            "-c",
            "C0123456789",
            "-u",
            "U1",
            "-m",
            "hi",
            "-t",
            "1700000000",
        ]);
        let err = run(cmd, &client_for(&server), &GlobalOpts::default())
            .await
            .unwrap_err();

        assert_eq!(err.to_string(), ERR_INVALID_THREAD_TS);
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn channel_not_found_falls_back_to_name_resolution() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat.postEphemeral"))
            .and(body_json(
                json!({ "channel": "general", "user": "U1", "text": "hi" }),
            ))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({ "ok": false, "error": "channel_not_found" })),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/conversations.list"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "channels": [{ "id": "C0123456789", "name_normalized": "general", "name": "general" }],
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/chat.postEphemeral"))
            .and(body_json(
                json!({ "channel": "C0123456789", "user": "U1", "text": "hi" }),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "ok": true })))
            .mount(&server)
            .await;

        let cmd = parse(&[
            "slack-cli",
            "send-ephemeral",
            "-c",
            "general",
            "-u",
            "U1",
            "-m",
            "hi",
        ]);
        run(cmd, &client_for(&server), &GlobalOpts::default())
            .await
            .unwrap();

        assert_eq!(server.received_requests().await.unwrap().len(), 3);
    }

    #[tokio::test]
    async fn api_errors_are_propagated() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat.postEphemeral"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": false,
                "error": "user_not_in_channel",
            })))
            .mount(&server)
            .await;

        let cmd = parse(&[
            "slack-cli",
            "send-ephemeral",
            "-c",
            "C0123456789",
            "-u",
            "U1",
            "-m",
            "hi",
        ]);
        let err = run(cmd, &client_for(&server), &GlobalOpts::default())
            .await
            .unwrap_err();
        assert_eq!(err.to_string(), "API Error: user_not_in_channel");
    }
}
