# Rust移植仕様書: search / users / usergroups / reaction / pin

対象TSソース（読み取り済み）:

- `src/index.ts`
- `src/commands/search.ts`, `users.ts`, `usergroups.ts`, `reaction.ts`, `pin.ts`
- `src/utils/`: `command-wrapper.ts`, `command-support.ts`, `option-parsers.ts`, `constants.ts`, `validators.ts`, `terminal-sanitizer.ts`, `client-factory.ts`, `config-helper.ts`, `error-utils.ts`, `errors.ts`, `token-utils.ts`, `date-utils.ts`, `channel-resolver.ts`
- `src/utils/slack-operations/`: `base-client.ts`, `user-operations.ts`, `usergroup-operations.ts`, `search-operations.ts`, `pin-operations.ts`, `reaction-operations.ts`, `channel-operations.ts`（一部）
- `src/utils/formatters/`: `base-formatter.ts`, `search-formatters.ts`, `members-formatters.ts`
- `src/types/commands.ts`, `src/types/slack.ts`

本書に書いたのは上記ファイルから読み取れた事実のみ。読んでいない範囲は「不明」と明記した。

---

## 0. 全体像

### 0.1 コマンド登録

`src/index.ts` の `createProgram()` が commander の `Command` を組み立てる。

- プログラム名: `slack-cli`
- description: `CLI tool to send messages via Slack API`
- `--version`: `package.json` の `version`（`__dirname/../package.json` を実行時に読む）
- `postAction` フックで `checkForUpdates({ packageName, currentVersion })` を毎回実行（update-notifier の中身は未読・不明）
- 本書の対象は `setupSearchCommand()` / `setupUsersCommand()` / `setupUsergroupsCommand()` / `setupReactionCommand()` / `setupPinCommand()` の5つ。
- **エイリアスは1つも定義されていない**（`.alias()` の呼び出しは対象5ファイルに存在しない）。

対象コマンド数: **トップレベル5個、実行可能な末端コマンド12個**（search / users×4 / usergroups×2 / reaction×2 / pin×3）。

### 0.2 サブコマンド構造

```
slack-cli
├─ search                      （末端。サブコマンドなし）
├─ users
│  ├─ list
│  ├─ info
│  ├─ lookup
│  └─ presence
├─ usergroups
│  ├─ list
│  └─ members
├─ reaction
│  ├─ add
│  └─ remove
└─ pin
   ├─ add
   ├─ remove
   └─ list
```

`users` / `usergroups` / `reaction` / `pin` は親コマンド自身に action を持たない（引数なしで呼ぶと commander のヘルプ挙動になる。commander のバージョン依存で終了コードが変わるため要確認）。

### 0.3 位置引数

**対象5ファイルには位置引数が一つも存在しない。** 全てオプションフラグで受け取る。

### 0.4 共通オプション

| ロング | ショート | 型 | デフォルト | 必須 | 備考 |
| --- | --- | --- | --- | --- | --- |
| `--profile <profile>` | なし | string | なし（未指定なら設定の defaultProfile） | 任意 | 全12コマンドに存在 |
| `--format <format>` | なし | string | `table` | 任意 | `search`, `users *`, `usergroups *`, `pin list` に存在。`reaction add/remove`, `pin add/remove` には無い |

`--format` の許容値は `table` / `simple` / `json`（`optionValidators.format`）。
※ `formatValidators.outputFormat` には `compact` も含まれるが、本書の対象コマンドはどれも `optionValidators.format` を使っており `compact` は通らない。

### 0.5 共通処理チェーン

1. commander が引数をパース（`requiredOption` 未指定はここでエラー、**終了コード 1**、commander の `error()` 経路）
2. `preAction` フック = `createValidationHook([...])`：バリデータを先頭から順に実行し、最初に非nullが返った時点で `thisCommand.error(\`Error: ${msg}\`)` を呼んで停止。commander の `error()` は stderr 出力後 `process.exit(1)`。
3. action = `wrapCommand(fn)`：`fn` を try/catch し、例外時は
   - stderr に `✗ Error:`（赤）+ `redactSlackTokens(sanitizeTerminalText(extractErrorMessage(error)))`
   - `NODE_ENV === 'development'` のときだけスタックトレースを灰色で追加出力（同様にサニタイズ＋トークン伏字）
   - `process.exit(1)`
4. `createSlackClient(profile)` → `getConfigOrThrow(profile)` → `ProfileConfigManager.getConfig()`。設定が無ければ `ConfigurationError`:
   `No configuration found for profile "<name>". Use "slack-cli config set --token <token> --profile <name>" to set up.`
   （`<name>` は `--profile` 指定値 → デフォルトプロファイル名 → `default` の順で決まる）
5. Slack WebClient を生成（`base-client.ts`）:
   - `retryConfig: { retries: 0 }` — **SDKの自動リトライは無効**
   - `logLevel: ERROR`
   - `rateLimiter = pLimit(RATE_LIMIT.CONCURRENT_REQUESTS = 3)`（並行3本）

正常終了時は明示的な `process.exit` を呼ばない＝**終了コード 0**。

### 0.6 出力サニタイズ（移植で落とせない仕様）

`terminal-sanitizer.ts`:

- `sanitizeTerminalText`: OSCシーケンス（`ESC ] ... BEL|ESC \`）とANSIシーケンスを除去し、さらに制御文字（`< 0x20`、`0x7F`、`0x80–0x9F`）を削除。**タブ(0x09)と改行(0x0A)は残す**。
- `sanitizeSingleLineText`: 上記のうえで `\s+` → 半角スペース1個、前後trim。TSV行やテーブルセルの偽造防止用。
- `sanitizeTerminalData<T>`: 文字列・配列・**プレーンオブジェクト（prototypeが `Object.prototype` か `null` のもののみ）**を再帰的にサニタイズ。それ以外の値（数値・boolean・クラスインスタンス等）はそのまま。
- `redactSlackTokens`: `/xox[bpoars]-[A-Za-z0-9-]+/gi` を `<先頭4文字小文字>-***-REDACTED` に置換。エラー出力経路のみで使用。

---

## 1. `slack-cli search`

### 1.1 定義

- description: `Search messages in Slack workspace`
- サブコマンド・位置引数なし

### 1.2 オプション

| ロング | ショート | 型 | デフォルト | 必須 | 排他 |
| --- | --- | --- | --- | --- | --- |
| `--query <query>` | `-q` | string | なし | **必須**（`requiredOption`） | なし |
| `--sort <sort>` | なし | string | `score` | 任意 | なし。許容値 `score` / `timestamp` |
| `--sort-dir <direction>` | なし | string | `desc` | 任意 | なし。許容値 `asc` / `desc` |
| `--number <count>` | `-n` | string（数値文字列） | 未指定時は 20 | 任意 | なし。1–100 |
| `--page <page>` | なし | string（数値文字列） | 未指定時は 1 | 任意 | なし。1–100 |
| `--format <format>` | なし | string | `table` | 任意 | なし |
| `--profile <profile>` | なし | string | なし | 任意 | なし |

commander の option name 変換により `--sort-dir` は `options.sortDir` になる。

### 1.3 バリデーション（preAction、この順で評価）

| 検証 | 条件 | メッセージ |
| --- | --- | --- |
| `searchSort` | `--sort` が `score`/`timestamp` 以外 | `Error: Invalid sort '<値>'. Must be one of: score, timestamp` |
| `searchSortDir` | `--sort-dir` が `asc`/`desc` 以外 | `Error: Invalid sort direction '<値>'. Must be one of: asc, desc` |
| `searchCount` | `--number` が数値でない | `Error: Count must be a number` |
| `searchCount` | `< 1` | `Error: Count must be at least 1` |
| `searchCount` | `> 100` | `Error: Count must be at most 100` |
| `searchPage` | `--page` が数値でない | `Error: Page must be a number` |
| `searchPage` | `< 1` | `Error: Page must be at least 1` |
| `searchPage` | `> 100` | `Error: Page must be at most 100` |
| `format` | `--format` が3値以外 | `Error: Invalid format '<値>'. Must be one of: table, simple, json` |

全て終了コード 1。

**注意（挙動の重複）**: バリデータ通過後、`parseCount()` が再度クランプする（`API_LIMITS`: DEFAULT_SEARCH_COUNT=20, MIN=1, MAX=100 / PAGE MIN=1, MAX=100）。バリデータが `parseInt` ベースなので `--number 5abc` は `parseInt` で 5 と解釈されて通る点に注意（`parseInt("5abc",10) === 5`）。同様に `--page 0` はバリデータで弾かれる。`--number` が空文字なら commander が値要求エラー。

### 1.4 Slack API

`search.messages`（`SearchOperations.searchMessages`）

送信パラメータ:

```
query    = options.query（そのまま。加工なし）
sort     = 'score' | 'timestamp'
sort_dir = 'asc' | 'desc'
count    = クランプ済み整数
page     = クランプ済み整数
```

レスポンスからの取り出し:

| 内部フィールド | 取得元 | フォールバック |
| --- | --- | --- |
| `query` | `response.query` | 引数の `query` |
| `matches[]` | `response.messages.matches[]` の `text`/`user`/`username`/`ts`/`channel.id`/`channel.name`/`permalink` | `[]` |
| `totalCount` | `response.messages.pagination.total_count` | `0` |
| `page` | `response.messages.pagination.page` | `1` |
| `pageCount` | `response.messages.pagination.page_count` | `0` |

`rateLimiter` は**通していない**（`searchMessages` は素の呼び出し）。

### 1.5 出力

タイムスタンプは `formatTimestampFixed`（`parseFloat(ts) * 1000` を **UTC** で `YYYY-MM-DD HH:MM:SS`）。

#### format=table（デフォルト）

chalk 付き。空行を含む。

```
（空行）
Search results for "deploy" (37 matches)     ← bold
Page 2/4                                     ← gray。pageCount > 1 のときのみ
（空行）
[2026-08-17 04:12:33] #dev-acejob alice      ← 時刻gray / チャンネルblue / ユーザーcyan
デプロイ完了しました
https://example.slack.com/archives/C123/p1755... ← gray。permalink があるときのみ
（空行）
... （matchesの数だけ繰り返し）
Displayed 20 of 37 match(es)                 ← green
```

- match が0件のとき: 見出し行の直後に `No messages found`（yellow）を出して終了（`Page x/y` も `Displayed` 行も出ない）。
- チャンネル表示は `name` があれば `#<name>`、無ければ `channel.id`、それも無ければ `unknown`。
- ユーザー表示は `username` → `user` → `Unknown` の順。
- 本文が空なら `(no text)`。
- 本文は `sanitizeTerminalText`（改行を保持するため複数行になり得る）、それ以外は `sanitizeSingleLineText`。

#### format=simple

