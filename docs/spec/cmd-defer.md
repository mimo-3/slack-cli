# コマンド仕様: `draft` / `scheduled` / `reminder`

移植元: `/Users/mimo/organizations/open-source/slack-cli`（package.json version `0.24.1`）
対象ファイル: `src/commands/draft.ts`, `src/commands/scheduled.ts`, `src/commands/reminder.ts`

本書は上記3ファイルと、そこから辿れるヘルパー（`src/utils/` 配下、`src/types/commands.ts`, `src/types/slack.ts`）を実際に読んで抽出した事実のみを記載する。読んでいない範囲は「不明」と明記する。

---

## 0. 全体像（`src/index.ts`）

- CLI 名: `slack-cli`、説明 `CLI tool to send messages via Slack API`、`--version` は package.json の version。
- commander の `program.addCommand()` で 25 個のトップレベルコマンドを登録。本書の対象は `scheduled`（登録順6番目）、`reminder`（22番目）、`draft`（25番目、最後）。
- `program.hook('postAction', ...)` で全コマンド実行後に `checkForUpdates({ packageName, currentVersion })` を呼ぶ（更新通知。実装詳細 `src/utils/update-notifier.ts` は未読のため挙動の詳細は不明）。
- エントリ: `runCli(argv = process.argv)` が `program.parseAsync(argv)` を await。`require.main === module` のときのみ自動実行。
- 3コマンドとも**トップレベルコマンド自体にはアクションがない**（サブコマンド専用の親コマンド）。サブコマンド無指定時の挙動は commander のデフォルト（ヘルプ表示）に依存。

### 共通の実行ラッパー

| 要素 | 実体 | 挙動 |
| --- | --- | --- |
| `wrapCommand(action)` | `src/utils/command-wrapper.ts` | action を try/catch。例外時に `console.error(chalk.red('✗ Error:'), redactSlackTokens(sanitizeTerminalText(extractErrorMessage(error))))` を出し、`NODE_ENV === 'development'` かつ Error なら stack も gray で出力、その後 `process.exit(1)` |
| `withSlackClient(options, fn)` | `src/utils/command-support.ts` | `parseProfile(options.profile)` → `createSlackClient(profile)` → `fn(client)` |
| `createSlackClient(profile)` | `src/utils/client-factory.ts` | `getConfigOrThrow(profile)` でトークン取得 → `new SlackApiClient(config.token)` |
| `getConfigOrThrow` | `src/utils/config-helper.ts` | 設定が無ければ `ConfigurationError`。文言は `No configuration found for profile "<name>". Use "slack-cli config set --token <token> --profile <name>" to set up.`（`<name>` は指定 profile、無ければデフォルトプロファイル名、無ければ `default`） |
| `renderByFormat(options, data, renderers)` | `src/utils/command-support.ts` | `parseFormat(options.format)`（未指定なら `table`）。`json` なら renderers.json、無ければ `console.log(JSON.stringify(sanitizeTerminalData(data), null, 2))`。`simple` かつ renderer があれば simple、それ以外は table |
| `createValidationHook([...])` | `src/utils/validators.ts` | commander の `preAction` フック。`thisCommand.opts()` を各バリデータに渡し、文字列が返ったら `thisCommand.error(\`Error: ${error}\`)`（commander の既定でプロセス終了。既定終了コード 1） |
| `sanitizeTerminalText` / `sanitizeTerminalData` / `sanitizeSingleLineText` | `src/utils/terminal-sanitizer.ts` | ANSI/OSC エスケープ列を除去し、C0 制御（tab/LF 除く）・DEL・C1 制御を除去。`sanitizeSingleLineText` はさらに空白連続を単一スペースに畳んで trim |

### Slack クライアントの共通挙動（`src/utils/slack-operations/base-client.ts`）

