//! `slack-cli draft` — ローカルに保存する下書きの管理。
//!
//! `draft send` だけが Slack Web API を呼ぶ。他のサブコマンドはローカルの下書きストアしか
//! 触らないため、トークン未設定でも動かせるように `run()` はクライアントを `Option` で受ける。
//! 呼び出し側は [`DraftCommand::needs_client`] を見て組み立てるかどうかを決める。
//!
//! 保存フォーマットは TypeScript 版の `~/.slack-cli/drafts.json` と完全互換にする。
//! レコードのキー順（`channel`/`user` → `message` → `thread` → `id` → `createdAt`）が
//! そのまま JSON 出力に出るため、`serde_json` の `preserve_order` に頼って
//! [`serde_json::Map`] を挿入順で組み立てている。

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use clap::{Args, Subcommand};
use colored::Colorize;
use rand::Rng;
use serde_json::{json, Map, Value};

use crate::cli::common::{
    fetch_lookup_channels, find_channel_id, is_channel_id, is_message_ts, not_found_error,
};
use crate::cli::GlobalOpts;
use crate::client::pagination::PaginationOpts;
use crate::client::SlackClient;
use crate::config::CONFIG_DIR_NAME;
use crate::error::SlackCliError;
use crate::output::sanitize::sanitize_terminal_text;
use crate::output::{self, OutputFormat};

pub const ERR_NO_TARGET: &str = "Either --channel or --user must be specified";
pub const ERR_BOTH_TARGETS: &str = "Cannot specify both --channel and --user";
pub const ERR_INVALID_THREAD_TS: &str = "Invalid thread timestamp format";
pub const MSG_NO_DRAFTS: &str = "No drafts found";

const DRAFTS_FILE_NAME: &str = "drafts.json";
/// `channel_not_found` フォールバック（移植方針 G1）の起点になる Slack エラーコード。
const CHANNEL_NOT_FOUND: &str = "channel_not_found";
const USER_LOOKUP_PAGE_SIZE: u32 = 200;
/// dry-run では `conversations.open` が実行されないため、DM 先の代わりに置く印。
const DRY_RUN_CHANNEL: &str = "(dry-run)";
/// `draft list --format table` の本文列の切り詰め幅。
const MESSAGE_PREVIEW_CHARS: usize = 60;

#[derive(Args, Debug)]
pub struct DraftCommand {
    #[command(subcommand)]
    pub command: DraftSubcommand,
}

#[derive(Subcommand, Debug)]
pub enum DraftSubcommand {
    /// Save a message as a local draft
    Save {
        /// Target channel name or ID
        #[arg(short, long, value_name = "CHANNEL")]
        channel: Option<String>,

        /// Target user for DM
        #[arg(long, value_name = "USERNAME")]
        user: Option<String>,

        /// Message content
        #[arg(short, long, required = true, value_name = "MESSAGE")]
        message: String,

        /// Thread timestamp to reply to
        #[arg(short, long, value_name = "THREAD")]
        thread: Option<String>,
    },
    /// List saved drafts
    List,
    /// Show the full content of a draft
    Show {
        /// Draft ID
        #[arg(long, required = true, value_name = "DRAFT_ID")]
        id: String,
    },
    /// Send a saved draft
    Send {
        /// Draft ID
        #[arg(long, required = true, value_name = "DRAFT_ID")]
        id: String,

        /// Keep the draft after sending
        #[arg(long)]
        keep: bool,
    },
    /// Delete a saved draft
    Delete {
        /// Draft ID
        #[arg(long, required = true, value_name = "DRAFT_ID")]
        id: String,
    },
}

impl DraftCommand {
    /// Slack Web API クライアントが要るサブコマンドかどうか。
    pub fn needs_client(&self) -> bool {
        matches!(self.command, DraftSubcommand::Send { .. })
    }
}

pub async fn run(
    cmd: DraftCommand,
    client: Option<&SlackClient>,
    global: &GlobalOpts,
) -> Result<(), SlackCliError> {
    let store = DraftStore::new()?;
    let mut stdout = std::io::stdout();
    let result = run_with(cmd, &store, client, global, &mut stdout).await;
    stdout.flush()?;
    result
}

/// `run()` の本体。ストアと出力先を差し替えられるようにしてある。
async fn run_with(
    cmd: DraftCommand,
    store: &DraftStore,
    client: Option<&SlackClient>,
    global: &GlobalOpts,
    out: &mut dyn Write,
) -> Result<(), SlackCliError> {
    let format = global.output_format();

    match cmd.command {
        DraftSubcommand::Save {
            channel,
            user,
            message,
            thread,
        } => {
            // TypeScript 版は --thread の形式検証を preAction フックで先に走らせる
            validate_thread_ts(thread.as_deref())?;
            let draft = store.save(
                channel.as_deref(),
                user.as_deref(),
                &message,
                thread.as_deref(),
            )?;
            let message = format!(
                "✓ Draft saved (id: {}, target: {})",
                draft_field(&draft, "id").unwrap_or_default(),
                format_target(&draft)
            );
            finish(out, format, &message, &draft)
        }

        DraftSubcommand::List => {
            let drafts = store.read()?;
            if drafts.is_empty() {
                // 移植方針 G14: 機械可読フォーマットでは人間向けテキストを出さない
                if format == OutputFormat::Table {
                    writeln!(out, "{MSG_NO_DRAFTS}")?;
                    return Ok(());
                }
                return output::format_value(&json!([]), format, out);
            }

            let value = if format == OutputFormat::Table {
                Value::Array(drafts.iter().map(list_row).collect())
            } else {
                Value::Array(drafts)
            };
            output::format_value(&value, format, out)
        }

        DraftSubcommand::Show { id } => {
            let draft = store.get(&id)?.ok_or_else(|| not_found(&id))?;
            if format == OutputFormat::Table {
                write!(out, "{}", render_show(&draft))?;
                return Ok(());
            }
            output::format_value(&draft, format, out)
        }

        DraftSubcommand::Send { id, keep } => {
            let client = client.ok_or_else(|| {
                SlackCliError::Configuration("`draft send` requires an API client".to_string())
            })?;
            send_draft(store, client, &id, keep, format, out).await
        }

        DraftSubcommand::Delete { id } => {
            store.delete(&id)?;
            let message = format!("✓ Draft {} deleted", sanitize_terminal_text(&id));
            finish(out, format, &message, &json!({ "ok": true, "id": id }))
        }
    }
}

