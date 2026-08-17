//! カーソルページネーション。
//!
//! Slack と Notion で規約が違うので、notion-cli の実装から次の 3 点を作り直した。
//!
//! - カーソルの在り処が `response_metadata.next_cursor`。`has_more` は無く、
//!   **`next_cursor` が空文字なら終わり**。
//! - 結果配列のキーがエンドポイントごとに違う（`channels` / `members` / `messages` /
//!   `matches` …）ため、キー名を引数で受け取る。`messages.matches` のようなドット区切りの
//!   ネストしたパスも指定できる。
//! - リクエストは GET + クエリパラメータが主。JSON ボディ版も用意してある。
//!
//! 無限ループ防止（最大ページ数・同一カーソルの検出）は notion-cli から引き継いだ。

use serde_json::Value;

use super::SlackClient;
use crate::error::SlackCliError;

/// 無限ページングを止めるための安全弁。
const MAX_PAGES: u32 = 10_000;

const CURSOR_PARAM: &str = "cursor";
const LIMIT_PARAM: &str = "limit";
const RESPONSE_METADATA_KEY: &str = "response_metadata";
const NEXT_CURSOR_KEY: &str = "next_cursor";

#[derive(Debug, Clone, Default)]
pub struct PaginationOpts {
    /// 1 リクエストあたりの取得件数（Slack の `limit` パラメータ）。
    pub page_size: Option<u32>,
    /// 開始カーソル。
    pub cursor: Option<String>,
    /// 最後まで辿るか。false かつ `limit` も無ければ 1 ページで打ち切る。
    pub fetch_all: bool,
    /// 収集する総件数の上限。
    pub limit: Option<u32>,
}

impl PaginationOpts {
    pub fn all() -> Self {
        Self {
            fetch_all: true,
            ..Self::default()
        }
    }

    /// 1 ページだけで止めるか（`fetch_all` でも `limit` でもない）。
    fn single_page(&self) -> bool {
        !self.fetch_all && self.limit.is_none()
    }
}

impl SlackClient {
    /// GET エンドポイントをページングして結果を集める。
    ///
    /// `results_key` は結果配列のキー。`"channels"` のような単一キーのほか、
    /// `"messages.matches"`（`search.messages`）のようなドット区切りパスも受ける。
    pub async fn paginate_get(
        &self,
        method: &str,
        params: &[(&str, &str)],
        results_key: &str,
        opts: &PaginationOpts,
    ) -> Result<Vec<Value>, SlackCliError> {
        self.paginate(opts, results_key, |cursor, page_size| {
            let mut query: Vec<(String, String)> = params
                .iter()
                .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                .collect();
            if let Some(size) = page_size {
                query.push((LIMIT_PARAM.to_string(), size.to_string()));
            }
            if let Some(c) = cursor {
                query.push((CURSOR_PARAM.to_string(), c));
            }
            let method = method.to_string();
            async move {
                let borrowed: Vec<(&str, &str)> = query
                    .iter()
                    .map(|(k, v)| (k.as_str(), v.as_str()))
                    .collect();
                self.get(&method, &borrowed).await
            }
        })
        .await
    }

    /// JSON ボディの POST エンドポイントをページングする。
    pub async fn paginate_post_json(
        &self,
        method: &str,
        base_body: &Value,
        results_key: &str,
        opts: &PaginationOpts,
    ) -> Result<Vec<Value>, SlackCliError> {
        self.paginate(opts, results_key, |cursor, page_size| {
            let mut body = base_body.clone();
            if let Some(obj) = body.as_object_mut() {
                if let Some(size) = page_size {
                    obj.insert(LIMIT_PARAM.to_string(), Value::from(size));
                }
                if let Some(c) = cursor {
                    obj.insert(CURSOR_PARAM.to_string(), Value::String(c));
                }
            }
            let method = method.to_string();
            async move { self.post_json(&method, &body).await }
        })
        .await
    }

