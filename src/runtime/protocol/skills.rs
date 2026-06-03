use std::path::Path;

#[derive(Debug, Clone, Default)]
pub struct SkillContent {
    pub name: String,
    pub description: String,
    pub style_instructions: String,
}

// Bundled default skill content — always available even if
// .thunk/skills/ is empty. Disk files take priority over these.

const BUNDLED_CONCISE: &str = r#"# concise

## Description
Shortest correct answer. No padding, no restatement, no filler.

## Style Instructions
Answer the question directly. Use the minimum words needed for
correctness. Do not restate the question. Do not add caveats
unless they change the answer. Do not summarize at the end.
If a one-sentence answer is correct, give one sentence.
"#;

const BUNDLED_THOROUGH: &str = r#"# thorough

## Description
Comprehensive coverage. Anticipate follow-up questions. Show work.

## Style Instructions
Cover the full answer including edge cases and alternatives.
Show reasoning steps, not just conclusions. Anticipate the next
question and address it. Flag assumptions explicitly. Prefer
slightly more than slightly less — gaps are harder to fix than
length.
"#;

const BUNDLED_EDUCATIONAL: &str = r#"# educational

## Description
Explain reasoning, define terms, build understanding step by step.

## Style Instructions
Explain why, not just what. Define terms before using them.
Build from what the reader knows toward what they don't. Use
concrete examples before abstract principles. Check that the
explanation actually transfers understanding, not just information.
"#;

const BUNDLED_CRITICAL: &str = r#"# critical

## Description
Surface weaknesses, question assumptions, steelman then counter.

## Style Instructions
Identify the strongest version of the argument or approach first.
Then surface its weaknesses, hidden assumptions, and failure modes.
Do not soften real problems. Distinguish between fatal flaws and
acceptable tradeoffs. Be direct — hedging criticism is not helpful.
"#;

const BUNDLED_CREATIVE: &str = r#"# creative

## Description
Explore alternatives, reframe the problem, surface novel angles.

## Style Instructions
Do not default to the obvious approach. Explore at least two
alternative framings before settling. Question whether the problem
as stated is the real problem. Surface non-obvious constraints and
non-obvious freedoms. Prefer unexpected useful ideas over expected
safe ones.
"#;

fn parse_skill_md(content: &str, name: &str) -> SkillContent {
    let mut result = SkillContent {
        name: name.to_string(),
        ..Default::default()
    };

    let mut sections: [(&str, &mut String); 2] = [
        ("## Description", &mut result.description),
        ("## Style Instructions", &mut result.style_instructions),
    ];

    let lines: Vec<&str> = content.lines().collect();
    for (header, field) in &mut sections {
        let mut found = false;
        let mut buf = Vec::new();
        for line in &lines {
            if *line == *header {
                found = true;
                continue;
            }
            if found {
                if line.starts_with("## ") {
                    break;
                }
                buf.push(*line);
            }
        }
        if found {
            **field = buf.join("\n").trim().to_string();
        } else {
            eprintln!(
                "[thunk] skill '{}': missing section '{}', using empty string",
                name, header
            );
        }
    }
    result
}

pub struct SkillLoader;

impl SkillLoader {
    /// Load a skill by name. Checks `.thunk/skills/<name>.md`
    /// first; falls back to bundled defaults. Returns `Err` if the
    /// name is not recognized and no file exists.
    pub fn load(name: &str, thunk_dir: &Path) -> Result<SkillContent, String> {
        let path = thunk_dir.join("skills").join(format!("{name}.md"));
        if path.exists() {
            match std::fs::read_to_string(&path) {
                Ok(content) => return Ok(parse_skill_md(&content, name)),
                Err(e) => eprintln!("[thunk] skill '{}': failed to read {:?}: {}", name, path, e),
            }
        }
        match name {
            "concise" => Ok(parse_skill_md(BUNDLED_CONCISE, name)),
            "thorough" => Ok(parse_skill_md(BUNDLED_THOROUGH, name)),
            "educational" => Ok(parse_skill_md(BUNDLED_EDUCATIONAL, name)),
            "critical" => Ok(parse_skill_md(BUNDLED_CRITICAL, name)),
            "creative" => Ok(parse_skill_md(BUNDLED_CREATIVE, name)),
            _ => Err(format!(
                "unknown skill '{}' — available: concise, thorough, educational, critical, creative",
                name
            )),
        }
    }

