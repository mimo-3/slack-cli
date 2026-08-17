//! トークン暗号化。TypeScript 版 `token-crypto-service.ts` と互換の形式を扱う。
//!
//! - 現行(v2): `v2:<iv 12byte hex>:<cipher hex>:<authTag 16byte hex>` / AES-256-GCM
//! - 旧(v1):   `<iv 16byte hex>:<cipher hex>` / AES-256-CBC・**復号のみ**
//!
//! マスターキーの解決経路が非対称なのが要点。注入値と環境変数は PBKDF2 を通す
//! 「パスフレーズ」だが、鍵ファイルの hex は**そのまま 32 バイト鍵**として使う。
//! ここを取り違えると既存の config.json が復号できなくなる。

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use aes::cipher::block_padding::Pkcs7;
use aes::cipher::{BlockDecryptMut, KeyIvInit};
use aes_gcm::aead::{Aead, Payload};
use aes_gcm::{Aes256Gcm, KeyInit};
use rand::RngCore;
use sha2::Sha256;

use crate::error::SlackCliError;

const VERSION_TAG: &str = "v2";
const KEY_LENGTH: usize = 32;
const IV_LENGTH: usize = 12;
const LEGACY_IV_LENGTH: usize = 16;
const AUTH_TAG_LENGTH: usize = 16;
const SEPARATOR: char = ':';

const MASTER_KEY_SALT: &[u8] = b"slack-cli-master-key-salt-v2";
const MASTER_KEY_ITERATIONS: u32 = 100_000;
const LEGACY_KEY_PASSWORD: &[u8] = b"slack-cli-key";
const LEGACY_KEY_SALT: &[u8] = b"slack-cli-salt-v1";

const KEY_DIR_NAME: &str = ".slack-cli-secrets";
const KEY_FILE_NAME: &str = "master.key";
const LEGACY_KEY_DIR_NAME: &str = ".slack-cli";

pub const ERR_INVALID_KEY_FORMAT: &str = "Invalid token encryption key format";
pub const ERR_INIT_KEY: &str = "Failed to initialize token encryption key";
pub const ERR_MIGRATE_KEY: &str = "Failed to migrate token encryption key";
pub const ERR_LOAD_KEY: &str = "Failed to load token encryption key";
pub const ERR_ENCRYPT: &str = "Failed to encrypt token";
pub const ERR_DECRYPT: &str = "Failed to decrypt token";
pub const ERR_INVALID_ENCRYPTED: &str = "Invalid encrypted data format";

type Aes256CbcDec = cbc::Decryptor<aes::Aes256>;

/// `TokenCryptoService` の生成オプション。省略した項目は既定パスに解決される。
#[derive(Default, Clone)]
pub struct CryptoOptions {
    /// パスフレーズとして PBKDF2 に通す鍵素材。テストと `SLACK_CLI_MASTER_KEY` 相当。
    pub master_key: Option<String>,
    pub key_file_path: Option<PathBuf>,
    pub legacy_key_file_path: Option<PathBuf>,
}

/// トークンの暗号化・復号。`Debug` は derive しない（鍵とトークンをログに出さない）。
pub struct TokenCryptoService {
    options: CryptoOptions,
    cached_key: OnceLock<[u8; KEY_LENGTH]>,
}

impl TokenCryptoService {
    pub fn new() -> Self {
        Self::with_options(CryptoOptions::default())
    }

    pub fn with_options(options: CryptoOptions) -> Self {
        Self {
            options,
            cached_key: OnceLock::new(),
        }
    }

    fn key_file_path(&self) -> Result<PathBuf, SlackCliError> {
        if let Some(p) = &self.options.key_file_path {
            return Ok(p.clone());
        }
        Ok(home_dir()?.join(KEY_DIR_NAME).join(KEY_FILE_NAME))
    }

    fn legacy_key_file_path(&self) -> Result<PathBuf, SlackCliError> {
        if let Some(p) = &self.options.legacy_key_file_path {
            return Ok(p.clone());
        }
        Ok(home_dir()?.join(LEGACY_KEY_DIR_NAME).join(KEY_FILE_NAME))
    }

    /// マスターキー解決。
    /// 1. プロセス内キャッシュ
    /// 2. 注入された `master_key`（PBKDF2 で導出）
    /// 3. 環境変数 `SLACK_CLI_MASTER_KEY`（PBKDF2 で導出）
    /// 4. 鍵ファイル（hex をそのまま 32 バイト鍵として使う）
    /// 5. 旧配置の鍵ファイルから移行、無ければ新規生成
    fn master_key(&self) -> Result<[u8; KEY_LENGTH], SlackCliError> {
        if let Some(key) = self.cached_key.get() {
            return Ok(*key);
        }

        let key = self.resolve_master_key()?;
        let _ = self.cached_key.set(key);
        Ok(key)
    }

