//! Poll-based tail of LEAPP's `Screen_Output.html`, producing plain-text line batches, and the
//! streaming loop that follows a spawned process (ARCHITECTURE.md D7, §6 steps 5–6).
//!
//! LEAPP appends one record per message: `message<br>` plus a newline (`\n`, or `\r\n` on
//! Windows), opening and closing the file each time (LEAPP-CLI.md Q8). Messages are not
//! HTML-escaped and may contain markup or evidence-derived text, so every record becomes plain
//! text: decoded as UTF-8 lossily, tags stripped, one line per text line, and lines over 8 KiB
//! truncated with `…`.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::process::{ExitInfo, Handle};

/// How often a run's `Screen_Output.html` is polled (ARCHITECTURE.md §6 step 5).
pub const POLL_INTERVAL: Duration = Duration::from_millis(250);
/// The longest line emitted, in bytes (including the `…` of a truncated line).
pub const MAX_LINE_BYTES: usize = 8 * 1024;
/// The most lines in one batch (`RunEvent::Log`, CONTRACTS.md §11).
pub const MAX_BATCH_LINES: usize = 500;
/// How many stdout/stderr lines `follow` returns (`RunEvent::StdioTail`).
pub const STDIO_TAIL_LINES: usize = 200;

/// The record separator; a newline must follow it.
const BREAK: &[u8] = b"<br>";
/// At most this much of the file is read per poll; the rest follows on the next ones.
const MAX_READ_PER_POLL: u64 = 4 << 20;
/// A record still unterminated at this size is emitted (truncated) so memory stays bounded.
const MAX_PENDING_BYTES: usize = 1 << 20;
/// `last_lines` reads at most this much from the end of a file.
const LAST_LINES_MAX_BYTES: u64 = 1 << 20;
const ELLIPSIS: char = '…';

/// Follows a growing `Screen_Output.html`. The file is opened for each poll and never held open.
#[derive(Debug)]
pub struct ScreenOutputTail {
    path: PathBuf,
    offset: u64,
    /// Bytes of a record whose separator has not arrived yet.
    pending: Vec<u8>,
}

impl ScreenOutputTail {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            offset: 0,
            pending: Vec::new(),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The lines of the records completed since the last poll. A file that does not exist (yet)
    /// yields no lines.
    pub fn poll(&mut self) -> io::Result<Vec<String>> {
        let bytes = self.read_new(Some(MAX_READ_PER_POLL))?;
        Ok(self.consume(&bytes))
    }

    /// Everything left, including a last record without a separator. Call it once the writer has
    /// exited.
    pub fn finish(&mut self) -> io::Result<Vec<String>> {
        let bytes = self.read_new(None)?;
        let mut lines = self.consume(&bytes);
        if !self.pending.is_empty() {
            push_record(&self.pending, &mut lines);
            self.pending.clear();
        }
        Ok(lines)
    }

