# `config` コマンド仕様（Rust移植用）

対象ソース（TypeScript版 `@mimo-3/slack-cli` v0.24.1）:

- `src/commands/config.ts`（コマンド定義）
- `src/commands/config-subcommands.ts`（ハンドラ実装）
- 参照した補助実装: `src/index.ts` / `src/utils/command-wrapper.ts` / `src/utils/profile-config.ts` / `src/utils/constants.ts` / `src/utils/token-utils.ts` / `src/utils/token-crypto-service.ts` / `src/utils/terminal-sanitizer.ts` / `src/utils/error-utils.ts` / `src/utils/errors.ts` / `src/utils/update-notifier.ts` / `src/types/config.ts`

## 0. 全体像（`src/index.ts`）

- バイナリ名（commander の `name()`）: `slack-cli`
- 説明: `CLI tool to send messages via Slack API`
- `--version` は `package.json` の `version` を、実行ファイルから見て `__dirname/../package.json` を読んで表示する。
- ルート `program` に `postAction` フックが1つ登録されており、**どのコマンドの実行後にも** `checkForUpdates()` が走る（詳細は §7）。
- ルートに登録されるコマンドは25個。登録順は
  `config, send, channels, history, unread, scheduled, search, edit, delete, upload, download, reaction, pin, users, usergroups, channel, members, send-ephemeral, join, leave, invite, reminder, bookmark, canvas, draft`。
  本書が扱うのは先頭の `config` のみ。
- `runCli(argv = process.argv)` が `program.parseAsync(argv)` を呼ぶだけ。エントリポイントは `require.main === module` 判定。

## 1. コマンド名・エイリアス・サブコマンド構造

親コマンド:

| 項目 | 値 |
| --- | --- |
| 名前 | `config` |
| 説明 | `Manage Slack CLI configuration` |
| エイリアス | **なし**（`alias()` の呼び出しは無い） |
| 親コマンド自身の action | **未定義**（サブコマンド必須。`slack-cli config` 単体の挙動は commander の既定に委ねられており、TSコード上に明示的な定義は無い＝**不明**） |

サブコマンド（全6個。エイリアスは全て無し）:

| サブコマンド | 使用法 | 説明 | ハンドラ |
| --- | --- | --- | --- |
| `set` | `slack-cli config set [options]` | `Set API token` | `handleSetToken` |
| `get` | `slack-cli config get [options]` | `Show current configuration` | `handleGetConfig` |
| `profiles` | `slack-cli config profiles` | `List all profiles` | `handleListProfiles` |
| `use` | `slack-cli config use <profile>` | `Switch to a different profile` | `handleUseProfile` |
| `current` | `slack-cli config current` | `Show current active profile` | `handleShowCurrentProfile` |
| `clear` | `slack-cli config clear [options]` | `Clear configuration` | `handleClearConfig` |

全ハンドラは `wrapCommand()` でラップされている（§6のエラー処理を参照）。

## 2. 位置引数

| サブコマンド | 位置引数 | 必須 | 意味 |
| --- | --- | --- | --- |
| `set` | なし | - | - |
| `get` | なし | - | - |
| `profiles` | なし | - | - |
| `use` | `<profile>` | **必須** | 切り替え先のプロファイル名。既存プロファイルでなければエラー |
| `current` | なし | - | - |
| `clear` | なし | - | - |

補足: `use` は commander の `.command('use <profile>')` 記法のため、引数欠落時は commander 自身がエラーを出す（文言はライブラリ既定。TSコードには無い）。ハンドラ `handleUseProfile(profile: string)` は第1引数に文字列を直接受け取る（他ハンドラはオプションオブジェクトを受け取る）ので、Rust側の引数受け渡し設計で差異に注意。

## 3. オプションフラグ

| サブコマンド | ロング | ショート | 値の型 | 既定値 | 必須 | 相互排他 |
| --- | --- | --- | --- | --- | --- | --- |
| `set` | `--token <token>` | なし | 文字列 | なし | 任意 | `--token-stdin` と同時指定不可 |
| `set` | `--token-stdin` | なし | boolean フラグ | `false`(未指定) | 任意 | `--token` と同時指定不可 |
| `set` | `--profile <profile>` | なし | 文字列 | なし（未指定時は「現在のプロファイル」に解決。表示上のヘルプ文言は `Profile name (default: "default")`） | 任意 | なし |
| `get` | `--profile <profile>` | なし | 文字列 | なし（未指定時は現在のプロファイル） | 任意 | なし |
| `clear` | `--profile <profile>` | なし | 文字列 | なし（未指定時は現在のプロファイル） | 任意 | なし |
| `profiles` / `use` / `current` | オプションなし | - | - | - | - | - |

