# ファイル系コマンド仕様（upload / download / canvas / bookmark）

Rust 移植用の仕様書。参照元は TypeScript 実装（`/Users/mimo/organizations/open-source/slack-cli`）の以下のファイル。

- `src/index.ts`
- `src/commands/upload.ts` / `download.ts` / `canvas.ts` / `bookmark.ts`
- `src/utils/command-support.ts` / `command-wrapper.ts` / `option-parsers.ts` / `validators.ts` / `errors.ts` / `error-utils.ts` / `client-factory.ts` / `config-helper.ts` / `constants.ts` / `terminal-sanitizer.ts` / `channel-resolver.ts`
- `src/utils/slack-operations/base-client.ts` / `file-operations.ts` / `canvas-operations.ts` / `star-operations.ts` / `channel-operations.ts`
- `src/utils/formatters/bookmark-formatters.ts` / `base-formatter.ts`
- `src/types/commands.ts` / `src/types/slack.ts`

読んでいない範囲（`profile-config.ts`、`token-crypto-service.ts`、`update-notifier.ts` の内部実装など）は「不明」と明記する。

---

## 0. 全体像（index.ts）

- バイナリ名（commander の `name`）: `slack-cli`
- 説明: `CLI tool to send messages via Slack API`
- バージョン: `package.json` の `version` を実行時に読み込んで `--version` に設定（`__dirname/../package.json`）
- 登録コマンド（登録順）: `config`, `send`, `channels`, `history`, `unread`, `scheduled`, `search`, `edit`, `delete`, `upload`, `download`, `reaction`, `pin`, `users`, `usergroups`, `channel`, `members`, `send-ephemeral`, `join`, `leave`, `invite`, `reminder`, `bookmark`, `canvas`, `draft`
- グローバル `postAction` フック: 全コマンド実行後に `checkForUpdates({ packageName, currentVersion })` を呼ぶ（更新通知。内部実装は本調査の対象外＝不明）
- エイリアスは本仕様の対象4コマンドには **一切定義されていない**（`.alias()` 呼び出しなし）
- 位置引数は本仕様の対象4コマンドには **一切存在しない**（すべてオプションフラグ）

### 共通の下地

| 要素 | 挙動 |
| --- | --- |
| `wrapCommand` | action を try/catch でくるむ。例外時 `console.error(chalk.red('✗ Error:'), <整形済みメッセージ>)` を出し `process.exit(1)`。`NODE_ENV === 'development'` かつ `Error` インスタンスなら stack をグレーで追加出力 |
| エラーメッセージ整形 | `extractErrorMessage(error)` → `sanitizeTerminalText` → `redactSlackTokens` の順（サニタイズを先に行うのはエスケープ挿入によるトークン分断対策とコメントに明記） |
| `extractErrorMessage` | `Error` なら `error.message`。Slack エラーコードが `missing_scope` かつ `data.needed` があれば `` `${message} (needed: scope1, scope2)` ``。`Error` でなければ `String(error)` |
| `createValidationHook` | commander の `preAction` フック。バリデータを順に評価し、最初に文字列を返した時点で `thisCommand.error(\`Error: ${msg}\`)` を呼んで中断（commander の `error()` は既定で終了コード **1**、`stderr` 出力） |
| `renderByFormat` | `format === 'json'` なら json レンダラ、無ければ `console.log(JSON.stringify(sanitizeTerminalData(data), null, 2))`。`simple` かつ simple レンダラありなら simple。それ以外は table |
| `parseFormat` | `format || 'table'` |
| クライアント生成 | `createSlackClient(profile)` → `getConfigOrThrow(profile)` → `ProfileConfigManager.getConfig(profile)`。設定が無ければ `ConfigurationError`（メッセージは下記）。トークンで `WebClient` を生成、`retryConfig.retries = 0`（自動リトライ無効）、`logLevel = ERROR` |
| 設定なしエラー文言 | `No configuration found for profile "<name>". Use "slack-cli config set --token <token> --profile <name>" to set up.`（`<name>` は指定プロファイル、無ければデフォルトプロファイル名、無ければ `default`） |
| 端末サニタイズ | `sanitizeTerminalText`: OSC/ANSI シーケンス除去 → 制御文字（<0x20、0x7f、0x80-0x9f）を除去（`\t` `\n` のみ残す）。`sanitizeSingleLineText`: さらに空白連続を単一スペースに畳んで trim。`sanitizeTerminalData`: 文字列・配列・プレーンオブジェクトを再帰的にサニタイズ |
| 並行制御 | `BaseSlackClient` が `pLimit(RATE_LIMIT.CONCURRENT_REQUESTS = 3)` の `rateLimiter` を持つが、**本仕様の4コマンドが通る経路では `rateLimiter` は使われていない**（`file-operations` / `canvas-operations` / `star-operations` はいずれも `this.client` を直接呼ぶ） |
| レート制限 | `handleRateLimit(error)` は「message に `rate limit` を含む場合 5 秒待つ」だけ。これも本仕様の4コマンド経路では呼ばれない（`ChannelOperations` の unread 系・`fetchChannelInfo` のみ使用） |

