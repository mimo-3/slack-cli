# チャンネル系コマンド仕様書（Rust移植用）

抽出元: `/Users/mimo/organizations/open-source/slack-cli`（`@mimo-3/slack-cli` v0.24.1）

対象ファイル:

- `src/commands/channels.ts`
- `src/commands/channel.ts`
- `src/commands/join.ts`
- `src/commands/leave.ts`
- `src/commands/invite.ts`
- `src/commands/members.ts`

参照した補助ファイル: `src/index.ts`, `src/utils/option-parsers.ts`, `src/utils/channel-formatter.ts`, `src/utils/channel-resolver.ts`, `src/utils/command-wrapper.ts`, `src/utils/command-support.ts`, `src/utils/client-factory.ts`, `src/utils/config-helper.ts`, `src/utils/profile-config.ts`, `src/utils/constants.ts`, `src/utils/errors.ts`, `src/utils/error-utils.ts`, `src/utils/token-utils.ts`, `src/utils/terminal-sanitizer.ts`, `src/utils/date-utils.ts`, `src/utils/validators.ts`, `src/utils/formatters/base-formatter.ts`, `src/utils/formatters/channels-list-formatters.ts`, `src/utils/formatters/channel-info-formatters.ts`, `src/utils/formatters/members-formatters.ts`, `src/utils/slack-operations/base-client.ts`, `src/utils/slack-operations/channel-operations.ts`, `src/utils/slack-operations/user-operations.ts`, `src/types/commands.ts`, `src/types/slack.ts`

本書に書いたのは、上記ファイルを実際に読んで確認できた内容のみ。読んでいない部分・実装から確定できない部分は「不明」と明記した。

---

## 0. 全体像

### 0.1 CLIのルート（`src/index.ts`）

- バイナリ名（commanderの`name`）: `slack-cli`
- description: `CLI tool to send messages via Slack API`
- version: `package.json` の `version` を実行時に読み込んで `--version` に設定
- `program.hook('postAction', ...)` で全コマンドのアクション後に `checkForUpdates({ packageName, currentVersion })` を実行（`src/utils/update-notifier.ts` の中身は未読のため挙動詳細は不明）
- ルートに登録されるサブコマンドは25個。本書の対象は `channels` / `channel` / `join` / `leave` / `invite` / `members` の6コマンド（`channel` は3サブコマンドを持つため、実行単位では計8）

登録順（`createProgram()` 内の `addCommand` 順）:
`config`, `send`, `channels`, `history`, `unread`, `scheduled`, `search`, `edit`, `delete`, `upload`, `download`, `reaction`, `pin`, `users`, `usergroups`, `channel`, `members`, `send-ephemeral`, `join`, `leave`, `invite`, `reminder`, `bookmark`, `canvas`, `draft`

### 0.2 コマンド一覧（本書の対象）

| # | コマンド | サブコマンド | エイリアス | 位置引数 | 説明（原文） |
|---|---|---|---|---|---|
| 1 | `channels` | なし | なし | なし | List Slack channels |
| 2 | `channel` | `info` | なし | なし | Display channel details including topic and purpose |
| 3 | `channel` | `set-topic` | なし | なし | Set the topic of a channel |
| 4 | `channel` | `set-purpose` | なし | なし | Set the purpose of a channel |
| 5 | `join` | なし | なし | なし | Join a channel |
| 6 | `leave` | なし | なし | なし | Leave a channel |
| 7 | `invite` | なし | なし | なし | Invite user(s) to a channel |
| 8 | `members` | なし | なし | なし | List channel members |

**位置引数は8コマンドすべてに存在しない。** すべてオプションフラグで値を渡す設計になっている。
**エイリアスも一切定義されていない**（`.alias()` の呼び出しなし）。

`channel` 親コマンド自体は description（`Manage channel topic, purpose, and info`）のみを持ち、`.action()` を持たない。したがって `slack-cli channel` 単体実行時はcommanderのデフォルト挙動（ヘルプ表示）になる。

### 0.3 共通の実行フロー

すべてのコマンドのアクションは `wrapCommand()`（`src/utils/command-wrapper.ts`）でラップされている。

```
1. （commander）requiredOption 未指定チェック → エラーなら commander が終了
2. （一部コマンドのみ）preAction hook で createValidationHook による検証
3. アクション本体
   3-1. parseProfile(options.profile) → string | undefined（そのまま返すだけ）
   3-2. createSlackClient(profile)
        → getConfigOrThrow(profile) で ~/.slack-cli/config.json からトークン取得（復号）
        → new SlackApiClient(token)
   3-3. Slack API 呼び出し
   3-4. 出力（console.log）
4. 例外が出たら wrapCommand が捕捉 → stderr に出力 → process.exit(1)
```

`channels` のみ `withSlackClient()`（`src/utils/command-support.ts`）というヘルパを使うが、中身は 3-1 + 3-2 と同じ（`parseProfile` → `createSlackClient` → コールバック実行）。

### 0.4 Slackクライアントの生成（`base-client.ts`）