`--token` のヘルプ文言（そのまま移植すること）:
`Slack API token (deprecated: may leak via shell history/process list)`

`--token-stdin` のヘルプ文言: `Read Slack API token from stdin`

### `config set` のトークン解決順（`resolveTokenInput`）

上から順に評価する。

1. `--token` と `--token-stdin` の同時指定 → 即エラー（§6）
2. `--token-stdin` 指定時 → stdin を EOF まで読み、UTF-8 化して `trim()`。空ならエラー
3. `--token` 指定時 → **stderr に黄色で警告**を出したうえで `trim()` した値を採用
   警告文: `Warning: --token may leak secrets via shell history/process list. Prefer --token-stdin or interactive input.`
4. 環境変数 `SLACK_CLI_TOKEN`（`trim()` 後が非空） → その値を採用
5. 上記いずれも無い → 対話プロンプト（TTY必須）。プロンプト文字列は `Slack API token: `、入力はエコーを止めてマスクする（`readline` の出力ストリームを差し替え、質問の書き込み直後に `isMuted = true` にする実装）。回答は `trim()`。回答後に `\n` を stdout へ書く。空文字ならエラー
   - `process.stdin.isTTY` または `process.stdout.isTTY` が偽ならプロンプトせずエラー
   - プロンプト中の SIGINT は `Token input cancelled` エラー

解決したトークンは `ProfileConfigManager.setToken(token, options.profile)` に渡される（`options.profile` は未加工＝未指定なら `undefined` のまま渡す点に注意。表示用のプロファイル名だけ別途 `getProfileName()` で解決している）。

## 4. Slack Web API 呼び出し

**`config` 配下のサブコマンドは Slack Web API を一切呼ばない。** `config.ts` / `config-subcommands.ts` のいずれにも Slack クライアント（`slack-client-service` 等）の import は無く、トークンの有効性検証も行わない。副作用はすべてローカルファイル操作。

代わりに触れる外部リソースは以下。

| 対象 | パス / URL | 用途 |
| --- | --- | --- |
| 設定ファイル | `~/.slack-cli/config.json`（`ConfigOptions.configDir` で差し替え可能。CLI からは差し替えていない） | プロファイルとトークンの永続化 |
| マスターキー | `~/.slack-cli-secrets/master.key` | トークン暗号化鍵（hex 64桁 + 改行） |
| 旧マスターキー | `~/.slack-cli/master.key` | 旧配置からの移行元 |
| npm registry | `https://registry.npmjs.org/<packageName>/latest` | 更新通知（§7） |
| 更新通知キャッシュ | `~/.slack-cli/update-notifier.json` | 同上 |

### 設定ファイルの形式（`src/types/config.ts`）

```json
{
  "profiles": {
    "default": { "token": "v2:<ivHex>:<cipherHex>:<authTagHex>", "updatedAt": "2026-01-01T00:00:00.000Z" }
  },
  "defaultProfile": "default"
}
```

- ディレクトリ権限 `0o700`、ファイル権限 `0o600`。
- 保存は「テンポラリファイル（`config.json.<pid>.<epochMillis>.tmp`、`flag: 'wx'`）へ書いてから `rename`」のアトミック書き込み。rename 失敗時は temp を削除して再 throw。
- 旧形式（トップレベルに `token` があり `profiles` が無い）を読んだ場合、`default` プロファイルへ自動移行して即保存する。
- 読み込み時にファイルが無ければ（ENOENT）空ストア `{ profiles: {} }` を返す。JSON パース失敗（SyntaxError）は `Invalid config file format`。

### トークン暗号化（`TokenCryptoService`）

| 項目 | 現行(v2) | レガシー(v1) |
| --- | --- | --- |
| アルゴリズム | AES-256-GCM | AES-256-CBC |
| 保存形式 | `v2:<iv 12byte hex>:<cipher hex>:<authTag 16byte hex>` | `<iv 16byte hex>:<cipher hex>` |
| 鍵 | マスターキー（下記） | `pbkdf2Sync('slack-cli-key', 'slack-cli-salt-v1', 100000, 32, 'sha256')` の固定鍵 |
| 用途 | 暗号化・復号 | **復号のみ**（読み出し時に v2 へ再暗号化して保存し直す） |