### チャンネル解決（upload / canvas list が使う）

`ChannelOperations.resolveChannelId(nameOrId)`:

1. `/^[CDG][A-Z0-9]{8,}$/` にマッチすればそのまま ID として返す（API 呼び出しなし）
2. マッチしなければチャンネル一覧を取得してキャッシュ（プロセス内 1 回、失敗時はキャッシュを破棄して再試行可能）
   - `conversations.list` を `types = "public_channel,private_channel,im,mpim"`, `exclude_archived = true`, `limit = 1000` で **`next_cursor` が空になるまでループ**（完全ページネーション）
   - `missing_scope` エラー時は `needed` スコープ（`channels:read`→public、`groups:read`→private、`im:read`→im、`mpim:read`→mpim）に対応する type を除いて 1 回だけリトライ。除外後 0 件、または全く減らなかった場合は元のエラーを再送出
3. 名前一致は「完全一致 → `#` を除いた一致 → 小文字化一致 → `name_normalized` 一致」の順で判定
4. 見つからない場合 `ApiError`:
   - 部分一致候補（`name.toLowerCase().includes(入力.toLowerCase())`、最大5件）があるとき: `Channel '<name>' not found. Did you mean one of these? a, b, c`
   - 候補なし: `Channel '<name>' not found. Make sure you are a member of this channel.`

---

## 1. `slack-cli upload`

サブコマンドなし。説明: `Upload a file or snippet to a Slack channel`。

### 位置引数

なし。

### オプション

| ロング | ショート | 型 | 既定値 | 必須 | 相互排他・条件 |
| --- | --- | --- | --- | --- | --- |
| `--channel <channel>` | `-c` | string | なし | **必須**（commander の `requiredOption`） | チャンネル名 or ID |
| `--file <file>` | `-f` | string | なし | 条件付き | `--content` と排他。どちらか一方が必須 |
| `--content <content>` | なし | string | なし | 条件付き | `--file` と排他。どちらか一方が必須 |
| `--filename <filename>` | なし | string | なし | 任意 | ファイル名の上書き |
| `--title <title>` | なし | string | なし | 任意 | |
| `--message <message>` | `-m` | string | なし | 任意 | `initial_comment` に渡る |
| `--filetype <filetype>` | なし | string | なし | 任意 | `snippet_type` に渡る（例: python, javascript, csv） |
| `--thread <thread>` | `-t` | string | なし | 任意 | `^\d{10}\.\d{6}$` に一致必須 |
| `--format <format>` | なし | string | `table` | 任意 | `table` / `simple` / `json` のみ |
| `--profile <profile>` | なし | string | なし | 任意 | |

### バリデーション（preAction、順序どおり・最初の1件で終了）