- `@slack/web-api` の `WebClient` を `retryConfig: { retries: 0 }`（自動リトライ無効）、`logLevel: ERROR` で生成。
- `pLimit(RATE_LIMIT.CONCURRENT_REQUESTS)` = 同時実行 3 のリミッタを保持。ただし本書の対象コマンドが使う API 呼び出しのうち、リミッタを通しているのは `users.info`（`getUserInfo`）だけで、**`chat.*` / `reminders.*` / `users.list` / `conversations.open` は素通し**。
- `handleRateLimit(error)`（`rate limit` を含むメッセージなら 5 秒待つ）は定義されているが、対象コマンド経路からは呼ばれていない。

---

## 1. `slack-cli draft`

親コマンド説明: `Manage message drafts (save, list, show, send, delete)`。エイリアスは**なし**。
サブコマンド: `save` / `list` / `show` / `send` / `delete`（いずれもエイリアスなし、位置引数なし）。

ドラフトは Slack API ではなく**ローカルファイル**に保存される（`src/utils/draft-store.ts`）。

### 1.1 ドラフトストア仕様（`DraftStore`）

| 項目 | 内容 |
| --- | --- |
| 保存先 | `<configDir>/drafts.json`。`configDir` 既定は `path.join(os.homedir(), '.slack-cli')` |
| レコード型 | `{ id: string; channel?: string; user?: string; message: string; thread?: string; createdAt: string }`（`createdAt` は `new Date().toISOString()`） |
| ID 生成 | `randomBytes(4).toString('hex')`（8桁 hex）。既存 ID と衝突する限り再生成 |
| 読み込み | JSON.parse。配列でなければ `[]`。要素は `typeof entry === 'object' && entry !== null && typeof entry.id === 'string' && typeof entry.message === 'string'` を満たすものだけ残す（不正要素は黙って捨てる）。`ENOENT` は `[]`、その他の I/O エラーは throw |
| 書き込み | `mkdir(configDir, { recursive: true, mode: 0o700 })` → `chmod(configDir, 0o700)` → 一時ファイル `<draftsPath>.<pid>.<Date.now()>.tmp` に `flag: 'wx'`, `mode: 0o600` で書き、`rename` で置換。rename 失敗時は一時ファイルを unlink して throw |
| 追記位置 | `save` は末尾 push（順序は挿入順） |
| `delete` | ID 不一致で件数が変わらなければ `ValidationError(\`Draft not found: ${id}\`)` |

### 1.2 `draft save`

説明: `Save a message as a local draft`

| ロング | ショート | 型 | 既定 | 必須 | 備考 |
| --- | --- | --- | --- | --- | --- |
| `--channel <channel>` | `-c` | string | なし | 条件付き | `--user` と**相互排他**。どちらか一方が必須 |
| `--user <username>` | なし | string | なし | 条件付き | 同上 |
| `--message <message>` | `-m` | string | なし | 実質必須 | commander 上は任意だがアクション内で必須検査 |
| `--thread <thread>` | `-t` | string | なし | 任意 | スレッド ts |

- 位置引数: なし。`--profile` は**持たない**（API を叩かないため）。
- preAction バリデーション: `optionValidators.threadTimestamp` — `--thread` があるとき `^\d{10}\.\d{6}$` にマッチしなければエラー（文言は `ERROR_MESSAGES.INVALID_THREAD_TIMESTAMP`。定数の実文字列は未確認＝不明）。
- Slack API 呼び出し: **なし**。
- 標準出力（フォーマット指定なし）:
  `✓ Draft saved (id: 1a2b3c4d, target: #general)`（緑）。target は `draft.user ? '@'+user : '#'+channel`。

エラー:

| 条件 | 文言 | 経路 / 終了コード |
| --- | --- | --- |
| `--channel` も `--user` も無い | `Either --channel or --user must be specified` | ValidationError → `✗ Error: ...` / 1 |
| 両方指定 | `Cannot specify both --channel and --user` | 同上 / 1 |
| `--message` 無し | `--message is required` | 同上 / 1 |
| `--thread` 形式不正 | `Error: <INVALID_THREAD_TIMESTAMP>` | commander `.error()` / 1 |
| ファイル書き込み失敗 | Node の I/O エラーメッセージがそのまま | wrapCommand / 1 |

### 1.3 `draft list`