```
[#dev-acejob] alice (2026-08-17 04:12:33): デプロイ完了しました
[#random] bob (2026-08-16 11:02:00): 了解です
... and 17 more match(es)      ← totalCount > matches.length のときのみ
```

0件のときは `No messages found` のみ（色なし）。本文も `sanitizeSingleLineText`（1行に潰す）。

#### format=json

`JSON.stringify(sanitizeTerminalData(transform(...)), null, 2)`。形は:

```json
{
  "query": "deploy",
  "totalCount": 37,
  "page": 2,
  "pageCount": 4,
  "matches": [
    {
      "channel": "dev-acejob",
      "username": "alice",
      "timestamp": "2026-08-17 04:12:33",
      "text": "デプロイ完了しました",
      "permalink": "https://..."
    }
  ]
}
```

- `channel` は **`#` を付けない**（`name || id || 'unknown'`）。table/simple と表記が違う。
- 0件でも `matches: []` を含む完全なJSONを出す（`No messages found` は出ない）。

#### 不明なフォーマット

`FormatterFactory.create()` は未知のキーなら table にフォールバックする。ただし preAction で弾かれるため到達しない。

### 1.6 エラー

| ケース | 出力 | 終了コード |
| --- | --- | --- |
| `-q` 未指定 | commander の `required option '-q, --query <query>' not specified` 相当 | 1 |
| バリデーション失敗 | 1.3 の表のメッセージ | 1 |
| 設定なし | `✗ Error: No configuration found for profile "..."...` | 1 |
| Slack APIエラー | `✗ Error: <error.message>`。`missing_scope` かつ `needed` があるときは `<message> (needed: search:read, ...)` | 1 |

`extractErrorMessage` は `error.data.error === 'missing_scope'`、または message に `missing_scope` を含む場合に needed スコープを付記する。

### 1.7 ページネーション・レート制限・並行

- ページ送りは**しない**。指定 `page` の1リクエストのみ。全件取得したければ利用者が `--page` を回す。
- リトライなし（WebClient の retries=0、`searchMessages` は独自リトライを持たない）。※`fetchSearchPage`（unread 用の private）だけは rate limit 文字列を見て最大3回リトライ＋5秒待機するが、`search` コマンドからは呼ばれない。
- 並行実行なし。

---

## 2. `slack-cli users`

親: description `List, search, and get information about workspace users`。

### 2.1 `users list`

description: `List workspace users`

| ロング | ショート | 型 | デフォルト | 必須 |
| --- | --- | --- | --- | --- |
| `--limit <number>` | なし | string | `'100'`（commander のデフォルト値として設定） | 任意 |
| `--format <format>` | なし | string | `table` | 任意 |
| `--profile <profile>` | なし | string | なし | 任意 |

バリデーション: `format` のみ。**`--limit` は一切検証されない**。`parseLimit` は `parseInt(limit ?? '100', 10)` で、`--limit abc` は `NaN` になる。`UserOperations.listUsers` は `!Number.isFinite(limit) || limit <= 0` のとき **空配列を返す**（＝`No users found` が出て終了コード 0）。

API: `users.list`

```
limit  = 200（固定。CLIの --limit とは別物）
cursor = 前ページの response_metadata.next_cursor（2ページ目以降）
```

ページネーション: `next_cursor` が尽きるまでループ。ただし累積件数が CLI の `--limit` 以上になった時点で `slice(0, limit)` して即 return。`rateLimiter` は通していない（逐次ループ）。

出力:

- 0件: `No users found`（プレーン）で return。
- **table**: `console.table` に以下の列のオブジェクト配列を渡す。列順は `id, name, real_name, email, is_bot, deleted`。`is_bot`/`deleted` は `Yes`/`No` 文字列。値は `sanitizeTerminalText` 済み＋`sanitizeTerminalData` 済み。`console.table` は Node が罫線付きテーブル（`┌─────┬` 形式、先頭に `(index)` 列）を描く。
  ```
  ┌─────────┬─────────────┬─────────┬─────────────┬───────────────────┬────────┬─────────┐
  │ (index) │ id          │ name    │ real_name   │ email             │ is_bot │ deleted │
  ├─────────┼─────────────┼─────────┼─────────────┼───────────────────┼────────┼─────────┤
  │ 0       │ 'U012ABC'   │ 'alice' │ 'Alice A'   │ 'alice@ex.com'    │ 'No'   │ 'No'    │
  └─────────┴─────────────┴─────────┴─────────────┴───────────────────┴────────┴─────────┘
  ```
  （罫線の正確な体裁は Node のバージョン依存。Rust側で1:1再現するなら要検討。）
- **simple**: 1行1ユーザーのTSV。`<id>\t<name>\t<real_name>` + email があれば末尾に ` <email>`（空白＋山括弧）。値は `sanitizeSingleLineText`。
  ```
  U012ABC	alice	Alice Anderson <alice@example.com>
  U345DEF	bot	Deploy Bot
  ```
- **json**: `renderByFormat` の既定 json 分岐 = `JSON.stringify(sanitizeTerminalData(users), null, 2)`。**Slack APIのユーザーオブジェクトをそのまま**出す（`SlackUser` 型で宣言されたフィールドだけではなく、レスポンスの全フィールドが残る点に注意）。

### 2.2 `users info`

description: `Get detailed information about a user`

