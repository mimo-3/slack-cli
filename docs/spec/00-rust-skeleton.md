# Rust骨格の流用元調査 — notion-cli

Slack CLI（TypeScript）をRustへ移植するにあたり、既存のRust製CLIである
`/Users/mimo/organizations/open-source/notion-cli`（v0.2.1, edition 2021, rust-version 1.75）の実装を読み、
そのまま流用できる骨格を洗い出した記録。

## 0. この文書の読み方（調査範囲）

実際に読んだファイル:

- `Cargo.toml`
- `src/main.rs`, `src/error.rs`
- `src/cli/mod.rs`, `src/cli/page.rs`, `src/cli/search.rs`, `src/cli/api.rs`, `src/cli/auth.rs`（一部）
- `src/client/mod.rs`, `src/client/auth.rs`, `src/client/request.rs`, `src/client/pagination.rs`
- `src/config/mod.rs`
- `src/output/mod.rs`, `json.rs`, `yaml.rs`, `csv_out.rs`, `table.rs`, `plain.rs`（冒頭）, `markdown.rs`（冒頭）
- `src/api/search.rs`
- 参考として `/Users/mimo/organizations/open-source/slack-cli/package.json`

読んでいないもの（本書に記述しない、または「未確認」と明記する）:
`src/api/files.rs`（564行、multipartアップロード実装）、`src/cli/file.rs`、`src/cli/db.rs`、
`src/cli/block.rs`、`src/cli/comment.rs`、`src/cli/user.rs`、`src/cli/config_cmd.rs`、
`src/models/` 配下、`src/output/plain.rs` と `markdown.rs` の後半、Slack CLI（TS）本体のソース。

## 1. モジュール構成と責務

notion-cli のソースは5394行、以下の8モジュールで構成される（`src/main.rs` の `mod` 宣言順）。

| モジュール | 行数の目安 | 責務 |
| --- | --- | --- |
| `main.rs` | 101 | エントリポイント。`Cli::parse()` → `run()` へディスパッチ。エラーは `eprintln!` + `process::exit(e.exit_code())`。URLからID抽出する `normalize_id()` もここ |
| `cli/` | 約1600 | clapのコマンド定義（`Cli` / `GlobalOpts` / `Command`）と、各サブコマンドの `run()`。引数を組み立ててAPI層を呼び、結果を `output::format_value` に渡すだけの薄い層 |
| `client/` | 約930 | HTTPクライアント本体。`mod.rs`=構造体と生成、`auth.rs`=ヘッダ組み立て、`request.rs`=GET/POST/PATCH/PUT/DELETE + リトライ + dry-run、`pagination.rs`=カーソルページング |
| `api/` | 約940 | エンドポイント単位の薄いラッパ。`impl NotionClient` を分割ファイルで拡張し、`self.post("/v1/search", ...)` のようにパスとボディを組み立てる |
| `models/` | 約230 | Notionオブジェクトのserde型定義（page/database/block/user/comment/rich_text/properties/common） |
| `output/` | 約840 | `OutputFormat` enumと、各フォーマッタ（json/yaml/csv+tsv/plain/markdown/id-only、加えて未接続のtable） |
| `config/` | 396 | 設定ファイルと資格情報ファイルの読み書き、プロファイル、トークン解決チェーン |
| `error.rs` | 67 | `CliError`（thiserror）と終了コードのマッピング、APIエラーレスポンス型 |
| `filter/` | 3行 | 未実装のプレースホルダ（コメントのみ。pest製フィルタDSLの構想が書かれている） |

重要な構造的特徴（Slack CLIでも踏襲する価値がある）:

- **`api/` と `cli/` が分かれている**。`api/*.rs` は `impl NotionClient { pub async fn search(...) }` の形で
  クライアントにメソッドを生やす。CLI側はHTTPを一切知らない。テストは `api/` 側に置かれている。
- **すべての層が `serde_json::Value` で通信している**。`models/` の型は存在するが、
  `client` の戻り値は一貫して `Result<Value, CliError>` で、`output` も `&serde_json::Value` を受ける。
  型を厳密に定義せずに済み、フォーマッタが汎用化できるという利点と、コンパイル時の保証が弱いという欠点が両方ある。
