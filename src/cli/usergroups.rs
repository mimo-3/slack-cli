//! `slack-cli usergroups` — ユーザーグループの一覧とメンバー照会。

use clap::{Args, Subcommand};
use serde_json::{json, Value};

use crate::cli::users::{real_name, render, string_at};
use crate::cli::GlobalOpts;
use crate::client::SlackClient;
use crate::error::SlackCliError;
use crate::output::OutputFormat;

pub const ERR_MEMBERS_TARGET_MISSING: &str = "You must specify either --id or --handle";
pub const ERR_MEMBERS_TARGET_CONFLICT: &str = "Cannot use both --id and --handle";
pub const MSG_NO_USERGROUPS: &str = "No usergroups found";
pub const MSG_NO_MEMBERS: &str = "No members found";

#[derive(Args, Debug)]
pub struct UsergroupsCommand {
    #[command(subcommand)]
    pub command: UsergroupsSubcommand,
}

#[derive(Subcommand, Debug)]
pub enum UsergroupsSubcommand {
    /// List user groups in the workspace
    List {
        /// Include disabled user groups
        #[arg(long)]
        include_disabled: bool,
    },
    /// List members of a user group
    Members {
        /// User group ID
        #[arg(long, value_name = "USERGROUP_ID")]
        id: Option<String>,

        /// User group handle (e.g. @engineers)
        #[arg(long, value_name = "HANDLE")]
        handle: Option<String>,
    },
}

