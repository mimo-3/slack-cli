# メッセージ系コマンド仕様（Rust移植用）

対象: `send` / `send-ephemeral` / `edit` / `delete`（計4コマンド、サブコマンドなし）

抽出元（すべて `/Users/mimo/organizations/open-source/slack-cli/`）:

- `src/index.ts`
- `src/commands/send.ts`, `send-ephemeral.ts`, `edit.ts`, `delete.ts`
- `src/utils/` 配下: `validators.ts`, `constants.ts`, `command-wrapper.ts`, `schedule-utils.ts`, `errors.ts`, `error-utils.ts`, `option-parsers.ts`, `client-factory.ts`, `config-helper.ts`, `channel-resolver.ts`, `slack-client-service.ts`, `terminal-sanitizer.ts`, `token-utils.ts`
- `src/utils/slack-operations/` 配下: `base-client.ts`, `message-write-operations.ts`, `user-operations.ts`, `channel-operations.ts`（該当箇所のみ）
- `src/types/commands.ts`

本書に書いたのは上記ファイルで実際に確認した内容のみ。未読部分に依存する事項は「不明」と明記した。

---

## 0. CLI全体像（index.ts）

| 項目 | 値 |
| --- | --- |
| バイナリ名 | `slack-cli` |
| 説明文 | `CLI tool to send messages via Slack API` |
| バージョン | `package.json` の `version` を実行時に読み込む（`__dirname/../package.json`） |
| CLIフレームワーク | commander |
| コマンド登録 | `program.addCommand(...)` を25回。順序は config, send, channels, history, unread, scheduled, search, edit, delete, upload, download, reaction, pin, users, usergroups, channel, members, send-ephemeral, join, leave, invite, reminder, bookmark, canvas, draft |
| グローバルフック | `postAction` で `checkForUpdates({ packageName, currentVersion })` を実行（全コマンド共通、アクション成功後）。実装詳細は `update-notifier.ts` 未読のため不明 |
| エントリ | `require.main === module` のとき `runCli(process.argv)` |

グローバルフラグは `--version` と commander 既定の `--help` のみ。`--profile` は各コマンド個別のオプションであり、グローバルではない。

### 4コマンド共通の実行フロー

1. commander が引数をパース（`requiredOption` 欠落はこの時点でエラー）
2. `preAction` フックで `createValidationHook([...])` が順にバリデータを実行。最初に返った文字列で `thisCommand.error(\`Error: ${message}\`)` を呼んで即終了
3. `wrapCommand(action)` の中で本体実行。例外は catch して `✗ Error: <message>` を stderr に出し `process.exit(1)`
4. 成功時は緑色の `✓ ...` を stdout に1行出力

### 認証・プロファイル解決

- `parseProfile(options.profile)` は単に値をそのまま返すだけ（`undefined` 可）
- `createSlackClient(profile)` → `getConfigOrThrow(profile)` → `ProfileConfigManager.getConfig(profile)`
- 設定ファイルは `~/.slack-cli/config.json`（`configDir` オプションで上書き可）
- 設定が無ければ `ConfigurationError`。文言は
  `No configuration found for profile "<profileName>". Use "slack-cli config set --token <token> --profile <profileName>" to set up.`
  `<profileName>` は 指定プロファイル → プロファイル一覧中の `isDefault` のもの → `'default'` の順で決定
- 環境変数からトークンを読む経路は `profile-config.ts` の grep 範囲では見つからなかった（トークンの暗号化は `token-crypto-service.ts` にあるが未読のため詳細は不明）

---

## 1. コマンド名・エイリアス・サブコマンド構造

| コマンド | エイリアス | サブコマンド | 説明文（`--help` に出る原文） |
| --- | --- | --- | --- |
| `send` | なし | なし | `Send or schedule a message to a Slack channel or DM` |
| `send-ephemeral` | なし | なし | `Send an ephemeral message visible only to a specific user in a channel` |
| `edit` | なし | なし | `Edit a sent message` |
| `delete` | なし | なし | `Delete a sent message` |

4コマンドとも位置引数を一切取らない（`.argument()` の呼び出しなし）。すべての入力はオプションフラグ経由。

