//! `slack-cli download` — Slack 上のファイルのダウンロード。
//!
//! アップロードと逆で、こちらはリダイレクトを**追う**必要がある。`url_private` は
//! 署名付きの配信ホストへ 302 することがあり、そこではトークンが要らないため。
//! reqwest の既定ポリシー（`limited(10)`）はクロスオリジン遷移で `Authorization` を
//! 落とすので、本体クライアント（リダイレクト禁止）とは別のクライアントを立てる。

use std::io::Write;
use std::path::{Path, PathBuf};

use clap::Args;
use colored::Colorize;
use serde_json::{json, Value};
use url::Url;

use crate::cli::GlobalOpts;
use crate::client::SlackClient;
use crate::error::SlackCliError;
use crate::output;

pub const ERR_NO_TARGET: &str = "You must specify either --url or --id";
pub const ERR_BOTH_TARGETS: &str = "Cannot use both --url and --id";
pub const ERR_NO_DOWNLOAD_URL: &str = "No download URL found for this file";

const FILES_INFO_METHOD: &str = "files.info";
/// URL からファイル名を導けなかったときの保存名。
const FALLBACK_FILE_NAME: &str = "download";

/// Bearer を付けてよい Slack のファイル配信ドメイン（移植方針 F1）。
const SLACK_FILE_HOSTS: [&str; 2] = ["slack.com", "slack-files.com"];
const SLACK_FILE_HOST_SUFFIXES: [&str; 3] = [".slack.com", ".slack-edge.com", ".slack-files.com"];

#[derive(Args, Debug)]
pub struct DownloadCommand {
    /// File URL (url_private or url_private_download from message)
    #[arg(short, long, value_name = "URL")]
    pub url: Option<String>,

    /// Slack file ID (e.g. F0BFXAEP1UZ)
    #[arg(short, long, value_name = "ID")]
    pub id: Option<String>,

    /// Output file path (defaults to original filename in current dir)
    #[arg(short, long, value_name = "PATH")]
    pub output: Option<String>,
}

pub async fn run(
    cmd: DownloadCommand,
    client: &SlackClient,
    global: &GlobalOpts,
) -> Result<(), SlackCliError> {
    let (url, file_name) = resolve_target(&cmd, client).await?;

    let output_path = match cmd.output.as_deref() {
        Some(path) => PathBuf::from(path),
        None => PathBuf::from(".").join(&file_name),
    };

    let size = fetch_to_file(client, &url, &output_path).await?;

    eprintln!("{}", format!("✓ Downloaded: {file_name}").green());

    let value = json!({
        "filePath": output_path.to_string_lossy(),
        "fileName": file_name,
        "size": size,
    });
    output::format_value(&value, global.output_format(), &mut std::io::stdout())?;
    Ok(())
}

/// `--url` / `--id` の排他を検証し、ダウンロード元 URL と保存名を確定させる。
async fn resolve_target(
    cmd: &DownloadCommand,
    client: &SlackClient,
) -> Result<(Url, String), SlackCliError> {
    match (cmd.url.as_deref(), cmd.id.as_deref()) {
        (Some(_), Some(_)) => Err(SlackCliError::Validation(ERR_BOTH_TARGETS.to_string())),
        (None, None) => Err(SlackCliError::Validation(ERR_NO_TARGET.to_string())),
        (Some(raw), None) => {
            let url = parse_url(raw)?;
            let name = file_name_from_url(&url);
            Ok((url, name))
        }
        (None, Some(id)) => {
            let response = client.get(FILES_INFO_METHOD, &[("file", id)]).await?;
            let file = response.get("file");

            let raw = file
                .and_then(|f| non_empty_str(f, "url_private_download"))
                .or_else(|| file.and_then(|f| non_empty_str(f, "url_private")))
                .ok_or_else(|| SlackCliError::File(ERR_NO_DOWNLOAD_URL.to_string()))?;

            let name = file
                .and_then(|f| non_empty_str(f, "name"))
                .unwrap_or(id)
                .to_string();
            Ok((parse_url(raw)?, safe_file_name(&name)))
        }
    }
}

fn non_empty_str<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
}

fn parse_url(raw: &str) -> Result<Url, SlackCliError> {
    Url::parse(raw).map_err(|e| SlackCliError::Validation(format!("Invalid URL: {e}")))
}

