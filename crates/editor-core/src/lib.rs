//! Original UI-independent document and editing contracts for `glyphfolio-editor`.
//!
//! Markdown is stored verbatim. All mutations pass through validated transactions;
//! presentation and platform input belong to adapters. Text storage and Unicode
//! segmentation use low-level libraries, never an existing editor controller.
//!
//! This initial engine is correctness-first. Snapshot export, dirty comparison,
//! grapheme commands, and grouped-history event diffs can scan the whole document.
//! They are not a claim of large-document or interactive performance qualification.

#![forbid(unsafe_code)]

use std::collections::VecDeque;
use std::fmt;
use std::ops::Range;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::time::{Duration, Instant};

use ropey::Rope;
use unicode_segmentation::UnicodeSegmentation;

mod table;
pub use table::{ColumnAlignment, MarkdownTable, TableCommand, TableRow};
mod commands;
pub use commands::{EditCommand, HeadingLevel};

static NEXT_DOCUMENT: AtomicU64 = AtomicU64::new(1);
const DEFAULT_HISTORY_BYTES: usize = 8 * 1024 * 1024;
const SUBSCRIBER_CAPACITY: usize = 64;

/// Absolute UTF-8 source-byte offset, validated by document operations.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ByteOffset(pub usize);

/// Absolute Unicode scalar index, distinct from source bytes and graphemes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ScalarIndex(pub usize);

/// Monotonic content revision. Undo and redo also advance it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Revision(pub u64);

/// Process-local document-session identity, distinct even for identical text.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DocumentId(u64);

/// Boundary association for an insertion or an ambiguous visual caret position.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Affinity {
    Before,
    #[default]
    After,
}

/// Directed logical source selection. Geometry remains adapter-owned.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Selection {
    pub anchor: ByteOffset,
    pub head: ByteOffset,
    pub affinity: Affinity,
}

impl Selection {
    pub fn caret(offset: ByteOffset) -> Self {
        Self {
            anchor: offset,
            head: offset,
            affinity: Affinity::After,
        }
    }

    pub fn start(self) -> ByteOffset {
        self.anchor.min(self.head)
    }
    pub fn end(self) -> ByteOffset {
        self.anchor.max(self.head)
    }
    pub fn range(self) -> Range<ByteOffset> {
        self.start()..self.end()
    }
    pub fn is_collapsed(self) -> bool {
        self.anchor == self.head
    }
}

/// Content origin controls history boundaries, not mutation authority.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TransactionOrigin {
    Typing,
    Delete,
    Paste,
    Composition,
    Formatting,
    Undo,
    Redo,
    #[default]
    Programmatic,
}

/// A replacement in a transaction's shared pre-edit coordinate space.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextChange {
    pub range: Range<ByteOffset>,
    pub inserted: String,
}

impl TextChange {
    pub fn new(range: Range<ByteOffset>, inserted: impl Into<String>) -> Self {
        Self {
            range,
            inserted: inserted.into(),
        }
    }
}

/// An immutable shared snapshot. The storage implementation is deliberately private.
#[derive(Clone, Debug)]
pub struct Snapshot {
    document_id: DocumentId,
    revision: Revision,
    text: Rope,
    content_state: u64,
}

impl Snapshot {
    pub fn document_id(&self) -> DocumentId {
        self.document_id
    }
    pub fn revision(&self) -> Revision {
        self.revision
    }
    pub fn byte_len(&self) -> usize {
        self.text.len_bytes()
    }
    /// Explicit full-document allocation, intended for persistence and small prototypes.
    pub fn text(&self) -> String {
        self.text.to_string()
    }
    /// Borrowed storage chunks, with no exposed Ropey types.
    pub fn chunks(&self) -> impl Iterator<Item = &str> {
        self.text.chunks()
    }
    pub fn is_char_boundary(&self, offset: ByteOffset) -> bool {
        valid_offset(&self.text, offset).is_ok()
    }
    /// Snap a user caret to an extended-grapheme boundary. A hard line supplies
    /// all required segmentation context (including CRLF) without copying the
    /// rest of a large document. Raw transactions may still use scalar positions.
    pub fn snap_grapheme(
        &self,
        offset: ByteOffset,
        affinity: Affinity,
    ) -> Result<ByteOffset, EditorError> {
        snap_grapheme_in_rope(&self.text, offset, affinity)
    }
    pub fn slice(&self, range: Range<ByteOffset>) -> Result<String, EditorError> {
        validate_range(&self.text, &range)?;
        Ok(self.text.byte_slice(range.start.0..range.end.0).to_string())
    }
    /// Byte to scalar-index conversion with explicit boundary validation.
    pub fn byte_to_scalar(&self, offset: ByteOffset) -> Result<ScalarIndex, EditorError> {
        valid_offset(&self.text, offset)?;
        Ok(ScalarIndex(self.text.byte_to_char(offset.0)))
    }
    pub fn scalar_to_byte(&self, scalar: ScalarIndex) -> Result<ByteOffset, EditorError> {
        if scalar.0 > self.text.len_chars() {
            return Err(EditorError::InvalidScalar(scalar));
        }
        Ok(ByteOffset(self.text.char_to_byte(scalar.0)))
    }
}

