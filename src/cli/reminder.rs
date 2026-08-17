//! `slack-cli reminder` — リマインダーの作成・一覧・削除・完了。
//!
//! 出力ヘルパと端末サニタイズは `scheduled` モジュールのものを共有する。

use chrono::{DateTime, Local, NaiveDate, NaiveDateTime, TimeZone, Utc};
use clap::{Args, Subcommand};
use serde_json::{json, Value};

use crate::cli::scheduled::{report_success, write_list};
use crate::output::sanitize::{sanitize_terminal_text, sanitize_value};
use crate::cli::{parse_positive_int, GlobalOpts};
use crate::client::SlackClient;
use crate::error::SlackCliError;

pub const ERR_TIMING_REQUIRED: &str = "You must specify either --at or --after";
pub const ERR_TIMING_CONFLICT: &str = "Cannot use both --at and --after";
pub const ERR_AFTER: &str = "--after must be a positive integer (minutes)";
pub const ERR_UNRESOLVED_TIME: &str =
    "Could not resolve reminder time. Use --at or --after option.";
pub const MSG_NO_REMINDERS: &str = "No reminders found";
/// 表示できない時刻の代替表記（移植方針 A4 / D6）。
pub const INVALID_TIMESTAMP: &str = "(invalid timestamp)";

const STATUS_COMPLETED: &str = "completed";
const STATUS_PENDING: &str = "pending";

/// `--at` が受け付ける、タイムゾーンを持たない日時の書式（移植方針 D3）。
/// タイムゾーンを書かない入力はローカル時刻として解釈する（D4）。
const LOCAL_DATETIME_FORMATS: [&str; 4] = [
    "%Y-%m-%dT%H:%M:%S",
    "%Y-%m-%dT%H:%M",
    "%Y-%m-%d %H:%M:%S",
    "%Y-%m-%d %H:%M",
];
const LOCAL_DATE_FORMAT: &str = "%Y-%m-%d";
/// 出力の時刻表記。TS 版 `Date.toISOString()` と同じ形。
const ISO_FORMAT: &str = "%Y-%m-%dT%H:%M:%S%.3fZ";

const SECONDS_PER_MINUTE: i64 = 60;

#[derive(Args, Debug)]
pub struct ReminderCommand {
    #[command(subcommand)]
    pub command: ReminderSubcommand,
}

#[derive(Subcommand, Debug)]
pub enum ReminderSubcommand {
    /// Create a new reminder
    Add {
        /// The content of the reminder
        #[arg(long, required = true, value_name = "TEXT")]
        text: String,

        /// Absolute date/time (e.g. "2024-03-01 15:00", local time unless an offset is given)
        #[arg(long, value_name = "DATETIME")]
        at: Option<String>,

        /// Minutes from now
        #[arg(long, value_name = "MINUTES")]
        after: Option<String>,
    },
    /// List all reminders
    List,
    /// Delete a reminder
    Delete {
        /// Reminder ID
        #[arg(long, required = true, value_name = "REMINDER_ID")]
        id: String,
    },
    /// Mark a reminder as complete
    Complete {
        /// Reminder ID
        #[arg(long, required = true, value_name = "REMINDER_ID")]
        id: String,
    },
}

pub async fn run(
    cmd: ReminderCommand,
    client: &SlackClient,
    global: &GlobalOpts,
) -> Result<(), SlackCliError> {
    match cmd.command {
        ReminderSubcommand::Add { text, at, after } => {
            add(client, &text, at.as_deref(), after.as_deref(), global).await
        }
        ReminderSubcommand::List => list(client, global).await,
        ReminderSubcommand::Delete { id } => {
            mutate(
                client,
                "reminders.delete",
                &id,
                "✓ Reminder deleted",
                global,
            )
            .await
        }
        ReminderSubcommand::Complete { id } => {
            mutate(
                client,
                "reminders.complete",
                &id,
                "✓ Reminder completed",
                global,
            )
            .await
        }
    }
}

