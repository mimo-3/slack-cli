//! `slack-cli channel` — チャンネルの詳細表示とトピック / 目的の更新。
//!
//! チャンネル名の解決・サニタイズ・時刻整形は `cli::channels` の関数を共有する。

use clap::{Args, Subcommand};
use serde_json::{Map, Value};

use crate::cli::channels::{format_channel_display, format_created_date};
use crate::cli::common::{resolve_channel_id, write_success};
use crate::cli::GlobalOpts;
use crate::client::SlackClient;
use crate::error::SlackCliError;
use crate::output::sanitize::sanitize_terminal_text;
use crate::output::{self, OutputFormat};

const TOPIC_FIELD: &str = "topic";
const PURPOSE_FIELD: &str = "purpose";

#[derive(Args, Debug)]
pub struct ChannelCommand {
    #[command(subcommand)]
    pub command: ChannelSubcommand,
}

#[derive(Subcommand, Debug)]
pub enum ChannelSubcommand {
    /// Display channel details including topic and purpose
    Info {
        /// Target channel name or ID
        #[arg(short, long, required = true, value_name = "CHANNEL")]
        channel: String,
    },
    /// Set the topic of a channel
    SetTopic {
        /// Target channel name or ID
        #[arg(short, long, required = true, value_name = "CHANNEL")]
        channel: String,

        /// New topic text
        #[arg(long, required = true, value_name = "TOPIC")]
        topic: String,
    },
    /// Set the purpose of a channel
    SetPurpose {
        /// Target channel name or ID
        #[arg(short, long, required = true, value_name = "CHANNEL")]
        channel: String,

        /// New purpose text
        #[arg(long, required = true, value_name = "PURPOSE")]
        purpose: String,
    },
}

pub async fn run(
    cmd: ChannelCommand,
    client: &SlackClient,
    global: &GlobalOpts,
) -> Result<(), SlackCliError> {
    match cmd.command {
        ChannelSubcommand::Info { channel } => show_info(client, global, &channel).await,
        ChannelSubcommand::SetTopic { channel, topic } => {
            update_text(
                client,
                global,
                &channel,
                "conversations.setTopic",
                TOPIC_FIELD,
                &topic,
            )
            .await
        }
        ChannelSubcommand::SetPurpose { channel, purpose } => {
            update_text(
                client,
                global,
                &channel,
                "conversations.setPurpose",
                PURPOSE_FIELD,
                &purpose,
            )
            .await
        }
    }
}

async fn show_info(
    client: &SlackClient,
    global: &GlobalOpts,
    channel_input: &str,
) -> Result<(), SlackCliError> {
    let channel_id = resolve_channel_id(client, channel_input).await?;
    let response = client
        .get(
            "conversations.info",
            &[
                ("channel", channel_id.as_str()),
                ("include_num_members", "true"),
            ],
        )
        .await?;

    let format = global.output_format();
    let channel = response.get("channel").cloned().unwrap_or(Value::Null);
    output::format_value(
        &channel_info_value(&channel, format),
        format,
        &mut std::io::stdout(),
    )
}

/// `conversations.info` のレスポンスを出力用の値にする。
///
/// `num_members` が無いときにキーごと落とすのは TS 版と同じ（移植方針 G20）。
/// `created` は機械可読なフォーマットでは Unix 秒のまま、table のときだけ
/// `YYYY-MM-DD`（UTC）にする（移植方針 D2。TS 版はロケール依存の表示だった）。
fn channel_info_value(channel: &Value, format: OutputFormat) -> Value {
    let text = |key: &str| {
        channel
            .get(key)
            .and_then(Value::as_str)
            .map(sanitize_terminal_text)
            .unwrap_or_default()
    };
    let flag = |key: &str| channel.get(key).and_then(Value::as_bool).unwrap_or(false);
    let nested_text = |key: &str| {
        channel
            .get(key)
            .and_then(|v| v.get("value"))
            .and_then(Value::as_str)
            .filter(|v| !v.is_empty())
            .map(|v| Value::String(sanitize_terminal_text(v)))
            .unwrap_or(Value::Null)
    };

    let mut value = Map::new();
    value.insert("id".into(), Value::String(text("id")));
    value.insert("name".into(), Value::String(text("name")));
    value.insert("is_private".into(), Value::Bool(flag("is_private")));
    value.insert("is_archived".into(), Value::Bool(flag("is_archived")));

    if let Some(created) = channel.get("created").and_then(Value::as_i64) {
        let rendered = if format == OutputFormat::Table {
            Value::String(format_created_date(Some(created)))
        } else {
            Value::from(created)
        };
        value.insert("created".into(), rendered);
    }

    if let Some(members) = channel.get("num_members").and_then(Value::as_u64) {
        value.insert("num_members".into(), Value::from(members));
    }

    value.insert(TOPIC_FIELD.into(), nested_text(TOPIC_FIELD));
    value.insert(PURPOSE_FIELD.into(), nested_text(PURPOSE_FIELD));

    Value::Object(value)
}

