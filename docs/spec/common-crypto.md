# common-crypto — トークン暗号化の完全仕様（TS 版との 100% 互換）

対象: `slack-cli`（TypeScript）のトークン暗号化を Rust で完全再現するための仕様書。
**絶対条件**: 既存ユーザーの `~/.slack-cli/config.json` と `~/.slack-cli-secrets/master.key` を、そのまま読み書きできること。

## 0. 参照した実装

読み取り専用で参照（編集していない）:

- `/Users/mimo/organizations/open-source/slack-cli/src/utils/token-crypto-service.ts`
- `/Users/mimo/organizations/open-source/slack-cli/src/utils/profile-config.ts`
- `/Users/mimo/organizations/open-source/slack-cli/src/utils/config-helper.ts`
- `/Users/mimo/organizations/open-source/slack-cli/src/utils/token-utils.ts`
- `/Users/mimo/organizations/open-source/slack-cli/src/utils/constants.ts`
- `/Users/mimo/organizations/open-source/slack-cli/src/types/config.ts`
- `/Users/mimo/organizations/open-source/slack-cli/tests/utils/token-crypto-service.test.ts`
- 既存仕様書: `docs/spec/common-client.md` §4、`docs/spec/cmd-config.md`
- Rust 側の既存実装の参考: `/Users/mimo/organizations/open-source/notion-cli/src/config/mod.rs`

本書に書いた数値・文字列はすべて上記ソースの実値である。推測を含む箇所は「推測」と明記する。

## 1. 定数一覧（`TokenCryptoService` のフィールド実値）

| 名前 | 値 |
| --- | --- |
| `algorithm` | `"aes-256-gcm"` |
| `legacyAlgorithm` | `"aes-256-cbc"` |
| `keyLength` | `32`（バイト） |
| `ivLength` | `12`（バイト、GCM） |
| `legacyIvLength` | `16`（バイト、CBC） |
| `authTagLength` | `16`（バイト） |
| `separator` | `":"`（U+003A、1 文字） |
| `version` | `"v2"` |
| `masterKeySalt` | `"slack-cli-master-key-salt-v2"` |
| `masterKeyIterations` | `100000` |
| レガシー鍵パスフレーズ | `"slack-cli-key"` |
| レガシー鍵 salt | `"slack-cli-salt-v1"` |
| レガシー鍵 反復回数 | `100000` |

salt・パスフレーズはいずれも **ASCII 文字列をそのまま UTF-8 バイト列として** PBKDF2 に渡す（Node の `crypto.pbkdf2Sync` に string を渡した場合の既定は UTF-8 解釈）。

## 2. v2 形式（AES-256-GCM）

### 2.1 保存文字列のフォーマット

```
v2:<iv_hex>:<ciphertext_hex>:<authtag_hex>
```

生成コードは `[version, iv.toString('hex'), encrypted, authTag.toString('hex')].join(':')`。

- 区切り文字は `:` ちょうど 1 文字。フィールドは常に 4 個。
- `iv_hex`: 12 バイト → **24 文字**の小文字 hex（Node の `Buffer.toString('hex')` は小文字を出力）。
- `ciphertext_hex`: 暗号文の hex。**長さ 0 も正当**（空トークンを暗号化すると空になる）。GCM はストリーム暗号なのでパディングなし、長さ = 平文バイト数。
- `authtag_hex`: 16 バイト → **32 文字**の hex。認証タグは**末尾の独立フィールド**であり、暗号文に連結されていない（Rust の `aes-gcm` の `encrypt()` は tag を末尾に付けた結果を返すので、そのままだと非互換。分離が必須）。
- 全体に prefix/suffix・改行・base64 は一切ない。

暗号化の詳細:

- 鍵: §4 で得た 32 バイトのマスターキー。
- IV: `crypto.randomBytes(12)` の乱数（毎回異なる。同じトークンでも出力が変わる）。
- **AAD（追加認証データ）は使わない**（空）。
- 平文は UTF-8 バイト列（Node の `cipher.update(token, 'utf8', 'hex')`）。
- タグ長は Node の既定 16 バイト。

### 2.2 形式判定（`isCurrentFormat`）— 復号前に必ず通す