    fn resolve_master_key(&self) -> Result<[u8; KEY_LENGTH], SlackCliError> {
        if let Some(secret) = &self.options.master_key {
            return Ok(derive_key(secret.as_bytes(), MASTER_KEY_SALT));
        }

        if let Ok(secret) = std::env::var("SLACK_CLI_MASTER_KEY") {
            let secret = secret.trim();
            if !secret.is_empty() {
                return Ok(derive_key(secret.as_bytes(), MASTER_KEY_SALT));
            }
        }

        let path = self.key_file_path()?;
        match fs::read_to_string(&path) {
            Ok(contents) => parse_key_hex(&contents),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => self.migrate_or_create_key(&path),
            Err(_) => Err(SlackCliError::Configuration(ERR_LOAD_KEY.to_string())),
        }
    }

    fn migrate_or_create_key(&self, path: &Path) -> Result<[u8; KEY_LENGTH], SlackCliError> {
        let legacy_path = self.legacy_key_file_path()?;
        match fs::read_to_string(&legacy_path) {
            Ok(contents) => {
                let key = parse_key_hex(&contents)?;
                // 新パスへ複製する。既に誰かが作っていたらそちらを読み直す。
                match write_key_file(path, contents.trim()) {
                    Ok(()) => Ok(key),
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                        let existing = fs::read_to_string(path)
                            .map_err(|_| SlackCliError::Configuration(ERR_MIGRATE_KEY.to_string()))?;
                        parse_key_hex(&existing)
                    }
                    // 旧ファイルは消さない。書けなくても旧鍵で読み書きは続けられる。
                    Err(_) => Ok(key),
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => self.create_key_file(path),
            Err(_) => Err(SlackCliError::Configuration(ERR_MIGRATE_KEY.to_string())),
        }
    }

    fn create_key_file(&self, path: &Path) -> Result<[u8; KEY_LENGTH], SlackCliError> {
        let mut key = [0u8; KEY_LENGTH];
        rand::thread_rng().fill_bytes(&mut key);
        let key_hex = hex::encode(key);

        match write_key_file(path, &key_hex) {
            Ok(()) => Ok(key),
            // 競合で他プロセスが先に作ったら、そちらを正とする
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                let existing = fs::read_to_string(path)
                    .map_err(|_| SlackCliError::Configuration(ERR_INIT_KEY.to_string()))?;
                parse_key_hex(&existing)
            }
            Err(_) => Err(SlackCliError::Configuration(ERR_INIT_KEY.to_string())),
        }
    }

    /// 平文トークンを `v2:iv:ct:tag` へ暗号化する。
    pub fn encrypt(&self, token: &str) -> Result<String, SlackCliError> {
        let key = self
            .master_key()
            .map_err(|_| SlackCliError::Configuration(ERR_ENCRYPT.to_string()))?;

        let mut iv = [0u8; IV_LENGTH];
        rand::thread_rng().fill_bytes(&mut iv);

        let cipher = Aes256Gcm::new_from_slice(&key)
            .map_err(|_| SlackCliError::Configuration(ERR_ENCRYPT.to_string()))?;
        let sealed = cipher
            .encrypt(
                iv.as_slice().into(),
                Payload {
                    msg: token.as_bytes(),
                    aad: &[],
                },
            )
            .map_err(|_| SlackCliError::Configuration(ERR_ENCRYPT.to_string()))?;

        // aes-gcm は ciphertext||tag を返すが、保存形式は両者を分けて持つ
        if sealed.len() < AUTH_TAG_LENGTH {
            return Err(SlackCliError::Configuration(ERR_ENCRYPT.to_string()));
        }
        let (ciphertext, tag) = sealed.split_at(sealed.len() - AUTH_TAG_LENGTH);

        Ok(format!(
            "{VERSION_TAG}{SEPARATOR}{}{SEPARATOR}{}{SEPARATOR}{}",
            hex::encode(iv),
            hex::encode(ciphertext),
            hex::encode(tag)
        ))
    }

