# 共通基盤層 仕様書（エラー / コマンド基盤 / 下書き / 更新通知 / 型定義）

移植元: `/Users/mimo/organizations/open-source/slack-cli`（TypeScript）
対象ファイル:

- `src/utils/errors.ts`
- `src/utils/error-utils.ts`
- `src/utils/command-support.ts`
- `src/utils/command-wrapper.ts`
- `src/utils/draft-store.ts`
- `src/utils/update-notifier.ts`
- `src/types/commands.ts` / `src/types/config.ts` / `src/types/slack.ts`

補助的に読んだファイル（値の実体を確認するため）:
`src/utils/constants.ts`, `src/utils/option-parsers.ts`, `src/utils/terminal-sanitizer.ts`,
`src/utils/token-utils.ts`, `src/utils/client-factory.ts`, `src/utils/config-helper.ts`,
`src/utils/slack-api-client.ts`, `src/index.ts`（先頭60行）, `package.json`。

本書に書いた事実はすべて上記ファイルの実読に基づく。読んでいない箇所は「不明」と明記する。

---

## 1. エラー型階層（`src/utils/errors.ts`）

### 1.1 クラス定義

```
Error
 └─ SlackCliError            (name = "SlackCliError",   code = コンストラクタ引数 code?: string)
     ├─ ConfigurationError   (name = "ConfigurationError", code = "CONFIGURATION_ERROR")
     ├─ ValidationError      (name = "ValidationError",    code = "VALIDATION_ERROR")
     ├─ ApiError             (name = "ApiError",           code = "API_ERROR")
     └─ FileError            (name = "FileError",          code = "FILE_ERROR")
```

- `SlackCliError` は `constructor(message: string, public code?: string)`。`code` は省略可能で、直接 `new SlackCliError(msg)` した場合 `code` は `undefined`。
- `this.name = this.constructor.name`（＝実クラス名）を設定する。JSONに出す用途はこのファイル内には無い。
- サブクラス4種はいずれも `message` のみを受け取り、`code` を固定文字列で親に渡す。追加フィールドは持たない。

### 1.2 Rustでの再現方針（設計上の要点）

- 単一 enum（例 `SlackCliError { Configuration(String), Validation(String), Api(String), File(String), Other{ message: String, code: Option<String> } }`）で表せる。
  `code()` は `"CONFIGURATION_ERROR" | "VALIDATION_ERROR" | "API_ERROR" | "FILE_ERROR" | Option<String>` を返す。
- `name` に相当する値（`"ConfigurationError"` など）はソース内で参照している箇所を本タスクの対象ファイル内には確認できなかった（対象外ファイルでの利用有無は**不明**）。

### 1.3 終了コード

- 対象ファイル内に登場する終了コードは **`process.exit(1)` の 1 種類のみ**（`command-wrapper.ts` の catch 節）。
- エラー種別ごとに終了コードを変えるロジックは存在しない。正常時は明示的な exit を行わない（Node のデフォルト = 0）。
- Rust: 全エラーで `std::process::exit(1)`、正常終了は 0。

---

## 2. Slackエラー解析（`src/utils/error-utils.ts`）

内部型（非公開）:

```ts
type SlackErrorData = { error?: string; needed?: string };
```

### 2.1 `getSlackErrorData(error: unknown): SlackErrorData | undefined`（非公開）

分岐:

1. `error` が `Error` インスタンスでない → `undefined`
2. `error.data` が falsy、または `typeof data !== 'object'` → `undefined`
3. それ以外 → `data` をそのまま `SlackErrorData` として返す（型検証なし）

※ `data` は Slack SDK が `Error` に生やす追加プロパティ。Rust側では「Slack APIレスポンスのボディを保持したエラー型」で再現する必要がある。

### 2.2 `getSlackErrorCode(error: unknown): string | undefined`（公開）

1. `data.error` が `string` かつ長さ > 0 → その値を返す
2. そうでなく `error` が `Error` かつ `error.message` に部分文字列 `"missing_scope"` を含む → `"missing_scope"` を返す
3. それ以外 → `undefined`

### 2.3 `getSlackNeededScopes(error: unknown): string[]`（公開）

1. `data.needed` が `string` でない、または空文字 → `[]`
2. そうでなければ `needed` を `","` で split → 各要素 `trim()` → 空文字を除外 → 配列で返す

### 2.4 `extractErrorMessage(error: unknown): string`（公開）

1. `error` が `Error` の場合:
   - `getSlackErrorCode(error) === 'missing_scope'` かつ `getSlackNeededScopes(error).length > 0`
     → `` `${error.message} (needed: ${scopes.join(', ')})` ``（区切りは **カンマ+スペース**）
   - それ以外 → `error.message`
2. `Error` でない場合 → `String(error)`（JSの `String()` セマンティクス。Rustでは `Display` 相当）

