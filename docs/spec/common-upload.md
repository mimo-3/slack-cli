# ファイルアップロード / ダウンロード 実装仕様（Rust 移植用）

対象コード（すべて読み取り専用の参照元）:

- `slack-cli/src/commands/upload.ts`
- `slack-cli/src/commands/download.ts`
- `slack-cli/src/utils/slack-operations/file-operations.ts`
- `slack-cli/src/utils/slack-operations/base-client.ts`
- `slack-cli/src/utils/validators.ts`（`fileOrContent` / `uploadThreadTimestamp`）
- `slack-cli/node_modules/@slack/web-api/dist/WebClient.js`
- `slack-cli/node_modules/@slack/web-api/dist/file-upload.js`
- `slack-cli/node_modules/@slack/web-api/dist/types/request/files.d.ts`
- `slack-cli/node_modules/@slack/web-api/dist/types/response/FilesGetUploadURLExternalResponse.d.ts`
- `slack-cli/node_modules/@slack/web-api/dist/types/response/FilesCompleteUploadExternalResponse.d.ts`
- `slack-cli/node_modules/form-data/lib/form_data.js`
- 流用元: `notion-cli/src/api/files.rs`, `notion-cli/src/client/mod.rs`, `notion-cli/src/client/request.rs`

CLI のオプション定義・出力フォーマットは `cmd-file.md` にある。本書は **HTTP レイヤの再現**に絞る。

---

## 0. 全体像

`slack-cli upload` は `client.uploadFile()` → `WebClient.files.uploadV2()` を呼ぶ。
`files.uploadV2` は Slack の Web API メソッドではなく **SDK 側のヘルパ**であり、内部で 3 段階の HTTP を撃つ。
Rust には SDK が無いので、この 3 段階を自前で書く必要がある。

```
files.getUploadURLExternal   (POST https://slack.com/api/..., x-www-form-urlencoded)
        ↓  upload_url, file_id
実データ POST                (POST <upload_url>, multipart/form-data)
        ↓
files.completeUploadExternal (POST https://slack.com/api/..., x-www-form-urlencoded)
        ↓  files[]
```

`slack-cli` 側の `uploadFile()` は先に `ChannelOperations.resolveChannelId()` でチャンネル名 → ID 変換を済ませ、
`channel_id` として渡す（`file-operations.ts:96`）。

---

## 1. 三段階の正確なリクエスト仕様

### 1-1. WebClient 共通の下地（`WebClient.js`）

すべての `slack.com/api/*` 呼び出しに共通する挙動。Rust 側でも同じにする。

- ベース URL: `https://slack.com/api/`（`slackApiUrl` 既定）。メソッド名を連結して `https://slack.com/api/files.getUploadURLExternal`。
- 既定ヘッダに `Authorization: Bearer <token>` を入れる（`WebClient.js:163`）。`User-Agent` も既定で付く。
- ボディのシリアライズは `serializeApiCallData`（`WebClient.js:536-613`）:
  - 値に Buffer / Stream が **含まれない**場合 → `Content-Type: application/x-www-form-urlencoded`、`node:querystring.stringify` で直列化。
  - 値が string / number / boolean 以外（配列・オブジェクト）は **JSON 文字列化してから**フォームフィールドに載せる。`blocks` がこれに該当。
  - 値が `undefined` / `null` のキーは丸ごと落とす。
- `apiCall` は `Object.assign({ team_id: this.teamId }, options)` を送る（`WebClient.js:201`）。`slack-cli` は `teamId` を渡していないので `team_id` は `undefined` になり、上記のルールで**送信されない**。
- `validateStatus: () => true`、`maxRedirects: 0`。200 以外は 429 を除きエラー（`WebClient.js:504`）。
- 429 のときは `retry-after` ヘッダ秒数だけ待って再試行。ただし `slack-cli` は `retryConfig: { retries: 0 }` を指定している（`base-client.ts:15-17`）ので、**再試行は 0 回**、429 は即エラー。
- レスポンス JSON の `ok: false` は SDK 側が `WebAPIPlatformError` として throw する。

