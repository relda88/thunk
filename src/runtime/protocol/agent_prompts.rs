pub(crate) fn build_review_prompt(target_str: &str, anchor: &str) -> String {
    format!(
        "[runtime:agent:review] Review workflow\n\
         Target: {target_str}\n\n\
         {anchor}\
         Use the review ability. Read the relevant files. \
         Produce a structured critique.\n\n\
         Format each issue as:\n\
         [critical|warning|note] <what is wrong> — \
         <why it matters> — <fix direction>\n\n\
         Correctness bugs first. Cite specific file and \
         line for every claim. Do not soften real bugs."
    )
}

pub(crate) fn build_investigate_prompt(target_str: &str, anchor: &str) -> String {
    format!(
        "[runtime:agent:investigate] Investigate workflow\n\
         Target: {target_str}\n\n\
         {anchor}\
         Use the investigate ability. Gather evidence \
         systematically.\n\n\
         Format your findings as:\n\
         KNOWN: <confirmed facts with evidence>\n\
         UNKNOWN: <open questions, ranked by value>\n\
         HYPOTHESIS: <current best explanation>\n\
         NEXT READS: <specific files/symbols to resolve \
         the top unknown>\n\n\
         Do not conclude until evidence is sufficient."
    )
}

pub(crate) fn build_refactor_prompt(target_str: &str, anchor: &str) -> String {
    format!(
        "[runtime:agent:refactor] Refactor investigation\n\
         Target: {target_str}\n\n\
         {anchor}\
         Use the refactor ability. Investigate the target. \
         Identify structural problems: coupling, duplication, \
         unclear naming, god functions, mixed responsibilities.\n\n\
         Report what you found — structure, problems, and \
         what a clean refactor would address. Be specific."
    )
}
