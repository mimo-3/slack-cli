# `history` / `unread` コマンド仕様（Rust移植用）

移植元: `/Users/mimo/organizations/open-source/slack-cli`（TypeScript / commander.js / @slack/web-api）

読んだファイル:

- `src/index.ts`
- `src/commands/history.ts` / `history-display.ts` / `history-validators.ts` / `unread.ts`
- `src/utils/` 配下: `command-support.ts`, `command-wrapper.ts`, `constants.ts`, `option-parsers.ts`, `validators.ts`, `error-utils.ts`, `errors.ts`, `client-factory.ts`, `config-helper.ts`, `channel-resolver.ts`, `channel-formatter.ts`, `date-utils.ts`, `format-utils.ts`, `mention-utils.ts`, `slack-patterns.ts`, `terminal-sanitizer.ts`, `token-utils.ts`, `slack-client-service.ts`
- `src/utils/formatters/`: `base-formatter.ts`, `history-formatters.ts`, `message-formatters.ts`, `channel-formatters.ts`
- `src/utils/slack-operations/`: `base-client.ts`, `channel-operations.ts`, `message-history-operations.ts`, `message-permalink-operations.ts`, `message-user-resolver.ts`, `search-operations.ts`（一部）
- `src/types/commands.ts`, `src/types/slack.ts`

未読ファイル（この仕様書の範囲外）: `profile-config.ts`（トークン保存の実装詳細）、`update-notifier.ts`（詳細）、`message-operations.ts`（委譲のみと推定されるが未読のため断定しない）、`search-operations.ts` の 26-70 行目の一部。

---

## 0. CLI 全体像（`src/index.ts`）

| 項目 | 値 |
| --- | --- |
| バイナリ名 | `slack-cli` |
| 説明 | `CLI tool to send messages via Slack API` |
| バージョン | `package.json` の `version` を実行時に読む（`--version`） |
| コマンド一覧 | config, send, channels, **history**, **unread**, scheduled, search, edit, delete, upload, download, reaction, pin, users, usergroups, channel, members, send-ephemeral, join, leave, invite, reminder, bookmark, canvas, draft |
| postAction フック | 全コマンド実行後に `checkForUpdates({packageName, currentVersion})` を await する（新バージョン通知） |

`history` と `unread` はいずれも **サブコマンドを持たないフラットなコマンド**。位置引数は **両方とも 0 個**（すべてオプションフラグで受ける）。

---

## 1. `slack-cli history`

### 1-1. コマンド構造

| 項目 | 値 |
| --- | --- |
| コマンド名 | `history` |
| エイリアス | なし |
| サブコマンド | なし |
| 位置引数 | なし |
| 説明 | `Get message history from a Slack channel` |

### 1-2. オプション

| ロング | ショート | 値 | 型 | デフォルト | 必須 | 備考 |
| --- | --- | --- | --- | --- | --- | --- |
| `--channel` | `-c` | `<channel>` | string | なし | **必須**（commander の `requiredOption`） | チャンネル名 or ID |
| `--number` | `-n` | `<number>` | string→int | 未指定時 `10`（`API_LIMITS.DEFAULT_MESSAGE_COUNT`） | 任意 | 1〜1000 |
| `--since` | なし | `<date>` | string（日付） | なし | 任意 | `YYYY-MM-DD HH:MM:SS` 想定 |
| `--thread` | `-t` | `<thread>` | string | なし | 任意 | スレッド ts。`1234567890.123456` 形式 |
| `--with-link` | なし | フラグ | bool | `false` | 任意 | 各メッセージの permalink を付ける |
| `--format` | なし | `<format>` | string | `"table"` | 任意 | `table` / `simple` / `json` |
| `--profile` | なし | `<profile>` | string | なし | 任意 | ワークスペースプロファイル |

#### 相互排他・優先関係

- `--thread` は排他ではないが**優先**される。`--thread` 指定時:
  - `--number` が指定されていれば `Warning: --number is ignored when --thread is specified.` を **標準出力**（`console.log`）に出す。
  - `--since` が指定されていれば `Warning: --since is ignored when --thread is specified.` を標準出力に出す。
  - `--number` / `--since` のバリデーション自体もスキップされる（preAction フックが `options.thread ? null : validator(...)` になっている）。
- それ以外に排他関係はない。

