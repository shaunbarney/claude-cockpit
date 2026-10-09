//! Incremental, append-only reading of session transcripts.
//!
//! Transcripts are JSONL files that only ever grow, so re-reading a whole
//! multi-megabyte file every 10 s just to pick up a few new lines is wasted
//! work. `TailCache` remembers, per file, the byte offset just past the last
//! complete line it consumed and hands each parser only the newly appended
//! complete lines. A trailing partial line (still being written) is left for
//! the next pass. If a file shrinks (truncated) or is replaced (different
//! inode on unix), its state is reset and the file is re-read from the start.

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// Read position within one file.
#[derive(Debug, Clone, Default, PartialEq)]
struct Cursor {
    offset: u64,
    ino: u64,
}

#[cfg(unix)]
fn inode(m: &std::fs::Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt;
    m.ino()
}

#[cfg(not(unix))]
fn inode(_m: &std::fs::Metadata) -> u64 {
    0
}

/// Outcome of reading the newly appended part of a file.
#[derive(Debug, PartialEq)]
pub(crate) struct Chunk {
    /// Complete lines appended since the last read (may be empty).
    pub text: String,
    /// True when the file was truncated/replaced and per-file state must be
    /// discarded before applying `text` (which then starts at byte 0).
    pub reset: bool,
}

/// Read the complete lines appended to `path` since `cur`, advancing `cur`.
/// Returns `None` when nothing new is available (or the file is unreadable).
fn read_appended(path: &Path, cur: &mut Cursor) -> Option<Chunk> {
    let meta = std::fs::metadata(path).ok()?;
    let len = meta.len();
    let ino = inode(&meta);
    let mut reset = false;
    if len < cur.offset || (cur.offset > 0 && ino != cur.ino) {
        *cur = Cursor::default();
        reset = true;
    }
    cur.ino = ino;
    if len == cur.offset {
        return reset.then(|| Chunk {
            text: String::new(),
            reset,
        });
    }
    let mut file = std::fs::File::open(path).ok()?;
    file.seek(SeekFrom::Start(cur.offset)).ok()?;
    let mut buf = Vec::with_capacity((len - cur.offset) as usize);
    file.take(len - cur.offset).read_to_end(&mut buf).ok()?;
    // Consume only through the last newline; a partial trailing line is still
    // being written and will be picked up whole next time.
    let end = buf.iter().rposition(|b| *b == b'\n').map(|i| i + 1)?;
    buf.truncate(end);
    cur.offset += end as u64;
    Some(Chunk {
        text: String::from_utf8_lossy(&buf).into_owned(),
        reset,
    })
}

/// Per-file incremental parse state: each file's cursor plus whatever the
/// parser accumulates for it (`S`).
#[derive(Debug)]
pub struct TailCache<S> {
    files: HashMap<PathBuf, (Cursor, S)>,
}

impl<S> Default for TailCache<S> {
    fn default() -> Self {
        Self {
            files: HashMap::new(),
        }
    }
}

impl<S: Default> TailCache<S> {
    /// Feed newly appended lines of every file in `paths` to `parse`, and drop
    /// state for files that no longer exist in `paths`.
    pub fn update(&mut self, paths: &[PathBuf], mut parse: impl FnMut(&Path, &mut S, &str)) {
        let live: std::collections::HashSet<&PathBuf> = paths.iter().collect();
        self.files.retain(|p, _| live.contains(p));
        for path in paths {
            let (cur, state) = self.files.entry(path.clone()).or_default();
            let Some(chunk) = read_appended(path, cur) else {
                continue;
            };
            if chunk.reset {
                *state = S::default();
            }
            if !chunk.text.is_empty() {
                parse(path, state, &chunk.text);
            }
        }
    }

    /// Iterate the accumulated per-file states.
    pub fn states(&self) -> impl Iterator<Item = &S> {
        self.files.values().map(|(_, s)| s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("cockpit-tail-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    #[test]
    fn reads_only_appended_complete_lines() {
        let p = tmp("append.jsonl");
        std::fs::write(&p, "a\nb\npart").unwrap();
        let mut cache: TailCache<Vec<String>> = TailCache::default();
        let paths = vec![p.clone()];
        let feed = |_: &Path, s: &mut Vec<String>, t: &str| {
            s.extend(t.lines().map(str::to_string));
        };
        cache.update(&paths, feed);
        assert_eq!(cache.states().next().unwrap(), &vec!["a", "b"]);

        // Finish the partial line and add one more.
        let mut f = std::fs::OpenOptions::new().append(true).open(&p).unwrap();
        f.write_all(b"ial\nc\n").unwrap();
        cache.update(&paths, feed);
        assert_eq!(
            cache.states().next().unwrap(),
            &vec!["a", "b", "partial", "c"]
        );

        // Nothing new: state unchanged.
        cache.update(&paths, feed);
        assert_eq!(cache.states().next().unwrap().len(), 4);
    }

    #[test]
    fn truncation_resets_state() {
        let p = tmp("trunc.jsonl");
        std::fs::write(&p, "a\nb\nc\n").unwrap();
        let mut cache: TailCache<Vec<String>> = TailCache::default();
        let paths = vec![p.clone()];
        let feed = |_: &Path, s: &mut Vec<String>, t: &str| {
            s.extend(t.lines().map(str::to_string));
        };
        cache.update(&paths, feed);
        std::fs::write(&p, "x\n").unwrap(); // shorter rewrite
        cache.update(&paths, feed);
        assert_eq!(cache.states().next().unwrap(), &vec!["x"]);
    }

    #[test]
    fn vanished_files_are_dropped() {
        let p = tmp("gone.jsonl");
        std::fs::write(&p, "a\n").unwrap();
        let mut cache: TailCache<Vec<String>> = TailCache::default();
        cache.update(std::slice::from_ref(&p), |_, s, t| s.push(t.to_string()));
        cache.update(&[], |_, s, t| s.push(t.to_string()));
        assert_eq!(cache.states().count(), 0);
    }
}