1. `fileOrContent`: 両方未指定 → `You must specify either --file or --content` / 両方指定 → `Cannot use both --file and --content`
2. `uploadThreadTimestamp`: `--thread` があり `^\d{10}\.\d{6}$` 不一致 → `Invalid thread timestamp format`
3. `format`: `--format` が `table|simple|json` 以外 → `` Invalid format '<値>'. Must be one of: table, simple, json ``

いずれも実際の出力は `Error: <上記文言>`（commander の `error()` 経由、終了コード 1）。

### action の処理順

1. `--file` 指定時、`fs.access(path)` で存在確認。失敗したら `FileError`（`code = 'FILE_ERROR'`）、メッセージ `File not found: <path>`
2. プロファイルから `SlackApiClient` を生成
3. `channel` を `resolveChannelId` で ID 化（名前指定なら `conversations.list` を全ページ取得）
4. `files.uploadV2` を呼ぶ

### Slack Web API 呼び出し

| 呼び出し | パラメータ |
| --- | --- |
| `conversations.list`（チャンネル名指定時のみ、ページネーション） | `types`, `exclude_archived=true`, `limit=1000`, `cursor` |
| `files.uploadV2` | `channel_id`（解決済み ID）／ファイル指定時 `file=<パス>`・`filename=<--filename または basename(パス)>`／content 指定時 `content=<文字列>`・`filename=<--filename の値そのまま。未指定なら undefined>`／任意で `title`, `initial_comment`, `snippet_type`, `thread_ts` |

`files.uploadV2` は Slack SDK 側のヘルパで、実際には `files.getUploadURLExternal` → HTTP PUT → `files.completeUploadExternal` の複合処理（SDK 内部。詳細な内訳は本調査では未確認＝不明）。

レスポンス処理:
- `response.ok === false` → `Error(response.error ?? 'files.uploadV2 failed')`
- `response.files[]` の各エントリで `ok === false` → `Error(entry.error ?? 'completeUploadExternal failed')`
- 各エントリの `files` を平坦化して収集（`{ id, name, title, permalink, permalink_public, url_private }`）

### 標準出力

出力データ構造: `{ channel: <--channel に渡された生の値>, files: UploadedFileInfo[] }`

**table（既定）**:

```
✓ File uploaded successfully to #general
  file_id: F0123456789
  permalink: https://example.slack.com/files/U01/F0123456789/report.csv
```

- 1行目は緑（chalk.green）。`#` は常に前置されるため、`-c C0123456789` のように ID を渡すと `#C0123456789` と表示される
- `file_id` は `f.id` がある行のみ、`permalink` は `f.permalink` がある行のみ出力。ファイルが複数あればその数だけ繰り返す

**simple**: simple レンダラが未定義なので **table と同一出力**（`renderByFormat` の仕様）。

**json**: `JSON.stringify(sanitizeTerminalData(output), null, 2)`。

```json
{
  "channel": "general",
  "files": [
    {
      "id": "F0123456789",
      "name": "report.csv",
      "title": "report.csv",
      "permalink": "https://example.slack.com/files/U01/F0123456789/report.csv"
    }
  ]
}
```

（`files` の各キーは Slack のレスポンス由来なので実際に含まれるキー集合は API 依存。TS 側の型は上記6キーを想定）

### エラーケース

| ケース | 文言 | 終了コード |
| --- | --- | --- |
| `--channel` 未指定 | commander 標準の `error: required option '-c, --channel <channel>' not specified` | 1 |
| `--file`/`--content` の指定不正 | `Error: You must specify either --file or --content` / `Error: Cannot use both --file and --content` | 1 |
| thread 形式不正 | `Error: Invalid thread timestamp format` | 1 |
| format 不正 | `Error: Invalid format '<値>'. Must be one of: table, simple, json` | 1 |
| ファイル不存在 | `✗ Error: File not found: <path>` | 1 |
| プロファイル設定なし | `✗ Error: No configuration found for profile "<name>". Use "slack-cli config set --token <token> --profile <name>" to set up.` | 1 |
| チャンネル解決失敗 | `✗ Error: Channel '<name>' not found. ...`（上述2種） | 1 |
| API 失敗 | `✗ Error: <Slack のエラー文字列>`（`missing_scope` の場合は ` (needed: files:write, ...)` が付く） | 1 |