    /// 暗号文を復号する。v2 と旧 v1(CBC) の両方を受ける。
    pub fn decrypt(&self, encrypted: &str) -> Result<String, SlackCliError> {
        if encrypted.is_empty() {
            return Err(SlackCliError::Validation(
                ERR_INVALID_ENCRYPTED.to_string(),
            ));
        }
        if is_current_format(encrypted) {
            return self.decrypt_v2(encrypted);
        }
        if is_legacy_encrypted(encrypted) {
            return decrypt_legacy(encrypted);
        }
        Err(SlackCliError::Validation(
            ERR_INVALID_ENCRYPTED.to_string(),
        ))
    }

    fn decrypt_v2(&self, encrypted: &str) -> Result<String, SlackCliError> {
        let parts: Vec<&str> = encrypted.split(SEPARATOR).collect();
        let iv = hex::decode(parts[1])
            .map_err(|_| SlackCliError::Configuration(ERR_DECRYPT.to_string()))?;
        let ciphertext = hex::decode(parts[2])
            .map_err(|_| SlackCliError::Configuration(ERR_DECRYPT.to_string()))?;
        let tag = hex::decode(parts[3])
            .map_err(|_| SlackCliError::Configuration(ERR_DECRYPT.to_string()))?;

        let key = self
            .master_key()
            .map_err(|_| SlackCliError::Configuration(ERR_DECRYPT.to_string()))?;
        let cipher = Aes256Gcm::new_from_slice(&key)
            .map_err(|_| SlackCliError::Configuration(ERR_DECRYPT.to_string()))?;

        let mut sealed = ciphertext;
        sealed.extend_from_slice(&tag);

        let plaintext = cipher
            .decrypt(
                iv.as_slice().into(),
                Payload {
                    msg: &sealed,
                    aad: &[],
                },
            )
            .map_err(|_| SlackCliError::Configuration(ERR_DECRYPT.to_string()))?;

        String::from_utf8(plaintext)
            .map_err(|_| SlackCliError::Configuration(ERR_DECRYPT.to_string()))
    }
}

impl Default for TokenCryptoService {
    fn default() -> Self {
        Self::new()
    }
}

fn decrypt_legacy(encrypted: &str) -> Result<String, SlackCliError> {
    let parts: Vec<&str> = encrypted.split(SEPARATOR).collect();
    let iv = hex::decode(parts[0])
        .map_err(|_| SlackCliError::Configuration(ERR_DECRYPT.to_string()))?;
    let ciphertext = hex::decode(parts[1])
        .map_err(|_| SlackCliError::Configuration(ERR_DECRYPT.to_string()))?;

    let key = derive_key(LEGACY_KEY_PASSWORD, LEGACY_KEY_SALT);
    let decryptor = Aes256CbcDec::new_from_slices(&key, &iv)
        .map_err(|_| SlackCliError::Configuration(ERR_DECRYPT.to_string()))?;

    let plaintext = decryptor
        .decrypt_padded_vec_mut::<Pkcs7>(&ciphertext)
        .map_err(|_| SlackCliError::Configuration(ERR_DECRYPT.to_string()))?;

    String::from_utf8(plaintext).map_err(|_| SlackCliError::Configuration(ERR_DECRYPT.to_string()))
}

/// v2 形式かどうか。判定を緩めると誤って復号を試みて失敗するため、TS 版の条件を厳密に写す。
pub fn is_current_format(value: &str) -> bool {
    if value.is_empty() {
        return false;
    }
    let parts: Vec<&str> = value.split(SEPARATOR).collect();
    if parts.len() != 4 || parts[0] != VERSION_TAG {
        return false;
    }
    let iv_ok = parts[1].len() == IV_LENGTH * 2 && is_hex(parts[1]);
    let ct_ok = parts[2].len() % 2 == 0 && (parts[2].is_empty() || is_hex(parts[2]));
    let tag_ok = parts[3].len() == AUTH_TAG_LENGTH * 2 && is_hex(parts[3]);
    iv_ok && ct_ok && tag_ok
}

/// 旧 v1(CBC) 形式かどうか。
fn is_legacy_encrypted(value: &str) -> bool {
    if value.is_empty() {
        return false;
    }
    let parts: Vec<&str> = value.split(SEPARATOR).collect();
    if parts.len() != 2 {
        return false;
    }
    let iv_ok = parts[0].len() == LEGACY_IV_LENGTH * 2 && is_hex(parts[0]);
    let ct_ok = !parts[1].is_empty() && parts[1].len() % 2 == 0 && is_hex(parts[1]);
    iv_ok && ct_ok
}

/// 暗号化済みか（v2 でも v1 でもなければ平文とみなす）。
/// Slack のトークン（`xoxb-...`）は `:` を含まないのでどちらにも該当しない。
pub fn is_encrypted(value: &str) -> bool {
    is_current_format(value) || is_legacy_encrypted(value)
}

