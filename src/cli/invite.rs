//! `slack-cli invite` — チャンネルへのユーザー招待。

use std::io::Write;

use clap::Args;
use colored::Colorize;
use serde_json::{json, Value};

use crate::cli::common::{channel_label, resolve_channel_id, write_success_line};
use crate::cli::GlobalOpts;
use crate::client::pagination::PaginationOpts;
use crate::client::SlackClient;
use crate::error::SlackCliError;
use crate::output::sanitize::sanitize_single_line_text;

pub const ERR_NO_USERS: &str = "At least one valid user ID is required";

/// `users.list` の 1 ページあたり取得件数（TS 版 `resolveUserIdByName` と同じ）。
const USER_LOOKUP_PAGE_SIZE: u32 = 200;

#[derive(Args, Debug)]
pub struct InviteCommand {
    /// Channel name or ID
    #[arg(short, long, required = true, value_name = "CHANNEL")]
    pub channel: String,

    /// Comma-separated user IDs or names to invite
    #[arg(short, long, required = true, value_name = "USERS")]
    pub users: String,

    /// Continue inviting valid users even if some IDs are invalid
    #[arg(long)]
    pub force: bool,
}

pub async fn run(
    cmd: InviteCommand,
    client: &SlackClient,
    global: &GlobalOpts,
) -> Result<(), SlackCliError> {
    let references = parse_user_references(&cmd.users)?;
    let channel_id = resolve_channel_id(client, &cmd.channel).await?;
    let user_ids = resolve_user_ids(client, &references).await?;

    let mut body = json!({
        "channel": channel_id,
        "users": user_ids.join(","),
    });
    // TS 版と同じく、--force は指定されたときだけキーごと送る
    if cmd.force {
        body["force"] = Value::Bool(true);
    }

    let response = client.post_json("conversations.invite", &body).await?;
    report_invite_errors(&response, &mut std::io::stderr())?;

    write_success_line(
        &mut std::io::stdout(),
        global,
        &format!(
            "✓ Invited user(s) to channel {}",
            channel_label(&cmd.channel)
        ),
    )
}

/// `--users` をカンマ分割・trim・空要素除去する。1 件も残らなければエラー。
fn parse_user_references(raw: &str) -> Result<Vec<&str>, SlackCliError> {
    let references: Vec<&str> = raw
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .collect();

    if references.is_empty() {
        return Err(SlackCliError::Validation(ERR_NO_USERS.to_string()));
    }
    Ok(references)
}

/// ID 形式の要素はそのまま使い、それ以外はユーザー名として解決する（移植方針 G18）。
/// 名前解決が要るときだけ `users.list` を 1 回引き、全要素で使い回す。
async fn resolve_user_ids(
    client: &SlackClient,
    references: &[&str],
) -> Result<Vec<String>, SlackCliError> {
    let needs_lookup = references
        .iter()
        .any(|reference| !is_user_id(strip_at(reference)));

    let users = if needs_lookup {
        client
            .paginate_get(
                "users.list",
                &[],
                "members",
                &PaginationOpts {
                    page_size: Some(USER_LOOKUP_PAGE_SIZE),
                    fetch_all: true,
                    ..PaginationOpts::default()
                },
            )
            .await?
    } else {
        Vec::new()
    };

    references
        .iter()
        .map(|reference| {
            let name = strip_at(reference);
            if is_user_id(name) {
                return Ok(name.to_string());
            }
            find_user_id(&users, name).ok_or_else(|| {
                SlackCliError::Validation(format!(
                    "User '{}' not found",
                    sanitize_single_line_text(name)
                ))
            })
        })
        .collect()
}

fn strip_at(reference: &str) -> &str {
    reference.strip_prefix('@').unwrap_or(reference)
}

/// ユーザー ID の形式判定（`U` + 大文字英数 8 文字以上）。
fn is_user_id(value: &str) -> bool {
    let mut chars = value.chars();
    if chars.next() != Some('U') {
        return false;
    }

    let rest: Vec<char> = chars.collect();
    rest.len() >= 8
        && rest
            .iter()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
}

/// TS 版 `resolveUserIdByName` と同じく `name` の小文字一致だけで探す。
fn find_user_id(users: &[Value], name: &str) -> Option<String> {
    let lowered = name.to_lowercase();
    users
        .iter()
        .find(|user| {
            user.get("name")
                .and_then(Value::as_str)
                .is_some_and(|n| n.to_lowercase() == lowered)
        })
        .and_then(|user| user.get("id").and_then(Value::as_str))
        .map(str::to_string)
}