/// `set-topic` / `set-purpose` の共通処理。渡す値も出す文言もフィールド名だけが違う。
async fn update_text(
    client: &SlackClient,
    global: &GlobalOpts,
    channel_input: &str,
    api_method: &str,
    field: &str,
    text: &str,
) -> Result<(), SlackCliError> {
    let channel_id = resolve_channel_id(client, channel_input).await?;

    let mut body = Map::new();
    body.insert("channel".into(), Value::String(channel_id.clone()));
    body.insert(field.into(), Value::String(text.to_string()));
    client.post_json(api_method, &Value::Object(body)).await?;

    let mut stdout = std::io::stdout();

    let mut result = Map::new();
    result.insert("ok".into(), Value::Bool(true));
    result.insert("channel".into(), Value::String(channel_id));
    result.insert(field.into(), Value::String(text.to_string()));

    write_success(
        &mut stdout,
        global,
        &success_message(field, &format_channel_display(channel_input)),
        &Value::Object(result),
    )
}

fn success_message(field: &str, channel_display: &str) -> String {
    let label = if field == TOPIC_FIELD {
        "Topic"
    } else {
        "Purpose"
    };
    format!("✓ {label} updated for {channel_display}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::Cli;
    use clap::error::ErrorKind;
    use clap::Parser;
    use serde_json::json;
    use url::Url;
    use wiremock::matchers::{any, body_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn client_for(server: &MockServer) -> SlackClient {
        SlackClient::new("test-token-value")
            .unwrap()
            .with_base_url(Url::parse(&format!("{}/", server.uri())).unwrap())
    }

    fn json_opts() -> GlobalOpts {
        GlobalOpts {
            json: true,
            ..GlobalOpts::default()
        }
    }

    fn sample_channel() -> Value {
        json!({
            "id": "C0123456789",
            "name": "general",
            "is_private": false,
            "created": 1_554_076_800_i64,
            "num_members": 128,
            "topic": { "value": "今日の話題" },
            "purpose": { "value": "会社全体のお知らせ" },
        })
    }

    #[test]
    fn parses_every_subcommand() {
        for argv in [
            vec!["slack-cli", "channel", "info", "-c", "general"],
            vec![
                "slack-cli",
                "channel",
                "set-topic",
                "-c",
                "general",
                "--topic",
                "t",
            ],
            vec![
                "slack-cli",
                "channel",
                "set-purpose",
                "-c",
                "general",
                "--purpose",
                "p",
            ],
        ] {
            Cli::try_parse_from(&argv).unwrap_or_else(|e| panic!("{argv:?} failed to parse: {e}"));
        }
    }

    #[test]
    fn set_topic_keeps_the_topic_text() {
        let cli = Cli::try_parse_from([
            "slack-cli",
            "channel",
            "set-topic",
            "-c",
            "general",
            "--topic",
            "release week",
        ])
        .unwrap();
        let crate::cli::Command::Channel(cmd) = cli.command else {
            panic!("expected the channel command");
        };
        let ChannelSubcommand::SetTopic { topic, .. } = cmd.command else {
            panic!("expected set-topic");
        };
        assert_eq!(topic, "release week");
    }

    #[test]
    fn topic_and_purpose_are_required() {
        for argv in [
            vec!["slack-cli", "channel", "set-topic", "-c", "general"],
            vec!["slack-cli", "channel", "set-purpose", "-c", "general"],
        ] {
            let err = Cli::try_parse_from(&argv).expect_err("the text argument is required");
            assert_eq!(err.kind(), ErrorKind::MissingRequiredArgument);
        }
    }

    #[test]
    fn info_json_keeps_the_typescript_shape() {
        let value = channel_info_value(&sample_channel(), OutputFormat::Json);
        assert_eq!(value["id"], "C0123456789");
        assert_eq!(value["is_private"], false);
        // 未定義は false に落とす
        assert_eq!(value["is_archived"], false);
        assert_eq!(value["created"], 1_554_076_800_i64);
        assert_eq!(value["num_members"], 128);
        assert_eq!(value["topic"], "今日の話題");
    }

    #[test]
    fn info_omits_num_members_and_nulls_unset_text() {
        // 移植方針 G20: num_members が無いときはキーごと落とす（TS 版と同じ）
        let value = channel_info_value(
            &json!({ "id": "C0123456789", "name": "general", "topic": { "value": "" } }),
            OutputFormat::Json,
        );
        assert!(value.get("num_members").is_none());
        assert!(value.get("created").is_none());
        assert_eq!(value["topic"], Value::Null);
        assert_eq!(value["purpose"], Value::Null);
    }

    #[test]
    fn info_table_shows_a_fixed_utc_date() {
        // 移植方針 D2: ロケール依存の toLocaleDateString をやめた
        let value = channel_info_value(&sample_channel(), OutputFormat::Table);
        assert_eq!(value["created"], "2019-04-01");
    }

    #[test]
    fn info_sanitizes_values_from_the_api() {
        let value = channel_info_value(
            &json!({
                "id": "C0123456789",
                "name": "\u{1b}[31mgeneral",
                "purpose": { "value": "\u{1b}]0;pwned\u{7}ok" },
            }),
            OutputFormat::Json,
        );
        assert_eq!(value["name"], "general");
        assert_eq!(value["purpose"], "ok");
    }

    #[test]
    fn success_messages_name_the_field_and_the_channel() {
        assert_eq!(
            success_message(TOPIC_FIELD, "#general"),
            "✓ Topic updated for #general"
        );
        assert_eq!(
            success_message(PURPOSE_FIELD, "C0123456789"),
            "✓ Purpose updated for C0123456789"
        );
    }

    #[tokio::test]
    async fn info_resolves_the_name_before_calling_conversations_info() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/conversations.list"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "channels": [{ "id": "C0123456789", "name": "general" }],
                "response_metadata": { "next_cursor": "" },
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/conversations.info"))
            .and(wiremock::matchers::query_param("channel", "C0123456789"))
            .and(wiremock::matchers::query_param(
                "include_num_members",
                "true",
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "channel": sample_channel(),
            })))
            .expect(1)
            .mount(&server)
            .await;

        show_info(&client_for(&server), &json_opts(), "general")
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn set_topic_posts_the_resolved_channel_id() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/conversations.setTopic"))
            .and(body_json(
                json!({ "channel": "C0123456789", "topic": "release week" }),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "ok": true })))
            .expect(1)
            .mount(&server)
            .await;

        update_text(
            &client_for(&server),
            &json_opts(),
            "C0123456789",
            "conversations.setTopic",
            TOPIC_FIELD,
            "release week",
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn set_purpose_propagates_api_errors() {
        let server = MockServer::start().await;
        Mock::given(any())
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": false,
                "error": "not_in_channel",
            })))
            .mount(&server)
            .await;

        let err = update_text(
            &client_for(&server),
            &GlobalOpts::default(),
            "C0123456789",
            "conversations.setPurpose",
            PURPOSE_FIELD,
            "team purpose",
        )
        .await
        .unwrap_err();
        assert_eq!(err.to_string(), "API Error: not_in_channel");
    }

    #[tokio::test]
    async fn run_dispatches_all_three_subcommands() {
        let server = MockServer::start().await;
        Mock::given(any())
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "channel": sample_channel(),
            })))
            .mount(&server)
            .await;
        let client = client_for(&server);

        for command in [
            ChannelSubcommand::Info {
                channel: "C0123456789".into(),
            },
            ChannelSubcommand::SetTopic {
                channel: "C0123456789".into(),
                topic: "t".into(),
            },
            ChannelSubcommand::SetPurpose {
                channel: "C0123456789".into(),
                purpose: "p".into(),
            },
        ] {
            run(ChannelCommand { command }, &client, &json_opts())
                .await
                .unwrap();
        }
        assert_eq!(server.received_requests().await.unwrap().len(), 3);
    }
}