- **`filter/` は空**。「モジュールだけ切っておいて後で埋める」やり方をしている。

## 2. 流用可否の判定

### 2.1 output層 — ほぼそのままコピーできる

`output/mod.rs` の `OutputFormat` enum、`FromStr`/`Display` 実装、`format_value()` のディスパッチは
Notion固有の要素がなく、そのままコピーできる。

- `json.rs`（9行）、`yaml.rs`（10行）、`csv_out.rs`（72行）、`table.rs`（57行）: **そのままコピーできる**。
  いずれも `&serde_json::Value` と `&mut dyn Write` だけに依存し、ドメイン知識がない。
- `id-only` 分岐（`format_value` 内にインライン）: **一部改変**。`value.get("id")` を見ているが、
  Slackの識別子はチャンネルなら `id`、メッセージなら `ts` と分かれるため、キーを差し替えるか複数キーを順に見る必要がある。
- `plain.rs`（196行）: **作り直し**。`map.get("object")` の値（`page` / `database` / `data_source` / `user` /
  `block` / `comment` / `list`）で描画を分岐しており、中身は完全にNotion専用。ただし
  「オブジェクト種別で分岐 → 種別ごとの整形関数 → 未知の型は key: value のフォールバック → 配列は再帰 →
  末尾に `--- (N results) ---`」という構造は流用価値が高い。Slackなら `message` / `channel` / `user` / `file` で分岐する形になる。
- `markdown.rs`（416行）: **作り直し**。Notionのブロック配列をMarkdownへ変換する専用ロジック（`RenderContext` が
  番号付きリストのカウンタを保持する）。Slack CLIでMarkdown出力が要るなら、mrkdwn→Markdown変換として別途書くことになる。

注意点: `table.rs` は実装されているが `OutputFormat` に `Table` バリアントが無く、`write_table` は
どこからも呼ばれていない（`grep` で定義箇所のみヒット）。**デッドコードなので、流用するなら
`OutputFormat::Table` の追加と `format_value` への接続を自分でやる必要がある**。

### 2.2 error層 — 一部改変

`error.rs` の骨格（`thiserror::Error` の enum、`#[error(transparent)] #[from]` による
reqwest/io/serde_json の取り込み、`exit_code()` によるプロセス終了コードのマッピング）は**そのままコピーできる**。

改変が要るのはバリアントの中身:

- `Api { status, code, message }` と `ErrorResponse`: SlackのWeb APIは**HTTP 200を返しつつ
  ボディの `ok: false` と `error: "channel_not_found"` でエラーを表す**ため、
  「非2xxのときだけエラー扱いする」現在の判定では取りこぼす。`ok` フィールドを見る分岐が追加で要る。
- `RateLimited { retry_after }`: Slackも `Retry-After` 付き429を返すのでそのまま活きる（終了コード4）。
- `NotAuthenticated`（終了コード3）: メッセージ文言をSlack向けに変えるだけ。
- `FilterParse` / `InvalidId` / `Pagination` / `Config` / `OAuth`: `FilterParse` は不要、
  `InvalidId` はチャンネルID検証に転用可、他はそのまま。

### 2.3 config層 — ほぼそのままコピーできる

`config/mod.rs` は Notion 依存が薄く、資産価値が最も高い部分。

- `CredentialsStore`: トークンを設定ファイルから分離し `credentials.json` に置く。`Debug` を derive しない
  （トークンがログに出ないようにする）。Unixではパーミッションが `0o077` に触れていたら stderr で警告する。
- `write_private()`: 親ディレクトリを0700で作り、`create_new` + `mode(0o600)` の一時ファイルへ書いて
  `rename` する原子的書き込み。失敗時は一時ファイルを消す。非Unixでは `fs::write` にフォールバック。
- `Profile` の手書き `Debug` 実装（トークンを `***` にマスクする）。
- `resolve_token()` の優先順位: 明示フラグ → 環境変数 → credentialsファイル → 設定ファイル内のレガシー平文。
- `get_value` / `set_value` によるドット区切りキーの設定操作（`notion config set defaults.page_size 50`）。