説明: `List saved drafts`

| ロング | ショート | 型 | 既定 | 必須 |
| --- | --- | --- | --- | --- |
| `--format <format>` | なし | `table` \| `simple` \| `json` | `table` | 任意 |

- preAction: `optionValidators.format` — 値が `table|simple|json` 以外なら `Invalid format '<value>'. Must be one of: table, simple, json`。
- Slack API 呼び出し: なし。
- 0 件のとき: `No drafts found`（プレーン、format 指定に関係なく先に return）。

出力形式:

- `table`（既定）: `console.table` に `{ id, target, created_at, message }` の配列を渡す。`message` は 60 文字超なら先頭 60 文字 + `...`。Node の `console.table` は罫線付きテーブル（`(index)` 列を含む）を出す。
  ```
  ┌─────────┬────────────┬──────────┬────────────────────────────┬─────────┐
  │ (index) │ id         │ target   │ created_at                 │ message │
  ├─────────┼────────────┼──────────┼────────────────────────────┼─────────┤
  │ 0       │ '1a2b3c4d' │ '#general' │ '2026-01-01T00:00:00.000Z' │ 'hello' │
  └─────────┴────────────┴──────────┴────────────────────────────┴─────────┘
  ```
  （罫線・引用符の正確な描画は Node の `console.table` 実装に依存）
- `simple`: 1件1行、半角スペース区切りで `<id> <createdAt> <target> <message>`（message は truncate されない）。
  ```
  1a2b3c4d 2026-01-01T00:00:00.000Z #general hello world
  ```
- `json`: 専用 renderer が無いため `renderByFormat` の既定分岐。**Draft オブジェクトの配列をそのまま**（キーは `id`, `channel`/`user`, `message`, `thread`, `createdAt` の camelCase）2スペースインデントで出力。
  ```json
  [
    {
      "channel": "general",
      "message": "hello world",
      "id": "1a2b3c4d",
      "createdAt": "2026-01-01T00:00:00.000Z"
    }
  ]
  ```
  ※ `save` 時のオブジェクト構築が `{ ...input, id, createdAt }` のため、**キー順は channel/user/message/thread → id → createdAt** になる。

### 1.4 `draft show`

説明: `Show the full content of a draft`

| ロング | ショート | 型 | 既定 | 必須 |
| --- | --- | --- | --- | --- |
| `--id <draftId>` | なし | string | なし | **必須**（`requiredOption`） |

- Slack API 呼び出し: なし。フォーマット指定なし（常に固定書式）。
- 出力:
  ```
  id: 1a2b3c4d
  target: #general
  thread: 1700000000.123456      ← draft.thread がある場合のみ
  created_at: 2026-01-01T00:00:00.000Z
  ---
  hello world
  ```
- エラー: 見つからない → `Draft not found: <id>` (ValidationError) → `✗ Error: Draft not found: <id>` / 終了コード 1。`--id` 未指定 → commander の required option エラー / 1。

### 1.5 `draft send`

説明: `Send a saved draft`

| ロング | ショート | 型 | 既定 | 必須 |
| --- | --- | --- | --- | --- |
| `--id <draftId>` | なし | string | なし | **必須** |
| `--keep` | なし | boolean フラグ | `false`（未指定時 undefined） | 任意 |
| `--profile <profile>` | なし | string | なし | 任意 |

処理順とAPI:

1. ローカルから draft 取得。無ければ `Draft not found: <id>`。
2. `withSlackClient` でクライアント生成（profile 解決 → トークン取得）。
3. `draft.user` がある場合:
   - `resolveUserIdByName(draft.user)`: 先頭 `@` を除去し小文字比較。`users.list({ limit: 200, cursor? })` を `response_metadata.next_cursor` が尽きるまでループし、`member.name` の小文字一致で ID を返す。見つからなければ `ApiError(\`User '<name>' not found\`)`。
   - `openDmChannel(userId)`: `conversations.open({ users: userId })` → `channel.id`。