### ページネーション・レート制限・並行

- ページネーションはチャンネル解決の `conversations.list` のみ（全ページ取得、上限なし）
- アップロード自体はリトライなし（`WebClient` の `retries: 0`）、レート制限のバックオフもなし
- 並行実行なし（逐次）

---

## 2. `slack-cli download`

サブコマンドなし。説明: `Download a file from Slack`。

### 位置引数

なし。

### オプション

| ロング | ショート | 型 | 既定値 | 必須 | 相互排他 |
| --- | --- | --- | --- | --- | --- |
| `--url <url>` | `-u` | string | なし | 条件付き | `--id` と排他。どちらか一方が必須 |
| `--id <id>` | `-i` | string | なし | 条件付き | `--url` と排他。どちらか一方が必須 |
| `--output <path>` | `-o` | string | なし（未指定時はカレントディレクトリ＋元ファイル名） | 任意 | |
| `--format <format>` | なし | string | `table` | 任意 | ヘルプ上は table/simple/json だが **format バリデータが付いていない**（未知の値は table 扱いにフォールバック） |
| `--profile <profile>` | なし | string | なし | 任意 | |

### バリデーション

このコマンドだけ共通バリデータではなくインラインの無名関数を1本だけ使う。

1. 両方未指定 → `You must specify either --url or --id`
2. 両方指定 → `Cannot use both --url and --id`

出力は `Error: <文言>`、終了コード 1。

### Slack Web API 呼び出し

| 条件 | 呼び出し | パラメータ |
| --- | --- | --- |
| `--id` 指定時 | `files.info` | `file = <id>` |
| いずれも（実ダウンロード） | Web API ではなく素の HTTP GET | URL に対し `Authorization: Bearer <token>` ヘッダ |

処理詳細:

- `--id`: `files.info` の結果から `file.url_private_download || file.url_private` を URL とし、`file.name || <id>` をファイル名とする。URL が空なら `Error('No download URL found for this file')`
- `--url`: URL をそのまま使い、ファイル名は `decodeURIComponent(basename(new URL(url).pathname))`
- 保存先: `options.outputPath`（= `--output`）があればそれ、なければ `join('.', fileName)`
- トークンが無い場合 `Error('No token available')`
- `response.ok` が偽 → `Error(\`Download failed: ${status} ${statusText}\`)`
- `response.body` が無い → `Error('No response body')`
- ボディをストリームで `createWriteStream(outputPath)` へ pipeline
- 書き込み後 `fs.promises.stat(outputPath)` でサイズ取得

### 標準出力

出力データ: `{ filePath, fileName, size }`

**table（既定）**:

```
✓ Downloaded: report.csv
  path: ./report.csv
  size: 12.3 KB
```

サイズ整形（`formatFileSize`）:
- `< 1024` → `<bytes> B`（整数）
- `< 1048576` → `(bytes/1024).toFixed(1) KB`
- それ以上 → `(bytes/1048576).toFixed(1) MB`（GB 以上の単位は無し）

**simple**: `filePath` のみを1行出力。

```
./report.csv
```

**json**:

```json
{
  "filePath": "./report.csv",
  "fileName": "report.csv",
  "size": 12600
}
```

### エラーケース

| ケース | 文言 | 終了コード |
| --- | --- | --- |
| `--url`/`--id` 両方未指定 | `Error: You must specify either --url or --id` | 1 |
| 両方指定 | `Error: Cannot use both --url and --id` | 1 |
| ダウンロード URL 無し | `✗ Error: No download URL found for this file` | 1 |
| トークン無し | `✗ Error: No token available` | 1 |
| HTTP 非 2xx | `✗ Error: Download failed: 404 Not Found` | 1 |
| ボディ無し | `✗ Error: No response body` | 1 |
| `--url` が URL としてパース不能 | Node の `new URL()` が投げる `TypeError`（例: `Invalid URL`）がそのまま `✗ Error: Invalid URL` として出る | 1 |
| 書き込み失敗（権限・存在しないディレクトリ等） | Node の fs エラーメッセージがそのまま | 1 |

