//! HTTP リクエストの実行、エラー判定、リトライ。
//!
//! Slack Web API の要注意点が 2 つある。
//!
//! - **HTTP 200 でもエラー**: ボディに `ok: false` と `error: "channel_not_found"` が入る。
//!   ステータスだけ見ていると取りこぼすので、成功パスで必ず `ok` を検査する。
//! - **レート制限は 429 + `Retry-After`**: TS 版はエラーメッセージの文字列一致で
//!   判定して固定 5 秒待っていたが、ここではヘッダを読んで指数バックオフ＋ジッタで待つ。

use std::time::Duration;

use rand::Rng;
use serde_json::Value;

use super::SlackClient;
use crate::error::{ErrorResponse, SlackCliError};

/// バックオフの上限（秒）。
const MAX_BACKOFF_SECS: u64 = 60;
/// Slack が 200 + `ok:false` でレート制限を伝えてくるときのエラーコード。
const RATELIMITED_CODE: &str = "ratelimited";

impl SlackClient {
    /// API メソッド名から URL を組み立て、オリジンを逸脱していないか検証する。
    /// `slack api <method>` のような生 API 呼び出しコマンドを足すときの防御線になる。
    pub(crate) fn api_url(&self, method: &str) -> Result<url::Url, SlackCliError> {
        if url::Url::parse(method).is_ok() {
            return Err(SlackCliError::Configuration(
                "API method must be relative to the configured Slack API origin".to_string(),
            ));
        }

        // 先頭のスラッシュ・バックスラッシュは弾く。落として繋ぐと
        // `//attacker.example/x`（プロトコル相対）が「オリジン内の変なパス」に化けて
        // 素通りしてしまい、オリジン検証もすり抜ける。
        if method.starts_with('/') || method.contains('\\') {
            return Err(SlackCliError::Configuration(format!(
                "API method must be a bare Slack method name, got: {method}"
            )));
        }

        let url = self.base_url.join(method).map_err(|e| {
            SlackCliError::Configuration(format!("Invalid API method {method}: {e}"))
        })?;

        if url.origin() != self.base_url.origin() {
            return Err(SlackCliError::Configuration(
                "API method must not change the configured Slack API origin".to_string(),
            ));
        }

        Ok(url)
    }

    /// GET リクエスト（クエリパラメータ付き）。読み取り系はこちら。
    pub async fn get(&self, method: &str, params: &[(&str, &str)]) -> Result<Value, SlackCliError> {
        let url = self.api_url(method)?;
        self.request_with_retry(|| {
            self.http
                .get(url.clone())
                .headers(self.auth_headers())
                .query(params)
        })
        .await
    }

    /// JSON ボディの POST。書き込み系はこちら。`--dry-run` では送信しない。
    pub async fn post_json(&self, method: &str, body: &Value) -> Result<Value, SlackCliError> {
        let url = self.api_url(method)?;
        if self.dry_run {
            return dry_run_log("POST", &url, Some(&body.to_string()));
        }
        self.post_json_always(method, body).await
    }

    /// JSON ボディを要求する**読み取り**エンドポイント向けの POST。
    /// `canvases.sections.lookup` のように副作用が無いものは `--dry-run` でも送信する
    /// （送信を飛ばすと結果が空になり、dry-run で読み取りコマンドが壊れるため）。
    pub async fn post_json_always(
        &self,
        method: &str,
        body: &Value,
    ) -> Result<Value, SlackCliError> {
        let url = self.api_url(method)?;
        self.request_with_retry(|| {
            self.http
                .post(url.clone())
                .headers(self.json_headers())
                .json(body)
        })
        .await
    }

    /// form-encoded の POST。`files.upload` 系など JSON を受けないエンドポイント向け。
    pub async fn post_form(
        &self,
        method: &str,
        params: &[(&str, &str)],
    ) -> Result<Value, SlackCliError> {
        let url = self.api_url(method)?;
        if self.dry_run {
            return dry_run_log("POST", &url, Some(&format!("{params:?}")));
        }
        self.request_with_retry(|| {
            self.http
                .post(url.clone())
                .headers(self.form_headers())
                .form(params)
        })
        .await
    }

