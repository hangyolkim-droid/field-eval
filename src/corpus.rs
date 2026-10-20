//! Embedded, zero-dependency knowledge index.
//!
//! Every manual is packed into a read-only memory segment with `include_str!`
//! at compile time; nothing is read from disk at runtime. Documents are split
//! into `## ` sections, and an inverted index is built once on first use.

use std::collections::HashMap;
use std::sync::OnceLock;

/// A manual compiled into the binary.
pub struct Doc {
    pub id: &'static str,
    pub title: &'static str,
    pub text: &'static str,
}

/// The complete offline corpus. Add a file to `assets/` and list it here.
pub const DOCS: &[Doc] = &[
    Doc {
        id: "first-aid.md",
        title: "Field First Aid",
        text: include_str!("../assets/first-aid.md"),
    },
    Doc {
        id: "survival.md",
        title: "Field Survival Reference",
        text: include_str!("../assets/survival.md"),
    },
    Doc {
        id: "vehicle.md",
        title: "Vehicle Troubleshooting Reference",
        text: include_str!("../assets/vehicle.md"),
    },
];

/// One retrievable section of a manual.
#[derive(Debug, Clone)]
pub struct Chunk {
    pub doc: &'static str,
    pub doc_title: &'static str,
    pub heading: &'static str,
    pub body: &'static str,
    /// Lowercased token counts, used for scoring.
    tokens: HashMap<String, u32>,
    len: f64,
}

impl Chunk {
    pub fn citation(&self) -> String {
        format!("{} \u{00a7} {}", self.doc, self.heading)
    }
}

pub struct Index {
    pub chunks: Vec<Chunk>,
    postings: HashMap<String, Vec<usize>>,
    doc_freq: HashMap<String, u32>,
    /// Mean chunk length in tokens, for BM25 length normalisation.
    avgdl: f64,
}

static INDEX: OnceLock<Index> = OnceLock::new();

/// The process-wide index, built on first call.
pub fn index() -> &'static Index {
    INDEX.get_or_init(build_index)
}

fn build_index() -> Index {
    let mut chunks = Vec::new();
    for doc in DOCS {
        for (heading, body) in split_sections(doc.text) {
            let tokens = tokenize(&format!("{heading} {body}"));
            let len = tokens.values().map(|c| *c as f64).sum::<f64>().max(1.0);
            chunks.push(Chunk {
                doc: doc.id,
                doc_title: doc.title,
                heading,
                body,
                tokens,
                len,
            });
        }
    }

    let mut postings: HashMap<String, Vec<usize>> = HashMap::new();
    let mut doc_freq: HashMap<String, u32> = HashMap::new();
    for (i, chunk) in chunks.iter().enumerate() {
        for term in chunk.tokens.keys() {
            postings.entry(term.clone()).or_default().push(i);
            *doc_freq.entry(term.clone()).or_insert(0) += 1;
        }
    }

    let avgdl = if chunks.is_empty() {
        1.0
    } else {
        chunks.iter().map(|c| c.len).sum::<f64>() / chunks.len() as f64
    };

    Index {
        chunks,
        postings,
        doc_freq,
        avgdl,
    }
}

/// Split a document on `## ` headings.
///
/// The preamble before the first heading is only kept as an "Overview"
/// section when it carries prose beyond the document title. A bare `# Title`
/// is metadata, and treating it as a three-token section lets it outrank real
/// sections on common words like "field".
fn split_sections(text: &'static str) -> Vec<(&'static str, &'static str)> {
    let mut out: Vec<(&'static str, &'static str)> = Vec::new();
    let mut heading = "Overview";
    let mut body_start = 0usize;
    let mut pos = 0usize;
    let mut preamble: Option<&'static str> = None;

    for line in text.split_inclusive('\n') {
        if let Some(h) = line.trim_end_matches(['\r', '\n']).strip_prefix("## ") {
            let body = text[body_start..pos].trim();
            if preamble.is_none() {
                preamble = Some(body);
            } else if !body.is_empty() {
                out.push((leak_heading(heading), body));
            }
            heading = h.trim();
            body_start = pos + line.len();
        }
        pos += line.len();
    }
    let body = text[body_start..].trim();
    if !body.is_empty() {
        out.push((leak_heading(heading), body));
    }

    // Keep the title-only preamble only when it actually says something.
    if let Some(pre) = preamble {
        let prose: String = pre
            .lines()
            .filter(|l| !l.trim_start().starts_with('#'))
            .collect::<Vec<_>>()
            .join("\n");
        if !prose.trim().is_empty() {
            out.insert(0, (leak_heading("Overview"), pre));
        }
    }
    out
}