## 2. 位置引数

| コマンド | 位置引数 |
| --- | --- |
| `send` | なし |
| `send-ephemeral` | なし |
| `edit` | なし |
| `delete` | なし |

---

## 3. オプションフラグ

commander の `.option()` は既定値なし（未指定なら `undefined`）。値付きフラグ `<...>` はすべて必須値。型はすべて文字列。

### 3.1 `send`

| ロング | ショート | 値 | 型 | 既定値 | 必須 | 説明 |
| --- | --- | --- | --- | --- | --- | --- |
| `--channel` | `-c` | `<channel>` | String | なし | 条件付き | 送信先チャンネル名 or ID |
| `--user` | なし | `<username>` | String | なし | 条件付き | ユーザー名でDM送信 |
| `--email` | なし | `<email>` | String | なし | 条件付き | メールアドレスでDM送信 |
| `--message` | `-m` | `<message>` | String | なし | 条件付き | 送信本文 |
| `--file` | `-f` | `<file>` | String | なし | 条件付き | 本文を含むファイル |
| `--blocks` | `-b` | `<json>` | String | なし | 任意 | Block Kit JSON配列文字列 |
| `--blocks-file` | なし | `<file>` | String | なし | 任意 | Block Kit JSON配列のファイル |
| `--thread` | `-t` | `<thread>` | String | なし | 任意 | 返信先スレッドts |
| `--at` | なし | `<time>` | String | なし | 任意 | 予約時刻（Unix秒 or ISO 8601） |
| `--after` | なし | `<minutes>` | String | なし | 任意 | N分後に予約 |
| `--profile` | なし | `<profile>` | String | なし | 任意 | 使用するワークスペースプロファイル |

相互排他・必須の関係（`preAction` の5バリデータ、この順で評価）:

1. `sendTarget`: `--channel` / `--user` / `--email` のうち**ちょうど1つ**必要
   - 全部なし → `You must specify one of: --channel, --user, or --email`
   - `--channel` と（`--user` または `--email`）→ `Cannot use --channel with --user or --email`
   - `--user` と `--email` → `Cannot use --user and --email together`
2. `messageOrFile`:
   - `--message` も `--file` も `--blocks` も `--blocks-file` も無い → `You must specify either --message or --file`
   - `--message` と `--file` の同時指定 → `Cannot use both --message and --file`
   - （blocks 系だけの指定は許容される。その場合 text は空文字になる）
3. `blocksOption`:
   - `--blocks` と `--blocks-file` の同時指定 → `Cannot use both --blocks and --blocks-file`
   - `--blocks` の値が JSON パース不能、または配列でない → `Invalid blocks JSON: must be a valid JSON array`
4. `threadTimestamp`: `--thread` があれば正規表現 `^\d{10}\.\d{6}$`。不一致 → `Invalid thread timestamp format`
5. `scheduleTiming`:
   - `--at` と `--after` 同時 → `Cannot use both --at and --after`
   - `--at` がパース不能 → `Invalid schedule time format. Use Unix timestamp (seconds) or ISO 8601 date-time`
   - `--at` の結果が現在時刻以下（`postAt <= floor(Date.now()/1000)`）→ `Schedule time must be in the future`
   - `--after` が `^\d+$` でない、または安全整数でない、または0以下 → `--after must be a positive integer (minutes)`
   - 注: `--after` に対する「未来かどうか」の追加チェックはない

### 3.2 `send-ephemeral`

| ロング | ショート | 値 | 型 | 既定値 | 必須 | 説明 |
| --- | --- | --- | --- | --- | --- | --- |
| `--channel` | `-c` | `<channel>` | String | なし | 実質必須（バリデータで担保） | 送信先チャンネル名 or ID |
| `--user` | `-u` | `<user>` | String | なし | 実質必須 | エフェメラルを見るユーザーID |
| `--message` | `-m` | `<message>` | String | なし | 実質必須 | 本文 |
| `--thread` | `-t` | `<thread>` | String | なし | 任意 | 返信先スレッドts |
| `--profile` | なし | `<profile>` | String | なし | 任意 | プロファイル |