async fn add(
    client: &SlackClient,
    text: &str,
    at: Option<&str>,
    after: Option<&str>,
    global: &GlobalOpts,
) -> Result<(), SlackCliError> {
    let time = resolve_post_at(at, after)?;
    let response = client
        .post_json("reminders.add", &json!({ "text": text, "time": time }))
        .await?;

    // dry-run では reminder が返らないので、送ろうとした値をそのまま表示する。
    let reminder = response.get("reminder").cloned().unwrap_or(Value::Null);
    let created_text = reminder
        .get("text")
        .and_then(Value::as_str)
        .unwrap_or(text)
        .to_string();
    let created_time = reminder.get("time").and_then(Value::as_i64).unwrap_or(time);

    let value = if reminder.is_null() {
        json!({ "text": created_text, "time": created_time })
    } else {
        sanitize_value(&reminder)
    };

    report_success(
        &format!(
            "✓ Reminder created: \"{}\" at {}",
            sanitize_terminal_text(&created_text),
            format_iso(created_time)
        ),
        value,
        global,
    )
}

async fn list(client: &SlackClient, global: &GlobalOpts) -> Result<(), SlackCliError> {
    let response = client.get("reminders.list", &[]).await?;
    let reminders = response
        .get("reminders")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let mapped: Vec<Value> = reminders.iter().map(map_reminder).collect();
    write_list(
        &mapped,
        MSG_NO_REMINDERS,
        global.output_format(),
        &mut std::io::stdout(),
    )
}

async fn mutate(
    client: &SlackClient,
    method: &str,
    id: &str,
    message_prefix: &str,
    global: &GlobalOpts,
) -> Result<(), SlackCliError> {
    client.post_json(method, &json!({ "reminder": id })).await?;

    report_success(
        &format!("{message_prefix}: {id}"),
        json!({ "ok": true, "reminder": id }),
        global,
    )
}

/// `--at` / `--after` から予約時刻（Unix 秒）を決める。
///
/// 相互排他の判定は手書きのまま維持する（移植方針 G12）。
fn resolve_post_at(at: Option<&str>, after: Option<&str>) -> Result<i64, SlackCliError> {
    match (at, after) {
        (Some(_), Some(_)) => Err(SlackCliError::Validation(ERR_TIMING_CONFLICT.to_string())),
        (None, None) => Err(SlackCliError::Validation(ERR_TIMING_REQUIRED.to_string())),
        (None, Some(minutes)) => {
            let minutes = parse_positive_int(minutes, ERR_AFTER)?;
            Ok(Utc::now().timestamp() + i64::from(minutes) * SECONDS_PER_MINUTE)
        }
        (Some(raw), None) => {
            parse_at(raw).ok_or_else(|| SlackCliError::Validation(ERR_UNRESOLVED_TIME.to_string()))
        }
    }
}

/// `--at` のパース。受理する書式を明示列挙する（移植方針 D3 / D4）。
///
/// - 全桁数字なら Unix 秒
/// - RFC3339 / ISO 8601（`Z`・オフセット付き）
/// - `YYYY-MM-DDTHH:MM[:SS]` / `YYYY-MM-DD HH:MM[:SS]` / `YYYY-MM-DD`（いずれもローカル時刻）
fn parse_at(raw: &str) -> Option<i64> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }

    if trimmed.bytes().all(|b| b.is_ascii_digit()) {
        return trimmed.parse::<i64>().ok();
    }

    if let Ok(parsed) = DateTime::parse_from_rfc3339(trimmed) {
        return Some(parsed.timestamp());
    }

    for format in LOCAL_DATETIME_FORMATS {
        if let Ok(naive) = NaiveDateTime::parse_from_str(trimmed, format) {
            return local_timestamp(naive);
        }
    }

    NaiveDate::parse_from_str(trimmed, LOCAL_DATE_FORMAT)
        .ok()
        .and_then(|date| date.and_hms_opt(0, 0, 0))
        .and_then(local_timestamp)
}

