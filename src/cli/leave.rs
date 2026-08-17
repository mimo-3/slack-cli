//! `slack-cli leave` — チャンネルからの退出。

use std::io::Write;

use clap::Args;
use colored::Colorize;
use serde_json::json;

use crate::cli::common::{channel_label, resolve_channel_id};
use crate::cli::GlobalOpts;
use crate::client::SlackClient;
use crate::error::SlackCliError;

#[derive(Args, Debug)]
pub struct LeaveCommand {
    /// Channel name or ID
    #[arg(short, long, required = true, value_name = "CHANNEL")]
    pub channel: String,
}

pub async fn run(
    cmd: LeaveCommand,
    client: &SlackClient,
    _global: &GlobalOpts,
) -> Result<(), SlackCliError> {
    let channel_id = resolve_channel_id(client, &cmd.channel).await?;
    client
        .post_json("conversations.leave", &json!({ "channel": channel_id }))
        .await?;

    writeln!(
        std::io::stdout(),
        "{}",
        format!("✓ Left channel {}", channel_label(&cmd.channel)).green()
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use clap::error::ErrorKind;
    use clap::Parser;
    use wiremock::matchers::{body_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;
    use crate::cli::join::tests::{client_for, mount_channel_lookup};
    use crate::cli::Cli;

    #[test]
    fn parses_the_channel() {
        let cli = Cli::try_parse_from(["slack-cli", "leave", "-c", "C1"]).unwrap();
        let crate::cli::Command::Leave(cmd) = cli.command else {
            panic!("expected the leave command");
        };
        assert_eq!(cmd.channel, "C1");
    }

    #[test]
    fn channel_is_required() {
        let err = Cli::try_parse_from(["slack-cli", "leave"]).expect_err("--channel is required");
        assert_eq!(err.kind(), ErrorKind::MissingRequiredArgument);
    }

    #[tokio::test]
    async fn an_id_is_posted_without_looking_up_the_channel_list() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/conversations.leave"))
            .and(body_json(json!({ "channel": "C0123456789" })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "ok": true })))
            .expect(1)
            .mount(&server)
            .await;

        run(
            LeaveCommand {
                channel: "C0123456789".into(),
            },
            &client_for(&server),
            &GlobalOpts::default(),
        )
        .await
        .unwrap();
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn a_name_is_resolved_before_leaving() {
        let server = MockServer::start().await;
        mount_channel_lookup(&server).await;
        Mock::given(method("POST"))
            .and(path("/conversations.leave"))
            .and(body_json(json!({ "channel": "C0123456789" })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "ok": true })))
            .expect(1)
            .mount(&server)
            .await;

        run(
            LeaveCommand {
                channel: "#general".into(),
            },
            &client_for(&server),
            &GlobalOpts::default(),
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn api_errors_are_propagated() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/conversations.leave"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": false,
                "error": "not_in_channel",
            })))
            .mount(&server)
            .await;

        let err = run(
            LeaveCommand {
                channel: "C0123456789".into(),
            },
            &client_for(&server),
            &GlobalOpts::default(),
        )
        .await
        .unwrap_err();
        assert_eq!(err.to_string(), "API Error: not_in_channel");
    }
}
