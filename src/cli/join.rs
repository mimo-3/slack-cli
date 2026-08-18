//! `slack-cli join` — チャンネルへの参加。

use std::io::Write;

use clap::Args;
use colored::Colorize;
use serde_json::{json, Value};

use crate::cli::common::{channel_label, resolve_channel_id, write_success_line};
use crate::cli::GlobalOpts;
use crate::client::SlackClient;
use crate::error::SlackCliError;
use crate::output::sanitize::sanitize_single_line_text;

#[derive(Args, Debug)]
pub struct JoinCommand {
    /// Channel name or ID
    #[arg(short, long, required = true, value_name = "CHANNEL")]
    pub channel: String,
}

pub async fn run(
    cmd: JoinCommand,
    client: &SlackClient,
    global: &GlobalOpts,
) -> Result<(), SlackCliError> {
    let channel_id = resolve_channel_id(client, &cmd.channel).await?;
    let response = client
        .post_json("conversations.join", &json!({ "channel": channel_id }))
        .await?;

    // TS 版は already_in_channel などの warning を握り潰していた（移植方針 G19）
    report_warnings(&response, &mut std::io::stderr())?;

    write_success_line(
        &mut std::io::stdout(),
        global,
        &format!("✓ Joined channel {}", channel_label(&cmd.channel)),
    )
}

/// Slack が返す warning を stderr に出す。stdout と終了コードには影響させない。
pub fn report_warnings(response: &Value, writer: &mut dyn Write) -> Result<(), SlackCliError> {
    let top_level = response.get("warning").and_then(Value::as_str);
    let listed = response
        .get("response_metadata")
        .and_then(|metadata| metadata.get("warnings"))
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(Value::as_str).collect::<Vec<_>>())
        .unwrap_or_default();

    let mut seen: Vec<&str> = Vec::new();
    for warning in top_level.into_iter().chain(listed) {
        if seen.contains(&warning) {
            continue;
        }
        seen.push(warning);
        writeln!(
            writer,
            "{}",
            format!("⚠ Warning: {}", sanitize_single_line_text(warning)).yellow()
        )?;
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use clap::error::ErrorKind;
    use clap::Parser;
    use url::Url;
    use wiremock::matchers::{body_json, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;
    use crate::cli::Cli;

    pub(crate) fn client_for(server: &MockServer) -> SlackClient {
        SlackClient::new("test-token-value")
            .unwrap()
            .with_base_url(Url::parse(&format!("{}/", server.uri())).unwrap())
    }

    /// 名前解決用の `conversations.list` を 1 ページ分だけ返す。
    pub(crate) async fn mount_channel_lookup(server: &MockServer) {
        Mock::given(method("GET"))
            .and(path("/conversations.list"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "channels": [
                    { "id": "C0123456789", "name": "general", "name_normalized": "general" },
                    { "id": "C9999999999", "name": "general-random" },
                ],
                "response_metadata": { "next_cursor": "" },
            })))
            .mount(server)
            .await;
    }

    #[test]
    fn parses_the_channel() {
        let cli = Cli::try_parse_from(["slack-cli", "join", "-c", "general"]).unwrap();
        let crate::cli::Command::Join(cmd) = cli.command else {
            panic!("expected the join command");
        };
        assert_eq!(cmd.channel, "general");
    }

    #[test]
    fn channel_is_required() {
        let err = Cli::try_parse_from(["slack-cli", "join"]).expect_err("--channel is required");
        assert_eq!(err.kind(), ErrorKind::MissingRequiredArgument);
    }
    #[tokio::test]
    async fn resolving_an_id_skips_the_channel_lookup() {
        let server = MockServer::start().await;
        mount_channel_lookup(&server).await;

        let resolved = resolve_channel_id(&client_for(&server), "C0123456789")
            .await
            .unwrap();
        assert_eq!(resolved, "C0123456789");
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn join_resolves_the_name_then_posts_the_channel_id() {
        let server = MockServer::start().await;
        mount_channel_lookup(&server).await;
        Mock::given(method("POST"))
            .and(path("/conversations.join"))
            .and(body_json(json!({ "channel": "C0123456789" })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "ok": true })))
            .expect(1)
            .mount(&server)
            .await;

        run(
            JoinCommand {
                channel: "general".into(),
            },
            &client_for(&server),
            &GlobalOpts::default(),
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn join_reports_an_unknown_channel_before_calling_the_api() {
        let server = MockServer::start().await;
        mount_channel_lookup(&server).await;

        let err = run(
            JoinCommand {
                channel: "nope".into(),
            },
            &client_for(&server),
            &GlobalOpts::default(),
        )
        .await
        .unwrap_err();
        assert_eq!(
            err.to_string(),
            "Channel 'nope' not found. Make sure you are a member of this channel."
        );
    }

    #[tokio::test]
    async fn join_propagates_api_errors() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/conversations.join"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": false,
                "error": "is_archived",
            })))
            .mount(&server)
            .await;

        let err = run(
            JoinCommand {
                channel: "C0123456789".into(),
            },
            &client_for(&server),
            &GlobalOpts::default(),
        )
        .await
        .unwrap_err();
        assert_eq!(err.to_string(), "API Error: is_archived");
    }

    #[tokio::test]
    async fn channel_lookup_retries_without_the_types_the_token_cannot_read() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/conversations.list"))
            .and(query_param(
                "types",
                "public_channel,private_channel,im,mpim",
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": false,
                "error": "missing_scope",
                "needed": "groups:read,im:read,mpim:read",
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/conversations.list"))
            .and(query_param("types", "public_channel"))
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

    #[test]
    fn warnings_are_deduplicated_and_written_to_the_given_stream() {
        let response = json!({
            "ok": true,
            "warning": "already_in_channel",
            "response_metadata": { "warnings": ["already_in_channel", "missing_charset"] },
        });

        let mut buf = Vec::new();
        report_warnings(&response, &mut buf).unwrap();
        let written = String::from_utf8(buf).unwrap();
        assert_eq!(written.lines().count(), 2);
        assert!(written.contains("already_in_channel"));
        assert!(written.contains("missing_charset"));

        let mut empty = Vec::new();
        report_warnings(&json!({ "ok": true }), &mut empty).unwrap();
        assert!(empty.is_empty());
    }
}