バリデータ順: `requiredChannel` → `requiredUser` → `requiredMessage` → `threadTimestamp`。
文言はそれぞれ `--channel is required` / `--user is required` / `--message is required` / `Invalid thread timestamp format`。
commander 側は `.option()`（`requiredOption` ではない）なので、必須性はバリデータのみで担保されている点に注意。相互排他の関係は無い。`--blocks` 系は無い（クライアント層は blocks 対応だがCLIからは渡さない）。

### 3.3 `edit`

| ロング | ショート | 値 | 型 | 既定値 | 必須 | 説明 |
| --- | --- | --- | --- | --- | --- | --- |
| `--channel` | `-c` | `<channel>` | String | なし | **必須**（`requiredOption`） | チャンネル名 or ID |
| `--ts` | なし | `<timestamp>` | String | なし | **必須**（`requiredOption`） | 編集対象メッセージのts |
| `--message` | `-m` | `<message>` | String | なし | 条件付き | 新しい本文 |
| `--file` | `-f` | `<file>` | String | なし | 条件付き | 新しい本文のファイル |
| `--blocks` | `-b` | `<json>` | String | なし | 任意 | Block Kit JSON配列文字列 |
| `--blocks-file` | なし | `<file>` | String | なし | 任意 | Block Kit JSON配列ファイル |
| `--profile` | なし | `<profile>` | String | なし | 任意 | プロファイル |

バリデータ順: `editTimestamp` → `messageOrFile` → `blocksOption`。

- `editTimestamp`: `--ts` が `^\d{10}\.\d{6}$` に一致しなければ `Invalid message timestamp format`
- `messageOrFile` / `blocksOption` は `send` と同一（文言も同じ）

### 3.4 `delete`

| ロング | ショート | 値 | 型 | 既定値 | 必須 | 説明 |
| --- | --- | --- | --- | --- | --- | --- |
| `--channel` | `-c` | `<channel>` | String | なし | **必須**（`requiredOption`） | チャンネル名 or ID |
| `--ts` | なし | `<timestamp>` | String | なし | **必須**（`requiredOption`） | 削除対象メッセージのts |
| `--profile` | なし | `<profile>` | String | なし | 任意 | プロファイル |

バリデータ: `deleteTimestamp` のみ。`editTimestamp` と実装は同一で、文言も `Invalid message timestamp format`。相互排他なし。

---

## 4. 呼び出すSlack Web APIメソッドとリクエストパラメータ

`WebClient` の生成設定（`base-client.ts`）:

- `new WebClient(token, { retryConfig: { retries: 0 }, logLevel: ERROR })` — 自動リトライは**無効**
- 同時に `pLimit(RATE_LIMIT.CONCURRENT_REQUESTS = 3)` のレートリミッタを作るが、本書対象の書き込み系（`chat.*`）では `rateLimiter` は使われていない（`users.info` など一部でのみ使用）

### 4.1 `send`

処理順:

| 手順 | 条件 | APIメソッド | パラメータ |
| --- | --- | --- | --- |
| 1 | `--user` 指定時 | `users.list` | `{ limit: 200, cursor? }` をカーソル尽きるまでループ。`member.name.toLowerCase() === username（先頭 `@` 除去）.toLowerCase()` で一致するidを返す |
| 2 | `--user` 指定時 | `conversations.open` | `{ users: <userId> }` → `channel.id` をDMチャンネルIDとして使う |
| 1' | `--email` 指定時 | `users.lookupByEmail` | `{ email }` → `user.id` |
| 2' | `--email` 指定時 | `conversations.open` | `{ users: <user.id> }` |
| 3 | 予約なし（`postAt === null`） | `chat.postMessage` | `{ channel, text, thread_ts?, blocks? }` — `thread_ts`/`blocks` は存在する時だけキーを付与 |
| 3' | 予約あり | `chat.scheduleMessage` | `{ channel, text, post_at, thread_ts?, blocks? }` |

重要: `send` では**チャンネル名→ID解決を行わない**。`--channel` の値をそのまま `channel` に渡す（Slack API 側の名前解決に委ねている）。`edit` / `delete` とは挙動が異なる。

`text` の決め方:

- `--file` 指定時: ファイルをUTF-8で読み、内容をそのまま `text` に（trim なし）
- それ以外: `--message ?? ''`（blocks のみの場合は空文字が送られる）

`blocks` の決め方:

- `--blocks-file`: UTF-8読み込み → `JSON.parse` → 配列でなければエラー
- `--blocks`: `JSON.parse`（この時点では配列チェックなし。バリデータ側で済んでいる）
- どちらもなければ `blocks` キー自体を送らない

`post_at` の決め方（`resolvePostAt`）:

- `--at` あり: `parseScheduledTimestamp(at)` — `^\d+$` なら10進整数としてUnix秒、そうでなければ `Date.parse()` の結果を1000で割って floor
- `--at` なし・`--after` あり: `floor(now_ms/1000) + minutes*60`
- どちらもなし: `null`（即時送信）

### 4.2 `send-ephemeral`

| APIメソッド | パラメータ |
| --- | --- |
| `chat.postEphemeral` | `{ channel: <--channel の生の値>, user: <--user の生の値>, text, thread_ts? }` |

チャンネル名→ID解決なし。`--user` はユーザーIDを期待している（ヘルプ文言が `User ID who will see the ephemeral message`）が、名前解決の実装は無い。`blocks` はCLIから渡らない。

### 4.3 `edit`

| 手順 | APIメソッド | パラメータ |
| --- | --- | --- |
| 1 | `conversations.list`（`--channel` がID形式でない場合のみ） | `{ types: <既定のlookup types>, exclude_archived: true, limit: 1000, cursor }` をカーソル尽きるまで |
| 2 | `chat.update` | `{ channel: <解決済みID>, ts, text, blocks? }` |

チャンネル解決は `ChannelResolver.resolveChannelId`:

- `^[CDG][A-Z0-9]{8,}$` に一致すればID扱いでそのまま使用
- そうでなければ全チャンネル取得 → 名前一致（完全一致 / `#` 除去 / 大文字小文字無視 / `name_normalized` 一致）
- 取得結果はインスタンス内に `channelLookupCache` としてメモ化（失敗時はキャッシュを捨てる）
- 既定の lookup types は `DEFAULT_CHANNEL_LOOKUP_TYPES`（`channel-operations.ts` 内。具体値は当該定数の定義行を未読のため不明）。取得エラー時は `getFallbackChannelLookupTypes` で types を絞って再試行するフォールバックがある

コマンド側では `blocks` の有無で呼び分けているが（`updateMessage(ch, ts, text, blocks)` / `updateMessage(ch, ts, text)`）、実装上は `blocks` が `undefined` ならキーを落とすので等価。

### 4.4 `delete`

| 手順 | APIメソッド | パラメータ |
| --- | --- | --- |
| 1 | `conversations.list`（名前指定時のみ、`edit` と同じ解決） | 同上 |
| 2 | `chat.delete` | `{ channel: <解決済みID>, ts }` |

---

## 5. 標準出力の形式

4コマンドとも**フォーマット指定オプション（`--format` 等）は無い**。成功時は chalk の緑色で1行のみ。JSON出力モードは存在しない。APIレスポンス（送信されたtsなど）は一切出力されない。

| コマンド | 条件 | stdout（色コードを除いた文字列） |
| --- | --- | --- |
| `send` | チャンネル宛・即時 | `✓ Message sent successfully to #<--channel の生の値>` |
| `send` | `--user` 宛・即時 | `✓ DM sent to @<username（先頭 @ を除去した値）>` |
| `send` | `--email` 宛・即時 | `✓ DM sent to <email>` |
| `send` | チャンネル宛・予約 | `✓ Message scheduled to #<channel> at <ISO8601>` |
| `send` | `--user`/`--email` 宛・予約 | `✓ Message scheduled to @<username> at <ISO8601>` / `✓ Message scheduled to <email> at <ISO8601>` |
| `send-ephemeral` | 成功 | `✓ Ephemeral message sent to #<--channel の生の値>` |
| `edit` | 成功 | `✓ Message updated successfully in #<--channel の生の値>` |
| `delete` | 成功 | `✓ Message deleted successfully from #<--channel の生の値>` |