pub async fn run(
    cmd: UsergroupsCommand,
    client: &SlackClient,
    global: &GlobalOpts,
) -> Result<(), SlackCliError> {
    let format = global.output_format();

    match cmd.command {
        UsergroupsSubcommand::List { include_disabled } => {
            let groups = fetch_usergroups(client, include_disabled).await?;
            render(&list_value(&groups, format), MSG_NO_USERGROUPS, format)
        }

        UsergroupsSubcommand::Members { id, handle } => {
            let usergroup_id = resolve_members_target(client, id, handle).await?;
            let response = client
                .get("usergroups.users.list", &[("usergroup", &usergroup_id)])
                .await?;

            let member_ids: Vec<String> = response
                .get("users")
                .and_then(Value::as_array)
                .map(|users| {
                    users
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default();

            let members = fetch_members(client, &member_ids).await?;
            render(&Value::Array(members), MSG_NO_MEMBERS, format)
        }
    }
}

/// `usergroups.list`。件数は常に取得し、無効なグループは指定時のみ含める。
async fn fetch_usergroups(
    client: &SlackClient,
    include_disabled: bool,
) -> Result<Vec<Value>, SlackCliError> {
    let mut params: Vec<(&str, &str)> = vec![("include_count", "true")];
    if include_disabled {
        params.push(("include_disabled", "true"));
    }

    let response = client.get("usergroups.list", &params).await?;
    Ok(response
        .get("usergroups")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default())
}

/// `--id` / `--handle` の相互排他を判定し、ユーザーグループ ID を確定する（移植方針 G12）。
async fn resolve_members_target(
    client: &SlackClient,
    id: Option<String>,
    handle: Option<String>,
) -> Result<String, SlackCliError> {
    match (id, handle) {
        (Some(_), Some(_)) => Err(SlackCliError::Validation(
            ERR_MEMBERS_TARGET_CONFLICT.to_string(),
        )),
        (None, None) => Err(SlackCliError::Validation(
            ERR_MEMBERS_TARGET_MISSING.to_string(),
        )),
        (Some(id), None) => Ok(id),
        (None, Some(handle)) => resolve_usergroup_id_by_handle(client, &handle).await,
    }
}

/// `@handle` からユーザーグループ ID を引く。解決用の一覧は常に無効なグループも含めて取得する。
async fn resolve_usergroup_id_by_handle(
    client: &SlackClient,
    handle: &str,
) -> Result<String, SlackCliError> {
    let stripped = handle.strip_prefix('@').unwrap_or(handle);
    let target = stripped.to_lowercase();

    let groups = fetch_usergroups(client, true).await?;
    groups
        .iter()
        .find(|group| string_at(group, "handle").to_lowercase() == target)
        .map(|group| string_at(group, "id"))
        .filter(|id| !id.is_empty())
        .ok_or_else(|| SlackCliError::Validation(format!("Usergroup '@{stripped}' not found")))
}

/// メンバー ID ごとに `users.info` を引く。個別の失敗は握り潰し、ID だけの行にフォールバックする。
/// 結果は入力順を保つ。
async fn fetch_members(
    client: &SlackClient,
    member_ids: &[String],
) -> Result<Vec<Value>, SlackCliError> {
    let mut members = Vec::with_capacity(member_ids.len());

    for user_id in member_ids {
        let member = match client.get("users.info", &[("user", user_id)]).await {
            Ok(response) => {
                let user = response.get("user").unwrap_or(&response);
                member_value(user_id, &string_at(user, "name"), &real_name(user))
            }
            Err(_) => member_value(user_id, "", ""),
        };
        members.push(member);
    }

    Ok(members)
}

fn member_value(id: &str, name: &str, real_name: &str) -> Value {
    json!({ "id": id, "name": name, "real_name": real_name })
}

/// `usergroups list` の出力値。table は TS 版の列構成に射影し、それ以外は API の生データを通す。
fn list_value(groups: &[Value], format: OutputFormat) -> Value {
    if format != OutputFormat::Table {
        return Value::Array(groups.to_vec());
    }

    Value::Array(
        groups
            .iter()
            .map(|group| {
                json!({
                    "id": string_at(group, "id"),
                    "handle": string_at(group, "handle"),
                    "name": string_at(group, "name"),
                    "description": string_at(group, "description"),
                    "user_count": group
                        .get("user_count")
                        .cloned()
                        .unwrap_or_else(|| Value::String(String::new())),
                })
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    use url::Url;
    use wiremock::matchers::{any, path, query_param, query_param_is_missing};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use crate::cli::Cli;

    fn parse(argv: &[&str]) -> UsergroupsSubcommand {
        let cli = Cli::try_parse_from(argv).unwrap();
        let crate::cli::Command::Usergroups(cmd) = cli.command else {
            panic!("expected the usergroups command");
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

    fn engineers() -> Value {
        json!({
            "id": "S012ABC",
            "handle": "engineers",
            "name": "Engineering Team",
            "description": "builders",
            "user_count": 2,
            "date_delete": 0,
        })
    }

    #[test]
    fn include_disabled_defaults_to_false() {
        let UsergroupsSubcommand::List { include_disabled } =
            parse(&["slack-cli", "usergroups", "list"])
        else {
            panic!("expected usergroups list");
        };
        assert!(!include_disabled);
    }

    #[test]
    fn members_takes_either_id_or_handle() {
        // 移植方針 G12: 相互排他は run() 側の手書き判定に残す
        let UsergroupsSubcommand::Members { id, handle } = parse(&[
            "slack-cli",
            "usergroups",
            "members",
            "--id",
            "S1",
            "--handle",
            "@eng",
        ]) else {
            panic!("expected usergroups members");
        };
        assert_eq!(id.as_deref(), Some("S1"));
        assert_eq!(handle.as_deref(), Some("@eng"));
    }

    #[tokio::test]
    async fn list_sends_include_disabled_only_when_requested() {
        let server = MockServer::start().await;
        Mock::given(path("/usergroups.list"))
            .and(query_param("include_count", "true"))
            .and(query_param_is_missing("include_disabled"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "usergroups": [engineers()],
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(path("/usergroups.list"))
            .and(query_param("include_disabled", "true"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "usergroups": [],
            })))
            .expect(1)
            .mount(&server)
            .await;

        let client = client_for(&server);
        for include_disabled in [false, true] {
            run(
                UsergroupsCommand {
                    command: UsergroupsSubcommand::List { include_disabled },
                },
                &client,
                &opts(OutputFormat::Json),
            )
            .await
            .unwrap();
        }
    }

    #[tokio::test]
    async fn list_propagates_api_errors() {
        let server = MockServer::start().await;
        Mock::given(any())
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": false,
                "error": "missing_scope",
                "needed": "usergroups:read",
            })))
            .mount(&server)
            .await;

        let err = run(
            UsergroupsCommand {
                command: UsergroupsSubcommand::List {
                    include_disabled: false,
                },
            },
            &client_for(&server),
            &opts(OutputFormat::Json),
        )
        .await
        .unwrap_err();
        assert!(
            matches!(&err, SlackCliError::Api { needed, .. } if needed == &["usergroups:read"]),
            "{err}"
        );
    }

    #[test]
    fn list_projects_columns_for_table_and_keeps_raw_fields_otherwise() {
        let groups = vec![engineers(), json!({ "id": "S2", "handle": "design" })];

        let table = list_value(&groups, OutputFormat::Table);
        assert_eq!(table[0]["user_count"], 2);
        assert_eq!(table[0]["description"], "builders");
        assert!(table[0].get("date_delete").is_none());
        // user_count が無いグループは空文字で埋める
        assert_eq!(table[1]["user_count"], "");

        let raw = list_value(&groups, OutputFormat::Json);
        assert_eq!(raw[0]["date_delete"], 0);
    }

    #[tokio::test]
    async fn members_requires_exactly_one_target() {
        let server = MockServer::start().await;
        let client = client_for(&server);

        let missing = resolve_members_target(&client, None, None).await.unwrap_err();
        assert_eq!(missing.to_string(), ERR_MEMBERS_TARGET_MISSING);

        let conflict = resolve_members_target(&client, Some("S1".into()), Some("@eng".into()))
            .await
            .unwrap_err();
        assert_eq!(conflict.to_string(), ERR_MEMBERS_TARGET_CONFLICT);

        assert!(server.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn members_resolves_a_handle_and_keeps_the_input_order() {
        let server = MockServer::start().await;
        Mock::given(path("/usergroups.list"))
            .and(query_param("include_disabled", "true"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "usergroups": [engineers()],
            })))
            .mount(&server)
            .await;
        Mock::given(path("/usergroups.users.list"))
            .and(query_param("usergroup", "S012ABC"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "users": ["U1", "U2"],
            })))
            .mount(&server)
            .await;
        Mock::given(path("/users.info"))
            .and(query_param("user", "U1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "user": { "id": "U1", "name": "alice", "real_name": "Alice Anderson" },
            })))
            .mount(&server)
            .await;
        // 個別の users.info の失敗は握り潰して ID だけの行にする
        Mock::given(path("/users.info"))
            .and(query_param("user", "U2"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": false,
                "error": "user_not_found",
            })))
            .mount(&server)
            .await;

        let client = client_for(&server);
        let usergroup_id = resolve_members_target(&client, None, Some("@Engineers".into()))
            .await
            .unwrap();
        assert_eq!(usergroup_id, "S012ABC");

        let members = fetch_members(&client, &["U1".to_string(), "U2".to_string()])
            .await
            .unwrap();
        assert_eq!(members[0], member_value("U1", "alice", "Alice Anderson"));
        assert_eq!(members[1], member_value("U2", "", ""));

        run(
            UsergroupsCommand {
                command: UsergroupsSubcommand::Members {
                    id: None,
                    handle: Some("@engineers".to_string()),
                },
            },
            &client,
            &opts(OutputFormat::Json),
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn unknown_handles_report_the_handle_with_an_at_sign() {
        let server = MockServer::start().await;
        Mock::given(any())
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "usergroups": [engineers()],
            })))
            .mount(&server)
            .await;

        let err = resolve_usergroup_id_by_handle(&client_for(&server), "@design")
            .await
            .unwrap_err();
        assert_eq!(err.to_string(), "Usergroup '@design' not found");
    }

    #[tokio::test]
    async fn members_of_an_empty_group_render_as_an_empty_array() {
        // 移植方針 G14: table 以外は空配列を出す
        let server = MockServer::start().await;
        Mock::given(path("/usergroups.users.list"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "users": [],
            })))
            .mount(&server)
            .await;

        let client = client_for(&server);
        for format in [OutputFormat::Json, OutputFormat::Table] {
            run(
                UsergroupsCommand {
                    command: UsergroupsSubcommand::Members {
                        id: Some("S012ABC".to_string()),
                        handle: None,
                    },
                },
                &client,
                &opts(format),
            )
            .await
            .unwrap();
        }
        // handle 未指定なので usergroups.list は呼ばれない
        assert_eq!(server.received_requests().await.unwrap().len(), 2);
    }
}