impl fmt::Display for Snapshot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for chunk in self.text.chunks() {
            f.write_str(chunk)?;
        }
        Ok(())
    }
}

/// A transaction based on one immutable document snapshot.
#[derive(Clone, Debug)]
pub struct Transaction {
    document_id: DocumentId,
    expected_revision: Revision,
    changes: Vec<TextChange>,
    selection_after: Option<Selection>,
    origin: TransactionOrigin,
    group_id: Option<u64>,
    grapheme_selection: Option<Affinity>,
}

impl Transaction {
    pub fn new(snapshot: &Snapshot) -> Self {
        Self {
            document_id: snapshot.document_id,
            expected_revision: snapshot.revision,
            changes: Vec::new(),
            selection_after: None,
            origin: TransactionOrigin::Programmatic,
            group_id: None,
            grapheme_selection: None,
        }
    }
    pub fn replace(mut self, range: Range<ByteOffset>, inserted: impl Into<String>) -> Self {
        self.changes.push(TextChange::new(range, inserted));
        self
    }
    pub fn with_selection(mut self, selection: Selection) -> Self {
        self.selection_after = Some(selection);
        self
    }
    /// Normalize the resulting user selection after edits can join adjacent
    /// graphemes. Omit this for explicit programmatic scalar-coordinate edits.
    pub fn with_grapheme_selection(mut self, affinity: Affinity) -> Self {
        self.grapheme_selection = Some(affinity);
        self
    }
    pub fn with_origin(mut self, origin: TransactionOrigin) -> Self {
        self.origin = origin;
        self
    }
    /// Explicit typing group hint; adjacency and selection-continuity checks still apply.
    pub fn with_group(mut self, group_id: u64) -> Self {
        self.group_id = Some(group_id);
        self
    }
}

/// Mapping of validated source positions across an atomic content change.
#[derive(Clone, Debug)]
pub struct ChangeMap {
    before_bytes: usize,
    changes: Vec<TextChange>,
}

impl ChangeMap {
    /// The caller's offset must be a valid boundary in the pre-edit snapshot.
    pub fn map_offset(
        &self,
        offset: ByteOffset,
        affinity: Affinity,
    ) -> Result<ByteOffset, EditorError> {
        if offset.0 > self.before_bytes {
            return Err(EditorError::InvalidOffset(offset));
        }
        let mut delta: i128 = 0;
        for change in &self.changes {
            if offset < change.range.start {
                break;
            }
            if offset == change.range.start || offset < change.range.end {
                let start = (change.range.start.0 as i128 + delta) as usize;
                return Ok(ByteOffset(
                    start
                        + if affinity == Affinity::After {
                            change.inserted.len()
                        } else {
                            0
                        },
                ));
            }
            delta +=
                change.inserted.len() as i128 - (change.range.end.0 - change.range.start.0) as i128;
        }
        Ok(ByteOffset((offset.0 as i128 + delta) as usize))
    }
    pub fn map_selection(&self, selection: Selection) -> Result<Selection, EditorError> {
        Ok(Selection {
            anchor: self.map_offset(selection.anchor, selection.affinity)?,
            head: self.map_offset(selection.head, selection.affinity)?,
            affinity: selection.affinity,
        })
    }
}

