# 共通クライアント層 仕様書（API / 認証）

TypeScript 実装 `slack-cli` の以下のファイルを読んで抽出した仕様。Rust 移植時の再実装対象。

読んだファイル（すべて `/Users/mimo/organizations/open-source/slack-cli/src/` 配下）:

- `utils/slack-api-client.ts`
- `utils/slack-client-service.ts`
- `utils/client-factory.ts`
- `utils/channel-resolver.ts`
- `utils/token-crypto-service.ts`
- `utils/token-utils.ts`
- `utils/profile-config.ts`
- `utils/config-helper.ts`
- 補助として読んだもの: `utils/constants.ts`, `utils/errors.ts`, `utils/error-utils.ts`, `utils/terminal-sanitizer.ts`, `utils/slack-operations/base-client.ts`, `utils/slack-operations/channel-operations.ts`, `types/config.ts`, `types/slack.ts`, `utils/slack-operations/search-operations.ts`（`listUnreadChannels` 部分のみ）

本書に書いていない挙動（各 `slack-operations/*` の詳細など）は本タスクのスコープ外であり、**未読**。

---

## 0. 全体の依存関係

```
CLI コマンド
  └─ createSlackClient(profile?)            [client-factory.ts]
       └─ getConfigOrThrow(profile?)        [config-helper.ts]
            └─ ProfileConfigManager         [profile-config.ts]
                 └─ TokenCryptoService      [token-crypto-service.ts]
       └─ new SlackApiClient(token)         [slack-client-service.ts]
            └─ createSlackClientContext(token) [slack-operations/base-client.ts]
                 └─ WebClient + p-limit(3)
            └─ 各 *Operations（ChannelOperations 等）
                 └─ ChannelResolver         [channel-resolver.ts]
```

---

## 1. `slack-api-client.ts`（re-export のみ）

ファイル全体が 2 行。ロジックなし。

```ts
export * from '../types/slack';
export { SlackApiClient, slackApiClient } from './slack-client-service';
```

Rust では独立モジュール不要。`pub use` 相当で足りる。

---

## 2. `config-helper.ts`

### `getConfigOrThrow(profile?: string, configManager = new ProfileConfigManager()): Promise<{ token: string }>`

- **入力**: プロファイル名（省略可）、設定マネージャ（DI 用、既定は新規インスタンス）
- **処理**:
  1. `configManager.getConfig(profile)` を呼ぶ。
  2. 結果が `null` の場合のみエラー。エラーメッセージに使うプロファイル名は次の優先順で決める:
     - `profile` 引数（非空なら採用）
     - `listProfiles()` の中で `isDefault === true` のものの `name`
     - 文字列 `"default"`
  3. `ConfigurationError(ERROR_MESSAGES.NO_CONFIG(profileName))` を throw。
  4. `null` でなければ `Config`（`{ token, updatedAt }`）をそのまま返す。返り値の型注釈は `{ token: string }` だが実体は `Config` オブジェクトそのもの。
- **副作用**: `getConfig` 経由でトークン再暗号化の書き込みが起こりうる（3 章参照）。失敗時に `listProfiles()` で設定ファイルをもう一度読む。
- **エラー文言**（`constants.ts`）:
  ```
  No configuration found for profile "{profileName}". Use "slack-cli config set --token <token> --profile {profileName}" to set up.
  ```

---

## 3. `profile-config.ts` — `ProfileConfigManager`

### 保存場所

- 設定ディレクトリ: `options.configDir` が指定されればそれ、なければ `${HOME}/.slack-cli`
- 設定ファイル: `<configDir>/config.json`
- ディレクトリ作成 mode: `0o700`（`FILE_PERMISSIONS.CONFIG_DIR`）
- ファイル作成 mode: `0o600`（`FILE_PERMISSIONS.CONFIG_FILE`）

### 保存フォーマット（`ConfigStore`）

```json
{
  "profiles": {
    "default": { "token": "<暗号化文字列>", "updatedAt": "<ISO8601>" }
  },
  "defaultProfile": "default"
}
```

- `JSON.stringify(store, null, 2)`（インデント 2 スペース）で書く。
- 旧フォーマット（マイグレーション対象）: トップレベルに `{ "token": ..., "updatedAt": ... }` があり `profiles` が無い形。

### 書き込み手順 `saveConfigStore`（原子的書き込み）

1. `mkdir -p <configDir>`（mode `0o700`）
2. 一時ファイルパス = `` `${configPath}.${process.pid}.${Date.now()}.tmp` ``
3. 一時ファイルに `flag: 'wx'`（排他作成。既存なら EEXIST で失敗）、mode `0o600`、UTF-8 で書く
4. `rename(tempPath, configPath)`
5. rename が失敗したら一時ファイルを `unlink`（失敗は握り潰す）し、元のエラーを再 throw

Rust 移植注: 一時ファイル名に PID とミリ秒エポックを含める。`wx` は `OpenOptions::new().write(true).create_new(true)`。mode は `std::os::unix::fs::OpenOptionsExt::mode`。

### 読み込み手順 `getConfigStore`（private）