    /// リトライ付きでリクエストを実行する。
    ///
    /// `RequestBuilder` は `send()` で消費されるため、毎回作り直せるようクロージャを受ける。
    pub(crate) async fn request_with_retry<F>(
        &self,
        build_request: F,
    ) -> Result<Value, SlackCliError>
    where
        F: Fn() -> reqwest::RequestBuilder,
    {
        // 全 API 呼び出しをここで直列化する（移植方針 B4 / B5）。
        // `close()` は呼ばないので acquire が失敗することはない。
        let _permit = self.concurrency.acquire().await.map_err(|e| {
            SlackCliError::Configuration(format!("Concurrency limiter closed: {e}"))
        })?;

        let mut last_wait = 1u64;

        for attempt in 0..=self.max_retries {
            let response = build_request().send().await?;
            let status = response.status();
            let retry_after_header = parse_retry_after(response.headers());

            if status.as_u16() == 429 {
                if attempt == self.max_retries {
                    return Err(SlackCliError::RateLimited {
                        retry_after: last_wait,
                    });
                }
                last_wait = backoff_seconds(retry_after_header.unwrap_or(1), attempt);
                tokio::time::sleep(Duration::from_secs(last_wait)).await;
                continue;
            }

            if status.is_success() {
                let body: Value = response.json().await?;

                // ここが Slack 固有の肝。200 でも ok:false ならエラー。
                if body.get("ok").and_then(Value::as_bool) == Some(false) {
                    let error_body: ErrorResponse =
                        serde_json::from_value(body.clone()).unwrap_or_default();

                    if error_body.error.as_deref() == Some(RATELIMITED_CODE) {
                        if attempt == self.max_retries {
                            return Err(SlackCliError::RateLimited {
                                retry_after: last_wait,
                            });
                        }
                        last_wait = backoff_seconds(retry_after_header.unwrap_or(1), attempt);
                        tokio::time::sleep(Duration::from_secs(last_wait)).await;
                        continue;
                    }

                    return Err(error_body.into_error(status.as_u16()));
                }

                return Ok(body);
            }

            // 非 2xx。ボディが Slack のエラー形式ならそこから、でなければステータスから組み立てる。
            let status_code = status.as_u16();
            let text = response.text().await.unwrap_or_default();
            if let Ok(error_body) = serde_json::from_str::<ErrorResponse>(&text) {
                if error_body.error.is_some() {
                    return Err(error_body.into_error(status_code));
                }
            }
            return Err(SlackCliError::Api {
                status: status_code,
                code: format!("http_{status_code}"),
                needed: Vec::new(),
            });
        }

        Err(SlackCliError::RateLimited {
            retry_after: last_wait,
        })
    }
}

/// 指数バックオフ + ±20% のジッタ。上限 60 秒、下限 1 秒。
fn backoff_seconds(retry_after: u64, attempt: u32) -> u64 {
    let backoff = retry_after
        .saturating_mul(1u64 << attempt.min(16))
        .min(MAX_BACKOFF_SECS);
    let jitter_range = (backoff as f64 * 0.2) as u64;
    let jitter = if jitter_range > 0 {
        rand::thread_rng().gen_range(0..=jitter_range * 2) as i64 - jitter_range as i64
    } else {
        0
    };
    (backoff as i64 + jitter).max(1) as u64
}

fn parse_retry_after(headers: &reqwest::header::HeaderMap) -> Option<u64> {
    headers
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<u64>().ok())
}

