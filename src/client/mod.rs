//! Slack Web API クライアント。
//!
//! notion-cli の骨格（オリジン検証・リダイレクト無効化・リトライ・dry-run）を踏襲しつつ、
//! Slack 固有の 3 点を作り直してある。
//!
//! 1. HTTP 200 でもボディの `ok: false` はエラー（`request.rs`）
//! 2. ページングは `response_metadata.next_cursor`、結果配列のキーは呼び出し側指定（`pagination.rs`）
//! 3. レート制限は HTTP 429 + `Retry-After` ヘッダで判定（TS 版の文字列マッチは踏襲しない）

pub mod auth;
pub mod pagination;
pub mod request;

use std::sync::Arc;

use tokio::sync::Semaphore;
use url::Url;

use crate::error::SlackCliError;

pub const DEFAULT_BASE_URL: &str = "https://slack.com/api/";

/// リトライ既定回数（初回 + 3 回）。
pub const DEFAULT_MAX_RETRIES: u32 = 3;

/// 同時に飛ばせる API リクエスト数（移植方針 B4）。
/// TS 版は `pLimit(3)` を持ちながら `users.info` にしか通していなかった。
pub const DEFAULT_CONCURRENCY: usize = 3;

/// 未読スキャン中の同時実行数（移植方針 B5）。
/// TS 版は都度 `pLimit(15)` を作っていたため共有リミッタと合算されていた。
/// ここでは同じセマフォの許可数を差し替えることで合算されないようにする。
pub const UNREAD_SCAN_CONCURRENCY: usize = 15;

pub struct SlackClient {
    pub(crate) http: reqwest::Client,
    pub(crate) base_url: Url,
    pub(crate) token: String,
    pub(crate) max_retries: u32,
    pub(crate) dry_run: bool,
    /// 全 API 呼び出しが通る共有セマフォ。
    pub(crate) concurrency: Arc<Semaphore>,
}

impl SlackClient {
    /// トークンからクライアントを組み立てる。
    pub fn new(token: impl Into<String>) -> Result<Self, SlackCliError> {
        let token = token.into();

        // ヘッダにできない値はここで弾く。ヘッダ組み立て側はユーザー入力を扱わなくて済む。
        reqwest::header::HeaderValue::from_str(&format!("Bearer {token}")).map_err(|_| {
            SlackCliError::Configuration("Invalid token: not a valid header value".into())
        })?;

        let base_url = Url::parse(DEFAULT_BASE_URL).expect("default base URL should always parse");

        let http = reqwest::Client::builder()
            .user_agent(format!("slack-cli/{}", env!("CARGO_PKG_VERSION")))
            // 認証ヘッダ付きのリクエストが攻撃者のホストへ転送されるのを防ぐ
            .redirect(reqwest::redirect::Policy::none())
            .build()?;

        Ok(Self {
            http,
            base_url,
            token,
            max_retries: DEFAULT_MAX_RETRIES,
            dry_run: false,
            concurrency: Arc::new(Semaphore::new(DEFAULT_CONCURRENCY)),
        })
    }

    /// 同時実行数の上限を差し替える。`unread` の全チャンネルスキャンだけ 15 に上げる。
    pub fn with_concurrency(mut self, permits: usize) -> Self {
        self.concurrency = Arc::new(Semaphore::new(permits.max(1)));
        self
    }

    /// `--dry-run` を反映する。書き込み系（POST）は送信せず stderr にログするだけになる。
    pub fn with_dry_run(mut self, dry_run: bool) -> Self {
        self.dry_run = dry_run;
        self
    }

    #[cfg(test)]
    pub fn with_base_url(mut self, url: Url) -> Self {
        self.base_url = url;
        self
    }

    #[cfg(test)]
    pub fn with_max_retries(mut self, max_retries: u32) -> Self {
        self.max_retries = max_retries;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_tokens_that_cannot_become_a_header() {
        assert!(SlackClient::new("valid-token").is_ok());
        assert!(SlackClient::new("bad\ntoken").is_err());
    }
}
