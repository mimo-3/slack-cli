use std::fs;
use std::net::TcpListener;
use std::process::{Command, Output};
use tempfile::TempDir;

fn run(home: &TempDir, args: &[&str], extra: &[(&str, &str)]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_slack-cli"));
    cmd.env_clear()
        .env("HOME", home.path())
        .env("NO_COLOR", "1");
    for (key, value) in extra {
        cmd.env(key, value);
    }
    cmd.args(args).output().unwrap()
}
fn fixture(home: &TempDir, default: Option<&str>) {
    let dir = home.path().join(".slack-cli");
    fs::create_dir_all(&dir).unwrap();
    let mut json = serde_json::json!({"profiles":{"default":{"token":"dummy-secret-token","updatedAt":"2026-01-01"},"work":{"token":"another-secret-token","updatedAt":"2026-01-01"}}});
    if let Some(name) = default {
        json["defaultProfile"] = name.into();
    }
    fs::write(dir.join("config.json"), json.to_string()).unwrap();
}
fn text(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}
fn succeeds(home: &TempDir, args: &[&str]) {
    let out = run(home, args, &[]);
    assert!(out.status.success(), "{:?}: {}", args, text(&out));
}
fn dry_args() -> Vec<&'static str> {
    vec!["--dry-run", "send", "-c", "C12345678", "-m", "hello"]
}

#[test]
fn missing_profile_fails_before_network_including_dry_run_and_drafts() {
    let home = TempDir::new().unwrap();
    fixture(&home, None);
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let proxy = format!("http://{}", listener.local_addr().unwrap());
    for args in [
        vec!["auth", "test"],
        dry_args(),
        vec!["send", "-c", "C12345678", "-m", "hello"],
        vec!["delete", "-c", "C12345678", "--ts", "1234567890.123456"],
        vec!["upload", "-c", "C12345678", "--content", "hello"],
        vec!["draft", "send", "--id", "1"],
    ] {
        let output = run(
            &home,
            &args,
            &[("HTTPS_PROXY", &proxy), ("HTTP_PROXY", &proxy)],
        );
        let error = text(&output);
        assert!(!output.status.success(), "{args:?}");
        assert!(
            error.contains("--profile <名前>")
                && error.contains("config use <名前>")
                && error.contains("default")
                && error.contains("work"),
            "{error}"
        );
        assert!(!error.contains("secret-token"));
        assert!(!error.contains("invalid_auth"), "{error}");
        assert!(listener.accept().is_err(), "unexpected proxy connection");
    }
    succeeds(&home, &["--help"]);
    succeeds(&home, &["draft", "list"]);
}

#[test]
fn set_use_current_clear_and_no_default() {
    let home = TempDir::new().unwrap();
    succeeds(
        &home,
        &[
            "config",
            "set",
            "--profile",
            "work",
            "--token",
            "dummy-token",
        ],
    );
    succeeds(
        &home,
        &[
            "config",
            "set",
            "--profile",
            "default",
            "--token",
            "dummy-token-two",
        ],
    );
    let current = run(&home, &["config", "current"], &[]);
    assert!(current.status.success());
    assert!(String::from_utf8_lossy(&current.stdout).contains("No default"));
    let profiles = run(&home, &["config", "profiles", "--json"], &[]);
    let profiles: serde_json::Value = serde_json::from_slice(&profiles.stdout).unwrap();
    assert!(profiles
        .as_array()
        .unwrap()
        .iter()
        .all(|p| p["default"] == false));
    for sub in ["set", "get", "clear"] {
        assert!(!run(&home, &["config", sub], &[]).status.success());
    }
    assert!(!run(&home, &["config", "use", "unknown"], &[])
        .status
        .success());
    succeeds(&home, &["config", "use", "work"]);
    let current = run(&home, &["config", "current", "--json"], &[]);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&current.stdout).unwrap()["profile"],
        "work"
    );
    succeeds(&home, &["config", "get"]);
    succeeds(&home, &dry_args());
    succeeds(&home, &["config", "clear"]);
    assert!(!run(&home, &dry_args(), &[]).status.success());
    let current = run(&home, &["config", "current", "--json"], &[]);
    assert!(
        serde_json::from_slice::<serde_json::Value>(&current.stdout).unwrap()["profile"].is_null()
    );
    succeeds(&home, &["config", "use", "default"]);
    succeeds(&home, &["config", "use", "--none"]);
    assert!(!run(&home, &dry_args(), &[]).status.success());
}

#[test]
fn old_config_and_resolution_precedence() {
    let home = TempDir::new().unwrap();
    fixture(&home, Some("work"));
    succeeds(&home, &dry_args());
    let out = run(&home, &["config", "profiles", "--json"], &[]);
    let profiles: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(profiles
        .as_array()
        .unwrap()
        .iter()
        .any(|p| p["profile"] == "work" && p["default"] == true));
    let mut explicit = dry_args();
    explicit.extend(["--profile", "missing"]);
    assert!(!run(&home, &explicit, &[]).status.success());
    assert!(run(&home, &explicit, &[("SLACK_CLI_TOKEN", "env-token")])
        .status
        .success());
    explicit.extend(["--token", "flag-token"]);
    assert!(run(&home, &explicit, &[("SLACK_CLI_TOKEN", "env-token")])
        .status
        .success());
    let mut named = dry_args();
    named.extend(["--profile", "default"]);
    succeeds(&home, &named);
    fixture(&home, Some("missing"));
    assert!(!run(&home, &dry_args(), &[]).status.success());
    fixture(&home, None);
    assert!(!run(&home, &dry_args(), &[]).status.success());
}

#[test]
fn resolution_order_returns_the_right_token() {
    if std::env::var_os("PROFILE_TEST_CHILD").is_some() {
        let manager = slack_cli::config::ProfileConfigManager::new().unwrap();
        assert_eq!(
            manager
                .resolve_token(Some("flag-value"), Some("default"))
                .unwrap(),
            "flag-value"
        );
        assert_eq!(
            manager.resolve_token(None, Some("default")).unwrap(),
            "environment-value"
        );
        return;
    }
    let home = TempDir::new().unwrap();
    fixture(&home, Some("work"));
    let output = Command::new(std::env::current_exe().unwrap())
        .env_clear()
        .env("HOME", home.path())
        .env("PROFILE_TEST_CHILD", "1")
        .env("SLACK_CLI_TOKEN", "environment-value")
        .args(["--exact", "resolution_order_returns_the_right_token"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", text(&output));
    let manager =
        slack_cli::config::ProfileConfigManager::with_options(slack_cli::config::ConfigOptions {
            config_dir: Some(home.path().join(".slack-cli")),
            crypto: Some(slack_cli::config::crypto::CryptoOptions {
                master_key: Some("test-key".into()),
                ..Default::default()
            }),
        })
        .unwrap();
    // The child has not resolved a profile; the fixture remains plaintext here.
    assert_eq!(
        manager.get_config(Some("default")).unwrap().unwrap().token,
        "dummy-secret-token"
    );
    assert_eq!(
        manager.get_config(None).unwrap().unwrap().token,
        "another-secret-token"
    );
}