```ts
new WebClient(token, {
  retryConfig: { retries: 0 },   // 自動リトライは無効
  logLevel: LogLevel.ERROR,
})
rateLimiter: pLimit(RATE_LIMIT.CONCURRENT_REQUESTS)  // 同時実行数3
```

`RATE_LIMIT`（`src/utils/constants.ts`）:

| キー | 値 |
|---|---|
| `CONCURRENT_REQUESTS` | 3 |
| `UNREAD_SCAN_CONCURRENT_REQUESTS` | 15 |
| `BATCH_SIZE` | 10 |
| `BATCH_DELAY_MS` | 1000 |
| `RETRY_CONFIG` | `{ retries: 3, factor: 2, minTimeout: 1000, maxTimeout: 30000 }` |

`RETRY_CONFIG` はWebClientには渡されていない。本書対象コマンドの経路では `fetchChannelInfo`（unread系のみ）で `retries` の値だけが使われる。**本書対象の8コマンドはいずれも `fetchChannelInfo` を通らないため、リトライは一切行われない。**

`BaseSlackClient.handleRateLimit(error)` は「エラーメッセージに `rate limit` という文字列が含まれていたら5秒待つ」だけの実装。これも本書対象コマンドの経路では呼ばれない。

### 0.5 チャンネル名 → ID 解決（`resolveChannelId`）

`channel info` / `set-topic` / `set-purpose` / `join` / `leave` / `invite` / `members` はすべて、APIを呼ぶ前に `ChannelOperations.resolveChannelId()` を通す。`channels` だけは通らない。

```
resolveChannelId(input):
  1. isChannelId(input) が true なら input をそのまま返す
     判定正規表現: /^[CDG][A-Z0-9]{8,}$/
  2. false なら channelLookupCache（プロセス内メモ化）を取得
     → listChannels({ types: 'public_channel,private_channel,im,mpim',
                      exclude_archived: true, limit: 1000 })
       ※ limit は DEFAULTS.CHANNELS_LIMIT = 1000
       ※ cursor が尽きるまで全ページ取得する
  3. missing_scope エラーだったらフォールバック
     needed スコープ → チャンネル種別のマップで、取れない種別を除外して再取得
       channels:read → public_channel
       groups:read   → private_channel
       im:read       → im
       mpim:read     → mpim
     除外後が空 or 除外できなかった場合は元のエラーを再throw
  4. findChannel(input, channels) でマッチング（下記4条件のいずれか）
     - c.name === input
     - c.name === input.replace('#', '')   ※ 最初の1つの '#' のみ除去
     - c.name?.toLowerCase() === input.toLowerCase()
     - c.name_normalized === input
  5. 見つからなければ ApiError を throw（メッセージは §6 参照）
```

キャッシュ（`channelLookupCache`）は `Promise<Channel[]>` を保持し、失敗時は `undefined` に戻して再取得可能にしている。CLIは1コマンド1プロセスなので、実質「1回の実行で最大1回だけ全チャンネル取得」の意味を持つ。

---

## 1. `slack-cli channels`

チャンネル一覧を表示する。

### 1.1 オプション

| ロング | ショート | 値の型 | デフォルト | 必須 | 相互排他 |
|---|---|---|---|---|---|
| `--type <type>` | なし | 文字列（`public` / `private` / `im` / `mpim` / `all`） | `'public'` | 任意 | なし |
| `--include-archived` | なし | 真偽（値なしフラグ） | `false` | 任意 | なし |
| `--format <format>` | なし | 文字列（`table` / `simple` / `json`） | `'table'` | 任意 | なし |
| `--limit <number>` | なし | 文字列（内部で `parseInt`） | `'100'` | 任意 | なし |
| `--profile <profile>` | なし | 文字列 | なし（未指定時は設定のデフォルトプロファイル） | 任意 | なし |

相互排他の関係は定義されていない。

**注意（バリデーションなし）**: `channels` には `preAction` の `createValidationHook` が付いていない。したがって
- `--format` に不正値（例 `--format xml`）を渡してもエラーにならず、`FormatterFactory.create()` のフォールバックで **table 形式にそのまま落ちる**
- `--type` に不正値（例 `--type foo`）を渡しても `getChannelTypes` のフォールバックで **`public_channel` として扱われる**
- `--limit` に非数値（例 `--limit abc`）を渡すと `parseInt` が `NaN` を返し、**`NaN` がそのままAPIパラメータ `limit` に乗る**

### 1.2 値の変換

| 入力 | 変換 | 結果 |
|---|---|---|
| `--type public` | `getChannelTypes` | `public_channel` |
| `--type private` | 同上 | `private_channel` |
| `--type im` | 同上 | `im` |
| `--type mpim` | 同上 | `mpim` |
| `--type all` | 同上 | `public_channel,private_channel,mpim,im` |
| 上記以外 | 同上（マップミス） | `public_channel` |
| `--include-archived` あり | `!parseBoolean(true)` | `exclude_archived: false` |
| `--include-archived` なし | `!parseBoolean(false)` | `exclude_archived: true` |
| `--limit N` | `parseInt(N, 10)` | 数値 |

### 1.3 Slack Web API

| メソッド | リクエストパラメータ |
|---|---|
| `conversations.list` | `types`（上記変換後の文字列）, `exclude_archived`（bool）, `limit`（数値）, `cursor`（2ページ目以降のみ） |