### ページネーション・レート制限・並行

いずれも無し。単発の `files.info` + 単発 HTTP GET。リトライなし。

---

## 3. `slack-cli canvas`

親コマンド `canvas`（説明: `Manage Slack Canvases`）にサブコマンド `read` / `list`。親コマンド自体に action は無い（引数なしで呼ぶと commander のヘルプ挙動）。

### 3-1. `canvas read`

説明: `Get the sections of a Canvas`

| ロング | ショート | 型 | 既定値 | 必須 |
| --- | --- | --- | --- | --- |
| `--id <canvas-id>` | `-i` | string | なし | **必須** |
| `--format <format>` | なし | string | `table` | 任意（`table|simple|json` バリデータあり） |
| `--profile <profile>` | なし | string | なし | 任意 |

API: `canvases.sections.lookup`
- パラメータ: `canvas_id = <--id>`, `criteria = { section_types: ['any_header'] }`
- **`any_header` 固定**なので、ヘッダを持つセクションのみが対象。カーソル/ページネーションは指定していない

出力: `response.sections || []`（`{ id?, elements? }` の配列）。テキストは `elements` を再帰的にたどり、`el.text` があれば `sanitizeTerminalText(el.text)`、無ければ子 `elements` を再帰、どちらも無ければ空文字。すべて **区切りなしで連結**（`join('')`）。

0 件のとき（format に関係なく、json でも）:

```
No sections found in canvas
```

**table**（`ID:` 部分はシアン）:

```
ID: temp:C:abc123  Content: プロジェクト概要
ID: (no id)  Content: (no content)
```

**simple**（タブ区切り）:

```
temp:C:abc123	プロジェクト概要
(no id)	(no content)
```

**json**: セクション配列をそのまま `JSON.stringify(sanitizeTerminalData(sections), null, 2)`。

```json
[
  {
    "id": "temp:C:abc123",
    "elements": [
      { "type": "text", "text": "プロジェクト概要" }
    ]
  }
]
```

### 3-2. `canvas list`

説明: `List canvases linked to a channel`

| ロング | ショート | 型 | 既定値 | 必須 |
| --- | --- | --- | --- | --- |
| `--channel <channel>` | `-c` | string | なし | **必須** |
| `--format <format>` | なし | string | `table` | 任意（バリデータあり） |
| `--profile <profile>` | なし | string | なし | 任意 |

API:
1. チャンネル名なら `conversations.list`（全ページ）で ID 解決
2. `files.list` を `channel = <解決済み ID>`, `types = 'spaces'` で呼ぶ（**`limit` も `page`/`cursor` も指定していない＝Slack 既定の 1 ページのみ**）

0 件のとき:

```
No canvases found in channel
```

**table**（`ID:` 部分はシアン）:

```
ID: F0123ABCD  Name: 週次メモ
```

**simple**:

```
F0123ABCD	週次メモ
```

`id` / `name` が無い場合はそれぞれ `(no id)` / `(no name)`。

**json**: `files.list` が返した file オブジェクトの配列をそのままサニタイズして出力（TS の型は `{ id?, name?, created?, filetype? }` だが、実レスポンスのキーはすべて含まれる）。

### canvas のエラーケース

| ケース | 文言 | 終了コード |
| --- | --- | --- |
| `--id` / `--channel` 未指定 | commander 標準 `error: required option ... not specified` | 1 |
| format 不正 | `Error: Invalid format '<値>'. Must be one of: table, simple, json` | 1 |
| プロファイル設定なし | `✗ Error: No configuration found for profile ...` | 1 |
| チャンネル解決失敗（list のみ） | `✗ Error: Channel '<name>' not found. ...` | 1 |
| Slack API エラー | `✗ Error: <Slack エラー文字列>`（`missing_scope` なら needed 付き） | 1 |

