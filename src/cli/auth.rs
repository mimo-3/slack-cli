//! `slack-cli auth` — トークンの疎通確認。検証は `auth.test` で行う。

use clap::{Args, Subcommand};
use serde_json::json;

use crate::cli::GlobalOpts;
use crate::client::SlackClient;
use crate::error::SlackCliError;
use crate::output;

#[derive(Args, Debug)]
pub struct AuthCommand {
    #[command(subcommand)]
    pub command: AuthSubcommand,
}

#[derive(Subcommand, Debug)]
pub enum AuthSubcommand {
    /// Call auth.test and print the full response
    Test,
    /// Show who the configured token belongs to
    Whoami,
}

pub async fn run(
    cmd: AuthCommand,
    client: &SlackClient,
    global: &GlobalOpts,
) -> Result<(), SlackCliError> {
    let response = client.auth_test().await?;
    let mut stdout = std::io::stdout();

    let value = match cmd.command {
        AuthSubcommand::Test => response,
        AuthSubcommand::Whoami => json!({
            "user": response.get("user"),
            "user_id": response.get("user_id"),
            "team": response.get("team"),
            "team_id": response.get("team_id"),
            "url": response.get("url"),
        }),
    };

    output::format_value(&value, global.output_format(), &mut stdout)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use url::Url;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;
    use crate::output::OutputFormat;

    fn client_for(server: &MockServer) -> SlackClient {
        SlackClient::new("test-token-value")
            .unwrap()
            .with_base_url(Url::parse(&format!("{}/", server.uri())).unwrap())
    }

    async fn mount_auth_test(server: &MockServer) {
        Mock::given(method("GET"))
            .and(path("/auth.test"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "url": "https://forward.slack.com/",
                "team": "forward",
                "user": "monet",
                "team_id": "T123",
                "user_id": "U123",
                "bot_id": "B123",
            })))
            .mount(server)
            .await;
    }

    #[tokio::test]
    async fn whoami_keeps_only_the_identity_fields() {
        let server = MockServer::start().await;
        mount_auth_test(&server).await;

        let response = client_for(&server).auth_test().await.unwrap();
        let whoami = json!({
            "user": response.get("user"),
            "user_id": response.get("user_id"),
            "team": response.get("team"),
            "team_id": response.get("team_id"),
            "url": response.get("url"),
        });

        assert_eq!(whoami["user_id"], "U123");
        assert!(whoami.get("bot_id").is_none());
    }

    #[tokio::test]
    async fn auth_test_output_is_rendered_in_the_requested_format() {
        let server = MockServer::start().await;
        mount_auth_test(&server).await;

        let response = client_for(&server).auth_test().await.unwrap();
        let mut buf = Vec::new();
        output::format_value(&response, OutputFormat::Json, &mut buf).unwrap();

        let rendered: serde_json::Value = serde_json::from_slice(&buf).unwrap();
        assert_eq!(rendered["team_id"], "T123");
    }

    #[tokio::test]
    async fn run_dispatches_both_subcommands() {
        let server = MockServer::start().await;
        mount_auth_test(&server).await;
        let client = client_for(&server);
        let global = GlobalOpts {
            json: true,
            ..GlobalOpts::default()
        };

        for command in [AuthSubcommand::Test, AuthSubcommand::Whoami] {
            run(AuthCommand { command }, &client, &global)
                .await
                .unwrap();
        }
        assert_eq!(server.received_requests().await.unwrap().len(), 2);
    }
}