### 1.4 ページネーション

`ChannelOperations.listChannels` は do-while ループで `response_metadata.next_cursor` が空になるまで**全ページ取得する**。

**重要**: `--limit` は「1ページあたりの件数」としてAPIに渡るだけで、**取得総件数の上限にはならない**。`--limit 100` を指定してもワークスペースに500チャンネルあれば500件すべてが取得・表示される。表示側で件数を絞る処理もない。

### 1.5 レスポンスのマッピング（`mapChannelToInfo`）

| 出力フィールド | 導出元 |
|---|---|
| `id` | `channel.id` |
| `name` | `sanitizeTerminalText(channel.name \|\| 'unnamed')` |
| `type` | 判定順: `is_channel && !is_private` → `public` / `is_group \|\| (is_channel && is_private)` → `private` / `is_im` → `im` / `is_mpim` → `mpim` / いずれも該当せず → `unknown` |
| `members` | `channel.num_members \|\| 0` |
| `created` | `new Date(created * 1000).toISOString().split('T')[0]` → `YYYY-MM-DD` |
| `purpose` | `sanitizeTerminalText(channel.purpose?.value \|\| '')` |

### 1.6 標準出力

#### 0件のとき（フォーマット指定に関係なく）

```
No channels found
```

（`ERROR_MESSAGES.NO_CHANNELS_FOUND`。stdout に出力し、終了コードは 0）

#### `--format table`（デフォルト）

ヘッダ2行 + 1チャンネル1行。パディングは半角文字数（`String.padEnd`）ベース。

```
Name              Type      Members  Created      Description
─────────────────────────────────────────────────────────────────
general           public    128      2019-04-01   会社全体のお知らせ
dev-acejob        public    42       2023-11-20   開発チームの相談ごと
```

- 1行目: 固定文字列 `Name              Type      Members  Created      Description`
- 2行目: `─`（U+2500）を65回
- 各行: `${name.padEnd(17)} ${type.padEnd(9)} ${members.padEnd(8)} ${created.padEnd(12)} ${purpose}`
  - name / purpose は `sanitizeSingleLineText` 適用済み
  - purpose は31文字以上のとき `substring(0, 27) + '...'`（結果30文字）

#### `--format simple`

チャンネル名のみを1行ずつ（`sanitizeSingleLineText` 適用）。

```
general
dev-acejob
```

#### `--format json`

`JSON.stringify(data, null, 2)`（インデント2）。

```json
[
  {
    "id": "C0123456789",
    "name": "general",
    "type": "public",
    "members": 128,
    "created": "2019-04-01T00:00:00Z",
    "purpose": "会社全体のお知らせ"
  }
]
```

`created` は table 用の `YYYY-MM-DD` に文字列連結で `T00:00:00Z` を足しているだけ（元のUnix時刻の時分秒は失われる）。

---

## 2. `slack-cli channel info`

### 2.1 オプション

| ロング | ショート | 値の型 | デフォルト | 必須 | 相互排他 |
|---|---|---|---|---|---|
| `--channel <channel>` | `-c` | 文字列（チャンネル名 or ID） | なし | **必須**（`requiredOption`） | なし |
| `--format <format>` | なし | 文字列（`table` / `simple` / `json`） | `'table'` | 任意 | なし |
| `--profile <profile>` | なし | 文字列 | なし | 任意 | なし |

### 2.2 バリデーション

`.hook('preAction', createValidationHook([optionValidators.format]))` あり。

`optionValidators.format` の実装:
```
options.format が truthy かつ ['table','simple','json'] に含まれない場合
  → `Invalid format '<値>'. Must be one of: table, simple, json`
```
`createValidationHook` はこの文字列を `thisCommand.error('Error: ' + msg)` に渡す。したがって実際の出力は `error: Error: Invalid format 'xml'. Must be one of: table, simple, json`（commanderが先頭に `error: ` を付ける）。

### 2.3 Slack Web API

| 順 | メソッド | パラメータ |
|---|---|---|
| 1 | （名前指定時のみ）`conversations.list` | `types: 'public_channel,private_channel,im,mpim'`, `exclude_archived: true`, `limit: 1000`, `cursor` |
| 2 | `conversations.info` | `channel: <解決済みID>`, `include_num_members: true` |

### 2.4 標準出力

#### `--format table`（デフォルト）

chalk による装飾あり（見出しは bold、ラベルは gray）。

```

Channel Info: #general

  ID:       C0123456789
  Name:     general
  Private:  No
  Archived: No
  Members:  128
  Created:  2019/4/1

  Topic:    今日の話題
  Purpose:  会社全体のお知らせ

```

- 先頭に空行（`\n` 付き文字列のため）、末尾にも空行
- `Members:` の行は `num_members` が `undefined` のときのみ省略
- `Created:` は `new Date(created * 1000).toLocaleDateString()` — **ロケール・タイムゾーン依存**
- `Topic` / `Purpose` が未設定のときは `(not set)`

#### `--format simple`

```
general (C0123456789)
Topic: 今日の話題
Purpose: 会社全体のお知らせ
Members: 128
```