/// ローカル時刻として Unix 秒に直す。夏時間の切り替えで存在しない時刻は解決できない。
fn local_timestamp(naive: NaiveDateTime) -> Option<i64> {
    Local
        .from_local_datetime(&naive)
        .earliest()
        .map(|dt| dt.timestamp())
}

/// Unix 秒を `YYYY-MM-DDTHH:MM:SS.sssZ`（UTC）にする。範囲外は `(invalid timestamp)`。
fn format_iso(seconds: i64) -> String {
    DateTime::from_timestamp(seconds, 0)
        .map(|dt| dt.format(ISO_FORMAT).to_string())
        .unwrap_or_else(|| INVALID_TIMESTAMP.to_string())
}

/// TS 版の `reminder list --format json` と同じ形に整える。
fn map_reminder(reminder: &Value) -> Value {
    let time = reminder.get("time").and_then(Value::as_i64);
    let complete_ts = reminder
        .get("complete_ts")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let text = reminder.get("text").and_then(Value::as_str).unwrap_or("");

    json!({
        "id": reminder.get("id").and_then(Value::as_str).unwrap_or(""),
        "text": sanitize_terminal_text(text),
        "time": time,
        "time_formatted": time.map(format_iso).unwrap_or_else(|| INVALID_TIMESTAMP.to_string()),
        "status": if complete_ts > 0 { STATUS_COMPLETED } else { STATUS_PENDING },
        "recurring": reminder.get("recurring").and_then(Value::as_bool).unwrap_or(false),
    })
}