### 1-2. Step 1: `files.getUploadURLExternal`

`WebClient.js:379-392`（`fetchAllUploadURLExternal`）。ファイル 1 件につき 1 回、`Promise.all` で並列。

リクエスト:

```
POST https://slack.com/api/files.getUploadURLExternal
Authorization: Bearer xoxp-...
Content-Type: application/x-www-form-urlencoded

filename=<name>&length=<bytes>&snippet_type=<type>
```

パラメータ（`files.d.ts:71-80` / 送信元は `WebClient.js:381-386`）:

| キー | 必須 | 値 |
| --- | --- | --- |
| `filename` | 必須 | 後述 1-5 で決まるファイル名 |
| `length` | 必須 | ファイル本体のバイト数（`Buffer.byteLength`）|
| `alt_text` | 任意 | `slack-cli` は渡さない（常に undefined → 送信されない）|
| `snippet_type` | 任意 | `--filetype` の値。未指定なら送信されない |

**ここに `channel_id` / `title` / `initial_comment` / `thread_ts` は渡らない**。それらは Step 3 に回る。

レスポンス（`FilesGetUploadURLExternalResponse.d.ts`）:

```json
{ "ok": true, "upload_url": "https://files.slack.com/upload/v1/...", "file_id": "F0123456789" }
```

失敗時は `ok:false` + `error` / `needed` / `provided` / `response_metadata.messages[]`。

### 1-3. Step 2: `upload_url` への実 POST

`WebClient.js:408-427`（`postFileUploadsToExternalURL`）。これも `Promise.all` で並列。

呼び出しは `this.makeRequest(upload_url, { body }, headers)`。
つまり **ボディは `{ body: <Buffer> }` という 1 フィールドのオブジェクト**。
`serializeApiCallData` が Buffer を検出して multipart に切り替える（`WebClient.js:562-598`）。

実際に飛ぶリクエスト:

```
POST https://files.slack.com/upload/v1/....
Authorization: Bearer xoxp-...
Content-Type: multipart/form-data; boundary=--------------------------<random>

----------------------------<random>
Content-Disposition: form-data; name="body"; filename="Untitled"
Content-Type: application/octet-stream

<file bytes>
----------------------------<random>--
```

決め手になる細部:

- **フィールド名は `body`**。ファイル名でもなく `file` でもない。`makeRequest(upload_url, { body }, ...)` のキーがそのまま multipart のフィールド名になる。
- **`filename` は必ず `"Untitled"`**。`WebClient.js:567-584` は値の `.name` / `.path` からファイル名を拾おうとするが、渡っているのは素の `Buffer` でどちらも持たないため `defaultFilename = 'Untitled'`（`WebClient.js:107`）に落ちる。
- **パートの `Content-Type` は `application/octet-stream`**。`form-data` の `_getContentType`（`form_data.js:253-283`）が `mime.lookup('Untitled')` に失敗し、`FormData.DEFAULT_CONTENT_TYPE` にフォールバックするため。つまり **Slack は本文 MIME を見ておらず、拡張子（= Step 1 の `filename`）で種別を決めている**。
- `Authorization` ヘッダは付く。`postFileUploadsToExternalURL` は `options.token`（呼び出し単位のトークン上書き）がある場合のみ明示的に足すが、無くても axios の既定ヘッダ（`WebClient.js:163` で設定）に `Authorization` が入っているので、結果として `files.slack.com` にも Bearer が送られる。
- **成功判定は `status !== 200` のみ**（`WebClient.js:420`）。`maxRedirects: 0` なので 3xx は失敗扱い。
- **レスポンスボディは使われない**。`{ ok: true, body: uploadRes.data }` を返すが、`filesUploadV2` は `await` するだけで戻り値を捨てている（`WebClient.js:369`）。Rust 側もステータスだけ見ればよい。

### 1-4. Step 3: `files.completeUploadExternal`

`WebClient.js:395-402` + グルーピングは `file-upload.js:191-228`（`getAllFileUploadsToComplete`）。