/// Headings are only a few dozen distinct strings; leaking them once keeps
/// the index free of self-referential lifetimes and costs a few KB.
fn leak_heading(h: &str) -> &'static str {
    Box::leak(h.to_string().into_boxed_str())
}

const STOPWORDS: &[&str] = &[
    "the", "a", "an", "and", "or", "of", "to", "in", "on", "for", "with", "is", "are", "was",
    "were", "be", "been", "it", "its", "this", "that", "these", "those", "as", "at", "by", "from",
    "if", "then", "than", "so", "but", "not", "no", "do", "does", "did", "can", "could", "should",
    "would", "will", "my", "your", "our", "their", "his", "her", "them", "we", "you", "i", "me",
    "what", "how", "when", "where", "why", "which", "who", "there", "here", "about", "into", "over",
    "up", "down", "out", "off", "very", "just", "also", "have", "has", "had", "may", "might",
];

fn tokenize(text: &str) -> HashMap<String, u32> {
    let mut counts: HashMap<String, u32> = HashMap::new();
    let mut word = String::new();
    for ch in text.chars() {
        if ch.is_alphanumeric() {
            word.extend(ch.to_lowercase());
        } else if !word.is_empty() {
            push_token(&mut counts, &word);
            word.clear();
        }
    }
    if !word.is_empty() {
        push_token(&mut counts, &word);
    }
    counts
}

fn push_token(counts: &mut HashMap<String, u32>, word: &str) {
    if word.len() < 2 || STOPWORDS.contains(&word) {
        return;
    }
    // Crude singular stem so "batteries" matches "battery".
    let stem = word
        .strip_suffix("ies")
        .map(|s| format!("{s}y"))
        .or_else(|| word.strip_suffix('s').filter(|s| s.len() > 3).map(str::to_string))
        .unwrap_or_else(|| word.to_string());
    *counts.entry(stem).or_insert(0) += 1;
}

/// One scored section, ready to inject as context.
pub struct Hit<'a> {
    pub chunk: &'a Chunk,
    pub score: f64,
}

/// Rank sections against a natural-language query.
///
/// BM25-flavoured scoring with a heading boost, computed over the small
/// in-memory index. Returns at most `k` hits, best first, never empty hits.
pub fn search(query: &str, k: usize) -> Vec<Hit<'static>> {
    let index = index();
    let q = tokenize(query);
    if q.is_empty() {
        return Vec::new();
    }

    let n = index.chunks.len() as f64;
    let avgdl = index.avgdl.max(1.0);
    // Standard BM25 constants; no tuning needed at this corpus size.
    let (k1, b) = (1.2_f64, 0.75_f64);
    let mut scores: HashMap<usize, f64> = HashMap::new();

    for (term, _) in &q {
        let Some(ids) = index.postings.get(term) else {
            continue;
        };
        let df = index.doc_freq.get(term).copied().unwrap_or(1) as f64;
        let idf = (1.0 + n / df).ln();
        for &i in ids {
            let chunk = &index.chunks[i];
            let tf = chunk.tokens.get(term).copied().unwrap_or(0) as f64;
            let norm = 1.0 - b + b * (chunk.len / avgdl);
            let mut s = idf * (tf * (k1 + 1.0)) / (tf + k1 * norm);
            if chunk.heading.to_lowercase().contains(term.as_str()) {
                s *= 1.5;
            }
            *scores.entry(i).or_insert(0.0) += s;
        }
    }

    let mut ranked: Vec<Hit<'static>> = scores
        .into_iter()
        .map(|(i, score)| Hit {
            chunk: &index.chunks[i],
            score,
        })
        .filter(|h| h.score > 0.0)
        .collect();
    ranked.sort_by(|a, b| b.score.total_cmp(&a.score));
    ranked.truncate(k);

    // Drop weak runners-up. A small model given three sections will happily
    // answer from the wrong one; sections scoring under a third of the best
    // match are noise, not context.
    if let Some(best) = ranked.first().map(|h| h.score) {
        let floor = best * 0.34;
        ranked.retain(|h| h.score >= floor);
    }
    ranked
}

/// Render hits as a compact context block for the system prompt.
pub fn render_context(hits: &[Hit<'static>]) -> String {
    let mut out = String::new();
    for (n, hit) in hits.iter().enumerate() {
        out.push_str(&format!(
            "[{}] {} (\"{}\") \u{2014} {}\n{}\n\n",
            n + 1,
            hit.chunk.doc,
            hit.chunk.doc_title,
            hit.chunk.heading,
            hit.chunk.body.trim()
        ));
    }
    out
}
