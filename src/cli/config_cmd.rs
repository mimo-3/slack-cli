//! `slack-cli config` — 設定とプロファイルの管理。Slack Web API は一切呼ばない。

use std::io::{IsTerminal, Read, Write};

use clap::{Args, Subcommand};
use colored::Colorize;
use serde_json::json;

use crate::cli::GlobalOpts;
use crate::config::{mask_token, no_config_message, ProfileConfigManager};
use crate::error::SlackCliError;
use crate::output;

pub const ERR_BOTH_TOKEN_SOURCES: &str = "Cannot use --token and --token-stdin together";
pub const ERR_EMPTY_STDIN: &str = "No token received from stdin";
pub const ERR_EMPTY_PROMPT: &str = "Token cannot be empty";
pub const ERR_NO_TOKEN: &str = "No token provided. Use --token-stdin, set SLACK_CLI_TOKEN, \
                                or run this command in an interactive terminal.";
pub const WARN_TOKEN_FLAG: &str = "Warning: --token may leak secrets via shell history/process \
                                   list. Prefer --token-stdin or interactive input.";
pub const MSG_NO_PROFILES: &str =
    "No profiles found. Use \"slack-cli config set --token <token>\" to create one.";

const TOKEN_ENV_VAR: &str = "SLACK_CLI_TOKEN";

#[derive(Args, Debug)]
pub struct ConfigCommand {
    #[command(subcommand)]
    pub command: ConfigSubcommand,
}

#[derive(Subcommand, Debug)]
pub enum ConfigSubcommand {
    /// Set the API token for a profile
    Set {
        /// Read the Slack API token from stdin
        #[arg(long)]
        token_stdin: bool,
    },
    /// Show the configuration of a profile
    Get,
    /// List all profiles
    Profiles,
    /// Switch to a different profile
    Use {
        /// Profile name to switch to
        profile: String,
    },
    /// Show the current active profile
    Current,
    /// Clear the configuration of a profile
    Clear,
}

pub async fn run(cmd: ConfigCommand, global: &GlobalOpts) -> Result<(), SlackCliError> {
    let manager = ProfileConfigManager::new()?;
    let mut stdout = std::io::stdout();

    match cmd.command {
        ConfigSubcommand::Set { token_stdin } => {
            let token = resolve_token_input(global.token.as_deref(), token_stdin)?;
            let profile = manager.set_token(&token, global.profile.as_deref())?;
            writeln!(
                stdout,
                "{}",
                format!("✓ Token saved successfully for profile \"{profile}\"").green()
            )?;
        }

        ConfigSubcommand::Get => {
            let profile = global
                .profile
                .clone()
                .unwrap_or(manager.current_profile()?);

            match manager.get_config(Some(&profile))? {
                None => {
                    writeln!(stdout, "{}", no_config_message(&profile).yellow())?;
                }
                Some(config) => {
                    let value = json!({
                        "profile": profile,
                        "token": mask_token(&config.token),
                        "updatedAt": config.updated_at,
                    });
                    output::format_value(&value, global.output_format(), &mut stdout)?;
                }
            }
        }

        ConfigSubcommand::Profiles => {
            let profiles = manager.list_profiles()?;
            if profiles.is_empty() {
                writeln!(stdout, "{}", MSG_NO_PROFILES.yellow())?;
            } else {
                // 注意: ここに出るトークンは保存されている暗号文をマスクしたもの
                // （TS 版も listProfiles は復号しない）。`config get` の表示とは形が違う。
                let value = serde_json::Value::Array(
                    profiles
                        .iter()
                        .map(|p| {
                            json!({
                                "profile": p.name,
                                "default": p.is_default,
                                "token": mask_token(&p.token),
                                "updatedAt": p.updated_at,
                            })
                        })
                        .collect(),
                );
                output::format_value(&value, global.output_format(), &mut stdout)?;
            }
        }

        ConfigSubcommand::Use { profile } => {
            manager.use_profile(&profile)?;
            writeln!(
                stdout,
                "{}",
                format!("✓ Switched to profile \"{profile}\"").green()
            )?;
        }

        ConfigSubcommand::Current => {
            let value = json!({ "profile": manager.current_profile()? });
            output::format_value(&value, global.output_format(), &mut stdout)?;
        }

        ConfigSubcommand::Clear => {
            let profile = manager.clear_config(global.profile.as_deref())?;
            writeln!(
                stdout,
                "{}",
                format!("✓ Profile \"{profile}\" cleared successfully").green()
            )?;
        }
    }

    Ok(())
}