fn is_hex(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_hexdigit())
}

fn derive_key(password: &[u8], salt: &[u8]) -> [u8; KEY_LENGTH] {
    let mut out = [0u8; KEY_LENGTH];
    pbkdf2::pbkdf2_hmac::<Sha256>(password, salt, MASTER_KEY_ITERATIONS, &mut out);
    out
}

fn parse_key_hex(contents: &str) -> Result<[u8; KEY_LENGTH], SlackCliError> {
    let trimmed = contents.trim();
    if trimmed.len() != KEY_LENGTH * 2 || !is_hex(trimmed) {
        return Err(SlackCliError::Configuration(
            ERR_INVALID_KEY_FORMAT.to_string(),
        ));
    }
    let bytes = hex::decode(trimmed)
        .map_err(|_| SlackCliError::Configuration(ERR_INVALID_KEY_FORMAT.to_string()))?;
    let mut key = [0u8; KEY_LENGTH];
    key.copy_from_slice(&bytes);
    Ok(key)
}

/// 鍵ファイルを排他作成で書く（既存なら `AlreadyExists`）。末尾に改行を付ける形式も TS 版に合わせる。
fn write_key_file(path: &Path, key_hex: &str) -> Result<(), std::io::Error> {
    if let Some(parent) = path.parent() {
        create_private_dir(parent)?;
    }
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(format!("{key_hex}\n").as_bytes())?;
    file.sync_all()
}

