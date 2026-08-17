//! clap のコマンド定義。derive スタイルの 2 段ネスト。
//!
//! # コマンドを 1 つ足す手順
//!
//! 1. `src/cli/<command>.rs` を作り、`pub struct XxxCommand`（`#[derive(Args)]`、中に
//!    `#[command(subcommand)] pub command: XxxSubcommand`）と
//!    `pub async fn run(cmd, &client, &global) -> Result<(), SlackCliError>` を書く。
//!    サブコマンドを持たない単発コマンドなら `#[derive(Args)] pub struct XxxArgs` だけでよい。
//! 2. このファイルの `mod` 宣言と `Command` enum にバリアントを 1 行ずつ足す。
//! 3. `src/main.rs` の `match` に 1 アームを足す。クライアントが要るコマンドは
//!    `build_client()` を通す。設定しか触らないコマンドは通さない。
//!
//! 各 `run()` の末尾は `output::format_value(&value, global.output_format(), &mut stdout())` に
//! 揃える。人間向けの補足メッセージは stderr、データは stdout に分けること。

pub mod auth;
pub mod bookmark;
pub mod canvas;
pub mod channel;
pub mod channels;
pub mod common;
pub mod config_cmd;
pub mod delete;
pub mod download;
pub mod draft;
pub mod edit;
pub mod history;
pub mod invite;
pub mod join;
pub mod leave;
pub mod members;
pub mod pin;
pub mod reaction;
pub mod reminder;
pub mod scheduled;
pub mod search;
pub mod send;
pub mod send_ephemeral;
pub mod unread;
pub mod upload;
pub mod usergroups;
pub mod users;

use clap::{Args, Parser, Subcommand};

use crate::error::SlackCliError;
use crate::output::OutputFormat;

#[derive(Parser, Debug)]
#[command(
    name = "slack-cli",
    version,
    about = "Command-line interface for the Slack Web API"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,

    #[command(flatten)]
    pub global: GlobalOpts,
}

#[derive(Args, Debug, Default)]
pub struct GlobalOpts {
    /// Slack API token (overrides the SLACK_CLI_TOKEN env var and the config file)
    #[arg(long, global = true, value_name = "TOKEN")]
    pub token: Option<String>,

    /// Configuration profile to use
    #[arg(long, global = true, value_name = "PROFILE")]
    pub profile: Option<String>,

    /// Output format
    #[arg(long, global = true, default_value = "table", value_name = "FORMAT")]
    pub format: OutputFormat,

    /// Shorthand for --format json
    #[arg(long, global = true)]
    pub json: bool,

    /// Show what would be sent without making any write request
    #[arg(long, global = true)]
    pub dry_run: bool,

    /// Disable colored output
    #[arg(long, global = true)]
    pub no_color: bool,
}

impl GlobalOpts {
    /// `--json` のショートハンドを織り込んだ実効フォーマット。
    pub fn output_format(&self) -> OutputFormat {
        if self.json {
            OutputFormat::Json
        } else {
            self.format
        }
    }
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Manage Slack CLI configuration
    Config(config_cmd::ConfigCommand),
    /// Verify the configured token
    Auth(auth::AuthCommand),
    /// Send or schedule a message to a Slack channel or DM
    Send(send::SendCommand),
    /// Send an ephemeral message visible only to a specific user in a channel
    SendEphemeral(send_ephemeral::SendEphemeralCommand),
    /// Edit a sent message
    Edit(edit::EditCommand),
    /// Delete a sent message
    Delete(delete::DeleteCommand),
    /// Get message history from a Slack channel
    History(history::HistoryCommand),
    /// Show unread messages across channels
    Unread(unread::UnreadCommand),
    /// List Slack channels
    Channels(channels::ChannelsCommand),
    /// Manage channel topic, purpose, and info
    Channel(channel::ChannelCommand),
    /// Join a channel
    Join(join::JoinCommand),
    /// Leave a channel
    Leave(leave::LeaveCommand),
    /// Invite user(s) to a channel
    Invite(invite::InviteCommand),
    /// List channel members
    Members(members::MembersCommand),
    /// Search messages in Slack workspace
    Search(search::SearchCommand),
    /// List, search, and get information about workspace users
    Users(users::UsersCommand),
    /// List user groups and their members
    Usergroups(usergroups::UsergroupsCommand),
    /// Add or remove emoji reactions on messages
    Reaction(reaction::ReactionCommand),
    /// Add, remove, or list pinned messages in a channel
    Pin(pin::PinCommand),
    /// Upload a file or snippet to a Slack channel
    Upload(upload::UploadCommand),
    /// Download a file from Slack
    Download(download::DownloadCommand),
    /// Manage Slack Canvases
    Canvas(canvas::CanvasCommand),
    /// Manage saved items (save for later)
    Bookmark(bookmark::BookmarkCommand),
    /// Manage message drafts (save, list, show, send, delete)
    Draft(draft::DraftCommand),
    /// Manage scheduled messages (list, cancel)
    Scheduled(scheduled::ScheduledCommand),
    /// Create, list, delete, or complete reminders
    Reminder(reminder::ReminderCommand),
}