#### バリデーション（preAction フック、`createValidationHook`）

実行順は以下。最初のエラーで `command.error()` を呼び終了する（後述の終了コード参照）。

1. `--thread` 未指定時のみ `optionValidators.messageCount`
   - `--number` が指定されていて `parseInt` が NaN → `Message count must be a number`
   - `< 1` → `Message count must be at least 1`
   - `> 1000` → `Message count must be at most 1000`
2. `--thread` 未指定時のみ `optionValidators.sinceDate`
   - `new Date(since)` が Invalid Date → `Invalid date format. Use YYYY-MM-DD HH:MM:SS`
3. `optionValidators.threadTimestamp`
   - `--thread` 指定時、正規表現 `^\d{10}\.\d{6}$` に一致しなければ → `Invalid thread timestamp format`
4. `optionValidators.format`
   - `--format` が `table` / `simple` / `json` のいずれでもなければ → ``Invalid format '<値>'. Must be one of: table, simple, json``

エラー出力は `thisCommand.error(\`Error: ${message}\`)` の形なので、最終的な文言は `Error: Message count must be at least 1` のようになる。

> 注意: `src/commands/history-validators.ts` には `validateMessageCount` / `validateDateFormat` も定義されているが、**`history.ts` からは `prepareSinceTimestamp` しか使われていない**。この 2 関数がどこから使われているかは未確認（この範囲では未使用）。移植時は `prepareSinceTimestamp` だけ写せばよい。文言は `Error: Message count must be between 1 and 1000` / `Error: Invalid date format. Use YYYY-MM-DD HH:MM:SS` と、上記フックの文言と微妙に異なる。

#### 値の正規化（`parseCount`）

バリデーション通過後、`limit` は `parseCount(options.number, 10, 1, 1000)` で決まる:

- `parseInt` が NaN → `10`
- `< 1` → `1` にクランプ
- `> 1000` → `1000` にクランプ

（フックで既に弾かれるためクランプが効くのは実質フックをすり抜けたケースのみ）

#### `--since` の変換（`prepareSinceTimestamp`）

`Math.floor(Date.parse(since) / 1000).toString()` を `oldest` に渡す。`Date.parse` は **ローカルタイムゾーン依存**（`"2026-01-01 10:00:00"` のような形式）。

### 1-3. 呼び出す Slack Web API

#### (A) 通常モード（`--thread` なし）

| 順 | API メソッド | パラメータ | 備考 |
| --- | --- | --- | --- |
| 1 | `conversations.list` | `types="public_channel,private_channel,im,mpim"`, `exclude_archived=true`, `limit=1000`, `cursor` | `--channel` が ID 形式（`^[CDG][A-Z0-9]{8,}$`）でない場合のみ。カーソルが尽きるまでループ。結果はクライアント内でキャッシュ |
| 1' | 同上（フォールバック） | 上記から不足スコープに対応する type を除いた `types` | 1 が `missing_scope` で失敗し、`needed` スコープが `channels:read`/`groups:read`/`im:read`/`mpim:read` にマップできる場合のみ 1 回だけ再試行 |
| 2 | `conversations.history` | `channel=<channelId>`, `limit=<limit>`, `oldest=<since秒>`（`--since` 指定時のみ） | **ページネーションなし**。1 回だけ呼ぶ |
| 3 | `users.info` | `user=<userId>` | メッセージ本文中のメンション `<@Uxxxx>` と `message.user` から抽出したユニークな user ID 全部に対し **1 件ずつ逐次** 呼ぶ |
| 4 | `chat.getPermalink` | `channel=<channelId>`, `message_ts=<ts>` | `--with-link` かつメッセージが 1 件以上のときのみ。**全メッセージ分を逐次** 呼ぶ |

#### (B) スレッドモード（`--thread` あり）

| 順 | API メソッド | パラメータ | 備考 |
| --- | --- | --- | --- |
| 1 | `conversations.list` | 上記と同じ | チャンネル名解決時のみ |
| 2 | `conversations.replies` | `channel=<channelId>`, `ts=<threadTs>`, `cursor` | `response_metadata.next_cursor` が空になるまで **全ページ取得**（limit 指定なし＝ API 既定） |
| 3 | `users.info` | 同上 | |
| 4 | `chat.getPermalink` | 同上 | `--with-link` 指定時 |

### 1-4. メッセージの並び順

