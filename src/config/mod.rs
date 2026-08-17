//! 設定ファイル（プロファイルとトークン）の読み書き。
//!
//! 保存先は `~/.slack-cli/config.json`。ディレクトリ 0700 / ファイル 0600。
//! 書き込みは「一時ファイルへ排他作成 → rename」の原子的書き込みで、途中状態を残さない。
//! 形式は TypeScript 版と互換なので、既存の config.json をそのまま読める。
//!
//! ```json
//! {
//!   "profiles": { "default": { "token": "v2:...", "updatedAt": "2026-01-01T00:00:00.000Z" } },
//!   "defaultProfile": "default"
//! }
//! ```

pub mod crypto;

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use crate::error::SlackCliError;
use crypto::{CryptoOptions, TokenCryptoService};

pub const DEFAULT_PROFILE_NAME: &str = "default";
pub const CONFIG_DIR_NAME: &str = ".slack-cli";
pub const CONFIG_FILE_NAME: &str = "config.json";
pub const ERR_INVALID_CONFIG_FORMAT: &str = "Invalid config file format";

const TOKEN_MASK_LENGTH: usize = 4;
const TOKEN_MIN_LENGTH: usize = 9;

/// 1 プロファイル分の保存内容。`token` は暗号化済み文字列。
/// `Debug` を手書きしてトークンを伏せる（`{:?}` でログに流れないように）。
#[derive(Clone, Serialize, Deserialize)]
pub struct StoredProfile {
    pub token: String,
    #[serde(rename = "updatedAt")]
    pub updated_at: String,
}

impl std::fmt::Debug for StoredProfile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StoredProfile")
            .field("token", &"***")
            .field("updated_at", &self.updated_at)
            .finish()
    }
}

/// config.json 全体。プロファイルの順序は `clear` 後の既定プロファイル再選出に
/// 影響するため、`IndexMap` で挿入順を保つ。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ConfigStore {
    #[serde(default)]
    pub profiles: IndexMap<String, StoredProfile>,
    #[serde(rename = "defaultProfile", skip_serializing_if = "Option::is_none")]
    pub default_profile: Option<String>,
}

/// 旧形式（トップレベルに token がある）を判定するための読み取り用構造。
#[derive(Deserialize)]
struct LegacyStore {
    token: Option<String>,
    #[serde(rename = "updatedAt")]
    updated_at: Option<String>,
    profiles: Option<serde_json::Value>,
}

/// `config profiles` 用のエントリ。`token` は**復号していない**保存値そのまま。
#[derive(Clone, Debug)]
pub struct ProfileEntry {
    pub name: String,
    pub token: String,
    pub updated_at: String,
    pub is_default: bool,
}

/// 復号済みのプロファイル設定。
#[derive(Clone)]
pub struct ResolvedConfig {
    pub token: String,
    pub updated_at: String,
}

impl std::fmt::Debug for ResolvedConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResolvedConfig")
            .field("token", &"***")
            .field("updated_at", &self.updated_at)
            .finish()
    }
}

/// 設定ファイルの入口。テストでは `with_options` で保存先と鍵を差し替える。
pub struct ProfileConfigManager {
    config_dir: PathBuf,
    crypto: TokenCryptoService,
}

/// `ProfileConfigManager` の生成オプション。
#[derive(Default)]
pub struct ConfigOptions {
    pub config_dir: Option<PathBuf>,
    pub crypto: Option<CryptoOptions>,
}

impl ProfileConfigManager {
    pub fn new() -> Result<Self, SlackCliError> {
        Self::with_options(ConfigOptions::default())
    }

    pub fn with_options(options: ConfigOptions) -> Result<Self, SlackCliError> {
        let config_dir = match options.config_dir {
            Some(dir) => dir,
            None => dirs::home_dir()
                .ok_or_else(|| {
                    SlackCliError::Configuration("Cannot determine home directory".into())
                })?
                .join(CONFIG_DIR_NAME),
        };
        Ok(Self {
            config_dir,
            crypto: TokenCryptoService::with_options(options.crypto.unwrap_or_default()),
        })
    }

    pub fn config_path(&self) -> PathBuf {
        self.config_dir.join(CONFIG_FILE_NAME)
    }

