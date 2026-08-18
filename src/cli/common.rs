//! コマンド間で共有するヘルパ。
//!
//! チャンネル参照の解決・ID 形式判定・タイムスタンプ整形は、当初 12 個のコマンドが
//! それぞれ写経していた。挙動が少しずつずれる原因になるためここに 1 本化してある。
//! 端末サニタイズは出力の責務なので `crate::output::sanitize` にある。

use std::io::Write;

use chrono::{DateTime, TimeZone, Utc};
use colored::Colorize;
use serde_json::{json, Value};
use unicode_width::UnicodeWidthChar;

use crate::cli::GlobalOpts;
use crate::client::pagination::PaginationOpts;
use crate::client::SlackClient;
use crate::error::SlackCliError;
use crate::output::sanitize::sanitize_single_line_text;
use crate::output::{self, OutputFormat};

/// 名前解決に使うチャンネル種別。
pub const CHANNEL_LOOKUP_TYPES: [&str; 4] = ["public_channel", "private_channel", "im", "mpim"];
/// 名前解決時の 1 ページあたり取得件数（TS 版 `DEFAULTS.CHANNELS_LIMIT`）。
pub const CHANNEL_LOOKUP_PAGE_SIZE: u32 = 1000;
/// 見つからなかったときに添える候補の最大数。
const SIMILAR_CHANNEL_LIMIT: usize = 5;
/// スコープ不足時に諦めるチャンネル種別の対応表。
const SCOPE_TO_CHANNEL_TYPE: [(&str, &str); 4] = [
    ("channels:read", "public_channel"),
    ("groups:read", "private_channel"),
    ("im:read", "im"),
    ("mpim:read", "mpim"),
];

/// Slack が「そんなチャンネルは無い」と返すときのエラーコード。
pub const CHANNEL_NOT_FOUND_CODE: &str = "channel_not_found";
/// スコープ不足のエラーコード。
pub const MISSING_SCOPE_CODE: &str = "missing_scope";

/// 時刻としてパースできなかった値の表示。
pub const INVALID_TIMESTAMP: &str = "(invalid timestamp)";

/// メッセージ ts の形式エラー文言（移植方針 G9 により reaction / pin もこれに揃える）。
pub const ERR_INVALID_MESSAGE_TS: &str = "Invalid message timestamp format";

/// チャンネル ID の形式判定（`/^[CDG][A-Z0-9]{8,}$/` 相当）。
/// 移植方針 G7 により緩いほう（8 文字以上）に統一する。
pub fn is_channel_id(value: &str) -> bool {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !matches!(first, 'C' | 'D' | 'G') {
        return false;
    }

    let rest: Vec<char> = chars.collect();
    rest.len() >= 8
        && rest
            .iter()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
}

/// Slack のユーザー ID 形式か（`U` / `W` 始まり）。
pub fn is_user_id(value: &str) -> bool {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !matches!(first, 'U' | 'W') {
        return false;
    }
    let rest: Vec<char> = chars.collect();
    rest.len() >= 8
        && rest
            .iter()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
}

/// メッセージ ts の形式判定（`^\d{10}\.\d{6}$`）。
pub fn is_message_ts(value: &str) -> bool {
    let Some((seconds, micros)) = value.split_once('.') else {
        return false;
    };
    seconds.len() == 10
        && micros.len() == 6
        && seconds.chars().all(|c| c.is_ascii_digit())
        && micros.chars().all(|c| c.is_ascii_digit())
}

/// 成功メッセージ用のチャンネル表記。サニタイズしたうえで、既に `#` が付いている場合と
/// チャンネル ID の場合は `#` を足さない（移植方針 E1 / G16）。
pub fn channel_label(input: &str) -> String {
    let sanitized = sanitize_single_line_text(input);
    if sanitized.starts_with('#') || is_channel_id(&sanitized) {
        sanitized
    } else {
        format!("#{sanitized}")
    }
}