`Members:` 行は `num_members` が `undefined` なら出さない。

#### `--format json`

```json
{
  "id": "C0123456789",
  "name": "general",
  "is_private": false,
  "is_archived": false,
  "created": 1554076800,
  "num_members": 128,
  "topic": "今日の話題",
  "purpose": "会社全体のお知らせ"
}
```

- `is_archived` は未定義なら `false` に落とす
- `topic` / `purpose` は未設定なら `null`
- `num_members` が undefined の場合、`JSON.stringify` の仕様でキーごと消える

---

## 3. `slack-cli channel set-topic`

### 3.1 オプション

| ロング | ショート | 値の型 | デフォルト | 必須 | 相互排他 |
|---|---|---|---|---|---|
| `--channel <channel>` | `-c` | 文字列 | なし | **必須** | なし |
| `--topic <topic>` | なし | 文字列 | なし | **必須** | なし |
| `--profile <profile>` | なし | 文字列 | なし | 任意 | なし |

`--format` は**ない**。preAction hook も**ない**。

### 3.2 Slack Web API

| 順 | メソッド | パラメータ |
|---|---|---|
| 1 | （名前指定時のみ）`conversations.list` | §0.5 と同じ |
| 2 | `conversations.setTopic` | `channel: <解決済みID>`, `topic: <--topic の値そのまま>` |

`--topic` の値に対する長さチェック・サニタイズ・空文字チェックは一切ない（空文字は commander が値なしとみなさないため `--topic ""` は通り、空トピックの設定になる）。

### 3.3 標準出力

成功時、stdout に1行。chalk なし（色なし）。

```
✓ Topic updated for #general
```

**`#` の後ろは解決後のIDではなく、ユーザーが `-c` に渡した文字列そのもの。** `-c C0123456789` を渡すと `✓ Topic updated for #C0123456789` になる。サニタイズもしていない。

---

## 4. `slack-cli channel set-purpose`

### 4.1 オプション

| ロング | ショート | 値の型 | デフォルト | 必須 | 相互排他 |
|---|---|---|---|---|---|
| `--channel <channel>` | `-c` | 文字列 | なし | **必須** | なし |
| `--purpose <purpose>` | なし | 文字列 | なし | **必須** | なし |
| `--profile <profile>` | なし | 文字列 | なし | 任意 | なし |

### 4.2 Slack Web API

| 順 | メソッド | パラメータ |
|---|---|---|
| 1 | （名前指定時のみ）`conversations.list` | §0.5 と同じ |
| 2 | `conversations.setPurpose` | `channel: <解決済みID>`, `purpose: <--purpose の値そのまま>` |

### 4.3 標準出力

```
✓ Purpose updated for #general
```

`set-topic` と同じく、`#` の後ろは `-c` の入力文字列そのまま。chalk なし。

---

## 5. `slack-cli join` / `slack-cli leave`

2コマンドは構造が完全に同一で、API メソッドと文言だけが違う。

### 5.1 オプション（両方共通）

| ロング | ショート | 値の型 | デフォルト | 必須 | 相互排他 |
|---|---|---|---|---|---|
| `--channel <channel>` | `-c` | 文字列 | なし | **必須** | なし |
| `--profile <profile>` | なし | 文字列 | なし | 任意 | なし |

`--format` なし、preAction hook なし。

### 5.2 Slack Web API

| コマンド | 順 | メソッド | パラメータ |
|---|---|---|---|
| `join` | 1 | （名前指定時のみ）`conversations.list` | §0.5 と同じ |
| `join` | 2 | `conversations.join` | `channel: <解決済みID>` |
| `leave` | 1 | （名前指定時のみ）`conversations.list` | §0.5 と同じ |
| `leave` | 2 | `conversations.leave` | `channel: <解決済みID>` |

APIレスポンスは戻り値として使わず捨てている（`Promise<void>`）。`join` の場合、Slack API は `already_in_channel` を warning として返すが、CLI側で warning を見ていないため成功扱いになる。

### 5.3 標準出力

`chalk.green` で緑色。

```
✓ Joined channel #general
```

```
✓ Left channel #general
```

こちらも `#` の後ろは `-c` の入力文字列そのまま（未サニタイズ）。

---

## 6. `slack-cli invite`

### 6.1 オプション

| ロング | ショート | 値の型 | デフォルト | 必須 | 相互排他 |
|---|---|---|---|---|---|
| `--channel <channel>` | `-c` | 文字列 | なし | **必須** | なし |
| `--users <users>` | `-u` | 文字列（カンマ区切りのユーザーID） | なし | **必須** | なし |
| `--force` | なし | 真偽（値なしフラグ） | 未指定時 `undefined`（`false` 明示なし） | 任意 | なし |
| `--profile <profile>` | なし | 文字列 | なし | 任意 | なし |

`--force` の説明: `Continue inviting valid users even if some IDs are invalid`

### 6.2 `--users` のパース

```ts
options.users.split(',').map(id => id.trim()).filter(id => id.length > 0)
```