1. `config.json` を UTF-8 で読む。
2. JSON パース。
3. `needsMigration(parsed)` が真（= `parsed.token` が truthy かつ `parsed.profiles` が falsy）なら `migrateOldConfig` を実行してその戻り値を返す。
4. それ以外はパース結果をそのまま `ConfigStore` として返す（スキーマ検証は無し）。
5. エラー分岐:
   - `ENOENT` → `{ profiles: {} }` を返す（エラーにしない）
   - JSON パースエラー（`SyntaxError`）→ `ConfigurationError('Invalid config file format')`
   - それ以外 → そのまま再 throw

### `migrateOldConfig(oldData)`（private）

1. `oldData.token` が `isEncrypted` なら復号、そうでなければ平文としてそのまま使う。
2. その平文を **現行形式で再暗号化**。
3. `{ profiles: { "default": { token: <暗号化>, updatedAt: oldData.updatedAt } }, defaultProfile: "default" }` を作る。
4. `saveConfigStore` で保存し、その store を返す。

### プロファイル解決順序（全メソッド共通）

`setToken` / `getConfig` / `clearConfig` はいずれも同じ順序:

1. 引数 `profile`（非空文字列なら採用）
2. `store.defaultProfile`
3. 定数 `"default"`（`DEFAULT_PROFILE_NAME`）

環境変数によるプロファイル指定は**この層には無い**。

### 公開メソッド

#### `setToken(token: string, profile?: string): Promise<void>`

- プロファイル名を上記順序で決定。
- `config = { token: cryptoService.encrypt(token), updatedAt: new Date().toISOString() }`
- `store.profiles[profileName] = config`
- `store.defaultProfile` が未設定、**または** プロファイル名が `"default"` のとき、`store.defaultProfile = profileName` にする。
- 保存。

#### `getConfig(profile?: string): Promise<Config | null>`

- プロファイル名を決定 → `store.profiles[profileName]` が無ければ `null` を返す。
- `decryptToken(config.token)`:
  - `cryptoService.isEncrypted(token)` が真なら `decrypt`
  - 偽なら平文としてそのまま返す（後方互換）
- **副作用（重要）**: `cryptoService.isCurrentFormat(config.token)` が偽（= 平文 or レガシー形式）なら、復号結果を現行形式で暗号化し直して `store` に書き戻し、`saveConfigStore` する。つまり読み取り操作がディスク書き込みを起こす。
- 返り値は `{ ...config, token: <復号済み平文> }`。

#### `listProfiles(): Promise<Profile[]>`

- `currentProfile = store.defaultProfile ?? "default"`
- `Object.entries(store.profiles)` を `{ name, config, isDefault: name === currentProfile }` にマップして返す。
- **復号しない**。`config.token` は暗号化文字列のまま。
- 順序は JS のオブジェクトキー挿入順。Rust では順序保持マップ（`IndexMap` 等）が必要。JSON パース時のキー順を保つこと。

#### `useProfile(profile: string): Promise<void>`

- `store.profiles[profile]` が無ければ `ConfigurationError('Profile "{profile}" does not exist')`
- あれば `store.defaultProfile = profile` にして保存。

#### `getCurrentProfile(): Promise<string>`

- `store.defaultProfile ?? "default"` を返す。

#### `clearConfig(profile?: string): Promise<void>`

- プロファイル名を決定し `delete store.profiles[profileName]`。
- 削除対象が `defaultProfile` だった場合:
  - 残りのプロファイルがあれば、**キー順の先頭**を新しい `defaultProfile` にして保存。
  - 残りが 0 件なら `config.json` を `unlink` し、そこで return（保存しない）。`unlink` が `ENOENT` 以外で失敗したら再 throw、`ENOENT` は無視。
- 削除対象が default でなかった場合はそのまま保存。

#### `maskToken(token: string): string`

`token-utils.ts` の `maskToken` に委譲するだけ。

---

## 4. `token-crypto-service.ts` — `TokenCryptoService`

### 定数（フィールド値そのまま）

| 名前 | 値 |
| --- | --- |
| `algorithm` | `aes-256-gcm` |
| `legacyAlgorithm` | `aes-256-cbc` |
| `keyLength` | `32`（バイト） |
| `ivLength` | `12`（バイト。GCM） |
| `legacyIvLength` | `16`（バイト。CBC） |
| `authTagLength` | `16`（バイト） |
| `separator` | `":"` |
| `version` | `"v2"` |
| `masterKeySalt` | `"slack-cli-master-key-salt-v2"` |
| `masterKeyIterations` | `100000` |

コンストラクタオプション `TokenCryptoServiceOptions`: `{ masterKey?, keyFilePath?, legacyKeyFilePath? }`。
なお `ProfileConfigManager` は `new TokenCryptoService()`（引数なし）で生成する。

### 鍵ファイル