4. `draft.user` が無い場合: **`draft.channel` をそのまま宛先に使う**（チャンネル名→ID の解決を行わない）。
5. `sendMessage(targetChannel, draft.message, draft.thread)` → `chat.postMessage({ channel, text, ...(thread_ts ? { thread_ts } : {}) })`（blocks は渡さない）。

| Slack API | パラメータ |
| --- | --- |
| `users.list` | `limit: 200`, `cursor`（2ページ目以降）※ `--user` 指定時のみ |
| `conversations.open` | `users: <userId>` ※ `--user` 指定時のみ |
| `chat.postMessage` | `channel`, `text`, `thread_ts`（draft.thread がある場合のみ） |

出力:

```
✓ Draft sent to #general        （緑）
Draft 1a2b3c4d deleted          （グレー。--keep 未指定かつ削除成功時）
```

`--keep` 未指定で削除に失敗した場合は `⚠ Message sent, but failed to delete draft <id>`（黄）を出すが、**終了コードは 0 のまま**（catch して握りつぶす）。

エラー:

| 条件 | 文言 | 終了コード |
| --- | --- | --- |
| draft 不在 | `✗ Error: Draft not found: <id>` | 1 |
| 設定なし | `✗ Error: No configuration found for profile "<name>". ...` | 1 |
| ユーザー不在 | `✗ Error: User '<name>' not found` | 1 |
| Slack API エラー | `✗ Error: <WebClient のエラーメッセージ>`。`missing_scope` かつ `data.needed` があると `<message> (needed: a, b)` が付く | 1 |

### 1.6 `draft delete`

説明: `Delete a saved draft`

| ロング | ショート | 型 | 既定 | 必須 |
| --- | --- | --- | --- | --- |
| `--id <draftId>` | なし | string | なし | **必須** |

- Slack API 呼び出し: なし。
- 出力: `✓ Draft <id> deleted`（緑）。
- エラー: `Draft not found: <id>` → `✗ Error: ...` / 1。

---

## 2. `slack-cli scheduled`

親コマンド説明: `Manage scheduled messages (list, cancel)`。エイリアスなし。
サブコマンド: `list` / `cancel`。位置引数はどちらも無し。

### 2.1 `scheduled list`

説明: `List scheduled messages`

| ロング | ショート | 型 | 既定 | 必須 |
| --- | --- | --- | --- | --- |
| `--channel <channel>` | `-c` | string（名前または ID） | なし | 任意（フィルタ） |
| `--limit <number>` | なし | string→`parseInt(…,10)` | `'50'`（commander 既定）、`parseLimit` の第2引数も 50 | 任意 |
| `--format <format>` | なし | `table` \| `simple` \| `json` | `table` | 任意 |
| `--profile <profile>` | なし | string | なし | 任意 |

- 相互排他: なし。
- preAction: `optionValidators.format` のみ。**`--limit` は一切検証されない**（`parseInt` が NaN を返しても素通しで API に渡る）。
- API:

| Slack API | パラメータ |
| --- | --- |
| `conversations.list`（`--channel` にチャンネル**名**を渡したときのみ、内部の ID 解決で使用） | `types: 'public_channel,private_channel,im,mpim'`, `exclude_archived: true`, `limit: 1000`, `cursor`（next_cursor が尽きるまでループ） |
| `chat.scheduledMessages.list` | `limit: <parseLimit の結果>`, `channel: <解決済みID>`（`--channel` 指定時のみ） |

- チャンネル解決（`ChannelResolver`）: `^[CDG][A-Z0-9]{8,}$` にマッチすれば ID とみなしてそのまま使用。そうでなければ全チャンネルを取得して名前一致（完全一致 / `#` 除去 / 大文字小文字無視 / `name_normalized` 一致）。見つからないとき、部分一致候補があれば `Channel '<name>' not found. Did you mean one of these? a, b`、無ければ `Channel '<name>' not found. Make sure you are a member of this channel.`（いずれも `ApiError`）。
- `conversations.list` が `missing_scope` で失敗したときは、不足スコープに対応するチャンネル種別（`channels:read`→public, `groups:read`→private, `im:read`→im, `mpim:read`→mpim）を除いて 1 回だけ再試行するフォールバックがある。
- 0 件のとき: `No scheduled messages found`。