リクエスト:

```
POST https://slack.com/api/files.completeUploadExternal
Authorization: Bearer xoxp-...
Content-Type: application/x-www-form-urlencoded

files=%5B%7B%22id%22%3A%22F0123%22%2C%22title%22%3A%22...%22%7D%5D&channel_id=C0123&initial_comment=...&thread_ts=...
```

パラメータ（`files.d.ts:54-67`）:

| キー | 型 | 備考 |
| --- | --- | --- |
| `files` | `[{ id, title? }, ...]` 1 件以上 | **JSON 文字列にエンコードして** urlencoded の 1 フィールドとして送る（`serializeApiCallData` の非プリミティブ→`JSON.stringify` 規則）|
| `channel_id` | string | 省略するとファイルはどこにも共有されない（private ファイル）|
| `thread_ts` | string | `channel_id` とセットのときだけ付く（`file-upload.js:205-211`）|
| `initial_comment` | string | 任意 |
| `blocks` | array | `slack-cli` は渡さない。`initial_comment` がある場合 Slack 側で無視される |

グルーピング規則（複数ファイル時に重要、`file-upload.js:196`）:

- キーは `` `:::${channel_id}:::${thread_ts}:::${initial_comment}:::${JSON.stringify(blocks)}` ``。
- **この 4 つが完全一致するファイルは 1 回の `completeUploadExternal` にまとめられ、1 通のメッセージに複数添付される**。
- 一致しないものは別呼び出しに分かれる。`Promise.all` で並列に投げる。
- `file_id` が無いエントリがあれば `Missing required file id for file upload completion` を throw。

レスポンス（`FilesCompleteUploadExternalResponse.d.ts`）: `{ ok, files: File[] }`。
`File` は `id` / `name` / `title` / `permalink` / `permalink_public` / `url_private` / `mimetype` / `filetype` / `size` / `created` / `shares` などを持つ。
`slack-cli` が使うのは `id` / `name` / `title` / `permalink` / `permalink_public` / `url_private` の 6 つだけ（`file-operations.ts:18-25`）。

### 1-5. `filename` と `title` の既定値

`file-upload.js:29-64`（`getFileUploadJob`）+ `warnIfMissingOrInvalidFileNameAndDefault`（`file-upload.js:291-306`）。

- `--file` 指定時: `slack-cli` 側で `params.filename = options.filename || basename(options.file)`（`file-operations.ts:104`）。SDK に届く時点で必ず値がある。
- `--content` 指定時: `params.filename = options.filename`。**`--filename` を付けなければ `undefined`** のまま SDK に渡り、SDK が `` `file.${options.filetype ?? 'txt'}` `` を既定にする。`slack-cli` は `--filetype` を `snippet_type` にマップしていて SDK の `filetype` は常に未設定なので、**結果は常に `file.txt`**。
- 拡張子が無いファイル名（`.` を含まない）は警告のみ、処理は続行。
- `title` の既定は `options.title ?? options.filename ?? fileName`（`file-upload.js:46`）。つまり `--title` 未指定ならファイル名がタイトルになる。

### 1-6. `slack-cli` 側のレスポンス解釈

`file-operations.ts:115-137`:

- トップレベル `ok === false` なら `error` を投げる。
- `response.files`（= `completeUploadExternal` の**呼び出しごとの結果配列**）を走査し、各要素の `ok === false` で throw、`entry.files` を平坦化して集める。
- 二重の `files` 入れ子（呼び出し単位の配列 → 各レスポンスの `files`）になっている点に注意。

---

## 2. 複数ファイル・サイズ上限・Content-Type

### 2-1. 複数ファイル