- 現行: `keyFilePath` 指定がなければ `${HOME}/.slack-cli-secrets/master.key`
- レガシー: `legacyKeyFilePath` 指定がなければ `${HOME}/.slack-cli/master.key`
- ファイル内容: **64 桁の小文字/大文字 hex + 末尾改行**。書き込み時は `` `${keyHex}\n` ``。
- 検証正規表現: `/^[0-9a-f]{64}$/i`（trim 後）。不一致なら `ConfigurationError('Invalid token encryption key format')`。
- 書き込み: ディレクトリを `mkdir -p` mode `0o700`、ファイルは `flag: 'wx'` + mode `0o600` + UTF-8。

### マスターキー解決順序 `getMasterKey()`

1. インスタンス内キャッシュ `cachedMasterKey`（プロセス内で 1 回だけ導出）
2. コンストラクタ注入 `options.masterKey` → `deriveMasterKey(secret)` で PBKDF2 導出
3. 環境変数 `SLACK_CLI_MASTER_KEY`（`.trim()` して非空なら採用）→ `deriveMasterKey`
4. 現行鍵ファイルを読む（**hex をそのまま 32 バイト鍵として使う。PBKDF2 は通さない**）
   - 読み込みが `ENOENT` の場合:
     - `migrateLegacyKeyFile()` を試す
       - レガシーファイルが `ENOENT` → `createKeyFile()`（新規ランダム生成）
       - レガシー側で `ConfigurationError` → そのまま throw
       - それ以外のエラー → `ConfigurationError('Failed to migrate token encryption key')`
   - `ENOENT` 以外で `ConfigurationError` → そのまま throw
   - それ以外 → `ConfigurationError('Failed to load token encryption key')`

**重要な非対称性**: 注入キー・環境変数は「パスフレーズ」として PBKDF2 に通すが、鍵ファイルの内容は **生の 32 バイト鍵**として直接使う。Rust でも必ずこの分岐を保つこと。

### `deriveMasterKey(secret)`（private）

```
PBKDF2-HMAC-SHA256(
  password = secret,
  salt     = "slack-cli-master-key-salt-v2",
  iter     = 100000,
  dkLen    = 32
)
```

### `deriveLegacyKey()`（private、復号専用）

```
PBKDF2-HMAC-SHA256(
  password = "slack-cli-key",
  salt     = "slack-cli-salt-v1",
  iter     = 100000,
  dkLen    = 32
)
```

固定値なのでコードを見れば誰でも復号できる。これは既存トークンの読み出し互換のためだけに存在。

### `createKeyFile()`（private）

1. 32 バイトの CSPRNG 乱数を hex 化。
2. `writeKeyFile` で `wx` 書き込み。
3. `EEXIST`（競合で他プロセスが先に作った）なら、そのファイルを読んで返す。
4. その他のエラーは `ConfigurationError('Failed to initialize token encryption key')`。

### `migrateLegacyKeyFile()`（private）

1. レガシー鍵ファイルを読む（無ければ `ENOENT` が伝播 → 呼び出し側で新規生成）。
2. 同じ hex を現行パスへ `wx` 書き込み。
3. 書き込みエラー時:
   - エラーが `code` を持たないオブジェクト → レガシー鍵をそのまま返す
   - `code !== 'EEXIST'`、または現行パスが存在しない → レガシー鍵をそのまま返す
4. 上記に該当しなければ現行パスを読み直して返す（成功時も同じ経路）。
5. レガシーファイルは**削除しない**。

### 暗号文フォーマット

**現行（v2）**: 4 セグメント、`:` 区切り

```
v2:<iv_hex 24桁>:<ciphertext_hex 偶数長・空文字可>:<authtag_hex 32桁>
```

- `iv` は 12 バイトのランダム。
- 暗号化は `AES-256-GCM`、AAD なし、平文は UTF-8、暗号文とタグは hex。

**レガシー（v1）**: 2 セグメント

```
<iv_hex 32桁>:<ciphertext_hex 偶数長・1桁以上>
```

- `AES-256-CBC`（PKCS#7 パディング、Node の既定）、鍵は `deriveLegacyKey()`。
- **復号のみサポート。新規に生成することはない。**

### 公開メソッド

#### `encrypt(token: string): string`

- マスターキー取得 → 12 バイト IV 生成 → AES-256-GCM で暗号化 → `v2:iv:ct:tag` を返す。
- 途中で発生した例外は**すべて握り潰して** `ConfigurationError('Failed to encrypt token')` に変換する（`getMasterKey` の詳細エラーも潰れる）。

#### `decrypt(encryptedData: string): string`

分岐:

1. 空文字（falsy）→ `ValidationError('Invalid encrypted data format')`
2. `isCurrentFormat` → v2 経路で復号
3. `isLegacyEncrypted` → v1 経路で復号
4. どちらでもない → `ValidationError('Invalid encrypted data format')`

catch:
- `ValidationError` はそのまま再 throw
- それ以外（鍵不一致・タグ検証失敗など全部）は `ConfigurationError('Failed to decrypt token')`

#### `isEncrypted(value: string): boolean`

`isCurrentFormat(value) || isLegacyEncrypted(value)`

#### `isCurrentFormat(value: string): boolean`（公開。`ProfileConfigManager` が再暗号化判定に使う）

真になる条件をすべて満たすこと:

- `value` が非空
- `value.split(':')` の要素数がちょうど 4
- `parts[0] === "v2"`
- `parts[1]`（iv）が `/^[0-9a-fA-F]+$/` かつ長さ 24
- `parts[2]`（ct）が空文字、または `/^[0-9a-fA-F]+$/`
- `parts[2].length % 2 === 0`
- `parts[3]`（tag）が `/^[0-9a-fA-F]+$/` かつ長さ 32

#### `isLegacyEncrypted(value)`（private）

- 非空
- 分割要素数がちょうど 2
- `parts[0]` が hex かつ長さ 32
- `parts[1]` が hex、長さ > 0、偶数長

注: Slack トークン（`xoxb-...`）は `:` を含まないので、平文トークンはどちらの判定にも該当しない。

---

## 5. `token-utils.ts`

### 定数

- `TOKEN_MASK_LENGTH = 4`
- `TOKEN_MIN_LENGTH = 9`

### `redactSlackTokens(text: string | undefined): string | undefined`

- 正規表現 `/xox[bpoars]-[A-Za-z0-9-]+/gi` にマッチする箇所すべてを置換。
- 置換後の文字列: マッチ先頭 4 文字を小文字化したもの + `-***-REDACTED`（例: `xoxb-***-REDACTED`）。
- 入力が `undefined` なら `undefined` を返す。
- 用途: スタックトレース等へのトークン混入防止。

### `maskToken(token: string): string`

- `token.length <= 9` なら `"****"` を返す。
- それ以外は `` `${先頭4文字}-****-****-${末尾4文字}` ``。

---

## 6. `client-factory.ts`

### `createSlackClient(profile?: string): Promise<SlackApiClient>`

```
config = await getConfigOrThrow(profile)
return new SlackApiClient(config.token)
```

分岐なし。CLI の各コマンドはこれを唯一の入口として使う。

---

## 7. `slack-client-service.ts` — `SlackApiClient`

### 構造

コンストラクタ `new SlackApiClient(token: string)`:

1. `createSlackClientContext(token)` で共有コンテキスト（`WebClient` + レートリミッタ）を 1 つ作る。
2. 11 個の Operations をそのコンテキストを共有して生成する:
   - `ChannelOperations(ctx)`
   - `MessageOperations(ctx, channelOps)`
   - `FileOperations(ctx, channelOps)`
   - `ReactionOperations(ctx, channelOps)`
   - `PinOperations(ctx, channelOps)`
   - `UserOperations(ctx)`
   - `UsergroupOperations(ctx)`
   - `SearchOperations(ctx)`
   - `ReminderOperations(ctx)`
   - `StarOperations(ctx)`
   - `CanvasOperations(ctx, channelOps)`

`channelOps` を共有していることが重要: チャンネル名→ID の解決キャッシュ（後述）がクライアント全体で 1 つになる。

### 公開メソッド一覧（ほぼ全部が単純な委譲）

| メソッド | 委譲先 |
| --- | --- |
| `sendMessage(channel, text, thread_ts?, blocks?)` | messageOps |
| `sendEphemeralMessage(channel, user, text, thread_ts?, blocks?)` | messageOps |
| `scheduleMessage(channel, text, post_at, thread_ts?, blocks?)` | messageOps |
| `updateMessage(channel, ts, text, blocks?)` | messageOps |
| `deleteMessage(channel, ts)` | messageOps |
| `listScheduledMessages(channel?, limit = 50)` | messageOps |
| `cancelScheduledMessage(channel, scheduledMessageId)` | messageOps |
| `listChannels(options: ListChannelsOptions)` | channelOps |
| `getChannelDetail(channelNameOrId)` | channelOps |
| `setTopic(channelNameOrId, topic)` | channelOps |
| `setPurpose(channelNameOrId, purpose)` | channelOps |
| `getHistory(channel, options: HistoryOptions)` | messageOps |
| `getThreadHistory(channel, threadTs)` | messageOps |
| `listUnreadChannels()` | **下記の特殊分岐あり** |
| `getChannelUnread(channelNameOrId)` | messageOps |
| `markAsRead(channelId)` | messageOps |
| `getPermalink(channel, messageTs) -> string \| null` | messageOps |
| `getPermalinks(channel, messageTimestamps[]) -> Map<ts, url>` | messageOps |
| `uploadFile(options: UploadFileOptions)` | fileOps |
| `downloadFile(options: DownloadFileOptions)` | fileOps |
| `addReaction(channel, timestamp, emoji)` | reactionOps |
| `removeReaction(channel, timestamp, emoji)` | reactionOps |
| `addPin(channel, timestamp)` / `removePin` / `listPins(channel)` | pinOps |
| `listUsers(limit?)` | userOps |
| `getUserInfo(userId)` | userOps |
| `lookupUserByEmail(email)` | userOps.lookupByEmail |
| `openDmChannel(userId) -> string` | userOps |
| `getUserPresence(userId)` | userOps.getPresence |
| `resolveUserIdByName(username) -> string` | userOps |
| `listUsergroups(includeDisabled?)` | usergroupOps |
| `listUsergroupMembers(usergroupId) -> string[]` | usergroupOps |
| `resolveUsergroupIdByHandle(handle) -> string` | usergroupOps |
| `searchMessages(query, options?)` | searchOps |
| `joinChannel` / `leaveChannel` | channelOps |
| `inviteToChannel(channelNameOrId, userIds[], force?)` | channelOps |
| `getChannelMembers(channelNameOrId, options?)` | channelOps |
| `addReminder(text, time)` / `listReminders()` / `deleteReminder(id)` / `completeReminder(id)` | reminderOps |
| `addStar(channel, timestamp)` / `listStars(count?)` / `removeStar(channel, timestamp)` | starOps |
| `readCanvas(canvasId)` / `listCanvases(channel)` | canvasOps |