```
value を ':' で split
parts.length === 4 かつ parts[0] === "v2"
parts[1] が /^[0-9a-fA-F]+$/ かつ 長さ === 24
parts[2] が空文字 or /^[0-9a-fA-F]+$/、かつ 長さが偶数
parts[3] が /^[0-9a-fA-F]+$/ かつ 長さ === 32
```

- 空文字列 `""` は falsy なので即 `false`。
- hex は**大文字も受理**する（判定は `[0-9a-fA-F]`）。ただし書き出しは常に小文字。
- `parts[2]` が空文字のケースを明示的に許すため、空トークンの暗号文も「現行形式」と判定される。

### 2.3 復号手順

1. `isCurrentFormat` を再検査（偽なら `ValidationError('Invalid encrypted data format')`）。
2. `iv = hex_decode(parts[1])`、`ct = hex_decode(parts[2])`、`tag = hex_decode(parts[3])`。
3. マスターキーを取得（§4）。
4. AES-256-GCM で `iv` / `tag` を用いて復号。AAD なし。
5. 復号結果を UTF-8 文字列として返す。

タグ検証失敗・鍵不一致は区別せず `ConfigurationError('Failed to decrypt token')` になる（TS 側は `decrypt` 全体を try/catch し、`ValidationError` 以外を握り潰して変換している）。

## 3. レガシー形式（AES-256-CBC）

### 3.1 保存文字列のフォーマット

```
<iv_hex>:<ciphertext_hex>
```

- バージョン prefix **なし**、フィールドは 2 個。
- `iv_hex`: 16 バイト → **32 文字**の hex。
- `ciphertext_hex`: **長さ 0 は不可**（`cipherHex.length > 0` が判定条件）。長さは偶数かつ、CBC + PKCS#7 なので実体は 16 バイトの倍数。
- 認証タグは存在しない（CBC は非認証）。

### 3.2 形式判定（`isLegacyEncrypted`）

```
value を ':' で split
parts.length === 2
parts[0] が /^[0-9a-fA-F]+$/ かつ 長さ === 32
parts[1] が /^[0-9a-fA-F]+$/ かつ 長さ > 0 かつ 長さが偶数
```

### 3.3 鍵（マスターキーとは無関係の固定鍵）

```
legacy_key = PBKDF2-HMAC-SHA256(
  password   = "slack-cli-key",
  salt       = "slack-cli-salt-v1",
  iterations = 100000,
  dkLen      = 32
)
```

**この鍵は環境変数・鍵ファイルの影響を一切受けない。** ソース中にパスフレーズが直書きされているため、レガシー形式の暗号文は誰でも復号できる（機密性はない）。Rust 側でも同じ固定値を使う必要がある。

### 3.4 復号手順

1. `isLegacyEncrypted` を再検査（偽なら `ValidationError`）。
2. `iv = hex_decode(parts[0])`（16 バイト）、`ct = hex_decode(parts[1])`。
3. `legacy_key` を導出。
4. AES-256-CBC で復号、**PKCS#7 パディングを除去**（Node の `createDecipheriv` の既定挙動。`setAutoPadding(false)` は呼んでいない）。
5. UTF-8 文字列として返す。

### 3.5 レガシーからの自動再暗号化

`ProfileConfigManager.getConfig()` は、読み出したトークンが `isCurrentFormat` でない場合（= 平文 or レガシー）、復号結果を v2 形式で暗号化し直して `config.json` に**書き戻す**。読み取り操作がディスク書き込みを伴う点は Rust でも再現が必要（`common-client.md` §3 に既述）。

## 4. マスターキーの解決 — 「ファイル」と「環境変数」の非対称

`getMasterKey()` の優先順位（上から順に、最初に成立したものを採用。プロセス内で 1 度だけ導出しキャッシュ）:

| 順 | 由来 | 32 バイト鍵の作り方 |
| --- | --- | --- |
| 1 | コンストラクタ注入 `options.masterKey` | **PBKDF2**（下記 §4.1） |
| 2 | 環境変数 `SLACK_CLI_MASTER_KEY`（`.trim()` して非空なら採用） | **PBKDF2**（下記 §4.1） |
| 3 | 現行鍵ファイル `~/.slack-cli-secrets/master.key` | **hex をデコードした生 32 バイトをそのまま鍵に使う。PBKDF2 は通さない** |
| 4 | 3 が `ENOENT` のとき: 旧鍵ファイル `~/.slack-cli/master.key` を読んで現行パスへ移行 | 同上（生バイト） |
| 5 | 4 も `ENOENT` のとき: 新規鍵ファイルを作成 | `randomBytes(32)` の hex を書き、その生バイトを鍵に使う |