    /// ページングの本体。`fetch_page(cursor, page_size)` で 1 ページ取ってくる。
    async fn paginate<F, Fut>(
        &self,
        opts: &PaginationOpts,
        results_key: &str,
        fetch_page: F,
    ) -> Result<Vec<Value>, SlackCliError>
    where
        F: Fn(Option<String>, Option<u32>) -> Fut,
        Fut: std::future::Future<Output = Result<Value, SlackCliError>>,
    {
        let mut collected: Vec<Value> = Vec::new();
        let mut cursor = opts.cursor.clone();
        let mut previous_cursor: Option<String> = None;
        let limit = opts.limit.unwrap_or(u32::MAX);
        let mut page_count: u32 = 0;

        loop {
            page_count += 1;
            if page_count > MAX_PAGES {
                return Err(SlackCliError::Pagination(format!(
                    "Exceeded maximum page count ({MAX_PAGES})"
                )));
            }

            let response = fetch_page(cursor.clone(), opts.page_size).await?;

            if let Some(items) = lookup_path(&response, results_key).and_then(Value::as_array) {
                for item in items {
                    if collected.len() as u32 >= limit {
                        return Ok(collected);
                    }
                    collected.push(item.clone());
                }
            }

            if opts.single_page() || collected.len() as u32 >= limit {
                break;
            }

            // Slack は has_more を持たない。next_cursor が空なら終わり。
            let next = next_cursor(&response);
            let Some(next) = next else { break };

            if previous_cursor.as_deref() == Some(next.as_str()) {
                return Err(SlackCliError::Pagination(
                    "Server returned the same cursor twice".into(),
                ));
            }

            previous_cursor = Some(next.clone());
            cursor = Some(next);
        }

        Ok(collected)
    }
}

/// `response_metadata.next_cursor` を読む。空文字と欠落はどちらも「終わり」。
fn next_cursor(response: &Value) -> Option<String> {
    response
        .get(RESPONSE_METADATA_KEY)?
        .get(NEXT_CURSOR_KEY)?
        .as_str()
        .filter(|c| !c.is_empty())
        .map(str::to_string)
}

