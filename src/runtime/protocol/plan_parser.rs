#[derive(Debug, Clone)]
pub struct PlanStep {
    pub title: String,
    pub description: String,
}

/// Parse a numbered plan from model output.
/// Format: "N. Title: Description" — one step per line.
/// Rejects if fewer than 2 steps parsed or any non-blank line
/// does not match the numbered format.
pub fn parse_plan(text: &str) -> Result<Vec<PlanStep>, String> {
    let mut steps: Vec<PlanStep> = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = line
            .split_once(". ")
            .and_then(|(num, rest)| num.parse::<usize>().ok().map(|_| rest))
        {
            if let Some((title, desc)) = rest.split_once(": ") {
                steps.push(PlanStep {
                    title: title.trim().to_string(),
                    description: desc.trim().to_string(),
                });
                continue;
            }
        }
        return Err(format!("could not parse plan — unexpected line: '{line}'"));
    }
    if steps.len() < 2 {
        return Err(format!(
            "plan must have at least 2 steps, got {}",
            steps.len()
        ));
    }
    Ok(steps)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_valid_plan() {
        let text = "1. Setup: Initialize the project\n\
                    2. Build: Compile all targets\n\
                    3. Test: Run the full test suite";
        let steps = parse_plan(text).unwrap();
        assert_eq!(steps.len(), 3);
        assert_eq!(steps[0].title, "Setup");
        assert_eq!(steps[0].description, "Initialize the project");
        assert_eq!(steps[1].title, "Build");
        assert_eq!(steps[2].title, "Test");
    }

    #[test]
    fn parse_rejects_freeform() {
        let text = "Here is my plan: step 1 do things, step 2 do more things";
        assert!(parse_plan(text).is_err());
    }

    #[test]
    fn parse_rejects_single_step() {
        let text = "1. Setup: Initialize the project";
        let err = parse_plan(text).unwrap_err();
        assert!(err.contains("at least 2 steps"));
    }

    #[test]
    fn parse_accepts_blank_lines() {
        let text = "1. Setup: Initialize the project\n\n2. Build: Compile all targets";
        let steps = parse_plan(text).unwrap();
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[0].title, "Setup");
        assert_eq!(steps[1].title, "Build");
    }

    #[test]
    fn parse_rejects_missing_colon() {
        let text = "1. Title no colon here\n2. Step: valid";
        let err = parse_plan(text).unwrap_err();
        assert!(err.contains("unexpected line"));
    }
}
