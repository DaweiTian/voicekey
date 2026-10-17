//! 转写精炼的离线回归评测。
//!
//! 本模块维护的不变量：人名字错率 / 口水词残留率两个产品指标只在这里定义，
//! 规则档与 LLM 档用同一份数据集、同一套公式测量，保证两档可比。

use serde::Deserialize;

pub const CASES_JSON: &str = include_str!("../eval/refine-cases.json");

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EvalCase {
    pub id: String,
    pub raw: String,
    #[serde(default)]
    pub names: Vec<String>,
    #[serde(default)]
    pub filler_tokens: Vec<String>,
    #[serde(default)]
    pub residual_allowed: usize,
    #[serde(default)]
    pub keep_tokens: Vec<String>,
    pub expected_rules: String,
    #[serde(default = "default_true")]
    pub rule_covered: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct EvalMetrics {
    /// 人名字错率：被改动/丢失的人名 token 数 ÷ 原始人名 token 数。
    pub name_error_rate: f64,
    /// 口水词残留率：超出 `residual_allowed` 的残留 token 数 ÷ 原始口水词 token 数。
    pub filler_residual_rate: f64,
    pub name_total: usize,
    pub name_errors: usize,
    pub filler_total: usize,
    pub filler_residuals: usize,
}

fn count_occurrences(haystack: &str, needle: &str) -> usize {
    if needle.is_empty() {
        return 0;
    }
    haystack.match_indices(needle).count()
}

pub fn load_cases() -> Vec<EvalCase> {
    serde_json::from_str(CASES_JSON).expect("refine-cases.json must deserialize")
}

pub fn evaluate(cases: &[EvalCase], outputs: &[String]) -> EvalMetrics {
    assert_eq!(cases.len(), outputs.len(), "one output per case");
    let mut m = EvalMetrics::default();
    for (case, out) in cases.iter().zip(outputs) {
        for name in &case.names {
            let expected = count_occurrences(&case.raw, name);
            let got = count_occurrences(out, name);
            m.name_total += expected;
            m.name_errors += expected.saturating_sub(got);
        }
        let mut filler_expected = 0usize;
        let mut filler_got = 0usize;
        for token in &case.filler_tokens {
            filler_expected += count_occurrences(&case.raw, token);
            filler_got += count_occurrences(out, token);
        }
        m.filler_total += filler_expected;
        m.filler_residuals += filler_got.saturating_sub(case.residual_allowed);
    }
    m.name_error_rate = if m.name_total == 0 {
        0.0
    } else {
        m.name_errors as f64 / m.name_total as f64
    };
    m.filler_residual_rate = if m.filler_total == 0 {
        0.0
    } else {
        m.filler_residuals as f64 / m.filler_total as f64
    };
    m
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::filler::strip_fillers;

    #[test]
    fn dataset_shape_is_sane() {
        let cases = load_cases();
        assert!((10..=20).contains(&cases.len()), "dataset must hold 10-20 cases");
        assert!(cases.iter().all(|c| !c.raw.is_empty() && !c.expected_rules.is_empty()));
    }

    #[test]
    fn rules_tier_meets_zero_error_targets() {
        let cases = load_cases();
        let outputs: Vec<String> = cases.iter().map(|c| strip_fillers(&c.raw)).collect();
        for (case, out) in cases.iter().zip(&outputs) {
            if case.rule_covered {
                assert_eq!(out, &case.expected_rules, "case {} rules output", case.id);
            }
            for keep in &case.keep_tokens {
                assert!(out.contains(keep), "case {}: keep token `{}` lost", case.id, keep);
            }
        }
        let m = evaluate(&cases, &outputs);
        assert_eq!(m.name_error_rate, 0.0, "rule tier must never touch names");
        assert_eq!(m.filler_residual_rate, 0.0, "rule-covered fillers must be gone");
    }

    /// 手动 LLM 档评测：真实调用 API、产生费用。
    /// 运行：`cd src-tauri && cargo test refine_llm_eval -- --ignored --nocapture`
    /// 环境变量：VOICEKEY_REFINE_EVAL_API_KEY（必填）、VOICEKEY_REFINE_EVAL_MODEL（可选）。
    #[tokio::test]
    #[ignore = "manual: calls the real LLM API"]
    async fn refine_llm_eval() {
        let api_key = std::env::var("VOICEKEY_REFINE_EVAL_API_KEY")
            .expect("set VOICEKEY_REFINE_EVAL_API_KEY");
        let mut config = crate::config::LLMRefineConfig::default();
        config.filler_cleanup = "llm".into();
        config.user_glossary = vec![
            "张伟".into(),
            "李娜".into(),
            "王强".into(),
            "陈静".into(),
        ];
        config.mimo.api_key = api_key;
        if let Ok(model) = std::env::var("VOICEKEY_REFINE_EVAL_MODEL") {
            config.mimo.model = model;
        }
        let cases = load_cases();
        let mut outputs = Vec::new();
        for case in &cases {
            let out = crate::llm::refine_text(&case.raw, &config)
                .await
                .expect("refine call");
            outputs.push(out);
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
        let m = evaluate(&cases, &outputs);
        println!(
            "人名字错率={:.3} ({}/{})  口水词残留率={:.3} ({}/{})",
            m.name_error_rate, m.name_errors, m.name_total,
            m.filler_residual_rate, m.filler_residuals, m.filler_total
        );
        // 硬底线：LLM 不得摧毁人名/数字（DRES 过度删除崩塌失败模式的守卫）。
        assert!(
            m.name_error_rate <= 0.10,
            "LLM tier must not destroy names: {:.3}",
            m.name_error_rate
        );
    }
}
