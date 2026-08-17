//! エラー階層と終了コード。
//!
//! TypeScript 版の `SlackCliError` 派生（Configuration / Validation / Api / File）を
//! 単一 enum に畳み、`code()` で TS と同じコード文字列を返す。
//! Slack Web API は HTTP 200 でもボディの `ok: false` でエラーを表すため、
//! API エラーは HTTP ステータスではなく `error` フィールド（`channel_not_found` 等）を
//! 一次情報として保持する。

use std::fmt;

use serde::Deserialize;

/// エラー種別ごとのコード文字列（TypeScript 版 `errors.ts` の `code` と一致させる）。
pub const CODE_CONFIGURATION: &str = "CONFIGURATION_ERROR";
pub const CODE_VALIDATION: &str = "VALIDATION_ERROR";
pub const CODE_API: &str = "API_ERROR";
pub const CODE_FILE: &str = "FILE_ERROR";

#[derive(thiserror::Error, Debug)]
pub enum SlackCliError {
    #[error("Not authenticated. Run `slack-cli config set --token-stdin` first.")]
    NotAuthenticated,

    /// Slack Web API が `ok: false` を返した場合。`code` は `channel_not_found` などの
    /// Slack エラーコード。`needed` は `missing_scope` 時に返る不足スコープ。
    #[error("{}", format_api_error(.code, .needed))]
    Api {
        status: u16,
        code: String,
        needed: Vec<String>,
    },

    #[error("Rate limited. Retry after {retry_after}s")]
    RateLimited { retry_after: u64 },

    /// 「チャンネル / ユーザーが見つからない」のように、Slack のエラーコードではなく
    /// CLI 側で組み立てた文章をそのまま出す API エラー。TS 版の `ApiError` に対応する。
    /// `Api` と違って `API Error: ` を前置しない。
    #[error("{0}")]
    NotFound(String),

    #[error("{0}")]
    Configuration(String),

    #[error("{0}")]
    Validation(String),

    #[error("{0}")]
    File(String),

    #[error("Pagination error: {0}")]
    Pagination(String),

    #[error("Invalid ID format: {0}")]
    InvalidId(String),

    #[error(transparent)]
    Http(#[from] reqwest::Error),

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

fn format_api_error(code: &str, needed: &[String]) -> String {
    if needed.is_empty() {
        format!("API Error: {code}")
    } else {
        format!("API Error: {code} (needed: {})", needed.join(", "))
    }
}

impl SlackCliError {
    /// TypeScript 版の `error.code` に対応する文字列。
    /// 対応する概念がないバリアントは `None`。
    pub fn code(&self) -> Option<&'static str> {
        match self {
            SlackCliError::Configuration(_) | SlackCliError::NotAuthenticated => {
                Some(CODE_CONFIGURATION)
            }
            SlackCliError::Validation(_) | SlackCliError::InvalidId(_) => Some(CODE_VALIDATION),
            SlackCliError::Api { .. }
            | SlackCliError::RateLimited { .. }
            | SlackCliError::NotFound(_) => Some(CODE_API),
            SlackCliError::File(_) => Some(CODE_FILE),
            _ => None,
        }
    }

    /// プロセス終了コード。
    ///
    /// 移植方針 J3 により、TS 版と同じく成功 0 / 失敗 1 の 2 値だけを使う。
    /// 種別ごとの細分化は既存スクリプトの `if [ $? -eq 1 ]` を壊すため入れない。
    /// エラー種別を機械的に区別したい場合は `code()` を使う。
    pub fn exit_code(&self) -> i32 {
        1
    }
}

/// Slack Web API のエラーレスポンスボディ。
///
/// ```json
/// { "ok": false, "error": "missing_scope", "needed": "channels:read", "provided": "chat:write" }
/// ```
#[derive(Deserialize, Debug, Default)]
pub struct ErrorResponse {
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub needed: Option<String>,
    #[serde(default)]
    pub provided: Option<String>,
}

impl ErrorResponse {
    /// `needed` をカンマ分割・trim・空要素除去した配列にする
    /// （TypeScript 版 `getSlackNeededScopes` と同じ規則）。
    pub fn needed_scopes(&self) -> Vec<String> {
        self.needed
            .as_deref()
            .map(|n| {
                n.split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// レスポンスボディを `SlackCliError::Api` に変換する。
    pub fn into_error(self, status: u16) -> SlackCliError {
        let needed = self.needed_scopes();
        SlackCliError::Api {
            status,
            code: self.error.unwrap_or_else(|| "unknown_error".to_string()),
            needed,
        }
    }
}

impl fmt::Display for ErrorResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.error.as_deref().unwrap_or("unknown_error"))?;
        if let Some(needed) = &self.needed {
            write!(f, " (needed: {needed})")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_failure_exits_with_one() {
        assert_eq!(SlackCliError::NotAuthenticated.exit_code(), 1);
        assert_eq!(SlackCliError::RateLimited { retry_after: 5 }.exit_code(), 1);
        assert_eq!(SlackCliError::Validation("x".into()).exit_code(), 1);
        assert_eq!(SlackCliError::NotFound("x".into()).exit_code(), 1);
        assert_eq!(
            SlackCliError::Api {
                status: 200,
                code: "channel_not_found".into(),
                needed: vec![],
            }
            .exit_code(),
            1
        );
    }

    #[test]
    fn not_found_prints_its_message_without_an_api_error_prefix() {
        let err = SlackCliError::NotFound("Channel 'x' not found.".into());
        assert_eq!(err.to_string(), "Channel 'x' not found.");
        assert_eq!(err.code(), Some(CODE_API));
    }

    #[test]
    fn codes_match_typescript_error_classes() {
        assert_eq!(
            SlackCliError::Configuration("x".into()).code(),
            Some(CODE_CONFIGURATION)
        );
        assert_eq!(
            SlackCliError::Validation("x".into()).code(),
            Some(CODE_VALIDATION)
        );
        assert_eq!(SlackCliError::File("x".into()).code(), Some(CODE_FILE));
        assert_eq!(
            SlackCliError::Api {
                status: 200,
                code: "channel_not_found".into(),
                needed: vec![],
            }
            .code(),
            Some(CODE_API)
        );
    }

    #[test]
    fn missing_scope_message_lists_needed_scopes() {
        let body: ErrorResponse = serde_json::from_value(serde_json::json!({
            "ok": false,
            "error": "missing_scope",
            "needed": "channels:read, groups:read,",
        }))
        .unwrap();

        assert_eq!(body.needed_scopes(), vec!["channels:read", "groups:read"]);
        let err = body.into_error(200);
        assert_eq!(
            err.to_string(),
            "API Error: missing_scope (needed: channels:read, groups:read)"
        );
    }

    #[test]
    fn error_without_needed_scopes_prints_code_only() {
        let body = ErrorResponse {
            error: Some("channel_not_found".into()),
            needed: None,
            provided: None,
        };
        assert_eq!(
            body.into_error(200).to_string(),
            "API Error: channel_not_found"
        );
    }
}