- カンマ区切り、各要素を trim、空要素は除去
- 結果が0件なら `throw new Error('At least one valid user ID is required')`
- ユーザーIDの形式チェック（`U` 始まりなど）は**していない**
- ユーザー名からIDへの解決は**していない**（`resolveUserIdByName` は呼ばれない）。IDのみが有効

### 6.3 Slack Web API

| 順 | メソッド | パラメータ |
|---|---|---|
| 1 | （名前指定時のみ）`conversations.list` | §0.5 と同じ |
| 2 | `conversations.invite` | `channel: <解決済みID>`, `users: <パース済み配列を ',' で join した文字列>`, `force`（`--force` が truthy のときのみキーごと追加） |

`force` は `...(force && { force })` というスプレッドなので、**未指定時はキー自体がリクエストに含まれない**。`force: false` は送られない。

### 6.4 標準出力

`chalk.green`。招待人数や個々の結果は表示しない。

```
✓ Invited user(s) to channel #general
```

`--force` 使用時に一部ユーザーが失敗したかどうかは出力から判別できない（レスポンスの `errors` フィールドを見ていない）。

---

## 7. `slack-cli members`

### 7.1 オプション

| ロング | ショート | 値の型 | デフォルト | 必須 | 相互排他 |
|---|---|---|---|---|---|
| `--channel <channel>` | `-c` | 文字列 | なし | **必須** | なし |
| `--limit <number>` | なし | 文字列（内部で `parseInt`） | `'100'` | 任意 | なし |
| `--format <format>` | なし | 文字列（`table` / `simple` / `json`） | `'table'` | 任意 | なし |
| `--profile <profile>` | なし | 文字列 | なし | 任意 | なし |

`.hook('preAction', createValidationHook([optionValidators.format]))` あり（§2.2 と同じ検証）。

### 7.2 Slack Web API

| 順 | メソッド | パラメータ | 備考 |
|---|---|---|---|
| 1 | （名前指定時のみ）`conversations.list` | §0.5 と同じ | |
| 2 | `conversations.members` | `channel: <解決済みID>`, `limit: <parseIntの結果 ?? 100>`, `cursor: undefined` | **1回のみ。ページングしない** |
| 3 | `users.info` | `user: <メンバーID>` | **メンバー1人につき1回**。`rateLimiter`（同時3）経由 |

`getChannelMembers` は `nextCursor` を戻り値に含めるが（`response_metadata.next_cursor || ''`）、`members` コマンド側では受け取っていない。したがって **`--limit` を超えるメンバーは取得されず、2ページ目以降にも進まない**。

### 7.3 並行実行・レート制限

- `result.members.map(async ...)` を `Promise.all` で走らせる → メンバー数ぶんの `users.info` を同時発火
- ただし `UserOperations.getUserInfo` の中で `this.rateLimiter(...)` を通すため、**実際の同時実行は `pLimit(3)` で3本に絞られる**（`rateLimiter` は `SharedSlackClientContext` 経由で全 operations が共有）
- `users.info` が失敗した場合、`try/catch` で握りつぶして `{ id, name: undefined, realName: undefined }` を返す。エラーは表示されない
- WebClient のリトライは `retries: 0` で無効。429 が返っても待たずに失敗扱い（→ ID のみ表示になる）

### 7.4 標準出力

#### 0件のとき

```
No members found
```

（ベタ書き文字列。`ERROR_MESSAGES` 経由ではない。stdout、終了コード 0）

#### `--format table`（デフォルト）

```
ID                Name              Real Name
────────────────────────────────────────────────────────────
U0123456789       daichi            堀越大地
U0987654321       hanako            山田花子
```

- 1行目: 固定文字列 `ID                Name              Real Name`
- 2行目: `─`（U+2500）を60回
- 各行: `${id.padEnd(17)} ${name.padEnd(17)} ${realName}`（すべて `sanitizeSingleLineText` 適用、undefined は `''`）

#### `--format simple`

タブ区切り（TSV）。

```
U0123456789	daichi	堀越大地
U0987654321	hanako	山田花子
```

`${id}\t${name}\t${realName}`。undefined は空文字。

#### `--format json`

```json
[
  {
    "id": "U0123456789",
    "name": "daichi",
    "real_name": "堀越大地"
  }
]
```

`name` / `real_name` は undefined のとき **`null` ではなく空文字 `""`**。

---

## 8. エラーケース・メッセージ文言・終了コード

### 8.1 終了コードのまとめ

| 発生源 | 終了コード | 出力先 |
|---|---|---|
| commander の `requiredOption` 未指定 | 1 | stderr |
| commander の未知オプション | 1 | stderr |
| `createValidationHook` → `thisCommand.error()` | 1（commanderの `error()` のデフォルト） | stderr |
| `wrapCommand` が捕捉した全例外 | **1**（`process.exit(1)`） | stderr |
| 正常終了（0件表示を含む） | 0 | stdout |

**終了コードは成功=0 / 失敗=1 の2値のみ。** エラー種別ごとの細分化（2, 3, ...）は実装されていない。

### 8.2 `wrapCommand` のエラー出力形式

