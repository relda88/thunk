use std::path::Path;

#[derive(Debug, Clone, Default)]
pub struct AbilityContent {
    pub name: String,
    pub invariants: String,
    pub specification: String,
    pub examples: String,
    pub amplifies: String,
    pub suppresses: String,
    pub reasoning_effect: String,
}

// Bundled default ability content — always available even if
// .thunk/abilities/ is empty. Disk files take priority over these.

const BUNDLED_DEBUG: &str = r#"# debug

## Invariants
- Trace the actual failure path before proposing a fix
- Separate symptom from root cause explicitly
- Never assume the first plausible cause is the real cause
- Confirm the fix addresses the root cause, not just the symptom

## Specification
1. Identify what is failing and what the expected behavior is
2. Trace execution from the failure point backwards to find where
   the invariant breaks
3. Distinguish between the proximate cause (what triggered the
   failure) and the root cause (why the system allowed it)
4. Check for related failure modes — if one thing is broken, what
   else might be?
5. Propose a fix that addresses the root cause
6. State what would confirm the fix is correct

## Examples
- "The panic is at line 42, but the root cause is the None returned
  at line 17 when the config key is missing"
- "This is a symptom of the missing bounds check upstream, not a
  problem with the rendering logic itself"

## Amplifies
- Causal chain tracing
- Root cause analysis
- Failure mode enumeration
- Evidence-based diagnosis

## Suppresses
- Guessing without tracing
- Fixing symptoms without understanding cause
- Proposing multiple unrelated fixes simultaneously
- Assuming the obvious explanation is correct

## Reasoning Effect
Traces failure paths methodically. Distinguishes proximate from root
cause. Does not propose fixes until the cause is established.
"#;

const BUNDLED_REVIEW: &str = r#"# review

## Invariants
- Surface real problems, not style preferences
- Every claim about a bug must cite evidence from the code
- Distinguish correctness issues from design issues from nitpicks
- Never soft-pedal a real bug to seem polite

## Specification
1. Read the code for what it actually does, not what it intends to do
2. Check invariants: are all preconditions validated? Are all paths
   handled? Are error cases propagated correctly?
3. Check edge cases: empty inputs, boundary values, concurrent access,
   resource exhaustion
4. Identify correctness issues first, then design issues, then style
5. For each issue: state what is wrong, why it matters, and what
   the fix direction is

## Examples
- "This returns Ok on line 23 even when the write fails — the error
  is silently discarded"
- "The lock is released before the invariant is restored — this is
  a correctness bug under concurrent access"

## Amplifies
- Correctness auditing
- Edge case enumeration
- Invariant checking
- Evidence-based criticism

## Suppresses
- Style-only feedback framed as bugs
- Vague concerns without code evidence
- Excessive hedging on real issues
- Approval-seeking language

## Reasoning Effect
Audits for correctness first. Cites specific lines and conditions.
Does not conflate style with bugs. Does not soften real problems.
"#;

const BUNDLED_REFACTOR: &str = r#"# refactor

## Invariants
- No behavior changes — refactoring is structural only
- Every change must be justified by improved clarity, reduced
  coupling, or eliminated duplication
- If behavior changes are needed, flag them separately
- Preserve all existing tests; add tests if coverage is thin

## Specification
1. Identify what structural problem the refactor addresses:
   duplication, coupling, unclear naming, god function, etc.
2. State the target structure before proposing changes
3. Break the refactor into safe, independently-verifiable steps
4. For each step: what changes, what stays the same, how to verify
5. Flag any behavior that appears accidental (bugs disguised as
   features) without fixing them silently

## Examples
- "Extract the validation logic into its own function — it's used
  in three places and will diverge if not unified"
- "This function has two responsibilities: parsing and validation.
  Split them so each can be tested independently"

## Amplifies
- Structural clarity
- Single responsibility
- Duplication elimination
- Safe incremental steps

## Suppresses
- Behavior changes disguised as cleanup
- Rewrites framed as refactors
- Big-bang changes without intermediate safe states
- Premature optimization

## Reasoning Effect
Focuses on structure, not behavior. Proposes incremental steps.
Flags accidental behavior rather than silently changing it.
"#;

const BUNDLED_INVESTIGATE: &str = r#"# investigate

## Invariants
- Map what is known and unknown before concluding
- Never close investigation prematurely
- Distinguish confirmed facts from working hypotheses
- Reopen the question if evidence contradicts the hypothesis

## Specification
1. State what is known with confidence
2. State what is unknown or ambiguous
3. Identify the highest-value unknowns to resolve first
4. Propose specific reads or checks that would reduce uncertainty
5. Update the hypothesis as evidence comes in
6. Conclude only when the evidence is sufficient