マスターキーの決定順:
1. コンストラクタ引数 `masterKey`（CLI 経路では未使用）→ `pbkdf2Sync(secret, 'slack-cli-master-key-salt-v2', 100000, 32, 'sha256')`
2. 環境変数 `SLACK_CLI_MASTER_KEY`（trim 後非空）→ 同じ派生
3. `~/.slack-cli-secrets/master.key` を読む（hex 64桁の正規表現検証あり。**この場合は派生せず生バイト列をそのまま鍵にする**）
4. 無ければ `~/.slack-cli/master.key`（旧配置）から移行（新パスへ `wx` で書き出し）
5. それも無ければ 32 バイト乱数を生成して新パスへ作成（`EEXIST` なら読み直し）

暗号化されていない平文トークンは後方互換のためそのまま読める。`getConfig()` は「現行形式でない」トークンを読んだ時点で v2 に再暗号化してディスクへ書き戻す（**読み取り系コマンドが書き込みを起こす**）。

## 5. 標準出力の形式

`config` 系にフォーマット指定オプション（`--format` 等）は **存在しない**。出力は常に人間向けのテキスト1形式のみ。色は `chalk`（非 TTY では自動的に無色になる）。

### `config set`（成功時 / stdout）

```
✓ Token saved successfully for profile "default"
```
- 全体が緑。`✓ ` は緑テキストの一部。
- プロファイル名は `--profile` 指定値、未指定なら `getCurrentProfile()`（= `defaultProfile` または `default`）。
- `--token` 使用時は、これに先立って stderr に黄色の警告1行（§3）。

### `config get`

設定がある場合（stdout, 3行）:
```
Configuration for profile "default":
  Token: xoxb-****-****-1a2b
  Updated: 2026-01-01T00:00:00.000Z
```
- 1行目は bold。`Token:` の値は cyan、`Updated:` の値は gray。
- マスク仕様（`maskToken`）: トークン長が **9以下**なら `****` を返す。それ以外は `先頭4文字 + "-****-****-" + 末尾4文字`。
- `Updated` は保存時の `new Date().toISOString()` をそのまま出す（例: `2026-01-01T00:00:00.000Z`）。

設定が無い場合（stdout・黄色・**終了コード 0**）:
```
No configuration found for profile "work". Use "slack-cli config set --token <token> --profile work" to set up.
```

### `config profiles`

プロファイルがある場合（stdout）:
```
Available profiles:
  * default (xoxb-****-****-1a2b)
    work (xoxb-****-****-9z8y)
```
- 1行目は bold。各行は `  ` + マーカー(`*` か半角スペース) + ` ` + プロファイル名(cyan) + ` (` + マスク済みトークン + `)`。
- マーカーの `*` は `getCurrentProfile()` と一致する行に付く。
- **重要**: ここで使うトークンは `listProfiles()` が返す**生の保存値**（＝暗号化済み文字列）をそのままマスクしたもの。`maskToken` は復号しないため、実際には `v2-****-****-<末尾4>` のような表示になる。`config get` の表示（復号後をマスク）とは形が違う。移植時にこの差異を保つか直すかを決めること。

プロファイルが0件の場合（stdout・黄色・終了コード 0）:
```
No profiles found. Use "slack-cli config set --token <token>" to create one.
```

### `config use <profile>`（成功時 / stdout・緑）

```
✓ Switched to profile "work"
```

### `config current`（stdout）

```
Current profile: default
```
- 行全体 bold、プロファイル名部分がさらに cyan。設定ファイルが無くても `default` を出す（エラーにならない）。

### `config clear`（成功時 / stdout・緑）

```
✓ Profile "work" cleared successfully
```
- 存在しないプロファイル名を指定しても **エラーにならず**この成功メッセージが出る（`delete store.profiles[name]` が no-op のため）。
- 削除対象が現在の既定プロファイルだった場合、残りプロファイルの**最初のキー**（`Object.keys` 順＝JSON の挿入順）が新しい `defaultProfile` になる。残り0件なら `config.json` 自体を `unlink` する（ENOENT は無視）。

## 6. エラーケース・メッセージ・終了コード

すべてのハンドラは `wrapCommand()` に包まれ、例外は次のように処理される。

1. `extractErrorMessage(error)` でメッセージ化
   - `Error` かつ Slack エラーコードが `missing_scope` かつ `data.needed` がある場合のみ `"<message> (needed: a, b)"` 形式（`config` では実質使われない）
   - `Error` 以外は `String(error)`