CLI 本体は `new TokenCryptoService()`（引数なし）でしか生成しないため、実運用で効くのは 2〜5。

### 4.1 パスフレーズ由来（注入 / 環境変数）の導出

```
key = PBKDF2-HMAC-SHA256(
  password   = secret,                            // trim 済みの文字列を UTF-8 バイト列に
  salt       = "slack-cli-master-key-salt-v2",    // ASCII 28 バイト
  iterations = 100000,
  dkLen      = 32
)
```

環境変数の場合のみ `.trim()` が入る（前後の空白・改行を除去）。注入値は trim しない。

### 4.2 鍵ファイル由来

- 読み込み: ファイル全体を UTF-8 で読み、`trim()`。
- 検証: `/^[0-9a-f]{64}$/i`（大文字小文字を問わない hex 64 文字ちょうど）。不一致は `ConfigurationError('Invalid token encryption key format')`。
- 鍵: `hex_decode(trimmed)` の 32 バイトを**そのまま** AES 鍵にする。

### 4.3 鍵ファイルの書き出し

- ディレクトリ: `mkdir -p`、mode `0o700`。
- ファイル本文: `"<hex64>\n"`（**末尾に改行 1 個**）。
- ファイル mode: `0o600`、フラグ `wx`（= `O_CREAT | O_EXCL`。既存なら `EEXIST`）。
- 新規作成時: `randomBytes(32).toString('hex')`（小文字 64 文字）。
- `EEXIST` の場合は既存ファイルを読み直して採用（並行実行時の競合対策）。それ以外の失敗は `ConfigurationError('Failed to initialize token encryption key')`。

### 4.4 旧鍵ファイルからの移行（`migrateLegacyKeyFile`）

1. `~/.slack-cli/master.key` を読む（読めなければその ENOENT が上位に伝播 → 新規作成へ）。
2. 同じ hex を `~/.slack-cli-secrets/master.key` へ `wx` で書く。
3. 書き込みが失敗した場合:
   - エラーオブジェクトに `code` が無い → 旧鍵をそのまま返す。
   - `code !== 'EEXIST'`、または `EEXIST` だが現行パスが存在しない → 旧鍵をそのまま返す。
   - `EEXIST` かつ現行パスが存在する → 現行パスを読み直して返す。
4. 成功時は現行パスを読み直して返す（書いた内容と同一）。
5. **旧ファイルは削除しない。**

### 4.5 非対称性の帰結（最重要）

同じ 64 文字 hex を「ファイルに置く」場合と「`SLACK_CLI_MASTER_KEY` に入れる」場合で、**得られる AES 鍵は別物**になる。ファイルは生バイト、環境変数は PBKDF2 通し。ここを取り違えると既存トークンが一切復号できない。Rust 側でも必ずこの分岐を保つこと。

## 5. 設定ファイル

### 5.1 パスとパーミッション

| 対象 | パス | mode |
| --- | --- | --- |
| 設定 | `~/.slack-cli/config.json` | ファイル `0o600` / ディレクトリ `0o700` |
| 現行マスターキー | `~/.slack-cli-secrets/master.key` | ファイル `0o600` / ディレクトリ `0o700` |
| 旧マスターキー | `~/.slack-cli/master.key` | （移行元。読むだけ） |

- ホームは Node の `os.homedir()`。`configDir` はコンストラクタで差し替え可能だが CLI からは差し替えていない。
- XDG ディレクトリは**使っていない**。notion-cli の Rust 実装（`dirs::config_dir()`）とは方針が違うので、そちらをコピーしてはいけない。

### 5.2 保存方法（アトミック書き込み）

1. `mkdir -p <configDir>`（mode `0o700`）。
2. テンポラリファイル `config.json.<pid>.<epochMillis>.tmp` に、mode `0o600`・フラグ `wx` で書く。
3. `rename(temp, config.json)`。失敗したら temp を削除して再 throw。

