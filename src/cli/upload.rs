//! `slack-cli upload` — ファイル / スニペットのアップロード。
//!
//! Node SDK の `files.uploadV2` は Web API のメソッドではなく 3 段合成のヘルパなので、
//! ここでは `files.getUploadURLExternal` → 実データの multipart POST →
//! `files.completeUploadExternal` を自前で撃つ。
//! 2 段目のフィールド名 `body` / パートのファイル名 `Untitled` / パート MIME
//! `application/octet-stream` は SDK が偶発的に決めている既定値で、Slack はファイル種別を
//! 1 段目の `filename` の拡張子だけで判定する。


use std::path::Path;

use clap::Args;
use colored::Colorize;
use serde_json::{json, Value};

use crate::cli::common::{channel_label as display_channel, resolve_channel_id};
use crate::cli::GlobalOpts;
use crate::client::SlackClient;
use crate::error::SlackCliError;
use crate::output;

pub const ERR_NO_SOURCE: &str = "You must specify either --file or --content";
pub const ERR_BOTH_SOURCES: &str = "Cannot use both --file and --content";
pub const ERR_INVALID_THREAD: &str = "Invalid thread timestamp format";

/// `--content` かつ `--filename` 未指定のときの既定ファイル名。
/// SDK の `file.${filetype ?? 'txt'}` は `filetype` が常に未設定なので必ずこの値になる。
const DEFAULT_CONTENT_FILENAME: &str = "file.txt";

const UPLOAD_FIELD_NAME: &str = "body";
const UPLOAD_PART_FILENAME: &str = "Untitled";
const UPLOAD_PART_MIME: &str = "application/octet-stream";

const GET_UPLOAD_URL_METHOD: &str = "files.getUploadURLExternal";
const COMPLETE_UPLOAD_METHOD: &str = "files.completeUploadExternal";

/// 出力に載せるファイル情報のキー。Slack のレスポンスにある他のキーは落とす。
const FILE_KEYS: [&str; 6] = [
    "id",
    "name",
    "title",
    "permalink",
    "permalink_public",
    "url_private",
];

#[derive(Args, Debug)]
pub struct UploadCommand {
    /// Channel name or ID
    #[arg(short, long, required = true, value_name = "CHANNEL")]
    pub channel: String,

    /// File path to upload
    #[arg(short, long, value_name = "FILE")]
    pub file: Option<String>,

    /// Text content to upload as snippet
    #[arg(long, value_name = "CONTENT")]
    pub content: Option<String>,

    /// Override filename
    #[arg(long, value_name = "FILENAME")]
    pub filename: Option<String>,

    /// File title
    #[arg(long, value_name = "TITLE")]
    pub title: Option<String>,

    /// Initial comment with the file
    #[arg(short, long, value_name = "MESSAGE")]
    pub message: Option<String>,

    /// Snippet type (e.g. python, javascript, csv)
    #[arg(long, value_name = "FILETYPE")]
    pub filetype: Option<String>,

    /// Thread timestamp to upload as reply
    #[arg(short, long, value_name = "THREAD")]
    pub thread: Option<String>,
}

pub async fn run(
    cmd: UploadCommand,
    client: &SlackClient,
    global: &GlobalOpts,
) -> Result<(), SlackCliError> {
    validate_thread_ts(cmd.thread.as_deref())?;
    let (data, filename) = load_payload(&cmd)?;
    let title = cmd.title.clone().unwrap_or_else(|| filename.clone());

    let channel_id = resolve_channel_id(client, &cmd.channel).await?;

    let files = if client.dry_run {
        // 実データ POST は本体クライアントのリトライ層を通らないので、
        // dry-run はここで打ち切って 3 段とも撃たない。
        eprintln!(
            "[dry-run] would upload {} bytes as \"{filename}\" to {channel_id}",
            data.len()
        );
        Vec::new()
    } else {
        let (upload_url, file_id) = get_upload_url(
            client,
            &filename,
            data.len() as u64,
            cmd.filetype.as_deref(),
        )
        .await?;

        post_file_data(client, &upload_url, data).await?;

        let response = complete_upload(
            client,
            &CompleteUpload {
                file_id: &file_id,
                title: &title,
                channel_id: &channel_id,
                thread_ts: cmd.thread.as_deref(),
                initial_comment: cmd.message.as_deref(),
            },
        )
        .await?;
        collect_files(&response)
    };

    eprintln!(
        "{}",
        format!(
            "✓ File uploaded successfully to {}",
            display_channel(&cmd.channel)
        )
        .green()
    );

    let value = json!({ "channel": cmd.channel, "files": files });
    output::format_value(&value, global.output_format(), &mut std::io::stdout())?;
    Ok(())
}