2. `sanitizeTerminalText()` で ANSI/OSC エスケープと制御文字（`\t` `\n` 以外の C0 / DEL / C1）を除去
3. `redactSlackTokens()` で `xox[bpoars]-...` にマッチする部分を `xoxb-***-REDACTED` 形式（先頭4文字を小文字化して使う）に置換
4. stderr へ `✗ Error:`（赤）+ 半角スペース + メッセージ を出力
5. `NODE_ENV === 'development'` かつ `Error` インスタンスなら、スタックトレースも同じサニタイズ＋伏字処理をして gray で stderr に追加出力
6. `process.exit(1)`

| # | 発生条件 | メッセージ本文 | 出力先 | 終了コード |
| --- | --- | --- | --- | --- |
| 1 | `config set --token X --token-stdin` | `Cannot use --token and --token-stdin together` | stderr | 1 |
| 2 | `--token-stdin` で読んだ内容が trim 後に空 | `No token received from stdin` | stderr | 1 |
| 3 | トークン未指定・環境変数なし・非TTY | `No token provided. Use --token-stdin, set SLACK_CLI_TOKEN, or run this command in an interactive terminal.` | stderr | 1 |
| 4 | 対話プロンプト中の SIGINT | `Token input cancelled` | stderr | 1 |
| 5 | 対話プロンプトで空入力 | `Token cannot be empty` | stderr | 1 |
| 6 | `config use` で存在しないプロファイル | `Profile "xxx" does not exist`（`ConfigurationError`） | stderr | 1 |
| 7 | `config.json` が不正 JSON（全サブコマンド共通） | `Invalid config file format` | stderr | 1 |
| 8 | マスターキーファイルが hex64 でない | `Invalid token encryption key format` | stderr | 1 |
| 9 | マスターキー読込失敗（ENOENT 以外） | `Failed to load token encryption key` | stderr | 1 |
| 10 | 旧鍵ファイルの移行失敗 | `Failed to migrate token encryption key` | stderr | 1 |
| 11 | 新規鍵ファイル作成失敗（EEXIST 以外） | `Failed to initialize token encryption key` | stderr | 1 |
| 12 | 暗号化失敗 | `Failed to encrypt token` | stderr | 1 |
| 13 | 復号失敗（形式は正しいが復号できない等） | `Failed to decrypt token` | stderr | 1 |
| 14 | 復号対象が空文字 / どの形式にも合致しない | `Invalid encrypted data format`（`ValidationError`） | stderr | 1 |

非エラー扱い（**終了コード 0** のまま黄色メッセージを stdout に出すだけ）:

- `config get` で設定が無い → `No configuration found for profile "<name>". ...`
- `config profiles` で0件 → `No profiles found. ...`

未定義のサブコマンドや `use` の引数欠落は commander が処理するため、文言・終了コードは TS コード上に定義が無く **不明**（commander 既定に依存）。

## 7. ページネーション・レート制限・並行実行

- **ページネーション: 該当なし。** ネットワーク越しの一覧取得が無い。
- **レート制限: 該当なし。** `constants.ts` に `RATE_LIMIT`（`CONCURRENT_REQUESTS: 3` など）はあるが、`config` 系からは参照されない。
- **並行実行**: ハンドラ内に並列処理は無い（すべて逐次 `await`）。ただし設定ファイル書き込みは「一意な temp 名 + `wx` + `rename`」で、同時実行時も部分書き込みは起きない設計。とはいえ read-modify-write のロックは無いため、同時に別プロファイルを `set` すると後勝ちで片方が失われる（TOCTOU）。
- **更新通知（`postAction` フック）**: 全コマンド実行後に `checkForUpdates()` が動く。
  - スキップ条件: 環境変数 `CI` が定義済み / `SLACK_CLI_DISABLE_UPDATE_NOTIFIER === '1'` / `process.stderr.isTTY` が偽
  - キャッシュ TTL 24時間、HTTP タイムアウト 2000ms（`AbortController`）
  - 新しいバージョンがあれば stderr に黄色で2行:
    `Update available: 0.24.1 -> 0.25.0` / `Run: npm install -g @mimo-3/slack-cli`
  - 例外は握り潰す（CLI 本体の挙動に影響させない）

## 8. Rust 移植で引っかかりそうな点