### 5.3 JSON 構造

現行フォーマット（`ConfigStore`）:

```json
{
  "profiles": {
    "default": {
      "token": "v2:<iv_hex>:<ct_hex>:<tag_hex>",
      "updatedAt": "2026-01-01T00:00:00.000Z"
    }
  },
  "defaultProfile": "default"
}
```

- インデントは `JSON.stringify(store, null, 2)` の **2 スペース**。末尾改行なし。
- `updatedAt` は `new Date().toISOString()`（UTC・ミリ秒 3 桁・`Z` 終端）。
- `defaultProfile` は省略可（`ConfigStore.defaultProfile?`）。無ければ `"default"` として扱う。

旧フォーマット（migration 対象）: トップレベルに `token` があり `profiles` が無いもの。

```json
{ "token": "...", "updatedAt": "..." }
```

判定は `Boolean(data.token && !data.profiles)`。検出したら、トークンを復号（`isEncrypted` なら復号、でなければ平文扱い）→ v2 で再暗号化 → `{ profiles: { default: {...} }, defaultProfile: "default" }` に置き換えて保存する。

### 5.4 キー順の意味

JSON のキー順は仕様上の意味を持たない（読み取りは順序非依存）が、**TS 版が書くバイト列と一致させたい**なら以下を守る:

- トップレベル: `profiles` → `defaultProfile`（空ストア `{ profiles: {} }` から生え、`migrateOldConfig` も `{ profiles, defaultProfile }` の順で構築するため）。
- プロファイル値: `token` → `updatedAt`（`Config` の生成順。再暗号化時も `{...config, token}` なので既存キー順が保たれる）。
- `profiles` 内のプロファイル名は**挿入順**（JS の文字列キーは挿入順を保つ）。

Rust では `HashMap` を使うとプロファイル順が壊れるため、既存ファイルの並びを保ちたければ `serde_json` の `preserve_order` feature（内部で `IndexMap`）を使う。構造体フィールドはソース上の宣言順に出力されるので、`token` → `updatedAt`、`profiles` → `default_profile` の順に宣言する。

### 5.5 トークンの読み出し優先順位

`ProfileConfigManager.getConfig()` はプロファイルの `token` フィールドしか見ない。値が

- `isCurrentFormat` → v2 で復号
- `isLegacyEncrypted` → CBC で復号 + v2 で再暗号化して書き戻し
- どちらでもない → **平文としてそのまま返す** + v2 で暗号化して書き戻し

## 6. Rust の実装（クレート選定とコード断片）

### 6.1 クレート

| 用途 | クレート | 備考 |
| --- | --- | --- |
| AES-256-GCM | `aes-gcm = "0.10"` | `Aes256Gcm`。tag 分離のため `AeadInPlace` を使う |
| AES-256-CBC | `aes = "0.8"` + `cbc = "0.4"` | `cbc::Decryptor<aes::Aes256>`、`block-padding` の `Pkcs7` |
| PBKDF2 | `pbkdf2 = "0.12"`（default-features = false） + `hmac = "0.12"` + `sha2 = "0.10"` | `pbkdf2_hmac::<Sha256>` |
| hex | `hex = "0.4"` | `encode` は小文字出力（Node と一致） |
| 乱数 | `rand = "0.8"`（`aes-gcm` の `OsRng` 経由でも可） | IV と鍵の生成 |
| 秘密の消去 | `zeroize = "1"` | 鍵バッファの Drop 時消去（TS 版には無い改善。互換性には影響しない） |
| JSON | `serde` + `serde_json`（feature `preserve_order`） | §5.4 |
| ホームディレクトリ | `dirs = "5"` の `home_dir()` | `config_dir()` ではない |

推測: クレートのバージョンは 2026-08 時点で広く使われている系列を挙げたもので、実際の採用版は `Cargo.toml` 作成時に確定させる。API 名は上記メジャー系列のもの。

### 6.2 定数