**そのままコピーできる**。改変は文字列の差し替え程度:
ディレクトリ名 `notion-cli` → `slack-cli`、環境変数 `NOTION_API_TOKEN` → `SLACK_TOKEN`（Slackは
bot token と user token を持ち得るので2スロット必要になる可能性がある。TS版の実装は未確認）、
`Defaults.output_format` / `page_size` はそのまま流用可。

### 2.4 HTTPクライアント — 一部改変

`client/mod.rs` + `request.rs` + `auth.rs` の3点セット。骨格はそのまま使える。

改変が要る点:

1. **エラー判定**: 上述のとおり Slack は 200 + `ok: false`。`request_with_retry` の成功パスで
   `body["ok"] == false` を検査してエラーに変換する処理を足す。
2. **認証ヘッダ**: `Notion-Version` ヘッダは不要。`Authorization: Bearer <token>` はそのまま。
   Slackの `chat.postMessage` などは `application/json; charset=utf-8` を要求するので Content-Type の調整が要る。
   一部エンドポイント（`files.upload` 系）は form-encoded / multipart。
3. **APIバージョンのピン留め**（`patch_with_api_version`）: Slackにバージョンヘッダの概念がないので不要。
4. **`api_url()` のオリジン検証**はそのまま流用すべき。絶対URL・プロトコル相対URL・バックスラッシュ権限部を
   すべて拒否し、`base_url` のオリジンから逸脱したら `CliError::Config` にする。これは
   `slack api` のような生API呼び出しコマンドを作るなら必須の防御。
5. **リダイレクト無効化**（`redirect::Policy::none()`）もそのまま。認証ヘッダ付きリクエストが
   攻撃者のホストへ転送されるのを防ぐ。
6. **dry-run**: POST/PATCH/PUT/DELETE のときだけ送信せず stderr にログして `{}` を返す。GETは実行する。
   Slack CLIでも「送信せずに文面を確認する」用途にそのまま使える。

リトライ: **あり**。429と529のみ対象で `max_retries = 3` 固定。`Retry-After` ヘッダを読み、
`retry_after * 2^attempt`（上限60秒）に ±20% のジッタを乗せて `tokio::time::sleep`。
リトライ回数を使い切ると `CliError::RateLimited` を返す。**そのままコピーできる**（Slackも同じ形式の429を返す）。

### 2.5 ページネーション — 一部改変

`client/pagination.rs` の `PaginationOpts { page_size, start_cursor, fetch_all, limit }` と
`paginate_post` / `paginate_get` は、カーソル型ページングの実装として完成度が高い。

流用できる安全策:

- `MAX_PAGES = 10_000` の無限ループ防止。
- `has_more == true` なのに `next_cursor` が無ければエラー。
- 前回と同じカーソルが返ってきたらエラー（サーバ側バグでの無限ループ防止）。
- `fetch_all` でも `limit` でもない場合は1ページだけ取得して打ち切る。
- GET側は `url::form_urlencoded::Serializer` でカーソルをURLエンコードする（`a&b+#%` のような
  不透明カーソルが壊れないことをテストで担保している）。

改変が要る点: レスポンス形状がNotion固有。Slackは
`response_metadata.next_cursor` にカーソルを置き、`has_more` は無く「`next_cursor` が空文字なら終わり」という規約、
かつ結果配列のキーはエンドポイントごとに `channels` / `members` / `messages` などと変わる。
したがって **`results` / `has_more` / `next_cursor` の3箇所を、キー名を引数で受け取る形に一般化する改変**が必要。
リクエスト側も、Slackのページングは主にGET/form-encodedのクエリ `cursor` と `limit` なので
`paginate_post` の body 埋め込み（`page_size` / `start_cursor`）は使わないか書き換えになる。

### 2.6 認証（auth） — 一部改変

`cli/auth.rs`（539行）は `login` / `logout` / `whoami` / `switch` の4サブコマンド。