/// `--file` / `--content` の排他を検証し、本文と送信ファイル名を確定させる。
///
/// `--file` はファイル名が basename にフォールバックするが、`--content` は
/// `--filename` 未指定なら常に `file.txt` になる（TS 版の非対称をそのまま維持）。
fn load_payload(cmd: &UploadCommand) -> Result<(Vec<u8>, String), SlackCliError> {
    match (cmd.file.as_deref(), cmd.content.as_deref()) {
        (Some(_), Some(_)) => Err(SlackCliError::Validation(ERR_BOTH_SOURCES.to_string())),
        (None, None) => Err(SlackCliError::Validation(ERR_NO_SOURCE.to_string())),
        (Some(path), None) => {
            let data = std::fs::read(path)
                .map_err(|_| SlackCliError::File(format!("File not found: {path}")))?;
            let name = cmd.filename.clone().unwrap_or_else(|| basename(path));
            Ok((data, name))
        }
        (None, Some(content)) => {
            let name = cmd
                .filename
                .clone()
                .unwrap_or_else(|| DEFAULT_CONTENT_FILENAME.to_string());
            Ok((content.as_bytes().to_vec(), name))
        }
    }
}

fn basename(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string())
}

/// `^\d{10}\.\d{6}$` 相当。正規表現クレートを足さずに桁だけ数える。
fn validate_thread_ts(thread: Option<&str>) -> Result<(), SlackCliError> {
    let Some(ts) = thread else { return Ok(()) };

    let valid = match ts.split_once('.') {
        Some((seconds, micros)) => {
            seconds.len() == 10
                && micros.len() == 6
                && seconds.bytes().all(|b| b.is_ascii_digit())
                && micros.bytes().all(|b| b.is_ascii_digit())
        }
        None => false,
    };

    if valid {
        Ok(())
    } else {
        Err(SlackCliError::Validation(ERR_INVALID_THREAD.to_string()))
    }
}

/// 1 段目。`channel_id` / `title` / `initial_comment` はここには渡らない。
async fn get_upload_url(
    client: &SlackClient,
    filename: &str,
    length: u64,
    snippet_type: Option<&str>,
) -> Result<(String, String), SlackCliError> {
    let length = length.to_string();
    let mut params: Vec<(&str, &str)> = vec![("filename", filename), ("length", &length)];
    if let Some(snippet_type) = snippet_type {
        params.push(("snippet_type", snippet_type));
    }

    let response = client.post_form(GET_UPLOAD_URL_METHOD, &params).await?;
    let upload_url = required_str(&response, "upload_url")?;
    let file_id = required_str(&response, "file_id")?;
    Ok((upload_url, file_id))
}

fn required_str(response: &Value, key: &str) -> Result<String, SlackCliError> {
    response
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| {
            SlackCliError::Api {
                status: 200,
                code: format!("missing_{key}"),
                needed: Vec::new(),
            }
        })
}

/// 2 段目。`upload_url` はサーバ由来の絶対 URL なので、Bearer を載せる前にホストを確認する。
/// 本体クライアントはリダイレクト禁止なので、TS 版の `maxRedirects: 0` と同じく 3xx は失敗になる。
async fn post_file_data(
    client: &SlackClient,
    upload_url: &str,
    data: Vec<u8>,
) -> Result<(), SlackCliError> {
    let url = url::Url::parse(upload_url).map_err(|e| {
        SlackCliError::Api {
            status: 200,
            code: format!("invalid_upload_url: {e}"),
            needed: Vec::new(),
        }
    })?;

    if !is_trusted_upload_url(&url, &client.base_url) {
        return Err(SlackCliError::Validation(format!(
            "Refusing to upload to a non-Slack host: {}",
            url.host_str().unwrap_or("<none>")
        )));
    }

    let part = reqwest::multipart::Part::bytes(data)
        .file_name(UPLOAD_PART_FILENAME)
        .mime_str(UPLOAD_PART_MIME)
        .map_err(|e| SlackCliError::Configuration(format!("Invalid upload part MIME: {e}")))?;
    let form = reqwest::multipart::Form::new().part(UPLOAD_FIELD_NAME, part);

    let response = client
        .http
        .post(url)
        .headers(client.auth_headers())
        .multipart(form)
        .send()
        .await?;

    if response.status() != reqwest::StatusCode::OK {
        return Err(SlackCliError::File(format!(
            "Failed to upload file (status: {})",
            response.status().as_u16()
        )));
    }
    Ok(())
}