/// ダウンロードして保存し、書き込んだバイト数を返す。既存ファイルは無警告で上書きする
/// （移植方針 F4）。
async fn fetch_to_file(
    client: &SlackClient,
    url: &Url,
    output_path: &Path,
) -> Result<u64, SlackCliError> {
    let http = reqwest::Client::builder()
        .user_agent(format!("slack-cli/{}", env!("CARGO_PKG_VERSION")))
        .build()?;

    let mut request = http.get(url.clone());
    // Slack 以外のホストにはトークンを付けない。付けないだけでリクエスト自体は通す
    // ので、公開 URL を渡す使い方はそのまま動く。
    if should_attach_token(url, &client.base_url) {
        request = request.headers(client.auth_headers());
    }

    let mut response = request.send().await?;
    let status = response.status();
    if !status.is_success() {
        return Err(SlackCliError::File(
            format!(
                "Download failed: {} {}",
                status.as_u16(),
                status.canonical_reason().unwrap_or_default()
            )
            .trim_end()
            .to_string(),
        ));
    }

    let mut file = std::fs::File::create(output_path)?;
    let mut size: u64 = 0;
    while let Some(chunk) = response.chunk().await? {
        size += chunk.len() as u64;
        file.write_all(&chunk)?;
    }
    file.flush()?;
    Ok(size)
}

fn should_attach_token(url: &Url, base_url: &Url) -> bool {
    if url.origin() == base_url.origin() {
        return true;
    }
    if url.scheme() != "https" {
        return false;
    }
    let Some(host) = url.host_str() else {
        return false;
    };
    SLACK_FILE_HOSTS.contains(&host)
        || SLACK_FILE_HOST_SUFFIXES
            .iter()
            .any(|suffix| host.ends_with(suffix))
}

/// URL のパス末尾をパーセントデコードして保存名にする。
fn file_name_from_url(url: &Url) -> String {
    let last = url
        .path_segments()
        .and_then(|mut segments| segments.rfind(|s| !s.is_empty()))
        .unwrap_or_default();
    safe_file_name(&percent_decode(last))
}

/// 保存名からディレクトリ成分を落とす。`..` や `/` を含む名前で
/// カレントディレクトリの外に書き出さないための最低限の防御。
fn safe_file_name(raw: &str) -> String {
    let base = raw
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or_default()
        .trim_matches(char::from(0));

    if base.is_empty() || base == "." || base == ".." {
        FALLBACK_FILE_NAME.to_string()
    } else {
        base.to_string()
    }
}