    /// List all available skill names: bundled defaults plus any
    /// `.md` files in `.thunk/skills/` not already in the bundled set.
    pub fn list_available(thunk_dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = vec![
            "concise".into(),
            "thorough".into(),
            "educational".into(),
            "critical".into(),
            "creative".into(),
        ];
        let skills_dir = thunk_dir.join("skills");
        if let Ok(entries) = std::fs::read_dir(&skills_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) == Some("md") {
                    if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                        if !names.contains(&stem.to_string()) {
                            names.push(stem.to_string());
                        }
                    }
                }
            }
        }
        names.sort();
        names
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::*;

    const FULL_MD: &str = r#"# test-skill

## Description
A skill for testing purposes.

## Style Instructions
Do the test thing correctly.
"#;

    #[test]
    fn parse_all_sections_present() {
        let result = parse_skill_md(FULL_MD, "test-skill");
        assert_eq!(result.name, "test-skill");
        assert!(
            !result.description.is_empty(),
            "description must be populated"
        );
        assert!(
            !result.style_instructions.is_empty(),
            "style_instructions must be populated"
        );
    }

    #[test]
    fn parse_missing_section_no_panic() {
        let md = "# partial\n\n## Description\nSome description.\n";
        let result = parse_skill_md(md, "partial");
        assert!(!result.description.is_empty());
        // Missing Style Instructions — empty string, not a panic.
        assert!(result.style_instructions.is_empty());
    }

    #[test]
    fn load_returns_bundled_when_no_file() {
        let dir = tempdir().unwrap();
        // skills/ subdir does not exist — must fall back to bundled.
        let result = SkillLoader::load("concise", dir.path());
        let content = result.expect("bundled concise must load");
        assert_eq!(content.name, "concise");
        assert!(!content.description.is_empty());
        assert!(!content.style_instructions.is_empty());
    }

    #[test]
    fn load_returns_error_for_unknown() {
        let dir = tempdir().unwrap();
        let result = SkillLoader::load("nonexistent", dir.path());
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("unknown skill"));
    }

    #[test]
    fn load_prefers_disk_file_over_bundled() {
        let dir = tempdir().unwrap();
        let skills_dir = dir.path().join("skills");
        fs::create_dir_all(&skills_dir).unwrap();
        fs::write(
            skills_dir.join("concise.md"),
            "# concise\n## Description\nCustom description.\n## Style Instructions\nCustom instructions.\n",
        )
        .unwrap();

        let result = SkillLoader::load("concise", dir.path()).unwrap();
        assert!(
            result.description.contains("Custom"),
            "disk file must override bundled content"
        );
    }

    #[test]
    fn list_available_includes_bundled() {
        let dir = tempdir().unwrap();
        let names = SkillLoader::list_available(dir.path());
        assert!(names.contains(&"concise".to_string()));
        assert!(names.contains(&"thorough".to_string()));
        assert!(names.contains(&"educational".to_string()));
        assert!(names.contains(&"critical".to_string()));
        assert!(names.contains(&"creative".to_string()));
    }

    #[test]
    fn list_available_includes_custom() {
        let dir = tempdir().unwrap();
        let skills_dir = dir.path().join("skills");
        fs::create_dir_all(&skills_dir).unwrap();
        fs::write(skills_dir.join("custom.md"), "# custom\n").unwrap();

        let names = SkillLoader::list_available(dir.path());
        assert!(names.contains(&"custom".to_string()));
        // Bundled names still present.
        assert!(names.contains(&"concise".to_string()));
    }

    #[test]
    fn list_available_excludes_non_md_files() {
        let dir = tempdir().unwrap();
        let skills_dir = dir.path().join("skills");
        fs::create_dir_all(&skills_dir).unwrap();
        fs::write(skills_dir.join(".gitkeep"), "").unwrap();
        fs::write(skills_dir.join("readme.txt"), "").unwrap();

        let names = SkillLoader::list_available(dir.path());
        assert!(!names.contains(&".gitkeep".to_string()));
        assert!(!names.contains(&"readme".to_string()));
    }

    #[test]
    fn list_available_is_sorted() {
        let dir = tempdir().unwrap();
        let names = SkillLoader::list_available(dir.path());
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted, "list_available must return sorted names");
    }

    #[test]
    fn all_bundled_skills_parse_without_empty_fields() {
        for name in ["concise", "thorough", "educational", "critical", "creative"] {
            let dir = tempdir().unwrap();
            let content = SkillLoader::load(name, dir.path())
                .unwrap_or_else(|e| panic!("bundled '{name}' failed to load: {e}"));
            assert!(!content.description.is_empty(), "{name}: description empty");
            assert!(
                !content.style_instructions.is_empty(),
                "{name}: style_instructions empty"
            );
        }
    }
}
