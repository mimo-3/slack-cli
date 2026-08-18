//! `slack-cli send` — チャンネル / DM へのメッセージ送信と予約送信。
//!
//! チャンネル名の扱いは移植方針 G1 に従う。まず利用者の指定値をそのまま Slack に渡し、
//! `channel_not_found` が返ったときだけ `conversations.list` で名前解決して 1 度だけ再送する。
//! 先に解決を挟まないのは、`channels:read` が無いトークンでの送信を壊さないため。

use std::io::Write;

use chrono::{Local, NaiveDate, NaiveDateTime, TimeZone, Utc};
use clap::Args;
use serde_json::{json, Value};

use crate::cli::common::{
    channel_label as format_channel_label, fetch_lookup_channels, find_channel_id, is_channel_id,
    is_message_ts, not_found_error, write_success,
};
use crate::cli::{parse_positive_int, GlobalOpts};
use crate::client::pagination::PaginationOpts;
use crate::client::SlackClient;
use crate::error::SlackCliError;

pub const ERR_NO_TARGET: &str = "You must specify one of: --channel, --user, or --email";
pub const ERR_CHANNEL_WITH_DM: &str = "Cannot use --channel with --user or --email";
pub const ERR_USER_AND_EMAIL: &str = "Cannot use --user and --email together";
pub const ERR_NO_MESSAGE: &str = "You must specify either --message or --file";
pub const ERR_MESSAGE_AND_FILE: &str = "Cannot use both --message and --file";
pub const ERR_BOTH_BLOCKS: &str = "Cannot use both --blocks and --blocks-file";
pub const ERR_INVALID_BLOCKS: &str = "Invalid blocks JSON: must be a valid JSON array";
pub const ERR_INVALID_THREAD_TS: &str = "Invalid thread timestamp format";
pub const ERR_BOTH_SCHEDULE: &str = "Cannot use both --at and --after";
pub const ERR_INVALID_AT: &str =
    "Invalid schedule time format. Use Unix timestamp (seconds) or ISO 8601 date-time";
pub const ERR_PAST_SCHEDULE: &str = "Schedule time must be in the future";
pub const ERR_INVALID_AFTER: &str = "--after must be a positive integer (minutes)";

/// `channel_not_found` フォールバック（移植方針 G1）の起点になる Slack エラーコード。
const CHANNEL_NOT_FOUND: &str = "channel_not_found";
const USER_LOOKUP_PAGE_SIZE: u32 = 200;
/// dry-run では `conversations.open` が実行されないため、DM 先の代わりに置く印。
const DRY_RUN_CHANNEL: &str = "(dry-run)";

#[derive(Args, Debug)]
pub struct SendCommand {
    /// Target channel name or ID
    #[arg(short, long, value_name = "CHANNEL")]
    pub channel: Option<String>,

    /// Send DM to user by username
    #[arg(long, value_name = "USERNAME")]
    pub user: Option<String>,

    /// Send DM to user by email address
    #[arg(long, value_name = "EMAIL")]
    pub email: Option<String>,

    /// Message to send
    #[arg(short, long, value_name = "MESSAGE")]
    pub message: Option<String>,

    /// File containing message content
    #[arg(short, long, value_name = "FILE")]
    pub file: Option<String>,

    /// Block Kit JSON array string
    #[arg(short, long, value_name = "JSON")]
    pub blocks: Option<String>,

    /// File containing Block Kit JSON array
    #[arg(long, value_name = "FILE")]
    pub blocks_file: Option<String>,

    /// Thread timestamp to reply to
    #[arg(short, long, value_name = "THREAD")]
    pub thread: Option<String>,

    /// Schedule time. Accepts Unix seconds, RFC3339, `YYYY-MM-DD[ HH:MM[:SS]]`.
    /// A value without a timezone is read as local time
    #[arg(long, value_name = "TIME")]
    pub at: Option<String>,

    /// Schedule message after N minutes
    #[arg(long, value_name = "MINUTES")]
    pub after: Option<String>,
}

/// 送信先の指定方法。3 つのうちちょうど 1 つだけが選べる。
#[derive(Debug, PartialEq, Eq)]
enum Target {
    Channel(String),
    User(String),
    Email(String),
}