    fn read_new(&mut self, limit: Option<u64>) -> io::Result<Vec<u8>> {
        let mut file = match File::open(&self.path) {
            Ok(file) => file,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e),
        };
        if file.metadata()?.len() < self.offset {
            // The file was replaced or truncated (LEAPP never does this): start over.
            self.offset = 0;
            self.pending.clear();
        }
        file.seek(SeekFrom::Start(self.offset))?;
        let mut bytes = Vec::new();
        match limit {
            Some(limit) => file.take(limit).read_to_end(&mut bytes)?,
            None => file.read_to_end(&mut bytes)?,
        };
        self.offset += bytes.len() as u64;
        Ok(bytes)
    }

    /// Adds bytes read from the file and returns the lines of the records they complete.
    fn consume(&mut self, bytes: &[u8]) -> Vec<String> {
        self.pending.extend_from_slice(bytes);
        let mut lines = Vec::new();
        let mut start = 0;
        let mut search = 0;
        while let Some(found) = find(&self.pending[search..], BREAK) {
            let at = search + found;
            let after = at + BREAK.len();
            let end = match &self.pending[after..] {
                [b'\n', ..] => after + 1,
                [b'\r', b'\n', ..] => after + 2,
                // The newline has not arrived yet.
                [] | [b'\r'] => break,
                // A `<br>` inside a message; the tag is stripped later.
                _ => {
                    search = after;
                    continue;
                }
            };
            push_record(&self.pending[start..at], &mut lines);
            start = end;
            search = end;
        }
        self.pending.drain(..start);
        if self.pending.len() > MAX_PENDING_BYTES {
            // Keep the last bytes, which may be the start of a separator (`<br>\r`).
            let cut = self.pending.len() - (BREAK.len() + 1);
            push_record(&self.pending[..cut], &mut lines);
            self.pending.drain(..cut);
        }
        lines
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// One record as plain-text lines.
fn push_record(record: &[u8], lines: &mut Vec<String>) {
    let text = strip_tags(&String::from_utf8_lossy(record));
    for line in text.split('\n') {
        lines.push(truncate_line(line.strip_suffix('\r').unwrap_or(line)));
    }
}

/// Removes markup: a `<` followed by a letter, `/`, `!` or `?` up to the next `>`. Any other `<`
/// (as in `a < b`) is text.
fn strip_tags(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find('<') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let is_tag = after
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || matches!(c, '/' | '!' | '?'));
        match after.find('>') {
            Some(close) if is_tag => rest = &after[close + 1..],
            _ => {
                out.push('<');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Cuts a line to at most `MAX_LINE_BYTES` bytes, ending in `…`, at a character boundary.
fn truncate_line(line: &str) -> String {
    if line.len() <= MAX_LINE_BYTES {
        return line.to_owned();
    }
    let mut end = MAX_LINE_BYTES - ELLIPSIS.len_utf8();
    while !line.is_char_boundary(end) {
        end -= 1;
    }
    let mut out = String::with_capacity(end + ELLIPSIS.len_utf8());
    out.push_str(&line[..end]);
    out.push(ELLIPSIS);
    out
}

/// The last `count` lines of a text file (a stdout/stderr log), decoded lossily, `\r\n` handled
/// and each line truncated like log lines. Reads at most the last 1 MiB. A missing file has no
/// lines.
pub fn last_lines(path: &Path, count: usize) -> io::Result<Vec<String>> {
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let start = file.metadata()?.len().saturating_sub(LAST_LINES_MAX_BYTES);
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    let text = String::from_utf8_lossy(&bytes);
    let mut lines: Vec<&str> = text.split('\n').collect();
    if start > 0 {
        // The first line is cut off.
        lines.remove(0);
    }
    if lines.last() == Some(&"") {
        lines.pop();
    }
    let skip = lines.len().saturating_sub(count);
    Ok(lines[skip..]
        .iter()
        .map(|line| truncate_line(line.strip_suffix('\r').unwrap_or(line)))
        .collect())
}

/// How a followed process ended, with the tails of its stdout and stderr logs.
#[derive(Clone, Debug)]
pub struct StreamEnd {
    pub exit: ExitInfo,
    /// The last 200 lines of stdout (`RunEvent::StdioTail`); empty if the log was unreadable.
    pub stdout_tail: Vec<String>,
    /// The last 200 lines of stderr; empty if the log was unreadable.
    pub stderr_tail: Vec<String>,
}

/// Streams `tail` while `handle` runs: every `interval` the new lines go to `on_lines` in batches
/// of at most 500. After the exit it drains the tail (including a last unterminated record) and
/// reads the last 200 lines of the stdout and stderr logs. Tail read errors are logged and retried
/// on the next poll; they never end the stream.
pub fn follow(
    handle: &Handle,
    tail: &mut ScreenOutputTail,
    interval: Duration,
    mut on_lines: impl FnMut(Vec<String>),
) -> io::Result<StreamEnd> {
    let mut warned = false;
    let exit = loop {
        let exit = handle.wait_timeout(interval)?;
        let lines = if exit.is_some() {
            tail.finish()
        } else {
            tail.poll()
        };
        match lines {
            Ok(lines) => emit_batches(lines, &mut on_lines),
            Err(e) if !warned => {
                log::warn!("cannot read {}: {e}", tail.path().display());
                warned = true;
            }
            Err(_) => {}
        }
        if let Some(exit) = exit {
            break exit;
        }
    };
    Ok(StreamEnd {
        exit,
        stdout_tail: stdio_tail(handle.stdout_log()),
        stderr_tail: stdio_tail(handle.stderr_log()),
    })
}

fn stdio_tail(path: &Path) -> Vec<String> {
    last_lines(path, STDIO_TAIL_LINES).unwrap_or_else(|e| {
        log::warn!("cannot read {}: {e}", path.display());
        Vec::new()
    })
}

fn emit_batches(lines: Vec<String>, on_lines: &mut impl FnMut(Vec<String>)) {
    let mut lines = lines.into_iter().peekable();
    while lines.peek().is_some() {
        on_lines(lines.by_ref().take(MAX_BATCH_LINES).collect());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{self, OpenOptions};
    use std::io::Write;

    fn tail() -> ScreenOutputTail {
        ScreenOutputTail::new("unused")
    }

    const SAMPLE: &[u8] =
        b"Processing started<br>\n<b>iLEAPP</b> v1<br>\r\nx < 5 & y<br>\nlast<br>\n";

    #[test]
    fn splits_records_on_br_and_newline() {
        assert_eq!(
            tail().consume(SAMPLE),
            ["Processing started", "iLEAPP v1", "x < 5 & y", "last"]
        );
    }

    #[test]
    fn every_chunk_boundary_gives_the_same_lines() {
        let expected = tail().consume(SAMPLE);
        for split in 0..=SAMPLE.len() {
            let mut t = tail();
            let mut lines = t.consume(&SAMPLE[..split]);
            lines.extend(t.consume(&SAMPLE[split..]));
            assert_eq!(lines, expected, "split at {split}");
            assert!(t.pending.is_empty());
        }
        // Byte by byte.
        let mut t = tail();
        let lines: Vec<String> = SAMPLE.iter().flat_map(|b| t.consume(&[*b])).collect();
        assert_eq!(lines, expected);
    }

    #[test]
    fn crlf_split_between_polls() {
        let mut t = tail();
        assert!(t.consume(b"one<br>\r").is_empty());
        assert_eq!(t.consume(b"\ntwo<br>"), ["one"]);
        assert_eq!(t.consume(b"\r\n"), ["two"]);
    }

    #[test]
    fn partial_records_wait_for_their_separator() {
        let mut t = tail();
        assert!(t.consume(b"half a rec").is_empty());
        assert!(t.consume(b"ord<b").is_empty());
        assert_eq!(t.consume(b"r>\n"), ["half a record"]);
    }

    #[test]
    fn br_inside_a_message_is_not_a_separator() {
        assert_eq!(tail().consume(b"a<br>b<br/>c<br>\n"), ["abc"]);
    }

    #[test]
    fn strips_tags_but_keeps_other_angle_brackets() {
        for (html, text) in [
            ("<b>bold</b> <a href=\"x\">link</a>", "bold link"),
            ("<!-- note -->text<?pi?>", "text"),
            ("a < b and b > c", "a < b and b > c"),
            ("1 <2", "1 <2"),
            ("<unclosed", "<unclosed"),
            ("<", "<"),
            ("", ""),
        ] {
            assert_eq!(strip_tags(html), text, "{html:?}");
        }
    }

    #[test]
    fn invalid_utf8_is_replaced() {
        assert_eq!(
            tail().consume(b"caf\xe9 \xff\xfe<br>\n"),
            ["caf\u{fffd} \u{fffd}\u{fffd}"]
        );
    }

    #[test]
    fn embedded_newlines_become_separate_lines() {
        assert_eq!(
            tail().consume(b"Traceback:\r\n  line 1\nError<br>\r\n"),
            ["Traceback:", "  line 1", "Error"]
        );
        assert_eq!(tail().consume(b"<br>\n"), [""]);
    }

    #[test]
    fn huge_lines_are_truncated_at_a_char_boundary() {
        let line = "é".repeat(6000); // 12,000 bytes
        let mut record = line.clone().into_bytes();
        record.extend_from_slice(b"<br>\n");
        let lines = tail().consume(&record);
        assert_eq!(lines.len(), 1);
        let cut = &lines[0];
        assert!(cut.len() <= MAX_LINE_BYTES, "{}", cut.len());
        assert!(cut.len() > MAX_LINE_BYTES - 8);
        assert!(cut.ends_with('…'));
        assert!(line.starts_with(cut.trim_end_matches('…')));
        // Exactly the limit is kept whole.
        let exact = "a".repeat(MAX_LINE_BYTES);
        assert_eq!(truncate_line(&exact), exact);
    }

    #[test]
    fn an_endless_record_is_emitted_in_bounded_pieces() {
        let mut t = tail();
        let lines = t.consume(&vec![b'x'; MAX_PENDING_BYTES + 100]);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].ends_with('…'));
        assert!(t.pending.len() <= BREAK.len() + 1);
        assert_eq!(t.consume(b"y<br>\n"), ["xxxxxy"]);
    }

    #[test]
    fn polls_a_growing_file_that_appears_late() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Screen_Output.html");
        let mut t = ScreenOutputTail::new(&path);
        assert!(t.poll().unwrap().is_empty());
        let append = |bytes: &[u8]| {
            let mut file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .unwrap();
            file.write_all(bytes).unwrap();
        };
        append(b"one<br>\ntw");
        assert_eq!(t.poll().unwrap(), ["one"]);
        assert!(t.poll().unwrap().is_empty());
        append(b"o<br>\r\nthree");
        assert_eq!(t.poll().unwrap(), ["two"]);
        // After the writer exited, the unterminated record is a line too.
        assert_eq!(t.finish().unwrap(), ["three"]);
        assert!(t.finish().unwrap().is_empty());
    }

    #[test]
    fn a_replaced_file_is_read_from_the_start() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Screen_Output.html");
        fs::write(&path, "first record<br>\n").unwrap();
        let mut t = ScreenOutputTail::new(&path);
        assert_eq!(t.poll().unwrap(), ["first record"]);
        fs::write(&path, "new<br>\n").unwrap();
        assert_eq!(t.poll().unwrap(), ["new"]);
    }

    #[test]
    fn last_lines_of_a_log() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("leapp.stdout.log");
        assert!(last_lines(&path, 200).unwrap().is_empty());
        fs::write(&path, "").unwrap();
        assert!(last_lines(&path, 200).unwrap().is_empty());
        fs::write(&path, "a\r\nb\nc").unwrap();
        assert_eq!(last_lines(&path, 200).unwrap(), ["a", "b", "c"]);
        assert_eq!(last_lines(&path, 2).unwrap(), ["b", "c"]);
        let many: String = (0..1000).map(|i| format!("line {i}\n")).collect();
        fs::write(&path, many).unwrap();
        let tail = last_lines(&path, 200).unwrap();
        assert_eq!(tail.len(), 200);
        assert_eq!(tail[0], "line 800");
        assert_eq!(tail[199], "line 999");
    }

    #[test]
    fn last_lines_reads_only_the_end_of_a_big_log() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("big.log");
        let mut text = "x".repeat(2 << 20); // one 2 MiB line, cut off by the 1 MiB window
        text.push_str("\nend 1\nend 2\n");
        fs::write(&path, text).unwrap();
        assert_eq!(last_lines(&path, 200).unwrap(), ["end 1", "end 2"]);
    }

    #[test]
    fn batches_hold_at_most_500_lines() {
        let mut sizes = Vec::new();
        let lines: Vec<String> = (0..1200).map(|i| i.to_string()).collect();
        emit_batches(lines, &mut |batch: Vec<String>| sizes.push(batch.len()));
        assert_eq!(sizes, [500, 500, 200]);
        emit_batches(Vec::new(), &mut |_: Vec<String>| panic!("no empty batches"));
    }
}
