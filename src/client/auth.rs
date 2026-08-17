//! 認証ヘッダの組み立てと `auth.test` による疎通確認。
//!
//! Notion と違い API バージョンヘッダは無い。Slack は `chat.postMessage` などで
//! `application/json; charset=utf-8` を要求するため、JSON 送信時は charset まで付ける。

use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use serde_json::Value;

use super::SlackClient;
use crate::error::SlackCliError;

pub const AUTH_TEST_METHOD: &str = "auth.test";
pub const CONTENT_TYPE_JSON: &str = "application/json; charset=utf-8";
pub const CONTENT_TYPE_FORM: &str = "application/x-www-form-urlencoded; charset=utf-8";

impl SlackClient {
    /// `Authorization` のみを持つヘッダ。GET とフォーム送信で使う。
    pub(crate) fn auth_headers(&self) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {}", self.token))
                .expect("token was validated when the client was built"),
        );
        headers
    }

    /// JSON ボディ送信用のヘッダ。
    pub(crate) fn json_headers(&self) -> HeaderMap {
        let mut headers = self.auth_headers();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static(CONTENT_TYPE_JSON));
        headers
    }

    /// フォーム送信用のヘッダ。
    pub(crate) fn form_headers(&self) -> HeaderMap {
        let mut headers = self.auth_headers();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static(CONTENT_TYPE_FORM));
        headers
    }

    /// トークンの有効性を確認する。成功時は `auth.test` のレスポンスをそのまま返す。
    pub async fn auth_test(&self) -> Result<Value, SlackCliError> {
        self.get(AUTH_TEST_METHOD, &[]).await
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use url::Url;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;

    const TEST_TOKEN: &str = "test-token-value";

    fn client_for(server: &MockServer) -> SlackClient {
        SlackClient::new(TEST_TOKEN)
            .unwrap()
            .with_base_url(Url::parse(&format!("{}/", server.uri())).unwrap())
    }

    #[tokio::test]
    async fn auth_test_sends_bearer_token_and_returns_identity() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/auth.test"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "user": "monet",
                "user_id": "U123",
                "team": "forward",
                "team_id": "T123",
            })))
            .expect(1)
            .mount(&server)
            .await;

        let response = client_for(&server).auth_test().await.unwrap();
        assert_eq!(response["user_id"], "U123");

        let requests = server.received_requests().await.unwrap();
        assert_eq!(
            requests[0].headers.get("authorization").unwrap(),
            &format!("Bearer {TEST_TOKEN}")
        );
    }

    #[tokio::test]
    async fn auth_test_surfaces_invalid_auth_as_an_api_error() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/auth.test"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({ "ok": false, "error": "invalid_auth" })),
            )
            .mount(&server)
            .await;

        let err = client_for(&server).auth_test().await.unwrap_err();
        assert!(
            matches!(&err, SlackCliError::Api { code, .. } if code == "invalid_auth"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn json_and_form_headers_carry_the_expected_content_type() {
        let client = SlackClient::new(TEST_TOKEN).unwrap();
        assert_eq!(
            client.json_headers().get(CONTENT_TYPE).unwrap(),
            CONTENT_TYPE_JSON
        );
        assert_eq!(
            client.form_headers().get(CONTENT_TYPE).unwrap(),
            CONTENT_TYPE_FORM
        );
        assert!(client.auth_headers().get(CONTENT_TYPE).is_none());
    }
}
