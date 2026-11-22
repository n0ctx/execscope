//! Priority-aware brief selection under an approximate token budget.
//!
//! Estimation contract: `estimate` is a deterministic, intentionally conservative
//! upper-bound approximation calibrated against the `o200k_base` and `cl100k_base`
//! tokenizers on report-shaped text (identifiers, paths, key=value pairs, CJK and
//! long untrusted fields). It is not an exact count for any particular model;
//! other tokenizers can differ. Whole facts are dropped, lowest priority first, until
//! the estimate fits; text is never cut inside a UTF-8 code point.

/// Lower number = more important. Heads of a line are kept or dropped with their line.
pub(crate) type Priority = u8;

pub(crate) struct Part {
    priority: Priority,
    text: String,
}

pub(crate) struct Line {
    head: Part,
    /// Placed between the head and the first item, only when items remain.
    glue: &'static str,
    sep: &'static str,
    items: Vec<Part>,
    /// A list line has no fact of its own: it disappears with its last item.
    list: bool,
}

impl Line {
    pub(crate) fn new(priority: Priority, head: impl Into<String>) -> Self {
        Self {
            head: Part {
                priority,
                text: head.into(),
            },
            glue: " ",
            sep: " ",
            items: vec![],
            list: false,
        }
    }

    pub(crate) fn list(prefix: impl Into<String>, sep: &'static str) -> Self {
        Self {
            glue: "",
            sep,
            list: true,
            ..Self::new(Priority::MAX, prefix)
        }
    }

    pub(crate) fn joined(mut self, glue: &'static str, sep: &'static str) -> Self {
        self.glue = glue;
        self.sep = sep;
        self
    }

    pub(crate) fn push(&mut self, priority: Priority, text: impl Into<String>) -> &mut Self {
        let text = text.into();
        if !text.is_empty() {
            self.items.push(Part { priority, text });
        }
        self
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Slot {
    Head(usize),
    Item(usize, usize),
}

/// Renders lines, dropping the least important facts until the estimate fits `budget`.
pub(crate) fn render(lines: Vec<Line>, budget: usize) -> String {
    let mut kept: Vec<(bool, Vec<bool>)> = lines
        .iter()
        .map(|l| (true, vec![true; l.items.len()]))
        .collect();
    let compose = |kept: &[(bool, Vec<bool>)], omitted: usize| -> String {
        let mut out = Vec::new();
        for (line, (head, items)) in lines.iter().zip(kept) {
            if !head {
                continue;
            }
            let parts: Vec<&str> = line
                .items
                .iter()
                .zip(items)
                .filter(|(_, k)| **k)
                .map(|(p, _)| p.text.as_str())
                .collect();
            if parts.is_empty() {
                if !line.list {
                    out.push(line.head.text.clone());
                }
            } else {
                out.push(format!(
                    "{}{}{}",
                    line.head.text,
                    line.glue,
                    parts.join(line.sep)
                ));
            }
        }
        if omitted > 0 {
            out.push(format!("omitted={omitted} use_detail=full"));
        }
        out.join("\n")
    };
    // Removal order: least important first; ties remove later lines and later items first.
    let mut order: Vec<(Priority, Slot)> = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        if !line.list {
            order.push((line.head.priority, Slot::Head(i)));
        }
        for (j, item) in line.items.iter().enumerate() {
            order.push((item.priority, Slot::Item(i, j)));
        }
    }
    order.sort_by(|a, b| {
        b.0.cmp(&a.0).then_with(|| {
            let key = |s: &Slot| match *s {
                Slot::Head(i) => (i, 0),
                Slot::Item(i, j) => (i, j + 1),
            };
            key(&b.1).cmp(&key(&a.1))
        })
    });
    let mut omitted = 0;
    let mut text = compose(&kept, 0);
    for (priority, slot) in order {
        if estimate(&text) <= budget {
            return text;
        }
        // The execution boundary is never dropped; it is shortened as a last resort.
        if priority <= 1 {
            break;
        }
        match slot {
            Slot::Head(i) if kept[i].0 => {
                kept[i].0 = false;
                omitted += 1 + kept[i].1.iter().filter(|k| **k).count();
            }
            Slot::Item(i, j) if kept[i].0 && kept[i].1[j] => {
                kept[i].1[j] = false;
                omitted += 1;
            }
            _ => continue,
        }
        text = compose(&kept, omitted);
    }
    if estimate(&text) <= budget {
        return text;
    }
    truncate(&text, budget)
}

/// Last resort for a single oversized fact: cut on a character boundary.
fn truncate(text: &str, budget: usize) -> String {
    let mut out = String::new();
    for c in text.chars() {
        out.push(c);
        if estimate(&out) + estimate("…") > budget {
            out.pop();
            break;
        }
    }
    out.push('…');
    out
}

/// Conservative token estimate; see the module contract.
pub(crate) fn estimate(text: &str) -> usize {
    let mut tokens = 0;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c.is_ascii_alphanumeric() {
            let mut run = vec![c as u8];
            while let Some(c) = chars.next_if(char::is_ascii_alphanumeric) {
                run.push(c as u8);
            }
            tokens += alphanumeric(&run);
        } else if c == ' ' {
            // A single space is usually merged into the following word.
        } else if c.is_ascii() {
            tokens += 1;
        } else {
            // Common CJK is about one token per character; rare blocks and
            // supplementary planes fall back to byte tokens.
            tokens += match (c.len_utf8(), c) {
                (3, '\u{3400}'..='\u{4dbf}') => 3,
                (2 | 3, _) => 2,
                _ => 4,
            };
        }
    }
    tokens
}