- SDK は `file_uploads: [...]` による複数ファイルをサポートする（`file-upload.js:90-120`）。`channel_id` / `initial_comment` / `thread_ts` / `blocks` は**トップレベルにだけ**書ける。各エントリに書くとエラー（`buildInvalidFilesUploadParamError`）。
- **ただし現行 `slack-cli` の CLI は `--file` / `--content` を各 1 個しか受け付けず、`file_uploads` を使っていない**（`upload.ts:53-71`、`validators.ts:243-251` が `--file` と `--content` の同時指定も禁止）。したがって移植の必須スコープは「1 ファイル」。
- 複数対応を後で足す場合、Step 1 と Step 2 はファイルごとに並列、Step 3 は 1-4 のグルーピング規則で束ねる、という形になる。
- `channels`（旧 API 互換）はカンマ区切り 2 個以上でエラー（`file-upload.js:260-265`）。`slack-cli` は `channel_id` しか使わないので移植不要。

### 2-2. ファイルサイズ

- **コード中にサイズ上限のチェックは存在しない**。SDK も `slack-cli` も一切検査していない。上限は Slack サーバ側の判定に委ねられ、`files.getUploadURLExternal` が `ok:false` を返す形で現れる。
- 推測ではなく事実として書けるのは「クライアント側に上限判定は無い」まで。Slack 公式ドキュメントが示す上限値（1GB 等）は本コードからは確認できないので、実装時は**エラーメッセージをそのまま透過する**方針が安全。
- `getFileDataLength`（`file-upload.js:155-160`）は `Buffer.byteLength(data, 'utf8')`。Buffer に対しては単に `data.length` と同じ。**ファイル全体をメモリに読み込む**（`readFileSync`、`file-upload.js:138`）実装なので、ストリーミングはしていない。Rust でも同じ挙動にするなら `std::fs::read` で足りるが、大きなファイルを扱うなら `reqwest::Body::wrap_stream` に置き換える余地がある（挙動は変わらない。Step 1 で `length` を先に確定させる必要があるので、`metadata().len()` を先に取ればストリーミング可能）。

### 2-3. Content-Type の決定

3 か所で意味が違うので混同しない。

| 場所 | 値 | 決め方 |
| --- | --- | --- |
| `getUploadURLExternal` / `completeUploadExternal` のリクエスト | `application/x-www-form-urlencoded` | 固定（バイナリを含まないため）|
| `upload_url` への POST のリクエスト全体 | `multipart/form-data; boundary=...` | `form-data` が生成 |
| multipart の `body` パートの `Content-Type` | `application/octet-stream` | 常にこれ（1-3 参照）|

**ファイル種別の判定に MIME は一切関与しない。Slack は `filename` の拡張子だけを見る。**
`notion-cli` の `detect_content_type()` に相当する処理は Slack 側では不要（流用するなら別用途）。

---

## 3. ダウンロード側

`file-operations.ts:53-93`。

### 3-1. URL の決定

- `--id` 指定時: `files.info({ file: <id> })` を呼び、`file.url_private_download || file.url_private` を使う。両方無ければ `No download URL found for this file`。ファイル名は `file.name || <fileId>`。
- `--url` 指定時: URL をそのまま使う。ファイル名は `decodeURIComponent(basename(new URL(url).pathname))`。
- 出力パスは `options.outputPath || join(options.outputDir || '.', fileName)`。CLI からは `--output` が `outputPath` に入る（`download.ts:50`）。`outputDir` は CLI から渡されない。

### 3-2. HTTP

```ts
const response = await fetch(url, { headers: { Authorization: `Bearer ${token}` } });
```

- **WebClient / axios を通さず、Node のグローバル `fetch` を直接使う**。したがって WebClient の再試行・レート制限・キュー・`maxRedirects: 0` は**一切適用されない**。
- `response.ok` が false なら `Download failed: ${status} ${statusText}`。
- `response.body` を `stream/promises.pipeline` で `createWriteStream(outputPath)` に流す。**ストリーミング保存**なのでメモリには載せない。
- 保存後に `fs.promises.stat(outputPath).size` を読んでサイズを返す。

### 3-3. リダイレクト追従の実態（移植の要注意点）