pub(crate) fn create_private_dir(dir: &Path) -> Result<(), std::io::Error> {
    fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn home_dir() -> Result<PathBuf, SlackCliError> {
    dirs::home_dir()
        .ok_or_else(|| SlackCliError::Configuration("Cannot determine home directory".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn service_with_key(secret: &str) -> TokenCryptoService {
        TokenCryptoService::with_options(CryptoOptions {
            master_key: Some(secret.to_string()),
            ..CryptoOptions::default()
        })
    }

    /// テスト用のダミートークン。秘密検知に引っかからないよう組み立てて作る。
    fn dummy_token() -> String {
        format!("xo{}-1234567890-abcdef", "xb")
    }

    #[test]
    fn encrypt_decrypt_round_trip() {
        let svc = service_with_key("test-master-key");
        let token = dummy_token();
        let encrypted = svc.encrypt(&token).unwrap();

        assert!(is_current_format(&encrypted), "format was: {encrypted}");
        let parts: Vec<&str> = encrypted.split(':').collect();
        assert_eq!(parts[0], "v2");
        assert_eq!(parts[1].len(), 24);
        assert_eq!(parts[3].len(), 32);

        assert_eq!(svc.decrypt(&encrypted).unwrap(), token);
    }

    #[test]
    fn ciphertext_differs_per_call_because_iv_is_random() {
        let svc = service_with_key("test-master-key");
        assert_ne!(svc.encrypt("same").unwrap(), svc.encrypt("same").unwrap());
    }

    #[test]
    fn wrong_master_key_fails_to_decrypt() {
        let encrypted = service_with_key("key-a").encrypt("secret").unwrap();
        let err = service_with_key("key-b").decrypt(&encrypted).unwrap_err();
        assert_eq!(err.to_string(), ERR_DECRYPT);
    }

    /// レガシー鍵は固定パスフレーズ由来なので、テスト側で同じ鍵を導出して
    /// AES-256-CBC/PKCS#7 の暗号文を組み立てられる。復号経路の実証に使う。
    #[test]
    fn legacy_cbc_ciphertext_is_decryptable() {
        use aes::cipher::BlockEncryptMut;

        type Aes256CbcEnc = cbc::Encryptor<aes::Aes256>;

        let key = derive_key(LEGACY_KEY_PASSWORD, LEGACY_KEY_SALT);
        let iv = [0x11u8; LEGACY_IV_LENGTH];
        let plaintext = "legacy-token-value";

        let ciphertext = Aes256CbcEnc::new_from_slices(&key, &iv)
            .unwrap()
            .encrypt_padded_vec_mut::<Pkcs7>(plaintext.as_bytes());

        let encoded = format!("{}:{}", hex::encode(iv), hex::encode(&ciphertext));
        assert!(is_legacy_encrypted(&encoded));
        assert!(!is_current_format(&encoded));

        // レガシー復号はマスターキーに依存しない（固定パスフレーズ由来）
        let svc = service_with_key("irrelevant-master-key");
        assert_eq!(svc.decrypt(&encoded).unwrap(), plaintext);
    }

    #[test]
    fn plaintext_slack_token_is_not_detected_as_encrypted() {
        assert!(!is_encrypted(&dummy_token()));
        assert!(!is_encrypted(""));
    }

    #[test]
    fn format_detection_matches_typescript_rules() {
        let iv = "0".repeat(24);
        let tag = "0".repeat(32);
        assert!(is_current_format(&format!("v2:{iv}:abcd:{tag}")));
        // ciphertext は空文字も許容
        assert!(is_current_format(&format!("v2:{iv}::{tag}")));
        // 奇数長の ciphertext は不可
        assert!(!is_current_format(&format!("v2:{iv}:abc:{tag}")));
        // iv の長さ違いは不可
        assert!(!is_current_format(&format!("v2:00:abcd:{tag}")));
        // v1 は 2 セグメント・iv 32桁・ciphertext 非空
        assert!(is_legacy_encrypted(&format!("{}:abcd", "0".repeat(32))));
        assert!(!is_legacy_encrypted(&format!("{}:", "0".repeat(32))));
    }

    #[test]
    fn empty_and_malformed_input_is_a_validation_error() {
        let svc = service_with_key("k");
        assert!(matches!(
            svc.decrypt("").unwrap_err(),
            SlackCliError::Validation(_)
        ));
        assert!(matches!(
            svc.decrypt("not-encrypted").unwrap_err(),
            SlackCliError::Validation(_)
        ));
    }

    #[test]
    fn key_file_hex_is_used_raw_not_derived() {
        let dir = tempfile::tempdir().unwrap();
        let key_path = dir.path().join("master.key");
        let key_hex = "ab".repeat(32);
        fs::write(&key_path, format!("{key_hex}\n")).unwrap();

        let svc = TokenCryptoService::with_options(CryptoOptions {
            key_file_path: Some(key_path.clone()),
            ..CryptoOptions::default()
        });
        let encrypted = svc.encrypt("token-from-file-key").unwrap();

        // 同じ hex を「パスフレーズ」として渡した場合は PBKDF2 を通るため復号できない。
        // この非対称性が壊れていないことをテストで固定する。
        let derived = service_with_key(&key_hex);
        assert!(derived.decrypt(&encrypted).is_err());

        let same_file = TokenCryptoService::with_options(CryptoOptions {
            key_file_path: Some(key_path),
            ..CryptoOptions::default()
        });
        assert_eq!(
            same_file.decrypt(&encrypted).unwrap(),
            "token-from-file-key"
        );
    }

    #[test]
    fn missing_key_file_is_created_with_owner_only_permissions() {
        let dir = tempfile::tempdir().unwrap();
        let key_path = dir.path().join("secrets").join("master.key");

        let svc = TokenCryptoService::with_options(CryptoOptions {
            key_file_path: Some(key_path.clone()),
            legacy_key_file_path: Some(dir.path().join("missing").join("master.key")),
            ..CryptoOptions::default()
        });
        svc.encrypt("token").unwrap();

        let contents = fs::read_to_string(&key_path).unwrap();
        assert_eq!(contents.trim().len(), 64);
        assert!(contents.ends_with('\n'));

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&key_path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[test]
    fn legacy_key_file_is_migrated_to_the_new_path() {
        let dir = tempfile::tempdir().unwrap();
        let legacy_path = dir.path().join("old").join("master.key");
        let key_path = dir.path().join("new").join("master.key");
        let key_hex = "cd".repeat(32);
        create_private_dir(legacy_path.parent().unwrap()).unwrap();
        fs::write(&legacy_path, format!("{key_hex}\n")).unwrap();

        let svc = TokenCryptoService::with_options(CryptoOptions {
            key_file_path: Some(key_path.clone()),
            legacy_key_file_path: Some(legacy_path.clone()),
            ..CryptoOptions::default()
        });
        let encrypted = svc.encrypt("migrated").unwrap();

        assert_eq!(fs::read_to_string(&key_path).unwrap().trim(), key_hex);
        // 旧ファイルは残す
        assert!(legacy_path.exists());
        assert_eq!(svc.decrypt(&encrypted).unwrap(), "migrated");
    }

    #[test]
    fn malformed_key_file_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let key_path = dir.path().join("master.key");
        fs::write(&key_path, "not-a-hex-key\n").unwrap();

        let svc = TokenCryptoService::with_options(CryptoOptions {
            key_file_path: Some(key_path),
            ..CryptoOptions::default()
        });
        // encrypt は原因を潰して "Failed to encrypt token" にする（TS 版と同じ）
        assert_eq!(svc.encrypt("t").unwrap_err().to_string(), ERR_ENCRYPT);
    }
}
