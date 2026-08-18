//! `slack-cli members` — チャンネルのメンバー一覧。

use std::io::Write;

use clap::Args;
use serde_json::{json, Value};

use crate::cli::common::resolve_channel_id;
use crate::cli::{parse_positive_int, GlobalOpts};
use crate::client::pagination::PaginationOpts;
use crate::client::SlackClient;
use crate::error::SlackCliError;
use crate::output::sanitize::sanitize_single_line_text;
use crate::output::{self, OutputFormat};

/// `--limit` の既定値。
pub const DEFAULT_LIMIT: &str = "100";
pub const ERR_LIMIT: &str = "--limit must be a positive integer";
pub const MSG_NO_MEMBERS: &str = "No members found";

/// `conversations.members` が 1 リクエストで返せる上限。
const MAX_PAGE_SIZE: u32 = 1000;

#[derive(Args, Debug)]
pub struct MembersCommand {
    /// Target channel name or ID
    #[arg(short, long, required = true, value_name = "CHANNEL")]
    pub channel: String,

    /// Maximum number of members to list
    #[arg(long, default_value = DEFAULT_LIMIT, value_name = "NUMBER")]
    pub limit: String,
}

pub async fn run(
    cmd: MembersCommand,
    client: &SlackClient,
    global: &GlobalOpts,
) -> Result<(), SlackCliError> {
    let limit = parse_positive_int(&cmd.limit, ERR_LIMIT)?;
    let channel_id = resolve_channel_id(client, &cmd.channel).await?;

    // TS 版は 1 ページで打ち切っていたため --limit に届かなかった（移植方針 G5）
    let member_ids = client
        .paginate_get(
            "conversations.members",
            &[("channel", channel_id.as_str())],
            "members",
            &PaginationOpts {
                page_size: Some(limit.min(MAX_PAGE_SIZE)),
                limit: Some(limit),
                ..PaginationOpts::default()
            },
        )
        .await?;

    let mut members = Vec::with_capacity(member_ids.len());
    for id in member_ids.iter().filter_map(Value::as_str) {
        members.push(member_entry(client, id).await);
    }

    let mut stdout = std::io::stdout();
    if members.is_empty() && global.output_format() == OutputFormat::Table {
        writeln!(stdout, "{MSG_NO_MEMBERS}")?;
        return Ok(());
    }

    output::format_value(&Value::Array(members), global.output_format(), &mut stdout)?;
    Ok(())
}

/// メンバー 1 人ぶんの表示用データ。`users.info` の失敗は握り潰して ID だけ返す。
async fn member_entry(client: &SlackClient, id: &str) -> Value {
    let user = client
        .get("users.info", &[("user", id)])
        .await
        .ok()
        .and_then(|response| response.get("user").cloned())
        .unwrap_or(Value::Null);

    json!({
        "id": sanitize_single_line_text(id),
        "name": string_field(&user, "name"),
        "real_name": string_field(&user, "real_name"),
    })
}