デフォルト値はここで与えられているものだけ列挙: `listScheduledMessages` の `limit = 50`。

### `listUnreadChannels()` の分岐（唯一のロジック）

```ts
try {
  const channels = await this.searchOps.listUnreadChannels();
  return await this.channelOps.enrichUnreadChannels(channels);
} catch {
  return this.channelOps.listUnreadChannels();
}
```

- 第一経路: search API（`is:unread` 系クエリ、詳細は search-operations 側）で未読チャンネルを引き、`conversations.info` で肉付け。
- 第一経路が**どんな理由でも**失敗したらフォールバックとして `channelOps.listUnreadChannels()`（全チャンネル走査）を使う。エラー内容は見ない。
- `enrichUnreadChannels` 内の失敗も含めて catch される点に注意。

### モジュールレベル `slackApiClient` オブジェクト

```ts
export const slackApiClient = {
  listChannels: async (token, options) => new SlackApiClient(token).listChannels(options),
};
```

呼び出しごとに新しいクライアント（＝新しいチャンネルキャッシュ）を作る。

---

## 8. `channel-resolver.ts` — `ChannelResolver`

状態を持たない純粋なヘルパー。シングルトン `channelResolver` を export している。

### `isChannelId(s: string): boolean`

正規表現 `/^[CDG][A-Z0-9]{8,}$/`

- 先頭 1 文字が `C` / `D` / `G`
- 続いて大文字英数字が 8 文字以上
- **小文字は不可**、全体マッチ

### `findChannel(channelName: string, channels: Channel[]): Channel | undefined`

配列を先頭から走査し、次のいずれかを満たす最初の要素を返す:

1. `c.name === channelName`（完全一致）
2. `c.name === channelName.replace('#', '')`（**最初の `#` 1 個だけ**除去した文字列と一致。JS の `String.replace` に文字列を渡すと 1 回だけ置換される点に注意）
3. `c.name?.toLowerCase() === channelName.toLowerCase()`（大文字小文字無視）
4. `c.name_normalized === channelName`

Rust 移植注: 条件 2 の「`#` を 1 個だけ除去」は `replacen("#", "", 1)` 相当。`replace` 全置換にすると挙動が変わる。

### `getSimilarChannels(channelName, channels, limit = 5): string[]`

- `c.name.toLowerCase().includes(channelName.toLowerCase())` で部分一致フィルタ
- 先頭から `limit` 件（既定 5）を取り、`name` の配列にする
- `name` が undefined の要素は `c.name?.toLowerCase()` が undefined になりフィルタで落ちる

### `resolveChannelError(channelName, channels): ApiError`

- 候補あり（1 件以上）:
  ```
  Channel '{sanitized名}' not found. Did you mean one of these? {候補1, 候補2, ...}
  ```
  候補は `, ` 区切り。チャンネル名・候補名の双方に `sanitizeTerminalText` を適用する。
- 候補なし:
  ```
  Channel '{sanitized名}' not found. Make sure you are a member of this channel.
  ```

`sanitizeTerminalText` の仕様（`terminal-sanitizer.ts`）:

- OSC シーケンス `][^]*(?:|\\)` を除去
- ANSI シーケンス `(?:[@-Z\\-_]|\[[0-?]*[ -/]*[@-~])` を除去
- 残った文字のうち、`0x09`（TAB）と `0x0A`（LF）以外の制御文字（`< 0x20`、`0x7F`、`0x80..=0x9F`）を 1 文字ずつ削除
- 空文字入力は空文字を返す

### `resolveChannelId(channelNameOrId, getChannels): Promise<string>`

1. `isChannelId(channelNameOrId)` が真ならそのまま返す（**API 呼び出しなし**）
2. 偽なら `getChannels()` を await してチャンネル一覧を取得
3. `findChannel` で探し、見つからなければ `resolveChannelError` の `ApiError` を throw
4. 見つかれば `channel.id` を返す

---

## 9. チャンネル名→ID キャッシュ（`slack-operations/channel-operations.ts`）

`ChannelResolver` 自体はキャッシュを持たない。キャッシュは `ChannelOperations` 側にある。

### キャッシュの実体

```ts
private channelLookupCache?: Promise<Channel[]>;
```

