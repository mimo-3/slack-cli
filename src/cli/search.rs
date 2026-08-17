//! `slack-cli search` — ワークスペース内のメッセージ検索。

use std::io::Write;

use clap::Args;
use colored::Colorize;
use serde_json::{json, Value};

use crate::cli::GlobalOpts;
use crate::client::SlackClient;
use crate::error::SlackCliError;
use crate::output::sanitize::sanitize_terminal_text;
use crate::output::{self, OutputFormat};

pub const DEFAULT_SORT: &str = "score";
pub const DEFAULT_SORT_DIR: &str = "desc";
pub const DEFAULT_COUNT: u32 = 20;
pub const DEFAULT_PAGE: u32 = 1;

const SORT_VALUES: [&str; 2] = ["score", "timestamp"];
const SORT_DIR_VALUES: [&str; 2] = ["asc", "desc"];

const COUNT_MIN: i64 = 1;
const COUNT_MAX: i64 = 100;
const PAGE_MIN: i64 = 1;
const PAGE_MAX: i64 = 100;

const API_METHOD: &str = "search.messages";

pub const MSG_NO_MESSAGES: &str = "No messages found";
pub const INVALID_TIMESTAMP: &str = "(invalid timestamp)";
pub const NO_TEXT: &str = "(no text)";
const UNKNOWN_CHANNEL: &str = "unknown";
const UNKNOWN_USER: &str = "Unknown";

#[derive(Args, Debug)]
pub struct SearchCommand {
    /// Search query
    #[arg(short, long, required = true, value_name = "QUERY")]
    pub query: String,

    /// Sort by: score or timestamp
    #[arg(long, default_value = DEFAULT_SORT, value_name = "SORT")]
    pub sort: String,

    /// Sort direction: asc or desc
    #[arg(long, default_value = DEFAULT_SORT_DIR, value_name = "DIRECTION")]
    pub sort_dir: String,

    /// Number of results per page (1-100)
    #[arg(short, long, value_name = "COUNT")]
    pub number: Option<String>,

    /// Page number (1-100)
    #[arg(long, value_name = "PAGE")]
    pub page: Option<String>,
}

pub async fn run(
    cmd: SearchCommand,
    client: &SlackClient,
    global: &GlobalOpts,
) -> Result<(), SlackCliError> {
    let sort = validate_one_of(&cmd.sort, &SORT_VALUES, "sort")?;
    let sort_dir = validate_one_of(&cmd.sort_dir, &SORT_DIR_VALUES, "sort direction")?;
    let count = parse_bounded(
        cmd.number.as_deref(),
        DEFAULT_COUNT,
        COUNT_MIN,
        COUNT_MAX,
        "Count",
    )?;
    let page = parse_bounded(
        cmd.page.as_deref(),
        DEFAULT_PAGE,
        PAGE_MIN,
        PAGE_MAX,
        "Page",
    )?;

    let count_param = count.to_string();
    let page_param = page.to_string();

    // ページ送りはしない。利用者が --page を回す（TS 版と同じ）。
    let response = client
        .get(
            API_METHOD,
            &[
                ("query", cmd.query.as_str()),
                ("sort", sort),
                ("sort_dir", sort_dir),
                ("count", count_param.as_str()),
                ("page", page_param.as_str()),
            ],
        )
        .await?;

    let result = transform(&response, &cmd.query);
    emit(
        &result,
        global.output_format(),
        &mut std::io::stdout(),
        &mut std::io::stderr(),
    )
}

/// `--sort` / `--sort-dir` の許容値チェック。
fn validate_one_of<'a>(
    raw: &'a str,
    allowed: &[&str],
    label: &str,
) -> Result<&'a str, SlackCliError> {
    if allowed.contains(&raw) {
        return Ok(raw);
    }
    Err(SlackCliError::Validation(format!(
        "Invalid {label} '{raw}'. Must be one of: {}",
        allowed.join(", ")
    )))
}

/// `--number` / `--page` の厳格パースと範囲検証（移植方針 A2 / A5）。
/// TS 版は `parseInt` の前方一致で `5abc` を 5 として受理していたが、ここでは弾く。
fn parse_bounded(
    raw: Option<&str>,
    default: u32,
    min: i64,
    max: i64,
    label: &str,
) -> Result<u32, SlackCliError> {
    let Some(raw) = raw else {
        return Ok(default);
    };

    let parsed: i64 = raw
        .trim()
        .parse()
        .map_err(|_| SlackCliError::Validation(format!("{label} must be a number")))?;

    if parsed < min {
        return Err(SlackCliError::Validation(format!(
            "{label} must be at least {min}"
        )));
    }
    if parsed > max {
        return Err(SlackCliError::Validation(format!(
            "{label} must be at most {max}"
        )));
    }
    Ok(parsed as u32)
}