| ロング | ショート | 型 | デフォルト | 必須 |
| --- | --- | --- | --- | --- |
| `--id <userId>` | なし | string | なし | **必須**（`requiredOption`） |
| `--format <format>` | なし | string | `table` | 任意 |
| `--profile <profile>` | なし | string | なし | 任意 |

API: `users.info`、パラメータ `user = options.id`。**`rateLimiter` を通す唯一の users 系呼び出し**（`this.rateLimiter(() => client.users.info(...))`）。

出力:

- **table**（`renderUserInfo`）: ラベル揃えのキー・バリュー。表ではない。
  ```
  ID:           U012ABC
  Name:         alice
  Real Name:    Alice Anderson
  Display Name: alice
  Email:        alice@example.com
  Title:        Engineer
  Timezone:     Asia/Tokyo (Japan Standard Time)
  Status:       :palm_tree: 休暇中
  Admin:        No
  Bot:          No
  Deleted:      No
  ```
  - ラベル幅は固定文字列（`ID:` の後に11スペースなど、ソース上べた書き）。
  - Timezone行は `<tz> (<tz_label>)`。両方空でも ` ()` が出る。
  - Status行は `<status_emoji> <status_text>`。両方空でも末尾に空白が残る。
- **simple**: `renderByFormat` に `simple` レンダラを渡していないため、**table と同じ出力になる**（`renderers.simple` が無いと table にフォールバック）。
- **json**: `JSON.stringify(sanitizeTerminalData(user), null, 2)`。APIのuserオブジェクトそのまま。

エラー: `--id` 未指定 → commander の required option エラー（終了1）。存在しないID → Slack が `user_not_found` を返し `✗ Error: <SDKのmessage>`、終了1。

### 2.3 `users lookup`

description: `Look up a user by email address`

| ロング | ショート | 型 | デフォルト | 必須 |
| --- | --- | --- | --- | --- |
| `--email <email>` | なし | string | なし | **必須** |
| `--format <format>` | なし | string | `table` | 任意 |
| `--profile <profile>` | なし | string | なし | 任意 |

**メールアドレス形式のバリデーションは無い**（`format` のみ検証）。

API: `users.lookupByEmail`、パラメータ `email = options.email`。rateLimiter なし。

出力: `users info` と完全に同じレンダラ（table = `renderUserInfo`、simple は table にフォールバック、json は生オブジェクト）。

エラー: 該当なし → Slack の `users_not_found` エラーがそのまま `✗ Error: ...` に出る。終了1。

### 2.4 `users presence`

description: `Check user presence status (active/away)`

| ロング | ショート | 型 | デフォルト | 必須 | 排他 |
| --- | --- | --- | --- | --- | --- |
| `--id <userId>` | なし | string | なし | 任意 | `--name` と**相互排他かつどちらか必須** |
| `--name <username>` | なし | string | なし | 任意 | 同上（`@username` 形式可） |
| `--format <format>` | なし | string | `table` | 任意 | |
| `--profile <profile>` | なし | string | なし | 任意 | |

排他チェックは preAction ではなく **action 内の素の `throw new Error`**（＝ `wrapCommand` が拾って `✗ Error:` 形式で出す。commander の `Error: ` 形式ではない）:

- 両方未指定: `✗ Error: You must specify either --id or --name` / 終了1
- 両方指定: `✗ Error: Cannot use both --id and --name` / 終了1

`--name` 指定時は `resolveUserIdByName`:

- 先頭の `@` を1つ剥がし、小文字化して `user.name` と**大文字小文字を無視して**比較（`real_name` や `display_name` は見ない）
- API: `users.list`（`limit: 200`、cursor でページ送り）を、一致が見つかるまで**全ページ走査**
- 見つからなければ `ApiError`: `User '<@抜きの名前>' not found` → `✗ Error: User 'alice' not found` / 終了1

その後 API: `users.getPresence`、パラメータ `user = <解決済みID>`。返すのは `{ presence: response.presence }` のみ（`online`/`away` など）。

出力（`{ userId, presence }` を渡す）:

- **table**: `console.table` に1行。列は `user`, `presence`。
  ```
  ┌─────────┬───────────┬──────────┐
  │ (index) │ user      │ presence │
  ├─────────┼───────────┼──────────┤
  │ 0       │ 'U012ABC' │ 'active' │
  └─────────┴───────────┴──────────┘
  ```
- **simple**: `<userId>\t<presence>` の1行。
- **json**: **`presence` オブジェクトのみ**を出す（userId は含まれない）。
  ```json
  {
    "presence": "active"
  }
  ```

---

## 3. `slack-cli usergroups`

親: description `List user groups and their members`。

### 3.1 `usergroups list`

description: `List user groups in the workspace`

| ロング | ショート | 型 | デフォルト | 必須 |
| --- | --- | --- | --- | --- |
| `--include-disabled` | なし | boolean フラグ（値なし） | 未指定時 `false` | 任意 |
| `--format <format>` | なし | string | `table` | 任意 |
| `--profile <profile>` | なし | string | なし | 任意 |

commander により `options.includeDisabled` になる。

API: `usergroups.list`

```
include_count    = true（常に）
include_disabled = true（--include-disabled 指定時のみキー自体を付ける。未指定ならキーごと送らない）
```

出力:

- 0件: `No usergroups found` で return（終了0）。
- **table**: `console.table`、列 `id, handle, name, description, user_count`。文字列は `sanitizeSingleLineText`、`user_count` は数値のまま（undefined なら空文字 `''`）。handle に `@` は付かない。
- **simple**: `<id>\t@<handle>\t<name>`（**simple のときだけ handle に `@` が付く**）。description と user_count は出ない。
  ```
  S012ABC	@engineers	Engineering Team
  ```