/// `config set` のトークン解決。優先順に評価する。
/// 1. `--token` と `--token-stdin` の同時指定はエラー
/// 2. `--token-stdin`（EOF まで読んで trim）
/// 3. `--token`（stderr に警告を出す）
/// 4. 環境変数 `SLACK_CLI_TOKEN`
/// 5. 対話プロンプト（TTY が必要）
fn resolve_token_input(
    flag_token: Option<&str>,
    token_stdin: bool,
) -> Result<String, SlackCliError> {
    if flag_token.is_some() && token_stdin {
        return Err(SlackCliError::Validation(ERR_BOTH_TOKEN_SOURCES.into()));
    }

    if token_stdin {
        let mut buffer = String::new();
        std::io::stdin().read_to_string(&mut buffer)?;
        return non_empty(&buffer, ERR_EMPTY_STDIN);
    }

    if let Some(token) = flag_token {
        eprintln!("{}", WARN_TOKEN_FLAG.yellow());
        return non_empty(token, ERR_EMPTY_PROMPT);
    }

    if let Ok(token) = std::env::var(TOKEN_ENV_VAR) {
        if !token.trim().is_empty() {
            return Ok(token.trim().to_string());
        }
    }

    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        return Err(SlackCliError::Validation(ERR_NO_TOKEN.into()));
    }

    let entered = dialoguer::Password::new()
        .with_prompt("Slack API token")
        .interact()
        .map_err(|_| SlackCliError::Validation("Token input cancelled".into()))?;
    non_empty(&entered, ERR_EMPTY_PROMPT)
}

fn non_empty(value: &str, message: &str) -> Result<String, SlackCliError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(SlackCliError::Validation(message.to_string()));
    }
    Ok(trimmed.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    use crate::cli::Cli;

    #[test]
    fn parses_every_config_subcommand() {
        for argv in [
            vec!["slack-cli", "config", "set", "--token-stdin"],
            vec!["slack-cli", "config", "get"],
            vec!["slack-cli", "config", "profiles"],
            vec!["slack-cli", "config", "use", "work"],
            vec!["slack-cli", "config", "current"],
            vec!["slack-cli", "config", "clear", "--profile", "work"],
        ] {
            Cli::try_parse_from(&argv).unwrap_or_else(|e| panic!("{argv:?} failed to parse: {e}"));
        }
    }

    #[test]
    fn config_use_requires_a_profile_argument() {
        let err = Cli::try_parse_from(["slack-cli", "config", "use"])
            .expect_err("the profile argument is required");
        assert_eq!(
            err.kind(),
            clap::error::ErrorKind::MissingRequiredArgument
        );
    }

    #[test]
    fn token_flag_and_token_stdin_cannot_be_combined() {
        let err = resolve_token_input(Some("t"), true).unwrap_err();
        assert_eq!(err.to_string(), ERR_BOTH_TOKEN_SOURCES);
    }

    #[test]
    fn token_flag_is_trimmed() {
        assert_eq!(resolve_token_input(Some("  abc  "), false).unwrap(), "abc");
    }

    #[test]
    fn blank_values_are_rejected() {
        assert_eq!(
            non_empty("   ", ERR_EMPTY_STDIN).unwrap_err().to_string(),
            ERR_EMPTY_STDIN
        );
        assert_eq!(non_empty(" abc \n", ERR_EMPTY_STDIN).unwrap(), "abc");
    }
}