副作用: なし（純粋関数）。

---

## 3. コマンド補助（`src/utils/command-support.ts`）

非公開インターフェース:

```ts
interface ProfileOption { profile?: string }
interface FormatOption  { format?: string }
interface FormatRenderers<T> {
  table: (data: T) => void;      // 必須
  simple?: (data: T) => void;    // 任意
  json?: (data: T) => void;      // 任意
}
```

### 3.1 `withSlackClient<TOptions extends ProfileOption, TResult>(options, action): Promise<TResult>`

処理:

1. `profile = parseProfile(options.profile)` — `parseProfile` は**恒等関数**（`profile` をそのまま返す。`undefined` も `undefined` のまま）
2. `client = await createSlackClient(profile)`
   - `createSlackClient` は `getConfigOrThrow(profile)` でトークンを取得し `new SlackApiClient(config.token)` を返す
   - `getConfigOrThrow`: `ProfileConfigManager.getConfig(profile)` が falsy のとき、
     `profileName = profile ?? listProfiles() の中で isDefault が true のものの name ?? 'default'` を決め、
     `ConfigurationError(ERROR_MESSAGES.NO_CONFIG(profileName))` を throw
3. `return await action(client)`

副作用: 設定ファイル読み込み、Slackクライアント生成。`action` の例外はそのまま伝播。

### 3.2 `renderByFormat<T>(options: FormatOption, data: T, renderers: FormatRenderers<T>): void`

1. `format = parseFormat(options.format)` — `format || 'table'`（空文字も `'table'` にフォールバック）
2. `format === 'json'` の場合:
   - `renderers.json` があればそれを呼んで return
   - 無ければ `console.log(JSON.stringify(sanitizeTerminalData(data), null, 2))`（**インデント2スペース**、stdout）して return
3. `format === 'simple'` かつ `renderers.simple` がある → `renderers.simple(data)` で return
   - `simple` レンダラが無い場合はフォールスルーして table になる
4. 上記いずれでもない → `renderers.table(data)`

`format` が `'table'/'simple'/'json'` 以外の未知の値でもエラーにならず table になる。

参考: `sanitizeTerminalData<T>(value)` は再帰的サニタイズ。

- `string` → `sanitizeTerminalText`
- 配列 → 各要素を再帰
- **プレーンオブジェクト**（プロトタイプが `Object.prototype` または `null`）のみ → 各値を再帰
- それ以外（数値・真偽値・null・クラスインスタンス・Map等）→ そのまま

---

## 4. 全コマンド共通ラッパー（`src/utils/command-wrapper.ts`）

### 4.1 `wrapCommand<T>(action: CommandAction<T>): CommandAction<T>`

`type CommandAction<T = unknown> = (options: T) => Promise<void> | void;`

返される関数の挙動:

1. `await action(options)` を実行
2. 例外を捕捉したとき（**この順序が重要**）:
   1. `extractErrorMessage(error)`
   2. `sanitizeTerminalText(...)` — 制御文字・ANSI/OSCシーケンス除去
   3. `redactSlackTokens(...)` — トークンのマスク
      → コメント上の意図: 「エスケープシーケンスで分断されたトークンも確実に伏せるため、先にサニタイズする」
   4. `console.error(chalk.red('✗ Error:'), <上記結果>)` — **stderr**、赤色のラベル `✗ Error:` と本文をスペース区切りで出力（`console.error` の可変長引数はスペース連結）
3. `process.env.NODE_ENV === 'development'` かつ `error instanceof Error` のとき、
   `console.error(chalk.gray(redactSlackTokens(sanitizeTerminalText(error.stack ?? ''))))` を追加出力（stderr、グレー）
4. `process.exit(1)`

副作用: stderr出力、プロセス終了。

#### 4.1.1 `sanitizeTerminalText(value: string): string` の正確な仕様

- 空文字/falsy → `''`
- OSCシーケンス除去（正規表現）: `\][^]*(?:|\\)` を全置換で削除
- ANSIシーケンス除去: `(?:[@-Z\\-_]|\[[0-?]*[ -/]*[@-~])` を全置換で削除
- その後、**Unicodeコードポイント単位**で走査し、次を削除:
  - `code < 0x20`（ASCII制御）
  - `code === 0x7F`（DEL）
  - `0x80 <= code <= 0x9F`（C1制御）
  - ただし `0x09`（TAB）と `0x0A`（LF）は残す（`0x0D` CRは削除される）
- 注: 判定は `for...of`（コードポイント単位）で取り出した文字の `charCodeAt(0)`。サロゲートペアは先頭コードユニットで判定されるが、`0xD800` 以上なので削除条件に該当しない。

関連関数（同ファイル、本層で使う可能性あり）:
`sanitizeSingleLineText(value)` = `sanitizeTerminalText(value).replace(/\s+/g, ' ').trim()`。