- **json**: `JSON.stringify(sanitizeTerminalData(usergroups), null, 2)`。APIの生オブジェクト配列。

ページネーション: なし（`usergroups.list` は1回のみ）。

### 3.2 `usergroups members`

description: `List members of a user group`

| ロング | ショート | 型 | デフォルト | 必須 | 排他 |
| --- | --- | --- | --- | --- | --- |
| `--id <usergroupId>` | なし | string | なし | 任意 | `--handle` と相互排他かつどちらか必須 |
| `--handle <handle>` | なし | string | なし | 任意 | 同上（`@engineers` 形式可） |
| `--format <format>` | なし | string | `table` | 任意 | |
| `--profile <profile>` | なし | string | なし | 任意 | |

排他チェックは action 内の `throw new Error`（`wrapCommand` 経由）:

- 両方未指定: `✗ Error: You must specify either --id or --handle` / 終了1
- 両方指定: `✗ Error: Cannot use both --id and --handle` / 終了1

`--handle` 指定時は `resolveUsergroupIdByHandle`:

- 先頭 `@` を剥がして小文字化、`usergroup.handle` と小文字比較
- 解決には `usergroups.list({ include_count: true, include_disabled: true })` を**常に disabled 込みで**呼ぶ
- 未発見: `ApiError`: `Usergroup '@<@抜きの名前>' not found`（メッセージ内に `@` が付く）/ 終了1

API呼び出し順:

1. （handle指定時のみ）`usergroups.list`
2. `usergroups.users.list`、パラメータ `usergroup = <ID>` → ユーザーIDの配列
3. メンバーIDそれぞれについて `users.info`（`user = <ID>`）

3の並行実行が重要:

```ts
await Promise.all(memberIds.map(async (userId) => { try { ... } catch { return { id: userId }; } }))
```

- `Promise.all` で**全件同時にキック**するが、`getUserInfo` は `this.rateLimiter`（pLimit(3)）を通るので**実効同時実行は3**。
- 個別の `users.info` が失敗しても握り潰して `{ id: userId }`（name/realName なし）にフォールバックする。全体は失敗しない。

出力（`createMembersFormatter`。`renderByFormat` は使わず専用フォーマッタ）:

- メンバー0件: `No members found` で return（終了0）。
- **table**（`console.table` ではなく手書き整形）:
  ```
  ID                Name              Real Name
  ────────────────────────────────────────────────────────────
  U012ABC           alice             Alice Anderson
  U345DEF           bob               Bob Brown
  ```
  - ヘッダは固定文字列 `ID                Name              Real Name`
  - 2行目は U+2500（`─`）を **60個**
  - 各行は `id.padEnd(17)` + 半角スペース1個 + `name.padEnd(17)` + 半角スペース1個 + `realName`
  - `padEnd` は **UTF-16コードユニット数**基準なので、日本語の real_name では見た目が揃わない
- **simple**: `<id>\t<name>\t<realName>`（値が無い場合は空文字。タブは常に2個）
- **json**:
  ```json
  [
    { "id": "U012ABC", "name": "alice", "real_name": "Alice Anderson" },
    { "id": "U345DEF", "name": "", "real_name": "" }
  ]
  ```
  （`realName` → `real_name` にキー名が変換される点に注意）

---

## 4. `slack-cli reaction`

親: description `Add or remove emoji reactions on messages`。

### 4.1 `reaction add` / `reaction remove`

description: `Add a reaction to a message` / `Remove a reaction from a message`。オプション構成は**完全に同一**。

| ロング | ショート | 型 | デフォルト | 必須 |
| --- | --- | --- | --- | --- |
| `--channel <channel>` | `-c` | string（チャンネル名 or ID） | なし | **必須** |
| `--timestamp <timestamp>` | `-t` | string | なし | **必須** |
| `--emoji <emoji>` | `-e` | string | なし | **必須** |
| `--profile <profile>` | なし | string | なし | 任意 |

相互排他関係はなし。

バリデーション（preAction, `optionValidators.reactionTimestamp`）:

- `--timestamp` が `/^\d{10}\.\d{6}$/` に一致しない → `Error: Invalid thread timestamp format` / 終了1
  （メッセージ文言は `ERROR_MESSAGES.INVALID_THREAD_TIMESTAMP`。「thread」と書かれているが reaction でもこの文言が出る）
- **10桁.6桁の固定形式のみ許容**。`1755400000.1` や11桁秒は弾かれる。

API:

1. `ChannelOperations.resolveChannelId(channel)`
   - `/^[CDG][A-Z0-9]{8,}$/` に一致すればそのままIDとして使う（API呼び出しなし）
   - 一致しなければ `conversations.list` を `types=public_channel,private_channel,im,mpim`, `exclude_archived=true`, `limit=1000` でカーソル全ページ取得し、名前で照合（完全一致 → `#` 除去一致 → 大文字小文字無視一致 → `name_normalized` 一致）
   - スコープ不足時は不足スコープに対応する types を落として再試行するフォールバックがある（`MISSING_SCOPE_TO_CHANNEL_TYPE`）
   - 見つからない場合:
     - 部分一致候補あり: `Channel '<name>' not found. Did you mean one of these? a, b, c`（最大5件）
     - 候補なし: `Channel '<name>' not found. Make sure you are a member of this channel.`
   - 解決結果はインスタンス内で `channelLookupCache` にキャッシュ（1プロセス1回）
