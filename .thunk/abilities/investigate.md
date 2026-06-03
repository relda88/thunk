# investigate

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