/// One atomic observer event. `changes` use the `before_revision` coordinate space.
#[derive(Clone, Debug)]
pub struct CommittedChange {
    pub document_id: DocumentId,
    pub before_revision: Revision,
    pub revision: Revision,
    pub selection_before: Selection,
    pub selection_after: Selection,
    pub origin: TransactionOrigin,
    pub changes: Vec<TextChange>,
    pub change_map: ChangeMap,
    pub snapshot: Snapshot,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EditorError {
    ForeignDocument {
        expected: DocumentId,
        actual: DocumentId,
    },
    StaleRevision {
        expected: Revision,
        actual: Revision,
    },
    InvalidOffset(ByteOffset),
    InvalidRange(Range<ByteOffset>),
    InvalidScalar(ScalarIndex),
    OverlappingChanges,
    EmptyTransaction,
    ReservedOrigin,
    RevisionExhausted,
}

impl fmt::Display for EditorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for EditorError {}

/// Presentation metadata only; Rich rendering is not implemented by this crate.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PresentationMode {
    #[default]
    Source,
    Rich,
}

#[derive(Clone, Debug, Default)]
pub struct ViewState {
    pub presentation: PresentationMode,
    pub source_anchor: ByteOffset,
    pub preferred_x: Option<f32>,
}

impl ViewState {
    pub fn set_presentation(&mut self, mode: PresentationMode) {
        self.presentation = mode;
    }
}

#[derive(Clone, Debug)]
struct HistoryStep {
    forward: Vec<TextChange>,
    inverse: Vec<TextChange>,
}

#[derive(Debug)]
struct HistoryEntry {
    steps: Vec<HistoryStep>,
    selection_before: Selection,
    selection_after: Selection,
    content_before: u64,
    content_after: u64,
    bytes: usize,
    typing_end: Option<ByteOffset>,
    group_id: Option<u64>,
    last_edit: Instant,
    generation: u64,
}

/// A document session with its original transaction and history engine.
pub struct Editor {
    snapshot: Snapshot,
    selection: Selection,
    saved: Snapshot,
    undo: VecDeque<HistoryEntry>,
    redo: Vec<HistoryEntry>,
    history_limit: usize,
    history_bytes: usize,
    generation: u64,
    next_content_state: u64,
    subscribers: Vec<SyncSender<CommittedChange>>,
}

impl Editor {
    /// Execute an original source-editing command as one atomic history step.
    /// Commands preserve unrelated spelling and line endings; a true no-op
    /// returns `None` without changing the selection, revision, or history.
    pub fn execute(
        &mut self,
        command: EditCommand,
    ) -> Result<Option<CommittedChange>, EditorError> {
        commands::execute(self, command)
    }

    pub fn new(text: &str) -> Self {
        let snapshot = Snapshot {
            document_id: DocumentId(NEXT_DOCUMENT.fetch_add(1, Ordering::Relaxed)),
            revision: Revision(0),
            text: Rope::from_str(text),
            content_state: 0,
        };
        Self {
            saved: snapshot.clone(),
            snapshot,
            selection: Selection::default(),
            undo: VecDeque::new(),
            redo: Vec::new(),
            history_limit: DEFAULT_HISTORY_BYTES,
            history_bytes: 0,
            generation: 0,
            next_content_state: 1,
            subscribers: Vec::new(),
        }
    }

    /// Bounds retained history text plus record estimates. An oversized edit remains
    /// valid but clears retained history. This is not a process/allocator memory limit.
    pub fn with_history_limit(mut self, bytes: usize) -> Self {
        self.history_limit = bytes;
        self.enforce_history_limit();
        self
    }

    pub fn snapshot(&self) -> Snapshot {
        self.snapshot.clone()
    }
    pub fn selection(&self) -> Selection {
        self.selection
    }
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
    pub fn history_bytes(&self) -> usize {
        self.history_bytes
    }

    pub fn set_selection(&mut self, selection: Selection) -> Result<(), EditorError> {
        validate_selection(&self.snapshot.text, selection)?;
        if selection != self.selection {
            self.break_history_group();
        }
        self.selection = selection;
        Ok(())
    }

    pub fn break_history_group(&mut self) {
        self.generation = self.generation.wrapping_add(1);
    }