ページネーション・レート制限・並行: チャンネル解決以外にページネーションなし。並行実行なし。リトライなし。

---

## 4. `slack-cli bookmark`

親コマンド `bookmark`（説明: `Manage saved items (save for later)`）。実体は Slack の **stars API**（save for later）であって、`bookmarks.*` API ではない点に注意。サブコマンドは `add` / `list` / `remove`。

### 4-1. `bookmark add`

説明: `Save a message for later`

| ロング | ショート | 型 | 既定値 | 必須 |
| --- | --- | --- | --- | --- |
| `--channel <channel>` | `-c` | string | なし | **必須** |
| `--ts <timestamp>` | なし | string | なし | **必須** |
| `--profile <profile>` | なし | string | なし | 任意 |

- **バリデーションフックなし**（ts の形式チェックも無い）。ヘルプ文言は「Channel ID」だが、`resolveChannelId` を通さず **生の値をそのまま API に渡す**（＝チャンネル名は解決されない）
- API: `stars.add` に `channel`, `timestamp`
- 出力（format オプション無し、常にこの形・緑）:

```
✓ Saved message 1712345678.123456 in C0123456789
```

- 出力値は API レスポンスではなく **入力値をそのまま**（サニタイズもされていない）

### 4-2. `bookmark list`

説明: `List saved items`

| ロング | ショート | 型 | 既定値 | 必須 |
| --- | --- | --- | --- | --- |
| `--limit <limit>` | なし | string（内部で `parseInt(値, 10)`） | `'100'` | 任意 |
| `--format <format>` | なし | string | `table` | 任意（バリデータあり） |
| `--profile <profile>` | なし | string | なし | 任意 |

- `parseLimit`: `parseInt(limit || '100', 10)`。**NaN のガードなし**（`--limit abc` は `NaN` が `count` としてそのまま API に渡る）
- API: `stars.list` に `count = <limit>`, `cursor = undefined`（`listStars(count, cursor?)` の cursor はサービス層から渡されず常に未指定）。レスポンスの `items` のみ使用し、`response_metadata` は **無視＝ページネーションなし（1ページのみ）**
- 0 件のとき（format に関係なく）: `No saved items found`
- フォーマッタ選択は `FormatterFactory.create(format)`。未知の format は table にフォールバック（ただし preAction バリデータで先に弾かれる）

**table**（固定幅・パディング。ヘッダ行 + `─` の罫線）:

列幅: Channel=16, Timestamp=20, Text=40（本文は 38 文字で切り詰め）, Saved At=26

```
Channel         Timestamp           Text                                    Saved At
────────────────────────────────────────────────────────────────────────────────────────────────────
C0123456789     1712345678.123456   お疲れさまです。明日の件ですが          2026-04-05T12:34:38.000Z
```

- 各セルは `sanitizeSingleLineText`（改行・タブを空白へ畳む）
- `Saved At` は `new Date(date_create * 1000).toISOString()`
- 値が列幅を超えた場合、`padEnd` は切り詰めないので列がずれる（Text 列のみ 38 文字で `slice`）

**simple**（タブ区切り、ヘッダなし）:

```
C0123456789	1712345678.123456	お疲れさまです。明日の件ですが	2026-04-05T12:34:38.000Z
```

Text はここでは切り詰めない。

**json**（キーを詰め替えた配列。`sanitizeTerminalData` 適用済み）:

```json
[
  {
    "type": "message",
    "channel": "C0123456789",
    "timestamp": "1712345678.123456",
    "text": "お疲れさまです。明日の件ですが",
    "date_create": 1712345678,
    "saved_at": "2026-04-05T12:34:38.000Z"
  }
]
```

### 4-3. `bookmark remove`

説明: `Remove a saved item`