2. `reactions.add` / `reactions.remove`
   ```
   channel   = 解決済みID
   timestamp = options.timestamp
   name      = emoji から先頭 ':' と末尾 ':' を1個ずつ除去した文字列
   ```
   `:tada:` でも `tada` でも同じ。中間のコロン（skin tone 記法など）は触らない。

出力（成功時、stdout、green）:

```
✓ Reaction :tada: added to message in #dev-acejob
✓ Reaction :tada: removed from message in #dev-acejob
```

- **`--emoji` の値をそのまま埋め込む**ため、`--emoji :tada:` と渡すと `:​:tada::` になる（コロンが二重）。
- **`--channel` の値をそのまま埋め込む**ため、ID指定なら `#C012ABC` と表示される。
- **サニタイズを通していない**（`sanitizeTerminalText` なし）。ユーザー入力そのままなので、Rust側で同じにするか改善するかは判断が要る。
- `--format` は存在しないので JSON 出力は不可。

エラー: `already_reacted` / `no_reaction` / `message_not_found` などは Slack SDK のエラーメッセージがそのまま `✗ Error: ...` に出る。終了1。個別ハンドリングは**していない**。

ページネーション・並行: チャンネル名解決のときだけ `conversations.list` を全ページ舐める。reaction 自体は1リクエスト。並行なし、リトライなし。

---

## 5. `slack-cli pin`

親: description `Add, remove, or list pinned messages in a channel`。

### 5.1 `pin add` / `pin remove`

description: `Pin a message in a channel` / `Unpin a message in a channel`。

| ロング | ショート | 型 | デフォルト | 必須 |
| --- | --- | --- | --- | --- |
| `--channel <channel>` | `-c` | string | なし | **必須** |
| `--timestamp <timestamp>` | `-t` | string | なし | **必須** |
| `--profile <profile>` | なし | string | なし | 任意 |

バリデーション: `optionValidators.pinTimestamp`。中身は `reactionTimestamp` と同一（`/^\d{10}\.\d{6}$/`、失敗時 `Error: Invalid thread timestamp format`、終了1）。

API:

1. `resolveChannelId`（4.1 と同じ手順・同じエラー文言）
2. `pins.add` / `pins.remove`、パラメータ `channel = 解決済みID`, `timestamp = options.timestamp`

出力（green、サニタイズなし、`--channel` の入力値をそのまま埋める）:

```
✓ Pin added to message in #dev-acejob
✓ Pin removed from message in #dev-acejob
```

`--format` なし。

### 5.2 `pin list`

description: `List pinned items in a channel`

| ロング | ショート | 型 | デフォルト | 必須 |
| --- | --- | --- | --- | --- |
| `--channel <channel>` | `-c` | string | なし | **必須** |
| `--format <format>` | なし | string | `table` | 任意 |
| `--profile <profile>` | なし | string | なし | 任意 |

バリデーション: `format` のみ。

API: `resolveChannelId` → `pins.list`（`channel = 解決済みID`）。返るのは `response.items`（無ければ `[]`）。ページネーションなし。

出力:

- 0件: `No pinned items found` で return（終了0）。
- `created`（Unix秒）は `new Date(created * 1000).toISOString()` = **ISO8601（例 `2026-08-17T04:12:33.000Z`）**。search の `formatTimestampFixed` とは形式が違う点に注意。`created` が falsy（0/undefined）なら空文字。
- **table**: `console.table`、列 `type, created, created_by, ts, text`。
  - `type` は `item.type || 'unknown'`
  - `created_by` / `ts` / `text` は `sanitizeTerminalText`（**改行が残る**。`sanitizeSingleLineText` ではない）
  ```
  ┌─────────┬──────────────────────────┬────────────┬──────────────────┬──────────────┐
  │ (index) │ type      │ created                  │ created_by │ ts               │ text │
  ...
  ```
- **simple**: `<created> <ts> <text>`（半角スペース区切り、created が空なら先頭が空白始まりになる）
  ```
  2026-08-17T04:12:33.000Z 1755403953.123456 デプロイ完了しました
  ```
- **json**: `JSON.stringify(sanitizeTerminalData(items), null, 2)` = **Slack APIの items 生オブジェクト**（`created` は整形されず Unix秒のまま）。

`pin list` は `renderByFormat` を使わず、action 内で `format === 'json'` / `'simple'` / それ以外（table）を直接分岐している。

---

## 6. エラーメッセージ一覧（対象コマンド全体）