`<ISO8601>` は `new Date(postAt * 1000).toISOString()` の値、すなわち `2026-08-17T09:30:00.000Z` 形式（ミリ秒3桁・UTC・`Z` 付き）。

出力例:

```
$ slack-cli send -c general -m "hello"
✓ Message sent successfully to #general

$ slack-cli send -c "#general" -m "hello"
✓ Message sent successfully to ##general

$ slack-cli send --user @alice -m "hi"
✓ DM sent to @alice

$ slack-cli send -c general -m "later" --after 30
✓ Message scheduled to #general at 2026-08-17T10:00:00.000Z

$ slack-cli edit -c C0123456789 --ts 1712345678.123456 -m "fixed"
✓ Message updated successfully in #C0123456789
```

注意点（そのまま移植すべき既存挙動）:

- `#` は常にコード側で前置するため、ユーザーが `#general` と渡すと `##general` になる
- `edit` / `delete` はチャンネルIDを渡しても `#C0123456789` と表示する
- 成功メッセージは常に `console.log`（stdout）、エラーは `console.error`（stderr）

---

## 6. エラーケース・文言・終了コード

### 6.1 終了コードの体系

| 発生源 | 終了コード | 出力先 | 形式 |
| --- | --- | --- | --- |
| commander の引数エラー（`requiredOption` 欠落、未知オプション等） | 1（commander 既定の `exitCode` は 1） | stderr | commander 既定形式 + usage |
| `preAction` バリデータ（`thisCommand.error()`） | 1 | stderr | `error: Error: <メッセージ>`（commander が `error: ` を前置し、コード側が `Error: ` を付ける二重前置） |
| アクション本体の例外（`wrapCommand`） | `process.exit(1)` → 1 | stderr | `✗ Error: <サニタイズ・トークン伏字後のメッセージ>` |
| 正常終了 | 0 | — | — |

`wrapCommand` は例外種別で分岐しない。すべて exit code 1。`NODE_ENV === 'development'` のときのみ、続けて灰色でスタックトレースも出す（こちらもサニタイズ + トークン伏字を通す）。

### 6.2 バリデーションエラー一覧

| コマンド | 条件 | メッセージ |
| --- | --- | --- |
| send | ターゲット未指定 | `You must specify one of: --channel, --user, or --email` |
| send | `--channel` + `--user`/`--email` | `Cannot use --channel with --user or --email` |
| send | `--user` + `--email` | `Cannot use --user and --email together` |
| send / edit | 本文もファイルもblocksも無い | `You must specify either --message or --file` |
| send / edit | `--message` と `--file` 同時 | `Cannot use both --message and --file` |
| send / edit | `--blocks` と `--blocks-file` 同時 | `Cannot use both --blocks and --blocks-file` |
| send / edit | `--blocks` が不正JSON or 非配列 | `Invalid blocks JSON: must be a valid JSON array` |
| send / send-ephemeral | `--thread` の形式不正 | `Invalid thread timestamp format` |
| send | `--at` と `--after` 同時 | `Cannot use both --at and --after` |
| send | `--at` パース不能 | `Invalid schedule time format. Use Unix timestamp (seconds) or ISO 8601 date-time` |
| send | `--at` が過去 or 現在 | `Schedule time must be in the future` |
| send | `--after` が正整数でない | `--after must be a positive integer (minutes)` |
| send-ephemeral | `--channel` 未指定 | `--channel is required` |
| send-ephemeral | `--user` 未指定 | `--user is required` |
| send-ephemeral | `--message` 未指定 | `--message is required` |
| edit / delete | `--ts` の形式不正 | `Invalid message timestamp format` |

いずれも最初に見つかった1件のみ報告して終了。

### 6.3 実行時エラー一覧

