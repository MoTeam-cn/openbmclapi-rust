//! Cursor and whitespace handling shared by the block, flow and scalar readers.

use super::error::YamlError;

/// A character cursor that tracks 1-based line numbers and columns.
pub(super) struct Parser {
    chars: Vec<char>,
    pos: usize,
    line: usize,
    line_start: usize,
}

/// A saved cursor position, used to backtrack over an empty nested block.
#[derive(Clone, Copy)]
pub(super) struct Mark {
    pos: usize,
    line: usize,
    line_start: usize,
}

impl Parser {
    /// Wrap an input string in a cursor positioned at its first character.
    pub(super) fn new(input: &str) -> Self {
        Parser {
            chars: input.chars().collect(),
            pos: 0,
            line: 1,
            line_start: 0,
        }
    }

    /// 1-based line the cursor is on.
    pub(super) fn line(&self) -> usize {
        self.line
    }

    /// Build an error anchored at the cursor.
    pub(super) fn error(&self, message: impl Into<String>) -> YamlError {
        YamlError::new(self.line, message)
    }

    /// Save the cursor so an empty nested block can leave it untouched.
    pub(super) fn mark(&self) -> Mark {
        Mark {
            pos: self.pos,
            line: self.line,
            line_start: self.line_start,
        }
    }

    /// Restore a position saved by mark.
    pub(super) fn reset(&mut self, mark: Mark) {
        self.pos = mark.pos;
        self.line = mark.line;
        self.line_start = mark.line_start;
    }

    /// Character under the cursor.
    pub(super) fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    /// Character offset positions ahead of the cursor.
    pub(super) fn peek_at(&self, offset: usize) -> Option<char> {
        self.chars.get(self.pos + offset).copied()
    }

    /// Consume one character, keeping the line counter current.
    pub(super) fn bump(&mut self) -> Option<char> {
        let c = self.chars.get(self.pos).copied()?;
        self.pos += 1;
        if c == '\n' {
            self.line += 1;
            self.line_start = self.pos;
        }
        Some(c)
    }

    /// Column of the cursor inside its line.
    pub(super) fn column(&self) -> usize {
        self.pos - self.line_start
    }

    /// Consume spaces and tabs, stopping at a line break.
    pub(super) fn skip_inline_spaces(&mut self) {
        while matches!(self.peek(), Some(' ') | Some('\t')) {
            self.bump();
        }
    }

    /// True when only a line break or a comment follows on this line.
    pub(super) fn at_line_end(&self) -> bool {
        matches!(self.peek(), None | Some('\n') | Some('\r') | Some('#'))
    }

    /// Advance past blank lines, comments and indentation to the next content character.
    ///
    /// Returns false at end of input; a tab used for indentation is an error.
    pub(super) fn skip_to_content(&mut self) -> Result<bool, YamlError> {
        loop {
            match self.peek() {
                None => return Ok(false),
                Some(' ') => {
                    self.bump();
                }
                Some('\t') => {
                    if self.pos == self.line_start {
                        return Err(self.error("tab characters are not allowed for indentation"));
                    }
                    self.bump();
                }
                Some('\r') | Some('\n') => {
                    self.bump();
                }
                Some('#') => self.skip_comment(),
                _ => {
                    self.reject_document_marker()?;
                    return Ok(true);
                }
            }
        }
    }

    /// True when the cursor is on a minus sign that opens a block sequence entry.
    pub(super) fn is_sequence_entry(&self) -> bool {
        self.peek() == Some('-')
            && matches!(
                self.peek_at(1),
                None | Some(' ') | Some('\t') | Some('\n') | Some('\r')
            )
    }

    /// A hash only starts a comment at the start of a line or after whitespace.
    pub(super) fn comment_starts_here(&self) -> bool {
        self.pos == self.line_start
            || matches!(
                self.pos.checked_sub(1).and_then(|i| self.chars.get(i)),
                Some(' ') | Some('\t')
            )
    }

    /// Skip whitespace and comments inside a flow collection, crossing lines.
    pub(super) fn skip_flow_ws(&mut self) {
        loop {
            match self.peek() {
                Some(' ') | Some('\t') | Some('\n') | Some('\r') => {
                    self.bump();
                }
                Some('#') if self.comment_starts_here() => self.skip_comment(),
                _ => break,
            }
        }
    }

    /// True when text follows the cursor verbatim.
    pub(super) fn starts_with(&self, text: &str) -> bool {
        text.chars()
            .enumerate()
            .all(|(offset, c)| self.peek_at(offset) == Some(c))
    }

    fn skip_comment(&mut self) {
        while let Some(c) = self.peek() {
            if c == '\n' {
                break;
            }
            self.bump();
        }
    }

    fn reject_document_marker(&self) -> Result<(), YamlError> {
        if self.column() != 0 {
            return Ok(());
        }
        for marker in ["---", "..."] {
            if !self.starts_with(marker) {
                continue;
            }
            if matches!(
                self.peek_at(marker.len()),
                None | Some(' ') | Some('\t') | Some('\n') | Some('\r')
            ) {
                return Err(self.error(format!("document markers ({marker}) are not supported")));
            }
        }
        Ok(())
    }
}
