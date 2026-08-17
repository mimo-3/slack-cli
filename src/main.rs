//! slack-cli のエントリポイント。
//!
//! ここは薄く保つ。`Cli::parse()` から対応する `cli::<command>::run()` へ渡すだけで、
//! HTTP もフォーマットもこのファイルには置かない。

use std::io::Write;
use std::process;

use clap::Parser;
use colored::Colorize;

use slack_cli::cli::{self, Cli, Command, GlobalOpts};
use slack_cli::client::SlackClient;
use slack_cli::config::ProfileConfigManager;
use slack_cli::error::SlackCliError;
use slack_cli::output::sanitize::{redact_slack_tokens, sanitize_terminal_text};

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    if cli.global.no_color {
        colored::control::set_override(false);
    }

    let result = run(cli).await;

    // process::exit はデストラクタを走らせないので、ここで明示的に流し切る（移植方針 J8）。
    let _ = std::io::stdout().flush();

    if let Err(e) = result {
        // 順序は TS 版と同じ「サニタイズ → 伏字」。逆にすると、エスケープで分断された
        // トークンが伏字をすり抜ける（移植方針 E）。
        let message = redact_slack_tokens(&sanitize_terminal_text(&e.to_string()));
        eprintln!("{} {message}", "✗ Error:".red());
        let _ = std::io::stderr().flush();
        process::exit(e.exit_code());
    }
}

async fn run(cli: Cli) -> Result<(), SlackCliError> {
    match cli.command {
        // 設定しか触らないコマンドはクライアントを組み立てない
        Command::Config(cmd) => cli::config_cmd::run(cmd, &cli.global).await,
        Command::Auth(cmd) => {
            let client = build_client(&cli.global)?;
            cli::auth::run(cmd, &client, &cli.global).await
        }

        // draft はローカルの下書きストアが主で、`draft send` だけが API を呼ぶ。
        // トークン未設定でも list / show / delete が動くよう、必要なときだけ組み立てる。
        Command::Draft(cmd) => {
            let client = if cmd.needs_client() {
                Some(build_client(&cli.global)?)
            } else {
                None
            };
            cli::draft::run(cmd, client.as_ref(), &cli.global).await
        }

        Command::Send(cmd) => {
            let client = build_client(&cli.global)?;
            cli::send::run(cmd, &client, &cli.global).await
        }
        Command::SendEphemeral(cmd) => {
            let client = build_client(&cli.global)?;
            cli::send_ephemeral::run(cmd, &client, &cli.global).await
        }
        Command::Edit(cmd) => {
            let client = build_client(&cli.global)?;
            cli::edit::run(cmd, &client, &cli.global).await
        }
        Command::Delete(cmd) => {
            let client = build_client(&cli.global)?;
            cli::delete::run(cmd, &client, &cli.global).await
        }
        Command::History(cmd) => {
            let client = build_client(&cli.global)?;
            cli::history::run(cmd, &client, &cli.global).await
        }
        Command::Unread(cmd) => {
            let client = build_client(&cli.global)?;
            cli::unread::run(cmd, &client, &cli.global).await
        }
        Command::Channels(cmd) => {
            let client = build_client(&cli.global)?;
            cli::channels::run(cmd, &client, &cli.global).await
        }
        Command::Channel(cmd) => {
            let client = build_client(&cli.global)?;
            cli::channel::run(cmd, &client, &cli.global).await
        }
        Command::Join(cmd) => {
            let client = build_client(&cli.global)?;
            cli::join::run(cmd, &client, &cli.global).await
        }
        Command::Leave(cmd) => {
            let client = build_client(&cli.global)?;
            cli::leave::run(cmd, &client, &cli.global).await
        }
        Command::Invite(cmd) => {
            let client = build_client(&cli.global)?;
            cli::invite::run(cmd, &client, &cli.global).await
        }
        Command::Members(cmd) => {
            let client = build_client(&cli.global)?;
            cli::members::run(cmd, &client, &cli.global).await
        }
        Command::Search(cmd) => {
            let client = build_client(&cli.global)?;
            cli::search::run(cmd, &client, &cli.global).await
        }
        Command::Users(cmd) => {
            let client = build_client(&cli.global)?;
            cli::users::run(cmd, &client, &cli.global).await
        }
        Command::Usergroups(cmd) => {
            let client = build_client(&cli.global)?;
            cli::usergroups::run(cmd, &client, &cli.global).await
        }
        Command::Reaction(cmd) => {
            let client = build_client(&cli.global)?;
            cli::reaction::run(cmd, &client, &cli.global).await
        }
        Command::Pin(cmd) => {
            let client = build_client(&cli.global)?;
            cli::pin::run(cmd, &client, &cli.global).await
        }
        Command::Upload(cmd) => {
            let client = build_client(&cli.global)?;
            cli::upload::run(cmd, &client, &cli.global).await
        }
        Command::Download(cmd) => {
            let client = build_client(&cli.global)?;
            cli::download::run(cmd, &client, &cli.global).await
        }
        Command::Canvas(cmd) => {
            let client = build_client(&cli.global)?;
            cli::canvas::run(cmd, &client, &cli.global).await
        }
        Command::Bookmark(cmd) => {
            let client = build_client(&cli.global)?;
            cli::bookmark::run(cmd, &client, &cli.global).await
        }
        Command::Scheduled(cmd) => {
            let client = build_client(&cli.global)?;
            cli::scheduled::run(cmd, &client, &cli.global).await
        }
        Command::Reminder(cmd) => {
            let client = build_client(&cli.global)?;
            cli::reminder::run(cmd, &client, &cli.global).await
        }
    }
}

/// トークンを解決して API クライアントを組み立てる。
/// 解決順は `--token` → 環境変数 `SLACK_CLI_TOKEN` → 設定ファイルのプロファイル。
fn build_client(global: &GlobalOpts) -> Result<SlackClient, SlackCliError> {
    let manager = ProfileConfigManager::new()?;
    let token = manager.resolve_token(global.token.as_deref(), global.profile.as_deref())?;
    Ok(SlackClient::new(token)?.with_dry_run(global.dry_run))
}