fn percent_decode(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut decoded: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;

    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(high), Some(low)) = (hex_value(bytes[i + 1]), hex_value(bytes[i + 2])) {
                decoded.push(high * 16 + low);
                i += 3;
                continue;
            }
        }
        decoded.push(bytes[i]);
        i += 1;
    }

    String::from_utf8_lossy(&decoded).into_owned()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use crate::cli::Cli;
    use crate::output::OutputFormat;

    const TEST_TOKEN: &str = "test-token-value";

    fn parse(argv: &[&str]) -> DownloadCommand {
        let cli = Cli::try_parse_from(argv).unwrap();
        let crate::cli::Command::Download(cmd) = cli.command else {
            panic!("expected the download command");
        };
        cmd
    }

    fn client_for(server: &MockServer) -> SlackClient {
        SlackClient::new(TEST_TOKEN)
            .unwrap()
            .with_base_url(Url::parse(&format!("{}/", server.uri())).unwrap())
    }

    fn json_opts() -> GlobalOpts {
        GlobalOpts {
            json: true,
            ..GlobalOpts::default()
        }
    }

    fn command() -> DownloadCommand {
        DownloadCommand {
            url: None,
            id: None,
            output: None,
        }
    }

    async fn mount_file_bytes(server: &MockServer, body: &'static [u8]) {
        Mock::given(method("GET"))
            .and(path("/files-pri/T1-F1/report.csv"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(body))
            .mount(server)
            .await;
    }

    async fn mount_files_info(server: &MockServer, file: Value) {
        Mock::given(method("GET"))
            .and(path("/files.info"))
            .and(query_param("file", "F0BFXAEP1UZ"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({ "ok": true, "file": file })),
            )
            .mount(server)
            .await;
    }

    #[test]
    fn url_and_id_are_both_optional_at_parse_time() {
        // 移植方針 G12: 「どちらか一方が要る」の判定は run() 側に残す
        let cmd = parse(&["slack-cli", "download"]);
        assert!(cmd.url.is_none());
        assert!(cmd.id.is_none());
    }

    #[test]
    fn parses_id_and_output() {
        let cmd = parse(&[
            "slack-cli",
            "download",
            "-i",
            "F0BFXAEP1UZ",
            "-o",
            "out.png",
        ]);
        assert_eq!(cmd.id.as_deref(), Some("F0BFXAEP1UZ"));
        assert_eq!(cmd.output.as_deref(), Some("out.png"));
    }

    #[tokio::test]
    async fn url_and_id_are_mutually_exclusive_and_one_is_required() {
        let server = MockServer::start().await;
        let client = client_for(&server);

        let err = resolve_target(&command(), &client).await.unwrap_err();
        assert_eq!(err.to_string(), ERR_NO_TARGET);

        let both = DownloadCommand {
            url: Some("https://files.slack.com/a".into()),
            id: Some("F1".into()),
            output: None,
        };
        let err = resolve_target(&both, &client).await.unwrap_err();
        assert_eq!(err.to_string(), ERR_BOTH_TARGETS);

        assert!(server.received_requests().await.unwrap().is_empty());
    }

    #[test]
    fn file_names_come_from_the_decoded_url_basename() {
        let url =
            Url::parse("https://files.slack.com/files-pri/T1-F1/%E8%B3%87%E6%96%99.pdf").unwrap();
        assert_eq!(file_name_from_url(&url), "資料.pdf");

        let url = Url::parse("https://files.slack.com/files-pri/T1-F1/report.csv?t=xoxe").unwrap();
        assert_eq!(file_name_from_url(&url), "report.csv");
    }

    #[test]
    fn traversal_attempts_in_the_file_name_are_neutralised() {
        assert_eq!(safe_file_name("../../etc/passwd"), "passwd");
        assert_eq!(safe_file_name(".."), FALLBACK_FILE_NAME);
        assert_eq!(safe_file_name(""), FALLBACK_FILE_NAME);

        let url = Url::parse("https://files.slack.com/x/%2e%2e%2f%2e%2e").unwrap();
        assert_eq!(file_name_from_url(&url), FALLBACK_FILE_NAME);
    }

    #[test]
    fn only_slack_file_hosts_get_the_bearer_token() {
        let base = Url::parse("https://slack.com/api/").unwrap();
        for trusted in [
            "https://files.slack.com/files-pri/T1-F1/a.csv",
            "https://slack.com/files/a",
            "https://a.slack-edge.com/x",
            "https://slack-files.com/x",
        ] {
            assert!(
                should_attach_token(&Url::parse(trusted).unwrap(), &base),
                "{trusted} should be trusted"
            );
        }
        for untrusted in [
            "https://attacker.example/steal",
            "https://notslack.com/x",
            "http://files.slack.com/x",
            "https://files.slack.com.attacker.example/x",
        ] {
            assert!(
                !should_attach_token(&Url::parse(untrusted).unwrap(), &base),
                "{untrusted} must not receive the token"
            );
        }
    }

    #[tokio::test]
    async fn downloads_by_id_using_the_download_url_and_reports_the_size() {
        let server = MockServer::start().await;
        mount_files_info(
            &server,
            json!({
                "id": "F0BFXAEP1UZ",
                "name": "report.csv",
                "url_private": format!("{}/files-pri/T1-F1/ignored.csv", server.uri()),
                "url_private_download": format!("{}/files-pri/T1-F1/report.csv", server.uri()),
            }),
        )
        .await;
        mount_file_bytes(&server, b"a,b\n1,2\n").await;

        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("saved.csv");
        let cmd = DownloadCommand {
            url: None,
            id: Some("F0BFXAEP1UZ".into()),
            output: Some(out.to_string_lossy().into_owned()),
        };

        run(cmd, &client_for(&server), &json_opts()).await.unwrap();

        assert_eq!(std::fs::read(&out).unwrap(), b"a,b\n1,2\n");

        // Slack と同一オリジンなので Bearer が付く
        let requests = server.received_requests().await.unwrap();
        let download = requests
            .iter()
            .find(|r| r.url.path().ends_with("report.csv"))
            .unwrap();
        assert_eq!(
            download.headers.get("authorization").unwrap(),
            &format!("Bearer {TEST_TOKEN}")
        );
    }

    #[tokio::test]
    async fn falls_back_to_url_private_and_to_the_file_id_as_a_name() {
        let server = MockServer::start().await;
        mount_files_info(
            &server,
            json!({
                "id": "F0BFXAEP1UZ",
                "url_private": format!("{}/files-pri/T1-F1/report.csv", server.uri()),
            }),
        )
        .await;

        let cmd = DownloadCommand {
            url: None,
            id: Some("F0BFXAEP1UZ".into()),
            output: None,
        };
        let (url, name) = resolve_target(&cmd, &client_for(&server)).await.unwrap();
        assert!(url.path().ends_with("report.csv"));
        assert_eq!(name, "F0BFXAEP1UZ");
    }

    #[tokio::test]
    async fn a_file_without_any_url_is_an_error() {
        let server = MockServer::start().await;
        mount_files_info(&server, json!({ "id": "F0BFXAEP1UZ", "url_private": "" })).await;

        let cmd = DownloadCommand {
            url: None,
            id: Some("F0BFXAEP1UZ".into()),
            output: None,
        };
        let err = resolve_target(&cmd, &client_for(&server))
            .await
            .unwrap_err();
        assert_eq!(err.to_string(), ERR_NO_DOWNLOAD_URL);
        assert_eq!(err.code(), Some(crate::error::CODE_FILE));
    }

    #[tokio::test]
    async fn api_errors_from_files_info_are_surfaced() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/files.info"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({ "ok": false, "error": "file_not_found" })),
            )
            .mount(&server)
            .await;

        let cmd = DownloadCommand {
            url: None,
            id: Some("F0BFXAEP1UZ".into()),
            output: None,
        };
        let err = resolve_target(&cmd, &client_for(&server))
            .await
            .unwrap_err();
        assert_eq!(err.to_string(), "API Error: file_not_found");
    }

    #[tokio::test]
    async fn a_non_2xx_download_reports_status_and_reason() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/files-pri/T1-F1/missing.csv"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;

        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("missing.csv");
        let url = Url::parse(&format!("{}/files-pri/T1-F1/missing.csv", server.uri())).unwrap();

        let err = fetch_to_file(&client_for(&server), &url, &out)
            .await
            .unwrap_err();
        assert_eq!(err.to_string(), "Download failed: 404 Not Found");
        assert!(!out.exists(), "nothing should have been written");
    }

    #[tokio::test]
    async fn non_slack_hosts_never_receive_the_token() {
        let slack = MockServer::start().await;
        let elsewhere = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"public".to_vec()))
            .mount(&elsewhere)
            .await;

        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("public.txt");
        let url = Url::parse(&format!("{}/public.txt", elsewhere.uri())).unwrap();

        let size = fetch_to_file(&client_for(&slack), &url, &out)
            .await
            .unwrap();
        assert_eq!(size, 6);

        let requests = elsewhere.received_requests().await.unwrap();
        assert!(
            requests[0].headers.get("authorization").is_none(),
            "the token must not leak to a non-Slack host"
        );
    }

    #[tokio::test]
    async fn existing_files_are_overwritten_without_a_prompt() {
        let server = MockServer::start().await;
        mount_file_bytes(&server, b"new").await;

        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("report.csv");
        std::fs::write(&out, b"old contents that are longer").unwrap();

        let url = Url::parse(&format!("{}/files-pri/T1-F1/report.csv", server.uri())).unwrap();
        fetch_to_file(&client_for(&server), &url, &out)
            .await
            .unwrap();
        assert_eq!(std::fs::read(&out).unwrap(), b"new");
    }

    #[tokio::test]
    async fn output_defaults_to_the_file_name_in_the_current_directory() {
        let server = MockServer::start().await;
        mount_file_bytes(&server, b"x").await;

        let cmd = DownloadCommand {
            url: Some(format!("{}/files-pri/T1-F1/report.csv", server.uri())),
            id: None,
            output: None,
        };
        let (_, name) = resolve_target(&cmd, &client_for(&server)).await.unwrap();
        assert_eq!(
            PathBuf::from(".").join(&name).to_string_lossy(),
            "./report.csv"
        );
    }

    #[tokio::test]
    async fn every_output_format_renders_the_result() {
        let value = json!({ "filePath": "./report.csv", "fileName": "report.csv", "size": 12600 });
        for format in [
            OutputFormat::Json,
            OutputFormat::Table,
            OutputFormat::Yaml,
            OutputFormat::Csv,
        ] {
            let mut buf = Vec::new();
            output::format_value(&value, format, &mut buf).unwrap();
            assert!(
                String::from_utf8(buf).unwrap().contains("report.csv"),
                "{format} lost the file name"
            );
        }
    }

    #[tokio::test]
    async fn run_writes_the_file_and_prints_the_json_envelope() {
        let server = MockServer::start().await;
        mount_file_bytes(&server, b"hello").await;

        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("hello.txt");
        let cmd = DownloadCommand {
            url: Some(format!("{}/files-pri/T1-F1/report.csv", server.uri())),
            id: None,
            output: Some(out.to_string_lossy().into_owned()),
        };

        run(cmd, &client_for(&server), &json_opts()).await.unwrap();
        assert_eq!(std::fs::read(&out).unwrap(), b"hello");
    }
}