出力形式（`post_at` は `new Date(post_at * 1000).toISOString()`）:

- `table`: `console.table` に `{ id, channel, post_at, text }`。`channel` は `channel_id` の生値（名前解決しない）。text は truncate しない。
- `simple`: `<post_at ISO> <channel_id> <id> <text>` をスペース区切りで1行ずつ。
  ```
  2026-02-01T09:00:00.000Z C0123456789 Q1234ABCD 明日の会議の件
  ```
- `json`: 専用 renderer なし → Slack API の `scheduled_messages` 要素をそのまま出力（`id`, `channel_id`, `post_at`, `date_created`, `text?`。post_at は epoch 秒の数値のまま、ISO 変換されない）。
  ```json
  [
    {
      "id": "Q1234ABCD",
      "channel_id": "C0123456789",
      "post_at": 1769936400,
      "date_created": 1769850000,
      "text": "明日の会議の件"
    }
  ]
  ```

### 2.2 `scheduled cancel`

説明: `Cancel a scheduled message`

| ロング | ショート | 型 | 既定 | 必須 |
| --- | --- | --- | --- | --- |
| `--channel <channel>` | `-c` | string | なし | **必須** |
| `--id <scheduledMessageId>` | なし | string | なし | **必須** |
| `--profile <profile>` | なし | string | なし | 任意 |

- preAction バリデーションなし。
- API: チャンネル名なら上記と同じ経路で `conversations.list` により ID 解決 → `chat.deleteScheduledMessage({ channel: <ID>, scheduled_message_id: <--id> })`。
- 出力: `✓ Scheduled message <id> cancelled`（緑）。
- エラー: 必須オプション欠落は commander（終了1）、チャンネル解決失敗・API エラーは `✗ Error: ...` で終了1。

---

## 3. `slack-cli reminder`

親コマンド説明: `Create, list, delete, or complete reminders`。エイリアスなし。
サブコマンド: `add` / `list` / `delete` / `complete`。位置引数はいずれも無し。
※ `scheduled` / `draft` と違い、`withSlackClient` ではなく `parseProfile` + `createSlackClient` を直接呼ぶ（実質同じ処理）。

### 3.1 `reminder add`

説明: `Create a new reminder`

| ロング | ショート | 型 | 既定 | 必須 |
| --- | --- | --- | --- | --- |
| `--text <text>` | なし | string | なし | **必須** |
| `--at <datetime>` | なし | string（例 `"2024-03-01 15:00"`） | なし | `--after` と択一 |
| `--after <minutes>` | なし | string（正の整数・分） | なし | `--at` と択一 |
| `--profile <profile>` | なし | string | なし | 任意 |

- 相互排他: `--at` と `--after` は**どちらか一方が必須、両方指定は不可**（`optionValidators.reminderTiming`）。
- 時刻解決（`resolvePostAt` → `parseScheduledTimestamp`）:
  - `--at` が全桁数字なら epoch 秒として `Number.parseInt`（safe integer でなければ null）。
  - それ以外は `Date.parse(trimmed)`（**JS の Date パーサ依存**）。NaN なら null。成功すれば `Math.floor(ms/1000)`。
  - `--after` は `^\d+$` かつ safe integer かつ > 0 のときのみ `Math.floor(Date.now()/1000) + minutes*60`。
- API: `reminders.add({ text, time })`。レスポンスの `reminder` を返す。
- 出力: `✓ Reminder created: "<reminder.text>" at <new Date(reminder.time*1000).toISOString()>`（緑）。**API のレスポンス値**を表示する点に注意。
  ```
  ✓ Reminder created: "ミーティング" at 2026-03-01T06:00:00.000Z
  ```

エラー:

| 条件 | 文言 | 終了コード |
| --- | --- | --- |
| `--at`・`--after` 両方なし | `Error: You must specify either --at or --after` | commander / 1 |
| 両方指定 | `Error: Cannot use both --at and --after` | commander / 1 |
| `--after` が正の整数でない | `Error: --after must be a positive integer (minutes)` | commander / 1 |
| `--text` 未指定 | commander の required option エラー | 1 |
| `--at` がパース不能 | `✗ Error: Could not resolve reminder time. Use --at or --after option.` | 1 |
| API エラー | `✗ Error: <message>` | 1 |

※ `--at` パース不能ケースは preAction を通り抜けるため、**クライアント生成（＝設定読み込み）の後**にエラーになる。設定が無い環境ではその前に設定エラーで落ちる。

### 3.2 `reminder list`

説明: `List all reminders`

| ロング | ショート | 型 | 既定 | 必須 |
| --- | --- | --- | --- | --- |
| `--format <format>` | なし | `table` \| `simple` \| `json` | `table` | 任意 |
| `--profile <profile>` | なし | string | なし | 任意 |

- preAction: `optionValidators.format`。
- API: `reminders.list()`（**引数なし**）。`response.reminders` が無ければ空配列。
- 0 件のとき: `No reminders found`。
- 出力は `createReminderFormatter(format)`（`src/utils/formatters/reminder-formatters.ts`）。`FormatterFactory.create()` は未知の format なら table にフォールバック（ただし preAction で弾かれる）。
- `Reminder` 型: `{ id, text, time, complete_ts, recurring }`。`status` は `complete_ts > 0 ? 'completed' : 'pending'`、`time` は ISO 文字列化。

出力形式:

- `table`: 罫線なしの固定幅パディング。列幅 id=14, text=30, time=26, status=10。ヘッダ行の下に `─`（U+2500）を 80 個。text は `sanitizeSingleLineText` 後 `slice(0, 28)` してから 30 桁パディング。id/time/status は truncate しない（幅を超えるとずれる）。
  ```
  ID            Text                          Time                      Status    
  ────────────────────────────────────────────────────────────────────────────────
  Rm123456      ミーティング                       2026-03-01T06:00:00.000Z  pending   
  ```
- `simple`: **タブ区切り** `<id>\t<text>\t<time ISO>\t<status>`（id/text は `sanitizeSingleLineText`）。
- `json`: `[{ id, text, time, time_formatted, status, recurring }]` を `sanitizeTerminalData` 通しで 2スペースインデント出力（`complete_ts` は出さず、`status` と `time_formatted` を足す）。
  ```json
  [
    {
      "id": "Rm123456",
      "text": "ミーティング",
      "time": 1772344800,
      "time_formatted": "2026-03-01T06:00:00.000Z",
      "status": "pending",
      "recurring": false
    }
  ]
  ```

### 3.3 `reminder delete`

| ロング | ショート | 型 | 既定 | 必須 |
| --- | --- | --- | --- | --- |
| `--id <reminderId>` | なし | string | なし | **必須** |
| `--profile <profile>` | なし | string | なし | 任意 |

- API: `reminders.delete({ reminder: <id> })`。
- 出力: `✓ Reminder deleted: <id>`（緑）。エラーは `✗ Error: ...` / 1。

### 3.4 `reminder complete`

| ロング | ショート | 型 | 既定 | 必須 |
| --- | --- | --- | --- | --- |
| `--id <reminderId>` | なし | string | なし | **必須** |
| `--profile <profile>` | なし | string | なし | 任意 |

- API: `reminders.complete({ reminder: <id> })`。
- 出力: `✓ Reminder completed: <id>`（緑）。エラーは `✗ Error: ...` / 1。

---

## 4. ページネーション・レート制限・並行実行

