use std::io::Write;

use crate::error::SlackCliError;

/// JSON をインデント2スペースで書き出す（TypeScript 版 `JSON.stringify(v, null, 2)` と同じ）。
pub fn write_json(value: &serde_json::Value, writer: &mut dyn Write) -> Result<(), SlackCliError> {
    let output = serde_json::to_string_pretty(value)?;
    writeln!(writer, "{output}")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn writes_pretty_json_with_trailing_newline() {
        let mut buf = Vec::new();
        write_json(&json!({ "ok": true }), &mut buf).unwrap();
        assert_eq!(String::from_utf8(buf).unwrap(), "{\n  \"ok\": true\n}\n");
    }
}