    /// Bounded nonblocking change stream (64 events). A lagging subscriber is
    /// disconnected rather than silently receiving an incomplete stream. After
    /// disconnection the host must obtain a fresh snapshot and subscribe again.
    /// Hosts must check revisions and cannot treat disconnection as an idle stream.
    pub fn subscribe(&mut self) -> Receiver<CommittedChange> {
        let (sender, receiver) = mpsc::sync_channel(SUBSCRIBER_CAPACITY);
        self.subscribers.push(sender);
        receiver
    }

    /// Acknowledge exactly the snapshot that reached durable storage. Older than
    /// the last accepted acknowledgement is ignored; newer typing remains dirty.
    /// Returns whether this acknowledgement advanced/confirmed the save baseline.
    pub fn acknowledge_saved(&mut self, snapshot: &Snapshot) -> Result<bool, EditorError> {
        self.ensure_document(snapshot.document_id)?;
        if snapshot.revision > self.snapshot.revision {
            return Err(EditorError::StaleRevision {
                expected: snapshot.revision,
                actual: self.snapshot.revision,
            });
        }
        if snapshot.revision < self.saved.revision {
            return Ok(false);
        }
        self.saved = snapshot.clone();
        self.break_history_group();
        Ok(true)
    }

    /// Content equivalence rather than revision equality. The fallback comparison
    /// can scan the document when separate edit histories produce identical bytes.
    pub fn is_dirty(&self) -> bool {
        self.snapshot.content_state != self.saved.content_state
            && self.snapshot.text != self.saved.text
    }

    pub fn apply(&mut self, transaction: Transaction) -> Result<CommittedChange, EditorError> {
        self.ensure_document(transaction.document_id)?;
        if transaction.expected_revision != self.snapshot.revision {
            return Err(EditorError::StaleRevision {
                expected: transaction.expected_revision,
                actual: self.snapshot.revision,
            });
        }
        if matches!(
            transaction.origin,
            TransactionOrigin::Undo | TransactionOrigin::Redo
        ) {
            return Err(EditorError::ReservedOrigin);
        }
        let mut changes = transaction.changes;
        normalize_changes(&self.snapshot.text, &mut changes)?;
        let before = self.snapshot.clone();
        let selection_before = self.selection;
        let (text, inverse) = apply_changes(&before.text, &changes)?;
        if text == before.text {
            return Err(EditorError::EmptyTransaction);
        }
        let map = ChangeMap {
            before_bytes: before.byte_len(),
            changes: changes.clone(),
        };
        let mut selection_after = match transaction.selection_after {
            Some(selection) => selection,
            None => map.map_selection(selection_before)?,
        };
        validate_selection(&text, selection_after)?;
        if let Some(affinity) = transaction.grapheme_selection {
            selection_after.anchor =
                snap_grapheme_in_rope(&text, selection_after.anchor, affinity)?;
            selection_after.head = snap_grapheme_in_rope(&text, selection_after.head, affinity)?;
        }
        let revision = Revision(
            before
                .revision
                .0
                .checked_add(1)
                .ok_or(EditorError::RevisionExhausted)?,
        );
        let content_after = self.next_content_state;
        let next_content_state = content_after
            .checked_add(1)
            .ok_or(EditorError::RevisionExhausted)?;
        let now = Instant::now();
        let typing_end = typing_end(
            &changes,
            selection_before,
            selection_after,
            transaction.origin,
        );
        let bytes = history_size(&changes, &inverse);
        let can_join = typing_end.is_some()
            && self.undo.back().is_some_and(|last| {
                last.typing_end == Some(selection_before.head)
                    && last.selection_after == selection_before
                    && last.group_id == transaction.group_id
                    && last.generation == self.generation
                    && now.duration_since(last.last_edit) <= Duration::from_millis(400)
            });

        // All fallible validation has completed. Commit atomically from here.
        self.next_content_state = next_content_state;
        for entry in self.redo.drain(..) {
            self.history_bytes -= entry.bytes;
        }
        let step = HistoryStep {
            forward: changes.clone(),
            inverse,
        };
        if can_join {
            let entry = self.undo.back_mut().expect("checked history entry");
            entry.steps.push(step);
            entry.selection_after = selection_after;
            entry.content_after = content_after;
            entry.bytes = entry.bytes.saturating_add(bytes);
            entry.typing_end = typing_end;
            entry.last_edit = now;
        } else {
            self.undo.push_back(HistoryEntry {
                steps: vec![step],
                selection_before,
                selection_after,
                content_before: before.content_state,
                content_after,
                bytes,
                typing_end,
                group_id: transaction.group_id,
                last_edit: now,
                generation: self.generation,
            });
        }
        self.history_bytes = self.history_bytes.saturating_add(bytes);
        self.enforce_history_limit();
        self.snapshot = Snapshot {
            document_id: before.document_id,
            revision,
            text,
            content_state: content_after,
        };
        self.selection = selection_after;
        let event = CommittedChange {
            document_id: before.document_id,
            before_revision: before.revision,
            revision,
            selection_before,
            selection_after,
            origin: transaction.origin,
            changes,
            change_map: map,
            snapshot: self.snapshot.clone(),
        };
        self.notify(&event);
        Ok(event)
    }

