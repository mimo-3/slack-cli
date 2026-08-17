use std::io::Write;

use comfy_table::{ContentArrangement, Table};

use crate::error::SlackCliError;

/// JSON のオブジェクト配列を ASCII テーブルとして描画する。
pub fn write_table(value: &serde_json::Value, writer: &mut dyn Write) -> Result<(), SlackCliError> {
    let rows: Vec<&serde_json::Value> = match value {
        serde_json::Value::Array(arr) => arr.iter().collect(),
        obj @ serde_json::Value::Object(_) => vec![obj],
        scalar => {
            writeln!(writer, "{scalar}")?;
            return Ok(());
        }
    };

    if rows.is_empty() {
        writeln!(writer, "(no results)")?;
        return Ok(());
    }

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

    // オブジェクトが 1 つも無い（スカラーの配列）場合は 1 列で並べる
    if headers.is_empty() {
        for row in &rows {
            writeln!(writer, "{}", cell_to_string(row))?;
        }
        return Ok(());
    }

    let mut table = Table::new();
    table.set_content_arrangement(ContentArrangement::Dynamic);
    table.set_header(&headers);

    for row in &rows {
        let cells: Vec<String> = headers
            .iter()
            .map(|h| row.get(h).map(cell_to_string).unwrap_or_default())
            .collect();
        table.add_row(cells);
    }

    writeln!(writer, "{table}")?;
    Ok(())
}

fn cell_to_string(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Null => String::new(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn render(value: &serde_json::Value) -> String {
        let mut buf = Vec::new();
        write_table(value, &mut buf).unwrap();
        String::from_utf8(buf).unwrap()
    }

    #[test]
    fn renders_headers_and_rows() {
        let out = render(&json!([
            { "id": "C1", "name": "general" },
            { "id": "C2", "name": "random" },
        ]));
        assert!(out.contains("id"), "{out}");
        assert!(out.contains("name"), "{out}");
        assert!(out.contains("general"), "{out}");
        assert!(out.contains("random"), "{out}");
    }

    #[test]
    fn empty_array_reports_no_results() {
        assert_eq!(render(&json!([])), "(no results)\n");
    }

    #[test]
    fn scalar_array_renders_one_value_per_line() {
        assert_eq!(render(&json!(["U1", "U2"])), "U1\nU2\n");
    }
}
