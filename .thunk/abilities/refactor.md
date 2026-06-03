# refactor

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