## Examples
- "We know X from the logs, but we don't know whether Y is the
  cause or a symptom — reading Z would distinguish them"
- "This hypothesis fits the observed behavior but contradicts the
  invariant at line 47 — that needs explaining before we proceed"

## Amplifies
- Systematic evidence gathering
- Hypothesis updating
- Uncertainty acknowledgment
- High-value question prioritization

## Suppresses
- Premature conclusions
- Ignoring contradicting evidence
- Treating hypotheses as facts
- Skipping investigation to propose solutions

## Reasoning Effect
Maps knowns and unknowns explicitly. Updates hypotheses as evidence
arrives. Does not conclude until evidence is sufficient.
"#;

const BUNDLED_EXPLAIN: &str = r#"# explain

## Invariants
- Match explanation depth to the audience and question
- Use concrete examples, not abstract descriptions alone
- Never sacrifice accuracy for simplicity
- State what is being simplified when simplifying

## Specification
1. Identify what the audience already knows and what the gap is
2. Start with the concrete case before the general principle
3. Use a minimal working example to ground the explanation
4. Build from the example to the general case
5. Check that the explanation actually answers the question asked
6. Flag simplifications explicitly when made

## Examples
- Start with "here is what happens in this specific case" before
  "here is the general rule"
- "This is a simplification — the full behavior also covers X,
  but that's not relevant here"

## Amplifies
- Concrete examples first
- Audience-appropriate depth
- Explicit simplification flagging
- Question-answer alignment

## Suppresses
- Abstract-first explanations
- Jargon without definition
- Over-explaining what the audience already knows
- Under-explaining what they don't

## Reasoning Effect
Leads with concrete cases. Builds to general principles. Matches
depth to audience. Flags simplifications without hiding them.
"#;

fn parse_ability_md(content: &str, name: &str) -> AbilityContent {
    let mut result = AbilityContent {
        name: name.to_string(),
        ..Default::default()
    };

    let mut sections: [(&str, &mut String); 6] = [
        ("## Invariants", &mut result.invariants),
        ("## Specification", &mut result.specification),
        ("## Examples", &mut result.examples),
        ("## Amplifies", &mut result.amplifies),
        ("## Suppresses", &mut result.suppresses),
        ("## Reasoning Effect", &mut result.reasoning_effect),
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
                "[thunk] ability '{}': missing section '{}', using empty string",
                name, header
            );
        }
    }
    result
}

pub struct AbilityLoader;

impl AbilityLoader {
    /// Load an ability by name. Checks `.thunk/abilities/<name>.md`
    /// first; falls back to bundled defaults. Returns `Err` if the
    /// name is not recognized and no file exists.
    pub fn load(name: &str, thunk_dir: &Path) -> Result<AbilityContent, String> {
        let path = thunk_dir.join("abilities").join(format!("{name}.md"));
        if path.exists() {
            match std::fs::read_to_string(&path) {
                Ok(content) => return Ok(parse_ability_md(&content, name)),
                Err(e) => eprintln!(
                    "[thunk] ability '{}': failed to read {:?}: {}",
                    name, path, e
                ),
            }
        }
        match name {
            "debug" => Ok(parse_ability_md(BUNDLED_DEBUG, name)),
            "review" => Ok(parse_ability_md(BUNDLED_REVIEW, name)),
            "refactor" => Ok(parse_ability_md(BUNDLED_REFACTOR, name)),
            "investigate" => Ok(parse_ability_md(BUNDLED_INVESTIGATE, name)),
            "explain" => Ok(parse_ability_md(BUNDLED_EXPLAIN, name)),
            _ => Err(format!(
                "unknown ability '{}' — available: debug, review, refactor, investigate, explain",
                name
            )),
        }
    }