    pub fn undo(&mut self) -> Result<Option<CommittedChange>, EditorError> {
        self.replay_history(false)
    }
    pub fn redo(&mut self) -> Result<Option<CommittedChange>, EditorError> {
        self.replay_history(true)
    }

    pub fn insert_text(
        &mut self,
        text: &str,
        origin: TransactionOrigin,
    ) -> Result<CommittedChange, EditorError> {
        let range = self.selection.range();
        let after = Selection::caret(ByteOffset(range.start.0 + text.len()));
        self.apply(
            Transaction::new(&self.snapshot)
                .replace(range, text)
                .with_origin(origin)
                .with_selection(after)
                .with_grapheme_selection(Affinity::After),
        )
    }

    pub fn delete_backward(&mut self) -> Result<Option<CommittedChange>, EditorError> {
        self.delete_grapheme(false)
    }
    pub fn delete_forward(&mut self) -> Result<Option<CommittedChange>, EditorError> {
        self.delete_grapheme(true)
    }

    /// Initial correctness implementation materializes the document so grapheme
    /// segmentation receives context across every rope chunk. Optimize after profiling.
    pub fn move_grapheme(&mut self, forward: bool, extend: bool) -> Result<(), EditorError> {
        let head = if !extend && !self.selection.is_collapsed() {
            if forward {
                self.selection.end()
            } else {
                self.selection.start()
            }
        } else {
            self.grapheme_neighbor(self.selection.head, forward)
        };
        self.set_selection(Selection {
            anchor: if extend { self.selection.anchor } else { head },
            head,
            affinity: self.selection.affinity,
        })
    }

    fn delete_grapheme(&mut self, forward: bool) -> Result<Option<CommittedChange>, EditorError> {
        // API selections permit scalar boundaries so they can represent source
        // positions. User deletion expands an interior position/partial selection
        // to whole extended graphemes instead of orphaning marks or emoji parts.
        let text = self.snapshot.text.to_string();
        let mut range = self.selection.range();
        if self.selection.is_collapsed() {
            for (start, grapheme) in text.grapheme_indices(true) {
                let end = start + grapheme.len();
                let inside = start < range.start.0 && range.start.0 < end;
                let neighbor = if forward {
                    start == range.start.0
                } else {
                    end == range.start.0
                };
                if inside || neighbor {
                    range = ByteOffset(start)..ByteOffset(end);
                    break;
                }
            }
        } else {
            for (start, grapheme) in text.grapheme_indices(true) {
                let end = start + grapheme.len();
                if start < range.start.0 && range.start.0 < end {
                    range.start = ByteOffset(start);
                }
                if start < range.end.0 && range.end.0 < end {
                    range.end = ByteOffset(end);
                }
            }
        }
        if range.is_empty() {
            return Ok(None);
        }
        let selection = Selection::caret(range.start);
        self.apply(
            Transaction::new(&self.snapshot)
                .replace(range, "")
                .with_selection(selection)
                .with_origin(TransactionOrigin::Delete),
        )
        .map(Some)
    }

    fn grapheme_neighbor(&self, offset: ByteOffset, forward: bool) -> ByteOffset {
        let text = self.snapshot.text.to_string();
        if forward {
            ByteOffset(
                text.grapheme_indices(true)
                    .map(|(index, _)| index)
                    .find(|&index| index > offset.0)
                    .unwrap_or(text.len()),
            )
        } else {
            ByteOffset(
                text.grapheme_indices(true)
                    .map(|(index, _)| index)
                    .take_while(|&index| index < offset.0)
                    .last()
                    .unwrap_or(0),
            )
        }
    }