| 観点 | 実装 |
| --- | --- |
| `chat.scheduledMessages.list` | **ページネーションしない**。1回の呼び出しで `limit` 件を取り、`response_metadata.next_cursor` は無視 |
| `reminders.list` | ページネーションの概念なし（引数なしで全件） |
| `users.list`（draft send の `--user` 解決） | `limit: 200` で `next_cursor` が空になるまでループ。一致が見つかった時点で早期 return |
| `conversations.list`（チャンネル名解決） | `limit: 1000` で `next_cursor` が尽きるまでループ。結果は `ChannelOperations` インスタンス内の `channelLookupCache`（Promise キャッシュ）に保持。失敗時はキャッシュを破棄 |
| レート制限 | `WebClient` の自動リトライは**無効**（`retries: 0`）。対象コマンドの API 呼び出しは `pLimit(3)` を通さない（`users.info` のみ通すが対象経路では未使用）。429 はそのまま例外になり `✗ Error:` で終了1 |
| 並行実行 | 対象3コマンドはすべて逐次。`Promise.all` 等の並行処理なし |
| ドラフトファイルの排他 | ファイルロックは無い。read → 変更 → temp write → rename の read-modify-write なので、同時実行では**後勝ちで欠落し得る**（rename 自体は atomic） |

---

## 5. エラー表示と終了コードのまとめ

| 発生源 | 表示 | 終了コード |
| --- | --- | --- |
| commander の必須オプション欠落・未知オプション | commander 標準の usage エラー（英語） | 1 |
| `createValidationHook` 経由（format / threadTimestamp / reminderTiming） | `Error: <メッセージ>` を commander の `.error()` で出力 | 1 |
| `wrapCommand` が捕捉した例外 | `✗ Error: <message>`（赤）。`NODE_ENV=development` のとき stack も出力 | 1（`process.exit(1)`） |
| 正常終了 | — | 0 |

`extractErrorMessage`（`src/utils/error-utils.ts`）: Error の `data.error === 'missing_scope'`（またはメッセージに `missing_scope` を含む）かつ `data.needed` があるとき、`<message> (needed: scope1, scope2)` に整形する。Error 以外は `String(error)`。
出力前に `sanitizeTerminalText` → `redactSlackTokens`（`src/utils/token-utils.ts`。実装未読のため詳細は不明）を通す。

---

## 6. Rust 移植で引っかかりそうな点

