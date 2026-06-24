#[derive(Debug, Clone)]
pub struct DiagnosticMessage {
    pub level: String,
    pub message: String,
    pub file_name: Option<String>,
    pub line_start: Option<u32>,
    pub rendered: Option<String>,
}

pub fn parse_diagnostics(output: &str) -> Vec<DiagnosticMessage> {
    output
        .lines()
        .filter_map(|line| {
            let obj: serde_json::Value = serde_json::from_str(line).ok()?;
            if obj["reason"].as_str() != Some("compiler-message") {
                return None;
            }
            let msg = &obj["message"];
            let level = msg["level"].as_str()?.to_string();
            if level != "error" {
                return None;
            }
            let message = msg["message"].as_str()?.to_string();
            let rendered = msg["rendered"].as_str().map(|s| s.to_string());
            let (file_name, line_start) = if let Some(spans) = msg["spans"].as_array() {
                let primary = spans
                    .iter()
                    .find(|s| s["is_primary"].as_bool() == Some(true));
                if let Some(span) = primary {
                    let file = span["file_name"].as_str().map(|s| s.to_string());
                    let line = span["line_start"].as_u64().map(|n| n as u32);
                    (file, line)
                } else {
                    (None, None)
                }
            } else {
                (None, None)
            };
            Some(DiagnosticMessage {
                level,
                message,
                file_name,
                line_start,
                rendered,
            })
        })
        .collect()
}

pub(crate) fn parse_ruff_diagnostics(output: &str) -> Vec<DiagnosticMessage> {
    let arr = match serde_json::from_str::<serde_json::Value>(output) {
        Ok(serde_json::Value::Array(a)) => a,
        _ => return vec![],
    };
    arr.into_iter()
        .filter_map(|obj| {
            let file_name = obj["filename"].as_str().map(|s| s.to_string());
            let raw_msg = obj["message"].as_str().unwrap_or("").to_string();
            if file_name.is_none() && raw_msg.is_empty() {
                return None;
            }
            let code = obj["code"].as_str().unwrap_or("");
            let message = if code.is_empty() {
                raw_msg
            } else {
                format!("{code}: {raw_msg}")
            };
            let line_start = obj["location"]["row"].as_u64().map(|r| r as u32);
            Some(DiagnosticMessage {
                level: "warning".to_string(),
                message,
                file_name,
                line_start,
                rendered: None,
            })
        })
        .collect()
}

pub fn format_diagnostics(diagnostics: &[DiagnosticMessage]) -> String {
    if diagnostics.is_empty() {
        return String::new();
    }
    let joined: String = diagnostics
        .iter()
        .map(|d| match (&d.file_name, d.line_start) {
            (Some(file), Some(line)) => format!("{}:{}: {}", file, line, d.message),
            (Some(file), None) => format!("{}: {}", file, d.message),
            (None, _) => d.message.clone(),
        })
        .collect::<Vec<_>>()
        .join("\n");

    if joined.len() <= 2000 {
        return joined;
    }
    let cut = joined[..2000].rfind('\n').unwrap_or(0);
    format!("{}\n... (truncated)", &joined[..cut])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn compiler_message_json(level: &str, message: &str, file: &str, line: u64) -> String {
        format!(
            r#"{{"reason":"compiler-message","package_id":"test","manifest_path":"/tmp/Cargo.toml","target":{{"kind":["lib"],"name":"test"}},"message":{{"rendered":"rendered text","children":[],"code":null,"level":"{level}","message":"{message}","spans":[{{"byte_end":10,"byte_start":0,"column_end":5,"column_start":1,"expansion":null,"file_name":"{file}","is_primary":true,"label":null,"line_end":{line},"line_start":{line},"suggested_replacement":null,"suggestion_applicability":null,"text":[]}}]}}}}"#
        )
    }

    #[test]
    fn parse_diagnostics_returns_error_entry() {
        let json = compiler_message_json("error", "mismatched types", "src/main.rs", 42);
        let result = parse_diagnostics(&json);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].level, "error");
        assert_eq!(result[0].message, "mismatched types");
        assert_eq!(result[0].file_name.as_deref(), Some("src/main.rs"));
        assert_eq!(result[0].line_start, Some(42));
    }

    #[test]
    fn parse_diagnostics_filters_warnings() {
        let json = compiler_message_json("warning", "unused variable", "src/main.rs", 10);
        let result = parse_diagnostics(&json);
        assert!(result.is_empty(), "warnings must be filtered out");
    }

    #[test]
    fn parse_diagnostics_skips_non_json_lines() {
        let output =
            "   Compiling myproject v0.1.0\n    Finished dev [unoptimized] target(s) in 1.23s\n";
        let result = parse_diagnostics(output);
        assert!(
            result.is_empty(),
            "non-JSON cargo boilerplate must produce no diagnostics"
        );
    }

    #[test]
    fn format_diagnostics_produces_file_line_message() {
        let diagnostics = vec![
            DiagnosticMessage {
                level: "error".into(),
                message: "mismatched types".into(),
                file_name: Some("src/lib.rs".into()),
                line_start: Some(7),
                rendered: None,
            },
            DiagnosticMessage {
                level: "error".into(),
                message: "cannot borrow as mutable".into(),
                file_name: Some("src/main.rs".into()),
                line_start: Some(23),
                rendered: None,
            },
        ];
        let out = format_diagnostics(&diagnostics);
        assert_eq!(
            out,
            "src/lib.rs:7: mismatched types\nsrc/main.rs:23: cannot borrow as mutable"
        );
    }

    #[test]
    fn format_diagnostics_empty_input_returns_empty_string() {
        assert_eq!(format_diagnostics(&[]), "");
    }

    fn ruff_message_json(code: &str, message: &str, file: &str, row: u64) -> String {
        format!(
            r#"[{{"code":"{code}","message":"{message}","filename":"{file}","location":{{"row":{row},"column":1}},"end_location":{{"row":{row},"column":5}},"fix":null,"noqa_row":{row}}}]"#
        )
    }

    #[test]
    fn parse_ruff_diagnostics_single_violation() {
        let json = ruff_message_json("E225", "msg", "src/main.py", 42);
        let result = parse_ruff_diagnostics(&json);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].level, "warning");
        assert_eq!(result[0].message, "E225: msg");
        assert_eq!(result[0].file_name.as_deref(), Some("src/main.py"));
        assert_eq!(result[0].line_start, Some(42));
    }

    #[test]
    fn parse_ruff_diagnostics_empty_array() {
        let result = parse_ruff_diagnostics("[]");
        assert!(result.is_empty());
    }

    #[test]
    fn parse_ruff_diagnostics_invalid_json() {
        let result = parse_ruff_diagnostics("not json");
        assert!(result.is_empty());
    }
}