| 種別 | クラス（`code`） | メッセージ |
| --- | --- | --- |
| 設定なし | `ConfigurationError`（`CONFIGURATION_ERROR`） | `No configuration found for profile "<name>". Use "slack-cli config set --token <token> --profile <name>" to set up.` |
| 本文ファイル読み込み失敗 | `FileError`（`FILE_ERROR`） | `Error reading file <path>: <原因メッセージ>` |
| blocksファイル読み込み失敗 | `FileError` | `Error reading blocks file <path>: <原因メッセージ>` |
| blocksファイルのJSONが不正（`SyntaxError`） | `FileError` | `Invalid blocks JSON: must be a valid JSON array` |
| blocksファイルの中身が配列でない | `FileError` | `Error reading blocks file <path>: blocks must be a JSON array`（内部で投げた `Error('blocks must be a JSON array')` が `SyntaxError` ではないため read エラー扱いのメッセージに包まれる） |
| `--blocks` の `JSON.parse` 失敗（アクション本体） | 素の `SyntaxError` | Node の文言そのまま。ただしバリデータで先に弾かれるため通常到達しない |
| ユーザー名解決失敗 | `ApiError`（`API_ERROR`） | `User '<name>' not found`（`@` 除去後の名前） |
| チャンネル名解決失敗（候補あり） | `ApiError` | `Channel '<name>' not found. Did you mean one of these? <候補をカンマ+空白区切りで最大5件>` |
| チャンネル名解決失敗（候補なし） | `ApiError` | `Channel '<name>' not found. Make sure you are a member of this channel.` |
| Slack API エラー | `@slack/web-api` の例外がそのまま伝播 | `extractErrorMessage` により、`missing_scope` かつ `data.needed` があるときのみ `<元メッセージ> (needed: <scope1, scope2>)` に加工。それ以外は `error.message` |

`extractErrorMessage` の詳細:

- `Error` インスタンスなら `error.message`
- `data.error === 'missing_scope'`（または `message` に `missing_scope` を含む）かつ `data.needed` が非空文字列なら、`needed` をカンマ分割・trim・空要素除去して ` (needed: ...)` を付加
- `Error` でなければ `String(error)`

出力前の加工（`wrapCommand`）:

1. `sanitizeTerminalText`: OSCシーケンス・ANSIシーケンスを除去し、タブ(0x09)と改行(0x0A)以外の制御文字（0x00–0x1F、0x7F、0x80–0x9F）を削除
2. `redactSlackTokens`: `/xox[bpoars]-[A-Za-z0-9-]+/gi` に一致する部分を `<先頭4文字を小文字化>-***-REDACTED` に置換

チャンネル未検出エラーの生成時にはさらに `sanitizeTerminalText` がチャンネル名と候補名に個別に適用される（二重サニタイズ）。

---

## 7. ページネーション・レート制限・並行実行

| 項目 | 実装 |
| --- | --- |
| ページネーション（`users.list`） | `send --user` の名前解決で `limit: 200` 固定、`response_metadata.next_cursor` が空になるまで do-while。一致が見つかった時点で即return（全ページ走査は行わない） |
| ページネーション（`conversations.list`） | `edit` / `delete` のチャンネル名解決で `limit: 1000`（`DEFAULTS.CHANNELS_LIMIT`）、`next_cursor` が falsy になるまで do-while。全ページを配列に蓄積してから名前一致を探す |
| ページネーション（`conversations.open`, `chat.*`） | なし |
| チャンネル一覧のキャッシュ | `ChannelOperations` インスタンス内の `channelLookupCache`（Promiseをメモ化）。CLIは1コマンド1プロセスなので実質1回だけ効く。失敗時はキャッシュを破棄して再取得可能にする |
| リトライ | `WebClient` の `retryConfig.retries = 0`。SDKの自動リトライは無効。アプリ側の明示的リトライも本書対象パスには無い |
| レート制限 | `pLimit(3)`（`RATE_LIMIT.CONCURRENT_REQUESTS`）を共有コンテキストに持つが、`chat.postMessage` / `chat.postEphemeral` / `chat.scheduleMessage` / `chat.update` / `chat.delete` / `users.list` / `conversations.list` / `conversations.open` / `users.lookupByEmail` はいずれも `rateLimiter` を経由していない。実質、本書の4コマンドではレートリミッタは効いていない |
| `handleRateLimit` | `BaseSlackClient` に定義（`rate limit` を含むエラーで5秒待つ）はあるが、本書対象パスからは呼ばれていない |
| 並行実行 | 4コマンドとも直列。並行処理は行わない（`RATE_LIMIT.UNREAD_SCAN_CONCURRENT_REQUESTS = 15` は unread 系の話で無関係） |
| 参考定数 | `RATE_LIMIT.BATCH_SIZE = 10`, `BATCH_DELAY_MS = 1000`, `RETRY_CONFIG = { retries: 3, factor: 2, minTimeout: 1000, maxTimeout: 30000 }`（本書対象パスでは未使用） |