```rust
const VERSION: &str = "v2";
const SEPARATOR: char = ':';
const KEY_LEN: usize = 32;
const IV_LEN: usize = 12;
const LEGACY_IV_LEN: usize = 16;
const TAG_LEN: usize = 16;
const MASTER_KEY_SALT: &[u8] = b"slack-cli-master-key-salt-v2";
const MASTER_KEY_ITERATIONS: u32 = 100_000;
const LEGACY_PASSPHRASE: &[u8] = b"slack-cli-key";
const LEGACY_SALT: &[u8] = b"slack-cli-salt-v1";
const LEGACY_ITERATIONS: u32 = 100_000;
```

### 6.3 鍵導出

```rust
use hmac::Hmac;
use sha2::Sha256;

fn derive_master_key(secret: &str) -> [u8; KEY_LEN] {
    let mut out = [0u8; KEY_LEN];
    pbkdf2::pbkdf2::<Hmac<Sha256>>(
        secret.as_bytes(),
        MASTER_KEY_SALT,
        MASTER_KEY_ITERATIONS,
        &mut out,
    )
    .expect("pbkdf2 output length is valid");
    out
}

fn derive_legacy_key() -> [u8; KEY_LEN] {
    let mut out = [0u8; KEY_LEN];
    pbkdf2::pbkdf2::<Hmac<Sha256>>(
        LEGACY_PASSPHRASE,
        LEGACY_SALT,
        LEGACY_ITERATIONS,
        &mut out,
    )
    .expect("pbkdf2 output length is valid");
    out
}
```

鍵ファイル由来は PBKDF2 を通さない:

```rust
fn parse_file_key(contents: &str) -> Result<[u8; KEY_LEN], CryptoError> {
    let hex_str = contents.trim();
    let is_hex64 = hex_str.len() == 64 && hex_str.bytes().all(|b| b.is_ascii_hexdigit());
    if !is_hex64 {
        return Err(CryptoError::config("Invalid token encryption key format"));
    }
    let bytes = hex::decode(hex_str).map_err(|_| {
        CryptoError::config("Invalid token encryption key format")
    })?;
    let mut key = [0u8; KEY_LEN];
    key.copy_from_slice(&bytes);
    Ok(key)
}
```

### 6.4 v2 暗号化

```rust
use aes_gcm::{aead::AeadInPlace, Aes256Gcm, KeyInit, Nonce};

fn encrypt_v2(key: &[u8; KEY_LEN], plaintext: &str) -> Result<String, CryptoError> {
    let cipher = Aes256Gcm::new(key.into());
    let mut iv = [0u8; IV_LEN];
    rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut iv);
    let nonce = Nonce::from_slice(&iv);

    let mut buf = plaintext.as_bytes().to_vec();
    // AAD は空。tag を戻り値で受け取り、ciphertext とは別フィールドにする
    let tag = cipher
        .encrypt_in_place_detached(nonce, b"", &mut buf)
        .map_err(|_| CryptoError::config("Failed to encrypt token"))?;

    Ok(format!(
        "{}:{}:{}:{}",
        VERSION,
        hex::encode(iv),
        hex::encode(&buf),
        hex::encode(tag)
    ))
}
```

`Aes256Gcm::encrypt()`（非 detached）は tag を暗号文末尾に連結した bytes を返すため、そのまま hex 化すると TS 版と非互換になる。**必ず detached 版を使う**か、末尾 16 バイトを切り出して分ける。

### 6.5 v2 復号

```rust
fn decrypt_v2(key: &[u8; KEY_LEN], value: &str) -> Result<String, CryptoError> {
    let parts: Vec<&str> = value.split(SEPARATOR).collect();
    if !is_current_format(&parts) {
        return Err(CryptoError::validation("Invalid encrypted data format"));
    }
    let iv = hex::decode(parts[1]).unwrap();
    let mut buf = hex::decode(parts[2]).unwrap();
    let tag = hex::decode(parts[3]).unwrap();

    let cipher = Aes256Gcm::new(key.into());
    cipher
        .decrypt_in_place_detached(
            Nonce::from_slice(&iv),
            b"",
            &mut buf,
            aes_gcm::Tag::from_slice(&tag),
        )
        .map_err(|_| CryptoError::config("Failed to decrypt token"))?;

    String::from_utf8(buf).map_err(|_| CryptoError::config("Failed to decrypt token"))
}

fn is_current_format(parts: &[&str]) -> bool {
    parts.len() == 4
        && parts[0] == VERSION
        && parts[1].len() == IV_LEN * 2
        && parts[1].bytes().all(|b| b.is_ascii_hexdigit())
        && parts[2].len() % 2 == 0
        && parts[2].bytes().all(|b| b.is_ascii_hexdigit())
        && parts[3].len() == TAG_LEN * 2
        && parts[3].bytes().all(|b| b.is_ascii_hexdigit())
}
```

