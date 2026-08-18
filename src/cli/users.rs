//! `slack-cli users` — ワークスペースのユーザー照会。

use std::io::Write;

use clap::{Args, Subcommand};
use serde_json::{json, Value};

use crate::cli::{parse_positive_int, GlobalOpts};
use crate::client::pagination::PaginationOpts;
use crate::client::SlackClient;
use crate::error::SlackCliError;
use crate::output::{self, OutputFormat};

/// `users list --limit` の既定値。
pub const DEFAULT_LIST_LIMIT: &str = "100";

/// `users.list` の 1 ページあたりの取得件数。CLI の `--limit`（総件数の上限）とは別物。
pub(crate) const USERS_PAGE_SIZE: u32 = 200;

pub const ERR_LIMIT: &str = "--limit must be a positive integer";
pub const ERR_PRESENCE_TARGET_MISSING: &str = "You must specify either --id or --name";
pub const ERR_PRESENCE_TARGET_CONFLICT: &str = "Cannot use both --id and --name";
pub const MSG_NO_USERS: &str = "No users found";

#[derive(Args, Debug)]
pub struct UsersCommand {
    #[command(subcommand)]
    pub command: UsersSubcommand,
}

#[derive(Subcommand, Debug)]
pub enum UsersSubcommand {
    /// List workspace users
    List {
        /// Maximum number of users to list
        #[arg(long, default_value = DEFAULT_LIST_LIMIT, value_name = "NUMBER")]
        limit: String,
    },
    /// Get detailed information about a user
    Info {
        /// User ID
        #[arg(long, required = true, value_name = "USER_ID")]
        id: String,
    },
    /// Look up a user by email address
    Lookup {
        /// Email address to look up
        #[arg(long, required = true, value_name = "EMAIL")]
        email: String,
    },
    /// Check user presence status (active/away)
    Presence {
        /// User ID
        #[arg(long, value_name = "USER_ID")]
        id: Option<String>,

        /// Username (e.g. @username)
        #[arg(long, value_name = "USERNAME")]
        name: Option<String>,
    },
}

pub async fn run(
    cmd: UsersCommand,
    client: &SlackClient,
    global: &GlobalOpts,
) -> Result<(), SlackCliError> {
    let format = global.output_format();

    match cmd.command {
        UsersSubcommand::List { limit } => {
            let limit = parse_positive_int(&limit, ERR_LIMIT)?;
            let users = client
                .paginate_get(
                    "users.list",
                    &[],
                    "members",
                    &PaginationOpts {
                        page_size: Some(USERS_PAGE_SIZE),
                        fetch_all: true,
                        limit: Some(limit),
                        ..PaginationOpts::default()
                    },
                )
                .await?;

            render(&list_value(&users, format), MSG_NO_USERS, format)
        }

        UsersSubcommand::Info { id } => {
            let response = client.get("users.info", &[("user", &id)]).await?;
            render_value(&user_detail_value(user_of(&response), format), format)
        }

        UsersSubcommand::Lookup { email } => {
            let response = client
                .get("users.lookupByEmail", &[("email", &email)])
                .await?;
            render_value(&user_detail_value(user_of(&response), format), format)
        }

        UsersSubcommand::Presence { id, name } => {
            let user_id = resolve_presence_target(client, id, name).await?;
            let response = client
                .get("users.getPresence", &[("user", &user_id)])
                .await?;
            let presence = response.get("presence").cloned().unwrap_or(Value::Null);
            render_value(&presence_value(&user_id, presence), format)
        }
    }
}

/// `--id` / `--name` の相互排他を判定し、ユーザー ID を確定する（移植方針 G12: 判定は手書きのまま）。
async fn resolve_presence_target(
    client: &SlackClient,
    id: Option<String>,
    name: Option<String>,
) -> Result<String, SlackCliError> {
    match (id, name) {
        (Some(_), Some(_)) => Err(SlackCliError::Validation(
            ERR_PRESENCE_TARGET_CONFLICT.to_string(),
        )),
        (None, None) => Err(SlackCliError::Validation(
            ERR_PRESENCE_TARGET_MISSING.to_string(),
        )),
        (Some(id), None) => Ok(id),
        (None, Some(name)) => resolve_user_id_by_name(client, &name).await,
    }
}

/// `@name` からユーザー ID を引く。先頭の `@` を 1 つ剥がし、`user.name` と大文字小文字を無視して比較する。
pub(crate) async fn resolve_user_id_by_name(
    client: &SlackClient,
    name: &str,
) -> Result<String, SlackCliError> {
    let stripped = name.strip_prefix('@').unwrap_or(name);
    let target = stripped.to_lowercase();

    let users = client
        .paginate_get(
            "users.list",
            &[],
            "members",
            &PaginationOpts {
                page_size: Some(USERS_PAGE_SIZE),
                fetch_all: true,
                ..PaginationOpts::default()
            },
        )
        .await?;

    users
        .iter()
        .find(|user| string_at(user, "name").to_lowercase() == target)
        .map(|user| string_at(user, "id"))
        .filter(|id| !id.is_empty())
        .ok_or_else(|| SlackCliError::Validation(format!("User '{stripped}' not found")))
}