#### 4.1.2 `redactSlackTokens(text)` の正確な仕様

- パターン: `/xox[bpoars]-[A-Za-z0-9-]+/gi`（大文字小文字を区別しない、グローバル）
- 置換: マッチ全体の**先頭4文字を小文字化**したものを prefix とし、`` `${prefix}-***-REDACTED` `` に置換
  例: `xoxb-123-abc` → `xoxb-***-REDACTED` / `XOXP-...` → `xoxp-***-REDACTED`
- `undefined` を渡すと `undefined` を返す（オーバーロード）。

関連: `maskToken(token)` — 長さ `<= TOKEN_MIN_LENGTH(9)` なら `'****'`、それ以外は
`` `${先頭4文字}-****-****-${末尾4文字}` ``（`TOKEN_MASK_LENGTH = 4`）。

### 4.2 `getProfileName(configManager, providedProfile?): Promise<string>`

- シグネチャ: `configManager: { getCurrentProfile: () => Promise<string> }`, `providedProfile?: string`
- 戻り: `providedProfile || await configManager.getCurrentProfile()`
  （`providedProfile` が**空文字の場合も**フォールバックする＝ falsy 判定）

---

## 5. 下書きのローカル保存（`src/utils/draft-store.ts`）

### 5.1 データ型

```ts
interface Draft {
  id: string;          // 8桁hex（4バイト乱数）
  channel?: string;
  user?: string;
  message: string;
  thread?: string;
  createdAt: string;   // ISO 8601（new Date().toISOString()）
}

interface DraftInput { channel?: string; user?: string; message: string; thread?: string }
interface DraftStoreOptions { configDir?: string }
```

### 5.2 保存先

- `configDir` = `options.configDir` または `path.join(os.homedir(), '.slack-cli')`
- ファイル = `<configDir>/drafts.json`
- 形式: **`Draft` オブジェクトの JSON 配列**、`JSON.stringify(drafts, null, 2)`（2スペースインデント）、UTF-8
- パーミッション: ディレクトリ `0o700`（`FILE_PERMISSIONS.CONFIG_DIR`）、ファイル `0o600`（`FILE_PERMISSIONS.CONFIG_FILE`）

### 5.3 公開メソッド

| メソッド | シグネチャ | 挙動 |
| --- | --- | --- |
| `save` | `(input: DraftInput) => Promise<Draft>` | 既存を読み、`{...input, id: generateId(既存), createdAt: now.toISOString()}` を末尾に push して全件書き戻し、生成した `Draft` を返す。`id`/`createdAt` は input の同名キーを**上書きする**（スプレッドの後に定義しているため） |
| `list` | `() => Promise<Draft[]>` | 読み込んだ配列をそのまま返す（並べ替えなし＝保存順） |
| `get` | `(id: string) => Promise<Draft \| null>` | `id` 完全一致の最初の1件、無ければ `null` |
| `delete` | `(id: string) => Promise<void>` | `id` 一致を除いた配列を書き戻す。1件も減らなかった場合は `ValidationError("Draft not found: {id}")` を throw（この場合ファイルは書き換えない） |

### 5.4 内部処理

`generateId(existing)`:

- `randomBytes(4).toString('hex')`（＝ 8文字の小文字hex）を生成し、既存 `id` と衝突する間ループ（do-while）。

`readDrafts()`:

1. `drafts.json` を UTF-8 で読む
2. `JSON.parse` する（**パース失敗の例外はそのまま伝播する**。ENOENT以外は catch されない）
3. 結果が配列でなければ `[]`
4. 配列要素のうち「オブジェクトかつ非null、`id` が string、`message` が string」のものだけを残す（他フィールドは検証しない）
5. 読み込みエラーが `ENOENT`（ファイル無し）なら `[]` を返す。それ以外の I/O エラーは throw

`writeDrafts(drafts)`（アトミック書き込み）:

1. `fs.mkdir(configDir, { recursive: true, mode: 0o700 })`
2. `fs.chmod(configDir, 0o700)`（既存ディレクトリの権限も必ず矯正する）
3. 一時ファイル名 `` `${draftsPath}.${process.pid}.${Date.now()}.tmp` `` に
   `writeFile(..., { encoding: 'utf-8', mode: 0o600, flag: 'wx' })`
   - `flag: 'wx'` = 排他作成。既に同名が存在すれば失敗（EEXIST）
4. `fs.rename(tempPath, draftsPath)`。rename が失敗した場合は一時ファイルを `unlink`（失敗は無視）してから元エラーを再 throw

Rust移植上の注意: `flag: 'wx'` は `OpenOptions::new().write(true).create_new(true)`、mode は
`std::os::unix::fs::OpenOptionsExt::mode(0o600)` に対応。Windowsのパーミッション扱いは元コードでも未定義（**不明**）。