async fn send_draft(
    store: &DraftStore,
    client: &SlackClient,
    id: &str,
    keep: bool,
    format: OutputFormat,
    out: &mut dyn Write,
) -> Result<(), SlackCliError> {
    let draft = store.get(id)?.ok_or_else(|| not_found(id))?;
    let message = draft_field(&draft, "message")
        .unwrap_or_default()
        .to_string();

    let mut body = json!({ "text": message });
    if let Some(thread) = draft_field(&draft, "thread") {
        insert(&mut body, "thread_ts", Value::String(thread.to_string()));
    }

    // 宛先の決め方は TypeScript 版と同じで、--user が優先。
    // チャンネル宛のときだけ移植方針 G1 の名前解決フォールバックを通す。
    let response = match draft_field(&draft, "user") {
        Some(user) => {
            let name = user.strip_prefix('@').unwrap_or(user);
            let user_id = resolve_user_id_by_name(client, name).await?;
            let dm_channel = open_dm_channel(client, &user_id).await?;
            insert(&mut body, "channel", Value::String(dm_channel));
            client.post_json("chat.postMessage", &body).await?
        }
        None => {
            let channel = draft_field(&draft, "channel").ok_or_else(|| {
                SlackCliError::Validation(format!(
                    "Draft {} has neither a channel nor a user to send to",
                    sanitize_terminal_text(id)
                ))
            })?;
            insert(&mut body, "channel", Value::String(channel.to_string()));
            post_with_channel_fallback(client, &body, channel).await?
        }
    };

    let sent = format!("✓ Draft sent to {}", format_target(&draft));
    if format == OutputFormat::Table {
        writeln!(out, "{}", sanitize_terminal_text(&sent).green())?;
    } else {
        output::format_value(&response, format, out)?;
    }

    // dry-run では送信していないので、下書きは消さない。
    let dry_run = response.get("dry_run").and_then(Value::as_bool) == Some(true);
    if !keep && !dry_run {
        let note = match store.delete(id) {
            Ok(()) => format!("Draft {} deleted", sanitize_terminal_text(id)).normal(),
            // 送信自体は成功しているので、削除の失敗では終了コードを 1 にしない
            Err(_) => format!(
                "⚠ Message sent, but failed to delete draft {}",
                sanitize_terminal_text(id)
            )
            .yellow(),
        };
        if format == OutputFormat::Table {
            writeln!(out, "{note}")?;
        } else {
            eprintln!("{note}");
        }
    }

    Ok(())
}

/// 成功時の出力。`--format table`（既定）は TypeScript 版と同じ 1 行の成功メッセージ、
/// 機械可読フォーマットを明示されたときは対象の値をそのまま流す。
fn finish(
    out: &mut dyn Write,
    format: OutputFormat,
    message: &str,
    value: &Value,
) -> Result<(), SlackCliError> {
    if format == OutputFormat::Table {
        writeln!(out, "{}", sanitize_terminal_text(message).green())?;
        return Ok(());
    }
    output::format_value(value, format, out)
}

fn not_found(id: &str) -> SlackCliError {
    SlackCliError::Validation(format!("Draft not found: {}", sanitize_terminal_text(id)))
}

// ---------------------------------------------------------------------------
// 下書きストア
// ---------------------------------------------------------------------------

/// `~/.slack-cli/drafts.json` の読み書き。
///
/// TypeScript 版と同じく read-modify-write にファイルロックを掛けない
/// （移植方針 F5: 今回は修正せず、既知の制限として残す）。
pub struct DraftStore {
    dir: PathBuf,
}

impl DraftStore {
    pub fn new() -> Result<Self, SlackCliError> {
        let dir = dirs::home_dir()
            .ok_or_else(|| SlackCliError::Configuration("Cannot determine home directory".into()))?
            .join(CONFIG_DIR_NAME);
        Ok(Self { dir })
    }

    pub fn with_dir(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn path(&self) -> PathBuf {
        self.dir.join(DRAFTS_FILE_NAME)
    }

    /// 下書き一覧。ファイルが無ければ空。
    ///
    /// `id` と `message` が文字列である要素だけを残す寛容な読み込みは TypeScript 版のまま。
    /// パース失敗は空配列に倒さずエラーにする（移植方針 J6）。空に倒すと、次の書き込みで
    /// 既存の下書きが丸ごと消える。
    pub fn read(&self) -> Result<Vec<Value>, SlackCliError> {
        let path = self.path();
        let contents = match fs::read_to_string(&path) {
            Ok(contents) => contents,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e.into()),
        };

        let parsed: Value = serde_json::from_str(&contents).map_err(|_| {
            SlackCliError::File(format!("Invalid drafts file format: {}", path.display()))
        })?;

        let Value::Array(entries) = parsed else {
            return Ok(Vec::new());
        };

        Ok(entries.into_iter().filter(is_draft_like).collect())
    }