pub async fn run(
    cmd: SendCommand,
    client: &SlackClient,
    global: &GlobalOpts,
) -> Result<(), SlackCliError> {
    // 検証は TypeScript 版のバリデータと同じ順序で、最初の 1 件だけ報告する。
    let target = validate_target(&cmd)?;
    validate_message_or_file(&cmd)?;
    let inline_blocks = validate_blocks(&cmd)?;
    validate_thread_ts(cmd.thread.as_deref())?;
    let post_at = resolve_post_at(
        cmd.at.as_deref(),
        cmd.after.as_deref(),
        Utc::now().timestamp(),
    )?;

    let text = read_message_text(&cmd)?;
    let blocks = match &cmd.blocks_file {
        Some(path) => Some(read_blocks_file(path)?),
        None => inline_blocks,
    };

    let (channel, label) = resolve_target(client, &target).await?;

    let mut body = json!({ "channel": channel, "text": text });
    if let Some(thread) = &cmd.thread {
        insert(&mut body, "thread_ts", Value::String(thread.clone()));
    }
    if let Some(blocks) = blocks {
        insert(&mut body, "blocks", blocks);
    }

    let (method, message) = match post_at {
        Some(post_at) => {
            insert(&mut body, "post_at", Value::from(post_at));
            (
                "chat.scheduleMessage",
                format!(
                    "✓ Message scheduled to {label} at {}",
                    format_iso8601_millis(post_at)
                ),
            )
        }
        None => match target {
            Target::Channel(_) => (
                "chat.postMessage",
                format!("✓ Message sent successfully to {label}"),
            ),
            _ => ("chat.postMessage", format!("✓ DM sent to {label}")),
        },
    };

    // DM は解決済みのチャンネル ID なので、名前解決フォールバックの対象にしない。
    let response = match &target {
        Target::Channel(raw) => post_with_channel_fallback(client, method, &body, raw).await?,
        _ => client.post_json(method, &body).await?,
    };

    finish(global, &message, &response)
}

/// 成功時の出力。`--format table`（既定）は TypeScript 版と同じ 1 行の成功メッセージ、
/// 機械可読フォーマットを明示されたときは Slack のレスポンスをそのまま流す。
pub(crate) fn finish(
    global: &GlobalOpts,
    message: &str,
    response: &Value,
) -> Result<(), SlackCliError> {
    let mut stdout = std::io::stdout();
    write_success(&mut stdout, global, message, response)?;
    stdout.flush()?;
    Ok(())
}

/// 移植方針 G1 のフォールバック送信。生値で 1 回送り、`channel_not_found` のときだけ
/// 名前解決して 1 回だけ再送する。
pub(crate) async fn post_with_channel_fallback(
    client: &SlackClient,
    method: &str,
    body: &Value,
    raw_channel: &str,
) -> Result<Value, SlackCliError> {
    let error = match client.post_json(method, body).await {
        Ok(response) => return Ok(response),
        Err(error) => error,
    };

    let is_channel_not_found =
        matches!(&error, SlackCliError::Api { code, .. } if code == CHANNEL_NOT_FOUND);
    if !is_channel_not_found || is_channel_id(raw_channel) {
        return Err(error);
    }

    // 一覧が引けない（スコープ不足など）ときは、話をすり替えず元のエラーを返す。
    let Ok(channels) = fetch_lookup_channels(client).await else {
        return Err(error);
    };
    let Some(resolved) = find_channel_id(&channels, raw_channel) else {
        return Err(not_found_error(raw_channel, &channels));
    };

    let mut retried = body.clone();
    insert(&mut retried, "channel", Value::String(resolved));
    client.post_json(method, &retried).await
}

/// `--thread` / `--timestamp` の形式検証（`^\d{10}\.\d{6}$`）。
pub(crate) fn validate_thread_ts(thread: Option<&str>) -> Result<(), SlackCliError> {
    match thread {
        Some(value) if !is_message_ts(value) => {
            Err(SlackCliError::Validation(ERR_INVALID_THREAD_TS.to_string()))
        }
        _ => Ok(()),
    }
}

fn validate_target(cmd: &SendCommand) -> Result<Target, SlackCliError> {
    match (&cmd.channel, &cmd.user, &cmd.email) {
        (None, None, None) => Err(SlackCliError::Validation(ERR_NO_TARGET.to_string())),
        (Some(_), Some(_), _) | (Some(_), _, Some(_)) => {
            Err(SlackCliError::Validation(ERR_CHANNEL_WITH_DM.to_string()))
        }
        (None, Some(_), Some(_)) => Err(SlackCliError::Validation(ERR_USER_AND_EMAIL.to_string())),
        (Some(channel), None, None) => Ok(Target::Channel(channel.clone())),
        (None, Some(user), None) => Ok(Target::User(user.clone())),
        (None, None, Some(email)) => Ok(Target::Email(email.clone())),
    }
}

