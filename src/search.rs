use anyhow::{Context, Result, bail, ensure};
use rusqlite::{Connection, OpenFlags, Row, params};
use serde_json::{Value, json};
use std::path::Path;

const FILE: &str = ".memoc-search.sqlite3";
const REBUILD: &str = "Search index unavailable or incompatible. Run 'memoc build' and deploy the entire _site directory.";

#[derive(Default)]
pub struct Section {
    pub anchor: String,
    pub heading: String,
    pub text: String,
}

pub fn create(root: &Path) -> Result<Connection> {
    let db = Connection::open(root.join(FILE))?;
    db.execute_batch(
        "
        PRAGMA user_version=1;
        BEGIN;
        CREATE TABLE passage (
            id TEXT NOT NULL UNIQUE, source TEXT NOT NULL, position INTEGER NOT NULL,
            title TEXT NOT NULL, heading TEXT NOT NULL, folder TEXT NOT NULL,
            url TEXT NOT NULL, page INTEGER, body TEXT NOT NULL,
            UNIQUE(source, position)
        );
        CREATE VIRTUAL TABLE terms USING fts5(
            title, heading, folder, body, content='passage', content_rowid='rowid'
        );
    ",
    )?;
    Ok(db)
}

pub fn finish(db: Connection) -> Result<()> {
    db.execute_batch("INSERT INTO terms(terms) VALUES('rebuild'); COMMIT;")?;
    Ok(())
}

pub struct Document<'a> {
    pub source: &'a str,
    pub title: &'a str,
    pub folder: &'a str,
    pub url: String,
}

impl Document<'_> {
    pub fn index(
        &self,
        db: &Connection,
        sections: impl IntoIterator<Item = (Section, Option<u32>)>,
    ) -> Result<()> {
        let mut insert = db.prepare_cached(
            "INSERT INTO passage(id,source,position,title,heading,folder,url,page,body)
            VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
        )?;
        let mut position = 0;
        for (section, page) in sections {
            let mut url = self.url.clone();
            if !section.anchor.is_empty() {
                url.push('#');
                url.push_str(&section.anchor);
            }
            let text = if section.text.trim().is_empty() {
                &section.heading
            } else {
                &section.text
            };
            for (chunk, body) in chunks(text).into_iter().enumerate() {
                let id = format!("{url}:{chunk}");
                insert.execute(params![
                    id,
                    self.source,
                    position,
                    self.title,
                    section.heading,
                    self.folder,
                    url,
                    page,
                    body
                ])?;
                position += 1;
            }
        }
        Ok(())
    }

    pub fn index_pdf(&self, db: &Connection, path: &Path) -> Result<()> {
        use pdf_oxide::{PdfDocument, document::ReadingOrder};
        // Some unsupported PDF objects cause the extractor to panic instead of returning an error.
        let pages = std::panic::catch_unwind(|| -> Result<_> {
            let pdf = PdfDocument::open(path)?;
            ensure!(!pdf.is_encrypted(), "encrypted PDF");
            let mut pages = Vec::new();
            for index in 0..pdf.page_count()? {
                let page = u32::try_from(index + 1)?;
                // Use the declared reading order, with column detection for untagged manuals.
                let spans = pdf.extract_spans_with_reading_order(index, ReadingOrder::Structure)?;
                let text = spans
                    .into_iter()
                    .map(|span| span.text)
                    .collect::<Vec<_>>()
                    .join("\n");
                pages.push((
                    Section {
                        anchor: format!("page={page}"),
                        text,
                        heading: String::new(),
                    },
                    Some(page),
                ));
            }
            Ok(pages)
        })
        .unwrap_or_else(|_| Err(anyhow::anyhow!("unsupported PDF content")));
        match pages {
            Ok(pages) => self.index(db, pages),
            Err(error) => {
                eprintln!("memoc: warning: could not index {}: {error}", self.source);
                Ok(())
            }
        }
    }
}

fn chunks(text: &str) -> Vec<String> {
    const WORDS: usize = 250;
    let mut chunks = Vec::new();
    let mut current = Vec::new();
    for paragraph in text.split("\n\n") {
        let words: Vec<_> = paragraph.split_whitespace().collect();
        if !current.is_empty() && current.len() + words.len() > WORDS {
            chunks.push(current.join(" "));
            current.clear();
        }
        for word in words {
            if current.len() == WORDS {
                chunks.push(current.join(" "));
                current.clear();
            }
            current.push(word);
        }
    }
    if !current.is_empty() {
        chunks.push(current.join(" "));
    }
    chunks
}