- 通常モード: `conversations.history` は新しい順なので、表示前に **配列を reverse**（古い順で表示）。
- スレッドモード: `preserveOrder: true` のため **API の返り順のまま**（古い順）。

### 1-5. 標準出力

#### 共通の前処理

- 全テキストは `sanitizeTerminalText` を通す: OSC シーケンス（`ESC ] ... BEL/ST`）と ANSI CSI/Fe シーケンスを除去し、さらに制御文字（`< 0x20`、`0x7F`、`0x80–0x9F`）を落とす。ただし **タブ `0x09` と改行 `0x0A` は残す**。
- メンション `<@Uxxxx>` は `@username` に置換。username が解決できなければ user ID をそのまま `@Uxxxx` として表示。
- 表示ユーザー名（`resolveUsername`）: `message.user` があれば users マップ引き（無ければ `Unknown User`）、無くて `bot_id` があれば `Bot`、どちらも無ければ `Unknown`。
- タイムスタンプ（`formatTimestampFixed`）: Slack ts を **UTC** で `YYYY-MM-DD HH:MM:SS` に整形（ゼロ埋め）。

#### `--format table`（既定）

```
（空行）
Message History for #general:      ← chalk.bold

[2026-08-17 01:23:45] alice        ← [ts] は chalk.gray、ユーザー名は chalk.cyan
こんにちは @bob
  📎 report.pdf, application/pdf, 1.2 MB https://files.slack.com/...   ← 📎行は chalk.yellow、URLは chalk.blue
https://acme.slack.com/archives/C123/p1755...   ← --with-link 時のみ、chalk.blue
（空行）
[2026-08-17 01:25:00] Bot
(no text)
（空行）
✓ Displayed 2 message(s)          ← chalk.green
```

- メッセージが 0 件のとき: 見出し行の後に `No messages found`（chalk.yellow）を出して終了（`✓ Displayed ...` は出ない）。
- `message.text` が空/未定義なら `(no text)`。
- ファイルラベルは `name || title || 'unnamed'` に `mimetype`、サイズを `, ` で連結。
- サイズ表記（`formatFileSize`）: `<1024` → `123 B` / `<1MiB` → `12.3 KB`（小数1桁）/ それ以上 → `1.2 MB`（小数1桁）。
- ファイル URL は `url_private_download || url_private || permalink`、いずれも無ければ URL 部を出さない。

#### `--format simple`

1 メッセージ 1 行。見出し・件数サマリなし。

```
[2026-08-17 01:23:45] alice: こんにちは @bob [📎 report.pdf] https://acme.slack.com/archives/...
```

- 形式: `[{ts}] {username}: {text}{fileSuffix}{linkSuffix}`
- `fileSuffix` はファイルがあるとき ` [📎 name1, name2]`（名前のみ、mimetype/サイズなし）。
- `linkSuffix` は `--with-link` かつ permalink 取得成功時のみ ` {url}`。
- 0 件のとき: `No messages found`（色なし）。

#### `--format json`

`JSON.stringify(..., null, 2)` を 1 回 `console.log`。文字列は全て `sanitizeTerminalData` で再帰的にサニタイズされる。

```json
{
  "channel": "general",
  "messages": [
    {
      "ts": "1755400000.123456",
      "timestamp": "2026-08-17 01:23:45",
      "user": "alice",
      "user_id": "U0123ABCD",
      "text": "こんにちは <@U0999ZZZZ>",
      "thread_ts": "1755400000.123456",
      "reply_count": 3,
      "files": [
        {
          "id": "F123",
          "name": "report.pdf",
          "mimetype": "application/pdf",
          "filetype": "pdf",
          "size": 1234567,
          "url": "https://files.slack.com/..."
        }
      ],
      "permalink": "https://acme.slack.com/archives/C123/p1755400000123456"
    }
  ],
  "total": 1
}
```

キーの出現規則:

- `channel` は `--channel` に渡された **入力文字列そのまま**（ID を渡せば ID が入る。`#` は付かない）。
- `user_id` は `message.user` が存在するときのみ。
- `thread_ts` / `reply_count` はそれぞれ値が `undefined` でないときのみ。
- `files` は 1 件以上のときのみ。
- `permalink` は permalink マップに ts が存在するときのみ。
- **JSON の `text` はメンション置換されない生テキスト**（table/simple と挙動が違う）。`(no text)` フォールバックのみ適用。
- 0 件でも `{"channel": ..., "messages": [], "total": 0}` を出す（`No messages found` は出ない）。