- `fetch` の既定は `redirect: "follow"`（undici の既定上限 20 回）。**リダイレクトは自動で追われる**。
- ただし fetch 仕様の HTTP-redirect fetch は、**リダイレクト先のオリジンが変わったとき `Authorization` ヘッダを落とす**。Slack の `url_private` は `files.slack.com` を指し、実体配信で別ホスト（CDN / 署名付き URL）へ 302 する場合がある。その場合 2 ホップ目には Bearer が付かないが、署名付き URL 側は元々トークン不要なので成立している、という構造。
- したがって Rust 側は「リダイレクトを追う」かつ「クロスオリジンで Authorization を落とす」の両方を満たす必要がある。`reqwest` の既定 (`Policy::limited(10)`) はクロスオリジン遷移時に `Authorization` / `Cookie` / `Proxy-Authorization` / `WWW-Authenticate` を除去するので、**既定のままでよい**。`notion-cli` が使っている `Policy::none()` を流用してはいけない。
- 認証エラー時の落とし穴: トークンが無効だと Slack は 401 ではなく **200 でログイン HTML を返す**ことがある。`response.ok` しか見ていない現行実装はこれを検知できず、HTML をファイルとして保存してしまう。移植時に `Content-Type` を確認する改善を入れる余地がある（現行互換を優先するなら入れない）。

---

## 4. `notion-cli` から流用できるコード断片

`notion-cli/src/api/files.rs` は Notion の 3 段階アップロードで、**API 契約は違うが骨格はそのまま使える**。

### 4-1. そのまま流用できるもの

**(a) multipart POST のビルダを毎回作り直す（`files.rs:83-99`）**

multipart のボディは 1 回で消費されるため、リトライ用クロージャの**中で**フォームを組む。この構造は必須。

```rust
self.request_with_retry(|| {
    let file = reqwest::multipart::Part::bytes(data.clone())
        .file_name(filename.clone())
        .mime_str(&content_type)
        .expect("content type was validated before sending");
    let form = reqwest::multipart::Form::new().part("file", file);
    self.http.post(url.clone()).headers(headers.clone()).multipart(form)
})
.await
```

**(b) `mime_str` の事前検証（`files.rs:73-75`）**

クロージャ内では `?` が使えないので、`Part::bytes(Vec::new()).mime_str(ct)` を先に 1 回だけ走らせて妥当性を確認しておく。Slack 側は `application/octet-stream` 固定なのでこの検証自体は不要になるが、パターンとして覚えておく価値がある。

**(c) `Content-Type` ヘッダの手動除去（`files.rs:80-81`）**

```rust
let mut headers = self.notion_headers();
headers.remove(reqwest::header::CONTENT_TYPE);
```

既定ヘッダに `Content-Type: application/json` を入れている場合、`.multipart()` が付ける boundary 付きヘッダと衝突する。Slack 版でも共通ヘッダに `Content-Type` を持たせるなら同じ除去が要る。

**(d) URL 組み立てのオリジン固定（`request.rs:10-29`）**

```rust
pub(crate) fn api_url(&self, path: &str) -> Result<url::Url, CliError> {
    if url::Url::parse(path).is_ok() {
        return Err(CliError::Config("API path must be relative ...".into()));
    }
    let url = self.base_url.join(path)?;
    if url.origin() != self.base_url.origin() {
        return Err(CliError::Config("API path must not change the origin".into()));
    }
    Ok(url)
}
```

`file_id` などサーバ由来の値をパスに埋める箇所で、絶対 URL を注入されてトークンを外部に送らないための防御。テスト `send_never_treats_file_upload_id_as_an_absolute_target`（`files.rs:541-563`）がそのまま参考になる。
**ただし Slack の Step 2 は `upload_url` という「サーバから返された絶対 URL」に POST するので、このガードはそのままは使えない。** 代わりに `upload_url` のホストを検証する（後述 5-4）。

**(e) 429 リトライ（`request.rs:141-205`）**

`retry-after` を読み、指数バックオフ + ジッタ。ただし `slack-cli` は `retries: 0` なので**現行互換を狙うなら使わない**。将来入れるなら流用元として有効。

**(f) 部分読み出しループ（`files.rs:196-222`）**