    /// List all available ability names: bundled defaults plus any
    /// `.md` files in `.thunk/abilities/` not already in the bundled set.
    pub fn list_available(thunk_dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = vec![
            "debug".into(),
            "review".into(),
            "refactor".into(),
            "investigate".into(),
            "explain".into(),
        ];
        let abilities_dir = thunk_dir.join("abilities");
        if let Ok(entries) = std::fs::read_dir(&abilities_dir) {
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

    const FULL_MD: &str = r#"# test-ability

## Invariants
Do the thing correctly.

## Specification
1. Step one.
2. Step two.

## Examples
- Example A
- Example B

## Amplifies
- Good stuff

## Suppresses
- Bad stuff

## Reasoning Effect
Does things well.
"#;

    #[test]
    fn parse_all_sections_present() {
        let result = parse_ability_md(FULL_MD, "test-ability");
        assert_eq!(result.name, "test-ability");
        assert!(
            !result.invariants.is_empty(),
            "invariants must be populated"
        );
        assert!(
            !result.specification.is_empty(),
            "specification must be populated"
        );
        assert!(!result.examples.is_empty(), "examples must be populated");
        assert!(!result.amplifies.is_empty(), "amplifies must be populated");
        assert!(
            !result.suppresses.is_empty(),
            "suppresses must be populated"
        );
        assert!(
            !result.reasoning_effect.is_empty(),
            "reasoning_effect must be populated"
        );
    }

    #[test]
    fn parse_missing_section_no_panic() {
        let md = r#"# partial

## Invariants
Some invariant.

## Specification
Some spec.
"#;
        let result = parse_ability_md(md, "partial");
        assert!(!result.invariants.is_empty());
        assert!(!result.specification.is_empty());
        // Missing sections are empty strings, not panics.
        assert!(result.examples.is_empty());
        assert!(result.amplifies.is_empty());
        assert!(result.suppresses.is_empty());
        assert!(result.reasoning_effect.is_empty());
    }

    #[test]
    fn load_returns_bundled_when_no_file() {
        let dir = tempdir().unwrap();
        // abilities/ subdir does not exist — must fall back to bundled.
        let result = AbilityLoader::load("debug", dir.path());
        let content = result.expect("bundled debug must load");
        assert_eq!(content.name, "debug");
        assert!(!content.invariants.is_empty());
        assert!(!content.specification.is_empty());
    }

    #[test]
    fn load_returns_error_for_unknown() {
        let dir = tempdir().unwrap();
        let result = AbilityLoader::load("nonexistent", dir.path());
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("unknown ability"));
    }

    #[test]
    fn load_prefers_disk_file_over_bundled() {
        let dir = tempdir().unwrap();
        let abilities_dir = dir.path().join("abilities");
        fs::create_dir_all(&abilities_dir).unwrap();
        fs::write(
            abilities_dir.join("debug.md"),
            "# debug\n## Invariants\nCustom invariant.\n## Specification\nCustom spec.\n## Examples\nEx.\n## Amplifies\nAmp.\n## Suppresses\nSup.\n## Reasoning Effect\nEffect.\n",
        )
        .unwrap();

        let result = AbilityLoader::load("debug", dir.path()).unwrap();
        assert!(
            result.invariants.contains("Custom"),
            "disk file must override bundled content"
        );
    }

    #[test]
    fn list_available_includes_bundled() {
        let dir = tempdir().unwrap();
        let names = AbilityLoader::list_available(dir.path());
        assert!(names.contains(&"debug".to_string()));
        assert!(names.contains(&"review".to_string()));
        assert!(names.contains(&"refactor".to_string()));
        assert!(names.contains(&"investigate".to_string()));
        assert!(names.contains(&"explain".to_string()));
    }

    #[test]
    fn list_available_includes_custom() {
        let dir = tempdir().unwrap();
        let abilities_dir = dir.path().join("abilities");
        fs::create_dir_all(&abilities_dir).unwrap();
        fs::write(abilities_dir.join("custom.md"), "# custom\n").unwrap();

        let names = AbilityLoader::list_available(dir.path());
        assert!(names.contains(&"custom".to_string()));
        // Bundled names still present.
        assert!(names.contains(&"debug".to_string()));
    }

    #[test]
    fn list_available_excludes_non_md_files() {
        let dir = tempdir().unwrap();
        let abilities_dir = dir.path().join("abilities");
        fs::create_dir_all(&abilities_dir).unwrap();
        fs::write(abilities_dir.join(".gitkeep"), "").unwrap();
        fs::write(abilities_dir.join("readme.txt"), "").unwrap();

        let names = AbilityLoader::list_available(dir.path());
        assert!(!names.contains(&".gitkeep".to_string()));
        assert!(!names.contains(&"readme".to_string()));
    }

    #[test]
    fn list_available_is_sorted() {
        let dir = tempdir().unwrap();
        let names = AbilityLoader::list_available(dir.path());
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted, "list_available must return sorted names");
    }

    #[test]
    fn all_bundled_abilities_parse_without_empty_fields() {
        for name in ["debug", "review", "refactor", "investigate", "explain"] {
            let dir = tempdir().unwrap();
            let content = AbilityLoader::load(name, dir.path())
                .unwrap_or_else(|e| panic!("bundled '{name}' failed to load: {e}"));
            assert!(!content.invariants.is_empty(), "{name}: invariants empty");
            assert!(
                !content.specification.is_empty(),
                "{name}: specification empty"
            );
            assert!(!content.examples.is_empty(), "{name}: examples empty");
            assert!(!content.amplifies.is_empty(), "{name}: amplifies empty");
            assert!(!content.suppresses.is_empty(), "{name}: suppresses empty");
            assert!(
                !content.reasoning_effect.is_empty(),
                "{name}: reasoning_effect empty"
            );
        }
    }
}