#### 不正な `--format`

preAction フックで弾かれるので到達しないが、フォーマッタ側は `FormatterFactory.create()` が未知キーで `table` にフォールバックする。

### 1-6. エラーと終了コード

| ケース | 出力先 | 文言 | 終了コード |
| --- | --- | --- | --- |
| `--channel` 未指定 | stderr | commander 既定 `error: required option '-c, --channel <channel>' not specified` | 1（commander の `error()` 既定 exitCode） |
| バリデーション違反（count/date/thread ts/format） | stderr | `Error: <上記文言>` | 1 |
| プロファイル設定なし | stderr | `✗ Error: No configuration found for profile "<name>". Use "slack-cli config set --token <token> --profile <name>" to set up.` | 1 |
| チャンネル名が解決できず、部分一致候補あり | stderr | `✗ Error: Channel '<name>' not found. Did you mean one of these? foo, bar`（最大 5 件） | 1 |
| チャンネル名が解決できず候補なし | stderr | `✗ Error: Channel '<name>' not found. Make sure you are a member of this channel.` | 1 |
| Slack API エラー全般 | stderr | `✗ Error: <error.message>` | 1 |
| Slack API が `missing_scope` かつ `needed` あり | stderr | `✗ Error: <error.message> (needed: channels:history, groups:history)` | 1 |
| permalink 取得失敗 | — | **エラーにしない**。`--with-link` 全体が try/catch され、失敗時は permalink 無しで履歴を表示（graceful degrade）。個別 ts の失敗も握り潰す | 0 |
| `users.info` 失敗 | — | エラーにしない。その user ID をユーザー名として使う | 0 |

エラー出力の共通処理（`wrapCommand`）:

1. `extractErrorMessage(error)` でメッセージ抽出
2. `sanitizeTerminalText` を通す
3. `redactSlackTokens` で `xox[bpoars]-...` を `xoxb-***-REDACTED` に置換
4. `console.error(chalk.red('✗ Error:'), <上記>)`
5. `NODE_ENV === 'development'` のときのみ、同様に処理したスタックトレースを chalk.gray で追加出力
6. `process.exit(1)`

### 1-7. ページネーション・レート制限・並行実行

| 項目 | 挙動 |
| --- | --- |
| `conversations.history` | ページネーションしない（1 リクエストのみ、`limit` = `--number`） |
| `conversations.replies` | `next_cursor` が尽きるまで全ページ |
| `conversations.list`（名前解決） | `next_cursor` が尽きるまで全ページ。1 クライアントインスタンス内で Promise をキャッシュし、複数回の解決で 1 回だけ取得。失敗したらキャッシュを破棄 |
| `users.info` | **逐次 for ループ**。並行なし。ユニーク user 数だけリクエストが飛ぶ |
| `chat.getPermalink` | **逐次 for ループ**。メッセージ数だけリクエストが飛ぶ |
| WebClient のリトライ | `retryConfig: { retries: 0 }` で **SDK 自動リトライを無効化** |
| 手動レート制限リトライ | `history` 経路には無い（`getHistory` / `getThreadHistory` / `getPermalinks` はリトライしない） |
| `pLimit` レート制限器 | `RATE_LIMIT.CONCURRENT_REQUESTS = 3` の limiter がクライアントに存在するが、**history 経路では使われていない** |

---

## 2. `slack-cli unread`

### 2-1. コマンド構造

| 項目 | 値 |
| --- | --- |
| コマンド名 | `unread` |
| エイリアス | なし |
| サブコマンド | なし |
| 位置引数 | なし |
| 説明 | `Show unread messages across channels` |
| バリデーションフック | **なし**（`--format` すら検証されない） |

### 2-2. オプション

