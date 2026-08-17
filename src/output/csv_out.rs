use std::io::Write;

use crate::error::SlackCliError;

/// JSON 値を CSV（区切り文字を変えれば TSV）として書き出す。
/// オブジェクトの配列は 1 行 1 オブジェクト、キーがヘッダになる。
/// 単一オブジェクトは 1 行。スカラーはそのまま 1 行で出す。
pub fn write_csv(
    value: &serde_json::Value,
    writer: &mut dyn Write,
    delimiter: u8,
) -> Result<(), SlackCliError> {
    let rows: Vec<&serde_json::Value> = match value {
        serde_json::Value::Array(arr) => arr.iter().collect(),
        obj @ serde_json::Value::Object(_) => vec![obj],
        scalar => {
            writeln!(writer, "{}", value_to_string(scalar))?;
            return Ok(());
        }
    };

    if rows.is_empty() {
        return Ok(());
    }

    // 全行のキーを初出順に集める
    let mut headers: Vec<String> = Vec::new();
    for row in &rows {
        if let serde_json::Value::Object(map) = row {
            for key in map.keys() {
                if !headers.contains(key) {
                    headers.push(key.clone());
                }
            }
        }
    }

    let mut csv_writer = csv::WriterBuilder::new()
        .delimiter(delimiter)
        .from_writer(writer);

    csv_writer
        .write_record(&headers)
        .map_err(|e| SlackCliError::Configuration(format!("CSV write error: {e}")))?;

    for row in &rows {
        let fields: Vec<String> = headers
            .iter()
            .map(|h| row.get(h).map(value_to_string).unwrap_or_default())
            .collect();
        csv_writer
            .write_record(&fields)
            .map_err(|e| SlackCliError::Configuration(format!("CSV write error: {e}")))?;
    }

    csv_writer
        .flush()
        .map_err(|e| SlackCliError::Configuration(format!("CSV flush error: {e}")))?;

    Ok(())
}

fn value_to_string(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Null => String::new(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Number(n) => n.to_string(),
        other => serde_json::to_string(other).unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn render(value: &serde_json::Value, delimiter: u8) -> String {
        let mut buf = Vec::new();
        write_csv(value, &mut buf, delimiter).unwrap();
        String::from_utf8(buf).unwrap()
    }

    #[test]
    fn unions_headers_across_rows_in_first_seen_order() {
        let out = render(
            &json!([
                { "id": "C1", "name": "general" },
                { "id": "C2", "is_private": true },
            ]),
            b',',
        );
        assert_eq!(out, "id,name,is_private\nC1,general,\nC2,,true\n");
    }

    #[test]
    fn tsv_uses_tab_delimiter() {
        let out = render(&json!({ "id": "C1", "name": "general" }), b'\t');
        assert_eq!(out, "id\tname\nC1\tgeneral\n");
    }
}