/// `--force` 時の部分失敗を stderr に出す。stdout と終了コードには影響させない（移植方針 G19）。
fn report_invite_errors(response: &Value, writer: &mut dyn Write) -> Result<(), SlackCliError> {
    let Some(errors) = response.get("errors").and_then(Value::as_array) else {
        return Ok(());
    };

    for entry in errors {
        if entry.get("ok").and_then(Value::as_bool) == Some(true) {
            continue;
        }
        let user = entry
            .get("user")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        let reason = entry
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("unknown_error");
        writeln!(
            writer,
            "{}",
            format!(
                "⚠ Warning: could not invite {}: {}",
                sanitize_single_line_text(user),
                sanitize_single_line_text(reason)
            )
            .yellow()
        )?;
    }
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

    fn invite(channel: &str, users: &str, force: bool) -> InviteCommand {
        InviteCommand {
            channel: channel.to_string(),
            users: users.to_string(),
            force,
        }
    }

    async fn mount_user_list(server: &MockServer) {
        Mock::given(method("GET"))
            .and(path("/users.list"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "members": [
                    { "id": "U0123456789", "name": "daichi" },
                    { "id": "U9876543210", "name": "hanako" },
                ],
                "response_metadata": { "next_cursor": "" },
            })))
            .mount(server)
            .await;
    }

    #[test]
    fn users_stay_a_single_comma_separated_string() {
        let cli =
            Cli::try_parse_from(["slack-cli", "invite", "-c", "C1", "-u", "U1,U2", "--force"])
                .unwrap();
        let crate::cli::Command::Invite(cmd) = cli.command else {
            panic!("expected the invite command");
        };
        assert_eq!(cmd.users, "U1,U2");
        assert!(cmd.force);
    }

    #[test]
    fn users_is_required() {
        let err = Cli::try_parse_from(["slack-cli", "invite", "-c", "C1"])
            .expect_err("--users is required");
        assert_eq!(err.kind(), ErrorKind::MissingRequiredArgument);
    }

    #[test]
    fn user_references_are_trimmed_and_emptied_out() {
        assert_eq!(
            parse_user_references(" U1 , ,U2,").unwrap(),
            vec!["U1", "U2"]
        );
        assert_eq!(
            parse_user_references(",, ").unwrap_err().to_string(),
            ERR_NO_USERS
        );
    }

    #[test]
    fn user_ids_are_told_apart_from_names() {
        assert!(is_user_id("U0123456789"));
        assert!(!is_user_id("U123"));
        assert!(!is_user_id("daichi"));
        assert!(!is_user_id("C0123456789"));
        assert_eq!(strip_at("@daichi"), "daichi");
        assert_eq!(strip_at("daichi"), "daichi");
    }

    #[tokio::test]
    async fn ids_are_sent_as_is_without_a_user_lookup() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/conversations.invite"))
            .and(body_json(json!({
                "channel": "C0123456789",
                "users": "U0123456789,U9876543210",
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "ok": true })))
            .expect(1)
            .mount(&server)
            .await;

        run(
            invite("C0123456789", "U0123456789, U9876543210", false),
            &client_for(&server),
            &GlobalOpts::default(),
        )
        .await
        .unwrap();
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn names_are_resolved_and_force_is_only_sent_when_asked() {
        let server = MockServer::start().await;
        mount_channel_lookup(&server).await;
        mount_user_list(&server).await;
        Mock::given(method("POST"))
            .and(path("/conversations.invite"))
            .and(body_json(json!({
                "channel": "C0123456789",
                "users": "U9876543210,U0123456789",
                "force": true,
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "ok": true })))
            .expect(1)
            .mount(&server)
            .await;

        run(
            invite("general", "@hanako,U0123456789", true),
            &client_for(&server),
            &GlobalOpts::default(),
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn an_unknown_user_name_stops_the_invite() {
        let server = MockServer::start().await;
        mount_user_list(&server).await;

        let err = run(
            invite("C0123456789", "nobody", false),
            &client_for(&server),
            &GlobalOpts::default(),
        )
        .await
        .unwrap_err();
        assert_eq!(err.to_string(), "User 'nobody' not found");
    }

    #[tokio::test]
    async fn blank_user_lists_fail_before_any_request() {
        let server = MockServer::start().await;

        let err = run(
            invite("C0123456789", " , ", false),
            &client_for(&server),
            &GlobalOpts::default(),
        )
        .await
        .unwrap_err();
        assert_eq!(err.to_string(), ERR_NO_USERS);
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn api_errors_are_propagated() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/conversations.invite"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": false,
                "error": "cant_invite_self",
            })))
            .mount(&server)
            .await;

        let err = run(
            invite("C0123456789", "U0123456789", false),
            &client_for(&server),
            &GlobalOpts::default(),
        )
        .await
        .unwrap_err();
        assert_eq!(err.to_string(), "API Error: cant_invite_self");
    }

    #[test]
    fn partial_failures_are_reported_per_user() {
        let response = json!({
            "ok": true,
            "errors": [
                { "user": "U0123456789", "ok": false, "error": "cant_invite" },
                { "user": "U9876543210", "ok": true },
            ],
        });

        let mut buf = Vec::new();
        report_invite_errors(&response, &mut buf).unwrap();
        let written = String::from_utf8(buf).unwrap();
        assert_eq!(written.lines().count(), 1);
        assert!(written.contains("U0123456789"));
        assert!(written.contains("cant_invite"));

        let mut empty = Vec::new();
        report_invite_errors(&json!({ "ok": true }), &mut empty).unwrap();
        assert!(empty.is_empty());
    }
}