| ロング | ショート | 値 | 型 | デフォルト | 必須 | 備考 |
| --- | --- | --- | --- | --- | --- | --- |
| `--channel` | `-c` | `<channel>` | string | なし | 任意 | 指定時は単一チャンネルモード、未指定時は全チャンネルモード |
| `--format` | なし | `<format>` | string | `"table"` | 任意 | `table` / `simple` / `json`。未知値は table にフォールバック（エラーにならない） |
| `--count-only` | なし | フラグ | bool | `false` | 任意 | 件数のみ |
| `--limit` | なし | `<number>` | string→int | `"50"`（`DEFAULTS.UNREAD_DISPLAY_LIMIT`） | 任意 | **全チャンネルモードでのみ有効**。単一チャンネルモードでは無視 |
| `--mark-read` | なし | フラグ | bool | `false` | 任意 | 取得後に既読化 |
| `--profile` | なし | `<profile>` | string | なし | 任意 | |

相互排他の宣言はゼロ。`--channel` の有無で分岐するだけ。`--count-only` は `--format` より優先される（後述）。

### 2-3. モード A: 単一チャンネル（`--channel` 指定）

#### API 呼び出し

| 順 | API | パラメータ | 備考 |
| --- | --- | --- | --- |
| 1 | `conversations.list` | `types="public_channel,private_channel,im,mpim"`, `exclude_archived=true`, `limit=1000`, `cursor` | `--channel` が ID 形式でないときのみ、全ページ |
| 2 | `conversations.info` | `channel=<channelId>` | `last_read` を得る |
| 3 | `conversations.history` | `channel=<channelId>`, `oldest=<last_read>`, `limit=200`, `cursor` | **`next_cursor` が尽きるまで全ページ**。未読の総数をカウントするため |
| 4 | `users.info` | `user=<userId>` | プレビュー対象メッセージ（最大 50 件）から抽出した user ID を逐次 |
| 5 | `conversations.mark` | `channel=<channelId>`, `ts=<現在時刻の秒(小数)>` | `--mark-read` 時のみ、出力の**後**に実行 |

- `totalUnreadCount` = 全ページのメッセージ数合計。
- 表示するメッセージは先頭から **最大 50 件**（`DEFAULTS.UNREAD_MESSAGE_PREVIEW_LIMIT`）。`--limit` はここに効かない。
- 3 のみ、レート制限エラー時に最大 3 回リトライする（後述）。

#### 出力（`message-formatters.ts`）

チャンネル名は `formatChannelName`: 先頭が `#` でなければ `#` を付ける。名前が無ければ `#unknown`。タイムスタンプは `formatSlackTimestamp`（`new Date(ts*1000).toLocaleString()` ＝ **ロケール・TZ 依存**）。

`table`:

```
#general: 12 unread messages         ← chalk.bold

2026/8/17 1:23:45 alice              ← 時刻 chalk.gray、著者 chalk.cyan
こんにちは @bob

2026/8/17 1:25:00 U0999ZZZZ
(no text)

Showing latest 50 of 12 unread messages   ← displayedMessageCount < totalUnreadCount のときのみ、chalk.gray
```

`simple`:

```
#general (12)
[2026/8/17 1:23:45] alice: こんにちは @bob
Showing latest 50 of 120 unread messages
```

`json`:

```json
{
  "channel": "#general",
  "channelId": "C0123ABCD",
  "unreadCount": 120,
  "messages": [
    { "timestamp": "2026/8/17 1:23:45", "author": "alice", "text": "こんにちは <@U0999ZZZZ>" }
  ],
  "displayedMessageCount": 50,
  "isTruncated": true
}
```

- 著者名は `users.get(message.user) || message.user`、`message.user` が無ければ `"unknown"`（history の `Bot` / `Unknown User` とは規則が違う）。
- `--count-only` 時は見出し 1 行のみで、メッセージ本体・`Showing latest ...`・JSON の `messages` / `displayedMessageCount` / `isTruncated` は出力されない。
- JSON の `text` はここでも **メンション未置換の生テキスト**。
- `--mark-read` 成功時、最後に `✓ Marked messages in #general as read`（chalk.green）。

### 2-4. モード B: 全チャンネル（`--channel` 未指定）

#### API 呼び出し

まず `search.messages` ベースの経路を試し、**例外が出たら丸ごと** `users.conversations` ベースの経路にフォールバックする（`SlackApiClient.listUnreadChannels` の try/catch）。

経路 1（優先）:

| 順 | API | パラメータ | 備考 |
| --- | --- | --- | --- |
| 1 | `search.messages` | `query="is:unread"`, `sort="timestamp"`, `sort_dir="desc"`, `count=100`, `page=1` | 1 ページ目で `page_count` を得る |
| 2 | `search.messages` | 同上、`page=2..page_count` | **並行実行**（`Promise.all` + `pLimit(3)`） |
| 3 | （集計） | — | チャンネル ID ごとにマッチ数を数え `unread_count` とし、最大 ts を `last_read` に入れる。`last_read` の降順にソート |
| 4 | `conversations.info` | `channel=<id>`, `include_num_members=false` | 集計した各チャンネルに対し **並行 15 本**（`pLimit(15)`）で情報を補完 |
| 5 | `users.info` | `user=<channel.user>` | IM で表示名が取れないときのみ（`@username` 生成用） |

経路 2（フォールバック）:

| 順 | API | パラメータ | 備考 |
| --- | --- | --- | --- |
| 1 | `users.conversations` | `types="public_channel,private_channel,im,mpim"`, `exclude_archived=true`, `limit=200`, `cursor` | 全ページ |
| 2 | `conversations.info` | `channel=<id>`, `include_num_members=false` | 未読数が応答に含まれないチャンネル、または名前が取れない非 IM/MPIM のみ。**並行 15 本** |
| 3 | — | — | `unread_count` が 0 のチャンネルを除外 |

`--mark-read` 時:

| API | パラメータ | 備考 |
| --- | --- | --- |
| `conversations.mark` | `channel=<id>`, `ts=<現在時刻の秒>` | 未読チャンネル **全件に対して逐次** 実行。`--limit` で表示を絞っても **表示外のチャンネルも既読になる** |

#### 出力（`channel-formatters.ts`）

表示対象は `channels.slice(0, limit)`。チャンネル名は `display_name || formatChannelName(name)` を `sanitizeSingleLineText`（空白・改行・タブの連続を単一スペースに畳んで trim）した値。

未読ゼロ件のとき（フォーマット問わず、`--count-only` でも）:

```
✓ No unread messages     ← chalk.green
```

`table`（`--count-only` 無し）:

```
Channel          Unread  Last Message        ← chalk.bold
──────────────────────────────────────────────────    ← '─' × 50
#general         12      2026/8/17 1:23:45
@alice           3       Unknown
```

- チャンネル名は `padEnd(16)`、未読数は `padEnd(6)`、その後にスペース 2 個。
- `last_read` が無ければ `Unknown`。`formatSlackTimestamp` はロケール依存。

`simple`:

```
#general (12)
@alice (3)
```

`json`:

```json
[
  { "channel": "#general", "channelId": "C0123ABCD", "unreadCount": 12 }
]
```

（`json` の `channel` は `sanitizeSingleLineText` を通していない点が table/simple と異なる。ただし `JsonFormatter` 側で `sanitizeTerminalData` は掛かる）

`--count-only`（**`--format` の値を無視して専用フォーマッタが選ばれる**）:

```
#general: 12
@alice: 3
Total: 15 unread messages     ← chalk.bold
```

`--mark-read` 成功時、最後に `✓ Marked all messages as read`（chalk.green）。

### 2-5. エラーと終了コード

| ケース | 文言 | 終了コード |
| --- | --- | --- |
| プロファイル設定なし | `✗ Error: No configuration found for profile "<name>". ...` | 1 |
| `--channel` が解決できない | `✗ Error: Channel '<name>' not found. Did you mean one of these? ...` / `... Make sure you are a member of this channel.` | 1 |
| `--format` が不正 | エラーにならない。table にフォールバック | 0 |
| `--limit` が数値でない | エラーにならない。`parseInt` が NaN → `slice(0, NaN)` = **空配列**（ヘッダのみ出力される）。※コード上の帰結であり実行確認はしていない | 0 |
| 全チャンネルモードの `search.messages` 失敗 | エラーにせず `users.conversations` 経路へフォールバック | 0 |
| 全チャンネルモードの個別 `conversations.info` 失敗 | 経路 1 では 5 秒待ってそのチャンネルを保持、経路 2 では 5 秒待って `null`（＝除外） | 0 |
| `--mark-read` の `conversations.mark` 失敗 | 例外が伝播 → `✗ Error: <message>` | 1 |
| その他 API エラー | `✗ Error: <message>`（`missing_scope` 時は ` (needed: ...)` 付与） | 1 |

### 2-6. ページネーション・レート制限・並行実行