---

## 6. 更新通知（`src/utils/update-notifier.ts`）

### 6.1 定数

| 名前 | 値 |
| --- | --- |
| `DEFAULT_CACHE_TTL_MS` | `24 * 60 * 60 * 1000` = 86,400,000 ms（24時間） |
| `DEFAULT_REQUEST_TIMEOUT_MS` | `2000` ms |
| キャッシュファイル名 | `update-notifier.json` |
| 既定キャッシュディレクトリ | `path.join(os.homedir(), '.slack-cli')` |
| レジストリURL | `https://registry.npmjs.org/{encodeURIComponent(packageName)}/latest` |
| リクエストヘッダ | `accept: application/json` |

呼び出し元（`src/index.ts`）: commander の `program.hook('postAction', ...)` で
`checkForUpdates({ packageName: packageJson.name, currentVersion: packageJson.version })` を実行。
`package.json` の実値は `name = "@mimo-3/slack-cli"`, `version = "0.24.1"`（移植時点）。
`@` と `/` を含むスコープ名は `encodeURIComponent` により `%40mimo-3%2Fslack-cli` になる。

### 6.2 `checkForUpdates(options): Promise<void>`

```ts
interface CheckForUpdatesOptions {
  packageName: string;
  currentVersion: string;
  cacheDir?: string;
  cacheTtlMs?: number;
  fetchImpl?: typeof fetch;   // テスト用の差し替え
}
```

分岐フロー:

1. `shouldSkipUpdateCheck()` が true → **何もせず return**
   - スキップ条件（OR）:
     - `process.env.CI !== undefined`（値が空文字でも「定義されていれば」スキップ）
     - `process.env.SLACK_CLI_DISABLE_UPDATE_NOTIFIER === '1'`（厳密に文字列 `"1"`）
     - `!process.stderr.isTTY`（stderr が TTY でない＝パイプ/リダイレクト時はスキップ）
2. `cachePath = path.join(options.cacheDir ?? homedir()/'.slack-cli', 'update-notifier.json')`
3. `cacheTtlMs = options.cacheTtlMs ?? 86400000`（`??` なので `0` は尊重される）
4. try ブロック:
   - `cached = await readCache(cachePath)`
   - `cached` が存在し `isFresh(cached.lastCheckedAt, cacheTtlMs)` なら `latestVersion = cached.latestVersion`、
     そうでなければ `latestVersion = await fetchLatestVersion(...)`
   - `semverGt(latestVersion, currentVersion)` が true なら `notifyUpdate(...)`
5. catch: **完全に握りつぶす**（コメント: 更新チェックが通常のCLI動作に影響してはならない）

### 6.3 補助関数

`isFresh(lastCheckedAt, cacheTtlMs)`:

- `Date.parse(lastCheckedAt)` が NaN → `false`
- それ以外 → `Date.now() - lastCheckedTime < cacheTtlMs`
  （負の差分＝未来の時刻でも true になる）

`fetchLatestVersion(packageName, fetchImpl = fetch, cachePath)`:

1. `AbortController` を作り、`setTimeout(abort, 2000)` を仕掛ける
2. `GET https://registry.npmjs.org/{encoded}/latest`（headers: `accept: application/json`, signal付き）
3. `!response.ok` → `throw new Error("Unexpected status: {status}")`
4. `payload = await response.json()`。`payload.version` が string でない、または空文字 →
   `throw new Error('Registry response does not contain a valid version')`
5. `writeCache(cachePath, { latestVersion: payload.version, lastCheckedAt: new Date().toISOString() })`
6. `payload.version` を返す
7. finally で `clearTimeout`

いずれの throw も呼び出し元 `checkForUpdates` の catch で握りつぶされる。

`readCache(cachePath)`:

- ファイルを UTF-8 で読み `JSON.parse`
- `latestVersion` と `lastCheckedAt` の**両方が string** でなければ `null`
- `ENOENT` なら `null`、それ以外の I/O エラーは throw（＝上位で握りつぶし）
- JSONパース失敗は throw（上位で握りつぶし）

`writeCache(cachePath, cache)`（draft-store と同じアトミック手順。ただし `chmod` は行わない）:

1. `fs.mkdir(dirname(cachePath), { recursive: true, mode: 0o700 })`
2. 一時ファイル `` `${cachePath}.${pid}.${Date.now()}.tmp` `` に
   `JSON.stringify(cache, null, 2)` を `{ encoding: 'utf-8', mode: 0o600, flag: 'wx' }` で書く
3. `rename` → 失敗時は一時ファイルを `unlink`（失敗無視）して再 throw

`notifyUpdate(currentVersion, latestVersion, packageName)` — **stderr** に黄色で2行:

```
Update available: {currentVersion} -> {latestVersion}
Run: npm install -g {packageName}
```

`isErrnoException(error)`: `typeof error === 'object' && error !== null && 'code' in error`

### 6.4 バージョン比較

- `semver/functions/gt`（npm `semver` パッケージ）を使用。厳密な大なり比較。
- Rust では `semver` crate の `Version::parse` + `>` で概ね等価だが、
  npm semver がゆるく受理する表記（先頭 `v` 付き等）の扱いは**差異が出うる**。レジストリの `version` は
  通常正規のsemverなので実害は小さいが、パース失敗時は「通知しない」に倒すのが元挙動（例外は握りつぶされる）と整合する。

### 6.5 キャッシュJSON形式

```json
{
  "latestVersion": "0.24.1",
  "lastCheckedAt": "2026-01-01T00:00:00.000Z"
}
```

---

## 7. 参照している共通定数（`src/utils/constants.ts`）値の完全列挙

```
TOKEN_MASK_LENGTH      = 4
TOKEN_MIN_LENGTH       = 9
DEFAULT_PROFILE_NAME   = "default"

FILE_PERMISSIONS.CONFIG_DIR  = 0o700
FILE_PERMISSIONS.CONFIG_FILE = 0o600

API_LIMITS.MAX_MESSAGE_COUNT     = 1000
API_LIMITS.MIN_MESSAGE_COUNT     = 1
API_LIMITS.DEFAULT_MESSAGE_COUNT = 10
API_LIMITS.DEFAULT_SEARCH_COUNT  = 20
API_LIMITS.MAX_SEARCH_COUNT      = 100
API_LIMITS.MIN_SEARCH_COUNT      = 1
API_LIMITS.MAX_SEARCH_PAGE       = 100
API_LIMITS.MIN_SEARCH_PAGE       = 1

RATE_LIMIT.CONCURRENT_REQUESTS               = 3
RATE_LIMIT.UNREAD_SCAN_CONCURRENT_REQUESTS   = 15
RATE_LIMIT.BATCH_SIZE                        = 10
RATE_LIMIT.BATCH_DELAY_MS                    = 1000
RATE_LIMIT.RETRY_CONFIG = { retries: 3, factor: 2, minTimeout: 1000, maxTimeout: 30000 }

DEFAULTS.HISTORY_LIMIT                 = 20
DEFAULTS.CHANNELS_LIMIT                = 1000
DEFAULTS.UNREAD_DISPLAY_LIMIT          = 50
DEFAULTS.UNREAD_MESSAGE_PREVIEW_LIMIT  = 50

TIME_FORMAT = "YYYY-MM-DD HH:mm:ss"

OPTION_DEFAULTS = { format: "table", limit: 100, countOnly: false, includeArchived: false }
```

### 7.1 エラーメッセージ文言（`ERROR_MESSAGES`）

| キー | 文言（`{}` は補間） |
| --- | --- |
| `NO_CONFIG(profileName)` | `No configuration found for profile "{profileName}". Use "slack-cli config set --token <token> --profile {profileName}" to set up.` |
| `PROFILE_NOT_FOUND(profileName)` | `Profile "{profileName}" not found` |
| `NO_PROFILES_FOUND` | `No profiles found. Use "slack-cli config set --token <token>" to create one.` |
| `INVALID_CONFIG_FORMAT` | `Invalid config file format` |
| `NO_MESSAGE_OR_FILE` | `You must specify either --message or --file` |
| `BOTH_MESSAGE_AND_FILE` | `Cannot use both --message and --file` |
| `INVALID_BLOCKS_JSON` | `Invalid blocks JSON: must be a valid JSON array` |
| `BLOCKS_FILE_READ_ERROR(file, error)` | `Error reading blocks file {file}: {error}` |
| `BOTH_BLOCKS_AND_BLOCKS_FILE` | `Cannot use both --blocks and --blocks-file` |
| `INVALID_THREAD_TIMESTAMP` | `Invalid thread timestamp format` |
| `INVALID_SCHEDULE_AT` | `Invalid schedule time format. Use Unix timestamp (seconds) or ISO 8601 date-time` |
| `INVALID_SCHEDULE_AFTER` | `--after must be a positive integer (minutes)` |
| `BOTH_SCHEDULE_OPTIONS` | `Cannot use both --at and --after` |
| `SCHEDULE_TIME_IN_PAST` | `Schedule time must be in the future` |
| `API_ERROR(error)` | `API Error: {error}` |
| `CHANNEL_NOT_FOUND(channel)` | `Channel not found: {channel}` |
| `FILE_READ_ERROR(file, error)` | `Error reading file {file}: {error}` |
| `FILE_NOT_FOUND(file)` | `File not found: {file}` |
| `NO_CHANNELS_FOUND` | `No channels found` |
| `ERROR_LISTING_CHANNELS(error)` | `Error listing channels: {error}` |