/// dry-run では送信せず stderr にログして空のレスポンスを返す。
fn dry_run_log(
    http_method: &str,
    url: &url::Url,
    body: Option<&str>,
) -> Result<Value, SlackCliError> {
    eprintln!("[dry-run] {http_method} {url}");
    if let Some(body) = body {
        eprintln!("[dry-run] body: {body}");
    }
    Ok(serde_json::json!({ "ok": true, "dry_run": true }))
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use url::Url;
    use wiremock::matchers::{any, header, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;

    const TEST_TOKEN: &str = "test-token-value";

    fn client_for(server: &MockServer) -> SlackClient {
        SlackClient::new(TEST_TOKEN)
            .unwrap()
            .with_base_url(Url::parse(&format!("{}/", server.uri())).unwrap())
    }

    #[tokio::test]
    async fn ok_false_on_http_200_becomes_an_api_error() {
        let server = MockServer::start().await;
        Mock::given(any())
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({ "ok": false, "error": "channel_not_found" })),
            )
            .mount(&server)
            .await;

        let err = client_for(&server)
            .get("conversations.info", &[("channel", "C1")])
            .await
            .unwrap_err();

        match err {
            SlackCliError::Api { status, code, .. } => {
                assert_eq!(status, 200);
                assert_eq!(code, "channel_not_found");
            }
            other => panic!("unexpected error: {other}"),
        }
    }

    #[tokio::test]
    async fn missing_scope_error_carries_the_needed_scopes() {
        let server = MockServer::start().await;
        Mock::given(any())
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": false,
                "error": "missing_scope",
                "needed": "channels:read,groups:read",
                "provided": "chat:write",
            })))
            .mount(&server)
            .await;

        let err = client_for(&server)
            .get("conversations.list", &[])
            .await
            .unwrap_err();

        match err {
            SlackCliError::Api { code, needed, .. } => {
                assert_eq!(code, "missing_scope");
                assert_eq!(needed, vec!["channels:read", "groups:read"]);
            }
            other => panic!("unexpected error: {other}"),
        }
    }

    #[tokio::test]
    async fn ok_true_passes_through() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/conversations.info"))
            .and(query_param("channel", "C1"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({ "ok": true, "channel": { "id": "C1" } })),
            )
            .expect(1)
            .mount(&server)
            .await;

        let body = client_for(&server)
            .get("conversations.info", &[("channel", "C1")])
            .await
            .unwrap();
        assert_eq!(body["channel"]["id"], "C1");
    }

    #[tokio::test]
    async fn retries_on_429_using_the_retry_after_header() {
        let server = MockServer::start().await;
        Mock::given(any())
            .respond_with(
                ResponseTemplate::new(429)
                    .insert_header("retry-after", "1")
                    .set_body_json(json!({ "ok": false, "error": "ratelimited" })),
            )
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(any())
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "ok": true })))
            .mount(&server)
            .await;

        let body = client_for(&server)
            .with_max_retries(2)
            .get("auth.test", &[])
            .await
            .unwrap();
        assert_eq!(body["ok"], true);
        assert_eq!(server.received_requests().await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn gives_up_after_max_retries_with_a_rate_limited_error() {
        let server = MockServer::start().await;
        Mock::given(any())
            .respond_with(ResponseTemplate::new(429).insert_header("retry-after", "1"))
            .mount(&server)
            .await;

        let err = client_for(&server)
            .with_max_retries(1)
            .get("auth.test", &[])
            .await
            .unwrap_err();

        assert!(matches!(err, SlackCliError::RateLimited { .. }));
        assert_eq!(err.exit_code(), 1);
        // 初回 + リトライ 1 回
        assert_eq!(server.received_requests().await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn non_2xx_without_a_slack_body_reports_the_status() {
        let server = MockServer::start().await;
        Mock::given(any())
            .respond_with(ResponseTemplate::new(500).set_body_string("upstream exploded"))
            .mount(&server)
            .await;

        let err = client_for(&server).get("auth.test", &[]).await.unwrap_err();
        match err {
            SlackCliError::Api { status, code, .. } => {
                assert_eq!(status, 500);
                assert_eq!(code, "http_500");
            }
            other => panic!("unexpected error: {other}"),
        }
    }

    #[tokio::test]
    async fn json_post_sets_the_charset_content_type() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat.postMessage"))
            .and(header(
                "content-type",
                super::super::auth::CONTENT_TYPE_JSON,
            ))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({ "ok": true, "ts": "1700000000.000100" })),
            )
            .expect(1)
            .mount(&server)
            .await;

        let body = client_for(&server)
            .post_json(
                "chat.postMessage",
                &json!({ "channel": "C1", "text": "hello" }),
            )
            .await
            .unwrap();
        assert_eq!(body["ts"], "1700000000.000100");
    }

    #[tokio::test]
    async fn dry_run_never_sends_write_requests_but_still_sends_reads() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "ok": true })))
            .mount(&server)
            .await;

        let client = client_for(&server).with_dry_run(true);
        let posted = client
            .post_json("chat.postMessage", &json!({ "channel": "C1" }))
            .await
            .unwrap();
        assert_eq!(posted["dry_run"], true);

        client.get("auth.test", &[]).await.unwrap();

        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1, "dry-run must not send the POST");
        assert_eq!(requests[0].method.as_str(), "GET");
    }

    #[tokio::test]
    async fn requests_never_leave_the_configured_origin() {
        let slack = MockServer::start().await;
        let attacker = MockServer::start().await;
        Mock::given(any())
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "ok": true })))
            .mount(&attacker)
            .await;

        let client = client_for(&slack);
        for hostile in [
            // 絶対 URL
            format!("{}/steal", attacker.uri()),
            // プロトコル相対 URL
            "//attacker.example/steal".to_string(),
            // バックスラッシュ権限部（WHATWG URL では / と同じ扱いになる）
            "\\\\attacker.example/steal".to_string(),
            // ベース URL のパス接頭辞（/api/）から出る形
            "/steal".to_string(),
        ] {
            let err = client.get(&hostile, &[]).await.unwrap_err();
            assert!(
                matches!(err, SlackCliError::Configuration(_)),
                "{hostile} was not rejected: {err}"
            );
        }

        assert!(
            attacker.received_requests().await.unwrap().is_empty(),
            "off-origin hosts must never receive authenticated requests"
        );
    }

    #[tokio::test]
    async fn authenticated_requests_do_not_follow_cross_origin_redirects() {
        let slack = MockServer::start().await;
        let attacker = MockServer::start().await;
        Mock::given(any())
            .and(path("/auth.test"))
            .respond_with(
                ResponseTemplate::new(307)
                    .insert_header("location", format!("{}/capture", attacker.uri())),
            )
            .mount(&slack)
            .await;
        Mock::given(any())
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "ok": true })))
            .mount(&attacker)
            .await;

        let err = client_for(&slack).get("auth.test", &[]).await.unwrap_err();
        assert!(matches!(err, SlackCliError::Api { status: 307, .. }));
        assert!(
            attacker.received_requests().await.unwrap().is_empty(),
            "redirect targets must never receive authenticated requests"
        );
    }

    #[test]
    fn backoff_grows_and_stays_within_bounds() {
        for attempt in 0..6 {
            let wait = backoff_seconds(2, attempt);
            assert!(
                (1..=MAX_BACKOFF_SECS + 12).contains(&wait),
                "wait was {wait}"
            );
        }
        // 上限を超える指数でも 60 秒 ±20% に収まる
        assert!(backoff_seconds(30, 10) <= MAX_BACKOFF_SECS + 12);
    }
}