    pub fn get(&self, id: &str) -> Result<Option<Value>, SlackCliError> {
        Ok(self
            .read()?
            .into_iter()
            .find(|draft| draft_field(draft, "id") == Some(id)))
    }

    /// 下書きを 1 件追加して、追加したレコードを返す。
    pub fn save(
        &self,
        channel: Option<&str>,
        user: Option<&str>,
        message: &str,
        thread: Option<&str>,
    ) -> Result<Value, SlackCliError> {
        match (channel, user) {
            (None, None) => return Err(SlackCliError::Validation(ERR_NO_TARGET.to_string())),
            (Some(_), Some(_)) => {
                return Err(SlackCliError::Validation(ERR_BOTH_TARGETS.to_string()))
            }
            _ => {}
        }

        let mut drafts = self.read()?;

        // キー順は TypeScript 版の `{ ...input, id, createdAt }` に合わせる。
        // JSON 出力にそのまま出るので、挿入順を崩さないこと。
        let mut record = Map::new();
        if let Some(channel) = channel {
            record.insert("channel".into(), Value::String(channel.to_string()));
        }
        if let Some(user) = user {
            record.insert("user".into(), Value::String(user.to_string()));
        }
        record.insert("message".into(), Value::String(message.to_string()));
        if let Some(thread) = thread {
            record.insert("thread".into(), Value::String(thread.to_string()));
        }
        record.insert("id".into(), Value::String(generate_id(&drafts)));
        record.insert("createdAt".into(), Value::String(now_iso8601()));

        let draft = Value::Object(record);
        drafts.push(draft.clone());
        self.write(&drafts)?;
        Ok(draft)
    }

    pub fn delete(&self, id: &str) -> Result<(), SlackCliError> {
        let drafts = self.read()?;
        let remaining: Vec<Value> = drafts
            .iter()
            .filter(|draft| draft_field(draft, "id") != Some(id))
            .cloned()
            .collect();

        if remaining.len() == drafts.len() {
            return Err(not_found(id));
        }
        self.write(&remaining)
    }

    /// 一時ファイル + rename の原子的書き込み。ディレクトリ 0700 / ファイル 0600。
    fn write(&self, drafts: &[Value]) -> Result<(), SlackCliError> {
        create_private_dir(&self.dir)?;

        let path = self.path();
        let temp_path = path.with_extension(format!(
            "json.{}.{}.tmp",
            std::process::id(),
            epoch_millis()
        ));
        let contents = serde_json::to_string_pretty(&Value::Array(drafts.to_vec()))?;

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
            fs::rename(&temp_path, &path)?;
            Ok(())
        })();

        if result.is_err() {
            let _ = fs::remove_file(&temp_path);
        }
        result
    }
}

fn create_private_dir(dir: &Path) -> Result<(), SlackCliError> {
    fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// `id` と `message` が文字列のオブジェクトだけを下書きとして扱う。
fn is_draft_like(entry: &Value) -> bool {
    entry.is_object()
        && entry.get("id").and_then(Value::as_str).is_some()
        && entry.get("message").and_then(Value::as_str).is_some()
}

/// 4 バイト乱数の hex（8 桁）。既存 ID と衝突する限り引き直す。
fn generate_id(existing: &[Value]) -> String {
    loop {
        let mut bytes = [0u8; 4];
        rand::thread_rng().fill(&mut bytes);
        let id = hex::encode(bytes);
        if !existing
            .iter()
            .any(|draft| draft_field(draft, "id") == Some(id.as_str()))
        {
            return id;
        }
    }
}

fn now_iso8601() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

fn epoch_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or_default()
}

fn draft_field<'a>(draft: &'a Value, key: &str) -> Option<&'a str> {
    draft.get(key).and_then(Value::as_str)
}

// ---------------------------------------------------------------------------
// 表示
// ---------------------------------------------------------------------------

/// 宛先の表記。`#` / `@` が既に付いていれば足さず、チャンネル ID には `#` を付けない
/// （移植方針 G16）。
fn format_target(draft: &Value) -> String {
    if let Some(user) = draft_field(draft, "user") {
        let name = sanitize_terminal_text(user);
        let name = name.strip_prefix('@').unwrap_or(&name);
        return format!("@{name}");
    }

    let channel = sanitize_terminal_text(draft_field(draft, "channel").unwrap_or_default());
    if channel.starts_with('#') || is_channel_id(&channel) {
        channel
    } else {
        format!("#{channel}")
    }
}

/// `draft list --format table` の 1 行。本文は 60 文字で切り詰める。
fn list_row(draft: &Value) -> Value {
    let message = sanitize_terminal_text(draft_field(draft, "message").unwrap_or_default());
    json!({
        "id": sanitize_terminal_text(draft_field(draft, "id").unwrap_or_default()),
        "target": format_target(draft),
        "created_at": draft_field(draft, "createdAt").unwrap_or_default(),
        "message": truncate(&message, MESSAGE_PREVIEW_CHARS),
    })
}

/// `draft show` の固定書式。
fn render_show(draft: &Value) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "id: {}\n",
        sanitize_terminal_text(draft_field(draft, "id").unwrap_or_default())
    ));
    out.push_str(&format!("target: {}\n", format_target(draft)));
    if let Some(thread) = draft_field(draft, "thread") {
        out.push_str(&format!("thread: {}\n", sanitize_terminal_text(thread)));
    }
    out.push_str(&format!(
        "created_at: {}\n",
        sanitize_terminal_text(draft_field(draft, "createdAt").unwrap_or_default())
    ));
    out.push_str("---\n");
    out.push_str(&sanitize_terminal_text(
        draft_field(draft, "message").unwrap_or_default(),
    ));
    out.push('\n');
    out
}