1. **`console.table` の出力形式**。`draft list --table` と `scheduled list --table` は Node の `console.table` に丸投げしており、罫線・`(index)` 列・値のクォート（文字列は `'…'` で囲まれる）・全角文字の幅計算がすべて Node ランタイム依存。バイト単位で再現したいなら Node の実装仕様を別途確定させる必要がある。一方 `reminder list --table` は自前の固定幅パディングなので再現は容易（ただし `padEnd` は **UTF-16 コード単位**基準で、日本語では見た目がずれる。Rust で「見た目を揃える」実装にすると出力が変わる）。
2. **JSON 出力のキー順**。`draft list --format json` は `{ ...input, id, createdAt }` の構築順どおりに出力される（channel/user/message/thread → id → createdAt）。Rust の serde では struct 定義順になるため、順序を合わせるならフィールド宣言順を意図的に合わせる。`serde_json` の `BTreeMap` 化（アルファベット順）は不可。
3. **時刻パースの `Date.parse` 依存**。`reminder add --at "2024-03-01 15:00"` はブラウザ/Node の実装依存パーサで、タイムゾーン指定なしの場合は**ローカルタイムゾーン**として解釈される。`chrono` にそのまま対応する関数はないので、受け付ける書式を明示的に列挙する必要がある。全桁数字なら epoch 秒扱いという分岐も忘れずに。
4. **`--limit` が未検証**。`scheduled list --limit abc` は `parseInt` が `NaN` を返し、そのまま `chat.scheduledMessages.list({ limit: NaN })` に渡る（JSON 上は `null` 相当になる可能性がある）。Rust では `u32` パースが必ず失敗するため、**互換にするなら「不正値はエラーにせず limit を送らない/NaN 相当にする」挙動を意図的に決める**必要がある（現状の実際の HTTP 送信内容は未確認＝不明）。
5. **`draft send` はチャンネル名を解決しない**。`draft.user` が無い場合 `chat.postMessage({ channel: "general" })` のように**保存された文字列をそのまま**送る（`scheduled` 系は解決する）。この非対称性はバグに見えるが、仕様として維持するか直すかを移植前に決めること。
6. **フラグの真偽表現**。`--keep` は commander では未指定時 `undefined`、指定時 `true`。`!options.keep` で判定しているので Rust の `bool` に素直に落ちるが、`draft save` の `--message` のように「commander 上は optional なのにアクション内で必須」なオプションは、clap の `required = true` にすると**エラー文言と終了経路が変わる**（clap の usage エラー vs `✗ Error: --message is required`）。文言互換を取るなら optional のまま手動検査する。
7. **相互排他の実現方法の差**。`--channel`/`--user`（draft save）はアクション内検査、`--at`/`--after`（reminder add）は preAction フック。clap の `conflicts_with` / `required_unless_present` に置き換えると文言が変わる。文言互換を取るなら手動検査を維持。
8. **ローカルファイルのパーミッションと atomic write**。`0o700` ディレクトリ / `0o600` ファイル、`flag: 'wx'`（既存なら失敗）での一時ファイル書き込み → rename。Windows では `wx` + rename の挙動が異なる（既存ファイルへの rename が失敗し得る）。Rust では `std::fs::rename` と `OpenOptions::create_new(true)`、パーミッションは `std::os::unix::fs::PermissionsExt` で unix 限定になる。
9. **ドラフト読み込み時の寛容さ**。`id` と `message` が文字列である要素だけを残し、他は黙って捨てる。`channel`/`user`/`thread`/`createdAt` の型は検査していないため、`createdAt` が欠けたレコードも通る（表示は `undefined`）。serde の `deny_unknown_fields` や必須フィールド扱いにすると挙動が変わるので、`Option` + 手動フィルタで再現する。
10. **ターミナルサニタイズの文字単位**。`sanitizeTerminalText` は `for...of`（コードポイント単位）で走査し、C0/DEL/C1 を落とす。C1 判定 `code >= 0x80 && code <= 0x9f` は `charCodeAt(0)`＝UTF-16 コード単位なので、サロゲートペアの先頭が誤判定されることはない（先頭は 0xD800 以上）が、Rust の `char` ベース実装と同等になるかは境界を確認すること。
11. **色付け（chalk）**。`✓`/`✗`/`⚠` の記号と緑・赤・グレー・黄の使い分けを踏襲するなら、TTY 判定・`NO_COLOR`・`FORCE_COLOR` の扱いを chalk に合わせる必要がある（chalk の判定ロジックは未読＝詳細は不明）。
12. **`postAction` の更新チェック**。全コマンド完了後に `checkForUpdates` が走る。`draft send` などで `process.exit(1)` した場合はフックが走らないという副作用がある。Rust 移植時に相当機能を入れるかは別途判断（`update-notifier.ts` の中身は未読＝不明）。
13. **`reminders.*` API の legacy 扱い**。Slack の `reminders.add` / `list` / `delete` / `complete` はユーザートークン前提のメソッド群だが、本 CLI がどのトークン種別を使うかは `config` コマンド側の仕様であり、本書の対象範囲では未確認＝不明。
14. **エラーからのスコープ抽出**。`error.data.needed` をカンマ分割して `(needed: ...)` を付ける処理は `@slack/web-api` が例外に付ける `data` プロパティに依存する。Rust では自前の HTTP クライアントで `ok: false` レスポンスの `error` / `needed` を同等に拾う設計が必要。

---

## 7. 未確認・不明な点

- `ERROR_MESSAGES.INVALID_THREAD_TIMESTAMP` の実文字列（`src/utils/constants.ts` の該当行は未読）。
- `redactSlackTokens`（`src/utils/token-utils.ts`）の具体的な置換ルール。
- `checkForUpdates`（`src/utils/update-notifier.ts`）の挙動。
- `ProfileConfigManager` によるプロファイル解決・トークン保存形式（`src/utils/profile-config.ts`, `token-crypto-service.ts` は未読）。
- Node の `console.table` の厳密な出力仕様（罫線文字・幅計算・クォート規則）。
- `limit: NaN` が実際に HTTP リクエストへどう乗るか。