/// チャンネル名または ID をチャンネル ID に解決する。
/// ID 形式ならそのまま返し、そうでなければ会話一覧を引いて突き合わせる。
pub async fn resolve_channel_id(
    client: &SlackClient,
    input: &str,
) -> Result<String, SlackCliError> {
    if is_channel_id(input) {
        return Ok(input.to_string());
    }

    let channels = fetch_lookup_channels(client).await?;
    find_channel_id(&channels, input).ok_or_else(|| not_found_error(input, &channels))
}

/// 名前解決用の会話一覧。スコープ不足のときは読めない種別を落として 1 回だけ再試行する。
pub async fn fetch_lookup_channels(client: &SlackClient) -> Result<Vec<Value>, SlackCliError> {
    match list_lookup_channels(client, &CHANNEL_LOOKUP_TYPES).await {
        Ok(channels) => Ok(channels),
        Err(error) => match fallback_lookup_types(&error, &CHANNEL_LOOKUP_TYPES) {
            Some(types) => list_lookup_channels(client, &types).await,
            None => Err(error),
        },
    }
}

async fn list_lookup_channels(
    client: &SlackClient,
    types: &[&str],
) -> Result<Vec<Value>, SlackCliError> {
    let joined = types.join(",");
    client
        .paginate_get(
            "conversations.list",
            &[("types", joined.as_str()), ("exclude_archived", "true")],
            "channels",
            &PaginationOpts {
                page_size: Some(CHANNEL_LOOKUP_PAGE_SIZE),
                fetch_all: true,
                ..PaginationOpts::default()
            },
        )
        .await
}

/// `missing_scope` のとき、不足スコープに対応する種別を除いた再試行用の種別を返す。
/// 除外できない・全部除外されるなら再試行しない（元のエラーをそのまま返す）。
fn fallback_lookup_types<'a>(error: &SlackCliError, requested: &[&'a str]) -> Option<Vec<&'a str>> {
    let SlackCliError::Api { code, needed, .. } = error else {
        return None;
    };
    if code != MISSING_SCOPE_CODE {
        return None;
    }

    let blocked: Vec<&str> = needed
        .iter()
        .filter_map(|scope| {
            SCOPE_TO_CHANNEL_TYPE
                .iter()
                .find(|(name, _)| *name == scope.as_str())
                .map(|(_, channel_type)| *channel_type)
        })
        .collect();
    if blocked.is_empty() {
        return None;
    }

    let remaining: Vec<&'a str> = requested
        .iter()
        .filter(|channel_type| !blocked.contains(*channel_type))
        .copied()
        .collect();
    if remaining.is_empty() || remaining.len() == requested.len() {
        return None;
    }
    Some(remaining)
}

/// TS 版 `findChannel` と同じ 4 条件でマッチさせる。
pub fn find_channel_id(channels: &[Value], input: &str) -> Option<String> {
    let without_hash = input.replacen('#', "", 1);
    let lowered = input.to_lowercase();

    channels
        .iter()
        .find(|channel| {
            let name = channel.get("name").and_then(Value::as_str);
            let normalized = channel.get("name_normalized").and_then(Value::as_str);
            name == Some(input)
                || name == Some(without_hash.as_str())
                || name.is_some_and(|n| n.to_lowercase() == lowered)
                || normalized == Some(input)
        })
        .and_then(|channel| channel.get("id").and_then(Value::as_str))
        .map(str::to_string)
}

/// 「見つからない」エラー。TS 版と同じく API エラー扱い（終了コードは 1 のまま）。
pub fn not_found_error(input: &str, channels: &[Value]) -> SlackCliError {
    let lowered = input.to_lowercase();
    let suggestions: Vec<String> = channels
        .iter()
        .filter_map(|channel| channel.get("name").and_then(Value::as_str))
        .filter(|name| name.to_lowercase().contains(&lowered))
        .take(SIMILAR_CHANNEL_LIMIT)
        .map(sanitize_single_line_text)
        .collect();

    let name = sanitize_single_line_text(input);
    if suggestions.is_empty() {
        SlackCliError::NotFound(format!(
            "Channel '{name}' not found. Make sure you are a member of this channel."
        ))
    } else {
        SlackCliError::NotFound(format!(
            "Channel '{name}' not found. Did you mean one of these? {}",
            suggestions.join(", ")
        ))
    }
}