fn validate_message_or_file(cmd: &SendCommand) -> Result<(), SlackCliError> {
    let has_body = cmd.message.is_some()
        || cmd.file.is_some()
        || cmd.blocks.is_some()
        || cmd.blocks_file.is_some();
    if !has_body {
        return Err(SlackCliError::Validation(ERR_NO_MESSAGE.to_string()));
    }
    if cmd.message.is_some() && cmd.file.is_some() {
        return Err(SlackCliError::Validation(ERR_MESSAGE_AND_FILE.to_string()));
    }
    Ok(())
}

/// `--blocks` / `--blocks-file` の排他と、`--blocks` のパースを済ませる。
/// TypeScript 版は検証と本体で 2 回パースしていたが、ここでは 1 回で持ち回る。
fn validate_blocks(cmd: &SendCommand) -> Result<Option<Value>, SlackCliError> {
    if cmd.blocks.is_some() && cmd.blocks_file.is_some() {
        return Err(SlackCliError::Validation(ERR_BOTH_BLOCKS.to_string()));
    }
    let Some(raw) = &cmd.blocks else {
        return Ok(None);
    };

    let parsed: Value = serde_json::from_str(raw)
        .map_err(|_| SlackCliError::Validation(ERR_INVALID_BLOCKS.to_string()))?;
    if !parsed.is_array() {
        return Err(SlackCliError::Validation(ERR_INVALID_BLOCKS.to_string()));
    }
    Ok(Some(parsed))
}

/// `--at` / `--after` を Unix 秒に落とす。指定が無ければ即時送信（`None`）。
fn resolve_post_at(
    at: Option<&str>,
    after: Option<&str>,
    now: i64,
) -> Result<Option<i64>, SlackCliError> {
    if at.is_some() && after.is_some() {
        return Err(SlackCliError::Validation(ERR_BOTH_SCHEDULE.to_string()));
    }

    if let Some(at) = at {
        let post_at = parse_schedule_time(at)?;
        if post_at <= now {
            return Err(SlackCliError::Validation(ERR_PAST_SCHEDULE.to_string()));
        }
        return Ok(Some(post_at));
    }

    if let Some(after) = after {
        let minutes = i64::from(parse_positive_int(after, ERR_INVALID_AFTER)?);
        let post_at = minutes
            .checked_mul(60)
            .and_then(|seconds| now.checked_add(seconds))
            .ok_or_else(|| SlackCliError::Validation(ERR_INVALID_AFTER.to_string()))?;
        return Ok(Some(post_at));
    }

    Ok(None)
}

/// 受理する形式を明示列挙する（移植方針 D3）。タイムゾーンの無い入力はローカル時刻として
/// 解釈する（移植方針 D4）。
fn parse_schedule_time(raw: &str) -> Result<i64, SlackCliError> {
    let trimmed = raw.trim();
    let invalid = || SlackCliError::Validation(ERR_INVALID_AT.to_string());

    if !trimmed.is_empty() && trimmed.bytes().all(|b| b.is_ascii_digit()) {
        return trimmed.parse::<i64>().map_err(|_| invalid());
    }

    if let Ok(parsed) = chrono::DateTime::parse_from_rfc3339(trimmed) {
        return Ok(parsed.timestamp());
    }

    const NAIVE_FORMATS: [&str; 4] = [
        "%Y-%m-%dT%H:%M:%S",
        "%Y-%m-%dT%H:%M",
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%d %H:%M",
    ];
    for format in NAIVE_FORMATS {
        if let Ok(naive) = NaiveDateTime::parse_from_str(trimmed, format) {
            return local_timestamp(naive).ok_or_else(invalid);
        }
    }

    if let Ok(date) = NaiveDate::parse_from_str(trimmed, "%Y-%m-%d") {
        let naive = date.and_hms_opt(0, 0, 0).ok_or_else(invalid)?;
        return local_timestamp(naive).ok_or_else(invalid);
    }

    Err(invalid())
}