- **Promise をキャッシュする**（結果ではなく）。同時に複数の解決要求が来ても取得は 1 回だけ走る（in-flight 共有）。
- 取得が reject したら `channelLookupCache = undefined` に戻してエラーを再 throw する。次回は再取得される。
- **TTL なし、永続化なし、無効化 API なし**。`SlackApiClient` インスタンスの生存期間 = キャッシュ寿命。プロセス終了で消える。

Rust 移植注: `tokio::sync::OnceCell` はエラー時に再試行できるが、in-flight 共有の意味論を含めるなら `Arc<Mutex<Option<Shared<BoxFuture<...>>>>>` か `async_once_cell` + 失敗時クリアの自前実装が要る。

### `fetchChannelLookupChannels(types = ['public_channel','private_channel','im','mpim'])`

1. `listChannels({ types: types.join(','), exclude_archived: true, limit: 1000 })` を呼ぶ（`DEFAULTS.CHANNELS_LIMIT = 1000`）
2. 失敗したら `getFallbackChannelLookupTypes(error, types)` でフォールバック種別を計算
3. フォールバックが `null` なら元のエラーを再 throw、そうでなければ同じパラメータで種別だけ絞って再試行（**再試行は 1 回だけ**）

### `getFallbackChannelLookupTypes(error, requestedTypes)`

- `getSlackErrorCode(error) !== 'missing_scope'` なら `null`
  - `getSlackErrorCode`: エラーが `Error` かつ `error.data.error` が非空文字列ならそれ。無ければ `error.message` が `"missing_scope"` を含むとき `'missing_scope'`。それ以外 `undefined`。
- `getSlackNeededScopes(error)`（`error.data.needed` をカンマ分割・trim・空要素除去）を次の表でチャンネル種別に変換:

  | scope | channel type |
  | --- | --- |
  | `channels:read` | `public_channel` |
  | `groups:read` | `private_channel` |
  | `im:read` | `im` |
  | `mpim:read` | `mpim` |

- 変換結果が 0 件なら `null`
- 要求種別からブロック種別を除いた残りを作り、**残りが 0 件、または元と同じ件数**なら `null`
- それ以外は残りの種別配列を返す

---

## 10. ページネーション

### `listChannels(options: ListChannelsOptions): Promise<Channel[]>`

```
ListChannelsOptions = { types: string, exclude_archived: boolean, limit: number }
```

- `conversations.list({ types, exclude_archived, limit, cursor })` を do-while で呼ぶ。
- 各レスポンスの `channels` を全部連結。
- `cursor = response.response_metadata?.next_cursor`。
- **`cursor` が truthy な限りループ**。空文字 `""` / `undefined` / `null` で終了。上限ページ数の指定なし（無限ループ防止機構なし）。
- レートリミッタは通していない（この do-while は `rateLimiter` でラップされていない）。

### `fetchUserChannels(): Promise<Channel[]>`

- `users.conversations({ types: 'public_channel,private_channel,im,mpim', exclude_archived: true, limit: 200, cursor })`
- 同じ do-while パターン。ページサイズは固定 200。

### `getChannelMembers(channelNameOrId, options)`

- **ページネーションしない**。1 ページだけ返す。
- `conversations.members({ channel: <解決済みID>, limit: options.limit ?? 100, cursor: options.cursor })`
- 返り値 `{ members: string[], nextCursor: string }`。`next_cursor` が無ければ空文字 `""`。
- カーソルの引き回しは呼び出し側の責務。

### search 系のページネーション（`SearchOperations.listUnreadChannels`）

- 1 ページ目を取得し、`messages.pagination.page_count`（無ければ 1）を読む。
- 2 ページ目以降は **全ページを同時に投げる**（`Promise.all`）が、各リクエストは共有 `rateLimiter`（同時実行 3）を通す。
- 全マッチを連結してチャンネル単位に集約。

---

## 11. レートリミットとリトライ

### クライアント生成 `createSlackClientContext(token)`

```ts
{
  client: new WebClient(token, {
    retryConfig: { retries: 0 },   // SDK の自動リトライを完全に無効化
    logLevel: LogLevel.ERROR,
  }),
  rateLimiter: pLimit(RATE_LIMIT.CONCURRENT_REQUESTS),  // = 3
}
```

Slack SDK 側の自動リトライは**切ってある**。リトライは自前実装のみ。

### `RATE_LIMIT` 定数（`constants.ts`）

| キー | 値 |
| --- | --- |
| `CONCURRENT_REQUESTS` | `3` |
| `UNREAD_SCAN_CONCURRENT_REQUESTS` | `15` |
| `BATCH_SIZE` | `10` |
| `BATCH_DELAY_MS` | `1000` |
| `RETRY_CONFIG.retries` | `3` |
| `RETRY_CONFIG.factor` | `2` |
| `RETRY_CONFIG.minTimeout` | `1000` |
| `RETRY_CONFIG.maxTimeout` | `30000` |

**注意**: 読んだ範囲では `factor` / `minTimeout` / `maxTimeout` / `BATCH_SIZE` / `BATCH_DELAY_MS` を実際に参照しているコードは見当たらなかった（`retries` のみ `fetchChannelInfo` が参照）。他ファイルでの使用有無は未確認。指数バックオフは**実装されていない**（後述の固定 5 秒待ちのみ）。