- **トークンログイン**: `dialoguer::Password` で対話入力（argvに残さない）→ `/v1/users/me` を叩いて検証 →
  `config.store_token()` で保存。**そのままコピーできる**（検証エンドポイントを `auth.test` に差し替えるだけ）。
- **ブラウザOAuth**（`login_browser`）: 32文字の英数ランダム state をCSPRNGで生成 →
  `127.0.0.1:0` にTcpListenerをバインドしてポートを得る → `redirect_uri = http://localhost:<port>/callback` →
  `open::that(auth_url)` でブラウザを開く → ノンブロッキングacceptで最大10接続・300秒待ち →
  リクエスト行から `code` を取り出し state を照合 → `exchange_code_for_token` で
  `Authorization: Basic base64(client_id:client_secret)` を付けてトークン交換。
  **一部改変**で流用できる。Slack OAuth v2 のトークン交換は `oauth.v2.access` で、
  クライアント資格情報の渡し方（Basic か form パラメータか）とレスポンスの取り出し先
  （`access_token` か `authed_user.access_token` か）が違うため、そこだけ書き換える。
  ローカルループバックサーバ・state検証・タイムアウト処理は丸ごと流用可能。
- `whoami`: `/v1/users/me` の結果をフォーマッタに流すだけ。エンドポイント差し替えのみ。

## 3. clapのサブコマンド定義スタイル

**deriveスタイル**（`clap = { version = "4", features = ["derive", "env"] }`）。builderは使っていない。

ネストは「トップの `Command` enum が、各モジュールの `XxxCommand`（`#[derive(Args)]`）を1タプル要素で保持し、
その中に `#[command(subcommand)]` で `XxxSubcommand` enum を入れる」という2段構え。
グローバルオプションは `#[command(flatten)]` + `global = true` で全サブコマンドに配る。

```rust
// src/cli/mod.rs
#[derive(Parser)]
#[command(name = "notion", version, about = "Command-line interface for the Notion API")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,

    #[command(flatten)]
    pub global: GlobalOpts,
}

#[derive(Args, Debug)]
pub struct GlobalOpts {
    /// Notion API token (overrides env and config)
    #[arg(long, env = "NOTION_API_TOKEN", global = true, hide_env_values = true)]
    pub token: Option<String>,

    /// Output format
    #[arg(long, global = true, default_value = "plain")]
    pub format: OutputFormat,

    /// Shorthand for --format json
    #[arg(long, global = true)]
    pub json: bool,

    /// Show what would be done without making changes
    #[arg(long, global = true)]
    pub dry_run: bool,
    // ... profile / api_version / verbose / no_color
}

impl GlobalOpts {
    /// Resolve the effective output format, considering --json shorthand.
    pub fn output_format(&self) -> OutputFormat {
        if self.json { OutputFormat::Json } else { self.format }
    }
}

#[derive(Subcommand)]
pub enum Command {
    /// Manage authentication
    Auth(auth::AuthCommand),
    /// Search across all pages and databases
    Search(search::SearchArgs),
    /// Work with pages
    Page(page::PageCommand),
    // ...
}
```

2段目（`src/cli/page.rs`）。サブコマンドの引数は enum のバリアント内に直接書く:

```rust
#[derive(Args)]
pub struct PageCommand {
    #[command(subcommand)]
    pub command: PageSubcommand,
}

#[derive(Clone, Copy, ValueEnum)]
pub enum PageParentType {
    /// A regular Notion page
    Page,
    /// A database data source (use the data source ID, not the database ID)
    DataSource,
}

#[derive(Subcommand)]
pub enum PageSubcommand {
    /// Retrieve a page by ID
    Get {
        /// Page ID or URL
        id: String,
    },
    /// Create a new page
    Create {
        #[arg(long)]
        parent: String,
        #[arg(long)]
        title: String,
        /// Read content from stdin as JSON blocks
        #[arg(long)]
        stdin: bool,
        #[arg(long, value_enum, default_value_t = PageParentType::Page)]
        parent_type: PageParentType,
    },
    // ...
}
```

ディスパッチは `main.rs` で行い、クライアント生成もそこでまとめている:

```rust
async fn run(cli: Cli) -> Result<(), error::CliError> {
    let mut config = Config::load()?;
    match cli.command {
        Command::Auth(cmd) => cli::auth::run(cmd, &mut config, &cli.global).await,
        Command::Search(args) => {
            let client = client::NotionClient::from_opts(&cli.global, &config)?;
            cli::search::run(args, &client, &cli.global).await
        }
        // ...
    }
}
```

各サブコマンドの `run()` シグネチャは統一されている: `(cmd, &client, &global) -> Result<(), CliError>`。
末尾は必ず `output::format_value(&value, global.output_format(), &mut std::io::stdout())`。
人間向けの補足メッセージ（`Page {id} moved to trash.`）は **stderr** に出し、stdoutはデータ専用にしている。

値の制約は `value_parser` と `ValueEnum` を併用:

```rust
// src/cli/search.rs
#[arg(long, value_name = "TYPE", value_parser = ["page", "data_source"])]
pub filter: Option<String>,
```

`OutputFormat` は `FromStr` を実装しているため、`ValueEnum` を derive せずとも `#[arg(long)]` の
型として使える（`"yml"`→Yaml、`"md"`→Markdown のような別名を受けたいので手書きの `FromStr` を選んでいる）。

## 4. HTTPクライアントの作り方

構造体とビルダー:

```rust
// src/client/mod.rs
const DEFAULT_BASE_URL: &str = "https://api.notion.com";
const DEFAULT_API_VERSION: &str = "2026-03-11";

pub struct NotionClient {
    pub(crate) http: reqwest::Client,
    pub(crate) base_url: Url,
    pub(crate) token: String,
    pub(crate) api_version: String,
    pub(crate) max_retries: u32,
    pub(crate) dry_run: bool,
}

impl NotionClient {
    pub fn from_opts(opts: &GlobalOpts, config: &Config) -> Result<Self, CliError> {
        let token = config.resolve_token(opts.token.as_deref(), opts.profile.as_deref())?;
        // ヘッダにできない値はここで弾く。ヘッダ組み立て側はユーザー入力を扱わなくて済む
        reqwest::header::HeaderValue::from_str(&api_version)
            .map_err(|_| CliError::Config(format!("Invalid API version: {api_version}")))?;

        let http = reqwest::Client::builder()
            .user_agent(format!("notion-cli/{}", env!("CARGO_PKG_VERSION")))
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        Ok(Self { http, base_url, token, api_version, max_retries: 3, dry_run: opts.dry_run })
    }

    #[cfg(test)]
    pub fn with_base_url(mut self, url: Url) -> Self { self.base_url = url; self }
}
```

`with_base_url` が `#[cfg(test)]` になっているのが要点で、**wiremockのURLを差し込むためだけの穴**として
本番ビルドからは消える。

認証ヘッダ（`src/client/auth.rs` 全26行）:

```rust
impl NotionClient {
    pub(crate) fn notion_headers(&self) -> HeaderMap {
        self.notion_headers_with_version(&self.api_version)
    }

    pub(crate) fn notion_headers_with_version(&self, api_version: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            "Authorization",
            HeaderValue::from_str(&format!("Bearer {}", self.token))
                .expect("token should be valid header value"),
        );
        headers.insert("Notion-Version", HeaderValue::from_str(api_version).expect(...));
        headers.insert("Content-Type", HeaderValue::from_static("application/json"));
        headers
    }
}
```

パスの検証（SSRF/オリジン逸脱の防止）:

```rust
pub(crate) fn api_url(&self, path: &str) -> Result<url::Url, CliError> {
    if url::Url::parse(path).is_ok() {
        return Err(CliError::Config(
            "API path must be relative to the configured Notion API origin".to_string()));
    }
    let url = self.base_url.join(path)
        .map_err(|e| CliError::Config(format!("Invalid API path {path}: {e}")))?;
    if url.origin() != self.base_url.origin() {
        return Err(CliError::Config(
            "API path must not change the configured Notion API origin".to_string()));
    }
    Ok(url)
}
```