    /// 設定を読む。ファイルが無ければ空ストア。旧形式なら現行形式へ移行して保存する。
    pub fn load_store(&self) -> Result<ConfigStore, SlackCliError> {
        let path = self.config_path();
        let contents = match fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(ConfigStore::default())
            }
            Err(e) => return Err(e.into()),
        };

        let legacy: LegacyStore = serde_json::from_str(&contents).map_err(|_| {
            SlackCliError::Configuration(ERR_INVALID_CONFIG_FORMAT.to_string())
        })?;

        let needs_migration = legacy
            .token
            .as_ref()
            .is_some_and(|t| !t.is_empty())
            && legacy.profiles.is_none();

        if needs_migration {
            return self.migrate_legacy_store(legacy);
        }

        serde_json::from_str(&contents).map_err(|_| {
            SlackCliError::Configuration(ERR_INVALID_CONFIG_FORMAT.to_string())
        })
    }

    fn migrate_legacy_store(&self, legacy: LegacyStore) -> Result<ConfigStore, SlackCliError> {
        let raw = legacy.token.unwrap_or_default();
        let plaintext = if crypto::is_encrypted(&raw) {
            self.crypto.decrypt(&raw)?
        } else {
            raw
        };

        let mut profiles = IndexMap::new();
        profiles.insert(
            DEFAULT_PROFILE_NAME.to_string(),
            StoredProfile {
                token: self.crypto.encrypt(&plaintext)?,
                updated_at: legacy.updated_at.unwrap_or_else(now_iso8601),
            },
        );

        let store = ConfigStore {
            profiles,
            default_profile: Some(DEFAULT_PROFILE_NAME.to_string()),
        };
        self.save_store(&store)?;
        Ok(store)
    }

    /// 設定を原子的に書き込む。ディレクトリ 0700 / ファイル 0600。
    pub fn save_store(&self, store: &ConfigStore) -> Result<(), SlackCliError> {
        let contents = serde_json::to_string_pretty(store)?;
        write_private(&self.config_path(), &contents)
    }

    /// プロファイル名の解決順序（全メソッド共通）:
    /// 引数 → `defaultProfile` → `"default"`。
    fn resolve_profile_name(store: &ConfigStore, profile: Option<&str>) -> String {
        profile
            .filter(|p| !p.is_empty())
            .map(str::to_string)
            .or_else(|| store.default_profile.clone())
            .unwrap_or_else(|| DEFAULT_PROFILE_NAME.to_string())
    }

    /// トークンを暗号化して保存する。
    pub fn set_token(&self, token: &str, profile: Option<&str>) -> Result<String, SlackCliError> {
        let mut store = self.load_store()?;
        let name = Self::resolve_profile_name(&store, profile);

        store.profiles.insert(
            name.clone(),
            StoredProfile {
                token: self.crypto.encrypt(token)?,
                updated_at: now_iso8601(),
            },
        );

        // 既定が未設定のとき、または "default" を設定したときは既定を張り替える
        if store.default_profile.is_none() || name == DEFAULT_PROFILE_NAME {
            store.default_profile = Some(name.clone());
        }

        self.save_store(&store)?;
        Ok(name)
    }

    /// 復号済みの設定を取得する。プロファイルが無ければ `None`。
    ///
    /// 注意: 保存値が現行形式でない（平文 or 旧形式）場合、現行形式で暗号化し直して
    /// **ディスクへ書き戻す**。読み取り操作が書き込みを起こすのは TS 版と同じ挙動。
    pub fn get_config(&self, profile: Option<&str>) -> Result<Option<ResolvedConfig>, SlackCliError> {
        let mut store = self.load_store()?;
        let name = Self::resolve_profile_name(&store, profile);

        let Some(stored) = store.profiles.get(&name).cloned() else {
            return Ok(None);
        };

        let plaintext = if crypto::is_encrypted(&stored.token) {
            self.crypto.decrypt(&stored.token)?
        } else {
            stored.token.clone()
        };

        if !crypto::is_current_format(&stored.token) {
            let reencrypted = self.crypto.encrypt(&plaintext)?;
            if let Some(entry) = store.profiles.get_mut(&name) {
                entry.token = reencrypted;
            }
            self.save_store(&store)?;
        }

        Ok(Some(ResolvedConfig {
            token: plaintext,
            updated_at: stored.updated_at,
        }))
    }

    /// 設定が無ければエラーにする（TypeScript 版 `getConfigOrThrow` 相当）。
    pub fn get_config_or_error(
        &self,
        profile: Option<&str>,
    ) -> Result<ResolvedConfig, SlackCliError> {
        if let Some(config) = self.get_config(profile)? {
            return Ok(config);
        }
        let store = self.load_store()?;
        let name = Self::resolve_profile_name(&store, profile);
        Err(SlackCliError::Configuration(no_config_message(&name)))
    }

    /// プロファイル一覧。**復号しない**ので `token` は暗号文のまま。
    pub fn list_profiles(&self) -> Result<Vec<ProfileEntry>, SlackCliError> {
        let store = self.load_store()?;
        let current = store
            .default_profile
            .clone()
            .unwrap_or_else(|| DEFAULT_PROFILE_NAME.to_string());

        Ok(store
            .profiles
            .iter()
            .map(|(name, profile)| ProfileEntry {
                name: name.clone(),
                token: profile.token.clone(),
                updated_at: profile.updated_at.clone(),
                is_default: *name == current,
            })
            .collect())
    }

    /// 既定プロファイルを切り替える。存在しない名前はエラー。
    pub fn use_profile(&self, profile: &str) -> Result<(), SlackCliError> {
        let mut store = self.load_store()?;
        if !store.profiles.contains_key(profile) {
            return Err(SlackCliError::Configuration(format!(
                "Profile \"{profile}\" does not exist"
            )));
        }
        store.default_profile = Some(profile.to_string());
        self.save_store(&store)
    }

    pub fn current_profile(&self) -> Result<String, SlackCliError> {
        let store = self.load_store()?;
        Ok(store
            .default_profile
            .unwrap_or_else(|| DEFAULT_PROFILE_NAME.to_string()))
    }

    /// プロファイルを削除する。存在しない名前でもエラーにはしない（TS 版と同じ）。
    /// 既定を消した場合は残りの先頭を新しい既定にし、残り 0 件なら設定ファイルごと消す。
    pub fn clear_config(&self, profile: Option<&str>) -> Result<String, SlackCliError> {
        let mut store = self.load_store()?;
        let name = Self::resolve_profile_name(&store, profile);
        store.profiles.shift_remove(&name);

        if store.default_profile.as_deref() == Some(name.as_str()) {
            match store.profiles.keys().next() {
                Some(next) => store.default_profile = Some(next.clone()),
                None => {
                    match fs::remove_file(self.config_path()) {
                        Ok(()) => {}
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                        Err(e) => return Err(e.into()),
                    }
                    return Ok(name);
                }
            }
        }

        self.save_store(&store)?;
        Ok(name)
    }

    /// トークン解決チェーン: 明示指定 → 環境変数 `SLACK_CLI_TOKEN` → 設定ファイル。
    pub fn resolve_token(
        &self,
        explicit: Option<&str>,
        profile: Option<&str>,
    ) -> Result<String, SlackCliError> {
        if let Some(token) = explicit.map(str::trim).filter(|t| !t.is_empty()) {
            return Ok(token.to_string());
        }
        if let Ok(token) = std::env::var("SLACK_CLI_TOKEN") {
            let token = token.trim();
            if !token.is_empty() {
                return Ok(token.to_string());
            }
        }
        match self.get_config(profile)? {
            Some(config) => Ok(config.token),
            None => Err(SlackCliError::NotAuthenticated),
        }
    }
}