| ロング | ショート | 型 | 既定値 | 必須 |
| --- | --- | --- | --- | --- |
| `--channel <channel>` | `-c` | string | なし | **必須** |
| `--ts <timestamp>` | なし | string | なし | **必須** |
| `--profile <profile>` | なし | string | なし | 任意 |

- バリデーションフックなし。`resolveChannelId` も通さない
- API: `stars.remove` に `channel`, `timestamp`
- 出力（緑・固定）:

```
✓ Removed saved item 1712345678.123456 from C0123456789
```

### bookmark のエラーケース

| ケース | 文言 | 終了コード |
| --- | --- | --- |
| `--channel` / `--ts` 未指定 | commander 標準 `error: required option ... not specified` | 1 |
| format 不正（list のみ） | `Error: Invalid format '<値>'. Must be one of: table, simple, json` | 1 |
| プロファイル設定なし | `✗ Error: No configuration found for profile ...` | 1 |
| Slack API エラー（`message_not_found`, `already_starred`, `not_starred`, `missing_scope` 等） | `✗ Error: <Slack エラー文字列>` | 1 |

ページネーション・レート制限・並行: いずれも無し。`stars.list` の cursor は未実装。

---

## 5. コマンド数まとめ

- トップレベル: 4（`upload`, `download`, `canvas`, `bookmark`）
- サブコマンド込みの実行可能コマンド: 7（`upload`, `download`, `canvas read`, `canvas list`, `bookmark add`, `bookmark list`, `bookmark remove`）
- エイリアス: 0

---

## 6. Rust 移植で引っかかりそうな点

### 6-1. `files.uploadV2` は SDK 固有のヘルパ

Slack の Web API に `files.uploadV2` というメソッドは存在せず、Node SDK が `files.getUploadURLExternal` → 外部 URL への PUT → `files.completeUploadExternal` をまとめたもの。Rust では自前で3段構成を実装する必要がある。TS 側のレスポンス処理も `{ ok, files: [{ ok, error, files: [...] }] }` という **入れ子構造**を前提にしており、これは `completeUploadExternal` の結果を SDK が包み直した形。Rust の戻り値型を素の API に合わせるなら、出力 JSON の互換をどう取るか決めが必要。

### 6-2. `--filename` の非対称な既定値

`--file` 経路では `filename = options.filename || basename(filePath)` とフォールバックがあるが、`--content` 経路では `filename = options.filename`（未指定なら `undefined` のまま送信）。Rust で `Option<String>` を素直に扱うとこの非対称さを落としやすい。API に `filename` キー自体を送らないのと空文字を送るのは意味が違う。

### 6-3. `simple` フォーマットが実装されていない箇所がある

`upload` は simple レンダラを持たないため `--format simple` が table と同じ出力になる。Rust で「format ごとに分岐」を素直に書くと、ここで挙動が変わってしまう。`renderByFormat` のフォールバック規則（json レンダラ無し→汎用 JSON、simple レンダラ無し→table）をそのまま移植する必要がある。

### 6-4. `download` だけ format バリデータが無い

`upload` / `canvas` / `bookmark list` は `optionValidators.format` を持つが、`download` は url/id の排他チェックだけ。`--format xxx` は弾かれず table にフォールバックする。clap の `value_parser`（enum）でまとめて縛ると、この差異が消えて挙動が変わる。

### 6-5. 出力の空チェックが format を無視する

`canvas read` / `canvas list` / `bookmark list` の 0 件時は、`--format json` を指定していても `No sections found in canvas` のような **人間向けプレーンテキスト**を出す（空配列の JSON ではない）。JSON をパースする側から見ると壊れた出力だが、互換のためには再現が必要。ここを直すなら意図的な仕様変更として扱う。

### 6-6. ターミナルサニタイズの文字単位ループ