リトライとエラー変換（クロージャでリクエストを毎回作り直すのが肝。`RequestBuilder` は
`send()` で消費されるため、`F: Fn() -> reqwest::RequestBuilder` を受ける）:

```rust
pub(crate) async fn request_with_retry<F>(&self, build_request: F) -> Result<Value, CliError>
where F: Fn() -> reqwest::RequestBuilder,
{
    let mut last_retry_after = 1u64;
    for attempt in 0..=self.max_retries {
        let response = build_request().send().await?;
        let status = response.status();

        if status.is_success() {
            let body: Value = response.json().await?;
            return Ok(body);   // ← Slackではここで body["ok"] == false を判定する必要がある
        }

        if status.as_u16() == 429 || status.as_u16() == 529 {
            if attempt == self.max_retries {
                return Err(CliError::RateLimited { retry_after: last_retry_after });
            }
            let retry_after = response.headers().get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(1);

            // 指数バックオフ + ±20%ジッタ、上限60秒
            let backoff = retry_after.saturating_mul(1 << attempt).min(60);
            let jitter_range = (backoff as f64 * 0.2) as u64;
            let jitter = if jitter_range > 0 {
                rand::thread_rng().gen_range(0..=jitter_range * 2) as i64 - jitter_range as i64
            } else { 0 };
            let wait = (backoff as i64 + jitter).max(1) as u64;
            last_retry_after = wait;
            tokio::time::sleep(Duration::from_secs(wait)).await;
            continue;
        }

        let error_body = response.text().await.unwrap_or_default();
        if let Ok(err_resp) = serde_json::from_str::<ErrorResponse>(&error_body) {
            return Err(CliError::Api { status: err_resp.status, code: err_resp.code, message: err_resp.message });
        }
        return Err(CliError::Api { status: status.as_u16(), code: "unknown".to_string(), message: error_body });
    }
    Err(CliError::RateLimited { retry_after: last_retry_after })
}
```

各メソッドは薄い。dry-run分岐 → `request_with_retry` にクロージャを渡すだけ:

```rust
pub async fn post(&self, path: &str, body: &Value) -> Result<Value, CliError> {
    let url = self.api_url(path)?;
    if self.dry_run { return self.dry_run_log("POST", &url, Some(body)); }
    self.request_with_retry(|| self.http.post(url.clone()).headers(self.notion_headers()).json(body)).await
}
```

エンドポイント層（`src/api/search.rs`）は `impl NotionClient` を別ファイルで拡張する:

```rust
impl NotionClient {
    pub async fn search(&self, query: &str, filter_object_type: Option<&str>,
                        sort_direction: &str, sort_timestamp: &str,
                        pagination: &PaginationOpts) -> Result<Vec<Value>, CliError> {
        let mut body = json!({ "query": query,
            "sort": { "direction": sort_direction, "timestamp": sort_timestamp } });
        if let Some(obj_type) = filter_object_type {
            body["filter"] = json!({ "value": obj_type, "property": "object" });
        }
        self.paginate_post("/v1/search", &body, pagination).await
    }
}
```

## 5. テストの書き方（wiremock）

dev-dependency は `wiremock = "=0.6.4"` の1つだけ（バージョンを完全固定している）。
テストは全て `#[cfg(test)] mod tests` としてソースファイル内にコロケーションされ、
`tests/` ディレクトリは存在しない。非同期テストは `#[tokio::test]`。

基本形（モックサーバを立てて `with_base_url` で差し込む）:

```rust
#[cfg(test)]
mod tests {
    use serde_json::json;
    use url::Url;
    use wiremock::{matchers::{any, method, path, query_param}, Mock, MockServer, ResponseTemplate};
    use super::*;

    fn client_for(server: &MockServer) -> NotionClient {
        let base_url = Url::parse(&format!("{}/", server.uri())).unwrap();
        NotionClient::new("secret_test".to_string()).unwrap().with_base_url(base_url)
    }

    #[tokio::test]
    async fn paginate_post_collects_all_pages() {
        let server = MockServer::start().await;

        // 1ページ目だけに使われるモック（up_to_n_times で消費回数を制限）
        Mock::given(any())
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "results": [{"id": "1"}, {"id": "2"}], "has_more": true, "next_cursor": "cursor_page2"
            })))
            .up_to_n_times(1)
            .mount(&server).await;

        // 2ページ目以降にマッチするモック
        Mock::given(any())
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "results": [{"id": "3"}], "has_more": false, "next_cursor": null
            })))
            .mount(&server).await;

        let opts = PaginationOpts { fetch_all: true, ..Default::default() };
        let result = client_for(&server).paginate_post("/v1/test", &json!({}), &opts).await.unwrap();
        assert_eq!(result.len(), 3);
    }
}
```

リクエスト内容の検証は2通り使い分けている。

(a) `Mock::given(...)` のマッチャで期待を表明し、`.expect(1)` で呼ばれた回数を検証する:

```rust
Mock::given(method("POST"))
    .and(path("/v1/search"))
    .and(body_json(json!({
        "query": "", "sort": {"direction": "descending", "timestamp": "last_edited_time"},
        "filter": {"property": "object", "value": "data_source"},
        "page_size": 50,
    })))
    .respond_with(ResponseTemplate::new(200).set_body_json(json!({"results": [], "has_more": false})))
    .expect(1)
    .mount(&server).await;
```

クエリ文字列の検証は `query_param("start_cursor", "a&b+#%")`（デコード後の値でマッチするので、
エンコードが正しいことの証明になる）。

(b) 実行後に `server.received_requests()` を取ってヘッダ等を後から検査する:

```rust
let requests = notion.received_requests().await.unwrap();
for request in requests {
    assert_eq!(request.headers.get("authorization").unwrap(), &format!("Bearer {TEST_TOKEN}"));
    assert_eq!(request.headers.get("notion-version").unwrap(), TEST_API_VERSION);
}
```

「送信されないこと」の検証も同じ手段で行う。dry-run とオリジン逸脱拒否のテストは、
**攻撃者役のMockServerをもう1つ立てて `received_requests().is_empty()` を主張する**:

```rust
#[tokio::test]
async fn authenticated_requests_do_not_follow_cross_origin_redirects() {
    let notion = MockServer::start().await;
    let attacker = MockServer::start().await;
    Mock::given(any()).and(path("/redirect"))
        .respond_with(ResponseTemplate::new(307)
            .insert_header("location", format!("{}/capture", attacker.uri())))
        .mount(&notion).await;
    mount_json_response(&attacker, "/capture").await;

    let result = client_for(&notion).get("/redirect").await;
    assert!(matches!(result, Err(CliError::Api { status: 307, .. })));
    assert!(attacker.received_requests().await.unwrap().is_empty(),
        "redirect targets must never receive authenticated requests");
}
```

HTTPメソッドを横断して同じ性質を確かめるために、テスト用の小さな enum を切って全メソッドを回す手法も使われている
（`enum RequestMethod { Get, Post, Patch, Put, Delete }` に `const ALL: [Self; 5]` と `async fn send()` を生やす）。

clapの引数定義そのものも `Cli::try_parse_from([...])` でテストされている（`src/cli/mod.rs`）:

```rust
let error = Cli::try_parse_from(["notion", "db", "query", "source-id", "--filter", "Status = Done"])
    .err().expect("the unpublished filter DSL must be rejected");
assert_eq!(error.kind(), ErrorKind::UnknownArgument);
```

## 6. Cargo.toml の推奨依存（Slack CLI向けに調整）

notion-cli の実際の依存は本書冒頭の調査対象どおり。これをSlack CLI向けに過不足を調整すると以下になる。

```toml
[dependencies]
clap = { version = "4", features = ["derive", "env"] }
reqwest = { version = "0.12", features = ["json", "rustls-tls", "multipart"], default-features = false }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
tokio = { version = "1", features = ["rt-multi-thread", "macros", "time"] }
anyhow = "1"
thiserror = "2"
colored = "3"
csv = "1"
dirs = "6"
chrono = { version = "0.4", features = ["serde"] }
url = "2"
dialoguer = "0.11"
comfy-table = "7"
rand = "0.8"
open = "5"
base64 = "0.22"
urlencoding = "2"

[dev-dependencies]
wiremock = "=0.6.4"
```

