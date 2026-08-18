use std::io::Write;

use crate::error::SlackCliError;

/// YAML を書き出す。serde_yaml はメンテ終了済みのため後継の serde_yaml_ng を使う。
pub fn write_yaml(value: &serde_json::Value, writer: &mut dyn Write) -> Result<(), SlackCliError> {
    let output = serde_yaml_ng::to_string(value)
        .map_err(|e| SlackCliError::Configuration(format!("YAML serialization error: {e}")))?;
    write!(writer, "{output}")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn writes_yaml_mapping() {
        let mut buf = Vec::new();
        write_yaml(&json!({ "channel": "C1", "ok": true }), &mut buf).unwrap();
        let out = String::from_utf8(buf).unwrap();
        assert!(out.contains("channel: C1"), "yaml output was: {out}");
        assert!(out.contains("ok: true"), "yaml output was: {out}");
    }
}
