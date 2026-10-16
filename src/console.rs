//! A bounded buffer of robot console output, shared by the UI and MCP clients.

use std::collections::VecDeque;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

const CAPACITY: usize = 10_000;

#[derive(Serialize, Deserialize, JsonSchema, Copy, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum LineKind {
    /// Printed to stdout by robot code
    Stdout,
    /// Reported through the WPILib error API
    Error,
    Warning,
    /// A notice from DucTape itself, e.g. "Robot disabled by MCP client"
    DucTape,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct ConsoleLine {
    /// Monotonically increasing sequence number, usable as a cursor
    pub seq: u64,
    /// Milliseconds since the Unix epoch
    pub time_ms: u64,
    pub kind: LineKind,
    pub text: String,
}

#[derive(Default)]
pub struct Console {
    lines: VecDeque<ConsoleLine>,
    next_seq: u64,
}

impl Console {
    pub fn push(&mut self, kind: LineKind, text: String) -> ConsoleLine {
        let time_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or_default();
        let line = ConsoleLine {
            seq: self.next_seq,
            time_ms,
            kind,
            text,
        };
        self.next_seq += 1;
        if self.lines.len() == CAPACITY {
            self.lines.pop_front();
        }
        self.lines.push_back(line.clone());
        line
    }

    /// The sequence number the next line will get
    pub fn next_seq(&self) -> u64 {
        self.next_seq
    }

    /// The last `n` lines
    pub fn recent(&self, n: usize) -> Vec<ConsoleLine> {
        let skip = self.lines.len().saturating_sub(n);
        self.lines.iter().skip(skip).cloned().collect()
    }

    /// Lines with `seq >= since` matching `filter`, at most `limit` of them (the oldest first)
    pub fn since(
        &self,
        since: u64,
        limit: usize,
        filter: impl Fn(&ConsoleLine) -> bool,
    ) -> (Vec<ConsoleLine>, bool) {
        let mut matching = self
            .lines
            .iter()
            .filter(|line| line.seq >= since)
            .filter(|line| filter(line));
        let lines: Vec<_> = matching.by_ref().take(limit).cloned().collect();
        let truncated = matching.next().is_some();
        (lines, truncated)
    }

    pub fn clear(&mut self) {
        self.lines.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_and_limits() {
        let mut console = Console::default();
        for i in 0..5 {
            console.push(LineKind::Stdout, format!("line {i}"));
        }
        console.push(LineKind::Error, "boom".into());
        assert_eq!(console.next_seq(), 6);

        let (lines, truncated) = console.since(2, 2, |_| true);
        assert_eq!(lines.iter().map(|l| l.seq).collect::<Vec<_>>(), [2, 3]);
        assert!(truncated);

        let (errors, _) = console.since(0, 10, |l| l.kind == LineKind::Error);
        assert_eq!(errors.len(), 1);
        assert_eq!(console.recent(2).last().unwrap().text, "boom");

        console.clear();
        assert!(console.recent(10).is_empty());
        assert_eq!(console.push(LineKind::Stdout, "x".into()).seq, 6);
    }
}
