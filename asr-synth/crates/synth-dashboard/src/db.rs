use anyhow::{Context, Result};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS vocab (
    id INTEGER PRIMARY KEY,
    term TEXT UNIQUE NOT NULL,
    spoken_auto TEXT NOT NULL,
    spoken_override TEXT,
    reviewed INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS sentences (
    id INTEGER PRIMARY KEY,
    text TEXT NOT NULL,
    spoken TEXT NOT NULL,
    vocab_terms TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending',
    wav_path TEXT,
    parakeet_output TEXT,
    qwen_output TEXT,
    created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS transcriptions (
    id INTEGER PRIMARY KEY,
    text TEXT NOT NULL,
    timestamp TEXT NOT NULL,
    app TEXT,
    review_status TEXT NOT NULL DEFAULT 'pending',
    corrected_text TEXT,
    imported_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS jobs (
    id INTEGER PRIMARY KEY,
    job_type TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'running',
    config TEXT,
    log TEXT NOT NULL DEFAULT '',
    result TEXT,
    created_at TEXT NOT NULL,
    finished_at TEXT
);

CREATE TABLE IF NOT EXISTS candidate_sentences (
    id INTEGER PRIMARY KEY,
    text TEXT UNIQUE NOT NULL,
    spoken TEXT NOT NULL,
    vocab_terms TEXT NOT NULL,
    source TEXT NOT NULL,
    imported_at TEXT NOT NULL
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_transcriptions_dedup
    ON transcriptions(timestamp, text);
"#;

// --- Vocab ---

#[derive(Debug, Serialize, Deserialize)]
pub struct VocabRow {
    pub id: i64,
    pub term: String,
    pub spoken_auto: String,
    pub spoken_override: Option<String>,
    pub reviewed: bool,
}

impl VocabRow {
    /// The effective spoken form: override if set, otherwise auto
    pub fn spoken(&self) -> &str {
        self.spoken_override.as_deref().unwrap_or(&self.spoken_auto)
    }
}

// --- Sentences ---

#[derive(Debug, Serialize, Deserialize)]
pub struct SentenceRow {
    pub id: i64,
    pub text: String,
    pub spoken: String,
    pub vocab_terms: String, // JSON array
    pub status: String,
    pub wav_path: Option<String>,
    pub parakeet_output: Option<String>,
    pub qwen_output: Option<String>,
}

// --- Stats ---

#[derive(Debug, Serialize)]
pub struct CorpusStats {
    pub vocab_total: i64,
    pub vocab_reviewed: i64,
    pub vocab_with_override: i64,
    pub candidates_total: i64,
    pub sentences_total: i64,
    pub sentences_pending: i64,
    pub sentences_approved: i64,
    pub sentences_rejected: i64,
    pub sentences_with_audio: i64,
    pub sentences_with_asr: i64,
    pub transcriptions_total: i64,
}

// --- Jobs ---

#[derive(Debug, Serialize, Deserialize)]
pub struct Job {
    pub id: i64,
    pub job_type: String,
    pub status: String,
    pub config: Option<String>,
    pub log: String,
    pub result: Option<String>,
    pub created_at: String,
    pub finished_at: Option<String>,
}

// --- Log entry for Hark import ---

#[derive(Debug, Deserialize)]
pub struct LogEntry {
    pub text: String,
    pub timestamp: String,
    pub app: Option<String>,
}

pub struct Db {
    conn: Connection,
}

impl Db {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path).context("opening database")?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")
            .context("setting pragmas")?;
        conn.execute_batch(SCHEMA).context("creating schema")?;
        Ok(Db { conn })
    }

    // ==================== VOCAB ====================

    /// Seed the vocab table from the hardcoded PRONUNCIATION_OVERRIDES.
    /// Only inserts terms that don't already exist.
    pub fn seed_overrides(&self) -> Result<usize> {
        let now = now_str();
        let mut count = 0;
        for &(term, spoken) in synth_textgen::corpus::PRONUNCIATION_OVERRIDES {
            let auto = synth_textgen::corpus::to_spoken(term);
            let result = self.conn.execute(
                "INSERT OR IGNORE INTO vocab (term, spoken_auto, spoken_override, reviewed, created_at, updated_at)
                 VALUES (?1, ?2, ?3, 0, ?4, ?4)",
                params![term, auto, spoken, now],
            );
            if let Ok(n) = result {
                count += n;
            }
        }
        Ok(count)
    }

    /// Import extracted vocab entries into the table. Sets spoken_auto but
    /// does NOT overwrite existing spoken_override values.
    pub fn import_vocab(&self, entries: &[synth_textgen::corpus::VocabEntry]) -> Result<usize> {
        let now = now_str();
        let mut count = 0;
        for entry in entries {
            let result = self.conn.execute(
                "INSERT INTO vocab (term, spoken_auto, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?3)
                 ON CONFLICT(term) DO UPDATE SET spoken_auto = excluded.spoken_auto, updated_at = excluded.updated_at",
                params![entry.term, entry.spoken, now],
            );
            if let Ok(n) = result {
                count += n;
            }
        }
        Ok(count)
    }

    pub fn list_vocab(
        &self,
        search: Option<&str>,
        reviewed_only: Option<bool>,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<VocabRow>> {
        let mut conditions = Vec::new();
        let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
        let mut idx = 1;

        if let Some(q) = search {
            conditions.push(format!("term LIKE ?{idx}"));
            param_values.push(Box::new(format!("%{q}%")));
            idx += 1;
        }
        if let Some(rev) = reviewed_only {
            conditions.push(format!("reviewed = ?{idx}"));
            param_values.push(Box::new(rev as i64));
            idx += 1;
        }

        let where_clause = if conditions.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", conditions.join(" AND "))
        };

        let sql = format!(
            "SELECT id, term, spoken_auto, spoken_override, reviewed FROM vocab {where_clause} ORDER BY term COLLATE NOCASE LIMIT ?{idx} OFFSET ?{}",
            idx + 1
        );
        param_values.push(Box::new(limit));
        param_values.push(Box::new(offset));

        let refs: Vec<&dyn rusqlite::types::ToSql> = param_values.iter().map(|p| p.as_ref()).collect();
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(refs.as_slice(), |row| {
            Ok(VocabRow {
                id: row.get(0)?,
                term: row.get(1)?,
                spoken_auto: row.get(2)?,
                spoken_override: row.get(3)?,
                reviewed: row.get::<_, i64>(4)? != 0,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn vocab_count(&self) -> Result<i64> {
        Ok(self.conn.query_row("SELECT COUNT(*) FROM vocab", [], |r| r.get(0))?)
    }

    pub fn update_vocab_override(&self, id: i64, spoken_override: Option<&str>) -> Result<()> {
        self.conn.execute(
            "UPDATE vocab SET spoken_override = ?1, updated_at = ?2 WHERE id = ?3",
            params![spoken_override, now_str(), id],
        )?;
        Ok(())
    }

    pub fn set_vocab_reviewed(&self, id: i64, reviewed: bool) -> Result<()> {
        self.conn.execute(
            "UPDATE vocab SET reviewed = ?1, updated_at = ?2 WHERE id = ?3",
            params![reviewed as i64, now_str(), id],
        )?;
        Ok(())
    }

    /// Get a map of term → effective spoken form for all vocab with overrides
    pub fn get_spoken_overrides(&self) -> Result<HashMap<String, String>> {
        let mut stmt = self.conn.prepare(
            "SELECT term, COALESCE(spoken_override, spoken_auto) FROM vocab WHERE spoken_override IS NOT NULL",
        )?;
        let mut map = HashMap::new();
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        for row in rows {
            let (term, spoken) = row?;
            map.insert(term, spoken);
        }
        Ok(map)
    }

    // ==================== SENTENCES ====================

    pub fn insert_sentences(&self, sentences: &[synth_textgen::templates::GeneratedSentence]) -> Result<usize> {
        let now = now_str();
        let mut count = 0;
        for s in sentences {
            let vocab_json = serde_json::to_string(&s.vocab_terms)?;
            self.conn.execute(
                "INSERT INTO sentences (text, spoken, vocab_terms, status, created_at) VALUES (?1, ?2, ?3, 'pending', ?4)",
                params![s.text, s.spoken, vocab_json, now],
            )?;
            count += 1;
        }
        Ok(count)
    }

    pub fn list_sentences(
        &self,
        status: Option<&str>,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<SentenceRow>> {
        let (sql, param_values): (String, Vec<Box<dyn rusqlite::types::ToSql>>) = match status {
            Some(s) => (
                "SELECT id, text, spoken, vocab_terms, status, wav_path, parakeet_output, qwen_output \
                 FROM sentences WHERE status = ?1 ORDER BY id LIMIT ?2 OFFSET ?3".into(),
                vec![Box::new(s.to_string()), Box::new(limit), Box::new(offset)],
            ),
            None => (
                "SELECT id, text, spoken, vocab_terms, status, wav_path, parakeet_output, qwen_output \
                 FROM sentences ORDER BY id LIMIT ?1 OFFSET ?2".into(),
                vec![Box::new(limit), Box::new(offset)],
            ),
        };
        let refs: Vec<&dyn rusqlite::types::ToSql> = param_values.iter().map(|p| p.as_ref()).collect();
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(refs.as_slice(), |row| {
            Ok(SentenceRow {
                id: row.get(0)?,
                text: row.get(1)?,
                spoken: row.get(2)?,
                vocab_terms: row.get(3)?,
                status: row.get(4)?,
                wav_path: row.get(5)?,
                parakeet_output: row.get(6)?,
                qwen_output: row.get(7)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn update_sentence_status(&self, id: i64, status: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE sentences SET status = ?1 WHERE id = ?2",
            params![status, id],
        )?;
        Ok(())
    }

    pub fn update_sentence_spoken(&self, id: i64, spoken: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE sentences SET spoken = ?1, status = 'needs_resynth' WHERE id = ?2",
            params![spoken, id],
        )?;
        Ok(())
    }

    pub fn update_sentence_wav(&self, id: i64, wav_path: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE sentences SET wav_path = ?1 WHERE id = ?2",
            params![wav_path, id],
        )?;
        Ok(())
    }

    pub fn update_sentence_asr(&self, id: i64, parakeet: &str, qwen: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE sentences SET parakeet_output = ?1, qwen_output = ?2 WHERE id = ?3",
            params![parakeet, qwen, id],
        )?;
        Ok(())
    }

    // ==================== CANDIDATES ====================

    pub fn insert_candidate(&self, text: &str, spoken: &str, vocab_terms: &str, source: &str) -> Result<bool> {
        let result = self.conn.execute(
            "INSERT OR IGNORE INTO candidate_sentences (text, spoken, vocab_terms, source, imported_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![text, spoken, vocab_terms, source, now_str()],
        )?;
        Ok(result > 0)
    }

    pub fn candidate_count(&self) -> Result<i64> {
        Ok(self.conn.query_row("SELECT COUNT(*) FROM candidate_sentences", [], |r| r.get(0))?)
    }

    /// Pick N random candidates that aren't already in the sentences table
    pub fn pick_candidates(&self, count: i64) -> Result<Vec<(String, String, String)>> {
        let mut stmt = self.conn.prepare(
            "SELECT c.text, c.spoken, c.vocab_terms FROM candidate_sentences c
             WHERE c.text NOT IN (SELECT text FROM sentences)
             ORDER BY RANDOM() LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![count], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?))
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    // ==================== STATS ====================

    pub fn stats(&self) -> Result<CorpusStats> {
        Ok(CorpusStats {
            vocab_total: self.conn.query_row("SELECT COUNT(*) FROM vocab", [], |r| r.get(0))?,
            vocab_reviewed: self.conn.query_row("SELECT COUNT(*) FROM vocab WHERE reviewed = 1", [], |r| r.get(0))?,
            vocab_with_override: self.conn.query_row("SELECT COUNT(*) FROM vocab WHERE spoken_override IS NOT NULL", [], |r| r.get(0))?,
            candidates_total: self.conn.query_row("SELECT COUNT(*) FROM candidate_sentences", [], |r| r.get(0))?,
            sentences_total: self.conn.query_row("SELECT COUNT(*) FROM sentences", [], |r| r.get(0))?,
            sentences_pending: self.conn.query_row("SELECT COUNT(*) FROM sentences WHERE status = 'pending'", [], |r| r.get(0))?,
            sentences_approved: self.conn.query_row("SELECT COUNT(*) FROM sentences WHERE status = 'approved'", [], |r| r.get(0))?,
            sentences_rejected: self.conn.query_row("SELECT COUNT(*) FROM sentences WHERE status = 'rejected'", [], |r| r.get(0))?,
            sentences_with_audio: self.conn.query_row("SELECT COUNT(*) FROM sentences WHERE wav_path IS NOT NULL", [], |r| r.get(0))?,
            sentences_with_asr: self.conn.query_row("SELECT COUNT(*) FROM sentences WHERE parakeet_output IS NOT NULL", [], |r| r.get(0))?,
            transcriptions_total: self.conn.query_row("SELECT COUNT(*) FROM transcriptions", [], |r| r.get(0))?,
        })
    }

    // ==================== HARK IMPORT ====================

    pub fn import_hark_log(&self, path: &Path) -> Result<usize> {
        let content = std::fs::read_to_string(path).context("reading JSONL file")?;
        let now = now_str();
        let mut count = 0usize;
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() { continue; }
            let entry: LogEntry = match serde_json::from_str(line) {
                Ok(e) => e,
                Err(_) => continue,
            };
            let result = self.conn.execute(
                "INSERT OR IGNORE INTO transcriptions (text, timestamp, app, imported_at) VALUES (?1, ?2, ?3, ?4)",
                params![entry.text, entry.timestamp, entry.app, now],
            );
            if let Ok(changed) = result { count += changed; }
        }
        Ok(count)
    }

    // ==================== JOBS ====================

    pub fn create_job(&self, job_type: &str, config: Option<&str>) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO jobs (job_type, status, config, created_at) VALUES (?1, 'running', ?2, ?3)",
            params![job_type, config, now_str()],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn append_job_log(&self, job_id: i64, line: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE jobs SET log = log || ?1 || char(10) WHERE id = ?2",
            params![line, job_id],
        )?;
        Ok(())
    }

    pub fn finish_job(&self, job_id: i64, status: &str, result: Option<&str>) -> Result<()> {
        self.conn.execute(
            "UPDATE jobs SET status = ?1, result = ?2, finished_at = ?3 WHERE id = ?4",
            params![status, result, now_str(), job_id],
        )?;
        Ok(())
    }

    pub fn get_job(&self, job_id: i64) -> Result<Option<Job>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, job_type, status, config, log, result, created_at, finished_at FROM jobs WHERE id = ?1",
        )?;
        let mut rows = stmt.query_map(params![job_id], |row| {
            Ok(Job {
                id: row.get(0)?, job_type: row.get(1)?, status: row.get(2)?,
                config: row.get(3)?, log: row.get(4)?, result: row.get(5)?,
                created_at: row.get(6)?, finished_at: row.get(7)?,
            })
        })?;
        match rows.next() {
            Some(r) => Ok(Some(r?)),
            None => Ok(None),
        }
    }

    pub fn list_jobs(&self) -> Result<Vec<Job>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, job_type, status, config, log, result, created_at, finished_at FROM jobs ORDER BY id DESC LIMIT 50",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(Job {
                id: row.get(0)?, job_type: row.get(1)?, status: row.get(2)?,
                config: row.get(3)?, log: row.get(4)?, result: row.get(5)?,
                created_at: row.get(6)?, finished_at: row.get(7)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }
}

pub fn now_str() -> String {
    use std::time::SystemTime;
    let dur = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap();
    format!("{}Z", dur.as_secs())
}