/// `users.info` / `users.lookupByEmail` の `user` フィールド。欠けていればレスポンス全体を返す。
fn user_of(response: &Value) -> &Value {
    response.get("user").unwrap_or(response)
}

/// `users list` の出力値。table は TS 版の列構成に射影し、機械可読フォーマットは API の生データを通す。
fn list_value(users: &[Value], format: OutputFormat) -> Value {
    if format != OutputFormat::Table {
        return Value::Array(users.to_vec());
    }

    Value::Array(
        users
            .iter()
            .map(|user| {
                json!({
                    "id": string_at(user, "id"),
                    "name": string_at(user, "name"),
                    "real_name": real_name(user),
                    "email": profile_at(user, "email"),
                    "is_bot": yes_no(user.get("is_bot")),
                    "deleted": yes_no(user.get("deleted")),
                })
            })
            .collect(),
    )
}

/// `users info` / `users lookup` の出力値。table は項目名と値の 2 列、それ以外は API の生データ。
fn user_detail_value(user: &Value, format: OutputFormat) -> Value {
    if format != OutputFormat::Table {
        return user.clone();
    }

    let timezone = format!(
        "{} ({})",
        string_at(user, "tz"),
        string_at(user, "tz_label")
    );
    let status = format!(
        "{} {}",
        profile_at(user, "status_emoji"),
        profile_at(user, "status_text")
    );

    Value::Array(vec![
        field_row("ID", string_at(user, "id")),
        field_row("Name", string_at(user, "name")),
        field_row("Real Name", real_name(user)),
        field_row("Display Name", profile_at(user, "display_name")),
        field_row("Email", profile_at(user, "email")),
        field_row("Title", profile_at(user, "title")),
        field_row("Timezone", timezone),
        field_row("Status", status.trim().to_string()),
        field_row("Admin", yes_no(user.get("is_admin")).to_string()),
        field_row("Bot", yes_no(user.get("is_bot")).to_string()),
        field_row("Deleted", yes_no(user.get("deleted")).to_string()),
    ])
}

/// `users presence` の出力値。TS 版の json は `presence` だけだったが、
/// 全フォーマットで同じ値を流す骨格の方針に合わせて `user` を足してある（キーの削除・改名はしていない）。
fn presence_value(user_id: &str, presence: Value) -> Value {
    json!({ "user": user_id, "presence": presence })
}

fn field_row(field: &str, value: String) -> Value {
    json!({ "field": field, "value": value })
}

/// 0 件のときだけ人間向けの文言を出す（移植方針 G14: table 以外は空の JSON 構造を返す）。
pub(crate) fn render(
    value: &Value,
    empty_message: &str,
    format: OutputFormat,
) -> Result<(), SlackCliError> {
    let mut stdout = std::io::stdout();
    let is_empty = value.as_array().is_some_and(|items| items.is_empty());

    if is_empty && format == OutputFormat::Table {
        writeln!(stdout, "{empty_message}")?;
        return Ok(());
    }
    output::format_value(value, format, &mut stdout)
}

fn render_value(value: &Value, format: OutputFormat) -> Result<(), SlackCliError> {
    output::format_value(value, format, &mut std::io::stdout())
}