/// 文字単位の切り詰め。バイト境界を割らないので絵文字を含んでもパニックしない
/// （移植方針 C2。表示幅で測るには unicode-width が要るが、依存が入っていない）。
fn truncate(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    let head: String = value.chars().take(max_chars).collect();
    format!("{head}...")
}

// ---------------------------------------------------------------------------
// Slack API
// ---------------------------------------------------------------------------

/// 移植方針 G1 のフォールバック送信。生値で 1 回送り、`channel_not_found` のときだけ
/// 名前解決して 1 度だけ再送する。先に解決を挟まないのは、`channels:read` の無い
/// トークンでの送信を壊さないため。
async fn post_with_channel_fallback(
    client: &SlackClient,
    body: &Value,
    raw_channel: &str,
) -> Result<Value, SlackCliError> {
    let error = match client.post_json("chat.postMessage", body).await {
        Ok(response) => return Ok(response),
        Err(error) => error,
    };

    let is_channel_not_found =
        matches!(&error, SlackCliError::Api { code, .. } if code == CHANNEL_NOT_FOUND);
    if !is_channel_not_found || is_channel_id(raw_channel) {
        return Err(error);
    }

    // 一覧が引けない（スコープ不足など）ときは、話をすり替えず元のエラーを返す。
    let Ok(channels) = fetch_lookup_channels(client).await else {
        return Err(error);
    };
    let Some(resolved) = find_channel_id(&channels, raw_channel) else {
        return Err(not_found_error(raw_channel, &channels));
    };

    let mut retried = body.clone();
    insert(&mut retried, "channel", Value::String(resolved));
    client.post_json("chat.postMessage", &retried).await
}

async fn resolve_user_id_by_name(
    client: &SlackClient,
    name: &str,
) -> Result<String, SlackCliError> {
    let wanted = name.to_lowercase();
    let members = client
        .paginate_get(
            "users.list",
            &[],
            "members",
            &PaginationOpts {
                page_size: Some(USER_LOOKUP_PAGE_SIZE),
                fetch_all: true,
                ..PaginationOpts::default()
            },
        )
        .await?;

    members
        .iter()
        .find(|member| {
            member
                .get("name")
                .and_then(Value::as_str)
                .is_some_and(|n| n.to_lowercase() == wanted)
        })
        .and_then(|member| member.get("id").and_then(Value::as_str))
        .map(str::to_string)
        .ok_or_else(|| {
            SlackCliError::Validation(format!("User '{}' not found", sanitize_terminal_text(name)))
        })
}