| 項目 | 挙動 |
| --- | --- |
| 単一チャンネルの `conversations.history` | 全ページ（`limit=200`）。レート制限エラー時のみ最大 3 回リトライ（判定は `error.message.includes('rate limit')` という **文字列マッチ**、待機は固定 5 秒） |
| `search.messages` | 1 ページ目で総ページ数を取り、2 ページ目以降を `pLimit(3)` で並行。各ページはレート制限時 3 回までリトライ |
| `conversations.info`（全チャンネルモード） | `pLimit(15)`（`RATE_LIMIT.UNREAD_SCAN_CONCURRENT_REQUESTS`）で並行。レート制限時 3 回までリトライ |
| `users.conversations` | 全ページ（`limit=200`） |
| `users.info` | 逐次 |
| `conversations.mark` | 逐次（全件） |
| SDK 自動リトライ | 無効（`retries: 0`）。`logLevel: ERROR` |
| 定義はあるが未使用の定数 | `RATE_LIMIT.BATCH_SIZE=10`, `BATCH_DELAY_MS=1000`, `RETRY_CONFIG.factor/minTimeout/maxTimeout`（実際の待機は固定 5 秒でこれらを使っていない）、`DEFAULTS.HISTORY_LIMIT=20` |

---

## 3. 共通仕様（両コマンド）

### 3-1. チャンネル解決（`ChannelResolver`）

1. `^[CDG][A-Z0-9]{8,}$` にマッチすればそのまま ID として扱う（API 呼び出しなし）。
2. マッチしなければチャンネル一覧を取得し、以下の順で照合:
   - `c.name === input`
   - `c.name === input.replace('#', '')`（**最初の 1 個の `#` のみ除去**）
   - `c.name.toLowerCase() === input.toLowerCase()`
   - `c.name_normalized === input`
3. 見つからなければ、`name` に入力を小文字部分一致で含むチャンネルを最大 5 件挙げて `ApiError` を投げる。

なお `formatValidators.channelId` は `^[CDG][A-Z0-9]{10,}$` と桁数が違う（`{8,}` vs `{10,}`）。history/unread では前者だけが効く。

### 3-2. 設定・プロファイル

`createSlackClient(profile)` → `getConfigOrThrow(profile)` → `ProfileConfigManager.getConfig(profile)`。設定が無ければ `ConfigurationError`。プロファイル名の既定は「`--profile` の値 → プロファイル一覧中の `isDefault` の名前 → `"default"`」。トークン保存形式は `profile-config.ts` / `token-crypto-service.ts` を未読のため**不明**（暗号化されている可能性がある）。

### 3-3. 出力ストリーム

すべての正常出力・警告（`Warning: --number is ignored ...` を含む）は **stdout**。エラーのみ stderr。

---

## 4. Rust 移植で引っかかりそうな点

1. **`Date.parse` / `toLocaleString` のロケール・TZ 依存**
   - `--since` は `Date.parse("2026-01-01 10:00:00")` を使っており、**ローカルタイムゾーン**で解釈される。しかも `Date.parse` は非 ISO 文字列の解釈が実装依存。Rust では `chrono` でパースするフォーマットを明示する必要があり、ここで挙動差が出る。ISO 8601 と `YYYY-MM-DD HH:MM:SS` の両方を受ける実装が要る。
   - `history` の表示時刻（`formatTimestampFixed`）は **UTC 固定**、`unread` の表示時刻（`formatSlackTimestamp` = `toLocaleString()`）は **ローカル TZ かつロケール依存フォーマット**。同じ CLI 内で規則が食い違っており、`unread` の出力は Rust で 1:1 再現できない（Node の ICU 依存の `2026/8/17 1:23:45` のような形式）。ここは「ISO に揃える」など仕様変更の判断が必要。

2. **`users.info` / `chat.getPermalink` の逐次ループ**
   - 100 件の履歴に `--with-link` を付けると permalink だけで 100 リクエストが直列に走る。Rust では `futures` で並行化したくなるが、そうすると Slack のレート制限（`chat.getPermalink` は Tier 3 相当）に当たる挙動が変わる。移植の忠実度と実用性のトレードオフになる箇所。

3. **レート制限判定が `error.message.includes('rate limit')` という文字列マッチ**
   - Slack SDK のエラー文言に依存した実装。Rust では HTTP 429 と `Retry-After` ヘッダで判定するのが自然で、そのまま移せない。待機も固定 5 秒でヘッダを見ていない。