1. **`--profile` の二重解決**。`handleSetToken` は表示用に `getProfileName()`（未指定なら `getCurrentProfile()`）を使う一方、`setToken()` には `options.profile` を**未解決のまま**渡す。`setToken` 内部は `profile || store.defaultProfile || 'default'` で再解決するので通常は一致するが、両者を1つの解決関数にまとめると挙動が変わり得る。`get` / `clear` も同じ構造。
2. **`config profiles` の表示は暗号文をマスクしている**（§5）。`listProfiles()` が復号を通らないため。忠実移植か修正かを明示的に決めること。
3. **`config get` は読むだけなのに書き込む**。旧形式・平文トークンを検出すると v2 で再暗号化して保存する。読み取り専用パスを前提にした設計にすると再現できない。
4. **鍵の扱いが経路で非対称**。ファイル由来の鍵は生の32バイトをそのまま使い、環境変数/注入由来は PBKDF2（100000回, salt `slack-cli-master-key-salt-v2`）で派生する。Rust では `aes-gcm` + `pbkdf2` + `hmac-sha256` の組み合わせで再現でき、既存の `config.json` を読める互換性が要る。
5. **レガシー AES-256-CBC 復号の維持**。`aes-256-cbc` は Rust の主要 AEAD crate に無いので、`cbc` + `aes` + PKCS#7 パディングを自前で組む必要がある。Node の `createDecipheriv` は既定で PKCS#7 パディングを剥がす点に注意。
6. **形式判定の正規表現をそのまま守る**。v2 判定は「`:` 区切りで4要素／先頭が `v2`／iv は hex 24文字／cipher は空文字可・偶数長 hex／authTag は hex 32文字」。レガシー判定は「2要素／iv hex 32文字／cipher 非空・偶数長 hex」。緩めると誤判定して復号エラーになる。
7. **`wx` フラグ + `rename` のアトミック書き込み**。Rust では `OpenOptions::new().create_new(true)`、パーミッションは `std::os::unix::fs::OpenOptionsExt::mode(0o600)`、ディレクトリは `DirBuilderExt::mode(0o700)`。Windows ではパーミッションの概念が違うので分岐が要る（TS 版は無条件に mode を渡している）。
8. **マスク済みトークンの端数**。`maskToken` の閾値は「長さ **9以下** なら `****`」（`TOKEN_MIN_LENGTH = 9`、比較は `<=`）。境界を1ずらすとテストが落ちる。また `substring` は UTF-16 コードユニット単位なので、非 ASCII を含む値では Rust の `chars()` ベース実装と結果がずれ得る。
9. **対話プロンプトのマスク方式が独特**。「question を書き込んだ**直後**に `isMuted = true` にする」ため、プロンプト文字列は表示され入力文字は一切表示されない（`*` すら出ない）。Rust では `rpassword` 等でほぼ同等になるが、SIGINT 時に `Token input cancelled` を返す挙動は自前で用意する必要がある。
10. **stdin の読み方**。`--token-stdin` は EOF まで全部読んで trim する（1行読みではない）。複数行を渡すと途中の改行も含めた文字列が trim されるだけで、中間の改行は残る。
11. **`chalk` の色付け位置**。`✓` を含む成功メッセージは行全体が緑、`config get` の1行目は bold、値だけ cyan/gray、といった具合に着色範囲がバラバラ。`owo-colors` 等で移す際は範囲を1つずつ合わせること。非 TTY・`NO_COLOR` 時の自動無効化も chalk 相当の判定が要る。
12. **エラー時のサニタイズ順序**。「ANSI 除去 → トークン伏字」の順である必要がある（コメントにも明記）。逆にするとエスケープで分断されたトークンが伏字を逃れる。
13. **`process.exit(1)` の即時終了**。Rust では `std::process::exit` と同じくデストラクタが走らないので、書き込み途中のバッファに注意。stdout がパイプされている場合の flush 忘れも起きやすい。
14. **`config clear` は存在しないプロファイルでも成功する**。エラーにしたくなる箇所だが、忠実移植なら成功メッセージを出す。
15. **`defaultProfile` の再選出が JSON のキー順依存**。`Object.keys()` は挿入順を返すので、Rust では `serde_json` の `preserve_order` フィーチャ（`IndexMap`）が要る。`HashMap` にすると選ばれるプロファイルが非決定になる。
16. **`postAction` フックの移植先**。commander のフック相当が clap には無いので、コマンド実行後に必ず走る後処理としてメイン関数側に明示的に置く必要がある。
17. **`__dirname/../package.json` からのバージョン取得**は Rust では `env!("CARGO_PKG_VERSION")` に置き換わる。更新通知が参照する `packageName`（`@mimo-3/slack-cli`）は npm registry 前提なので、配布形態が変わるならこの機能の扱いを決める必要がある。