### `BaseSlackClient.handleRateLimit(error)`

```ts
if (error instanceof Error && error.message?.includes('rate limit')) {
  await sleep(5000);   // 固定 5 秒
}
```

- 判定は**エラーメッセージの文字列部分一致 `'rate limit'` のみ**。HTTP 429 も `Retry-After` ヘッダも見ていない。
- 該当しないエラーでは何も待たない（即 return）。

### `BaseSlackClient.delay(ms)`

単純な `setTimeout` ラッパー。

### リトライループの実例 `fetchChannelInfo(channelId)`

```
for attempt = 0, 1, 2, ... :
  try: return conversations.info({ channel: channelId, include_num_members: false }).channel
  catch e:
    isRateLimit = e is Error && e.message.includes('rate limit')
    if !isRateLimit || attempt >= 3: throw e     // RATE_LIMIT.RETRY_CONFIG.retries = 3
    await handleRateLimit(e)                      // 固定 5 秒待つ
```

- レート制限以外のエラーは即 throw。
- 最大試行回数: 初回 + リトライ 3 回 = **計 4 回**（`attempt` が 3 になった時点で throw なので、待ちが入るのは attempt=0,1,2 の 3 回）。

### 並列度の使い分け

- 共有 `rateLimiter`（並列 3）: search のページ取得など、`this.rateLimiter(...)` でラップされた呼び出し。
- 未読スキャン専用: `pLimit(15)` を**その場で新規生成**する（`listUnreadChannels` と `enrichUnreadChannels` でそれぞれ別インスタンス）。共有リミッタとは独立なので、未読スキャン中は最大 15 + 3 の並列が起こりうる。
- `listUnreadChannels` / `enrichUnreadChannels` 内の個々の失敗は `handleRateLimit` を挟んだ上で握り潰す（前者は `null` を返して結果から除外、後者は元のチャンネルをそのまま返す）。

---

## 12. エラー型（`errors.ts`）

| クラス | 親 | `code` |
| --- | --- | --- |
| `SlackCliError` | `Error` | 任意（コンストラクタ第 2 引数） |
| `ConfigurationError` | `SlackCliError` | `CONFIGURATION_ERROR` |
| `ValidationError` | `SlackCliError` | `VALIDATION_ERROR` |
| `ApiError` | `SlackCliError` | `API_ERROR` |
| `FileError` | `SlackCliError` | `FILE_ERROR` |

`name` は `this.constructor.name`（＝クラス名）が入る。

Rust では `enum SlackCliError { Configuration(String), Validation(String), Api(String), File(String) }` に `code()` を生やす形で対応可能。

---

## 13. 本層で使う定数の全列挙（`constants.ts` より）

```
TOKEN_MASK_LENGTH   = 4
TOKEN_MIN_LENGTH    = 9
DEFAULT_PROFILE_NAME = "default"

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

DEFAULTS.HISTORY_LIMIT                 = 20
DEFAULTS.CHANNELS_LIMIT                = 1000
DEFAULTS.UNREAD_DISPLAY_LIMIT          = 50
DEFAULTS.UNREAD_MESSAGE_PREVIEW_LIMIT  = 50

TIME_FORMAT = "YYYY-MM-DD HH:mm:ss"
```

### メッセージ文言（本層に関係するもの）

`ERROR_MESSAGES`:

```
NO_CONFIG(p)             = `No configuration found for profile "${p}". Use "slack-cli config set --token <token> --profile ${p}" to set up.`
PROFILE_NOT_FOUND(p)     = `Profile "${p}" not found`
NO_PROFILES_FOUND        = 'No profiles found. Use "slack-cli config set --token <token>" to create one.'
INVALID_CONFIG_FORMAT    = 'Invalid config file format'
API_ERROR(e)             = `API Error: ${e}`
CHANNEL_NOT_FOUND(c)     = `Channel not found: ${c}`
NO_CHANNELS_FOUND        = 'No channels found'
ERROR_LISTING_CHANNELS(e)= `Error listing channels: ${e}`
FILE_READ_ERROR(f, e)    = `Error reading file ${f}: ${e}`
FILE_NOT_FOUND(f)        = `File not found: ${f}`
```

（`NO_MESSAGE_OR_FILE` などメッセージ送信バリデーション系は本層のスコープ外なので割愛。実ファイルには存在する。）

`SUCCESS_MESSAGES`:

```
TOKEN_SAVED(p)       = `Token saved successfully for profile "${p}"`
PROFILE_SWITCHED(p)  = `Switched to profile "${p}"`
PROFILE_CLEARED(p)   = `Profile "${p}" cleared successfully`
MESSAGE_SENT(c)      = `Message sent successfully to #${c}`
MESSAGE_SCHEDULED(c, iso) = `Message scheduled to #${c} at ${iso}`
EPHEMERAL_MESSAGE_SENT(c) = `Ephemeral message sent to #${c}`
```

コード内直書きの文言:

```
'Invalid token encryption key format'
'Failed to initialize token encryption key'
'Failed to migrate token encryption key'
'Failed to load token encryption key'
'Failed to encrypt token'
'Failed to decrypt token'
'Invalid encrypted data format'
`Profile "${profile}" does not exist`
`Channel '${name}' not found. Did you mean one of these? ${a, b, c}`
`Channel '${name}' not found. Make sure you are a member of this channel.`
```

---

## 14. 型定義（`types/config.ts`, `types/slack.ts` の該当分）

```ts
interface Config       { token: string; updatedAt: string; }
interface Profile      { name: string; config: Config; isDefault?: boolean; }
interface ConfigStore  { profiles: Record<string, Config>; defaultProfile?: string; }
interface ConfigOptions{ configDir?: string; profile?: string; }