`is_current_format` は空文字列（split 結果が 1 要素）で自然に false になる。空 `parts[2]` は `all()` が真・長さ 0 が偶数なので通る（TS と一致）。

### 6.6 レガシー CBC 復号

```rust
use aes::cipher::{block_padding::Pkcs7, BlockDecryptMut, KeyIvInit};
type Aes256CbcDec = cbc::Decryptor<aes::Aes256>;

fn decrypt_legacy(value: &str) -> Result<String, CryptoError> {
    let parts: Vec<&str> = value.split(SEPARATOR).collect();
    if !is_legacy_format(&parts) {
        return Err(CryptoError::validation("Invalid encrypted data format"));
    }
    let iv = hex::decode(parts[0]).unwrap();
    let ct = hex::decode(parts[1]).unwrap();
    let key = derive_legacy_key();

    let plain = Aes256CbcDec::new(key[..].into(), iv[..].into())
        .decrypt_padded_vec_mut::<Pkcs7>(&ct)
        .map_err(|_| CryptoError::config("Failed to decrypt token"))?;

    String::from_utf8(plain).map_err(|_| CryptoError::config("Failed to decrypt token"))
}

fn is_legacy_format(parts: &[&str]) -> bool {
    parts.len() == 2
        && parts[0].len() == LEGACY_IV_LEN * 2
        && parts[0].bytes().all(|b| b.is_ascii_hexdigit())
        && !parts[1].is_empty()
        && parts[1].len() % 2 == 0
        && parts[1].bytes().all(|b| b.is_ascii_hexdigit())
}
```

### 6.7 鍵ファイルの作成（`O_EXCL` + mode）

```rust
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

fn write_key_file(path: &Path, key_hex: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
    }
    let mut f = fs::OpenOptions::new()
        .create_new(true)   // = wx / O_CREAT|O_EXCL、既存なら AlreadyExists
        .write(true)
        .mode(0o600)
        .open(path)?;
    f.write_all(format!("{key_hex}\n").as_bytes())?;  // 末尾改行を忘れない
    f.sync_all()
}
```

`ErrorKind::AlreadyExists` が Node の `EEXIST`、`ErrorKind::NotFound` が `ENOENT` に対応する。分岐条件は §4.3 / §4.4 のとおり。

### 6.8 マスターキー解決の骨組み

```rust
fn get_master_key(&self) -> Result<[u8; KEY_LEN], CryptoError> {
    if let Some(secret) = &self.injected_master_key {
        return Ok(derive_master_key(secret));           // PBKDF2
    }
    if let Ok(v) = std::env::var("SLACK_CLI_MASTER_KEY") {
        let v = v.trim();
        if !v.is_empty() {
            return Ok(derive_master_key(v));            // PBKDF2
        }
    }
    match fs::read_to_string(self.key_file_path()) {
        Ok(s) => parse_file_key(&s),                    // 生バイト
        Err(e) if e.kind() == ErrorKind::NotFound => self.migrate_or_create(),
        Err(_) => Err(CryptoError::config("Failed to load token encryption key")),
    }
}
```

TS 版はプロセス内で 1 度導出したらキャッシュする（`cachedMasterKey`）。Rust でも `OnceCell` 等で同じにしておくと、PBKDF2 10 万回の再実行を避けられる。

## 7. 互換性の検証手順

### 7.1 固定ベクタによる単体テスト（鍵を固定して決定論にする）

トークン実値は使わない。テスト用のダミー文字列（例: `DUMMY-TOKEN-FOR-TEST`）で十分。

1. **v2 の相互復号**
   - TS 側で `SLACK_CLI_MASTER_KEY=test-passphrase` を設定し、`new TokenCryptoService().encrypt('DUMMY-TOKEN-FOR-TEST')` の出力を控える。
   - Rust 側の `decrypt` に同じ文字列を渡し、同じ環境変数で平文が一致することを確認。
   - 逆向き（Rust で暗号化 → TS で復号）も行う。IV が乱数なので出力一致は比較しない。**復号結果の一致だけを見る**。