`File::seek` + `read_exact` で 1 パートずつバッファする。Slack は分割アップロードが無いので不要。

### 4-2. 流用してはいけないもの

- `Policy::none()`（`client/mod.rs:41,59`）: Notion API には必要だが、Slack の**ダウンロードには不適**（3-3 参照）。
- `detect_content_type()` / `block_type_for_content_type()`（`files.rs:239-288`）: Slack のアップロードでは MIME を送らないので出番が無い。ダウンロード時の HTML 検知などに転用する余地はある。
- `SINGLE_PART_LIMIT` / `PART_SIZE` / `MAX_PARTS`: Notion 固有の値。

---

## 5. reqwest での実装（コード断片）

以下は仕様に対応する最小形。エラー型・クライアント構造は `00-rust-skeleton.md` / `common-client.md` の定義に合わせて置き換えること。

### 5-1. Step 1

```rust
#[derive(serde::Deserialize)]
struct GetUploadUrlResponse {
    ok: bool,
    #[serde(default)] error: Option<String>,
    #[serde(default)] upload_url: Option<String>,
    #[serde(default)] file_id: Option<String>,
}

async fn get_upload_url_external(
    &self,
    filename: &str,
    length: u64,
    snippet_type: Option<&str>,
) -> Result<(String, String), CliError> {
    // undefined/null のキーは送らない、という TS 側の規則をそのまま再現する
    let mut form: Vec<(&str, String)> = vec![
        ("filename", filename.to_string()),
        ("length", length.to_string()),
    ];
    if let Some(t) = snippet_type {
        form.push(("snippet_type", t.to_string()));
    }

    let res: GetUploadUrlResponse = self
        .http
        .post(self.api_url("files.getUploadURLExternal")?)
        .bearer_auth(&self.token)
        .form(&form)          // Content-Type: application/x-www-form-urlencoded
        .send()
        .await?
        .json()
        .await?;

    if !res.ok {
        return Err(CliError::SlackApi(res.error.unwrap_or_else(|| "unknown".into())));
    }
    Ok((
        res.upload_url.ok_or_else(|| CliError::SlackApi("missing upload_url".into()))?,
        res.file_id.ok_or_else(|| CliError::SlackApi("missing file_id".into()))?,
    ))
}
```

`.form()` は `serde_urlencoded` を使うので、`Vec<(&str, String)>` でそのまま通る。

### 5-2. Step 2

TS 実装と**バイト単位で同じ**リクエストを作るなら、フィールド名 `body`、ファイル名 `Untitled`、MIME `application/octet-stream` の 3 点を固定する。

```rust
const UPLOAD_FIELD_NAME: &str = "body";
const UPLOAD_PART_FILENAME: &str = "Untitled";

async fn post_to_upload_url(&self, upload_url: &str, data: Vec<u8>) -> Result<(), CliError> {
    let part = reqwest::multipart::Part::bytes(data)
        .file_name(UPLOAD_PART_FILENAME)
        .mime_str("application/octet-stream")
        .expect("static mime is always valid");
    let form = reqwest::multipart::Form::new().part(UPLOAD_FIELD_NAME, part);

    let res = self
        .http
        .post(upload_url)
        .bearer_auth(&self.token)   // TS 版も axios 既定ヘッダ経由で Bearer を送っている
        .multipart(form)
        .send()
        .await?;

    // TS 版は maxRedirects: 0 + status !== 200 で失敗。3xx も失敗扱いにする
    if res.status() != reqwest::StatusCode::OK {
        return Err(CliError::Upload(format!(
            "Failed to upload file (status: {})",
            res.status()
        )));
    }
    Ok(()) // レスポンスボディは TS 版も使っていない
}
```

`maxRedirects: 0` を厳密に再現したい場合は、この POST 専用に `Policy::none()` のクライアントを持たせる。
ダウンロード用クライアントとは分けること（3-3）。

ストリーミングにする場合（TS 版と挙動は同じだがメモリを食わない）:

```rust
let len = tokio::fs::metadata(path).await?.len();          // Step 1 の length に使う
let file = tokio::fs::File::open(path).await?;
let body = reqwest::Body::wrap_stream(tokio_util::io::ReaderStream::new(file));
let part = reqwest::multipart::Part::stream_with_length(body, len)
    .file_name(UPLOAD_PART_FILENAME)
    .mime_str("application/octet-stream")?;
```

`stream_with_length` を使わないと `Content-Length` が付かず chunked になるため、`stream_with_length` を選ぶ。

### 5-3. Step 3

```rust
#[derive(serde::Serialize)]
struct FileEntry<'a> {
    id: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<&'a str>,
}

async fn complete_upload_external(
    &self,
    files: &[FileEntry<'_>],
    channel_id: Option<&str>,
    thread_ts: Option<&str>,
    initial_comment: Option<&str>,
) -> Result<Vec<SlackFile>, CliError> {
    // files は「JSON 文字列を 1 フィールドに載せる」形。ここが最も間違えやすい
    let files_json = serde_json::to_string(files)?;
    let mut form: Vec<(&str, String)> = vec![("files", files_json)];
    if let Some(c) = channel_id { form.push(("channel_id", c.to_string())); }
    // thread_ts は channel_id とセットのときだけ（file-upload.js:205）
    if let (Some(_), Some(t)) = (channel_id, thread_ts) {
        form.push(("thread_ts", t.to_string()));
    }
    if let Some(m) = initial_comment { form.push(("initial_comment", m.to_string())); }

    let res: CompleteUploadResponse = self
        .http
        .post(self.api_url("files.completeUploadExternal")?)
        .bearer_auth(&self.token)
        .form(&form)
        .send()
        .await?
        .json()
        .await?;

    if !res.ok {
        return Err(CliError::SlackApi(res.error.unwrap_or_else(|| "unknown".into())));
    }
    Ok(res.files.unwrap_or_default())
}
```

### 5-4. 3 段を束ねる

```rust
pub async fn upload_file(&self, opts: UploadFileOptions) -> Result<Vec<SlackFile>, CliError> {
    let channel_id = self.resolve_channel_id(&opts.channel).await?;

    let (data, filename) = match (&opts.file_path, &opts.content) {
        (Some(p), None) => {
            let data = tokio::fs::read(p).await?;
            let name = opts.filename.clone().unwrap_or_else(|| {
                Path::new(p).file_name().unwrap_or_default().to_string_lossy().into_owned()
            });
            (data, name)
        }
        (None, Some(c)) => (
            c.clone().into_bytes(),
            // --content かつ --filename 未指定なら常に "file.txt"（1-5 参照）
            opts.filename.clone().unwrap_or_else(|| "file.txt".to_string()),
        ),
        _ => return Err(CliError::Validation("You must specify either --file or --content".into())),
    };

    let title = opts.title.clone().unwrap_or_else(|| filename.clone()); // 既定はファイル名

    let (upload_url, file_id) = self
        .get_upload_url_external(&filename, data.len() as u64, opts.snippet_type.as_deref())
        .await?;

    // upload_url はサーバ由来の絶対 URL。Bearer を載せる前にホストを確認する
    let parsed = url::Url::parse(&upload_url)
        .map_err(|e| CliError::SlackApi(format!("invalid upload_url: {e}")))?;
    if parsed.scheme() != "https" || !matches!(parsed.host_str(), Some(h) if h.ends_with(".slack.com")) {
        return Err(CliError::SlackApi("upload_url points outside slack.com".into()));
    }

    self.post_to_upload_url(&upload_url, data).await?;

    self.complete_upload_external(
        &[FileEntry { id: &file_id, title: Some(&title) }],
        Some(&channel_id),
        opts.thread_ts.as_deref(),
        opts.initial_comment.as_deref(),
    )
    .await
}
```

`upload_url` のホスト検証は TS 版には無い**追加の防御**。`notion-cli` の `api_url` ガードと同じ発想で、Bearer を Slack 以外へ送らないようにする。現行互換を厳密に守るなら外してもよいが、外す判断は明示的に行うこと。