4. **`Map<string, string>` の users とフォーマッタの分岐**
   - `history` は「不明 → `Unknown User` / `Bot` / `Unknown`」、`unread` は「不明 → user ID そのまま / `unknown`」。同じ概念に 2 系統の規則がある。Rust で共通化したくなるが、そのまま統一すると出力が変わる。

5. **JSON 出力だけメンション未置換**
   - `history` / `unread` とも JSON の `text` は `<@U123>` のまま、table/simple は `@alice`。意図的か不明だが、テストが既存出力に依存する場合は温存が必要。

6. **`--count-only` が `--format` を上書きする**
   - `unread` の全チャンネルモードでは `--count-only` が `count` という**フォーマッタの第 4 種**を選ぶため、`--format json --count-only` でも JSON にならずテキストが出る。単一チャンネルモードでは逆に、`--count-only` はフォーマッタ内のフラグとして扱われ JSON のままになる。Rust で enum に整理するとこの非対称を落としやすい。

7. **`--limit` の適用範囲の非対称**
   - `--limit` は全チャンネルモードの表示件数にしか効かない。単一チャンネルのプレビューは定数 50 固定、`--mark-read` は表示件数と無関係に全件を既読化する。素直に設計し直すと副作用の範囲が変わる。

8. **`sanitizeTerminalText` の文字単位ループ**
   - JS の `for...of` は **コードポイント単位**で回るため、サロゲートペア（絵文字など）は分解されない。Rust の `chars()` はほぼ等価だが、C1 制御域（U+0080–U+009F）の除去を忘れると挙動が変わる。ANSI/OSC の正規表現も先に適用してから制御文字除去、という順序に意味がある。

9. **chalk の色付け**
   - TTY でないとき chalk は自動で色を落とす。Rust では `anstream` / `owo-colors` + `is-terminal` 等で同等の判定が要る。`NO_COLOR` / `FORCE_COLOR` の扱いも chalk 準拠にするか決める必要がある。

10. **エラー時の `process.exit(1)` 一本槍**
    - バリデーションエラーも API エラーも設定エラーも全部 exit 1。commander の `.error()` も既定 1。Rust で終了コードを細分化すると互換が崩れる。

11. **`conversations.list` キャッシュの寿命**
    - `ChannelOperations` インスタンス内の `Promise` キャッシュ。Rust では `OnceCell` / `tokio::sync::OnceCell` 相当で、かつ**失敗時にキャッシュを捨てる**（再試行可能にする）挙動まで写す必要がある。

12. **`missing_scope` フォールバック**
    - `conversations.list` が `missing_scope` で落ちたとき、`needed` スコープから引けるチャンネル種別を除いて 1 回だけ再試行する。ただし「全種別が blocked」または「1 つも除外できない」ときは再試行せず元エラーを投げる。Slack エラーの `data.needed`（カンマ区切り）へのアクセスが必要で、Rust の SDK/自前クライアントでエラーボディを保持する設計にしておく必要がある。

13. **`unread` 全チャンネルモードの二重経路**
    - `search.messages` 経路は `is:unread` 検索が使えるトークン（ユーザートークン）前提で、失敗したら `users.conversations` 経路に落ちる。しかも両経路で `unread_count` の意味が違う（前者は検索ヒット数、後者は Slack が返す未読数）。`last_read` フィールドに「最新メッセージの ts」を詰めている（本来の意味と違う値）点も、table 表示の `Last Message` 列に効いている。Rust で型を切るとこの流用が破綻するので、フィールド名を分ける判断が要る。

14. **`p-limit` 相当**
    - `pLimit(3)` / `pLimit(15)` は Rust では `futures::stream::buffer_unordered(n)` か `tokio::sync::Semaphore` で置き換え。共有 limiter がクライアント単位である点（`unread` の scan limiter は呼び出しごとに新規作成）に注意。

15. **未使用コードの持ち込み**
    - `history-validators.ts` の `validateMessageCount` / `validateDateFormat` は history から未使用、`RATE_LIMIT.BATCH_*` や `DEFAULTS.HISTORY_LIMIT` も未使用。移植時に写さない判断でよいが、他コマンドから使われていないかは全体確認が必要（この調査範囲では未確認）。