/// `--limit` / `--number` / `--page` / `--after` のような数値フラグの厳格パース。
///
/// 前後の空白のみ許し、それ以外の非数字が混ざればエラーにする（移植方針 A1 / A2）。
/// `message` には `--limit must be a positive integer` のようなフラグ固有の文言を渡す。
pub fn parse_positive_int(raw: &str, message: &str) -> Result<u32, SlackCliError> {
    let parsed: u32 = raw
        .trim()
        .parse()
        .map_err(|_| SlackCliError::Validation(message.to_string()))?;

    if parsed == 0 {
        return Err(SlackCliError::Validation(message.to_string()));
    }
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::error::ErrorKind;

    #[test]
    fn global_options_are_accepted_after_the_subcommand() {
        let cli = Cli::try_parse_from([
            "slack-cli",
            "auth",
            "test",
            "--profile",
            "work",
            "--json",
            "--dry-run",
        ])
        .unwrap();

        assert_eq!(cli.global.profile.as_deref(), Some("work"));
        assert!(cli.global.dry_run);
        assert_eq!(cli.global.output_format(), OutputFormat::Json);
    }

    #[test]
    fn json_shorthand_wins_over_format() {
        let cli =
            Cli::try_parse_from(["slack-cli", "auth", "test", "--format", "csv", "--json"]).unwrap();
        assert_eq!(cli.global.output_format(), OutputFormat::Json);

        let cli = Cli::try_parse_from(["slack-cli", "auth", "test", "--format", "csv"]).unwrap();
        assert_eq!(cli.global.output_format(), OutputFormat::Csv);
    }

    #[test]
    fn default_format_is_table() {
        let cli = Cli::try_parse_from(["slack-cli", "auth", "test"]).unwrap();
        assert_eq!(cli.global.output_format(), OutputFormat::Table);
    }

    #[test]
    fn unknown_format_is_rejected_at_parse_time() {
        let err = Cli::try_parse_from(["slack-cli", "auth", "test", "--format", "markdown"])
            .expect_err("markdown is not a supported format");
        // OutputFormat は手書きの FromStr を value_parser に使うので ValueValidation になる
        assert_eq!(err.kind(), ErrorKind::ValueValidation);
        assert!(
            err.to_string().contains("Unknown output format: markdown"),
            "error was: {err}"
        );
    }

    #[test]
    fn positive_integers_reject_anything_but_digits() {
        const MSG: &str = "--limit must be a positive integer";
        assert_eq!(parse_positive_int(" 25 ", MSG).unwrap(), 25);
        for raw in ["abc", "12abc", "3.7", "-1", "0", ""] {
            assert_eq!(
                parse_positive_int(raw, MSG).unwrap_err().to_string(),
                MSG,
                "{raw:?} should have been rejected"
            );
        }
    }

    #[test]
    fn every_top_level_command_is_registered() {
        for argv in [
            vec!["slack-cli", "send", "-m", "hi"],
            vec!["slack-cli", "send-ephemeral", "-c", "C1", "-u", "U1", "-m", "hi"],
            vec!["slack-cli", "edit", "-c", "C1", "--ts", "1.1"],
            vec!["slack-cli", "delete", "-c", "C1", "--ts", "1.1"],
            vec!["slack-cli", "history", "-c", "C1"],
            vec!["slack-cli", "unread"],
            vec!["slack-cli", "channels"],
            vec!["slack-cli", "channel", "info", "-c", "C1"],
            vec!["slack-cli", "join", "-c", "C1"],
            vec!["slack-cli", "leave", "-c", "C1"],
            vec!["slack-cli", "invite", "-c", "C1", "-u", "U1"],
            vec!["slack-cli", "members", "-c", "C1"],
            vec!["slack-cli", "search", "-q", "x"],
            vec!["slack-cli", "users", "list"],
            vec!["slack-cli", "usergroups", "list"],
            vec!["slack-cli", "reaction", "add", "-c", "C1", "-t", "1.1", "-e", "tada"],
            vec!["slack-cli", "pin", "list", "-c", "C1"],
            vec!["slack-cli", "upload", "-c", "C1"],
            vec!["slack-cli", "download", "-i", "F1"],
            vec!["slack-cli", "canvas", "list", "-c", "C1"],
            vec!["slack-cli", "bookmark", "list"],
            vec!["slack-cli", "draft", "list"],
            vec!["slack-cli", "scheduled", "list"],
            vec!["slack-cli", "reminder", "list"],
        ] {
            Cli::try_parse_from(&argv).unwrap_or_else(|e| panic!("{argv:?} failed to parse: {e}"));
        }
    }

    #[test]
    fn global_options_reach_every_subcommand_depth() {
        let cli = Cli::try_parse_from([
            "slack-cli",
            "reminder",
            "add",
            "--text",
            "standup",
            "--after",
            "5",
            "--format",
            "yaml",
        ])
        .unwrap();
        assert_eq!(cli.global.output_format(), OutputFormat::Yaml);
    }

    #[test]
    fn a_subcommand_is_required() {
        let err = Cli::try_parse_from(["slack-cli"]).expect_err("a subcommand must be given");
        assert_eq!(
            err.kind(),
            ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
        );
    }
}