/// Slack の ts（`"1700000000.000100"`）を UTC の `YYYY-MM-DD HH:MM:SS` にする。
/// パースできない値・範囲外は `(invalid timestamp)`（移植方針 A4 / D6）。
pub fn format_message_timestamp(ts: &str) -> String {
    parse_slack_ts(ts)
        .map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
        .unwrap_or_else(|| INVALID_TIMESTAMP.to_string())
}

/// Unix 秒を UTC の `YYYY-MM-DD` にする。範囲外は `(invalid timestamp)`。
pub fn format_unix_date(seconds: i64) -> String {
    Utc.timestamp_opt(seconds, 0)
        .single()
        .map(|dt| dt.format("%Y-%m-%d").to_string())
        .unwrap_or_else(|| INVALID_TIMESTAMP.to_string())
}

/// Unix 秒を RFC3339（UTC・ミリ秒付き）にする。範囲外は `(invalid timestamp)`（移植方針 D5）。
pub fn format_unix_rfc3339(seconds: i64) -> String {
    Utc.timestamp_opt(seconds, 0)
        .single()
        .map(|dt| {
            dt.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
                .to_string()
        })
        .unwrap_or_else(|| INVALID_TIMESTAMP.to_string())
}

/// Slack ts を `DateTime<Utc>` にする。厳格パース（前方一致は許さない: 移植方針 A2 / A4）。
pub fn parse_slack_ts(ts: &str) -> Option<DateTime<Utc>> {
    let trimmed = ts.trim();
    if trimmed.is_empty() {
        return None;
    }
    let seconds: f64 = trimmed.parse().ok()?;
    if !seconds.is_finite() {
        return None;
    }
    let whole = seconds.trunc();
    if whole < i64::MIN as f64 || whole > i64::MAX as f64 {
        return None;
    }
    let nanos = ((seconds - whole) * 1_000_000_000.0).round() as u32;
    Utc.timestamp_opt(whole as i64, nanos.min(999_999_999))
        .single()
}

/// 表示幅（`unicode-width`）で切り詰める。切り詰めたら `suffix` を足す（移植方針 C1 / C2）。
/// grapheme を割らないよう char 単位で進めるため、バイト境界でパニックしない。
pub fn truncate_display(value: &str, max_width: usize, suffix: &str) -> String {
    if display_width(value) <= max_width {
        return value.to_string();
    }

    let suffix_width = display_width(suffix);
    let budget = max_width.saturating_sub(suffix_width);
    let mut used = 0;
    let mut out = String::new();
    for c in value.chars() {
        let w = c.width().unwrap_or(0);
        if used + w > budget {
            break;
        }
        used += w;
        out.push(c);
    }
    out.push_str(suffix);
    out
}

/// 端末上の表示幅。
pub fn display_width(value: &str) -> usize {
    value.chars().map(|c| c.width().unwrap_or(0)).sum()
}

/// 書き込み系コマンドの成功出力。table では緑の 1 行、それ以外はデータを出す。
///
/// `--dry-run` のときは書き込みリクエストを送っていないので、成功を名乗ってはいけない。
/// table では何も出さず（何を送ろうとしたかは stderr の `[dry-run] POST ...` が伝えている）、
/// 機械可読フォーマットでは送っていないと分かる封筒だけを出す。
pub(crate) fn write_success(
    out: &mut dyn Write,
    global: &GlobalOpts,
    message: &str,
    value: &Value,
) -> Result<(), SlackCliError> {
    let format = global.output_format();

    if global.dry_run {
        if format != OutputFormat::Table {
            output::format_value(&json!({ "ok": true, "dry_run": true }), format, out)?;
        }
        return Ok(());
    }

    if format == OutputFormat::Table {
        return write_success_line(out, global, message);
    }
    output::format_value(value, format, out)
}