判断の内訳:

| クレート | 判断 | 理由 |
| --- | --- | --- |
| `clap` (derive, env) | 必須 | 4章のスタイルをそのまま踏襲する |
| `reqwest` (json, rustls-tls, multipart, default-features=false) | 必須 | rustls固定でOpenSSLへの依存を避ける。multipart は `files.upload` 系で要る |
| `serde` / `serde_json` | 必須 | 全層が `Value` を流す設計の前提 |
| `tokio` | 必須（featureは絞る） | notion-cli は `features = ["full"]` だが、実際に使うのは `#[tokio::main]`（rt-multi-thread + macros）と `time::sleep` だけ。ビルド時間のため絞ってよい |
| `thiserror` | 必須 | `CliError` の定義 |
| `anyhow` | 任意 | notion-cli は依存に入れているが、読んだ範囲では `CliError` で完結しており使用箇所を確認できなかった。**不要なら外す** |
| `dirs` | 必須 | 設定・資格情報ディレクトリの解決 |
| `url` | 必須 | `api_url()` のオリジン検証と `form_urlencoded` によるクエリ組み立て |
| `dialoguer` | 必須 | トークンをargvに残さない対話入力 |
| `rand` | 必須 | リトライのジッタとOAuth stateのCSPRNG生成 |
| `open` | OAuthを実装するなら必須 | ブラウザ起動 |
| `base64` / `urlencoding` | OAuthを実装するなら必須 | Basic認証ヘッダとリダイレクトURIのエンコード |
| `comfy-table` | 表出力を実装するなら必須 | notion-cli では未接続のデッドコードだった。Slack CLIでは `OutputFormat::Table` を実際に配線する前提で入れる |
| `csv` | csv/tsv出力を出すなら必須 | |
| `colored` | 推奨 | `--no-color` で `colored::control::set_override(false)` する形をそのまま使える |
| `chrono` | 推奨 | Slackの `ts`（Unix秒.マイクロ秒）を人間可読に整形するのに要る |
| `serde_yaml` | 判断保留 | notion-cli は 0.9 を使っているが serde_yaml はメンテ終了済み。YAML出力が要るなら後継（例: serde_yaml_ng / serde_norway）への差し替えを検討する。**要らないなら落とす** |
| `uuid` | 落としてよい | notion-cli では設定ファイルの一時ファイル名生成にしか使っていない。プロセスIDやカウンタで代替できる |
| `wiremock`（dev） | 必須 | 5章のテスト方式の前提。バージョンは `=` 固定されていた |

Slack固有で追加検討が要るもの（notion-cli には存在しない）:

- **並行実行の制御**: TS版は `p-limit` に依存している。Rustでは `tokio::sync::Semaphore`（tokio標準）や
  `futures::stream::buffer_unordered` で足りる可能性が高く、新規クレートは不要と見込まれる。
  ただしTS版のどこで並行度を絞っているかは未確認。
- **Slack SDK**: TS版は `@slack/web-api` を使っているが、Rust側は notion-cli 同様に
  reqwest直叩き + `serde_json::Value` で組む前提とする（サードパーティのSlackクレートは調査していない）。
- **バージョン比較**: TS版の `semver` 依存が何に使われているかは未確認。自己アップデート通知の類なら
  Rustでは `semver` クレートが対応するが、要否は未確定。

## 7. 未確認・要調査

- Slack CLI（TS）のソース本体（`src/commands`, `src/utils`, `src/types`）は未読。
  したがって「移すべきコマンドの一覧」「現在の認証トークンの扱い」「p-limit / semver の用途」は本書では不明。
- notion-cli の `src/api/files.rs`（564行）にあるはずのmultipartアップロード実装は未読。
  Slackのファイルアップロード移植時に再確認する価値がある。
- `src/models/` の型定義の使われ方（`Value` 中心の設計の中でどこが型を使っているか）は未確認。