### 7.2 成功メッセージ文言（`SUCCESS_MESSAGES`）

| キー | 文言 |
| --- | --- |
| `TOKEN_SAVED(profileName)` | `Token saved successfully for profile "{profileName}"` |
| `PROFILE_SWITCHED(profileName)` | `Switched to profile "{profileName}"` |
| `PROFILE_CLEARED(profileName)` | `Profile "{profileName}" cleared successfully` |
| `MESSAGE_SENT(channel)` | `Message sent successfully to #{channel}` |
| `MESSAGE_SCHEDULED(channel, postAtIso)` | `Message scheduled to #{channel} at {postAtIso}` |
| `EPHEMERAL_MESSAGE_SENT(channel)` | `Ephemeral message sent to #{channel}` |

### 7.3 本層で直接使う固定文言

| 出所 | 文言 |
| --- | --- |
| `command-wrapper.ts` | `✗ Error:`（赤） |
| `draft-store.ts` | `Draft not found: {id}`（ValidationError） |
| `update-notifier.ts` | `Update available: {current} -> {latest}`（黄・stderr） |
| `update-notifier.ts` | `Run: npm install -g {packageName}`（黄・stderr） |
| `update-notifier.ts` | `Unexpected status: {status}`（内部Error、外部に出ない） |
| `update-notifier.ts` | `Registry response does not contain a valid version`（内部Error、外部に出ない） |
| `error-utils.ts` | `{message} (needed: {scope1, scope2})` |
| `token-utils.ts` | `{prefix}-***-REDACTED` / `****` / `{4文字}-****-****-{4文字}` |

### 7.4 環境変数

| 変数 | 用途 | 判定 |
| --- | --- | --- |
| `NODE_ENV` | `development` のときスタックトレースを stderr に出す | 厳密一致 `=== 'development'` |
| `CI` | 定義されていれば更新チェックをスキップ | `!== undefined`（値は不問） |
| `SLACK_CLI_DISABLE_UPDATE_NOTIFIER` | `"1"` で更新チェック無効 | 厳密一致 |

---

## 8. オプションパーサ（`src/utils/option-parsers.ts`）

| 関数 | シグネチャ | 挙動 |
| --- | --- | --- |
| `parseFormat` | `(format?: string, defaultFormat = 'table') => string` | `format \|\| defaultFormat`（空文字もデフォルト） |
| `parseLimit` | `(limit: string \| undefined, defaultLimit: number) => number` | `parseInt(limit \|\| String(defaultLimit), 10)`。**NaNチェック無し**（不正文字列は NaN を返す） |
| `parseBoolean` | `(value?: boolean, defaultValue = false) => boolean` | `value !== undefined ? value : defaultValue` |
| `parseCount` | `(count, defaultCount, min?, max?) => number` | `parseInt` → NaN なら `defaultCount` → `min` 未満なら `min` → `max` 超なら `max` |
| `parseProfile` | `(profile?: string) => string \| undefined` | 恒等 |
| `parseListOptions` | `(options: ListOptions, defaults?: Partial<ParsedListOptions>) => ParsedListOptions` | `OPTION_DEFAULTS` に `defaults` をマージし、`format`/`limit`/`countOnly` を上記関数で解決 |

注: `parseInt` は先頭の数字列だけを読む（`"12abc"` → 12、`" 5"` → 5）。Rustの `str::parse::<i64>()` は
これらを拒否するため、忠実に再現するなら先頭数字列の抽出を自前で行う必要がある。

---

## 9. Slack APIレスポンス型 / ドメイン型一覧（`src/types/slack.ts`）

すべて TypeScript の `interface`。`?` は省略可能（`undefined` 許容）。フィールド名は Slack API のスネークケースをそのまま保持しているものと、CLI内部で組み立てたキャメルケースのものが混在する。

### 9.1 チャンネル

**`Channel`**
| フィールド | 型 | 必須 |
| --- | --- | --- |
| `id` | string | ✓ |
| `name` | string | ✓ |
| `display_name` | string | |
| `user` | string | |
| `is_channel` | boolean | |
| `is_group` | boolean | |
| `is_im` | boolean | |
| `is_mpim` | boolean | |
| `is_private` | boolean | ✓ |
| `created` | number | ✓ |
| `is_archived` | boolean | |
| `is_general` | boolean | |
| `unlinked` | number | |
| `name_normalized` | string | |
| `is_shared` | boolean | |
| `is_ext_shared` | boolean | |
| `is_org_shared` | boolean | |
| `is_member` | boolean | |
| `num_members` | number | |
| `unread_count` | number | |
| `unread_count_display` | number | |
| `last_read` | string | |
| `topic` | `{ value: string; creator?: string; last_set?: number }` | |
| `purpose` | `{ value: string; creator?: string; last_set?: number }` | |