    fn ensure_document(&self, id: DocumentId) -> Result<(), EditorError> {
        if id != self.snapshot.document_id {
            Err(EditorError::ForeignDocument {
                expected: self.snapshot.document_id,
                actual: id,
            })
        } else {
            Ok(())
        }
    }

    fn replay_history(&mut self, redo: bool) -> Result<Option<CommittedChange>, EditorError> {
        let entry = if redo {
            self.redo.last()
        } else {
            self.undo.back()
        };
        let Some(entry) = entry else {
            return Ok(None);
        };
        let before = self.snapshot.clone();
        let mut text = before.text.clone();
        if redo {
            for step in &entry.steps {
                text = apply_changes(&text, &step.forward)?.0;
            }
        } else {
            for step in entry.steps.iter().rev() {
                text = apply_changes(&text, &step.inverse)?.0;
            }
        }
        let selection_after = if redo {
            entry.selection_after
        } else {
            entry.selection_before
        };
        validate_selection(&text, selection_after)?;
        let revision = Revision(
            before
                .revision
                .0
                .checked_add(1)
                .ok_or(EditorError::RevisionExhausted)?,
        );
        // Grouped history may contain sequential coordinate spaces. Publish its
        // equivalent minimal byte-safe replacement in the observer's base space.
        let changes = net_changes(&before.text, &text);
        let change_map = ChangeMap {
            before_bytes: before.byte_len(),
            changes: changes.clone(),
        };
        let state = if redo {
            entry.content_after
        } else {
            entry.content_before
        };
        let selection_before = self.selection;
        let entry = if redo {
            self.redo.pop().expect("checked redo entry")
        } else {
            self.undo.pop_back().expect("checked undo entry")
        };
        if redo {
            self.undo.push_back(entry);
        } else {
            self.redo.push(entry);
        }
        self.break_history_group();
        self.snapshot = Snapshot {
            document_id: before.document_id,
            revision,
            text,
            content_state: state,
        };
        self.selection = selection_after;
        let event = CommittedChange {
            document_id: before.document_id,
            before_revision: before.revision,
            revision,
            selection_before,
            selection_after,
            origin: if redo {
                TransactionOrigin::Redo
            } else {
                TransactionOrigin::Undo
            },
            changes,
            change_map,
            snapshot: self.snapshot.clone(),
        };
        self.notify(&event);
        Ok(Some(event))
    }

    fn enforce_history_limit(&mut self) {
        while self.history_bytes > self.history_limit {
            if let Some(entry) = self.undo.pop_front() {
                self.history_bytes -= entry.bytes;
            } else if let Some(entry) = self.redo.first() {
                let bytes = entry.bytes;
                self.redo.remove(0);
                self.history_bytes -= bytes;
            } else {
                self.history_bytes = 0;
                break;
            }
        }
    }

    fn notify(&mut self, event: &CommittedChange) {
        self.subscribers
            .retain(|subscriber| match subscriber.try_send(event.clone()) {
                Ok(()) => true,
                Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) => false,
            });
    }
}

fn snap_grapheme_in_rope(
    text: &Rope,
    offset: ByteOffset,
    affinity: Affinity,
) -> Result<ByteOffset, EditorError> {
    valid_offset(text, offset)?;
    let line = text.byte_to_line(offset.0);
    let start = text.line_to_byte(line);
    let value = text.line(line).to_string();
    let local = offset.0 - start;
    let mut boundaries = value
        .grapheme_indices(true)
        .map(|(index, _)| index)
        .chain([value.len()]);
    let snapped = match affinity {
        Affinity::After => boundaries
            .find(|index| *index >= local)
            .unwrap_or(value.len()),
        Affinity::Before => boundaries
            .take_while(|index| *index <= local)
            .last()
            .unwrap_or(0),
    };
    Ok(ByteOffset(start + snapped))
}