/// フォーマットに関わらず 1 行だけ出す書き込み系コマンド（pin / reaction など）向け。
/// `--dry-run` では何も出さない。
pub(crate) fn write_success_line(
    out: &mut dyn Write,
    global: &GlobalOpts,
    message: &str,
) -> Result<(), SlackCliError> {
    if global.dry_run {
        return Ok(());
    }
    writeln!(out, "{}", sanitize_single_line_text(message).green())?;
    Ok(())
}

/// stdout に出す `write_success`。書き込み先を差し替える必要がないコマンド向け。
pub(crate) fn report_success(
    global: &GlobalOpts,
    message: &str,
    value: &Value,
) -> Result<(), SlackCliError> {
    write_success(&mut std::io::stdout(), global, message, value)
}

#[cfg(test)]
mod tests {
    use crate::cli::GlobalOpts;
    use crate::output::OutputFormat;

    fn global_with(format: OutputFormat, dry_run: bool) -> GlobalOpts {
        GlobalOpts {
            format,
            dry_run,
            ..GlobalOpts::default()
        }
    }

    fn captured(global: &GlobalOpts) -> String {
        let mut buf = Vec::new();
        write_success(
            &mut buf,
            global,
            "✓ done",
            &json!({ "ok": true, "id": "X1" }),
        )
        .unwrap();
        String::from_utf8(buf).unwrap()
    }

    #[test]
    fn success_message_is_printed_when_the_request_really_went_out() {
        assert!(captured(&global_with(OutputFormat::Table, false)).contains("✓ done"));
    }

    #[test]
    fn dry_run_never_claims_success_on_the_table_output() {
        assert_eq!(captured(&global_with(OutputFormat::Table, true)), "");
    }

    #[test]
    fn dry_run_marks_the_machine_readable_output_instead_of_faking_a_result() {
        let out = captured(&global_with(OutputFormat::Json, true));
        assert!(out.contains("\"dry_run\""), "output was: {out}");
        // 送っていないので、あたかも作られたかのような id は出さない。
        assert!(!out.contains("X1"), "output was: {out}");
    }

    #[test]
    fn dry_run_silences_the_single_line_writer_too() {
        let mut buf = Vec::new();
        write_success_line(&mut buf, &global_with(OutputFormat::Table, true), "✓ done").unwrap();
        assert!(buf.is_empty());
    }

    use super::*;
    use serde_json::json;

    #[test]
    fn channel_ids_need_eight_characters_after_the_prefix() {
        assert!(is_channel_id("C12345678"));
        assert!(is_channel_id("D0123456789"));
        assert!(is_channel_id("G0123456789"));
        assert!(!is_channel_id("C1234567"), "7 characters is too short");
        assert!(!is_channel_id("U0123456789"), "U is a user, not a channel");
        assert!(!is_channel_id("general"));
        assert!(!is_channel_id("C012345678a"), "lowercase is not allowed");
        assert!(!is_channel_id(""));
    }

    #[test]
    fn message_timestamps_need_ten_dot_six_digits() {
        assert!(is_message_ts("1700000000.000100"));
        assert!(!is_message_ts("1700000000.0001"));
        assert!(!is_message_ts("170000000.000100"));
        assert!(!is_message_ts("1700000000"));
        assert!(!is_message_ts("abcdefghij.000100"));
    }

    #[test]
    fn channel_labels_never_double_the_hash_or_decorate_ids() {
        assert_eq!(channel_label("general"), "#general");
        assert_eq!(channel_label("#general"), "#general");
        assert_eq!(channel_label("C0123456789"), "C0123456789");
        assert_eq!(channel_label("gen\u{1b}[31meral"), "#general");
    }

