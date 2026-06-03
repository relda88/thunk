# debug

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