/// Slack のアップロード先ドメイン、または設定済み API オリジンと同一のときだけ Bearer を付ける。
fn is_trusted_upload_url(url: &url::Url, base_url: &url::Url) -> bool {
    if url.origin() == base_url.origin() {
        return true;
    }
    if url.scheme() != "https" {
        return false;
    }
    matches!(url.host_str(), Some(host) if host == "slack.com" || host.ends_with(".slack.com"))
}

struct CompleteUpload<'a> {
    file_id: &'a str,
    title: &'a str,
    channel_id: &'a str,
    thread_ts: Option<&'a str>,
    initial_comment: Option<&'a str>,
}

/// 3 段目。`files` は JSON 文字列を urlencoded の 1 フィールドに載せる形。
async fn complete_upload(
    client: &SlackClient,
    opts: &CompleteUpload<'_>,
) -> Result<Value, SlackCliError> {
    let files_json =
        serde_json::to_string(&json!([{ "id": opts.file_id, "title": opts.title }]))?;

    let mut params: Vec<(&str, &str)> = vec![
        ("files", &files_json),
        ("channel_id", opts.channel_id),
    ];
    if let Some(thread_ts) = opts.thread_ts {
        params.push(("thread_ts", thread_ts));
    }
    if let Some(comment) = opts.initial_comment {
        params.push(("initial_comment", comment));
    }

    client.post_form(COMPLETE_UPLOAD_METHOD, &params).await
}

fn collect_files(response: &Value) -> Vec<Value> {
    response
        .get("files")
        .and_then(Value::as_array)
        .map(|files| files.iter().map(pick_file_fields).collect())
        .unwrap_or_default()
}