/// ドット区切りのパスで値を辿る。`"channels"` でも `"messages.matches"` でも引ける。
fn lookup_path<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    path.split('.').try_fold(value, |current, key| current.get(key))
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use url::Url;
    use wiremock::matchers::{any, query_param, query_param_is_missing};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;

    fn client_for(server: &MockServer) -> SlackClient {
        SlackClient::new("test-token-value")
            .unwrap()
            .with_base_url(Url::parse(&format!("{}/", server.uri())).unwrap())
    }

    #[tokio::test]
    async fn follows_response_metadata_next_cursor_until_it_is_empty() {
        let server = MockServer::start().await;

        Mock::given(query_param_is_missing("cursor"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "channels": [{ "id": "C1" }, { "id": "C2" }],
                "response_metadata": { "next_cursor": "page2" },
            })))
            .mount(&server)
            .await;

        Mock::given(query_param("cursor", "page2"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "channels": [{ "id": "C3" }],
                // 空文字がページングの終端
                "response_metadata": { "next_cursor": "" },
            })))
            .mount(&server)
            .await;

        let results = client_for(&server)
            .paginate_get("conversations.list", &[], "channels", &PaginationOpts::all())
            .await
            .unwrap();

        assert_eq!(
            results.iter().map(|c| c["id"].as_str().unwrap()).collect::<Vec<_>>(),
            vec!["C1", "C2", "C3"]
        );
        assert_eq!(server.received_requests().await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn results_key_is_configurable_per_endpoint() {
        let server = MockServer::start().await;
        Mock::given(any())
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "members": ["U1", "U2"],
                "response_metadata": { "next_cursor": "" },
            })))
            .mount(&server)
            .await;

        let members = client_for(&server)
            .paginate_get(
                "conversations.members",
                &[("channel", "C1")],
                "members",
                &PaginationOpts::all(),
            )
            .await
            .unwrap();
        assert_eq!(members, vec![json!("U1"), json!("U2")]);

        // 存在しないキーを指定しても落とさず空を返す
        let none = client_for(&server)
            .paginate_get("conversations.members", &[], "channels", &PaginationOpts::all())
            .await
            .unwrap();
        assert!(none.is_empty());
    }

    #[tokio::test]
    async fn nested_results_key_reaches_search_matches() {
        let server = MockServer::start().await;
        Mock::given(any())
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "messages": {
                    "matches": [{ "ts": "1700000000.000100" }],
                    "total": 1,
                },
                "response_metadata": { "next_cursor": "" },
            })))
            .mount(&server)
            .await;

        let matches = client_for(&server)
            .paginate_get(
                "search.messages",
                &[("query", "hello")],
                "messages.matches",
                &PaginationOpts::all(),
            )
            .await
            .unwrap();
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0]["ts"], "1700000000.000100");
    }

    #[tokio::test]
    async fn stops_after_one_page_when_neither_fetch_all_nor_limit_is_set() {
        let server = MockServer::start().await;
        Mock::given(any())
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "channels": [{ "id": "C1" }],
                "response_metadata": { "next_cursor": "more" },
            })))
            .mount(&server)
            .await;

        let results = client_for(&server)
            .paginate_get(
                "conversations.list",
                &[],
                "channels",
                &PaginationOpts::default(),
            )
            .await
            .unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn limit_caps_the_collected_items() {
        let server = MockServer::start().await;
        Mock::given(any())
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "channels": [{ "id": "C1" }, { "id": "C2" }, { "id": "C3" }],
                "response_metadata": { "next_cursor": "next" },
            })))
            .mount(&server)
            .await;

        let results = client_for(&server)
            .paginate_get(
                "conversations.list",
                &[],
                "channels",
                &PaginationOpts {
                    limit: Some(2),
                    ..PaginationOpts::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(results.len(), 2);
    }

    #[tokio::test]
    async fn repeated_cursor_is_rejected_instead_of_looping_forever() {
        let server = MockServer::start().await;
        Mock::given(any())
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "channels": [{ "id": "C1" }],
                "response_metadata": { "next_cursor": "stuck" },
            })))
            .mount(&server)
            .await;

        let err = client_for(&server)
            .paginate_get("conversations.list", &[], "channels", &PaginationOpts::all())
            .await
            .unwrap_err();
        assert!(matches!(err, SlackCliError::Pagination(_)), "{err}");
    }

    #[tokio::test]
    async fn page_size_is_sent_as_the_limit_parameter() {
        let server = MockServer::start().await;
        Mock::given(query_param("limit", "200"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "channels": [],
                "response_metadata": { "next_cursor": "" },
            })))
            .expect(1)
            .mount(&server)
            .await;

        client_for(&server)
            .paginate_get(
                "conversations.list",
                &[],
                "channels",
                &PaginationOpts {
                    page_size: Some(200),
                    fetch_all: true,
                    ..PaginationOpts::default()
                },
            )
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn opaque_cursors_survive_url_encoding() {
        let server = MockServer::start().await;
        Mock::given(query_param_is_missing("cursor"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "channels": [],
                "response_metadata": { "next_cursor": "a&b+#%=/ z" },
            })))
            .mount(&server)
            .await;
        // デコード後の値でマッチするので、エンコードが正しいことの証明になる
        Mock::given(query_param("cursor", "a&b+#%=/ z"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "channels": [{ "id": "C9" }],
                "response_metadata": { "next_cursor": "" },
            })))
            .expect(1)
            .mount(&server)
            .await;

        let results = client_for(&server)
            .paginate_get("conversations.list", &[], "channels", &PaginationOpts::all())
            .await
            .unwrap();
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn next_cursor_treats_empty_and_missing_alike() {
        assert_eq!(
            next_cursor(&json!({ "response_metadata": { "next_cursor": "abc" } })),
            Some("abc".to_string())
        );
        assert_eq!(
            next_cursor(&json!({ "response_metadata": { "next_cursor": "" } })),
            None
        );
        assert_eq!(next_cursor(&json!({ "response_metadata": {} })), None);
        assert_eq!(next_cursor(&json!({ "ok": true })), None);
    }
}