/// `No configuration found ...` の文言（TypeScript 版 `ERROR_MESSAGES.NO_CONFIG`）。
pub fn no_config_message(profile: &str) -> String {
    format!(
        "No configuration found for profile \"{profile}\". \
         Use \"slack-cli config set --token <token> --profile {profile}\" to set up."
    )
}

/// トークンのマスク表示。長さ 9 以下なら `****`、それ以外は先頭4 + 末尾4 を残す。
pub fn mask_token(token: &str) -> String {
    let chars: Vec<char> = token.chars().collect();
    if chars.len() <= TOKEN_MIN_LENGTH {
        return "****".to_string();
    }
    let head: String = chars[..TOKEN_MASK_LENGTH].iter().collect();
    let tail: String = chars[chars.len() - TOKEN_MASK_LENGTH..].iter().collect();
    format!("{head}-****-****-{tail}")
}

fn now_iso8601() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// 一時ファイルへ排他作成で書いてから rename する原子的書き込み。
/// 途中で失敗したら一時ファイルを消して、平文の断片を残さない。
fn write_private(path: &Path, contents: &str) -> Result<(), SlackCliError> {
    if let Some(parent) = path.parent() {
        crypto::create_private_dir(parent)?;
    }

    let temp_path = path.with_extension(format!("json.{}.{}.tmp", std::process::id(), epoch_millis()));

    let result = (|| -> Result<(), SlackCliError> {
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temp_path)?;
        file.write_all(contents.as_bytes())?;
        file.sync_all()?;
        fs::rename(&temp_path, path)?;
        Ok(())
    })();

    if result.is_err() {
        let _ = fs::remove_file(&temp_path);
    }
    result
}