fn valid_offset(text: &Rope, offset: ByteOffset) -> Result<(), EditorError> {
    if offset.0 > text.len_bytes() {
        return Err(EditorError::InvalidOffset(offset));
    }
    let scalar = text.byte_to_char(offset.0);
    if text.char_to_byte(scalar) != offset.0 {
        return Err(EditorError::InvalidOffset(offset));
    }
    Ok(())
}

fn validate_range(text: &Rope, range: &Range<ByteOffset>) -> Result<(), EditorError> {
    if range.start > range.end {
        return Err(EditorError::InvalidRange(range.clone()));
    }
    valid_offset(text, range.start)?;
    valid_offset(text, range.end)
}

fn validate_selection(text: &Rope, selection: Selection) -> Result<(), EditorError> {
    valid_offset(text, selection.anchor)?;
    valid_offset(text, selection.head)
}

fn normalize_changes(text: &Rope, changes: &mut [TextChange]) -> Result<(), EditorError> {
    if changes.is_empty() {
        return Err(EditorError::EmptyTransaction);
    }
    for change in changes.iter() {
        validate_range(text, &change.range)?;
    }
    changes.sort_by_key(|change| (change.range.start, change.range.end));
    for pair in changes.windows(2) {
        if pair[1].range.start < pair[0].range.end || pair[1].range.start == pair[0].range.start {
            return Err(EditorError::OverlappingChanges);
        }
    }
    Ok(())
}

fn apply_changes(
    text: &Rope,
    changes: &[TextChange],
) -> Result<(Rope, Vec<TextChange>), EditorError> {
    let mut output = text.clone();
    let mut inverse = Vec::with_capacity(changes.len());
    let mut delta = 0_i128;
    for change in changes {
        validate_range(text, &change.range)?;
        let deleted = text
            .byte_slice(change.range.start.0..change.range.end.0)
            .to_string();
        let inverse_start = (change.range.start.0 as i128 + delta) as usize;
        inverse.push(TextChange::new(
            ByteOffset(inverse_start)..ByteOffset(inverse_start + change.inserted.len()),
            deleted,
        ));
        delta +=
            change.inserted.len() as i128 - (change.range.end.0 - change.range.start.0) as i128;
    }
    for change in changes.iter().rev() {
        let start = output.byte_to_char(change.range.start.0);
        let end = output.byte_to_char(change.range.end.0);
        output.remove(start..end);
        output.insert(start, &change.inserted);
    }
    Ok((output, inverse))
}

fn history_size(forward: &[TextChange], inverse: &[TextChange]) -> usize {
    forward
        .iter()
        .chain(inverse)
        .fold(std::mem::size_of::<HistoryEntry>(), |total, change| {
            total
                .saturating_add(std::mem::size_of::<TextChange>())
                .saturating_add(change.inserted.capacity())
        })
}

fn typing_end(
    changes: &[TextChange],
    before: Selection,
    after: Selection,
    origin: TransactionOrigin,
) -> Option<ByteOffset> {
    if origin != TransactionOrigin::Typing
        || !before.is_collapsed()
        || !after.is_collapsed()
        || changes.len() != 1
    {
        return None;
    }
    let change = &changes[0];
    if !change.range.is_empty()
        || change.range.start != before.head
        || change.inserted.contains(['\n', '\r'])
        || change.inserted.is_empty()
    {
        return None;
    }
    let end = ByteOffset(change.range.start.0 + change.inserted.len());
    (after.head == end).then_some(end)
}

fn net_changes(before: &Rope, after: &Rope) -> Vec<TextChange> {
    let before = before.to_string();
    let after = after.to_string();
    let mut start = before
        .bytes()
        .zip(after.bytes())
        .take_while(|(a, b)| a == b)
        .count();
    while !before.is_char_boundary(start) || !after.is_char_boundary(start) {
        start -= 1;
    }
    let max_suffix = before.len().min(after.len()) - start;
    let mut suffix = before
        .bytes()
        .rev()
        .zip(after.bytes().rev())
        .take(max_suffix)
        .take_while(|(a, b)| a == b)
        .count();
    while !before.is_char_boundary(before.len() - suffix)
        || !after.is_char_boundary(after.len() - suffix)
    {
        suffix -= 1;
    }
    vec![TextChange::new(
        ByteOffset(start)..ByteOffset(before.len() - suffix),
        &after[start..after.len() - suffix],
    )]
}