```ts
console.error(
  chalk.red('✗ Error:'),
  redactSlackTokens(sanitizeTerminalText(extractErrorMessage(error)))
);
if (process.env.NODE_ENV === 'development' && error instanceof Error) {
  console.error(chalk.gray(redactSlackTokens(sanitizeTerminalText(error.stack ?? ''))));
}
process.exit(1);
```

出力例:
```
✗ Error: Channel 'genral' not found. Did you mean one of these? general
```

処理順序が重要:
1. `extractErrorMessage(error)` でメッセージ化
2. `sanitizeTerminalText` で ANSI/OSC エスケープと制御文字を除去（**先にサニタイズするのは、エスケープ列で分断されたトークンも確実にマスクするため**。コード内コメントに明記あり）
3. `redactSlackTokens` で `xox[bpoars]-...` を `xoxb-***-REDACTED` 形式に置換
4. `NODE_ENV === 'development'` のときのみ、同じ処理を通したスタックトレースを gray で追加出力

### 8.3 `extractErrorMessage` の特殊処理（missing_scope）

```
error が Error かつ
  getSlackErrorCode(error) === 'missing_scope' かつ
  getSlackNeededScopes(error).length > 0
→ `${error.message} (needed: ${scopes.join(', ')})`
それ以外 → error.message（Error でなければ String(error)）
```

`getSlackErrorCode` は `error.data.error` を見る。それが無い場合、`error.message` に `missing_scope` という文字列が含まれていれば `'missing_scope'` を返すフォールバックあり。
`getSlackNeededScopes` は `error.data.needed` をカンマ分割・trim・空要素除去したもの。

出力例:
```
✗ Error: An API error occurred: missing_scope (needed: channels:read, groups:read)
```

### 8.4 コマンド別のエラーケース

| # | ケース | メッセージ | 発生箇所 | 終了コード |
|---|---|---|---|---|
| E1 | `-c` 未指定（channel info / set-topic / set-purpose / join / leave / invite / members） | `error: required option '-c, --channel <channel>' not specified` | commander | 1 |
| E2 | `--topic` 未指定（set-topic） | `error: required option '--topic <topic>' not specified` | commander | 1 |
| E3 | `--purpose` 未指定（set-purpose） | `error: required option '--purpose <purpose>' not specified` | commander | 1 |
| E4 | `-u` 未指定（invite） | `error: required option '-u, --users <users>' not specified` | commander | 1 |
| E5 | `--format` 不正値（**channel info / members のみ**） | `error: Error: Invalid format '<値>'. Must be one of: table, simple, json` | `createValidationHook` → `thisCommand.error()` | 1 |
| E6 | `--format` 不正値（**channels**） | エラーにならない。table にフォールバックして正常終了 | `FormatterFactory.create()` | 0 |
| E7 | プロファイルの設定がない | `✗ Error: No configuration found for profile "<名前>". Use "slack-cli config set --token <token> --profile <名前>" to set up.` | `getConfigOrThrow` → `ConfigurationError` | 1 |
| E8 | チャンネル名が見つからない（類似候補あり） | `✗ Error: Channel '<名前>' not found. Did you mean one of these? <候補をカンマ区切りで最大5件>` | `ChannelResolver.resolveChannelError` → `ApiError` | 1 |
| E9 | チャンネル名が見つからない（類似候補なし） | `✗ Error: Channel '<名前>' not found. Make sure you are a member of this channel.` | 同上 | 1 |
| E10 | `invite` で有効なユーザーIDが0件（例 `-u ",, "`） | `✗ Error: At least one valid user ID is required` | `invite.ts` の素の `Error` | 1 |
| E11 | Slack API エラー（`channel_not_found`, `not_in_channel`, `already_in_channel`, `cant_invite_self` など） | `✗ Error: <@slack/web-api が生成したメッセージ>` | WebClient | 1 |
| E12 | スコープ不足 | `✗ Error: <元メッセージ> (needed: <スコープ>)` | §8.3 | 1 |

E7 のプロファイル名決定ロジック（`getConfigOrThrow`）: `profile` 引数 → `listProfiles()` の中で `isDefault` の名前 → `'default'` の優先順。

E8 の類似候補は `getSimilarChannels`: `c.name?.toLowerCase().includes(入力.toLowerCase())` で部分一致するものを**先頭から最大5件**。距離計算などはしていない。

### 8.5 定義されているエラークラス（`src/utils/errors.ts`）

| クラス | 継承 | `code` |
|---|---|---|
| `SlackCliError` | `Error` | 引数で指定 |
| `ConfigurationError` | `SlackCliError` | `'CONFIGURATION_ERROR'` |
| `ValidationError` | `SlackCliError` | `'VALIDATION_ERROR'` |
| `ApiError` | `SlackCliError` | `'API_ERROR'` |
| `FileError` | `SlackCliError` | `'FILE_ERROR'` |

`code` は保持されるだけで、**終了コードへのマッピングには使われていない**（すべて 1）。

---

## 9. ページネーション・レート制限・並行実行の一覧