2. **鍵ファイル経路（生バイト）**
   - 一時ディレクトリに `master.key` として既知の hex 64 文字（例: `00112233...ff` のような固定値）+ 改行を置く。
   - 同じファイルを両実装に読ませ、TS で暗号化 → Rust で復号、その逆、の両方を通す。
   - **同じ hex を `SLACK_CLI_MASTER_KEY` に入れた場合に復号が失敗すること**も確認する（§4.5 の非対称が保たれている証拠）。
3. **レガシー CBC**
   - `derive_legacy_key()` の出力 hex が TS の `pbkdf2Sync('slack-cli-key','slack-cli-salt-v1',100000,32,'sha256')` と一致することを、まずバイト単位で比較（PBKDF2 単体の固定ベクタ）。
   - TS のテスト（`tests/utils/token-crypto-service.test.ts` の "should decrypt legacy AES-256-CBC encrypted token"）と同じ手順で `<iv_hex>:<ct_hex>` を作り、Rust で復号できることを確認。
   - Rust 側にレガシー**暗号化**は実装しない（TS 版も持たない）。
4. **形式判定の境界**
   - 空文字 / `:` 無し / IV 長違い / タグ長違い → `ValidationError` 相当。
   - `parts[2]` が空（空トークン）の v2 文字列 → 正常に空文字へ復号。
   - 大文字 hex の v2 文字列 → 受理される。
   - 平文の Slack トークン（`xox…（Slack トークン形式）`）→ `isEncrypted` が false。

### 7.2 実ファイルでのエンドツーエンド確認

1. 使い捨ての HOME を作る: `export HOME=$(mktemp -d)`。
2. TS 版で `slack-cli config set --token DUMMY-TOKEN-FOR-TEST --profile default` を実行。
3. 生成物を確認: `$HOME/.slack-cli/config.json`（`0600`）と `$HOME/.slack-cli-secrets/master.key`（`0600`、hex64+改行）。
4. **同じ HOME のまま** Rust 版で `slack-cli config list` / トークンを使うコマンドを実行し、復号できることを確認。
5. Rust 版で `config set` を実行した後、TS 版で読み直せることを確認（逆方向）。
6. `config.json` の差分を目視: キー順（`profiles` → `defaultProfile`、`token` → `updatedAt`）、2 スペースインデント、`updatedAt` の ISO 形式。
7. **レガシー migration**: 手順 3 の後で `master.key` を `$HOME/.slack-cli/master.key` に移し、`$HOME/.slack-cli-secrets` を消してから Rust 版を起動 → 現行パスに同じ hex がコピーされ、旧ファイルも残っていることを確認。
8. **平文トークンの取り込み**: `config.json` の `token` を平文文字列に書き換えて Rust 版で読み、v2 形式に書き換えられて保存されることを確認。

### 7.3 CI に置くもの

- §7.1 の 1〜4 を Rust の `#[test]` として実装（PBKDF2 固定ベクタは hex 直書きで決定論）。
- TS 版が生成した v2 文字列と、その復号に必要な鍵ファイル内容を、**ダミートークンに限って**テストフィクスチャとしてリポジトリに置く（実トークンは絶対に置かない）。これで TS 版が無い CI でも後方互換の回帰を検知できる。

## 8. 落とし穴チェックリスト

1. 鍵の非対称（ファイル = 生バイト / 環境変数 = PBKDF2）を取り違えない。
2. GCM の認証タグを暗号文に連結しない（detached を使う）。
3. レガシー鍵はマスターキーと無関係の固定鍵。
4. 鍵ファイルは末尾に改行 1 個。読み込み時は `trim()`。
5. `create_new(true)` + mode 0600 で書く（`EEXIST` は正常系の分岐）。
6. hex 判定は大文字も受理、出力は小文字。
7. 空の暗号文（空トークン）は v2 では正当、レガシーでは不正。
8. 設定パスは `~/.slack-cli`（XDG ではない）。
9. 読み取りが書き込みを起こす（レガシー・平文の自動 v2 化）。
10. エラーメッセージは TS と同一文字列を返す（`common-client.md` §末尾の一覧参照）。
