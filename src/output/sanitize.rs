//! 端末に出す文字列のサニタイズと、トークンの伏字。
//!
//! 移植方針 E 章は「出力ヘルパを通らない `println!` を書けない構造にする」ことを求めている。
//! 個々のコマンドが自前でサニタイザを持つと E1〜E4 のような漏れが再発するため、
//! 実装はこのモジュールに 1 本化し、各コマンドはここを経由する。
//!
//! エラー出力の処理順は TypeScript 版と同じ「サニタイズ → 伏字」を保つ。
//! 逆順にすると、エスケープシーケンスで分断されたトークンが伏字をすり抜ける。

use serde_json::Value;

const ESCAPE: char = '\u{1b}';
const BELL: char = '\u{7}';

/// 端末エスケープ注入を防ぐサニタイズ。OSC / ANSI シーケンスを取り除き、
/// TAB と LF 以外の制御文字（C0 / DEL / C1）を落とす。
pub fn sanitize_terminal_text(value: &str) -> String {
    let chars: Vec<char> = value.chars().collect();
    let mut sanitized = String::with_capacity(value.len());
    let mut index = 0;

    while index < chars.len() {
        if chars[index] == ESCAPE {
            if let Some(end) = escape_sequence_end(&chars, index) {
                index = end;
                continue;
            }
        }

        let code = chars[index] as u32;
        let is_allowed_whitespace = code == 0x09 || code == 0x0a;
        let is_control = code < 0x20 || code == 0x7f || (0x80..=0x9f).contains(&code);
        if is_allowed_whitespace || !is_control {
            sanitized.push(chars[index]);
        }
        index += 1;
    }

    sanitized
}

/// 1 行に収める値（表のセル・成功メッセージ）用。空白の連続を 1 個の空白に畳んで前後を落とす。
/// JS の `\s` に合わせて U+FEFF も空白として扱う（移植方針 J5）。
pub fn sanitize_single_line_text(value: &str) -> String {
    let mut collapsed = String::new();
    let mut pending_space = false;

    for c in sanitize_terminal_text(value).chars() {
        if c.is_whitespace() || c == '\u{feff}' {
            pending_space = !collapsed.is_empty();
            continue;
        }
        if pending_space {
            collapsed.push(' ');
            pending_space = false;
        }
        collapsed.push(c);
    }

    collapsed
}