---

## 8. Rustへ移すときに引っかかりそうな点

### 8.1 CLIパーサ（clap）との差異

- commander の `.option('-b, --blocks <json>')` は「値必須のオプション、既定値なし」。clap では `Option<String>` + `.value_name("json")` に対応する。既定値を勝手に入れないこと
- `--blocks-file` は commander が自動で `blocksFile` にキャメルケース変換する。Rust側の構造体フィールド名は自由だが、エラーメッセージ中では `--blocks-file` とハイフン形式で出す必要がある
- 必須性の担保箇所が2種類ある。`edit`/`delete` の `--channel`/`--ts` は commander の `requiredOption`（＝clapの `required = true` 相当）で、`send-ephemeral` の3つは手書きバリデータ。**エラー文言と出力形式が違う**（前者は commander 既定の `error: required option '-c, --channel <channel>' not specified`、後者は `error: Error: --channel is required`）。文言互換を取るなら両者を分けて実装する必要がある
- バリデータのエラーが `error: Error: ...` と `Error:` 二重になっている既存挙動。互換のために意図的に残すか直すかを決めること
- バリデータは配列順に評価して**最初の1件で終了**する。clap の組み込みバリデーション（`conflicts_with` 等）を使うと評価順が変わり、複数違反時に出るメッセージが変わる。順序互換が要るなら手書きバリデータ層をそのまま移植するほうが安全

### 8.2 時刻・数値

- `parseScheduledTimestamp` は「全部数字なら Unix秒、そうでなければ `Date.parse`」。`Date.parse` は ISO 8601 以外の緩い形式（`"March 1, 2026"` 等）も受けるJS固有の実装依存挙動。Rustで `chrono`/`time` の RFC3339 パーサに置き換えると受理範囲が狭まる。どこまで互換を取るか決める必要がある
- タイムゾーン無しのISO文字列（`2026-08-17T10:00:00`）の解釈がJSではローカル時刻。Rust側で UTC 扱いにすると挙動が変わる
- `Number.isSafeInteger` の境界（2^53-1）。Rust の `i64` で受けると、JSでは弾かれる巨大値が通る
- `--after` の分→秒変換 `minutes * 60` はJSでは倍精度のためオーバーフローしないが、Rustの `i64` 掛け算はチェック付きにすること
- 出力の `toISOString()` は必ずミリ秒3桁 + `Z`。`chrono` の既定 RFC3339 出力はミリ秒が0のとき省略されるため、`format("%Y-%m-%dT%H:%M:%S%.3fZ")` 相当を明示する必要がある
- `--at` の過去判定はバリデータ内で `Date.now()` を、実際の `post_at` 算出は `resolvePostAt` 内で再度 `Date.now()` を取る（`--after` の場合）。つまり時刻を2回取っている。厳密再現するならRustでも同様に

### 8.3 文字列・エンコーディング

- `sanitizeTerminalText` は `for...of` でJSの文字列を走査するため**コードポイント単位**（サロゲートペアは1つの char として扱われる）。`charCodeAt(0)` はサロゲートペアの上位を返すが、上位サロゲート（0xD800–）は削除対象レンジに入らないので実害は無い。Rustの `chars()` は同じくコードポイント単位なので概ね一致するが、不正なサロゲートを含む入力の扱いは異なる（RustのStringには不正サロゲートが存在できない）
- ファイル読み込みは `fs.readFile(path, 'utf-8')`。Node は不正なUTF-8を U+FFFD に置換して読む（エラーにしない）。Rust の `fs::read_to_string` は不正UTF-8でエラーになるため、`from_utf8_lossy` を使わないと挙動が変わる
- `redactSlackTokens` の正規表現は大文字小文字無視 + 貪欲。Rust の `regex` クレートで `(?i)xox[bpoars]-[A-Za-z0-9-]+` として移植可能だが、置換関数内で `match[0..4].to_lowercase()` する点を忘れない
- chalk は TTY でない場合や `NO_COLOR` 等で自動的に色を落とす。Rustでも同等の判定（`is_terminal` + 環境変数）を入れないと、パイプ時の出力にANSIが混ざって差分が出る