**`ChannelDetail`**: `id: string`, `name: string`, `is_private: boolean`, `is_archived?: boolean`, `created: number`, `num_members?: number`, `topic?`/`purpose?`（`Channel` と同じ形）

**`ListChannelsOptions`**: `types: string`, `exclude_archived: boolean`, `limit: number`（全て必須）

**`ChannelMembersOptions`**: `limit?: number`, `cursor?: string`
**`ChannelMembersResult`**: `members: string[]`, `nextCursor: string`（**キャメルケース**、必須）

### 9.2 メッセージ

**`Message`**: `type: string`（必須）, `text?`, `user?`, `bot_id?`, `ts: string`（必須）, `thread_ts?`, `reply_count?: number`, `attachments?: unknown[]`, `blocks?: unknown[]`, `files?: FileAttachment[]`

**`FileAttachment`**: `id: string`（必須）, `name?`, `title?`, `mimetype?`, `filetype?`, `size?: number`, `url_private?`, `url_private_download?`, `permalink?`

**`ScheduledMessage`**: `id: string`, `channel_id: string`, `post_at: number`, `date_created: number`, `text?: string`

**`HistoryOptions`**（Slack API 用。`src/types/commands.ts` の同名 CLI 型とは別物）: `limit: number`（必須）, `oldest?: string`

**`HistoryResult`**: `messages: Message[]`, `users: Map<string, string>`（**userId → 表示名のマップ**）

**`ChannelUnreadResult`**: `channel: Channel`, `messages: Message[]`, `users: Map<string, string>`, `totalUnreadCount: number`, `displayedMessageCount: number`

### 9.3 ユーザー / ユーザーグループ

**`SlackUser`**: `id?`, `name?`, `real_name?`, `profile?: { email?; display_name?; title?; status_text?; status_emoji? }`, `tz?`, `tz_label?`, `is_admin?: boolean`, `is_bot?: boolean`, `deleted?: boolean` — **全フィールド省略可**

**`SlackUsergroup`**: `id?`, `team_id?`, `name?`, `handle?`, `description?`, `is_external?: boolean`, `date_create?: number`, `date_delete?: number`, `user_count?: number` — 全て省略可

**`UserPresence`**: `presence: string`（必須）

### 9.4 検索

**`SearchMatch`**: `text?`, `user?`, `username?`, `ts?`, `channel: { id?: string; name?: string }`（**`channel` 自体は必須**）, `permalink?`

**`SearchMessagesOptions`**: `sort?: 'score' | 'timestamp'`, `sortDir?: 'asc' | 'desc'`, `count?: number`, `page?: number`

**`SearchResult`**: `query: string`, `matches: SearchMatch[]`, `totalCount: number`, `page: number`, `pageCount: number`（全て必須、キャメルケース）

### 9.5 ピン / スター / リマインダー

**`PinnedItem`**: `type?`, `created?: number`, `created_by?`, `message?: { text?; user?; ts? }`

**`StarredItem`**: `type: string`, `channel: string`, `message: { text: string; ts: string }`, `date_create: number`（全て必須）
**`StarListResult`**: `items: StarredItem[]`

**`Reminder`**: `id: string`, `text: string`, `time: number`, `complete_ts: number`, `recurring: boolean`（全て必須）

### 9.6 キャンバス

**`CanvasSectionElement`**: `type?`, `text?`, `elements?: CanvasSectionElement[]`（**再帰型**）
**`CanvasSection`**: `id?`, `elements?: CanvasSectionElement[]`
**`CanvasFile`**: `id?`, `name?`, `created?: number`, `filetype?`

### 9.7 設定型（`src/types/config.ts`）

**`Config`**: `token: string`, `updatedAt: string`
**`Profile`**: `name: string`, `config: Config`, `isDefault?: boolean`
**`ConfigStore`**: `profiles: Record<string, Config>`, `defaultProfile?: string`
**`ConfigOptions`**: `configDir?: string`, `profile?: string`

### 9.8 コマンドオプション型（`src/types/commands.ts`）一覧

すべて `profile?: string` を持つ（`ConfigUseOptions` を除く）。`format` はある場合 `'table' | 'simple' | 'json'`。

