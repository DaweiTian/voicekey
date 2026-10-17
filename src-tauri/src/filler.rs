//! 保守的规则级口水词清理（中文 ASR 转写）。
//!
//! 本模块维护的不变量：所有删除都必须"宁漏删，不错删"——当一个词既可能是
//! 停顿/口头禅也可能是实词（指示代词"那个方案"、应答"就是就是"）时一律保留。
//! 该模块是 `llmRefine.fillerCleanup` 三档中的 `rules` 档，也在 `llm` 档
//! 但润色关闭时兜底，保证用户的清理意图始终生效。

/// 用户配置的口水词清理档位（`llmRefine.fillerCleanup`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FillerCleanupMode {
    Off,
    Rules,
    Llm,
}

impl FillerCleanupMode {
    /// 未知值一律回落到 `Off`（配置来自持久化 JSON，宁可保守）。
    pub fn parse(raw: &str) -> Self {
        match raw.trim() {
            "rules" => Self::Rules,
            "llm" => Self::Llm,
            _ => Self::Off,
        }
    }
}

const HESITATIONS: [char; 4] = ['嗯', '呃', '额', '唔'];
const FILLER_WORDS: [&str; 3] = ["那个", "就是", "然后"];

fn is_boundary(c: char) -> bool {
    c.is_whitespace()
        || matches!(
            c,
            '，' | '。' | '！' | '？' | '、' | '；' | '：' | '…' | '—' | '“' | '”'
                | '（' | '）' | '《' | '》' | '\n' | '\t'
        )
}

/// 越界（文本开头/结尾）视为边界。
fn boundary_at(chars: &[char], idx: isize) -> bool {
    match usize::try_from(idx) {
        Ok(i) => chars.get(i).map(|&c| is_boundary(c)).unwrap_or(true),
        Err(_) => true,
    }
}

