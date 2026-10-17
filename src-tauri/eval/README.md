# refine eval dataset

Regression dataset for transcript refinement (deliverable D4).

- `refine-cases.json`: 10-20 ASR samples with target names (`names`), filler tokens
  (`fillerTokens`, `residualAllowed`), tokens that must survive (`keepTokens`), and the
  golden rule-tier output (`expectedRules`).
- Metrics (defined once in `src-tauri/src/refine_eval.rs`): 人名字错率 (name error rate)
  and 口水词残留率 (filler residual rate).

Run rule-tier eval (offline, deterministic): `cd src-tauri && cargo test refine_eval`.
Run LLM-tier eval (manual, needs `VOICEKEY_REFINE_EVAL_API_KEY`):
`cd src-tauri && cargo test refine_llm_eval -- --ignored --nocapture`.
Targets: 人名字错率 = 0 (rules) / <= 0.10 (LLM, hard assert); 口水词残留率 = 0 (rules).
