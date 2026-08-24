//! User-supplied context documents (spec §4). PDF and plain-text extraction so a user
//! can drop a brief, a report, or a transcript into the dashboard and have its facts
//! available to the retrieval layer.

use std::path::Path;

/// Extract text from a document by path. Supports PDF (via pdf-extract) and plain text;
/// other types return an error the UI can surface rather than silently ingesting bytes.
pub fn extract_document_text(path: &str) -> Result<String, String> {
    let p = Path::new(path);
    if !p.exists() {
        return Err(format!("documents: file not found: {path}"));
    }
    let ext = p
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_default();

    match ext.as_str() {
        "pdf" => pdf_extract::extract_text(p).map_err(|e| format!("documents: pdf: {e}")),
        "txt" | "md" | "markdown" | "text" | "csv" | "json" | "" => {
            std::fs::read_to_string(p).map_err(|e| format!("documents: read: {e}"))
        }
        other => Err(format!("documents: unsupported type '.{other}'")),
    }
}

/// Normalize extracted text: collapse runaway whitespace that PDF extraction leaves, so
/// the retrieval tokenizer isn't fed page-break noise.
pub fn normalize_extracted(text: &str) -> String {
    text.lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn reads_plain_text() {
        let dir = std::env::temp_dir();
        let path = dir.join("verity_test_doc.txt");
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(f, "Unemployment was 4.2% in May.").unwrap();
        let out = extract_document_text(path.to_str().unwrap()).unwrap();
        assert!(out.contains("4.2%"));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn missing_file_errors() {
        assert!(extract_document_text("/no/such/file.txt").is_err());
    }

    #[test]
    fn unsupported_type_errors() {
        let dir = std::env::temp_dir();
        let path = dir.join("verity_test.xyz");
        std::fs::write(&path, b"x").unwrap();
        assert!(extract_document_text(path.to_str().unwrap()).is_err());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn normalize_collapses_blank_lines() {
        let input = "line one\n\n\n   line two   \n\n";
        assert_eq!(normalize_extracted(input), "line one\nline two");
    }
}
