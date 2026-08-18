//! `slack-cli reaction` — メッセージへのリアクション追加 / 削除。
//!
//! タイムスタンプ検証は `pin` と共有するためここに置いてある。
//! チャンネル名解決と端末サニタイズは `cli::common` / `output::sanitize` にある。

use std::io::Write;

use clap::{Args, Subcommand};
use serde_json::json;

use crate::cli::common::{
    channel_label, resolve_channel_id, write_success_line, ERR_INVALID_MESSAGE_TS,
};
use crate::cli::GlobalOpts;
use crate::client::SlackClient;
use crate::error::SlackCliError;
use crate::output::sanitize::sanitize_single_line_text;

/// `--timestamp` の書式エラー。対象はスレッドではなくメッセージの ts なので、
/// `edit` / `delete` と同じ文言に揃える（移植方針 G9）。
pub const ERR_INVALID_TIMESTAMP: &str = ERR_INVALID_MESSAGE_TS;
pub const ERR_EMPTY_EMOJI: &str = "--emoji must not be empty";

#[derive(Args, Debug)]
pub struct ReactionCommand {
    #[command(subcommand)]
    pub command: ReactionSubcommand,
}

#[derive(Subcommand, Debug)]
pub enum ReactionSubcommand {
    /// Add a reaction to a message
    Add(ReactionArgs),
    /// Remove a reaction from a message
    Remove(ReactionArgs),
}

#[derive(Args, Debug)]
pub struct ReactionArgs {
    /// Channel name or ID
    #[arg(short, long, required = true, value_name = "CHANNEL")]
    pub channel: String,

    /// Message timestamp
    #[arg(short, long, required = true, value_name = "TIMESTAMP")]
    pub timestamp: String,

    /// Emoji name (with or without surrounding colons)
    #[arg(short, long, required = true, value_name = "EMOJI")]
    pub emoji: String,
}

pub async fn run(
    cmd: ReactionCommand,
    client: &SlackClient,
    global: &GlobalOpts,
) -> Result<(), SlackCliError> {
    let mut stdout = std::io::stdout();
    execute(cmd, client, global, &mut stdout).await
}

async fn execute(
    cmd: ReactionCommand,
    client: &SlackClient,
    global: &GlobalOpts,
    out: &mut dyn Write,
) -> Result<(), SlackCliError> {
    let (args, method, verb) = match cmd.command {
        ReactionSubcommand::Add(args) => (args, "reactions.add", "added to"),
        ReactionSubcommand::Remove(args) => (args, "reactions.remove", "removed from"),
    };

    validate_message_timestamp(&args.timestamp)?;

    let emoji = normalize_emoji(&args.emoji);
    if emoji.is_empty() {
        return Err(SlackCliError::Validation(ERR_EMPTY_EMOJI.to_string()));
    }

    let channel_id = resolve_channel_id(client, &args.channel).await?;
    client
        .post_json(
            method,
            &json!({
                "channel": channel_id,
                "timestamp": args.timestamp,
                "name": emoji,
            }),
        )
        .await?;

    // 移植方針 E2: `--emoji` も利用者入力なのでチャンネル名と同じくサニタイズする
    let shown_emoji = sanitize_single_line_text(&emoji);
    let label = channel_label(&args.channel);
    write_success_line(
        out,
        global,
        &format!("✓ Reaction :{shown_emoji}: {verb} message in {label}"),
    )
}

/// `--timestamp` を `1234567890.123456` の固定形式で検証する。
pub fn validate_message_timestamp(raw: &str) -> Result<(), SlackCliError> {
    let is_valid = match raw.split_once('.') {
        Some((secs, micros)) => {
            secs.len() == 10
                && micros.len() == 6
                && secs.bytes().all(|b| b.is_ascii_digit())
                && micros.bytes().all(|b| b.is_ascii_digit())
        }
        None => false,
    };

    if is_valid {
        Ok(())
    } else {
        Err(SlackCliError::Validation(ERR_INVALID_TIMESTAMP.to_string()))
    }
}