/// `search.messages` のレスポンスを出力用の構造に落とす。
/// キー名と件数フォールバックは TS 版の JSON 出力と同じ契約。
fn transform(response: &Value, fallback_query: &str) -> Value {
    let messages = response.get("messages");
    let pagination = messages.and_then(|m| m.get("pagination"));

    let matches: Vec<Value> = messages
        .and_then(|m| m.get("matches"))
        .and_then(Value::as_array)
        .map(|arr| arr.iter().map(transform_match).collect())
        .unwrap_or_default();

    json!({
        "query": sanitize_terminal_text(
            response.get("query").and_then(Value::as_str).unwrap_or(fallback_query),
        ),
        "totalCount": number_or(pagination, "total_count", 0),
        "page": number_or(pagination, "page", 1),
        "pageCount": number_or(pagination, "page_count", 0),
        "matches": matches,
    })
}

fn number_or(pagination: Option<&Value>, key: &str, fallback: u64) -> u64 {
    pagination
        .and_then(|p| p.get(key))
        .and_then(Value::as_u64)
        // TS 版は `?? 0` ではなく `|| 0` なので 0 もフォールバックに落ちるが、結果は同じ値になる
        .unwrap_or(fallback)
}

fn transform_match(item: &Value) -> Value {
    let channel = item.get("channel");
    let channel_label = first_non_empty(&[
        channel.and_then(|c| c.get("name")).and_then(Value::as_str),
        channel.and_then(|c| c.get("id")).and_then(Value::as_str),
    ])
    .unwrap_or(UNKNOWN_CHANNEL);

    let username = first_non_empty(&[
        item.get("username").and_then(Value::as_str),
        item.get("user").and_then(Value::as_str),
    ])
    .unwrap_or(UNKNOWN_USER);

    let timestamp = match item.get("ts").and_then(Value::as_str) {
        Some(ts) if !ts.is_empty() => format_timestamp(ts),
        _ => String::new(),
    };

    let text = first_non_empty(&[item.get("text").and_then(Value::as_str)]).unwrap_or(NO_TEXT);
    let permalink = first_non_empty(&[item.get("permalink").and_then(Value::as_str)]).unwrap_or("");

    json!({
        "channel": sanitize_terminal_text(channel_label),
        "username": sanitize_terminal_text(username),
        "timestamp": timestamp,
        "text": sanitize_terminal_text(text),
        "permalink": sanitize_terminal_text(permalink),
    })
}

/// TS 版の `a || b || fallback` と同じく、空文字も「値なし」として次の候補へ送る。
fn first_non_empty<'a>(candidates: &[Option<&'a str>]) -> Option<&'a str> {
    candidates
        .iter()
        .flatten()
        .copied()
        .find(|value| !value.is_empty())
}

/// Slack の ts（`1755400000.123456`）を UTC の `YYYY-MM-DD HH:MM:SS` にする。
/// パースできない値は `NaN-NaN-NaN ...` を出さず `(invalid timestamp)` にする（移植方針 A4 / D6）。
fn format_timestamp(ts: &str) -> String {
    let Ok(seconds) = ts.trim().parse::<f64>() else {
        return INVALID_TIMESTAMP.to_string();
    };
    if !seconds.is_finite() {
        return INVALID_TIMESTAMP.to_string();
    }

    let millis = (seconds * 1000.0).trunc();
    if millis < i64::MIN as f64 || millis > i64::MAX as f64 {
        return INVALID_TIMESTAMP.to_string();
    }

    match chrono::DateTime::from_timestamp_millis(millis as i64) {
        Some(dt) => dt.format("%Y-%m-%d %H:%M:%S").to_string(),
        None => INVALID_TIMESTAMP.to_string(),
    }
}

/// 出力。データは `out`、見出しや件数などの人間向けの補足は `notes`（stderr）へ分ける。
/// 構造化フォーマットは全体を、行指向のフォーマットは `matches` だけを出す。
fn emit(
    result: &Value,
    format: OutputFormat,
    out: &mut dyn Write,
    notes: &mut dyn Write,
) -> Result<(), SlackCliError> {
    if matches!(format, OutputFormat::Json | OutputFormat::Yaml) {
        return output::format_value(result, format, out);
    }

    write_notes(result, notes)?;
    let matches = result
        .get("matches")
        .cloned()
        .unwrap_or(Value::Array(vec![]));
    output::format_value(&matches, format, out)
}

