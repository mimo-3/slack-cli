//! 出力フォーマッタ。
//!
//! notion-cli から `json` / `yaml` / `csv` / `table` を流用した。
//! `plain` と `markdown` は Notion のブロック構造に密着していたため移植していない。
//! notion-cli では `table.rs` が `OutputFormat` に未接続のデッドコードだったが、
//! ここでは `Table` を既定フォーマットとして配線してある（TS 版の既定も `table`）。

pub mod csv_out;
pub mod json;
pub mod sanitize;
pub mod table;
pub mod yaml;

use std::fmt;
use std::io::Write;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::error::SlackCliError;

/// `--format id-only` が識別子として拾うキー。先頭から順に見て最初に見つかったものを使う。
/// Slack の識別子はチャンネル/ユーザーなら `id`、メッセージなら `ts` と分かれる。
const ID_KEYS: [&str; 3] = ["id", "ts", "user"];

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OutputFormat {
    #[default]
    Table,
    Json,
    Yaml,
    Csv,
    Tsv,
    IdOnly,
}

impl fmt::Display for OutputFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            OutputFormat::Table => "table",
            OutputFormat::Json => "json",
            OutputFormat::Yaml => "yaml",
            OutputFormat::Csv => "csv",
            OutputFormat::Tsv => "tsv",
            OutputFormat::IdOnly => "id-only",
        };
        write!(f, "{s}")
    }
}

impl FromStr for OutputFormat {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "table" => Ok(OutputFormat::Table),
            "json" => Ok(OutputFormat::Json),
            "yaml" | "yml" => Ok(OutputFormat::Yaml),
            "csv" => Ok(OutputFormat::Csv),
            "tsv" => Ok(OutputFormat::Tsv),
            "id-only" | "id" | "ids" => Ok(OutputFormat::IdOnly),
            _ => Err(format!("Unknown output format: {s}")),
        }
    }
}

/// JSON 値を指定フォーマットで書き出す。
pub fn format_value(
    value: &serde_json::Value,
    format: OutputFormat,
    writer: &mut dyn Write,
) -> Result<(), SlackCliError> {
    match format {
        OutputFormat::Table => table::write_table(value, writer),
        OutputFormat::Json => json::write_json(value, writer),
        OutputFormat::Yaml => yaml::write_yaml(value, writer),
        OutputFormat::Csv => csv_out::write_csv(value, writer, b','),
        OutputFormat::Tsv => csv_out::write_csv(value, writer, b'\t'),
        OutputFormat::IdOnly => write_ids(value, writer),
    }
}

fn write_ids(value: &serde_json::Value, writer: &mut dyn Write) -> Result<(), SlackCliError> {
    match value {
        serde_json::Value::Array(items) => {
            for item in items {
                if let Some(id) = extract_id(item) {
                    writeln!(writer, "{id}")?;
                }
            }
        }
        other => {
            if let Some(id) = extract_id(other) {
                writeln!(writer, "{id}")?;
            }
        }
    }
    Ok(())
}

fn extract_id(value: &serde_json::Value) -> Option<&str> {
    ID_KEYS
        .iter()
        .find_map(|key| value.get(key).and_then(serde_json::Value::as_str))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn render(value: &serde_json::Value, format: OutputFormat) -> String {
        let mut buf = Vec::new();
        format_value(value, format, &mut buf).unwrap();
        String::from_utf8(buf).unwrap()
    }

    #[test]
    fn parses_format_aliases() {
        assert_eq!("yml".parse::<OutputFormat>().unwrap(), OutputFormat::Yaml);
        assert_eq!("IDS".parse::<OutputFormat>().unwrap(), OutputFormat::IdOnly);
        assert!("plain".parse::<OutputFormat>().is_err());
        assert_eq!(OutputFormat::default(), OutputFormat::Table);
    }

    #[test]
    fn table_is_wired_into_format_value() {
        let out = render(
            &json!([{ "id": "C1", "name": "general" }]),
            OutputFormat::Table,
        );
        assert!(out.contains("general"), "table output was: {out}");
        assert!(
            out.contains('┆') || out.contains('|'),
            "table output was: {out}"
        );
    }

    #[test]
    fn id_only_falls_back_from_id_to_ts() {
        let out = render(
            &json!([
                { "id": "C123", "name": "general" },
                { "ts": "1700000000.000100", "text": "hi" },
                { "text": "no identifier" },
            ]),
            OutputFormat::IdOnly,
        );
        assert_eq!(out, "C123\n1700000000.000100\n");
    }

    #[test]
    fn json_and_yaml_round_trip_the_same_value() {
        let value = json!({ "ok": true, "channel": "C1" });
        let from_json: serde_json::Value =
            serde_json::from_str(&render(&value, OutputFormat::Json)).unwrap();
        let from_yaml: serde_json::Value =
            serde_yaml_ng::from_str(&render(&value, OutputFormat::Yaml)).unwrap();
        assert_eq!(from_json, value);
        assert_eq!(from_yaml, value);
    }
}