| 発生源 | 文言 | 経路 | 終了コード |
| --- | --- | --- | --- |
| commander | `error: required option '-q, --query <query>' not specified` 等 | stderr | 1 |
| validators | `Error: Invalid format '<v>'. Must be one of: table, simple, json` | `command.error()` | 1 |
| validators | `Error: Invalid sort '<v>'. Must be one of: score, timestamp` | 同上 | 1 |
| validators | `Error: Invalid sort direction '<v>'. Must be one of: asc, desc` | 同上 | 1 |
| validators | `Error: Count must be a number` / `Count must be at least 1` / `Count must be at most 100` | 同上 | 1 |
| validators | `Error: Page must be a number` / `Page must be at least 1` / `Page must be at most 100` | 同上 | 1 |
| validators | `Error: Invalid thread timestamp format`（reaction / pin の `--timestamp`） | 同上 | 1 |
| action内 throw | `✗ Error: You must specify either --id or --name` | wrapCommand | 1 |
| action内 throw | `✗ Error: Cannot use both --id and --name` | wrapCommand | 1 |
| action内 throw | `✗ Error: You must specify either --id or --handle` | wrapCommand | 1 |
| action内 throw | `✗ Error: Cannot use both --id and --handle` | wrapCommand | 1 |
| ConfigurationError | `✗ Error: No configuration found for profile "<p>". Use "slack-cli config set --token <token> --profile <p>" to set up.` | wrapCommand | 1 |
| ApiError | `✗ Error: User '<name>' not found` | wrapCommand | 1 |
| ApiError | `✗ Error: Usergroup '@<handle>' not found` | wrapCommand | 1 |
| ApiError | `✗ Error: Channel '<name>' not found. Did you mean one of these? a, b` | wrapCommand | 1 |
| ApiError | `✗ Error: Channel '<name>' not found. Make sure you are a member of this channel.` | wrapCommand | 1 |
| Slack SDK | `✗ Error: <SDKのmessage>`（`missing_scope` 時は ` (needed: x, y)` を付記） | wrapCommand | 1 |

「情報なし」系（`No users found` / `No usergroups found` / `No members found` / `No pinned items found` / `No messages found`）は**エラーではなく stdout、終了コード 0**。

---

## 7. ページネーション・レート制限・並行実行のまとめ

| コマンド | ページネーション | 並行 | リトライ |
| --- | --- | --- | --- |
| `search` | なし（`--page` で利用者が明示。1リクエストのみ） | なし | なし |
| `users list` | `users.list` を cursor で全ページ。ただし `--limit` 到達で打ち切り。ページサイズは常に200 | なし（逐次） | なし |
| `users info` | なし | pLimit(3) を通す（実質1本） | なし |
| `users lookup` | なし | なし | なし |
| `users presence` | `--name` 時に `users.list` を一致するまで全ページ走査 | なし | なし |
| `usergroups list` | なし | なし | なし |
| `usergroups members` | なし（`usergroups.users.list` は1回） | `Promise.all` × pLimit(3) で `users.info` を並列。個別失敗は握り潰し | なし |
| `reaction add/remove` | チャンネル名解決時に `conversations.list` 全ページ | なし | なし |
| `pin add/remove/list` | 同上 | なし | なし |

全体共通:

- WebClient は `retryConfig.retries = 0`。**429 の自動リトライは無効**。
- `pLimit(3)` は SlackApiClient インスタンス単位で共有（`createSlackClientContext`）。CLI は1コマンド1インスタンスなので実質そのコマンド内で共有。
- `handleRateLimit` は message に `rate limit` を含むとき5秒待つだけ。本書の対象コマンドからは呼ばれない（`listUnreadChannels` 系のみ）。
- `Retry-After` ヘッダの参照はソース上に見当たらない（SDK内部の挙動は未読・不明）。

---

## 8. Rust移植で引っかかりそうな点

### 8.1 CLIパーサ（clap）まわり

1. **`console.table` の再現**。`users list` / `users presence` / `pin list` の table 出力は Node の `console.table` に丸投げで、罫線・`(index)` 列・文字列の**シングルクォート囲み**・数値のクォートなしという Node 固有の体裁になる。clap には相当物がないので、`comfy-table` 等で描くと出力が変わる。**1:1互換を狙うなら `console.table` の出力仕様を別途固めるか、互換を諦める判断が要る。**
2. **`padEnd` は UTF-16 コードユニット基準**。`usergroups members` の table は日本語 real_name で崩れる。Rust の `str::len()` はバイト数、`chars().count()` はコードポイント数で、いずれも UTF-16 とは一致しない。既存の崩れ方まで再現するか、`unicode-width` で直すかを決める必要がある。
3. **オプション名のケバブ→キャメル変換**。`--sort-dir` → `sortDir`、`--include-disabled` → `includeDisabled`。clap では明示的にフィールド名を付けるだけなので問題は起きないが、JSON出力のキー名との対応は要確認。
4. **required option のエラー文言**が commander 固有。clap のデフォルトメッセージと異なる。互換テストを書くなら文言を上書きする必要がある。
5. **`--limit` に型検証がない**。`users list --limit abc` は現状「エラーにならず空リストで正常終了」。clap で `value_parser!(u32)` を付けると挙動が変わる（エラー終了になる）。**意図的に緩いのか事故なのかはソースからは判断できない（不明）。互換を取るならパース失敗時に空リスト＋終了0にする必要がある。**
6. **`parseInt` の前方一致パース**。`--number 5abc` → 5、`--page 3.7` → 3。Rust の `str::parse::<i64>()` はどちらもエラーになる。search の count/page 互換には自前の前方数値パースが要る。

### 8.2 バリデーション順序と経路の違い

7. **2種類のエラー経路が混在**。preAction バリデータは `Error: <msg>`（commander の書式、stderr）、action 内 throw は `✗ Error: <msg>`（赤、chalk）。`users presence` と `usergroups members` の排他チェックだけが後者。終了コードはどちらも1だが、**stderr の文字列が違う**ので互換テストで効いてくる。
8. **バリデータは最初の失敗で打ち切り**（`break`）。複数エラーがあっても1件しか出ない。clap はデフォルトで複数まとめて出す場合があるので要調整。
9. **バリデータ通過後に `parseCount` が再クランプ**という二重防御。ロジックを1箇所にまとめると、境界値の挙動（例: バリデータをすり抜けた値）が変わる可能性がある。