fn write_notes(result: &Value, notes: &mut dyn Write) -> Result<(), SlackCliError> {
    let query = result.get("query").and_then(Value::as_str).unwrap_or("");
    let total = result
        .get("totalCount")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let page = result.get("page").and_then(Value::as_u64).unwrap_or(1);
    let page_count = result.get("pageCount").and_then(Value::as_u64).unwrap_or(0);
    let shown = result
        .get("matches")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0);

    writeln!(
        notes,
        "{}",
        format!("Search results for \"{query}\" ({total} matches)").bold()
    )?;

    if shown == 0 {
        writeln!(notes, "{}", MSG_NO_MESSAGES.yellow())?;
        return Ok(());
    }

    if page_count > 1 {
        writeln!(notes, "{}", format!("Page {page}/{page_count}").dimmed())?;
    }
    writeln!(
        notes,
        "{}",
        format!("Displayed {shown} of {total} match(es)").green()
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::error::ErrorKind;
    use clap::Parser;
    use url::Url;
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use crate::cli::Cli;

    fn parse(argv: &[&str]) -> super::SearchCommand {
        let cli = Cli::try_parse_from(argv).unwrap();
        let crate::cli::Command::Search(cmd) = cli.command else {
            panic!("expected the search command");
        };
        cmd
    }

    fn client_for(server: &MockServer) -> SlackClient {
        SlackClient::new("test-token-value")
            .unwrap()
            .with_base_url(Url::parse(&format!("{}/", server.uri())).unwrap())
    }

    fn sample_response() -> Value {
        json!({
            "ok": true,
            "query": "deploy",
            "messages": {
                "matches": [
                    {
                        "text": "デプロイ完了しました",
                        "user": "U012ABC",
                        "username": "alice",
                        "ts": "1755403953.123456",
                        "channel": { "id": "C123", "name": "dev-acejob" },
                        "permalink": "https://example.slack.com/archives/C123/p1755403953",
                    },
                    {
                        "user": "U345DEF",
                        "ts": "",
                        "channel": { "id": "C999" },
                    },
                ],
                "pagination": { "total_count": 37, "page": 2, "page_count": 4 },
            },
        })
    }

    async fn mount(server: &MockServer, body: Value) {
        Mock::given(method("GET"))
            .and(path("/search.messages"))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .mount(server)
            .await;
    }

    fn render(result: &Value, format: OutputFormat) -> (String, String) {
        let mut out = Vec::new();
        let mut notes = Vec::new();
        emit(result, format, &mut out, &mut notes).unwrap();
        (
            String::from_utf8(out).unwrap(),
            String::from_utf8(notes).unwrap(),
        )
    }

    #[test]
    fn sort_defaults_match_the_typescript_version() {
        let cmd = parse(&["slack-cli", "search", "-q", "release"]);
        assert_eq!(cmd.sort, "score");
        assert_eq!(cmd.sort_dir, "desc");
        assert!(cmd.number.is_none());
        assert!(cmd.page.is_none());
    }

    #[test]
    fn parses_every_flag() {
        let cmd = parse(&[
            "slack-cli",
            "search",
            "-q",
            "release",
            "--sort",
            "timestamp",
            "--sort-dir",
            "asc",
            "-n",
            "50",
            "--page",
            "2",
        ]);
        assert_eq!(cmd.sort, "timestamp");
        assert_eq!(cmd.sort_dir, "asc");
        assert_eq!(cmd.number.as_deref(), Some("50"));
        assert_eq!(cmd.page.as_deref(), Some("2"));
    }

    #[test]
    fn query_is_required() {
        let err = Cli::try_parse_from(["slack-cli", "search"]).expect_err("--query is required");
        assert_eq!(err.kind(), ErrorKind::MissingRequiredArgument);
    }

    #[test]
    fn unknown_sort_values_are_left_to_run() {
        // 移植方針 G12: 値の妥当性は手書きバリデーションで見る（clap には委ねない）
        let cmd = parse(&["slack-cli", "search", "-q", "x", "--sort", "bogus"]);
        assert_eq!(cmd.sort, "bogus");
    }

    #[test]
    fn sort_and_sort_dir_report_the_typescript_messages() {
        assert_eq!(
            validate_one_of("bogus", &SORT_VALUES, "sort")
                .unwrap_err()
                .to_string(),
            "Invalid sort 'bogus'. Must be one of: score, timestamp"
        );
        assert_eq!(
            validate_one_of("sideways", &SORT_DIR_VALUES, "sort direction")
                .unwrap_err()
                .to_string(),
            "Invalid sort direction 'sideways'. Must be one of: asc, desc"
        );
        assert_eq!(
            validate_one_of("asc", &SORT_DIR_VALUES, "x").unwrap(),
            "asc"
        );
    }

    #[test]
    fn number_and_page_are_parsed_strictly() {
        assert_eq!(
            parse_bounded(None, DEFAULT_COUNT, COUNT_MIN, COUNT_MAX, "Count").unwrap(),
            20
        );
        assert_eq!(
            parse_bounded(Some(" 50 "), DEFAULT_COUNT, COUNT_MIN, COUNT_MAX, "Count").unwrap(),
            50
        );

        // TS 版は parseInt の前方一致で 5 として通していた入力
        for raw in ["5abc", "3.7", "abc", ""] {
            assert_eq!(
                parse_bounded(Some(raw), DEFAULT_COUNT, COUNT_MIN, COUNT_MAX, "Count")
                    .unwrap_err()
                    .to_string(),
                "Count must be a number",
                "{raw:?} should have been rejected"
            );
        }

        assert_eq!(
            parse_bounded(Some("0"), DEFAULT_COUNT, COUNT_MIN, COUNT_MAX, "Count")
                .unwrap_err()
                .to_string(),
            "Count must be at least 1"
        );
        assert_eq!(
            parse_bounded(Some("101"), DEFAULT_COUNT, COUNT_MIN, COUNT_MAX, "Count")
                .unwrap_err()
                .to_string(),
            "Count must be at most 100"
        );
        assert_eq!(
            parse_bounded(Some("101"), DEFAULT_PAGE, PAGE_MIN, PAGE_MAX, "Page")
                .unwrap_err()
                .to_string(),
            "Page must be at most 100"
        );
    }

    #[tokio::test]
    async fn validation_runs_before_the_api_call() {
        let server = MockServer::start().await;
        mount(&server, sample_response()).await;
        let client = client_for(&server);
        let global = GlobalOpts {
            json: true,
            ..GlobalOpts::default()
        };

        let cmd = parse(&["slack-cli", "search", "-q", "x", "--sort", "bogus"]);
        let err = run(cmd, &client, &global).await.unwrap_err();
        assert_eq!(
            err.to_string(),
            "Invalid sort 'bogus'. Must be one of: score, timestamp"
        );
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn sends_every_search_parameter() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/search.messages"))
            .and(query_param("query", "deploy prod"))
            .and(query_param("sort", "timestamp"))
            .and(query_param("sort_dir", "asc"))
            .and(query_param("count", "50"))
            .and(query_param("page", "2"))
            .respond_with(ResponseTemplate::new(200).set_body_json(sample_response()))
            .mount(&server)
            .await;

        let cmd = parse(&[
            "slack-cli",
            "search",
            "-q",
            "deploy prod",
            "--sort",
            "timestamp",
            "--sort-dir",
            "asc",
            "-n",
            "50",
            "--page",
            "2",
        ]);
        let global = GlobalOpts {
            json: true,
            ..GlobalOpts::default()
        };
        run(cmd, &client_for(&server), &global).await.unwrap();
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn defaults_are_sent_when_the_flags_are_omitted() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/search.messages"))
            .and(query_param("sort", "score"))
            .and(query_param("sort_dir", "desc"))
            .and(query_param("count", "20"))
            .and(query_param("page", "1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(sample_response()))
            .mount(&server)
            .await;

        let global = GlobalOpts {
            json: true,
            ..GlobalOpts::default()
        };
        run(
            parse(&["slack-cli", "search", "-q", "x"]),
            &client_for(&server),
            &global,
        )
        .await
        .unwrap();
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn api_errors_carry_the_slack_code_and_needed_scopes() {
        let server = MockServer::start().await;
        mount(
            &server,
            json!({ "ok": false, "error": "missing_scope", "needed": "search:read" }),
        )
        .await;

        let global = GlobalOpts::default();
        let err = run(
            parse(&["slack-cli", "search", "-q", "x"]),
            &client_for(&server),
            &global,
        )
        .await
        .unwrap_err();
        assert_eq!(
            err.to_string(),
            "API Error: missing_scope (needed: search:read)"
        );
    }

    #[test]
    fn transform_matches_the_documented_json_shape() {
        let result = transform(&sample_response(), "fallback");

        assert_eq!(result["query"], "deploy");
        assert_eq!(result["totalCount"], 37);
        assert_eq!(result["page"], 2);
        assert_eq!(result["pageCount"], 4);

        let first = &result["matches"][0];
        assert_eq!(first["channel"], "dev-acejob");
        assert_eq!(first["username"], "alice");
        assert_eq!(first["timestamp"], "2025-08-17 04:12:33");
        assert_eq!(first["text"], "デプロイ完了しました");
        assert_eq!(
            first["permalink"],
            "https://example.slack.com/archives/C123/p1755403953"
        );

        // 名前・本文・permalink が無いときのフォールバック
        let second = &result["matches"][1];
        assert_eq!(second["channel"], "C999");
        assert_eq!(second["username"], "U345DEF");
        assert_eq!(second["timestamp"], "");
        assert_eq!(second["text"], NO_TEXT);
        assert_eq!(second["permalink"], "");
    }

    #[test]
    fn transform_falls_back_when_the_response_is_empty() {
        let result = transform(&json!({ "ok": true }), "release notes");
        assert_eq!(result["query"], "release notes");
        assert_eq!(result["totalCount"], 0);
        assert_eq!(result["page"], 1);
        assert_eq!(result["pageCount"], 0);
        assert_eq!(result["matches"], json!([]));

        let unknown = transform(
            &json!({ "messages": { "matches": [{ "channel": {} }] } }),
            "q",
        );
        assert_eq!(unknown["matches"][0]["channel"], UNKNOWN_CHANNEL);
        assert_eq!(unknown["matches"][0]["username"], UNKNOWN_USER);
    }

    #[test]
    fn broken_timestamps_do_not_produce_nan() {
        assert_eq!(format_timestamp("1755403953.123456"), "2025-08-17 04:12:33");
        assert_eq!(format_timestamp("abc"), INVALID_TIMESTAMP);
        assert_eq!(format_timestamp("12abc"), INVALID_TIMESTAMP);
        assert_eq!(format_timestamp("1e400"), INVALID_TIMESTAMP);
        assert_eq!(format_timestamp("0"), "1970-01-01 00:00:00");
    }

    #[test]
    fn output_values_are_stripped_of_terminal_escapes() {
        let result = transform(
            &json!({
                "messages": { "matches": [{
                    "text": "line1\u{1b}[31mred\u{7}\nline2",
                    "username": "\u{1b}]0;title\u{7}alice",
                    "channel": { "name": "general" },
                }] },
            }),
            "q",
        );
        assert_eq!(result["matches"][0]["text"], "line1red\nline2");
        assert_eq!(result["matches"][0]["username"], "alice");
    }

    #[test]
    fn json_output_keeps_the_full_envelope() {
        let result = transform(&sample_response(), "q");
        let (out, notes) = render(&result, OutputFormat::Json);

        let parsed: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed["totalCount"], 37);
        assert_eq!(parsed["matches"].as_array().unwrap().len(), 2);
        assert!(notes.is_empty(), "notes were: {notes}");
    }

    #[test]
    fn row_formats_print_matches_and_send_the_summary_to_stderr() {
        let result = transform(&sample_response(), "q");
        let (out, notes) = render(&result, OutputFormat::Table);

        assert!(out.contains("dev-acejob"), "table output was: {out}");
        assert!(out.contains("alice"), "table output was: {out}");
        assert!(!out.contains("totalCount"), "table output was: {out}");
        assert!(notes.contains("Search results for \"deploy\" (37 matches)"));
        assert!(notes.contains("Page 2/4"));
        assert!(notes.contains("Displayed 2 of 37 match(es)"));

        let (tsv, _) = render(&result, OutputFormat::Tsv);
        assert!(tsv.contains('\t'), "tsv output was: {tsv}");
    }

    #[test]
    fn zero_matches_still_produce_structured_output() {
        let result = transform(
            &json!({ "query": "nope", "messages": { "matches": [] } }),
            "nope",
        );

        // 移植方針 G14: json は 0 件でも JSON を出す
        let (out, _) = render(&result, OutputFormat::Json);
        let parsed: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed["matches"], json!([]));

        let (_, notes) = render(&result, OutputFormat::Table);
        assert!(notes.contains(MSG_NO_MESSAGES), "notes were: {notes}");
        assert!(!notes.contains("Displayed"), "notes were: {notes}");
    }
}
