use std::path::Path;

use toml::Value;

pub struct CargoContext {
    pub package_name: String,
    pub version: String,
    pub edition: String,
    pub workspace_members: Vec<String>,
    pub direct_deps: Vec<(String, String)>,
}

impl CargoContext {
    pub fn load(root: &Path) -> Option<Self> {
        let toml_path = root.join("Cargo.toml");
        let contents = std::fs::read_to_string(&toml_path).ok()?;
        let value: Value = toml::from_str(&contents).ok()?;

        let table = value.as_table()?;

        let (package_name, version, edition) =
            if let Some(pkg) = table.get("package").and_then(Value::as_table) {
                let name = pkg
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let ver = pkg
                    .get("version")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let ed = pkg
                    .get("edition")
                    .and_then(Value::as_str)
                    .unwrap_or("2021")
                    .to_string();
                (name, ver, ed)
            } else if table.contains_key("workspace") {
                let name = root
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("")
                    .to_string();
                (name, String::new(), String::new())
            } else {
                (String::new(), String::new(), String::new())
            };

        let workspace_members = table
            .get("workspace")
            .and_then(Value::as_table)
            .and_then(|ws| ws.get("members"))
            .and_then(Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .take(10)
                    .collect()
            })
            .unwrap_or_default();

        let mut direct_deps: Vec<(String, String)> = table
            .get("dependencies")
            .and_then(Value::as_table)
            .map(|deps| {
                let mut pairs: Vec<(String, String)> = deps
                    .iter()
                    .map(|(name, spec)| {
                        let ver = match spec {
                            Value::String(s) => s.clone(),
                            Value::Table(t) => t
                                .get("version")
                                .and_then(Value::as_str)
                                .unwrap_or("")
                                .to_string(),
                            _ => String::new(),
                        };
                        (name.clone(), ver)
                    })
                    .collect();
                pairs.sort_by(|a, b| a.0.cmp(&b.0));
                pairs
            })
            .unwrap_or_default();

        if direct_deps.len() > 20 {
            direct_deps.truncate(20);
        }

        Some(Self {
            package_name,
            version,
            edition,
            workspace_members,
            direct_deps,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as IoWrite;

    fn load_from_str(toml_str: &str) -> Option<CargoContext> {
        let dir = tempfile::tempdir().ok()?;
        let path = dir.path().join("Cargo.toml");
        let mut f = std::fs::File::create(&path).ok()?;
        f.write_all(toml_str.as_bytes()).ok()?;
        drop(f);
        CargoContext::load(dir.path())
    }

    #[test]
    fn package_fields_populated() {
        let ctx = load_from_str(
            r#"
[package]
name = "myapp"
version = "1.2.3"
edition = "2021"

[dependencies]
serde = "1.0"
tokio = { version = "1.38", features = ["full"] }
"#,
        )
        .unwrap();
        assert_eq!(ctx.package_name, "myapp");
        assert_eq!(ctx.version, "1.2.3");
        assert_eq!(ctx.edition, "2021");
        assert!(ctx.workspace_members.is_empty());
        assert_eq!(ctx.direct_deps.len(), 2);
        let serde = ctx.direct_deps.iter().find(|(n, _)| n == "serde").unwrap();
        assert_eq!(serde.1, "1.0");
        let tokio = ctx.direct_deps.iter().find(|(n, _)| n == "tokio").unwrap();
        assert_eq!(tokio.1, "1.38");
    }

    #[test]
    fn workspace_root_no_package() {
        let ctx = load_from_str(
            r#"
[workspace]
members = ["crate-a", "crate-b", "crate-c"]
"#,
        )
        .unwrap();
        // package_name is derived from directory (tempdir name varies; just check it's non-empty)
        assert!(!ctx.package_name.is_empty());
        assert!(ctx.version.is_empty());
        assert_eq!(ctx.workspace_members, vec!["crate-a", "crate-b", "crate-c"]);
    }

    #[test]
    fn deps_capped_at_20() {
        let mut lines = String::from("[package]\nname=\"x\"\nversion=\"0.1\"\n\n[dependencies]\n");
        for i in 0..25 {
            lines.push_str(&format!("dep{i:02} = \"1.0\"\n"));
        }
        let ctx = load_from_str(&lines).unwrap();
        assert_eq!(ctx.direct_deps.len(), 20);
    }

    #[test]
    fn workspace_members_capped_at_10() {
        let members: Vec<String> = (0..15).map(|i| format!("\"crate-{i}\"")).collect();
        let toml = format!("[workspace]\nmembers = [{}]\n", members.join(","));
        let ctx = load_from_str(&toml).unwrap();
        assert_eq!(ctx.workspace_members.len(), 10);
    }

    #[test]
    fn table_dep_version_extracted() {
        let ctx = load_from_str(
            r#"
[package]
name = "t"
version = "0.1"

[dependencies]
clap = { version = "4.5", features = ["derive"] }
"#,
        )
        .unwrap();
        let clap = ctx.direct_deps.iter().find(|(n, _)| n == "clap").unwrap();
        assert_eq!(clap.1, "4.5");
    }

    #[test]
    fn missing_cargo_toml_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        assert!(CargoContext::load(dir.path()).is_none());
    }
}