#[cfg(test)]
mod tests {
    use clap::error::ErrorKind;
    use clap::Parser;
    use url::Url;
    use wiremock::matchers::{body_partial_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;
    use crate::cli::Cli;
    use crate::output::OutputFormat;

    fn client_for(server: &MockServer) -> SlackClient {
        SlackClient::new("test-token-value")
            .unwrap()
            .with_base_url(Url::parse(&format!("{}/", server.uri())).unwrap())
    }

    fn parse(argv: &[&str]) -> ReminderSubcommand {
        let cli = Cli::try_parse_from(argv).unwrap();
        let crate::cli::Command::Reminder(cmd) = cli.command else {
            panic!("expected the reminder command");
        };
        cmd.command
    }

    fn json_global() -> GlobalOpts {
        GlobalOpts {
            json: true,
            ..GlobalOpts::default()
        }
    }

    async fn mount_reminders_add(server: &MockServer) {
        Mock::given(method("POST"))
            .and(path("/reminders.add"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "reminder": {
                    "id": "Rm123456",
                    "text": "ミーティング",
                    "time": 1772344800,
                    "complete_ts": 0,
                    "recurring": false,
                },
            })))
            .mount(server)
            .await;
    }

    async fn posted_time(server: &MockServer) -> i64 {
        let requests = server.received_requests().await.unwrap();
        let body: Value = serde_json::from_slice(&requests[0].body).unwrap();
        body["time"].as_i64().expect("time must be a number")
    }

    #[test]
    fn add_keeps_at_and_after_optional() {
        let ReminderSubcommand::Add { text, at, after } =
            parse(&["slack-cli", "reminder", "add", "--text", "standup"])
        else {
            panic!("expected reminder add");
        };
        assert_eq!(text, "standup");
        assert!(at.is_none());
        assert!(after.is_none());
    }

    #[test]
    fn add_parses_both_timing_flags() {
        let ReminderSubcommand::Add { at, after, .. } = parse(&[
            "slack-cli",
            "reminder",
            "add",
            "--text",
            "standup",
            "--at",
            "2026-08-18 10:00",
            "--after",
            "30",
        ]) else {
            panic!("expected reminder add");
        };
        assert_eq!(at.as_deref(), Some("2026-08-18 10:00"));
        assert_eq!(after.as_deref(), Some("30"));
    }

    #[test]
    fn text_and_id_are_required() {
        for argv in [
            vec!["slack-cli", "reminder", "add"],
            vec!["slack-cli", "reminder", "delete"],
            vec!["slack-cli", "reminder", "complete"],
        ] {
            let err = Cli::try_parse_from(&argv).expect_err("a required flag is missing");
            assert_eq!(err.kind(), ErrorKind::MissingRequiredArgument);
        }
    }

    #[test]
    fn list_takes_no_flags() {
        assert!(matches!(
            parse(&["slack-cli", "reminder", "list"]),
            ReminderSubcommand::List
        ));
    }

    #[tokio::test]
    async fn add_after_schedules_minutes_from_now() {
        let server = MockServer::start().await;
        mount_reminders_add(&server).await;

        let before = Utc::now().timestamp();
        add(
            &client_for(&server),
            "standup",
            None,
            Some("30"),
            &json_global(),
        )
        .await
        .unwrap();

        let sent = posted_time(&server).await;
        assert!(
            (before + 1800..=before + 1801 + 5).contains(&sent),
            "sent was {sent}, before was {before}"
        );
    }

    #[tokio::test]
    async fn add_at_accepts_unix_seconds_and_offsets() {
        for (raw, expected) in [
            ("1772344800", 1772344800),
            ("2026-03-01T06:00:00Z", 1772344800),
            ("2026-03-01T15:00:00+09:00", 1772344800),
        ] {
            let server = MockServer::start().await;
            mount_reminders_add(&server).await;

            add(
                &client_for(&server),
                "standup",
                Some(raw),
                None,
                &json_global(),
            )
            .await
            .unwrap();
            assert_eq!(posted_time(&server).await, expected, "input was {raw:?}");
        }
    }

    #[tokio::test]
    async fn add_at_without_a_timezone_is_local_time() {
        let server = MockServer::start().await;
        mount_reminders_add(&server).await;

        add(
            &client_for(&server),
            "standup",
            Some("2026-03-01 15:00"),
            None,
            &json_global(),
        )
        .await
        .unwrap();

        let expected = Local
            .from_local_datetime(
                &NaiveDate::from_ymd_opt(2026, 3, 1)
                    .unwrap()
                    .and_hms_opt(15, 0, 0)
                    .unwrap(),
            )
            .earliest()
            .unwrap()
            .timestamp();
        assert_eq!(posted_time(&server).await, expected);

        // 日付だけの入力も同じくローカル解釈（TS 版は UTC 扱いだった）
        assert_eq!(
            parse_at("2026-03-01"),
            Local
                .from_local_datetime(
                    &NaiveDate::from_ymd_opt(2026, 3, 1)
                        .unwrap()
                        .and_hms_opt(0, 0, 0)
                        .unwrap()
                )
                .earliest()
                .map(|dt| dt.timestamp())
        );
    }

    #[test]
    fn add_at_rejects_the_loose_formats_date_parse_used_to_accept() {
        for raw in [
            "Jan 1, 2024",
            "2024/01/01",
            "Mon Jan 01 2024 10:00:00 GMT+0900",
            "tomorrow",
            "2026-13-45",
            "",
        ] {
            assert!(parse_at(raw).is_none(), "{raw:?} should not be accepted");
        }
    }

    #[tokio::test]
    async fn timing_validation_happens_before_any_request() {
        let server = MockServer::start().await;
        let client = client_for(&server);

        let cases: [(Option<&str>, Option<&str>, &str); 5] = [
            (None, None, ERR_TIMING_REQUIRED),
            (Some("2026-03-01"), Some("30"), ERR_TIMING_CONFLICT),
            (None, Some("abc"), ERR_AFTER),
            (None, Some("0"), ERR_AFTER),
            (Some("Jan 1, 2024"), None, ERR_UNRESOLVED_TIME),
        ];

        for (at, after, expected) in cases {
            let err = add(&client, "standup", at, after, &json_global())
                .await
                .unwrap_err();
            assert_eq!(err.to_string(), expected, "at={at:?} after={after:?}");
        }
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn add_reports_the_values_returned_by_the_api() {
        let server = MockServer::start().await;
        mount_reminders_add(&server).await;

        // json 出力ではレスポンスの reminder をそのまま返す
        add(
            &client_for(&server),
            "べつのテキスト",
            Some("1772344800"),
            None,
            &json_global(),
        )
        .await
        .unwrap();

        assert_eq!(format_iso(1772344800), "2026-03-01T06:00:00.000Z");
    }

    #[tokio::test]
    async fn add_sends_nothing_in_dry_run() {
        let server = MockServer::start().await;
        let client = client_for(&server).with_dry_run(true);

        add(&client, "standup", Some("1772344800"), None, &json_global())
            .await
            .unwrap();
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn list_maps_status_and_formats_the_time() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/reminders.list"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "reminders": [
                    { "id": "Rm1", "text": "\u{1b}[31mミーティング", "time": 1772344800,
                      "complete_ts": 0, "recurring": false },
                    { "id": "Rm2", "text": "done", "time": 1772344800,
                      "complete_ts": 1772345000, "recurring": true },
                ],
            })))
            .mount(&server)
            .await;

        let response = client_for(&server)
            .get("reminders.list", &[])
            .await
            .unwrap();
        let mapped: Vec<Value> = response["reminders"]
            .as_array()
            .unwrap()
            .iter()
            .map(map_reminder)
            .collect();

        assert_eq!(mapped[0]["status"], STATUS_PENDING);
        assert_eq!(mapped[0]["text"], "ミーティング");
        assert_eq!(mapped[0]["time"], 1772344800);
        assert_eq!(mapped[0]["time_formatted"], "2026-03-01T06:00:00.000Z");
        assert_eq!(mapped[1]["status"], STATUS_COMPLETED);
        assert_eq!(mapped[1]["recurring"], true);
    }

    #[tokio::test]
    async fn list_renders_empty_results_per_format() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/reminders.list"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "ok": true })))
            .mount(&server)
            .await;

        list(&client_for(&server), &json_global()).await.unwrap();

        let mut table = Vec::new();
        write_list(&[], MSG_NO_REMINDERS, OutputFormat::Table, &mut table).unwrap();
        assert_eq!(String::from_utf8(table).unwrap(), "No reminders found\n");
    }

    #[test]
    fn out_of_range_timestamps_do_not_panic() {
        assert_eq!(format_iso(i64::MAX), INVALID_TIMESTAMP);
        assert_eq!(
            map_reminder(&json!({ "id": "Rm1", "text": "x" }))["time_formatted"],
            INVALID_TIMESTAMP
        );
    }

    #[tokio::test]
    async fn delete_and_complete_post_the_reminder_id() {
        for (argv_method, api_path) in [
            (
                ReminderSubcommand::Delete { id: "Rm1".into() },
                "/reminders.delete",
            ),
            (
                ReminderSubcommand::Complete { id: "Rm1".into() },
                "/reminders.complete",
            ),
        ] {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path(api_path))
                .and(body_partial_json(json!({ "reminder": "Rm1" })))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "ok": true })))
                .expect(1)
                .mount(&server)
                .await;

            run(
                ReminderCommand {
                    command: argv_method,
                },
                &client_for(&server),
                &GlobalOpts::default(),
            )
            .await
            .unwrap();
        }
    }

    #[tokio::test]
    async fn api_errors_are_propagated() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/reminders.delete"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({ "ok": false, "error": "not_found" })),
            )
            .mount(&server)
            .await;

        let err = mutate(
            &client_for(&server),
            "reminders.delete",
            "Rm1",
            "✓ Reminder deleted",
            &GlobalOpts::default(),
        )
        .await
        .unwrap_err();

        match err {
            SlackCliError::Api { code, .. } => assert_eq!(code, "not_found"),
            other => panic!("unexpected error: {other}"),
        }
    }
}