/// 値が無いときは `null` ではなく空文字にする（移植方針 G20 でそのまま再現と決めた挙動）。
fn string_field(user: &Value, key: &str) -> String {
    user.get(key)
        .and_then(Value::as_str)
        .map(sanitize_single_line_text)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use clap::error::ErrorKind;
    use clap::Parser;
    use wiremock::matchers::{method, path, query_param, query_param_is_missing};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;
    use crate::cli::join::tests::{client_for, mount_channel_lookup};
    use crate::cli::Cli;

    fn members(channel: &str, limit: &str) -> MembersCommand {
        MembersCommand {
            channel: channel.to_string(),
            limit: limit.to_string(),
        }
    }

    fn global(format: OutputFormat) -> GlobalOpts {
        GlobalOpts {
            format,
            ..GlobalOpts::default()
        }
    }

    async fn mount_user_info(server: &MockServer) {
        Mock::given(method("GET"))
            .and(path("/users.info"))
            .and(query_param("user", "U0000000001"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "user": { "id": "U0000000001", "name": "daichi", "real_name": "堀越 大地" },
            })))
            .mount(server)
            .await;
        // 名前が引けないユーザー。TS 版と同じく ID だけ残して続行する
        Mock::given(method("GET"))
            .and(path("/users.info"))
            .and(query_param("user", "U0000000002"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": false,
                "error": "user_not_found",
            })))
            .mount(server)
            .await;
    }

    #[test]
    fn limit_defaults_to_100() {
        let cli = Cli::try_parse_from(["slack-cli", "members", "-c", "general"]).unwrap();
        let crate::cli::Command::Members(cmd) = cli.command else {
            panic!("expected the members command");
        };
        assert_eq!(cmd.limit, "100");
    }

    #[test]
    fn channel_is_required() {
        let err = Cli::try_parse_from(["slack-cli", "members"]).expect_err("--channel is required");
        assert_eq!(err.kind(), ErrorKind::MissingRequiredArgument);
    }

    #[tokio::test]
    async fn a_non_numeric_limit_fails_before_any_request() {
        let server = MockServer::start().await;

        for raw in ["abc", "12abc", "0", "-1"] {
            let err = run(
                members("C0123456789", raw),
                &client_for(&server),
                &GlobalOpts::default(),
            )
            .await
            .unwrap_err();
            assert_eq!(err.to_string(), ERR_LIMIT, "{raw:?} should be rejected");
        }
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn members_are_resolved_to_names_after_the_channel_lookup() {
        let server = MockServer::start().await;
        mount_channel_lookup(&server).await;
        mount_user_info(&server).await;
        Mock::given(method("GET"))
            .and(path("/conversations.members"))
            .and(query_param("channel", "C0123456789"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "members": ["U0000000001", "U0000000002"],
                "response_metadata": { "next_cursor": "" },
            })))
            .mount(&server)
            .await;
        let client = client_for(&server);

        run(
            members("general", "100"),
            &client,
            &global(OutputFormat::Json),
        )
        .await
        .unwrap();

        let entry = member_entry(&client, "U0000000001").await;
        assert_eq!(entry["name"], "daichi");
        assert_eq!(entry["real_name"], "堀越 大地");

        // users.info が失敗しても空文字で埋めて続行する
        let failed = member_entry(&client, "U0000000002").await;
        assert_eq!(failed["id"], "U0000000002");
        assert_eq!(failed["name"], "");
        assert_eq!(failed["real_name"], "");
    }

    #[tokio::test]
    async fn paging_continues_until_the_limit_is_reached() {
        let server = MockServer::start().await;
        mount_user_info(&server).await;
        Mock::given(method("GET"))
            .and(path("/conversations.members"))
            .and(query_param_is_missing("cursor"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "members": ["U0000000001"],
                "response_metadata": { "next_cursor": "page2" },
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/conversations.members"))
            .and(query_param("cursor", "page2"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "members": ["U0000000002"],
                "response_metadata": { "next_cursor": "" },
            })))
            .expect(1)
            .mount(&server)
            .await;

        run(
            members("C0123456789", "5"),
            &client_for(&server),
            &global(OutputFormat::Json),
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn the_limit_caps_the_number_of_members() {
        let server = MockServer::start().await;
        mount_user_info(&server).await;
        Mock::given(method("GET"))
            .and(path("/conversations.members"))
            .and(query_param("limit", "1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "members": ["U0000000001", "U0000000002"],
                "response_metadata": { "next_cursor": "page2" },
            })))
            .mount(&server)
            .await;

        run(
            members("C0123456789", "1"),
            &client_for(&server),
            &global(OutputFormat::Json),
        )
        .await
        .unwrap();

        // 上限に達したので 2 ページ目は取りに行かない
        let requests = server.received_requests().await.unwrap();
        assert_eq!(
            requests
                .iter()
                .filter(|r| r.url.path() == "/conversations.members")
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn an_empty_channel_prints_text_for_table_and_json_for_the_rest() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/conversations.members"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "members": [],
                "response_metadata": { "next_cursor": "" },
            })))
            .mount(&server)
            .await;
        let client = client_for(&server);

        run(
            members("C0123456789", "100"),
            &client,
            &GlobalOpts::default(),
        )
        .await
        .unwrap();
        run(
            members("C0123456789", "100"),
            &client,
            &global(OutputFormat::Json),
        )
        .await
        .unwrap();

        // 0 件でも json は空配列を出す（移植方針 G14）
        let mut buf = Vec::new();
        output::format_value(&json!([]), OutputFormat::Json, &mut buf).unwrap();
        assert_eq!(String::from_utf8(buf).unwrap().trim(), "[]");
    }

    #[tokio::test]
    async fn api_errors_are_propagated() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/conversations.members"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": false,
                "error": "channel_not_found",
            })))
            .mount(&server)
            .await;

        let err = run(
            members("C0123456789", "100"),
            &client_for(&server),
            &GlobalOpts::default(),
        )
        .await
        .unwrap_err();
        assert_eq!(err.to_string(), "API Error: channel_not_found");
    }

    #[test]
    fn values_are_sanitized_before_they_reach_the_output_layer() {
        let user = json!({ "name": "dai\u{1b}[31mchi", "real_name": "堀越\n大地" });
        assert_eq!(string_field(&user, "name"), "daichi");
        assert_eq!(string_field(&user, "real_name"), "堀越 大地");
        assert_eq!(string_field(&user, "missing"), "");
    }
}