fn pick_file_fields(file: &Value) -> Value {
    let mut picked = serde_json::Map::new();
    for key in FILE_KEYS {
        match file.get(key) {
            Some(value) if !value.is_null() => {
                picked.insert(key.to_string(), value.clone());
            }
            _ => {}
        }
    }
    Value::Object(picked)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::error::ErrorKind;
    use clap::Parser;
    use url::Url;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, Request, ResponseTemplate};

    use crate::cli::Cli;
    use crate::output::OutputFormat;

    fn client_for(server: &MockServer) -> SlackClient {
        SlackClient::new("test-token-value")
            .unwrap()
            .with_base_url(Url::parse(&format!("{}/", server.uri())).unwrap())
    }

    fn command(channel: &str) -> UploadCommand {
        UploadCommand {
            channel: channel.to_string(),
            file: None,
            content: None,
            filename: None,
            title: None,
            message: None,
            filetype: None,
            thread: None,
        }
    }

    fn json_opts() -> GlobalOpts {
        GlobalOpts {
            json: true,
            ..GlobalOpts::default()
        }
    }

    /// 3 段のうち 1 段目と 3 段目をモックし、2 段目は upload_url をモックサーバに向ける。
    async fn mount_upload_flow(server: &MockServer) {
        let upload_url = format!("{}/upload/v1/abc123", server.uri());
        Mock::given(method("POST"))
            .and(path("/files.getUploadURLExternal"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "upload_url": upload_url,
                "file_id": "F0123456789",
            })))
            .mount(server)
            .await;

        Mock::given(method("POST"))
            .and(path("/upload/v1/abc123"))
            .respond_with(ResponseTemplate::new(200).set_body_string("OK - 12"))
            .mount(server)
            .await;

        Mock::given(method("POST"))
            .and(path("/files.completeUploadExternal"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "files": [{
                    "id": "F0123456789",
                    "name": "report.csv",
                    "title": "report.csv",
                    "permalink": "https://example.slack.com/files/U01/F0123456789/report.csv",
                    "url_private": "https://files.slack.com/files-pri/T1-F1/report.csv",
                    "mimetype": "text/csv",
                    "size": 12,
                }],
            })))
            .mount(server)
            .await;
    }

    fn body_of<'a>(requests: &'a [Request], suffix: &str) -> &'a str {
        let request = requests
            .iter()
            .find(|r| r.url.path().ends_with(suffix))
            .unwrap_or_else(|| panic!("no request to {suffix}"));
        std::str::from_utf8(&request.body).unwrap()
    }

    #[test]
    fn parses_every_flag() {
        let cli = Cli::try_parse_from([
            "slack-cli",
            "upload",
            "-c",
            "general",
            "-f",
            "notes.md",
            "--content",
            "print(1)",
            "--filename",
            "snippet.py",
            "--title",
            "Snippet",
            "-m",
            "here you go",
            "--filetype",
            "python",
            "-t",
            "1700000000.000100",
        ])
        .unwrap();
        let crate::cli::Command::Upload(cmd) = cli.command else {
            panic!("expected the upload command");
        };
        assert_eq!(cmd.filetype.as_deref(), Some("python"));
        assert_eq!(cmd.filename.as_deref(), Some("snippet.py"));
    }

    #[test]
    fn channel_is_the_only_required_flag() {
        Cli::try_parse_from(["slack-cli", "upload", "-c", "general"]).unwrap();
        let err = Cli::try_parse_from(["slack-cli", "upload", "-f", "notes.md"])
            .expect_err("--channel is required");
        assert_eq!(err.kind(), ErrorKind::MissingRequiredArgument);
    }

    #[test]
    fn file_and_content_are_mutually_exclusive_and_one_is_required() {
        let mut cmd = command("general");
        assert_eq!(load_payload(&cmd).unwrap_err().to_string(), ERR_NO_SOURCE);

        cmd.file = Some("notes.md".into());
        cmd.content = Some("hi".into());
        assert_eq!(load_payload(&cmd).unwrap_err().to_string(), ERR_BOTH_SOURCES);
    }

    #[test]
    fn missing_files_report_the_path() {
        let mut cmd = command("general");
        cmd.file = Some("/nonexistent/path/report.csv".into());
        let err = load_payload(&cmd).unwrap_err();
        assert_eq!(err.to_string(), "File not found: /nonexistent/path/report.csv");
        assert_eq!(err.code(), Some(crate::error::CODE_FILE));
    }

    #[test]
    fn content_without_filename_defaults_to_file_txt() {
        let mut cmd = command("general");
        cmd.content = Some("hello".into());
        let (data, filename) = load_payload(&cmd).unwrap();
        assert_eq!(data, b"hello");
        assert_eq!(filename, DEFAULT_CONTENT_FILENAME);

        cmd.filename = Some("snippet.py".into());
        assert_eq!(load_payload(&cmd).unwrap().1, "snippet.py");
    }

    #[test]
    fn file_without_filename_defaults_to_the_basename() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("report.csv");
        std::fs::write(&path, b"a,b\n1,2\n").unwrap();

        let mut cmd = command("general");
        cmd.file = Some(path.to_string_lossy().into_owned());
        assert_eq!(load_payload(&cmd).unwrap().1, "report.csv");
    }

    #[test]
    fn thread_timestamps_must_be_ten_dot_six_digits() {
        assert!(validate_thread_ts(None).is_ok());
        assert!(validate_thread_ts(Some("1700000000.000100")).is_ok());
        for invalid in ["1700000000", "170000000.000100", "1700000000.00010", "abcdefghij.000100", "1700000000.0001a0"] {
            assert_eq!(
                validate_thread_ts(Some(invalid)).unwrap_err().to_string(),
                ERR_INVALID_THREAD,
                "{invalid:?} should have been rejected"
            );
        }
    }
    #[test]
    fn only_the_six_documented_file_keys_survive() {
        let picked = pick_file_fields(&json!({
            "id": "F1",
            "name": "a.csv",
            "mimetype": "text/csv",
            "permalink_public": null,
        }));
        assert_eq!(picked["id"], "F1");
        assert_eq!(picked["name"], "a.csv");
        assert!(picked.get("mimetype").is_none());
        assert!(picked.get("permalink_public").is_none());
    }

    #[test]
    fn upload_urls_outside_slack_are_rejected() {
        let base = Url::parse("https://slack.com/api/").unwrap();
        assert!(is_trusted_upload_url(
            &Url::parse("https://files.slack.com/upload/v1/x").unwrap(),
            &base
        ));
        assert!(!is_trusted_upload_url(
            &Url::parse("https://attacker.example/upload").unwrap(),
            &base
        ));
        assert!(!is_trusted_upload_url(
            &Url::parse("http://files.slack.com/upload").unwrap(),
            &base
        ));
    }
    #[tokio::test]
    async fn uploads_a_snippet_through_all_three_steps() {
        let server = MockServer::start().await;
        mount_upload_flow(&server).await;

        let mut cmd = command("C0123456789");
        cmd.content = Some("a,b\n1,2\n".into());
        cmd.filetype = Some("csv".into());
        cmd.message = Some("here you go".into());
        cmd.thread = Some("1700000000.000100".into());

        run(cmd, &client_for(&server), &json_opts()).await.unwrap();

        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 3, "expected exactly the three upload steps");

        // 1 段目: filename / length / snippet_type だけ。channel_id や title は載せない
        let step1 = body_of(&requests, "files.getUploadURLExternal");
        assert!(step1.contains("filename=file.txt"), "step1 was: {step1}");
        assert!(step1.contains("length=8"), "step1 was: {step1}");
        assert!(step1.contains("snippet_type=csv"), "step1 was: {step1}");
        assert!(!step1.contains("channel_id"), "step1 was: {step1}");

        // 2 段目: フィールド名 body / ファイル名 Untitled / パート MIME は octet-stream
        let step2 = body_of(&requests, "/upload/v1/abc123");
        assert!(step2.contains("name=\"body\""), "step2 was: {step2}");
        assert!(step2.contains("filename=\"Untitled\""), "step2 was: {step2}");
        assert!(
            step2.contains("application/octet-stream"),
            "step2 was: {step2}"
        );
        assert!(step2.contains("a,b"), "step2 was: {step2}");

        // 3 段目: files は JSON 文字列を 1 フィールドに載せた形
        let step3 = body_of(&requests, "files.completeUploadExternal");
        let decoded: String = form_urlencoded_value(step3, "files");
        assert_eq!(
            serde_json::from_str::<Value>(&decoded).unwrap(),
            json!([{ "id": "F0123456789", "title": "file.txt" }])
        );
        assert!(step3.contains("channel_id=C0123456789"), "step3: {step3}");
        assert!(step3.contains("thread_ts=1700000000.000100"), "step3: {step3}");
        assert!(step3.contains("initial_comment="), "step3: {step3}");
    }

    /// urlencoded ボディから 1 フィールドを取り出す（テスト専用の素朴なデコーダ）。
    fn form_urlencoded_value(body: &str, key: &str) -> String {
        let raw = body
            .split('&')
            .find_map(|pair| pair.strip_prefix(&format!("{key}=")))
            .unwrap_or_else(|| panic!("{key} not present in {body}"));

        let bytes = raw.as_bytes();
        let mut out: Vec<u8> = Vec::new();
        let mut i = 0;
        while i < bytes.len() {
            match bytes[i] {
                b'+' => {
                    out.push(b' ');
                    i += 1;
                }
                b'%' if i + 2 < bytes.len() => {
                    let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap();
                    out.push(u8::from_str_radix(hex, 16).unwrap());
                    i += 3;
                }
                other => {
                    out.push(other);
                    i += 1;
                }
            }
        }
        String::from_utf8(out).unwrap()
    }

    #[tokio::test]
    async fn title_defaults_to_the_filename_and_is_overridable() {
        let server = MockServer::start().await;
        mount_upload_flow(&server).await;

        let mut cmd = command("C0123456789");
        cmd.content = Some("x".into());
        cmd.title = Some("Weekly report".into());
        run(cmd, &client_for(&server), &json_opts()).await.unwrap();

        let requests = server.received_requests().await.unwrap();
        let step3 = body_of(&requests, "files.completeUploadExternal");
        let decoded = form_urlencoded_value(step3, "files");
        assert_eq!(
            serde_json::from_str::<Value>(&decoded).unwrap()[0]["title"],
            "Weekly report"
        );
    }

    #[tokio::test]
    async fn resolves_channel_names_before_uploading() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/conversations.list"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "channels": [{ "id": "C0123456789", "name": "general" }],
                "response_metadata": { "next_cursor": "" },
            })))
            .mount(&server)
            .await;
        mount_upload_flow(&server).await;

        let mut cmd = command("general");
        cmd.content = Some("x".into());
        run(cmd, &client_for(&server), &json_opts()).await.unwrap();

        let requests = server.received_requests().await.unwrap();
        let step3 = body_of(&requests, "files.completeUploadExternal");
        assert!(step3.contains("channel_id=C0123456789"), "step3: {step3}");
    }

    #[tokio::test]
    async fn api_errors_from_the_first_step_stop_the_upload() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/files.getUploadURLExternal"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": false,
                "error": "missing_scope",
                "needed": "files:write",
            })))
            .mount(&server)
            .await;

        let mut cmd = command("C0123456789");
        cmd.content = Some("x".into());
        let err = run(cmd, &client_for(&server), &json_opts())
            .await
            .unwrap_err();

        assert_eq!(err.to_string(), "API Error: missing_scope (needed: files:write)");
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn a_non_200_from_the_upload_url_is_reported_as_a_file_error() {
        let server = MockServer::start().await;
        let upload_url = format!("{}/upload/v1/abc123", server.uri());
        Mock::given(method("POST"))
            .and(path("/files.getUploadURLExternal"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "upload_url": upload_url,
                "file_id": "F1",
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/upload/v1/abc123"))
            .respond_with(ResponseTemplate::new(413))
            .mount(&server)
            .await;

        let mut cmd = command("C0123456789");
        cmd.content = Some("x".into());
        let err = run(cmd, &client_for(&server), &json_opts())
            .await
            .unwrap_err();
        assert_eq!(err.to_string(), "Failed to upload file (status: 413)");
    }

    #[tokio::test]
    async fn an_off_slack_upload_url_never_receives_the_token() {
        let slack = MockServer::start().await;
        let attacker = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&attacker)
            .await;
        Mock::given(method("POST"))
            .and(path("/files.getUploadURLExternal"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "upload_url": format!("{}/steal", attacker.uri()),
                "file_id": "F1",
            })))
            .mount(&slack)
            .await;

        let mut cmd = command("C0123456789");
        cmd.content = Some("secret".into());
        let err = run(cmd, &client_for(&slack), &json_opts())
            .await
            .unwrap_err();

        assert!(
            err.to_string().starts_with("Refusing to upload to a non-Slack host:"),
            "error was: {err}"
        );
        assert!(
            attacker.received_requests().await.unwrap().is_empty(),
            "the file must never leave Slack"
        );
    }

    #[tokio::test]
    async fn dry_run_sends_nothing_and_still_prints_the_envelope() {
        let server = MockServer::start().await;
        mount_upload_flow(&server).await;

        let mut cmd = command("C0123456789");
        cmd.content = Some("x".into());
        let client = client_for(&server).with_dry_run(true);
        run(cmd, &client, &json_opts()).await.unwrap();

        assert!(server.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn output_carries_the_channel_and_the_picked_file_fields() {
        let server = MockServer::start().await;
        mount_upload_flow(&server).await;

        let (_, file_id) = get_upload_url(&client_for(&server), "report.csv", 12, None)
            .await
            .unwrap();
        assert_eq!(file_id, "F0123456789");

        let response = complete_upload(
            &client_for(&server),
            &CompleteUpload {
                file_id: &file_id,
                title: "report.csv",
                channel_id: "C0123456789",
                thread_ts: None,
                initial_comment: None,
            },
        )
        .await
        .unwrap();

        let value = json!({ "channel": "general", "files": collect_files(&response) });
        for format in [OutputFormat::Json, OutputFormat::Table, OutputFormat::Yaml] {
            let mut buf = Vec::new();
            output::format_value(&value, format, &mut buf).unwrap();
            assert!(
                String::from_utf8(buf).unwrap().contains("F0123456789"),
                "{format} lost the file id"
            );
        }

        // id-only は封筒（channel + files）の直下に識別子が無いので何も出さない
        let mut buf = Vec::new();
        output::format_value(&value, OutputFormat::IdOnly, &mut buf).unwrap();
        assert!(buf.is_empty());

        let mut buf = Vec::new();
        output::format_value(&value, OutputFormat::Json, &mut buf).unwrap();
        let rendered: Value = serde_json::from_slice(&buf).unwrap();
        assert_eq!(rendered["channel"], "general");
        assert_eq!(rendered["files"][0]["id"], "F0123456789");
        assert!(rendered["files"][0].get("size").is_none());
    }
}