### 8.4 JSON / Block Kit

- `--blocks` はバリデータで一度パースし、アクション本体で**もう一度パース**している（二重パース）。Rustでは一度パースして持ち回せばよいが、その場合バリデータ段でのエラー文言との対応をずらさないこと
- `blocks` の型チェックは「配列であること」だけ。中身の構造は検証していない。`serde_json::Value` の配列として素通しするのが等価
- `--blocks-file` の非配列エラーだけ、`Error('blocks must be a JSON array')` を投げてすぐ隣の catch が `BLOCKS_FILE_READ_ERROR` で包むという分かりにくい経路。結果の文言 `Error reading blocks file <path>: blocks must be a JSON array` を再現したいなら、この包み込みを意図的に実装する必要がある
- JSONパース失敗の判定が `error instanceof SyntaxError`。Rustでは `serde_json::Error` の判別で代替するが、`serde_json` のパースエラーとI/Oエラーは元から型が別なので自然に分離できる

### 8.5 Slack API クライアント

- `@slack/web-api` はレスポンスの `ok: false` を例外に変換する。Rustで `reqwest` を直に使うなら、HTTP 200 + `{"ok": false, "error": "..."}` を自前でエラー化する層が必須
- エラーオブジェクトの `data.error` / `data.needed` を読む `extractErrorMessage` は SDK 固有のエラー形状に依存。Rust側のエラー型に `error: Option<String>` / `needed: Option<String>` を持たせる必要がある
- `...(thread_ts ? { thread_ts } : {})` というキー省略パターン。Rustでは `#[serde(skip_serializing_if = "Option::is_none")]` で等価にできる。**空文字とキー無しは別物**なので、`Option<String>` を維持すること
- `send` はチャンネル名解決をせず生値を渡し、`edit`/`delete` は解決する、という非対称。共通化したくなるが、挙動が変わるので分けたまま移植すること
- チャンネルID判定の正規表現が2箇所で食い違う。`ChannelResolver.isChannelId` は `^[CDG][A-Z0-9]{8,}$`、`formatValidators.channelId` は `^[CDG][A-Z0-9]{10,}$`。本書対象パスで使われるのは前者
- `conversations.list` の `types` に渡す `DEFAULT_CHANNEL_LOOKUP_TYPES` の実値と、エラー時の types フォールバック条件は未読のため不明。移植前に `channel-operations.ts` の該当箇所を確認すること
- `chat.postEphemeral` の `user` にユーザーIDをそのまま渡す（名前解決なし）。`send` の `--user` とは意味が違うので、Rust側で共通の「ユーザー指定」型にまとめると仕様が変わる

### 8.6 プロセス制御・その他

- `wrapCommand` は `process.exit(1)` を即座に呼ぶ。stdout がパイプでバッファに残っている場合の flush 順序に依存があるが、Rust では `std::process::exit` の前に明示 flush が要る
- `postAction` フックの `checkForUpdates` は**全コマンドの成功後**に走る。`process.exit(1)` するエラーパスでは走らない。Rustで移植する場合、成功パスのみのフックとして実装する。中身（ネットワークアクセスの有無・キャッシュ）は `update-notifier.ts` 未読のため不明
- `NODE_ENV === 'development'` でのスタックトレース出力は、Rustでは `RUST_BACKTRACE` とは別の独自環境変数で再現するか、仕様として落とすか決める
- 設定ファイルのパーミッション定数 `CONFIG_DIR = 0o700`, `CONFIG_FILE = 0o600` は Unix 前提。Windows 対応が要るなら別途検討（現状の実装がどうしているかは `profile-config.ts` 未読部分のため不明）
- トークンの暗号化まわり（`token-crypto-service.ts`）は未読。`getConfigOrThrow` は復号済みの `{ token }` を返す前提なので、Rust移植では設定ファイルフォーマットの互換性確認が必要