/// 夏時間の切り替えで曖昧・不在になる時刻は、早いほうの解釈を採る。
fn local_timestamp(naive: NaiveDateTime) -> Option<i64> {
    Local
        .from_local_datetime(&naive)
        .earliest()
        .map(|dt| dt.timestamp())
}

/// `toISOString()` と同じミリ秒 3 桁 + `Z`。
fn format_iso8601_millis(unix_seconds: i64) -> String {
    match Utc.timestamp_opt(unix_seconds, 0).single() {
        Some(dt) => dt.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string(),
        None => "(invalid timestamp)".to_string(),
    }
}

fn read_message_text(cmd: &SendCommand) -> Result<String, SlackCliError> {
    match &cmd.file {
        Some(path) => read_lossy(path)
            .map_err(|e| SlackCliError::File(format!("Error reading file {path}: {e}"))),
        None => Ok(cmd.message.clone().unwrap_or_default()),
    }
}

fn read_blocks_file(path: &str) -> Result<Value, SlackCliError> {
    let raw = read_lossy(path)
        .map_err(|e| SlackCliError::File(format!("Error reading blocks file {path}: {e}")))?;
    let parsed: Value = serde_json::from_str(&raw)
        .map_err(|_| SlackCliError::File(ERR_INVALID_BLOCKS.to_string()))?;
    if !parsed.is_array() {
        return Err(SlackCliError::File(format!(
            "Error reading blocks file {path}: blocks must be a JSON array"
        )));
    }
    Ok(parsed)
}