### 5-5. ダウンロード

```rust
pub fn download_http_client() -> Result<reqwest::Client, CliError> {
    Ok(reqwest::Client::builder()
        .user_agent(concat!("slack-cli-rs/", env!("CARGO_PKG_VERSION")))
        // 既定の Policy::limited(10)。クロスオリジン遷移で Authorization を落としてくれる（3-3）
        .build()?)
}

pub async fn download_file(&self, opts: DownloadFileOptions) -> Result<DownloadResult, CliError> {
    let (url, file_name) = match (&opts.file_id, &opts.url) {
        (Some(id), None) => {
            let info = self.files_info(id).await?;
            let url = info.url_private_download
                .or(info.url_private)
                .ok_or_else(|| CliError::Slack("No download URL found for this file".into()))?;
            (url, info.name.unwrap_or_else(|| id.clone()))
        }
        (None, Some(u)) => {
            let parsed = url::Url::parse(u)?;
            let base = parsed.path_segments().and_then(|s| s.last()).unwrap_or("");
            let name = percent_encoding::percent_decode_str(base).decode_utf8_lossy().into_owned();
            (u.clone(), name)
        }
        _ => return Err(CliError::Validation("You must specify either --url or --id".into())),
    };

    let out_path = opts.output_path.clone()
        .unwrap_or_else(|| PathBuf::from(opts.output_dir.as_deref().unwrap_or(".")).join(&file_name));

    let res = self.download_http.get(&url).bearer_auth(&self.token).send().await?;
    if !res.status().is_success() {
        return Err(CliError::Download(format!(
            "Download failed: {} {}",
            res.status().as_u16(),
            res.status().canonical_reason().unwrap_or("")
        )));
    }

    // TS 版と同じくストリーミングで書き出す
    let mut file = tokio::fs::File::create(&out_path).await?;
    let mut stream = res.bytes_stream();
    let mut size: u64 = 0;
    while let Some(chunk) = futures_util::StreamExt::next(&mut stream).await {
        let chunk = chunk?;
        size += chunk.len() as u64;
        tokio::io::AsyncWriteExt::write_all(&mut file, &chunk).await?;
    }
    tokio::io::AsyncWriteExt::flush(&mut file).await?;

    Ok(DownloadResult { file_path: out_path, file_name, size })
}
```

TS 版はサイズを保存後の `stat()` で取っているので厳密互換ならそちらでもよい。書き込みバイト数の積算でも同値になる（推測ではなく、追記でなく新規作成のため）。

---

## 6. 依存クレート

| 用途 | クレート |
| --- | --- |
| HTTP | `reqwest`（feature: `json`, `multipart`, `stream`）|
| フォーム直列化 | `reqwest` の `.form()`（内部で `serde_urlencoded`）|
| ストリーミング本文 | `tokio-util`（`ReaderStream`）, `futures-util`（`StreamExt`）|
| URL 解析 | `url` |
| パーセントデコード | `percent-encoding` |
| テスト | `wiremock`（`notion-cli` の `files.rs` テストがそのまま雛形になる）|

---

## 7. 移植チェックリスト

- [ ] Step 2 のフィールド名が `body`、ファイル名が `Untitled`、パート MIME が `application/octet-stream` になっている
- [ ] Step 3 の `files` が JSON 文字列として urlencoded の 1 フィールドに載っている
- [ ] `thread_ts` は `channel_id` があるときだけ送っている
- [ ] `--content` + `--filename` 未指定で `file.txt` になる
- [ ] `--title` 未指定でタイトルがファイル名になる
- [ ] `snippet_type` が Step 1 に、`title` が Step 3 に行っている（逆にしない）
- [ ] アップロード側は redirect を追わない / ダウンロード側は追う、とクライアントを分けている
- [ ] ダウンロードのリダイレクト時に Authorization がクロスオリジンへ漏れない
- [ ] 429 の扱いが `retries: 0`（現行互換）になっている