/// 絵文字名から先頭・末尾のコロンを 1 個ずつ落とす。中間のコロン（skin tone 記法）は触らない。
pub fn normalize_emoji(raw: &str) -> String {
    let trimmed = raw.trim();
    let without_head = trimmed.strip_prefix(':').unwrap_or(trimmed);
    without_head
        .strip_suffix(':')
        .unwrap_or(without_head)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::error::ErrorKind;
    use clap::Parser;
    use url::Url;
    use wiremock::matchers::{body_partial_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use crate::cli::Cli;

    fn client_for(server: &MockServer) -> SlackClient {
        SlackClient::new("test-token-value")
            .unwrap()
            .with_base_url(Url::parse(&format!("{}/", server.uri())).unwrap())
    }

    fn args(channel: &str, emoji: &str) -> ReactionArgs {
        ReactionArgs {
            channel: channel.to_string(),
            timestamp: "1700000000.000100".to_string(),
            emoji: emoji.to_string(),
        }
    }

    async fn mount_channel_list(server: &MockServer) {
        Mock::given(method("GET"))
            .and(path("/conversations.list"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "channels": [
                    { "id": "C0123456789", "name": "dev-acejob", "name_normalized": "dev-acejob" },
                    { "id": "C9999999999", "name": "dev-random", "name_normalized": "dev-random" },
                ],
                "response_metadata": { "next_cursor": "" },
            })))
            .mount(server)
            .await;
    }

    async fn run_capture(
        cmd: ReactionCommand,
        client: &SlackClient,
    ) -> Result<String, SlackCliError> {
        let mut buf = Vec::new();
        execute(cmd, client, &GlobalOpts::default(), &mut buf).await?;
        Ok(String::from_utf8(buf).unwrap())
    }

    #[test]
    fn add_parses_all_three_required_flags() {
        let cli = Cli::try_parse_from([
            "slack-cli",
            "reaction",
            "add",
            "-c",
            "C1",
            "-t",
            "1700000000.000100",
            "-e",
            "tada",
        ])
        .unwrap();
        let crate::cli::Command::Reaction(cmd) = cli.command else {
            panic!("expected the reaction command");
        };
        let ReactionSubcommand::Add(args) = cmd.command else {
            panic!("expected reaction add");
        };
        assert_eq!(args.emoji, "tada");
        assert_eq!(args.timestamp, "1700000000.000100");
    }

    #[test]
    fn emoji_is_required_on_both_subcommands() {
        for sub in ["add", "remove"] {
            let err = Cli::try_parse_from(["slack-cli", "reaction", sub, "-c", "C1", "-t", "1.1"])
                .expect_err("--emoji is required");
            assert_eq!(err.kind(), ErrorKind::MissingRequiredArgument);
        }
    }

    #[test]
    fn timestamps_must_be_ten_dot_six_digits() {
        assert!(validate_message_timestamp("1700000000.000100").is_ok());
        for raw in [
            "1755400000.1",
            "17554000001.000100",
            "1700000000",
            "170000000a.000100",
            " 1700000000.000100",
            "",
        ] {
            let err = validate_message_timestamp(raw).unwrap_err();
            assert_eq!(err.to_string(), ERR_INVALID_TIMESTAMP, "{raw:?}");
        }
    }

    #[test]
    fn emoji_loses_one_colon_on_each_end() {
        assert_eq!(normalize_emoji(":tada:"), "tada");
        assert_eq!(normalize_emoji("tada"), "tada");
        assert_eq!(normalize_emoji("::tada::"), ":tada:");
        assert_eq!(normalize_emoji("+1::skin-tone-2"), "+1::skin-tone-2");
        assert!(normalize_emoji(":").is_empty());
    }
    #[tokio::test]
    async fn add_resolves_the_channel_name_and_normalizes_the_emoji() {
        let server = MockServer::start().await;
        mount_channel_list(&server).await;
        Mock::given(method("POST"))
            .and(path("/reactions.add"))
            .and(body_partial_json(json!({
                "channel": "C0123456789",
                "timestamp": "1700000000.000100",
                "name": "tada",
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "ok": true })))
            .mount(&server)
            .await;

        let out = run_capture(
            ReactionCommand {
                command: ReactionSubcommand::Add(args("dev-acejob", ":tada:")),
            },
            &client_for(&server),
        )
        .await
        .unwrap();

        assert!(
            out.contains("Reaction :tada: added to message in #dev-acejob"),
            "output was: {out}"
        );
    }

    #[tokio::test]
    async fn remove_hits_the_remove_endpoint_and_skips_lookup_for_ids() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/reactions.remove"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "ok": true })))
            .mount(&server)
            .await;

        let out = run_capture(
            ReactionCommand {
                command: ReactionSubcommand::Remove(args("C0123456789", "tada")),
            },
            &client_for(&server),
        )
        .await
        .unwrap();

        assert!(
            out.contains("Reaction :tada: removed from message in C0123456789"),
            "output was: {out}"
        );
        let requests = server.received_requests().await.unwrap();
        assert_eq!(
            requests.len(),
            1,
            "an ID must not trigger conversations.list"
        );
    }

    #[tokio::test]
    async fn slack_errors_are_propagated() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/reactions.add"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": false,
                "error": "already_reacted",
            })))
            .mount(&server)
            .await;

        let err = run_capture(
            ReactionCommand {
                command: ReactionSubcommand::Add(args("C0123456789", "tada")),
            },
            &client_for(&server),
        )
        .await
        .unwrap_err();

        assert!(err.to_string().contains("already_reacted"), "{err}");
    }

    #[tokio::test]
    async fn unknown_channel_names_suggest_similar_ones() {
        let server = MockServer::start().await;
        mount_channel_list(&server).await;

        let err = run_capture(
            ReactionCommand {
                command: ReactionSubcommand::Add(args("dev", "tada")),
            },
            &client_for(&server),
        )
        .await
        .unwrap_err();

        assert_eq!(
            err.to_string(),
            "Channel 'dev' not found. Did you mean one of these? dev-acejob, dev-random"
        );
    }

    #[tokio::test]
    async fn unknown_channel_without_candidates_says_check_membership() {
        let server = MockServer::start().await;
        mount_channel_list(&server).await;

        let err = run_capture(
            ReactionCommand {
                command: ReactionSubcommand::Remove(args("marketing", "tada")),
            },
            &client_for(&server),
        )
        .await
        .unwrap_err();

        assert_eq!(
            err.to_string(),
            "Channel 'marketing' not found. Make sure you are a member of this channel."
        );
    }

    #[tokio::test]
    async fn invalid_arguments_are_rejected_before_any_request() {
        let server = MockServer::start().await;
        let client = client_for(&server);

        let mut bad_ts = args("C0123456789", "tada");
        bad_ts.timestamp = "1700000000".to_string();
        let err = run_capture(
            ReactionCommand {
                command: ReactionSubcommand::Add(bad_ts),
            },
            &client,
        )
        .await
        .unwrap_err();
        assert_eq!(err.to_string(), ERR_INVALID_TIMESTAMP);

        let err = run_capture(
            ReactionCommand {
                command: ReactionSubcommand::Add(args("C0123456789", ":")),
            },
            &client,
        )
        .await
        .unwrap_err();
        assert_eq!(err.to_string(), ERR_EMPTY_EMOJI);

        assert!(server.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn dry_run_sends_no_write_request() {
        let server = MockServer::start().await;
        let client = client_for(&server).with_dry_run(true);
        let global = GlobalOpts {
            dry_run: true,
            ..GlobalOpts::default()
        };

        let mut buf = Vec::new();
        execute(
            ReactionCommand {
                command: ReactionSubcommand::Add(args("C0123456789", "tada")),
            },
            &client,
            &global,
            &mut buf,
        )
        .await
        .unwrap();

        assert!(server.received_requests().await.unwrap().is_empty());
    }
}