/// Node の `readFile(path, 'utf-8')` に合わせ、不正な UTF-8 は U+FFFD に置換して読む。
fn read_lossy(path: &str) -> Result<String, std::io::Error> {
    let bytes = std::fs::read(path)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

async fn resolve_target(
    client: &SlackClient,
    target: &Target,
) -> Result<(String, String), SlackCliError> {
    match target {
        Target::Channel(channel) => Ok((channel.clone(), format_channel_label(channel))),
        Target::User(user) => {
            let name = user.strip_prefix('@').unwrap_or(user).to_string();
            let user_id = resolve_user_id_by_name(client, &name).await?;
            let channel = open_dm_channel(client, &user_id).await?;
            Ok((channel, format!("@{name}")))
        }
        Target::Email(email) => {
            let response = client
                .get("users.lookupByEmail", &[("email", email.as_str())])
                .await?;
            let user_id = response
                .get("user")
                .and_then(|user| user.get("id"))
                .and_then(Value::as_str)
                .ok_or_else(|| SlackCliError::Validation(format!("User '{email}' not found")))?;
            let channel = open_dm_channel(client, user_id).await?;
            Ok((channel, email.clone()))
        }
    }
}

async fn resolve_user_id_by_name(
    client: &SlackClient,
    name: &str,
) -> Result<String, SlackCliError> {
    let wanted = name.to_lowercase();
    let members = client
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
        .await?;

    members
        .iter()
        .find(|member| {
            member
                .get("name")
                .and_then(Value::as_str)
                .is_some_and(|n| n.to_lowercase() == wanted)
        })
        .and_then(|member| member.get("id").and_then(Value::as_str))
        .map(str::to_string)
        .ok_or_else(|| SlackCliError::Validation(format!("User '{name}' not found")))
}

async fn open_dm_channel(client: &SlackClient, user_id: &str) -> Result<String, SlackCliError> {
    let response = client
        .post_json("conversations.open", &json!({ "users": user_id }))
        .await?;

    // dry-run では実際に開かれないため、以降の書き込みも送られない印を返す。
    if response.get("dry_run").and_then(Value::as_bool) == Some(true) {
        return Ok(DRY_RUN_CHANNEL.to_string());
    }

    response
        .get("channel")
        .and_then(|channel| channel.get("id"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| SlackCliError::Api {
            status: 200,
            code: "no_dm_channel".to_string(),
            needed: Vec::new(),
        })
}

fn insert(body: &mut Value, key: &str, value: Value) {
    if let Some(object) = body.as_object_mut() {
        object.insert(key.to_string(), value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::output::sanitize::sanitize_terminal_text;
    use crate::output::{self, OutputFormat};
    use clap::Parser;
    use url::Url;
    use wiremock::matchers::{body_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use crate::cli::Cli;

    fn client_for(server: &MockServer) -> SlackClient {
        SlackClient::new("test-token-value")
            .unwrap()
            .with_base_url(Url::parse(&format!("{}/", server.uri())).unwrap())
    }

    fn parse(argv: &[&str]) -> SendCommand {
        let cli = Cli::try_parse_from(argv).expect("arguments should parse");
        match cli.command {
            crate::cli::Command::Send(cmd) => cmd,
            _ => panic!("expected the send command"),
        }
    }

    fn table_opts() -> GlobalOpts {
        GlobalOpts::default()
    }

    async fn mount_ok(server: &MockServer, api: &str) {
        Mock::given(method("POST"))
            .and(path(format!("/{api}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "channel": "C0123456789",
                "ts": "1700000000.000100",
            })))
            .mount(server)
            .await;
    }

    #[test]
    fn accepts_every_documented_flag() {
        let cmd = parse(&[
            "slack-cli",
            "send",
            "-c",
            "general",
            "-m",
            "hi",
            "-b",
            "[]",
            "-t",
            "1700000000.000100",
            "--at",
            "1700000000",
        ]);
        assert_eq!(cmd.channel.as_deref(), Some("general"));
        assert_eq!(cmd.blocks.as_deref(), Some("[]"));
    }

    #[test]
    fn every_target_flag_is_optional_at_parse_time() {
        // 相互排他と必須性は run() の手書きバリデーションで見る（移植方針 G12）
        Cli::try_parse_from(["slack-cli", "send"]).unwrap();
    }

    #[test]
    fn target_validation_reports_the_first_violation() {
        let cases = [
            (vec!["slack-cli", "send", "-m", "hi"], ERR_NO_TARGET),
            (
                vec![
                    "slack-cli",
                    "send",
                    "-c",
                    "general",
                    "--user",
                    "alice",
                    "-m",
                    "hi",
                ],
                ERR_CHANNEL_WITH_DM,
            ),
            (
                vec![
                    "slack-cli",
                    "send",
                    "-c",
                    "general",
                    "--email",
                    "a@example.com",
                    "-m",
                    "hi",
                ],
                ERR_CHANNEL_WITH_DM,
            ),
            (
                vec![
                    "slack-cli",
                    "send",
                    "--user",
                    "alice",
                    "--email",
                    "a@example.com",
                    "-m",
                    "hi",
                ],
                ERR_USER_AND_EMAIL,
            ),
        ];

        for (argv, expected) in cases {
            let err = validate_target(&parse(&argv)).unwrap_err();
            assert_eq!(err.to_string(), expected, "{argv:?}");
        }
    }

    #[test]
    fn body_validation_requires_message_file_or_blocks() {
        let err =
            validate_message_or_file(&parse(&["slack-cli", "send", "-c", "general"])).unwrap_err();
        assert_eq!(err.to_string(), ERR_NO_MESSAGE);

        let err = validate_message_or_file(&parse(&[
            "slack-cli",
            "send",
            "-c",
            "general",
            "-m",
            "hi",
            "-f",
            "body.txt",
        ]))
        .unwrap_err();
        assert_eq!(err.to_string(), ERR_MESSAGE_AND_FILE);

        // blocks だけの指定は許容される（text は空文字になる）
        validate_message_or_file(&parse(&["slack-cli", "send", "-c", "general", "-b", "[]"]))
            .unwrap();
    }

    #[test]
    fn blocks_validation_rejects_conflicts_and_non_arrays() {
        let err = validate_blocks(&parse(&[
            "slack-cli",
            "send",
            "-c",
            "general",
            "-b",
            "[]",
            "--blocks-file",
            "blocks.json",
        ]))
        .unwrap_err();
        assert_eq!(err.to_string(), ERR_BOTH_BLOCKS);

        for raw in ["{not json", "{\"a\":1}"] {
            let err = validate_blocks(&parse(&["slack-cli", "send", "-c", "general", "-b", raw]))
                .unwrap_err();
            assert_eq!(err.to_string(), ERR_INVALID_BLOCKS, "{raw}");
        }

        let parsed = validate_blocks(&parse(&[
            "slack-cli",
            "send",
            "-c",
            "general",
            "-b",
            "[{\"type\":\"divider\"}]",
        ]))
        .unwrap();
        assert_eq!(parsed.unwrap().as_array().unwrap().len(), 1);
    }

    #[test]
    fn thread_timestamps_must_be_ten_dot_six_digits() {
        validate_thread_ts(None).unwrap();
        validate_thread_ts(Some("1700000000.000100")).unwrap();
        for raw in [
            "1700000000",
            "170000000.000100",
            "1700000000.0001",
            "abc.def",
            "1700000000.00.100",
        ] {
            let err = validate_thread_ts(Some(raw)).unwrap_err();
            assert_eq!(err.to_string(), ERR_INVALID_THREAD_TS, "{raw}");
        }
    }

    #[test]
    fn schedule_accepts_unix_seconds_and_explicit_formats() {
        let now = 1_700_000_000;
        assert_eq!(
            resolve_post_at(Some("1700000600"), None, now).unwrap(),
            Some(1_700_000_600)
        );
        assert_eq!(
            resolve_post_at(Some("2030-01-02T03:04:05Z"), None, now).unwrap(),
            Some(1_893_553_445)
        );

        // タイムゾーン無しの入力はローカル時刻として解釈する（移植方針 D4）
        let local = resolve_post_at(Some("2030-01-02 03:04:05"), None, now)
            .unwrap()
            .unwrap();
        let expected = Local
            .from_local_datetime(
                &NaiveDateTime::parse_from_str("2030-01-02 03:04:05", "%Y-%m-%d %H:%M:%S").unwrap(),
            )
            .earliest()
            .unwrap()
            .timestamp();
        assert_eq!(local, expected);

        assert!(resolve_post_at(Some("2030-01-02"), None, now)
            .unwrap()
            .is_some());
    }

    #[test]
    fn schedule_rejects_conflicts_loose_formats_and_the_past() {
        let now = 1_700_000_000;
        assert_eq!(
            resolve_post_at(Some("1700000600"), Some("5"), now)
                .unwrap_err()
                .to_string(),
            ERR_BOTH_SCHEDULE
        );
        for raw in ["Jan 1, 2030", "2030/01/02", "", "12abc"] {
            assert_eq!(
                resolve_post_at(Some(raw), None, now)
                    .unwrap_err()
                    .to_string(),
                ERR_INVALID_AT,
                "{raw}"
            );
        }
        assert_eq!(
            resolve_post_at(Some("1699999999"), None, now)
                .unwrap_err()
                .to_string(),
            ERR_PAST_SCHEDULE
        );
        assert_eq!(
            resolve_post_at(Some(&now.to_string()), None, now)
                .unwrap_err()
                .to_string(),
            ERR_PAST_SCHEDULE
        );
        for raw in ["0", "-5", "3.5", "abc"] {
            assert_eq!(
                resolve_post_at(None, Some(raw), now)
                    .unwrap_err()
                    .to_string(),
                ERR_INVALID_AFTER,
                "{raw}"
            );
        }
        assert_eq!(
            resolve_post_at(None, Some("30"), now).unwrap(),
            Some(now + 1800)
        );
        assert_eq!(resolve_post_at(None, None, now).unwrap(), None);
    }

    #[test]
    fn iso8601_output_always_has_three_millisecond_digits() {
        assert_eq!(
            format_iso8601_millis(1_700_000_000),
            "2023-11-14T22:13:20.000Z"
        );
    }

    #[test]
    fn channel_labels_do_not_double_the_hash_or_decorate_ids() {
        assert_eq!(format_channel_label("general"), "#general");
        assert_eq!(format_channel_label("#general"), "#general");
        assert_eq!(format_channel_label("C0123456789"), "C0123456789");
    }

    #[test]
    fn channel_ids_need_a_prefix_and_eight_more_characters() {
        assert!(is_channel_id("C12345678"));
        assert!(is_channel_id("D0123456789"));
        assert!(!is_channel_id("C1234567"));
        assert!(!is_channel_id("c012345678"));
        assert!(!is_channel_id("general"));
        assert!(!is_channel_id(""));
    }

    #[test]
    fn terminal_sequences_are_stripped_from_messages() {
        let dirty = "gene\u{1b}[31mral\u{1b}]0;title\u{7}\u{7}\tok\n";
        assert_eq!(sanitize_terminal_text(dirty), "general\tok\n");
    }

    #[test]
    fn blocks_file_errors_keep_the_typescript_wording() {
        let dir = tempfile::tempdir().unwrap();

        let broken = dir.path().join("broken.json");
        std::fs::write(&broken, "{not json").unwrap();
        let err = read_blocks_file(broken.to_str().unwrap()).unwrap_err();
        assert_eq!(err.to_string(), ERR_INVALID_BLOCKS);

        let object = dir.path().join("object.json");
        std::fs::write(&object, "{\"a\":1}").unwrap();
        let err = read_blocks_file(object.to_str().unwrap()).unwrap_err();
        assert!(
            err.to_string().ends_with("blocks must be a JSON array"),
            "error was: {err}"
        );

        let missing = dir.path().join("missing.json");
        let err = read_blocks_file(missing.to_str().unwrap()).unwrap_err();
        assert!(
            err.to_string().starts_with("Error reading blocks file "),
            "error was: {err}"
        );
    }

    #[test]
    fn message_files_are_read_leniently_like_node() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("body.txt");
        std::fs::write(&file, [0x68, 0x69, 0xff]).unwrap();

        let cmd = parse(&[
            "slack-cli",
            "send",
            "-c",
            "general",
            "-f",
            file.to_str().unwrap(),
        ]);
        assert_eq!(read_message_text(&cmd).unwrap(), "hi\u{fffd}");

        let missing = parse(&["slack-cli", "send", "-c", "general", "-f", "nope.txt"]);
        let err = read_message_text(&missing).unwrap_err();
        assert!(
            err.to_string().starts_with("Error reading file nope.txt: "),
            "error was: {err}"
        );
    }

    #[tokio::test]
    async fn sends_a_plain_channel_message() {
        let server = MockServer::start().await;
        mount_ok(&server, "chat.postMessage").await;

        let cmd = parse(&["slack-cli", "send", "-c", "general", "-m", "hi"]);
        run(cmd, &client_for(&server), &table_opts()).await.unwrap();

        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1);
        let body: Value = serde_json::from_slice(&requests[0].body).unwrap();
        assert_eq!(body["channel"], "general");
        assert_eq!(body["text"], "hi");
        assert!(body.get("thread_ts").is_none());
        assert!(body.get("blocks").is_none());
    }

    #[tokio::test]
    async fn thread_and_blocks_are_only_sent_when_present() {
        let server = MockServer::start().await;
        mount_ok(&server, "chat.postMessage").await;

        let cmd = parse(&[
            "slack-cli",
            "send",
            "-c",
            "C0123456789",
            "-m",
            "hi",
            "-t",
            "1700000000.000100",
            "-b",
            "[{\"type\":\"divider\"}]",
        ]);
        run(cmd, &client_for(&server), &table_opts()).await.unwrap();

        let requests = server.received_requests().await.unwrap();
        let body: Value = serde_json::from_slice(&requests[0].body).unwrap();
        assert_eq!(body["thread_ts"], "1700000000.000100");
        assert_eq!(body["blocks"][0]["type"], "divider");
    }

    #[tokio::test]
    async fn scheduling_switches_the_api_method_and_adds_post_at() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat.scheduleMessage"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "scheduled_message_id": "Q123",
                "post_at": 1_900_000_000,
            })))
            .mount(&server)
            .await;

        let cmd = parse(&[
            "slack-cli",
            "send",
            "-c",
            "general",
            "-m",
            "hi",
            "--at",
            "1900000000",
        ]);
        run(cmd, &client_for(&server), &table_opts()).await.unwrap();

        let requests = server.received_requests().await.unwrap();
        let body: Value = serde_json::from_slice(&requests[0].body).unwrap();
        assert_eq!(body["post_at"], 1_900_000_000_i64);
    }

    #[tokio::test]
    async fn channel_not_found_falls_back_to_name_resolution() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat.postMessage"))
            .and(body_json(json!({ "channel": "general", "text": "hi" })))
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
                "channels": [{ "id": "C0123456789", "name": "general" }],
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/chat.postMessage"))
            .and(body_json(json!({ "channel": "C0123456789", "text": "hi" })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "ts": "1700000000.000100",
            })))
            .mount(&server)
            .await;

        let cmd = parse(&["slack-cli", "send", "-c", "general", "-m", "hi"]);
        run(cmd, &client_for(&server), &table_opts()).await.unwrap();

        assert_eq!(server.received_requests().await.unwrap().len(), 3);
    }

    #[tokio::test]
    async fn unresolvable_channels_report_suggestions() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat.postMessage"))
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
                "channels": [{ "id": "C0123456789", "name": "general-random" }],
            })))
            .mount(&server)
            .await;

        let cmd = parse(&["slack-cli", "send", "-c", "general", "-m", "hi"]);
        let err = run(cmd, &client_for(&server), &table_opts())
            .await
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "Channel 'general' not found. Did you mean one of these? general-random"
        );
    }

    #[tokio::test]
    async fn channel_ids_skip_the_fallback_and_surface_the_api_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat.postMessage"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({ "ok": false, "error": "channel_not_found" })),
            )
            .mount(&server)
            .await;

        let cmd = parse(&["slack-cli", "send", "-c", "C0123456789", "-m", "hi"]);
        let err = run(cmd, &client_for(&server), &table_opts())
            .await
            .unwrap_err();
        assert_eq!(err.to_string(), "API Error: channel_not_found");
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn other_api_errors_are_not_retried() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat.postMessage"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": false,
                "error": "missing_scope",
                "needed": "chat:write",
            })))
            .mount(&server)
            .await;

        let cmd = parse(&["slack-cli", "send", "-c", "general", "-m", "hi"]);
        let err = run(cmd, &client_for(&server), &table_opts())
            .await
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "API Error: missing_scope (needed: chat:write)"
        );
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn dm_by_username_resolves_the_user_then_opens_a_channel() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/users.list"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "members": [
                    { "id": "U111", "name": "bob" },
                    { "id": "U222", "name": "Alice" },
                ],
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/conversations.open"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "channel": { "id": "D0123456789" },
            })))
            .mount(&server)
            .await;
        mount_ok(&server, "chat.postMessage").await;

        let cmd = parse(&["slack-cli", "send", "--user", "@alice", "-m", "hi"]);
        run(cmd, &client_for(&server), &table_opts()).await.unwrap();

        let requests = server.received_requests().await.unwrap();
        let open: Value = serde_json::from_slice(&requests[1].body).unwrap();
        assert_eq!(open["users"], "U222");
        let post: Value = serde_json::from_slice(&requests[2].body).unwrap();
        assert_eq!(post["channel"], "D0123456789");
    }

    #[tokio::test]
    async fn unknown_usernames_are_reported_by_name() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/users.list"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({ "ok": true, "members": [] })),
            )
            .mount(&server)
            .await;

        let cmd = parse(&["slack-cli", "send", "--user", "@ghost", "-m", "hi"]);
        let err = run(cmd, &client_for(&server), &table_opts())
            .await
            .unwrap_err();
        assert_eq!(err.to_string(), "User 'ghost' not found");
    }

    #[tokio::test]
    async fn dm_by_email_uses_the_lookup_endpoint() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/users.lookupByEmail"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "user": { "id": "U333" },
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/conversations.open"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "channel": { "id": "D0999999999" },
            })))
            .mount(&server)
            .await;
        mount_ok(&server, "chat.postMessage").await;

        let cmd = parse(&["slack-cli", "send", "--email", "a@example.com", "-m", "hi"]);
        run(cmd, &client_for(&server), &table_opts()).await.unwrap();

        let requests = server.received_requests().await.unwrap();
        let post: Value = serde_json::from_slice(&requests[2].body).unwrap();
        assert_eq!(post["channel"], "D0999999999");
    }

    #[tokio::test]
    async fn json_format_prints_the_api_response_instead_of_the_success_line() {
        let server = MockServer::start().await;
        mount_ok(&server, "chat.postMessage").await;

        let global = GlobalOpts {
            json: true,
            ..GlobalOpts::default()
        };
        let cmd = parse(&["slack-cli", "send", "-c", "general", "-m", "hi"]);
        run(cmd, &client_for(&server), &global).await.unwrap();

        let mut buf = Vec::new();
        output::format_value(
            &json!({ "ok": true, "ts": "1700000000.000100" }),
            OutputFormat::Json,
            &mut buf,
        )
        .unwrap();
        let rendered: Value = serde_json::from_slice(&buf).unwrap();
        assert_eq!(rendered["ts"], "1700000000.000100");
    }

    #[tokio::test]
    async fn dry_run_skips_every_write_request() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/users.list"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "members": [{ "id": "U222", "name": "alice" }],
            })))
            .mount(&server)
            .await;

        let global = GlobalOpts {
            dry_run: true,
            ..GlobalOpts::default()
        };
        let client = client_for(&server).with_dry_run(true);
        let cmd = parse(&["slack-cli", "send", "--user", "alice", "-m", "hi"]);
        run(cmd, &client, &global).await.unwrap();

        // users.list（読み取り）だけが実際に飛ぶ
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }
}