/// JSON 値に含まれる文字列（キーと値の両方）をすべてサニタイズする。
pub fn sanitize_value(value: &Value) -> Value {
    match value {
        Value::String(s) => Value::String(sanitize_terminal_text(s)),
        Value::Array(items) => Value::Array(items.iter().map(sanitize_value).collect()),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(k, v)| (sanitize_terminal_text(k), sanitize_value(v)))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// Slack トークンらしき文字列を伏字にする。エラーメッセージを表示する直前に通す。
/// サニタイズより後に呼ぶこと（エスケープで分断されたトークンを取りこぼさないため）。
pub fn redact_slack_tokens(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let chars: Vec<char> = value.chars().collect();
    let mut index = 0;

    while index < chars.len() {
        match token_end(&chars, index) {
            Some(end) => {
                out.push_str("[REDACTED]");
                index = end;
            }
            None => {
                out.push(chars[index]);
                index += 1;
            }
        }
    }

    out
}

/// `chars[start]` が Slack トークンの先頭なら、その直後の位置を返す。
///
/// 対象は `xoxb-` / `xoxp-` / `xoxa-` / `xoxr-` / `xoxs-` / `xapp-` で始まり、
/// 英数字・ハイフンが 10 文字以上続くもの。
fn token_end(chars: &[char], start: usize) -> Option<usize> {
    const PREFIXES: [&str; 6] = ["xoxb-", "xoxp-", "xoxa-", "xoxr-", "xoxs-", "xapp-"];
    const MIN_BODY_LEN: usize = 10;

    let prefix_len = PREFIXES.iter().find_map(|prefix| {
        let len = prefix.chars().count();
        chars
            .get(start..start + len)?
            .iter()
            .collect::<String>()
            .eq_ignore_ascii_case(prefix)
            .then_some(len)
    })?;

    let mut end = start + prefix_len;
    while chars
        .get(end)
        .is_some_and(|c| c.is_ascii_alphanumeric() || *c == '-')
    {
        end += 1;
    }

    (end - start - prefix_len >= MIN_BODY_LEN).then_some(end)
}

/// `chars[start]` から始まるエスケープシーケンスの直後の位置を返す。
/// シーケンスとして閉じていなければ `None`（その場合 ESC は制御文字として落とす）。
fn escape_sequence_end(chars: &[char], start: usize) -> Option<usize> {
    match chars.get(start + 1)? {
        // OSC: ESC ] ... (BEL | ESC \)
        ']' => {
            let mut index = start + 2;
            while let Some(&c) = chars.get(index) {
                if c == BELL {
                    return Some(index + 1);
                }
                if c == ESCAPE {
                    return (chars.get(index + 1) == Some(&'\\')).then_some(index + 2);
                }
                index += 1;
            }
            None
        }
        // CSI: ESC [ [0-?]* [ -/]* [@-~]
        '[' => {
            let mut index = start + 2;
            while chars
                .get(index)
                .is_some_and(|c| ('\u{30}'..='\u{3f}').contains(c))
            {
                index += 1;
            }
            while chars
                .get(index)
                .is_some_and(|c| ('\u{20}'..='\u{2f}').contains(c))
            {
                index += 1;
            }
            chars
                .get(index)
                .filter(|c| ('\u{40}'..='\u{7e}').contains(*c))
                .map(|_| index + 1)
        }
        // ESC の次の 1 文字で閉じる短いシーケンス
        c if ('\u{40}'..='\u{5a}').contains(c) || ('\u{5c}'..='\u{5f}').contains(c) => {
            Some(start + 2)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn strips_osc_and_csi_sequences() {
        assert_eq!(
            sanitize_terminal_text("a\u{1b}]0;pwned\u{7}b"),
            "ab",
            "OSC sequences must be removed"
        );
        assert_eq!(sanitize_terminal_text("\u{1b}[31mred\u{1b}[0m"), "red");
        assert_eq!(sanitize_terminal_text("\u{1b}]0;unterminated"), "]0;unterminated");
    }

    #[test]
    fn keeps_tab_and_newline_but_drops_other_control_characters() {
        assert_eq!(sanitize_terminal_text("a\tb\nc\u{0}d\u{7f}e\u{9b}f"), "a\tb\ncdef");
    }

    #[test]
    fn multibyte_text_survives_intact() {
        assert_eq!(sanitize_terminal_text("日本語🎉"), "日本語🎉");
    }

    #[test]
    fn single_line_collapses_whitespace_including_the_bom() {
        assert_eq!(sanitize_single_line_text("  a \n\t b\u{feff}c  "), "a b c");
        assert_eq!(sanitize_single_line_text(""), "");
    }

    #[test]
    fn sanitizes_nested_json_keys_and_values() {
        let dirty = json!({ "na\u{1b}[31mme": ["ge\u{0}neral", 3] });
        assert_eq!(sanitize_value(&dirty), json!({ "name": ["general", 3] }));
    }

    /// テスト用のトークン風文字列。秘密走査に引っかからないよう接頭辞を組み立てて作る。
    fn fake_token(prefix: &str) -> String {
        format!("{prefix}-1234567890-abcdefABCDEF")
    }

    #[test]
    fn redacts_slack_tokens_but_leaves_ordinary_text() {
        for prefix in ["xoxb", "xoxp", "xapp"] {
            assert_eq!(
                redact_slack_tokens(&format!("token {} failed", fake_token(prefix))),
                "token [REDACTED] failed"
            );
        }
        // 短すぎる本体はトークンとみなさない
        assert_eq!(redact_slack_tokens("xoxb-short"), "xoxb-short");
        assert_eq!(redact_slack_tokens("nothing to hide"), "nothing to hide");
    }

    #[test]
    fn sanitizing_before_redacting_catches_split_tokens() {
        let split = format!("xoxb-\u{1b}[0m{}", "1234567890abcdef");
        assert_eq!(
            redact_slack_tokens(&sanitize_terminal_text(&split)),
            "[REDACTED]"
        );
    }
}