### 8.3 出力フォーマット

10. **format=simple が table にフォールバックするケースがある**。`users info` / `users lookup` は `simple` レンダラ未定義なので table と同一出力。**バグに見えるが現行仕様。**
11. **json出力の中身がコマンドで不統一**。search と usergroups members は変換済みDTO、users list/info/lookup と usergroups list と pin list は Slack API の生レスポンス。生レスポンスを返す側は、Rust で型を切ると**未知フィールドが落ちる**。`serde_json::Value` のまま通すか、`#[serde(flatten)] extra` を持たせる設計が要る。
12. **`users presence --format json` は presence だけ**を出し userId を含まない。table/simple とキーが揃っていない。
13. **タイムスタンプ形式が2種類**。search は `formatTimestampFixed`（UTC `YYYY-MM-DD HH:MM:SS`）、pin は `toISOString()`（`...T...Z`、ミリ秒付き）。chrono で書き分ける。
14. **chalk のカラーは TTY 判定で自動的に落ちる**（パイプ時は色なし）。Rust 側も同等の判定（`anstream` / `owo-colors` + `is-terminal`）が要る。`NO_COLOR` / `FORCE_COLOR` の扱いは chalk 依存で、TSソースには明示がない（不明）。
15. **`search` の table 出力は先頭に空行、matches ごとに末尾空行**。空行の数は互換テストで効く。

### 8.4 サニタイズとセキュリティ

16. **`sanitizeTerminalText` の仕様を正確に移す**必要がある。ANSI/OSCの正規表現、制御文字の範囲（`<0x20`、`0x7F`、`0x80–0x9F`）、タブと改行だけ残す点。Rust では `char` 単位で回せるが、TS の `for...of` は**コードポイント単位**（サロゲートペアを1文字として扱う）なので Rust の `chars()` と一致する。
17. **`sanitizeTerminalData` はプレーンオブジェクトだけ再帰する**。`serde_json::Value` で実装すると自然に一致するが、「数値・booleanは触らない」を守ること。
18. **`reaction` / `pin` の成功メッセージだけサニタイズされていない**。`--channel` / `--emoji` の値がそのまま stdout に出るので、エスケープシーケンスを含む引数で端末を汚せる。**現行と同じにするか、移植時に直すかを明示的に決めるべき。**
19. **`redactSlackTokens` はエラー経路にしか掛かっていない**。正規表現 `xox[bpoars]-[A-Za-z0-9-]+` と置換後の `<先頭4文字小文字>-***-REDACTED` を正確に移す。
20. **サニタイズ順序**: `redactSlackTokens(sanitizeTerminalText(msg))` の順（コメントに「エスケープで分断されたトークンも伏せるため先にサニタイズ」と明記されている）。逆にすると抜ける。

### 8.5 API層

21. **`@slack/web-api` 相当が Rust に無い**。`reqwest` で `search.messages` / `users.list` / `users.info` / `users.lookupByEmail` / `users.getPresence` / `usergroups.list` / `usergroups.users.list` / `reactions.add` / `reactions.remove` / `pins.add` / `pins.remove` / `pins.list` / `conversations.list` を自前で叩く。**`ok: false` のときの `error` / `needed` フィールド取り出し**（`extractErrorMessage` が依存）まで含めて実装が要る。
22. **`pLimit(3)` の再現**。`tokio::sync::Semaphore` で置き換えるのが素直。ただし現状 rateLimiter を通しているのは `users.info` だけで、他は素通しという**中途半端な状態**をそのまま写すのか統一するのかを決める必要がある。
23. **`usergroups members` の並行 + 個別失敗の握り潰し**。`futures::future::join_all` + 各 future 内で `Result` を握って `MemberInfo { id }` にフォールバック。**結果の順序は入力順を保つこと**（`Promise.all` は順序保証）。
24. **`channelLookupCache` はインスタンス寿命のキャッシュ**で、失敗時はキャッシュを破棄して次回再取得する（`.catch()` で `undefined` に戻す）。CLI は1コマンド1プロセスなので実質「1プロセス内で1回だけ取得」。Rust では `OnceCell`/`Mutex<Option<...>>` 相当だが、**失敗時にクリアする**挙動を落とさないこと。
25. **`missing_scope` フォールバック**（`conversations.list` のスコープ不足時に types を削って再試行）は `channel-operations.ts` にあり、reaction/pin のチャンネル名解決にも効く。移植対象に含める必要がある。
26. **`postAction` の `checkForUpdates`** が毎コマンド後に走る。ネットワークアクセスの有無・キャッシュ・失敗時の扱いは `update-notifier.ts` 未読のため**不明**。移植時に別途仕様化が要る。
27. **設定の読み込みとトークン復号**（`ProfileConfigManager` / `token-crypto-service.ts`）は本書の対象外。`getConfig` が「旧形式・平文トークンを見つけたら再暗号化して書き戻す」副作用を持つことだけソース上で確認できた。詳細は未読・不明。

### 8.6 その他の未確認事項（不明）

- `--help` / `--version` の正確な出力体裁（commander 依存）
- 親コマンド（`users` 等）を引数なしで実行したときの終了コード
- `NO_COLOR` などの環境変数の扱い
- `NODE_ENV=development` 以外のデバッグ手段の有無
- Slack API のレスポンスに含まれる未宣言フィールドの実際の内容（型定義は部分的）