    #[test]
    fn channel_lookup_matches_name_normalized_and_case() {
        let channels = vec![
            json!({ "id": "C0123456789", "name": "General", "name_normalized": "general" }),
            json!({ "id": "C9999999999", "name": "random" }),
        ];
        assert_eq!(
            find_channel_id(&channels, "General").as_deref(),
            Some("C0123456789")
        );
        assert_eq!(
            find_channel_id(&channels, "general").as_deref(),
            Some("C0123456789")
        );
        assert_eq!(
            find_channel_id(&channels, "#random").as_deref(),
            Some("C9999999999")
        );
        assert_eq!(find_channel_id(&channels, "nope"), None);
    }

    #[test]
    fn not_found_suggests_similar_channels() {
        let channels = vec![json!({ "id": "C0123456789", "name": "general-random" })];
        let message = not_found_error("general", &channels).to_string();
        assert!(
            message.contains("Did you mean one of these? general-random"),
            "{message}"
        );

        let message = not_found_error("nope", &channels).to_string();
        assert!(message.contains("Make sure you are a member"), "{message}");
    }

    #[test]
    fn timestamps_out_of_range_do_not_panic() {
        assert_eq!(
            format_message_timestamp("1700000000.000100"),
            "2023-11-14 22:13:20"
        );
        assert_eq!(format_message_timestamp("abc"), INVALID_TIMESTAMP);
        assert_eq!(format_message_timestamp("12abc"), INVALID_TIMESTAMP);
        assert_eq!(format_message_timestamp(""), INVALID_TIMESTAMP);
        assert_eq!(format_unix_date(i64::MAX), INVALID_TIMESTAMP);
        assert_eq!(format_unix_rfc3339(1554076800), "2019-04-01T00:00:00.000Z");
    }

    #[test]
    fn truncation_uses_display_width_and_never_splits_a_character() {
        assert_eq!(truncate_display("hello", 10, "..."), "hello");
        assert_eq!(truncate_display("hello world", 8, "..."), "hello...");
        // 全角は 2 カラム。8 カラムの予算から `...`（3）を引いた 5 カラム分 = 2 文字
        assert_eq!(truncate_display("日本語テスト", 8, "..."), "日本...");
        assert_eq!(truncate_display("🎉🎉🎉🎉", 5, ""), "🎉🎉");
        assert_eq!(display_width("日本語"), 6);
    }

    #[test]
    fn missing_scope_drops_only_the_blocked_channel_types() {
        let blocked = SlackCliError::Api {
            status: 200,
            code: MISSING_SCOPE_CODE.into(),
            needed: vec!["groups:read".into(), "mpim:read".into()],
        };
        assert_eq!(
            fallback_lookup_types(&blocked, &CHANNEL_LOOKUP_TYPES),
            Some(vec!["public_channel", "im"])
        );

        // 対応する種別が無いスコープ・別のエラーコードでは再試行しない
        let unrelated = SlackCliError::Api {
            status: 200,
            code: MISSING_SCOPE_CODE.into(),
            needed: vec!["chat:write".into()],
        };
        assert_eq!(
            fallback_lookup_types(&unrelated, &CHANNEL_LOOKUP_TYPES),
            None
        );

        // 全種別が塞がれているならフォールバックしても意味がない
        let everything = SlackCliError::Api {
            status: 200,
            code: MISSING_SCOPE_CODE.into(),
            needed: vec![
                "channels:read".into(),
                "groups:read".into(),
                "im:read".into(),
                "mpim:read".into(),
            ],
        };
        assert_eq!(
            fallback_lookup_types(&everything, &CHANNEL_LOOKUP_TYPES),
            None
        );

        let other = SlackCliError::Api {
            status: 200,
            code: CHANNEL_NOT_FOUND_CODE.into(),
            needed: vec![],
        };
        assert_eq!(fallback_lookup_types(&other, &CHANNEL_LOOKUP_TYPES), None);
    }

    #[test]
    fn user_ids_accept_both_u_and_w_prefixes() {
        assert!(is_user_id("U0123456789"));
        assert!(is_user_id("W0123456789"));
        assert!(!is_user_id("C0123456789"));
        assert!(!is_user_id("alice"));
    }
}