interface ListChannelsOptions { types: string; exclude_archived: boolean; limit: number; }
interface HistoryOptions      { limit: number; oldest?: string; }
interface ChannelMembersOptions { limit?: number; cursor?: string; }
interface ChannelMembersResult  { members: string[]; nextCursor: string; }
```

`Channel`（`channel-resolver` が参照するフィールドを含む全体）:

```ts
interface Channel {
  id: string;
  name: string;
  display_name?: string;
  user?: string;
  is_channel?: boolean;
  is_group?: boolean;
  is_im?: boolean;
  is_mpim?: boolean;
  is_private: boolean;
  created: number;
  is_archived?: boolean;
  is_general?: boolean;
  unlinked?: number;
  name_normalized?: string;
  is_shared?: boolean;
  is_ext_shared?: boolean;
  is_org_shared?: boolean;
  is_member?: boolean;
  num_members?: number;
  unread_count?: number;
  unread_count_display?: number;
  last_read?: string;
  topic?:   { value: string; creator?: string; last_set?: number };
  purpose?: { value: string; creator?: string; last_set?: number };
}
```

`HistoryResult` / `ChannelUnreadResult` は `users` フィールドが `Map<string, string>`（ユーザー ID → 表示名）。Rust では `HashMap<String, String>` 相当だが、JSON 直列化時の扱いは呼び出し側の実装次第（本タスクでは未確認）。

---

## 15. 使用している Slack API エンドポイント（本タスクで読んだ範囲）

| メソッド | 使用箇所 | 主なパラメータ |
| --- | --- | --- |
| `conversations.list` | `listChannels` | `types`, `exclude_archived`, `limit`, `cursor` |
| `users.conversations` | `fetchUserChannels` | `types`, `exclude_archived=true`, `limit=200`, `cursor` |
| `conversations.info` | `fetchChannelInfo` / `getChannelInfo` / `getChannelDetail` | `channel`, `include_num_members`（false / 省略 / true） |
| `conversations.members` | `getChannelMembers` | `channel`, `limit`, `cursor` |
| `conversations.setTopic` | `setTopic` | `channel`, `topic` |
| `conversations.setPurpose` | `setPurpose` | `channel`, `purpose` |
| `conversations.join` | `joinChannel` | `channel` |
| `conversations.leave` | `leaveChannel` | `channel` |
| `conversations.invite` | `inviteToChannel` | `channel`, `users`（カンマ結合）, `force`（真のときだけ付与） |
| `users.info` | `resolveChannelDisplayName` | `user` |
| `search.messages` | `SearchOperations` | 未詳細確認 |

ベース URL は Slack SDK（`@slack/web-api`）の既定（`https://slack.com/api/`）。コード内での明示指定は**無い**。

---

## 16. Rust 移植で注意が要る点（実装差になりやすい箇所）

1. **鍵解決の非対称性**: 注入キー/環境変数は PBKDF2 に通し、鍵ファイルは生バイト。混同すると既存トークンが復号できなくなる。
2. **`getConfig` の書き戻し**: 読み取り API が設定ファイルを書き換える。並行実行時の rename 競合は原子的書き込みで守られているが、内容の last-write-wins にはなる。
3. **キャッシュは Promise 共有**: 同時解決要求の重複排除まで含めて再現するなら future の共有が必要。
4. **プロファイル順序依存**: `clearConfig` の「残りの先頭を新 default」は JSON のキー順に依存する。順序保持マップが必須。
5. **`#` の 1 回だけ除去**: `replacen(..., 1)` を使うこと。
6. **レート制限判定が文字列一致**: Slack SDK の Rust クライアントではエラー表現が変わるため、429 / `Retry-After` ベースに置き換えるか、TS と同じ「メッセージに `rate limit` を含む」で揃えるかの方針決定が要る。
7. **ページネーションに上限がない**: `next_cursor` が返り続ける限り回る。Rust では上限ページ数を入れるかどうかが挙動差になる。
8. **例外の握り潰し**: `encrypt` / `decrypt` / `listUnreadChannels` は原因情報を捨てる。Rust で `anyhow` の source を残すと診断メッセージが TS と変わる。
9. **レガシー CBC 復号**: 固定パスフレーズ由来。`aes-256-cbc` + PKCS#7 を扱えるクレート（`aes` + `cbc` + `block-padding`）が必要。GCM は `aes-gcm`、PBKDF2 は `pbkdf2` + `hmac` + `sha2`。