| コマンド | ページネーション | 並行実行 | レート制限対策 |
|---|---|---|---|
| `channels` | `conversations.list` を cursor が尽きるまで全ページ。`--limit` はページサイズのみ | なし（直列） | なし（`retries: 0`、429時は即エラー） |
| `channel info` | 名前解決時の `conversations.list` は全ページ（`limit: 1000`）。`conversations.info` は1回 | なし | なし |
| `channel set-topic` | 名前解決時のみ全ページ | なし | なし |
| `channel set-purpose` | 名前解決時のみ全ページ | なし | なし |
| `join` | 名前解決時のみ全ページ | なし | なし |
| `leave` | 名前解決時のみ全ページ | なし | なし |
| `invite` | 名前解決時のみ全ページ | なし | なし |
| `members` | 名前解決時のみ全ページ。**`conversations.members` はページングしない（1回のみ）** | `users.info` を `Promise.all` で発火、`pLimit(3)` で3並列に制限 | `users.info` 失敗は握りつぶしてIDのみ表示。リトライなし |

`RATE_LIMIT.RETRY_CONFIG` / `BATCH_SIZE` / `BATCH_DELAY_MS` / `handleRateLimit()` は定義されているが、**本書の8コマンドの経路では一切使われない**（unread系の経路専用）。

---

## 10. Rustへ移すときに引っかかりそうな点

### 10.1 commander の暗黙挙動

1. **`requiredOption` のエラー文言** — commander は `error: required option '-c, --channel <channel>' not specified` という定型文を出す。clap のデフォルト文言（`error: the following required arguments were not provided:`）とは全く違う。互換を取るなら clap のエラーフォーマットを差し替える必要がある。
2. **`createValidationHook` の二重 `Error:`** — `thisCommand.error('Error: ' + msg)` に対して commander がさらに `error: ` を前置するため、実出力は `error: Error: Invalid format ...` になる。バグに見えるが現行挙動なので、互換優先なら再現する。
3. **`--include-archived` / `--force` の真偽値の扱い** — commander は値なしフラグを「未指定なら `false`（`.option(..., false)` でデフォルト指定した場合）/ `undefined`（指定しなかった場合）」に分ける。`--force` はデフォルト未指定なので `undefined` になり、`...(force && { force })` によりリクエストからキーごと消える。Rust の `bool` はこの三状態を持てないので、`Option<bool>` かフラグの有無を別途保持する必要がある。
4. **`channel` 親コマンドに action がない** — `slack-cli channel` 単体はヘルプ表示。clap では `arg_required_else_help` / `subcommand_required` で再現。
5. **postAction hook のアップデート通知** — 全コマンド実行後に走る。実装詳細（`update-notifier.ts`）は本調査で未読のため**不明**。移植時は別途調査が必要。

### 10.2 パースのゆるさをどこまで再現するか

6. **`parseInt` の緩さ** — `parseInt('abc', 10)` は `NaN`、`parseInt('12abc', 10)` は `12`。Rust の `str::parse::<u32>()` は両方エラーになる。`--limit abc` で `NaN` がAPIに乗る現行挙動は明らかにバグなので、**互換を捨てて厳格化する判断を明示的に下すべき**。
7. **`--type` / `--format` の不正値がサイレントにフォールバックする（`channels` のみ）** — clap の `value_parser!` で enum にすると自動的にエラーになるため、挙動が変わる。`channels` だけバリデーションが漏れているのは一貫性の欠如なので、Rust側では全コマンドで検証する方向に揃えるのが自然（ただし破壊的変更）。
8. **`--format` バリデータのリスト不一致** — `optionValidators.format` は `['table','simple','json']`、一方 `formatValidators.outputFormat`（本書対象コマンドからは未使用）は `['table','simple','json','compact']`。Rust で1つの enum に統一するとどちらに寄せるか決める必要がある。
9. **チャンネルID判定の正規表現が2種類ある** — `ChannelResolver.isChannelId` は `/^[CDG][A-Z0-9]{8,}$/`、`formatValidators.channelId` は `/^[CDG][A-Z0-9]{10,}$/`。実際に使われるのは前者。Rust では `regex` クレート or 手書き判定で、**8文字以上の方**を採用する。

### 10.3 出力の互換性