| 型 | フィールド |
| --- | --- |
| `ConfigSetOptions` | `token: string`, `profile?` |
| `ConfigGetOptions` | `profile?` |
| `ConfigUseOptions` | `profile: string`（必須） |
| `ConfigClearOptions` | `profile?` |
| `SendOptions` | `channel?`, `user?`, `email?`, `message?`, `file?`, `blocks?`, `blocksFile?`, `thread?`, `at?`, `after?`, `profile?` |
| `ScheduledListOptions` | `channel?`, `limit?`, `format?`, `profile?` |
| `ScheduledCancelOptions` | `channel: string`, `id: string`, `profile?` |
| `ChannelsOptions` | `type: 'public'\|'private'\|'im'\|'mpim'\|'all'`, `includeArchived: boolean`, `format`, `limit: string`（**すべて必須**）, `profile?` |
| `HistoryOptions` | `channel: string`, `number?`, `since?`, `thread?`, `withLink?: boolean`, `format?`, `profile?` |
| `UnreadOptions` | `channel?`, `format?`, `countOnly?: boolean`, `limit?`, `markRead?: boolean`, `profile?` |
| `UploadOptions` | `channel: string`, `file?`, `content?`, `filename?`, `title?`, `message?`, `filetype?`, `thread?`, `format?`, `profile?` |
| `EditOptions` | `channel: string`, `ts: string`, `message?`, `file?`, `blocks?`, `blocksFile?`, `profile?` |
| `DeleteOptions` | `channel: string`, `ts: string`, `profile?` |
| `ReactionOptions` | `channel: string`, `timestamp: string`, `emoji: string`, `profile?` |
| `PinOptions` | `channel: string`, `timestamp: string`, `profile?` |
| `PinListOptions` | `channel: string`, `format?`, `profile?` |
| `UsersListOptions` | `limit?`, `format?`, `profile?` |
| `UsersInfoOptions` | `id: string`, `format?`, `profile?` |
| `UsersLookupOptions` | `email: string`, `format?`, `profile?` |
| `UsersPresenceOptions` | `id?`, `name?`, `format?`, `profile?` |
| `UsergroupsListOptions` | `includeDisabled?: boolean`, `format?`, `profile?` |
| `UsergroupsMembersOptions` | `id?`, `handle?`, `format?`, `profile?` |
| `ChannelInfoOptions` | `channel: string`, `format?`, `profile?` |
| `ChannelSetTopicOptions` | `channel: string`, `topic: string`, `profile?` |
| `ChannelSetPurposeOptions` | `channel: string`, `purpose: string`, `profile?` |
| `MembersOptions` | `channel: string`, `limit?`, `format?`, `profile?` |
| `SendEphemeralOptions` | `channel: string`, `user: string`, `message: string`, `thread?`, `profile?` |
| `JoinOptions` / `LeaveOptions` | `channel: string`, `profile?` |
| `InviteOptions` | `channel: string`, `users: string`, `force?: boolean`, `profile?` |
| `SearchOptions` | `query: string`, `sort?: 'score'\|'timestamp'`, `sortDir?: 'asc'\|'desc'`, `number?`, `page?`, `format?`, `profile?` |
| `ReminderAddOptions` | `text: string`, `at?`, `after?`, `profile?` |
| `ReminderListOptions` | `format?`, `profile?` |
| `ReminderDeleteOptions` / `ReminderCompleteOptions` | `id: string`, `profile?` |
| `BookmarkAddOptions` / `BookmarkRemoveOptions` | `channel: string`, `ts: string`, `profile?` |
| `BookmarkListOptions` | `limit?`, `format?`, `profile?` |
| `CanvasReadOptions` | `id: string`, `format?`, `profile?` |
| `CanvasListOptions` | `channel: string`, `format?`, `profile?` |
| `DownloadOptions` | `url?`, `id?`, `output?`, `format?`, `profile?` |

数値系オプション（`limit` / `number` / `page`）はコマンド層では**すべて `string`**（commander の生値）で受け取り、
`option-parsers` で数値化する点に注意。

---

## 10. Rust移植時に注意すべき挙動差（事実ベースの列挙）

1. `wrapCommand` の出力順は「extract → sanitize → redact」。順序を変えるとエスケープ列で分断されたトークンが漏れる。
2. `renderByFormat` は未知の format 値でもエラーにせず table にフォールバックする。
3. `parseLimit` は NaN を返しうる（バリデーションなし）。`parseCount` のみ NaN/min/max を丸める。
4. `getSlackErrorCode` は `data.error` が無くても message に `missing_scope` が含まれれば `"missing_scope"` を返す（文字列部分一致）。
5. `draft-store` / `update-notifier` の書き込みはどちらも「temp(wx, 0600) → rename」。`draft-store` のみ書き込み前にディレクトリを `chmod 0700` する。
6. 更新チェックは stderr が TTY でない場合スキップされるため、パイプ実行では通知が出ない。
7. 更新チェック内の例外はすべて握りつぶされ、終了コードに影響しない。
8. `Map<string, string>` を含む型（`HistoryResult`, `ChannelUnreadResult`）は `sanitizeTerminalData` の再帰対象外（プレーンオブジェクトでないため素通り）。JSON出力時も `JSON.stringify` で `{}` になる点は元実装の挙動。