fn epoch_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn manager(dir: &TempDir) -> ProfileConfigManager {
        ProfileConfigManager::with_options(ConfigOptions {
            config_dir: Some(dir.path().join(CONFIG_DIR_NAME)),
            crypto: Some(CryptoOptions {
                master_key: Some("unit-test-master-key".to_string()),
                ..CryptoOptions::default()
            }),
        })
        .unwrap()
    }

    fn dummy_token(suffix: &str) -> String {
        format!("xo{}-0000000000-{suffix}", "xb")
    }

    #[test]
    fn set_then_get_round_trips_the_token() {
        let dir = TempDir::new().unwrap();
        let mgr = manager(&dir);
        let token = dummy_token("alpha");

        assert_eq!(mgr.set_token(&token, None).unwrap(), "default");
        assert_eq!(mgr.get_config(None).unwrap().unwrap().token, token);
    }

    #[test]
    fn token_is_encrypted_on_disk() {
        let dir = TempDir::new().unwrap();
        let mgr = manager(&dir);
        let token = dummy_token("beta");
        mgr.set_token(&token, None).unwrap();

        let raw = fs::read_to_string(mgr.config_path()).unwrap();
        assert!(!raw.contains(&token), "plaintext token leaked to disk");
        assert!(raw.contains("\"v2:"), "on-disk value was: {raw}");
    }

    #[test]
    fn config_file_is_owner_only() {
        let dir = TempDir::new().unwrap();
        let mgr = manager(&dir);
        mgr.set_token(&dummy_token("perm"), None).unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let file_mode = fs::metadata(mgr.config_path()).unwrap().permissions().mode();
            assert_eq!(file_mode & 0o777, 0o600);
            let dir_mode = fs::metadata(dir.path().join(CONFIG_DIR_NAME))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(dir_mode & 0o777, 0o700);
        }
    }

    #[test]
    fn atomic_write_leaves_no_temp_file() {
        let dir = TempDir::new().unwrap();
        let mgr = manager(&dir);
        mgr.set_token(&dummy_token("atomic"), None).unwrap();

        let leftovers: Vec<_> = fs::read_dir(dir.path().join(CONFIG_DIR_NAME))
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|name| name.ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "temp files left behind: {leftovers:?}");
    }

    #[test]
    fn profile_resolution_prefers_argument_then_default_then_literal_default() {
        let dir = TempDir::new().unwrap();
        let mgr = manager(&dir);

        mgr.set_token(&dummy_token("d"), None).unwrap();
        mgr.set_token(&dummy_token("w"), Some("work")).unwrap();
        // "work" は "default" ではないので既定は据え置き
        assert_eq!(mgr.current_profile().unwrap(), "default");

        mgr.use_profile("work").unwrap();
        assert_eq!(mgr.current_profile().unwrap(), "work");
        // 引数なしなら既定プロファイルが引かれる
        assert_eq!(mgr.get_config(None).unwrap().unwrap().token, dummy_token("w"));
        // 引数があればそちらが勝つ
        assert_eq!(
            mgr.get_config(Some("default")).unwrap().unwrap().token,
            dummy_token("d")
        );
    }

    #[test]
    fn use_profile_rejects_unknown_name() {
        let dir = TempDir::new().unwrap();
        let mgr = manager(&dir);
        let err = mgr.use_profile("nope").unwrap_err();
        assert_eq!(err.to_string(), "Profile \"nope\" does not exist");
    }

    #[test]
    fn clear_promotes_the_first_remaining_profile_to_default() {
        let dir = TempDir::new().unwrap();
        let mgr = manager(&dir);
        mgr.set_token(&dummy_token("a"), Some("alpha")).unwrap();
        mgr.set_token(&dummy_token("b"), Some("beta")).unwrap();
        mgr.use_profile("beta").unwrap();

        mgr.clear_config(Some("beta")).unwrap();
        // 残りの先頭（挿入順で alpha）が新しい既定になる
        assert_eq!(mgr.current_profile().unwrap(), "alpha");
    }

    #[test]
    fn clearing_the_last_profile_removes_the_config_file() {
        let dir = TempDir::new().unwrap();
        let mgr = manager(&dir);
        mgr.set_token(&dummy_token("only"), None).unwrap();

        mgr.clear_config(None).unwrap();
        assert!(!mgr.config_path().exists());
        // ファイルが無くても現在のプロファイル照会は "default" を返す
        assert_eq!(mgr.current_profile().unwrap(), "default");
    }

    #[test]
    fn clearing_an_unknown_profile_is_not_an_error() {
        let dir = TempDir::new().unwrap();
        let mgr = manager(&dir);
        mgr.set_token(&dummy_token("keep"), None).unwrap();
        assert_eq!(mgr.clear_config(Some("ghost")).unwrap(), "ghost");
        assert!(mgr.get_config(Some("default")).unwrap().is_some());
    }

    #[test]
    fn plaintext_token_is_readable_and_gets_reencrypted_on_read() {
        let dir = TempDir::new().unwrap();
        let mgr = manager(&dir);
        let token = dummy_token("legacyplain");

        crypto::create_private_dir(&dir.path().join(CONFIG_DIR_NAME)).unwrap();
        fs::write(
            mgr.config_path(),
            serde_json::json!({
                "profiles": { "default": { "token": token, "updatedAt": "2026-01-01T00:00:00.000Z" } },
                "defaultProfile": "default"
            })
            .to_string(),
        )
        .unwrap();

        assert_eq!(mgr.get_config(None).unwrap().unwrap().token, token);
        // 読み取りが書き戻しを起こしている
        let raw = fs::read_to_string(mgr.config_path()).unwrap();
        assert!(raw.contains("\"v2:"), "not re-encrypted: {raw}");
        assert!(!raw.contains(&token));
    }

    #[test]
    fn legacy_top_level_token_is_migrated_into_the_default_profile() {
        let dir = TempDir::new().unwrap();
        let mgr = manager(&dir);
        let token = dummy_token("oldshape");

        crypto::create_private_dir(&dir.path().join(CONFIG_DIR_NAME)).unwrap();
        fs::write(
            mgr.config_path(),
            serde_json::json!({ "token": token, "updatedAt": "2026-01-01T00:00:00.000Z" })
                .to_string(),
        )
        .unwrap();

        let store = mgr.load_store().unwrap();
        assert_eq!(store.default_profile.as_deref(), Some("default"));
        assert_eq!(
            store.profiles["default"].updated_at,
            "2026-01-01T00:00:00.000Z"
        );
        assert_eq!(mgr.get_config(None).unwrap().unwrap().token, token);
    }

    #[test]
    fn broken_json_reports_invalid_config_format() {
        let dir = TempDir::new().unwrap();
        let mgr = manager(&dir);
        crypto::create_private_dir(&dir.path().join(CONFIG_DIR_NAME)).unwrap();
        fs::write(mgr.config_path(), "{ not json").unwrap();

        assert_eq!(
            mgr.load_store().unwrap_err().to_string(),
            ERR_INVALID_CONFIG_FORMAT
        );
    }

    #[test]
    fn missing_config_resolves_to_not_authenticated() {
        let dir = TempDir::new().unwrap();
        let mgr = manager(&dir);
        // 環境変数の影響を受けないよう、明示トークン経路だけを見る
        assert_eq!(mgr.resolve_token(Some("explicit"), None).unwrap(), "explicit");
        assert!(mgr.get_config(None).unwrap().is_none());
        assert!(mgr
            .get_config_or_error(None)
            .unwrap_err()
            .to_string()
            .starts_with("No configuration found for profile \"default\""));
    }

    #[test]
    fn list_profiles_keeps_insertion_order_and_does_not_decrypt() {
        let dir = TempDir::new().unwrap();
        let mgr = manager(&dir);
        mgr.set_token(&dummy_token("1"), Some("alpha")).unwrap();
        mgr.set_token(&dummy_token("2"), Some("beta")).unwrap();
        mgr.use_profile("beta").unwrap();

        let profiles = mgr.list_profiles().unwrap();
        assert_eq!(
            profiles.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
            vec!["alpha", "beta"]
        );
        assert!(profiles[1].is_default);
        assert!(profiles[0].token.starts_with("v2:"));
    }

    #[test]
    fn mask_token_threshold_is_nine_characters() {
        assert_eq!(mask_token("123456789"), "****");
        assert_eq!(mask_token("1234567890"), "1234-****-****-7890");
        assert_eq!(mask_token(""), "****");
    }
}