10. **`padEnd` はUTF-16コードユニット数** — 日本語チャンネル名や絵文字が入ると桁が崩れる（現行実装が既に崩れている）。Rust で `format!("{:<17}")` を使うと**Unicodeスカラー値の数**でパディングされ、TSと結果が変わる。厳密互換を狙うなら UTF-16 長を数える必要があるが、実用上は表示幅（`unicode-width`）で揃えたほうが良い。挙動が変わることは明示すべき。
11. **`substring(0, 27)` も UTF-16 基準** — サロゲートペア（絵文字など）を分断して不正な文字列を作りうる。Rust の `&s[..27]` は UTF-8 バイト境界外でパニックするので、`chars().take(27)` などに置き換える必要がある（＝出力が変わる）。
12. **`toLocaleDateString()` の非決定性** — `channel info --format table` の `Created:` はロケール・タイムゾーン依存。日本ロケールなら `2019/4/1`、en-US なら `4/1/2019`。Rust には同等の「実行環境ロケール依存の日付整形」が標準にない。**フォーマットを固定する（例 `%Y-%m-%d`）か、`icu` 系クレートを入れるかの判断が必要**。
13. **`created + 'T00:00:00Z'`（channels の JSON）** — Unix秒 → UTC日付文字列 → 時刻部を捨てて `T00:00:00Z` を付け直す、という情報を落とす変換。素直に RFC3339 にすると出力が変わる。互換維持ならこの奇妙な変換をそのまま実装する。
14. **`JSON.stringify` の undefined 省略** — `channel info --format json` で `num_members` が undefined のときキーごと消える。Rust では `#[serde(skip_serializing_if = "Option::is_none")]` で再現。一方 `members --format json` は undefined を `''` に落としているので、**同じ「値なし」でもコマンドごとに扱いが違う**点に注意。
15. **`chalk` の色付けが混在** — `join` / `leave` / `invite` の成功メッセージは `chalk.green`、`channel set-topic` / `set-purpose` は色なし。`channel info --format table` は bold/gray。chalk は TTY 判定と `NO_COLOR`/`FORCE_COLOR` を自動で見る。Rust では `owo-colors` + `supports-color` などで同等の判定を自前で入れる必要がある。
16. **成功メッセージの `#` 後ろが未解決の入力値** — `-c C0123456789` を渡すと `✓ Joined channel #C0123456789` という不自然な出力になる。かつ `sanitizeTerminalText` を通していないので、**エスケープ列を含むチャンネル名を渡すとターミナルに注入されうる**（他の出力箇所は全部サニタイズしているのに、ここだけ漏れている）。移植時は修正候補。

### 10.4 非同期・並行まわり

17. **`pLimit` の共有** — `rateLimiter` は `SharedSlackClientContext` として全 operations インスタンスで共有されている。Rust では `Arc<Semaphore>`（tokio）を各 operations に配る形になる。**ただし現行実装は `getUserInfo` などごく一部でしか `rateLimiter` を通していない**（`conversations.list` などは素通し）。安易に全API呼び出しをセマフォで包むと挙動が変わる。
18. **`Promise.all` + `pLimit` の順序保証** — `members` の結果配列は入力順を保つ。Rust で `futures::stream::iter(...).buffer_unordered(3)` を使うと順序が崩れるので、`buffered(3)` を使うか、後でソートし直す。
19. **`channelLookupCache` のメモ化** — `Promise<Channel[]>` を保持し、失敗時のみ無効化する「in-flight を共有するキャッシュ」。Rust では `tokio::sync::OnceCell` だと失敗後の再試行ができないので、`Mutex<Option<...>>` + 明示的な invalidate か `async_once_cell` 系が要る。ただしCLIは1プロセス1コマンドなので、実際にキャッシュが効くのは同一コマンド内で `resolveChannelId` が複数回呼ばれるケースのみ。本書対象の8コマンドはいずれも1回しか呼ばないため、**実質不要とも言える**。
20. **`retries: 0` の意図** — 「レート制限を手動で扱うため」とコメントにあるが、本書対象コマンドの経路には手動処理が実装されていない。Rust に移す際、`reqwest-retry` などを安易に入れると挙動が変わる。現状の「429で即失敗」を再現するか、改善するかを決める。

### 10.5 その他

21. **`process.exit(1)` は即時終了** — Node の `process.exit` はバッファ未フラッシュのまま落ちうる。Rust の `std::process::exit` も同様なので、stdout を明示的に flush してから抜ける。
22. **`sanitizeTerminalText` の仕様** — OSC列（`]...(BEL|ESC\)`）→ ANSI列（`(?:[@-Z\\-_]|\[[0-?]*[ -/]*[@-~])`）の順で除去したあと、1文字ずつ走査して「C0制御文字（0x00-0x1F、ただし TAB 0x09 と LF 0x0A は残す）・DEL 0x7F・C1制御文字（0x80-0x9F）」を落とす。`for...of` で回しているのでコードポイント単位（サロゲートペアは壊れない）。Rust の `chars()` と同じ粒度。
23. **`redactSlackTokens` の正規表現** — `/xox[bpoars]-[A-Za-z0-9-]+/gi`。マッチした文字列の先頭4文字を小文字化して `<prefix>-***-REDACTED` に置換。`i` フラグがあるので `XOXB-...` も `xoxb-***-REDACTED` になる。
24. **設定ファイルの場所と暗号化** — `~/.slack-cli/config.json`（`ProfileConfigManager`）。トークンは `TokenCryptoService` で暗号化保存され、旧形式・平文は読み込み時に再暗号化して書き戻す。ディレクトリ 0o700 / ファイル 0o600。**暗号化アルゴリズムの詳細は `token-crypto-service.ts` 未読のため不明**。Rust移植時は「既存の TS 版が書いた config.json を読めるか」が要件になるなら、この実装の解析が必須。
25. **`--profile` の解決順** — `parseProfile` は値をそのまま返すだけ。実際の解決は `ProfileConfigManager.getConfig`: `引数の profile` → `store.defaultProfile` → `'default'`。
26. **`invite` のユーザーID未検証** — 名前（`@daichi` など）を渡してもそのまま `conversations.invite` に送られ、Slack側でエラーになる。他コマンドの `resolveUserIdByName` は使われていない。Rust側で親切に解決するかは仕様判断。