pub(crate) fn string_at(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn profile_at(user: &Value, key: &str) -> String {
    user.get("profile")
        .map(|profile| string_at(profile, key))
        .unwrap_or_default()
}

/// `real_name` はトップレベルにも `profile` 配下にも入りうる。
pub(crate) fn real_name(user: &Value) -> String {
    let top = string_at(user, "real_name");
    if top.is_empty() {
        profile_at(user, "real_name")
    } else {
        top
    }
}

fn yes_no(value: Option<&Value>) -> &'static str {
    if value.and_then(Value::as_bool).unwrap_or(false) {
        "Yes"
    } else {
        "No"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::error::ErrorKind;
    use clap::Parser;
    use url::Url;
    use wiremock::matchers::{any, method, path, query_param, query_param_is_missing};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use crate::cli::Cli;

    fn parse(argv: &[&str]) -> UsersSubcommand {
        let cli = Cli::try_parse_from(argv).unwrap();
        let crate::cli::Command::Users(cmd) = cli.command else {
            panic!("expected the users command");
        };
        cmd.command
    }

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

    fn alice() -> Value {
        json!({
            "id": "U012ABC",
            "name": "alice",
            "real_name": "Alice Anderson",
            "is_bot": false,
            "deleted": false,
            "is_admin": true,
            "tz": "Asia/Tokyo",
            "tz_label": "Japan Standard Time",
            "profile": {
                "email": "alice@example.com",
                "display_name": "alice",
                "title": "Engineer",
                "status_emoji": ":palm_tree:",
                "status_text": "on leave",
            },
            "color": "9f69e7",
        })
    }

    #[test]
    fn list_limit_defaults_to_100() {
        let UsersSubcommand::List { limit } = parse(&["slack-cli", "users", "list"]) else {
            panic!("expected users list");
        };
        assert_eq!(limit, "100");
    }

    #[test]
    fn presence_accepts_id_and_name_independently() {
        // 移植方針 G12: --id と --name の相互排他は run() 側の手書き判定に残す
        assert!(matches!(
            parse(&["slack-cli", "users", "presence", "--id", "U1"]),
            UsersSubcommand::Presence { .. }
        ));
        assert!(matches!(
            parse(&["slack-cli", "users", "presence", "--name", "@alice"]),
            UsersSubcommand::Presence { .. }
        ));
        assert!(matches!(
            parse(&[
                "slack-cli",
                "users",
                "presence",
                "--id",
                "U1",
                "--name",
                "@alice"
            ]),
            UsersSubcommand::Presence { .. }
        ));
    }

    #[test]
    fn info_and_lookup_require_their_key() {
        for argv in [
            vec!["slack-cli", "users", "info"],
            vec!["slack-cli", "users", "lookup"],
        ] {
            let err = Cli::try_parse_from(&argv).expect_err("a required flag is missing");
            assert_eq!(err.kind(), ErrorKind::MissingRequiredArgument);
        }
    }

    #[tokio::test]
    async fn list_follows_the_cursor_and_stops_at_the_limit() {
        let server = MockServer::start().await;
        Mock::given(query_param_is_missing("cursor"))
            .and(path("/users.list"))
            .and(query_param("limit", "200"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "members": [alice(), { "id": "U2", "name": "bob" }],
                "response_metadata": { "next_cursor": "page2" },
            })))
            .mount(&server)
            .await;
        Mock::given(query_param("cursor", "page2"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "members": [{ "id": "U3", "name": "carol" }],
                "response_metadata": { "next_cursor": "" },
            })))
            .mount(&server)
            .await;

        let client = client_for(&server);
        run(
            UsersCommand {
                command: UsersSubcommand::List {
                    limit: "3".to_string(),
                },
            },
            &client,
            &opts(OutputFormat::Json),
        )
        .await
        .unwrap();
        assert_eq!(server.received_requests().await.unwrap().len(), 2);

        // --limit 2 は 1 ページ目で打ち切られる
        run(
            UsersCommand {
                command: UsersSubcommand::List {
                    limit: "2".to_string(),
                },
            },
            &client,
            &opts(OutputFormat::Json),
        )
        .await
        .unwrap();
        assert_eq!(server.received_requests().await.unwrap().len(), 3);
    }

    #[tokio::test]
    async fn list_rejects_a_non_numeric_limit_before_calling_the_api() {
        // 移植方針 A1 / A3: TS 版は NaN のまま 0 件・終了コード 0 になっていた
        let server = MockServer::start().await;
        Mock::given(any())
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "ok": true })))
            .mount(&server)
            .await;

        for raw in ["abc", "12abc", "0", "-5"] {
            let err = run(
                UsersCommand {
                    command: UsersSubcommand::List {
                        limit: raw.to_string(),
                    },
                },
                &client_for(&server),
                &opts(OutputFormat::Json),
            )
            .await
            .unwrap_err();
            assert_eq!(
                err.to_string(),
                ERR_LIMIT,
                "{raw:?} should have been rejected"
            );
        }
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn api_errors_are_propagated() {
        let server = MockServer::start().await;
        Mock::given(any())
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": false,
                "error": "user_not_found",
            })))
            .mount(&server)
            .await;

        let err = run(
            UsersCommand {
                command: UsersSubcommand::Info {
                    id: "U404".to_string(),
                },
            },
            &client_for(&server),
            &opts(OutputFormat::Json),
        )
        .await
        .unwrap_err();
        assert!(
            matches!(&err, SlackCliError::Api { code, .. } if code == "user_not_found"),
            "{err}"
        );
    }

    #[tokio::test]
    async fn info_and_lookup_send_their_key_as_a_query_parameter() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/users.info"))
            .and(query_param("user", "U012ABC"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({ "ok": true, "user": alice() })),
            )
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/users.lookupByEmail"))
            .and(query_param("email", "alice@example.com"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({ "ok": true, "user": alice() })),
            )
            .expect(1)
            .mount(&server)
            .await;

        let client = client_for(&server);
        run(
            UsersCommand {
                command: UsersSubcommand::Info {
                    id: "U012ABC".to_string(),
                },
            },
            &client,
            &opts(OutputFormat::Table),
        )
        .await
        .unwrap();
        run(
            UsersCommand {
                command: UsersSubcommand::Lookup {
                    email: "alice@example.com".to_string(),
                },
            },
            &client,
            &opts(OutputFormat::Json),
        )
        .await
        .unwrap();
    }

    #[test]
    fn list_projects_columns_for_table_and_keeps_raw_fields_otherwise() {
        let users = vec![alice()];

        let table = list_value(&users, OutputFormat::Table);
        let row = &table[0];
        assert_eq!(row["email"], "alice@example.com");
        assert_eq!(row["is_bot"], "No");
        assert_eq!(row["deleted"], "No");
        assert!(
            row.get("color").is_none(),
            "table must not carry raw fields"
        );

        let raw = list_value(&users, OutputFormat::Json);
        assert_eq!(raw[0]["color"], "9f69e7");
    }

    #[test]
    fn user_detail_renders_labelled_rows_for_table_only() {
        let detail = user_detail_value(&alice(), OutputFormat::Table);
        let rows: Vec<(&str, &str)> = detail
            .as_array()
            .unwrap()
            .iter()
            .map(|row| {
                (
                    row["field"].as_str().unwrap(),
                    row["value"].as_str().unwrap(),
                )
            })
            .collect();

        assert_eq!(rows[0], ("ID", "U012ABC"));
        assert!(rows.contains(&("Timezone", "Asia/Tokyo (Japan Standard Time)")));
        assert!(rows.contains(&("Status", ":palm_tree: on leave")));
        assert!(rows.contains(&("Admin", "Yes")));
        assert!(rows.contains(&("Bot", "No")));

        // table 以外は API の生オブジェクトをそのまま通す
        assert_eq!(user_detail_value(&alice(), OutputFormat::Json), alice(),);
    }

    #[test]
    fn real_name_falls_back_to_the_profile() {
        let user = json!({ "id": "U1", "profile": { "real_name": "Bob Brown" } });
        assert_eq!(real_name(&user), "Bob Brown");
        assert_eq!(real_name(&json!({ "id": "U1" })), "");
    }

    #[tokio::test]
    async fn presence_requires_exactly_one_target() {
        let server = MockServer::start().await;
        let client = client_for(&server);

        let missing = resolve_presence_target(&client, None, None)
            .await
            .unwrap_err();
        assert_eq!(missing.to_string(), ERR_PRESENCE_TARGET_MISSING);

        let conflict = resolve_presence_target(&client, Some("U1".into()), Some("@a".into()))
            .await
            .unwrap_err();
        assert_eq!(conflict.to_string(), ERR_PRESENCE_TARGET_CONFLICT);

        assert!(server.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn presence_resolves_a_username_case_insensitively() {
        let server = MockServer::start().await;
        Mock::given(path("/users.list"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "members": [{ "id": "U9", "name": "Alice" }],
                "response_metadata": { "next_cursor": "" },
            })))
            .mount(&server)
            .await;
        Mock::given(path("/users.getPresence"))
            .and(query_param("user", "U9"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({ "ok": true, "presence": "active" })),
            )
            .expect(1)
            .mount(&server)
            .await;

        run(
            UsersCommand {
                command: UsersSubcommand::Presence {
                    id: None,
                    name: Some("@ALICE".to_string()),
                },
            },
            &client_for(&server),
            &opts(OutputFormat::Json),
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn unknown_usernames_report_the_stripped_name() {
        let server = MockServer::start().await;
        Mock::given(any())
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "members": [{ "id": "U9", "name": "bob" }],
                "response_metadata": { "next_cursor": "" },
            })))
            .mount(&server)
            .await;

        let err = resolve_user_id_by_name(&client_for(&server), "@alice")
            .await
            .unwrap_err();
        assert_eq!(err.to_string(), "User 'alice' not found");
    }

    #[test]
    fn presence_value_carries_both_the_user_and_the_presence() {
        let value = presence_value("U1", json!("away"));
        assert_eq!(value["user"], "U1");
        assert_eq!(value["presence"], "away");
    }

    #[test]
    fn empty_results_stay_machine_readable_outside_table() {
        // 移植方針 G14
        let mut buf = Vec::new();
        output::format_value(&json!([]), OutputFormat::Json, &mut buf).unwrap();
        assert_eq!(String::from_utf8(buf).unwrap().trim(), "[]");

        render(&json!([]), MSG_NO_USERS, OutputFormat::Table).unwrap();
    }
}