`sanitizeTerminalText` は JS の `for...of`（コードポイント単位）で走査し、`charCodeAt(0)` を見て判定している。サロゲートペア（絵文字など）は 1 コードポイントとして扱われ、先頭の上位サロゲート値（0xD800台）は制御文字判定に引っかからないので残る。Rust の `chars()` はスカラ値単位なので概ね一致するが、C1 制御（U+0080〜U+009F）の除去まで含めて同じ判定順序で実装しないと出力差が出る。JSON 出力も `sanitizeTerminalData` で **値を書き換えてから** シリアライズしている点に注意（生データではない）。

### 6-7. チャンネル解決キャッシュとスコープフォールバック

`resolveChannelId` は「プロセス内で1回だけ全チャンネルを取得」する遅延キャッシュ（`Promise` をキャッシュし、失敗時は捨てる）。Rust では `OnceCell` / `tokio::sync::OnceCell` 相当が必要だが、「失敗したらキャッシュを破棄して再試行可能にする」挙動は `OnceCell` では表現できない（`Mutex<Option<...>>` 等が要る）。加えて `missing_scope` 時の type 削減リトライロジック（除外後 0 件 or 全く減らなければ元エラー再送出）は分岐が細かい。

### 6-8. `bookmark add/remove` はチャンネル名を解決しない

ヘルプが「Channel ID」なのに検証も解決もしないため、チャンネル名を渡すと Slack 側が `channel_not_found` を返す。Rust で「親切に」名前解決を足すと挙動が変わる。互換優先なら渡された文字列をそのまま送る。

### 6-9. `parseLimit` の NaN 無防備

`bookmark list --limit abc` は `NaN` を `count` に載せて送る。JSON シリアライズで `NaN` は `null` になるため、実際には `count: null` 相当が飛ぶ（SDK の挙動次第。詳細は未確認＝不明）。Rust で `u32` にパースすると、ここは必然的にエラーになる。エラーにする／既定値に落とすのどちらにするか決めが必要。

### 6-10. `stars.*` は Slack 側で非推奨扱いの API 群

`stars.add` / `stars.list` / `stars.remove` を使っている。現行 Slack でのサポート状況・必要スコープ（`stars:read` / `stars:write`）については本調査ではソースから確認できていない＝不明。移植前に API の現況確認が要る。

### 6-11. ダウンロードのストリーミングと権限

`createWriteStream` + `pipeline` でストリーム保存し、**上書き確認をしない**（既存ファイルは黙って上書き）。また保存先ディレクトリが存在しない場合は fs のエラーがそのまま出る。ファイルパーミッションの明示指定は無し（`FILE_PERMISSIONS` は config 用途で、ダウンロードには適用されていない）。Rust では `reqwest` のストリームを `tokio::fs::File` に流す形になるが、同じ「無警告上書き」を維持するか判断が要る。

### 6-12. `--url` の任意 URL に Bearer トークンを付けて送る

`download --url` はホストを検証せず、渡された URL に対して無条件で `Authorization: Bearer <token>` を付ける。Slack ドメイン以外を渡せばトークンが外部に漏れる。移植時にホスト検証を入れるかは、互換性ではなくセキュリティの判断として扱うべき箇所。

### 6-13. 終了コードは常に 1

成功は 0、失敗は（バリデーションでも API エラーでも）すべて 1。エラー種別ごとの終了コード分けは存在しない。`SlackCliError` の `code`（`VALIDATION_ERROR` 等）は文字列プロパティとして持つだけで、終了コードにも出力にも反映されていない。

### 6-14. 色付け

`chalk` による色は成功メッセージ（緑）、エラー接頭辞（赤）、canvas の ID（シアン）、開発時 stack（グレー）のみ。chalk は TTY 判定で自動的に色を落とすため、Rust 側でも `NO_COLOR` / 非 TTY 時の無効化を合わせないとパイプ出力が変わる。

### 6-15. `postAction` の更新チェック

全コマンドの実行後に `checkForUpdates` が走る（`src/utils/update-notifier.ts`。内部実装は本調査で未読＝不明）。ネットワークアクセスや遅延が入る可能性があるので、Rust 版で同等機能を持たせるかは別途判断が要る。