fn alphanumeric(run: &[u8]) -> usize {
    let len = run.len();
    let digits = run.iter().filter(|b| b.is_ascii_digit()).count();
    if digits == len {
        return len.div_ceil(3);
    }
    if digits > 0 {
        // Mixed runs (hashes, ids, versions) split into short pieces.
        return (len * 3).div_ceil(4);
    }
    let vowels = run.iter().filter(|b| b"aeiouyAEIOUY".contains(b)).count();
    let switches = run
        .windows(2)
        .filter(|w| w[0].is_ascii_lowercase() != w[1].is_ascii_lowercase())
        .count();
    if switches * 3 > len {
        // Random-looking mixed case (base64, keys) is close to one token per letter.
        (len * 9).div_ceil(10)
    } else if vowels * 5 < len {
        (len * 2).div_ceil(3)
    } else {
        len.div_ceil(4)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reference counts from tiktoken 0.14 (`o200k_base`, `cl100k_base`); regenerate with
    /// `tests/token_calibration.py`. The estimate must never be below either.
    const CALIBRATION: &[(&str, usize, usize)] = include!("../tests/fixtures/token_calibration.in");

    #[test]
    fn estimate_is_conservative_on_calibration_corpus() {
        for (text, o200k, cl100k) in CALIBRATION {
            let estimate = estimate(text);
            assert!(
                estimate >= *o200k && estimate >= *cl100k,
                "{estimate} < reference ({o200k}/{cl100k}) for {text:?}"
            );
            // Stay useful: not wildly above the reference on report-shaped text.
            assert!(
                estimate <= (*o200k).max(*cl100k) * 6 + 8,
                "{estimate} too high for {text:?}"
            );
        }
    }

    #[test]
    fn drops_low_priority_facts_whole_and_keeps_order() {
        let mut a = Line::new(1, "os=macos/aarch64 exec=native");
        a.push(7, "developer=xcode");
        let mut tools = Line::list("tools=", " ");
        for i in 0..40 {
            tools.push(7, format!("tool{i}:1.0.{i}"));
        }
        let mut h = Line::new(3, "harness=pi process_linked");
        h.push(5, "mcp:2");
        let lines = vec![a, tools, h];
        let full = render(
            {
                let mut a = Line::new(1, "x");
                a.push(2, "y");
                vec![a]
            },
            600,
        );
        assert_eq!(full, "x y");
        let text = render(lines, 40);
        assert!(estimate(&text) <= 40, "{text}");
        assert!(text.starts_with("os=macos/aarch64 exec=native"), "{text}");
        assert!(text.contains("harness=pi process_linked"), "{text}");
        assert!(text.contains("omitted="), "{text}");
        assert!(!text.contains("tools=\n"), "{text}");
    }

    #[test]
    fn oversized_single_fact_is_cut_on_char_boundary() {
        let text = render(vec![Line::new(1, "测".repeat(500))], 100);
        assert!(estimate(&text) <= 100);
        assert!(text.ends_with('…'));
    }
}