/// 规则 A：删停顿词 run（至少一侧贴边界/标点），并吞掉紧跟的一个 `，`。
fn strip_hesitations(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out: Vec<char> = Vec::with_capacity(chars.len());
    let mut i = 0;
    while i < chars.len() {
        if HESITATIONS.contains(&chars[i]) {
            let start = i;
            while i < chars.len() && HESITATIONS.contains(&chars[i]) {
                i += 1;
            }
            if boundary_at(&chars, start as isize - 1) || boundary_at(&chars, i as isize) {
                if i < chars.len() && chars[i] == '，' {
                    i += 1;
                }
                continue;
            }
            out.extend_from_slice(&chars[start..i]);
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out.into_iter().collect()
}

/// 规则 B：句首+逗号、或连续重复 ≥2 次的 `那个/就是/然后` 才删；其余保留。
fn strip_clause_initial_fillers(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out: Vec<char> = Vec::with_capacity(chars.len());
    let mut i = 0;
    'scan: while i < chars.len() {
        for word in FILLER_WORDS {
            let w: Vec<char> = word.chars().collect();
            if !chars[i..].starts_with(&w[..]) {
                continue;
            }
            let mut k = 0usize;
            let mut j = i;
            while chars[j..].starts_with(&w[..]) {
                k += 1;
                j += w.len();
            }
            let comma_after = matches!(chars.get(j), Some('，'));
            let clause_start = match i.checked_sub(1).and_then(|p| chars.get(p)) {
                None => true,
                Some(&c) => matches!(c, '。' | '！' | '？' | '；' | '\n'),
            };
            if k >= 2 || (clause_start && comma_after) {
                i = j + usize::from(comma_after);
                continue 'scan;
            }
            // 实词用法（指示代词/顺序连接词/应答）原样保留。
            out.extend_from_slice(&chars[i..j]);
            i = j;
            continue 'scan;
        }
        out.push(chars[i]);
        i += 1;
    }
    out.into_iter().collect()
}

/// 规则 C：清理删除留下的标点残渣（仅全角，英文标点不碰）。
fn tidy_punctuation(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut prev: Option<char> = None;
    for c in text.chars() {
        if matches!(c, '，' | '、') {
            if out.is_empty() || matches!(prev, Some('，') | Some('、')) {
                continue;
            }
        }
        out.push(c);
        prev = Some(c);
    }
    out.trim_end_matches(['，', '、']).to_string()
}

/// 保守规则清理入口。幂等：`strip_fillers(strip_fillers(x)) == strip_fillers(x)`。
pub fn strip_fillers(text: &str) -> String {
    let stage1 = strip_hesitations(text);
    let stage2 = strip_clause_initial_fillers(&stage1);
    tidy_punctuation(&stage2)
}

/// 精炼管线调用的分发点。`llm` 档在润色开启时把清理交给精炼 prompt
/// （单次调用，绝不加第二次 LLM 请求）；润色关闭时回落到规则档兜底。
pub fn cleanup_stage(text: &str, filler_cleanup: &str, refine_enabled: bool) -> String {
    match FillerCleanupMode::parse(filler_cleanup) {
        FillerCleanupMode::Off => text.to_string(),
        FillerCleanupMode::Rules => strip_fillers(text),
        FillerCleanupMode::Llm if refine_enabled => text.to_string(),
        FillerCleanupMode::Llm => strip_fillers(text),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removes_hesitation_next_to_boundary_or_punctuation() {
        assert_eq!(strip_fillers("嗯，我觉得可以。"), "我觉得可以。");
        assert_eq!(strip_fillers("张伟，嗯，明天来。"), "张伟，明天来。");
        assert_eq!(strip_fillers("嗯嗯嗯，好的收到。"), "好的收到。");
    }

    #[test]
    fn keeps_hesitation_embedded_inside_words() {
        assert_eq!(strip_fillers("这个嗯嗯东西不好说"), "这个嗯嗯东西不好说");
    }

    #[test]
    fn removes_sentence_initial_filler_with_comma() {
        assert_eq!(strip_fillers("那个，我们开始吧。"), "我们开始吧。");
        assert_eq!(strip_fillers("然后，先保存一下。"), "先保存一下。");
        assert_eq!(
            strip_fillers("今天讲三件事。然后，看一下排期。"),
            "今天讲三件事。看一下排期。"
        );
    }

    #[test]
    fn keeps_meaningful_demonstratives_and_connectives() {
        assert_eq!(
            strip_fillers("我觉得那个方案还是不错的。"),
            "我觉得那个方案还是不错的。"
        );
        assert_eq!(strip_fillers("先做那个，再做这个。"), "先做那个，再做这个。");
        assert_eq!(
            strip_fillers("王强说那个接口下周能好。"),
            "王强说那个接口下周能好。"
        );
    }

    #[test]
    fn removes_consecutive_repeats() {
        assert_eq!(
            strip_fillers("那个那个那个，今天评审需求文档。"),
            "今天评审需求文档。"
        );
        assert_eq!(strip_fillers("帮我把那个那个文件删掉。"), "帮我把文件删掉。");
        assert_eq!(strip_fillers("就是就是，你说得对。"), "你说得对。");
    }

    #[test]
    fn never_touches_names_numbers_or_english() {
        assert_eq!(
            strip_fillers("嗯，张伟和李娜负责 v2.3版本，预算 15万。"),
            "张伟和李娜负责 v2.3版本，预算 15万。"
        );
        assert_eq!(
            strip_fillers("open the settings page and click save"),
            "open the settings page and click save"
        );
    }

    #[test]
    fn is_idempotent() {
        let once = strip_fillers("嗯，那个，今天下午三点开会。");
        assert_eq!(strip_fillers(&once), once);
        assert_eq!(once, "今天下午三点开会。");
    }

    #[test]
    fn cleanup_stage_honours_mode_and_refine_flag() {
        let raw = "嗯，先保存一下。";
        assert_eq!(cleanup_stage(raw, "off", false), raw);
        assert_eq!(cleanup_stage(raw, "rules", false), "先保存一下。");
        assert_eq!(cleanup_stage(raw, "rules", true), "先保存一下。");
        assert_eq!(cleanup_stage(raw, "llm", true), raw);
        assert_eq!(cleanup_stage(raw, "llm", false), "先保存一下。");
        assert_eq!(cleanup_stage(raw, "bogus", false), raw);
    }

    #[test]
    fn filler_cleanup_mode_parses_known_values() {
        assert_eq!(FillerCleanupMode::parse("off"), FillerCleanupMode::Off);
        assert_eq!(FillerCleanupMode::parse("rules"), FillerCleanupMode::Rules);
        assert_eq!(FillerCleanupMode::parse("llm"), FillerCleanupMode::Llm);
        assert_eq!(FillerCleanupMode::parse("other"), FillerCleanupMode::Off);
    }
}
