/// Operator-configurable context corpus for the transformer RAG layer.
///
/// Loads `[[docs]]` entries from TOML files and renders them as a
/// `<context_pack>` block injected into the system prompt before the
/// model call. The embedded default corpus (context/default.toml) ships
/// with the binary; operators may extend or replace it via SBH_CONTEXT_PATH.
use anyhow::{Context, Result};
use serde::Deserialize;
use std::fs;
use std::path::Path;

const DEFAULT_CONTEXT: &str = include_str!("../context/default.toml");

/// One context document: a titled, tagged block of text.
#[derive(Debug, Clone, Deserialize)]
pub struct ContextDoc {
    pub id: String,
    pub title: String,
    pub text: String,
    #[serde(default)]
    pub tags: Vec<String>,
}

#[derive(Deserialize, Default)]
struct TomlCorpus {
    #[serde(default)]
    docs: Vec<ContextDoc>,
}

/// A collection of context documents injected into the transformer prompt.
#[derive(Debug, Default, Clone)]
pub struct ContextCorpus {
    pub docs: Vec<ContextDoc>,
}

impl ContextCorpus {
    /// Returns the embedded default corpus compiled into the binary.
    pub fn embedded() -> Self {
        let parsed: TomlCorpus = toml::from_str(DEFAULT_CONTEXT)
            .expect("embedded context/default.toml is invalid TOML — build error");
        Self { docs: parsed.docs }
    }

    /// Load a single TOML file.
    pub fn load_file(file_path: &str) -> Result<Self> {
        let raw = fs::read_to_string(file_path)
            .with_context(|| format!("cannot read context file: {file_path}"))?;
        let parsed: TomlCorpus = toml::from_str(&raw)
            .with_context(|| format!("invalid TOML in context file: {file_path}"))?;
        Ok(Self { docs: parsed.docs })
    }

    /// Load all `*.toml` files from a directory, merging them.
    pub fn load_dir(dir_path: &str) -> Result<Self> {
        let mut corpus = Self::default();
        let dir = Path::new(dir_path);
        let entries = fs::read_dir(dir)
            .with_context(|| format!("cannot read context directory: {dir_path}"))?;
        for entry in entries {
            let path = entry?.path();
            if path.extension().and_then(|e| e.to_str()) == Some("toml") {
                let path_str = path.to_string_lossy();
                let loaded = Self::load_file(&path_str)?;
                corpus.merge(loaded);
            }
        }
        Ok(corpus)
    }

    /// Load from a path: if it's a directory, load all TOML files; if a file, load it.
    pub fn load(path: &str) -> Result<Self> {
        if Path::new(path).is_dir() {
            Self::load_dir(path)
        } else {
            Self::load_file(path)
        }
    }

    /// Merge another corpus into this one (appends docs).
    pub fn merge(&mut self, other: Self) {
        self.docs.extend(other.docs);
    }

    /// Render as a `<context_pack>` block, truncating to `max_chars` total.
    /// Whole docs are dropped (not split) to avoid broken context.
    pub fn render(&self, max_chars: usize) -> String {
        if self.docs.is_empty() {
            return String::new();
        }
        let mut buf = String::from("<context_pack>\n");
        for doc in &self.docs {
            let block = format!("## {}\n{}\n\n", doc.title, doc.text.trim());
            if buf.len() + block.len() + "</context_pack>".len() > max_chars {
                break;
            }
            buf.push_str(&block);
        }
        buf.push_str("</context_pack>");
        buf
    }

    pub fn len(&self) -> usize {
        self.docs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.docs.is_empty()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_corpus_loads_and_has_docs() {
        let corpus = ContextCorpus::embedded();
        assert!(!corpus.is_empty(), "embedded corpus must have at least one doc");
        assert!(corpus.len() >= 4, "expected 4 default docs");
    }

    #[test]
    fn embedded_corpus_has_expected_ids() {
        let corpus = ContextCorpus::embedded();
        let ids: Vec<&str> = corpus.docs.iter().map(|d| d.id.as_str()).collect();
        assert!(ids.contains(&"schema.telemetry"), "missing schema.telemetry");
        assert!(ids.contains(&"threat.prompt_injection"), "missing threat.prompt_injection");
        assert!(ids.contains(&"threat.social_engineering"), "missing threat.social_engineering");
        assert!(ids.contains(&"threat.adversarial_probing"), "missing threat.adversarial_probing");
    }

    #[test]
    fn render_produces_context_pack_tags() {
        let corpus = ContextCorpus::embedded();
        let rendered = corpus.render(usize::MAX);
        assert!(rendered.starts_with("<context_pack>"), "must start with opening tag");
        assert!(rendered.ends_with("</context_pack>"), "must end with closing tag");
        assert!(rendered.contains("## "), "must contain doc headers");
    }

    #[test]
    fn render_empty_corpus_is_empty_string() {
        let corpus = ContextCorpus::default();
        assert_eq!(corpus.render(usize::MAX), "");
    }

    #[test]
    fn render_respects_max_chars() {
        let corpus = ContextCorpus::embedded();
        // Only allow enough for the wrapper + maybe one doc
        let tiny_limit = 200;
        let rendered = corpus.render(tiny_limit);
        assert!(rendered.len() <= tiny_limit || rendered == "<context_pack>\n</context_pack>");
    }

    #[test]
    fn merge_combines_docs() {
        let mut a = ContextCorpus {
            docs: vec![ContextDoc {
                id: "a".into(), title: "Doc A".into(), text: "text a".into(), tags: vec![],
            }],
        };
        let b = ContextCorpus {
            docs: vec![ContextDoc {
                id: "b".into(), title: "Doc B".into(), text: "text b".into(), tags: vec![],
            }],
        };
        a.merge(b);
        assert_eq!(a.len(), 2);
    }

    #[test]
    fn load_file_returns_err_on_missing_file() {
        let result = ContextCorpus::load_file("/nonexistent/path/does_not_exist.toml");
        assert!(result.is_err());
    }

    #[test]
    fn load_file_returns_err_on_invalid_toml() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.toml");
        std::fs::write(&path, "not valid [[toml").unwrap();
        let result = ContextCorpus::load_file(path.to_str().unwrap());
        assert!(result.is_err());
    }

    #[test]
    fn load_dir_reads_all_toml_files() {
        let dir = tempfile::tempdir().unwrap();
        let toml_a = "[[docs]]\nid=\"a\"\ntitle=\"A\"\ntext=\"text a\"\n";
        let toml_b = "[[docs]]\nid=\"b\"\ntitle=\"B\"\ntext=\"text b\"\n";
        std::fs::write(dir.path().join("a.toml"), toml_a).unwrap();
        std::fs::write(dir.path().join("b.toml"), toml_b).unwrap();
        std::fs::write(dir.path().join("ignore.txt"), "not toml").unwrap();
        let corpus = ContextCorpus::load_dir(dir.path().to_str().unwrap()).unwrap();
        assert_eq!(corpus.len(), 2, "must load exactly 2 TOML docs, not the .txt file");
    }

    #[test]
    fn load_dispatches_file_vs_dir() {
        // file path
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("single.toml");
        std::fs::write(&path, "[[docs]]\nid=\"x\"\ntitle=\"X\"\ntext=\"t\"\n").unwrap();
        let corpus = ContextCorpus::load(path.to_str().unwrap()).unwrap();
        assert_eq!(corpus.len(), 1);

        // dir path
        let corpus2 = ContextCorpus::load(dir.path().to_str().unwrap()).unwrap();
        assert_eq!(corpus2.len(), 1);
    }
}