pub fn open(root: &Path) -> Result<Connection> {
    let db = Connection::open_with_flags(
        root.join(FILE),
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .context(REBUILD)?;
    let version: i64 = db
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .context(REBUILD)?;
    ensure!(version == 1, REBUILD);
    Ok(db)
}

fn passage(row: &Row<'_>) -> rusqlite::Result<Value> {
    Ok(json!({
        "id": row.get::<_, String>(0)?, "title": row.get::<_, String>(1)?,
        "heading": row.get::<_, String>(2)?, "url": row.get::<_, String>(3)?,
        "page": row.get::<_, Option<u32>>(4)?, "text": row.get::<_, String>(5)?,
    }))
}

pub fn search(db: &Connection, query: &str, limit: u64) -> Result<Vec<Value>> {
    let terms: Vec<_> = query
        .split(|ch: char| !ch.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(|word| format!("\"{word}\""))
        .collect();
    if terms.is_empty() {
        return Ok(Vec::new());
    }
    let mut stmt = db.prepare(
        "SELECT p.id,p.title,p.heading,p.url,p.page,snippet(terms,3,'','',' … ',48)
        FROM terms JOIN passage p ON p.rowid=terms.rowid WHERE terms MATCH ?1
        ORDER BY bm25(terms,4,3,2,1), p.source, p.position LIMIT ?2",
    )?;
    Ok(stmt
        .query_map(params![terms.join(" OR "), limit], passage)?
        .collect::<rusqlite::Result<_>>()?)
}

pub fn fetch(db: &Connection, id: &str, context: u64) -> Result<Vec<Value>> {
    let mut stmt = db.prepare(
        "SELECT p.id,p.title,p.heading,p.url,p.page,p.body
        FROM passage hit JOIN passage p ON p.source=hit.source
        WHERE hit.id=?1 AND p.position BETWEEN hit.position-?2 AND hit.position+?2
        ORDER BY p.position",
    )?;
    let rows: Vec<_> = stmt
        .query_map(params![id, context], passage)?
        .collect::<rusqlite::Result<_>>()?;
    if rows.is_empty() {
        bail!("Unknown passage ID. Search again for a current ID.");
    }
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::Folder;

    #[test]
    fn citations_follow_rendered_sections_and_physical_pdf_pages() {
        let input = Folder::new();
        input.write("manuals/README.md", "# Kitchen appliances");
        input.write(
            "manuals/Café.md",
            "# Oven\n\n## Modes\nFirst\n\n## Modes\nCafé SOP6302TX",
        );
        input.write(
            "manuals/manual.pdf",
            include_bytes!("../tests/fixtures/manual.pdf"),
        );
        input.write("manuals/bad.pdf", "invalid PDF");
        crate::build(&input.0).unwrap();
        let root = input.0.join("_site");
        let db = open(&root).unwrap();
        let rows = search(&db, "cafe SOP6302TX", 5).unwrap();
        assert_eq!(rows[0]["url"], "/manuals/Caf%C3%A9/#modes-1");
        let rows = search(&db, "interlock", 5).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["url"], "/manuals/manual.pdf#page=1");
        let id = rows[0]["id"].as_str().unwrap();
        let context = fetch(&db, id, 2).unwrap();
        assert_eq!(context.len(), 2);
        assert_eq!(context[1]["page"], 3);
        assert!(
            context[1]["text"]
                .as_str()
                .unwrap()
                .contains("silver latch")
        );
        assert_eq!(fetch(&db, id, 0).unwrap().len(), 1);
        assert!(fetch(&db, "unknown", 1).is_err());
        assert!(search(&db, "zzzznonexistent", 5).unwrap().is_empty());
        assert_eq!(
            std::fs::read(root.join("manuals/bad.pdf")).unwrap(),
            b"invalid PDF"
        );
    }

    #[test]
    fn publish_updates_and_rolls_back_index_with_site() {
        let input = Folder::new();
        input.write("mcp.md", "# MCP\nFirstword");
        crate::build(&input.0).unwrap();
        let root = input.0.join("_site");
        let find = |term| search(&open(&root).unwrap(), term, 5).unwrap();
        assert_eq!(find("firstword")[0]["url"], "/mcp/#mcp");
        input.write("mcp", "conflicting asset");
        assert!(
            crate::build(&input.0)
                .unwrap_err()
                .to_string()
                .contains("/mcp")
        );
        assert_eq!(find("firstword").len(), 1);
        std::fs::remove_file(input.0.join("mcp")).unwrap();
        input.write("mcp.md", "# MCP\nSecondword");
        crate::build(&input.0).unwrap();
        assert!(find("firstword").is_empty());
        assert_eq!(find("secondword").len(), 1);
        std::fs::remove_file(input.0.join("mcp.md")).unwrap();
        crate::build(&input.0).unwrap();
        assert!(find("secondword").is_empty());
    }
}