async fn open_dm_channel(client: &SlackClient, user_id: &str) -> Result<String, SlackCliError> {
    let response = client
        .post_json("conversations.open", &json!({ "users": user_id }))
        .await?;

    // dry-run では実際に開かれないため、以降の書き込みも送られない印を返す。
    if response.get("dry_run").and_then(Value::as_bool) == Some(true) {
        return Ok(DRY_RUN_CHANNEL.to_string());
    }

    response
        .get("channel")
        .and_then(|channel| channel.get("id"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| SlackCliError::Api {
            status: 200,
            code: "no_dm_channel".to_string(),
            needed: Vec::new(),
        })
}

/// `--thread` の形式検証（`^\d{10}\.\d{6}$`）。
fn validate_thread_ts(thread: Option<&str>) -> Result<(), SlackCliError> {
    match thread {
        Some(value) if !is_message_ts(value) => {
            Err(SlackCliError::Validation(ERR_INVALID_THREAD_TS.to_string()))
        }
        _ => Ok(()),
    }
}

fn insert(body: &mut Value, key: &str, value: Value) {
    if let Some(object) = body.as_object_mut() {
        object.insert(key.to_string(), value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::error::ErrorKind;
    use clap::Parser;
    use tempfile::TempDir;
    use url::Url;
    use wiremock::matchers::{body_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use crate::cli::Cli;

    fn parse(argv: &[&str]) -> DraftCommand {
        let cli = Cli::try_parse_from(argv).unwrap();
        let crate::cli::Command::Draft(cmd) = cli.command else {
            panic!("expected the draft command");
        };
        cmd
    }

    fn client_for(server: &MockServer) -> SlackClient {
        SlackClient::new("test-token-value")
            .unwrap()
            .with_base_url(Url::parse(&format!("{}/", server.uri())).unwrap())
    }

    fn store_in(dir: &TempDir) -> DraftStore {
        DraftStore::with_dir(dir.path())
    }

    fn table_opts() -> GlobalOpts {
        GlobalOpts::default()
    }

    fn json_opts() -> GlobalOpts {
        GlobalOpts {
            json: true,
            ..GlobalOpts::default()
        }
    }

    async fn exec(
        argv: &[&str],
        store: &DraftStore,
        client: Option<&SlackClient>,
        global: &GlobalOpts,
    ) -> Result<String, SlackCliError> {
        let mut buf = Vec::new();
        run_with(parse(argv), store, client, global, &mut buf).await?;
        Ok(String::from_utf8(buf).unwrap())
    }

    // -- 引数定義 ----------------------------------------------------------

    #[test]
    fn save_takes_every_flag_and_only_message_is_required() {
        // 移植方針 G11: --message は clap の required に寄せる
        // 移植方針 G12: --channel / --user の相互排他は run() 側の判定に残す
        let cmd = parse(&[
            "slack-cli",
            "draft",
            "save",
            "-c",
            "general",
            "--user",
            "alice",
            "-m",
            "hi",
            "-t",
            "1700000000.000100",
        ]);
        let DraftSubcommand::Save {
            channel,
            user,
            message,
            thread,
        } = cmd.command
        else {
            panic!("expected draft save");
        };
        assert_eq!(channel.as_deref(), Some("general"));
        assert_eq!(user.as_deref(), Some("alice"));
        assert_eq!(message, "hi");
        assert_eq!(thread.as_deref(), Some("1700000000.000100"));

        let err = Cli::try_parse_from(["slack-cli", "draft", "save", "-c", "general"])
            .expect_err("--message is required");
        assert_eq!(err.kind(), ErrorKind::MissingRequiredArgument);
    }

    #[test]
    fn only_send_needs_a_client() {
        assert!(parse(&["slack-cli", "draft", "send", "--id", "d1"]).needs_client());
        for argv in [
            vec!["slack-cli", "draft", "list"],
            vec!["slack-cli", "draft", "show", "--id", "d1"],
            vec!["slack-cli", "draft", "delete", "--id", "d1"],
            vec!["slack-cli", "draft", "save", "-m", "hi"],
        ] {
            assert!(
                !parse(&argv).needs_client(),
                "{argv:?} must not need a client"
            );
        }
    }

    #[test]
    fn keep_defaults_to_false() {
        let DraftSubcommand::Send { keep, .. } =
            parse(&["slack-cli", "draft", "send", "--id", "d1"]).command
        else {
            panic!("expected draft send");
        };
        assert!(!keep);
    }

    #[test]
    fn id_is_required_for_show_send_and_delete() {
        for sub in ["show", "send", "delete"] {
            let err =
                Cli::try_parse_from(["slack-cli", "draft", sub]).expect_err("--id is required");
            assert_eq!(err.kind(), ErrorKind::MissingRequiredArgument);
        }
    }

    // -- save --------------------------------------------------------------

    #[tokio::test]
    async fn save_writes_the_record_in_the_typescript_key_order() {
        let dir = TempDir::new().unwrap();
        let store = store_in(&dir);

        let out = exec(
            &[
                "slack-cli",
                "draft",
                "save",
                "-c",
                "general",
                "-m",
                "hello",
                "-t",
                "1700000000.000100",
            ],
            &store,
            None,
            &table_opts(),
        )
        .await
        .unwrap();
        assert!(out.contains("✓ Draft saved (id: "), "output was: {out}");
        assert!(out.contains("target: #general"), "output was: {out}");

        let raw = fs::read_to_string(store.path()).unwrap();
        let keys: Vec<&str> = raw
            .lines()
            .filter_map(|line| line.trim().strip_prefix('"'))
            .filter_map(|line| line.split('"').next())
            .collect();
        assert_eq!(
            keys,
            vec!["channel", "message", "thread", "id", "createdAt"]
        );

        let drafts = store.read().unwrap();
        assert_eq!(drafts.len(), 1);
        assert_eq!(draft_field(&drafts[0], "message"), Some("hello"));
        assert_eq!(draft_field(&drafts[0], "id").unwrap().len(), 8);
    }

    #[tokio::test]
    async fn save_rejects_missing_and_conflicting_targets() {
        let dir = TempDir::new().unwrap();
        let store = store_in(&dir);

        let err = exec(
            &["slack-cli", "draft", "save", "-m", "hi"],
            &store,
            None,
            &table_opts(),
        )
        .await
        .unwrap_err();
        assert_eq!(err.to_string(), ERR_NO_TARGET);

        let err = exec(
            &[
                "slack-cli",
                "draft",
                "save",
                "-c",
                "general",
                "--user",
                "alice",
                "-m",
                "hi",
            ],
            &store,
            None,
            &table_opts(),
        )
        .await
        .unwrap_err();
        assert_eq!(err.to_string(), ERR_BOTH_TARGETS);

        // 検証で落ちた場合はファイルを作らない
        assert!(!store.path().exists());
    }

    #[tokio::test]
    async fn save_rejects_a_malformed_thread_timestamp() {
        let dir = TempDir::new().unwrap();
        let err = exec(
            &[
                "slack-cli",
                "draft",
                "save",
                "-c",
                "general",
                "-m",
                "hi",
                "-t",
                "nope",
            ],
            &store_in(&dir),
            None,
            &table_opts(),
        )
        .await
        .unwrap_err();
        assert_eq!(err.to_string(), ERR_INVALID_THREAD_TS);
    }

    #[tokio::test]
    async fn save_emits_the_record_itself_in_machine_formats() {
        let dir = TempDir::new().unwrap();
        let out = exec(
            &["slack-cli", "draft", "save", "--user", "alice", "-m", "hi"],
            &store_in(&dir),
            None,
            &json_opts(),
        )
        .await
        .unwrap();

        let value: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(value["user"], "alice");
        assert_eq!(value["message"], "hi");
        assert!(value["id"].is_string());
    }

    #[test]
    fn generated_ids_never_collide_with_existing_ones() {
        let existing: Vec<Value> = (0..64)
            .map(|i| json!({ "id": format!("{i:08x}"), "message": "x" }))
            .collect();
        let id = generate_id(&existing);
        assert_eq!(id.len(), 8);
        assert!(!existing
            .iter()
            .any(|d| draft_field(d, "id") == Some(id.as_str())));
    }

    #[cfg(unix)]
    #[test]
    fn drafts_file_is_written_privately() {
        use std::os::unix::fs::PermissionsExt;

        let dir = TempDir::new().unwrap();
        let nested = dir.path().join("nested");
        let store = DraftStore::with_dir(&nested);
        store.save(Some("general"), None, "hi", None).unwrap();

        let file_mode = fs::metadata(store.path()).unwrap().permissions().mode() & 0o777;
        let dir_mode = fs::metadata(&nested).unwrap().permissions().mode() & 0o777;
        assert_eq!(file_mode, 0o600);
        assert_eq!(dir_mode, 0o700);
    }

    // -- list --------------------------------------------------------------

    #[tokio::test]
    async fn list_reports_an_empty_store_per_format() {
        let dir = TempDir::new().unwrap();
        let store = store_in(&dir);

        let table = exec(&["slack-cli", "draft", "list"], &store, None, &table_opts())
            .await
            .unwrap();
        assert_eq!(table.trim(), MSG_NO_DRAFTS);

        // 移植方針 G14: json では人間向けテキストではなく空配列
        let json_out = exec(&["slack-cli", "draft", "list"], &store, None, &json_opts())
            .await
            .unwrap();
        assert_eq!(serde_json::from_str::<Value>(&json_out).unwrap(), json!([]));
    }

    #[tokio::test]
    async fn list_json_keeps_the_stored_records_verbatim() {
        let dir = TempDir::new().unwrap();
        let store = store_in(&dir);
        store
            .save(None, Some("alice"), "hello world", None)
            .unwrap();

        let out = exec(&["slack-cli", "draft", "list"], &store, None, &json_opts())
            .await
            .unwrap();
        let value: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(value[0]["user"], "alice");
        assert_eq!(value[0]["message"], "hello world");
        // 表用の派生キーは混ぜない
        assert!(value[0].get("target").is_none());
    }

    #[tokio::test]
    async fn list_table_truncates_long_messages() {
        let dir = TempDir::new().unwrap();
        let store = store_in(&dir);
        let long = "あ".repeat(80);
        store.save(Some("general"), None, &long, None).unwrap();

        let drafts = store.read().unwrap();
        let row = list_row(&drafts[0]);
        assert_eq!(row["target"], "#general");
        let rendered = row["message"].as_str().unwrap();
        assert!(rendered.ends_with("..."));
        assert_eq!(rendered.chars().count(), MESSAGE_PREVIEW_CHARS + 3);

        let out = exec(&["slack-cli", "draft", "list"], &store, None, &table_opts())
            .await
            .unwrap();
        assert!(out.contains("target"), "table output was: {out}");
    }

    // -- show / delete -----------------------------------------------------

    #[tokio::test]
    async fn show_prints_the_fixed_layout_and_honours_json() {
        let dir = TempDir::new().unwrap();
        let store = store_in(&dir);
        let draft = store
            .save(Some("general"), None, "hello", Some("1700000000.000100"))
            .unwrap();
        let id = draft_field(&draft, "id").unwrap().to_string();

        let out = exec(
            &["slack-cli", "draft", "show", "--id", &id],
            &store,
            None,
            &table_opts(),
        )
        .await
        .unwrap();
        assert!(out.starts_with(&format!(
            "id: {id}\ntarget: #general\nthread: 1700000000.000100\n"
        )));
        assert!(out.ends_with("---\nhello\n"), "output was: {out}");

        let json_out = exec(
            &["slack-cli", "draft", "show", "--id", &id],
            &store,
            None,
            &json_opts(),
        )
        .await
        .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&json_out).unwrap()["message"],
            "hello"
        );
    }

    #[tokio::test]
    async fn show_and_delete_report_a_missing_draft() {
        let dir = TempDir::new().unwrap();
        let store = store_in(&dir);

        for sub in ["show", "delete"] {
            let err = exec(
                &["slack-cli", "draft", sub, "--id", "deadbeef"],
                &store,
                None,
                &table_opts(),
            )
            .await
            .unwrap_err();
            assert_eq!(err.to_string(), "Draft not found: deadbeef");
        }
    }

    #[tokio::test]
    async fn delete_removes_only_the_requested_draft() {
        let dir = TempDir::new().unwrap();
        let store = store_in(&dir);
        let kept = store.save(Some("general"), None, "keep", None).unwrap();
        let dropped = store.save(Some("random"), None, "drop", None).unwrap();
        let dropped_id = draft_field(&dropped, "id").unwrap().to_string();

        let out = exec(
            &["slack-cli", "draft", "delete", "--id", &dropped_id],
            &store,
            None,
            &table_opts(),
        )
        .await
        .unwrap();
        assert!(out.contains(&format!("✓ Draft {dropped_id} deleted")));

        let remaining = store.read().unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(draft_field(&remaining[0], "id"), draft_field(&kept, "id"));
    }

    // -- ストアの読み込み --------------------------------------------------

    #[test]
    fn reading_tolerates_malformed_entries_but_not_malformed_json() {
        let dir = TempDir::new().unwrap();
        let store = store_in(&dir);
        create_private_dir(dir.path()).unwrap();

        fs::write(
            store.path(),
            r#"[{"id":"a1","message":"ok"},{"id":42,"message":"no id"},{"id":"b2"},"nope",null]"#,
        )
        .unwrap();
        let drafts = store.read().unwrap();
        assert_eq!(drafts.len(), 1);
        assert_eq!(draft_field(&drafts[0], "id"), Some("a1"));

        // 配列でない JSON は TypeScript 版と同じく空扱い
        fs::write(store.path(), "{}").unwrap();
        assert!(store.read().unwrap().is_empty());

        // 移植方針 J6: パース不能なファイルは空に倒さずエラー
        fs::write(store.path(), "{ not json").unwrap();
        let err = store.read().unwrap_err();
        assert!(
            err.to_string().starts_with("Invalid drafts file format:"),
            "error was: {err}"
        );
    }

    #[test]
    fn reading_a_missing_file_yields_no_drafts() {
        let dir = TempDir::new().unwrap();
        assert!(store_in(&dir).read().unwrap().is_empty());
    }

    // -- send --------------------------------------------------------------

    async fn mount_post_message(server: &MockServer, channel: &str) {
        Mock::given(method("POST"))
            .and(path("/chat.postMessage"))
            .and(body_json(json!({ "text": "hello", "channel": channel })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "channel": channel,
                "ts": "1700000000.000200",
            })))
            .mount(server)
            .await;
    }

    #[tokio::test]
    async fn send_posts_to_the_stored_channel_and_deletes_the_draft() {
        let server = MockServer::start().await;
        mount_post_message(&server, "general").await;
        let client = client_for(&server);

        let dir = TempDir::new().unwrap();
        let store = store_in(&dir);
        let draft = store.save(Some("general"), None, "hello", None).unwrap();
        let id = draft_field(&draft, "id").unwrap().to_string();

        let out = exec(
            &["slack-cli", "draft", "send", "--id", &id],
            &store,
            Some(&client),
            &table_opts(),
        )
        .await
        .unwrap();

        assert!(
            out.contains("✓ Draft sent to #general"),
            "output was: {out}"
        );
        assert!(
            out.contains(&format!("Draft {id} deleted")),
            "output was: {out}"
        );
        assert!(store.read().unwrap().is_empty());
    }

    #[tokio::test]
    async fn send_keeps_the_draft_with_keep_and_passes_the_thread_ts() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat.postMessage"))
            .and(body_json(json!({
                "text": "hello",
                "thread_ts": "1700000000.000100",
                "channel": "C012345678",
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "ok": true })))
            .expect(1)
            .mount(&server)
            .await;
        let client = client_for(&server);

        let dir = TempDir::new().unwrap();
        let store = store_in(&dir);
        let draft = store
            .save(Some("C012345678"), None, "hello", Some("1700000000.000100"))
            .unwrap();
        let id = draft_field(&draft, "id").unwrap().to_string();

        let out = exec(
            &["slack-cli", "draft", "send", "--id", &id, "--keep"],
            &store,
            Some(&client),
            &table_opts(),
        )
        .await
        .unwrap();

        // チャンネル ID には # を付けない（移植方針 G16）
        assert!(
            out.contains("✓ Draft sent to C012345678"),
            "output was: {out}"
        );
        assert!(!out.contains("deleted"), "output was: {out}");
        assert_eq!(store.read().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn send_resolves_the_channel_name_after_channel_not_found() {
        // 移植方針 G1: まず生値で送り、channel_not_found のときだけ名前解決して 1 度だけ再送する
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat.postMessage"))
            .and(body_json(json!({ "text": "hello", "channel": "開発" })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": false,
                "error": "channel_not_found",
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/conversations.list"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "channels": [{ "id": "C012345678", "name": "開発" }],
            })))
            .mount(&server)
            .await;
        mount_post_message(&server, "C012345678").await;
        let client = client_for(&server);

        let dir = TempDir::new().unwrap();
        let store = store_in(&dir);
        let draft = store.save(Some("開発"), None, "hello", None).unwrap();
        let id = draft_field(&draft, "id").unwrap().to_string();

        let out = exec(
            &["slack-cli", "draft", "send", "--id", &id],
            &store,
            Some(&client),
            &table_opts(),
        )
        .await
        .unwrap();
        assert!(out.contains("✓ Draft sent to #開発"), "output was: {out}");
        assert!(store.read().unwrap().is_empty());
    }

    #[tokio::test]
    async fn send_reports_the_original_error_when_the_name_cannot_be_resolved() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat.postMessage"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": false,
                "error": "channel_not_found",
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/conversations.list"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "channels": [{ "id": "C012345678", "name": "general-archive" }],
            })))
            .mount(&server)
            .await;
        let client = client_for(&server);

        let dir = TempDir::new().unwrap();
        let store = store_in(&dir);
        let draft = store.save(Some("general"), None, "hello", None).unwrap();
        let id = draft_field(&draft, "id").unwrap().to_string();

        let err = exec(
            &["slack-cli", "draft", "send", "--id", &id],
            &store,
            Some(&client),
            &table_opts(),
        )
        .await
        .unwrap_err();
        assert_eq!(
            err.to_string(),
            "Channel 'general' not found. Did you mean one of these? general-archive"
        );
        // 送れなかった下書きは残す
        assert_eq!(store.read().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn send_opens_a_dm_for_a_user_draft() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/users.list"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "members": [
                    { "id": "U000000001", "name": "bob" },
                    { "id": "U000000002", "name": "alice" },
                ],
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/conversations.open"))
            .and(body_json(json!({ "users": "U000000002" })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "channel": { "id": "D012345678" },
            })))
            .expect(1)
            .mount(&server)
            .await;
        mount_post_message(&server, "D012345678").await;
        let client = client_for(&server);

        let dir = TempDir::new().unwrap();
        let store = store_in(&dir);
        // 先頭の @ は宛先解決でも表示でも一度だけ扱う
        let draft = store.save(None, Some("@alice"), "hello", None).unwrap();
        let id = draft_field(&draft, "id").unwrap().to_string();

        let out = exec(
            &["slack-cli", "draft", "send", "--id", &id],
            &store,
            Some(&client),
            &table_opts(),
        )
        .await
        .unwrap();
        assert!(out.contains("✓ Draft sent to @alice"), "output was: {out}");
    }

    #[tokio::test]
    async fn send_fails_when_the_user_cannot_be_found() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/users.list"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "members": [{ "id": "U000000001", "name": "bob" }],
            })))
            .mount(&server)
            .await;
        let client = client_for(&server);

        let dir = TempDir::new().unwrap();
        let store = store_in(&dir);
        let draft = store.save(None, Some("alice"), "hello", None).unwrap();
        let id = draft_field(&draft, "id").unwrap().to_string();

        let err = exec(
            &["slack-cli", "draft", "send", "--id", &id],
            &store,
            Some(&client),
            &table_opts(),
        )
        .await
        .unwrap_err();
        assert_eq!(err.to_string(), "User 'alice' not found");
        assert_eq!(store.read().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn send_surfaces_api_errors_and_keeps_the_draft() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat.postMessage"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": false,
                "error": "missing_scope",
                "needed": "chat:write",
            })))
            .mount(&server)
            .await;
        let client = client_for(&server);

        let dir = TempDir::new().unwrap();
        let store = store_in(&dir);
        let draft = store.save(Some("C012345678"), None, "hello", None).unwrap();
        let id = draft_field(&draft, "id").unwrap().to_string();

        let err = exec(
            &["slack-cli", "draft", "send", "--id", &id],
            &store,
            Some(&client),
            &table_opts(),
        )
        .await
        .unwrap_err();
        assert_eq!(
            err.to_string(),
            "API Error: missing_scope (needed: chat:write)"
        );
        assert_eq!(store.read().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn send_in_json_format_emits_the_api_response() {
        let server = MockServer::start().await;
        mount_post_message(&server, "C012345678").await;
        let client = client_for(&server);

        let dir = TempDir::new().unwrap();
        let store = store_in(&dir);
        let draft = store.save(Some("C012345678"), None, "hello", None).unwrap();
        let id = draft_field(&draft, "id").unwrap().to_string();

        let out = exec(
            &["slack-cli", "draft", "send", "--id", &id],
            &store,
            Some(&client),
            &json_opts(),
        )
        .await
        .unwrap();
        let value: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(value["ts"], "1700000000.000200");
    }

    #[tokio::test]
    async fn dry_run_send_neither_posts_nor_deletes() {
        let server = MockServer::start().await;
        let client = client_for(&server).with_dry_run(true);

        let dir = TempDir::new().unwrap();
        let store = store_in(&dir);
        let draft = store.save(Some("general"), None, "hello", None).unwrap();
        let id = draft_field(&draft, "id").unwrap().to_string();

        exec(
            &["slack-cli", "draft", "send", "--id", &id],
            &store,
            Some(&client),
            &table_opts(),
        )
        .await
        .unwrap();

        assert!(server.received_requests().await.unwrap().is_empty());
        assert_eq!(store.read().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn send_rejects_a_draft_without_any_target() {
        let dir = TempDir::new().unwrap();
        let store = store_in(&dir);
        create_private_dir(dir.path()).unwrap();
        fs::write(store.path(), r#"[{"id":"a1","message":"orphan"}]"#).unwrap();

        let server = MockServer::start().await;
        let client = client_for(&server);
        let err = exec(
            &["slack-cli", "draft", "send", "--id", "a1"],
            &store,
            Some(&client),
            &table_opts(),
        )
        .await
        .unwrap_err();
        assert!(
            err.to_string().contains("neither a channel nor a user"),
            "error was: {err}"
        );
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    // -- 表示ヘルパ --------------------------------------------------------

    #[test]
    fn targets_never_double_the_sigil() {
        // 移植方針 G16
        assert_eq!(format_target(&json!({ "channel": "general" })), "#general");
        assert_eq!(format_target(&json!({ "channel": "#general" })), "#general");
        assert_eq!(
            format_target(&json!({ "channel": "C012345678" })),
            "C012345678"
        );
        assert_eq!(format_target(&json!({ "user": "alice" })), "@alice");
        assert_eq!(format_target(&json!({ "user": "@alice" })), "@alice");
    }

    #[test]
    fn sanitizer_strips_escape_sequences_but_keeps_tabs_and_newlines() {
        assert_eq!(sanitize_terminal_text("a\u{1b}]0;title\u{7}b"), "ab");
        assert_eq!(sanitize_terminal_text("a\u{1b}[31mred\u{1b}[0m"), "ared");
        assert_eq!(sanitize_terminal_text("a\tb\nc"), "a\tb\nc");
        assert_eq!(sanitize_terminal_text("a\u{0}\u{7f}\u{9b}b"), "ab");
        assert_eq!(sanitize_terminal_text("日本語 🎉"), "日本語 🎉");
        // 端末制御を仕込んだチャンネル名は表示前に落ちる
        assert_eq!(
            format_target(&json!({ "channel": "gen\u{1b}[2Jeral" })),
            "#general"
        );
    }

    #[test]
    fn truncation_never_splits_a_character() {
        assert_eq!(truncate("abc", 5), "abc");
        assert_eq!(truncate("🎉🎉🎉", 2), "🎉🎉...");
        assert_eq!(truncate("あいうえお", 5), "あいうえお");
    }

    #[test]
    fn message_timestamps_need_ten_dot_six_digits() {
        assert!(is_message_ts("1700000000.000100"));
        for bad in [
            "1700000000",
            "170000000.000100",
            "1700000000.00010",
            "a.b",
            "",
        ] {
            assert!(!is_message_ts(bad), "{bad:?} should be rejected");
        }
    }

    #[test]
    fn channel_ids_need_at_least_nine_characters() {
        assert!(is_channel_id("C12345678"));
        assert!(is_channel_id("D012345678"));
        assert!(!is_channel_id("C1234567"));
        assert!(!is_channel_id("general"));
        assert!(!is_channel_id(""));
    }
}
