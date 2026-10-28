//! Original composition ownership and provisional text state.
//!
//! Native input APIs do not all carry a composition/session identifier. In
//! particular, egui-winit 0.37.0 does not forward winit's Enabled/Disabled events.
//! The caller must bind each event stream to an explicit [`InputEpoch`]. An epoch
//! is issued at an owned, trusted input boundary, never merely because a Commit
//! arrived. Cancellation invalidates it. A deliberate fresh-input action may
//! start a new epoch, allowing commit-only input (for example emoji) without
//! accepting an old cancelled stream's token.
//!
//! This module cannot infer the provenance of untagged operating-system events.
//! The native adapter must drain/discard cancelled batches and qualify its epoch
//! handoff against the real backend. Token tests alone are not real-IME evidence.

use std::{fmt, ops::Range};

use glyphfolio_editor_core::{
    ByteOffset, CommittedChange, Editor, EditorError, PresentationMode, Selection, Snapshot,
    Transaction, TransactionOrigin,
};

/// Opaque identity of one deliberately established native input owner.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InputEpoch(u64);

impl InputEpoch {
    /// Diagnostic identity only. Callers cannot construct valid epochs from it.
    pub fn diagnostic_id(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Debug)]
struct Owner {
    epoch: InputEpoch,
    snapshot: Snapshot,
    selection: Selection,
    replacement: Range<ByteOffset>,
}

#[derive(Clone, Debug)]
struct Preedit {
    text: String,
    active_range_chars: Option<Range<usize>>,
}

/// Display text is a disposable projection; all offsets here are display bytes.
/// It must never be persisted or passed to a source transaction as a source map.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompositionDisplay {
    pub text: String,
    pub caret_byte: usize,
    pub preedit_range: Option<Range<usize>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InputError {
    InactiveEpoch,
    StaleEpoch,
    OwnershipChanged,
    InvalidPreeditRange,
    EpochExhausted,
    Editor(EditorError),
}

impl fmt::Display for InputError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InactiveEpoch => formatter.write_str("No native input owner is active"),
            Self::StaleEpoch => {
                formatter.write_str("Ignored an event from a stale native input epoch")
            }
            Self::OwnershipChanged => formatter
                .write_str("Native input ownership changed document, revision, or selection"),
            Self::InvalidPreeditRange => {
                formatter.write_str("Native preedit selection is not a valid scalar range")
            }
            Self::EpochExhausted => formatter.write_str("Native input epoch counter exhausted"),
            Self::Editor(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for InputError {}

impl From<EditorError> for InputError {
    fn from(error: EditorError) -> Self {
        Self::Editor(error)
    }
}

/// Host-owned, UI-independent controller for one original native editing surface.
///
/// Empty preedit dismisses the visible overlay but retains composition ownership,
/// the replacement range, and pending mode/close barriers until Commit/interrupt.
#[derive(Clone, Debug, Default)]
pub struct CompositionSession {
    next_epoch: u64,
    owner: Option<Owner>,
    preedit: Option<Preedit>,
    pending_composition: bool,
    presentation: PresentationMode,
    pending_presentation: Option<PresentationMode>,
    expected_duplicate: Option<String>,
}

impl CompositionSession {
    /// Begin at an explicit fresh native input boundary after cancelling/draining
    /// the previous backend stream. Do not call this to bless an arbitrary Commit.
    pub fn begin_input(&mut self, editor: &Editor) -> Result<InputEpoch, InputError> {
        let next = self
            .next_epoch
            .checked_add(1)
            .ok_or(InputError::EpochExhausted)?;
        self.interrupt();
        self.next_epoch = next;
        let epoch = InputEpoch(next);
        let selection = editor.selection();
        self.owner = Some(Owner {
            epoch,
            snapshot: editor.snapshot(),
            selection,
            replacement: selection.range(),
        });
        Ok(epoch)
    }

    pub fn epoch(&self) -> Option<InputEpoch> {
        self.owner.as_ref().map(|owner| owner.epoch)
    }

    /// Cancel provisional state without mutating committed source or history.
    pub fn interrupt(&mut self) {
        self.owner = None;
        self.preedit = None;
        self.pending_composition = false;
        self.expected_duplicate = None;
        self.apply_pending_presentation();
    }

    /// Detect host edits/document changes before rendering or routing native events.
    /// A stale stream is invalidated; it is never silently attached to new source.
    pub fn synchronize(&mut self, editor: &Editor) -> Result<(), InputError> {
        if self
            .owner
            .as_ref()
            .is_some_and(|owner| !owner_matches(owner, editor))
        {
            self.interrupt();
            return Err(InputError::OwnershipChanged);
        }
        Ok(())
    }

    pub fn is_composing(&self) -> bool {
        self.pending_composition
    }

    pub fn can_close(&self) -> bool {
        !self.pending_composition
    }

    pub fn presentation(&self) -> PresentationMode {
        self.presentation
    }

    pub fn request_presentation(&mut self, mode: PresentationMode) {
        if self.pending_composition {
            self.pending_presentation = Some(mode);
        } else {
            self.presentation = mode;
        }
    }

    /// Update preedit. `active_range_chars` is measured in Unicode scalars, not
    /// UTF-8 bytes, UTF-16 units, or graphemes, matching the pinned toolkit event.
    pub fn preedit(
        &mut self,
        editor: &Editor,
        epoch: InputEpoch,
        text: &str,
        active_range_chars: Option<Range<usize>>,
    ) -> Result<(), InputError> {
        self.checked_owner(editor, epoch)?;
        let count = text.chars().count();
        if active_range_chars
            .as_ref()
            .is_some_and(|range| range.start > range.end || range.end > count)
        {
            return Err(InputError::InvalidPreeditRange);
        }
        self.pending_composition = true;
        self.expected_duplicate = None;
        self.preedit = (!text.is_empty()).then(|| Preedit {
            text: text.to_owned(),
            active_range_chars,
        });
        Ok(())
    }

    /// Commit through the document transaction engine. Commit-only input is valid
    /// for an active epoch even without preedit. Interrupted/stale tokens fail.
    /// An empty Commit cancels provisional source changes.
    pub fn commit(
        &mut self,
        editor: &mut Editor,
        epoch: InputEpoch,
        text: &str,
    ) -> Result<Option<CommittedChange>, InputError> {
        self.commit_prepared(editor, epoch, text, text, text.len())
    }

    pub(crate) fn commit_prepared(
        &mut self,
        editor: &mut Editor,
        epoch: InputEpoch,
        text: &str,
        replacement: &str,
        caret: usize,
    ) -> Result<Option<CommittedChange>, InputError> {
        if caret > replacement.len() || !replacement.is_char_boundary(caret) {
            return Err(InputError::InvalidPreeditRange);
        }
        let owner = self.checked_owner(editor, epoch)?;
        let result = if text.is_empty() {
            Ok(None)
        } else {
            let end = ByteOffset(owner.replacement.start.0 + caret);
            let selection = Selection::caret(end);
            let transaction = Transaction::new(&owner.snapshot)
                .replace(owner.replacement.clone(), replacement)
                .with_selection(selection)
                .with_grapheme_selection(glyphfolio_editor_core::Affinity::After)
                .with_origin(TransactionOrigin::Composition);
            match editor.apply(transaction) {
                Ok(change) => Ok(Some(change)),
                // Replacing selected text by identical text still collapses the
                // selection but does not create a content revision/history step.
                Err(EditorError::EmptyTransaction) => {
                    editor.set_selection(Selection::caret(
                        editor
                            .snapshot()
                            .snap_grapheme(end, glyphfolio_editor_core::Affinity::After)?,
                    ))?;
                    Ok(None)
                }
                Err(error) => Err(InputError::Editor(error)),
            }
        };
        if result.is_err() {
            self.interrupt();
            return result;
        }
        self.preedit = None;
        self.pending_composition = false;
        self.expected_duplicate = (!text.is_empty()).then(|| text.to_owned());
        self.apply_pending_presentation();
        self.refresh_owner(editor, epoch);
        result
    }

    /// Apply platform DeleteSurrounding counts to committed source scalar units.
    /// During a pending composition the source range is staged until nonempty
    /// Commit, so cancellation persists neither provisional text nor deletion.
    /// Outside composition this is its own atomic native edit/history step.
    pub fn delete_surrounding(
        &mut self,
        editor: &mut Editor,
        epoch: InputEpoch,
        before_chars: usize,
        after_chars: usize,
    ) -> Result<Option<CommittedChange>, InputError> {
        let owner = self.checked_owner(editor, epoch)?;
        let text = owner.snapshot.text();
        let at = text[..owner.selection.head.0].chars().count();
        let range = ByteOffset(scalar_byte(&text, at.saturating_sub(before_chars)))
            ..ByteOffset(scalar_byte(&text, at.saturating_add(after_chars)));
        if range.is_empty() {
            return Ok(None);
        }
        if self.pending_composition {
            let replacement = ByteOffset(owner.replacement.start.0.min(range.start.0))
                ..ByteOffset(owner.replacement.end.0.max(range.end.0));
            if let Some(owner) = &mut self.owner {
                owner.replacement = replacement;
            }
            return Ok(None);
        }
        let transaction = Transaction::new(&owner.snapshot)
            .replace(range.clone(), "")
            .with_selection(Selection::caret(range.start))
            .with_origin(TransactionOrigin::Composition);
        let change = editor.apply(transaction)?;
        self.expected_duplicate = None;
        self.refresh_owner(editor, epoch);
        Ok(Some(change))
    }

    /// Suppress at most one backend Text duplicate immediately after Commit.
    /// The router must call [`Self::clear_duplicate`] when a different native
    /// event intervenes; this is not a time-based text deduplicator.
    pub fn should_suppress_duplicate_text(&mut self, text: &str) -> bool {
        self.expected_duplicate.take().as_deref() == Some(text)
    }

    pub fn clear_duplicate(&mut self) {
        self.expected_duplicate = None;
    }

    /// Produce disposable text/caret geometry input, never authoritative Markdown.
    pub fn display(
        &self,
        snapshot: &Snapshot,
        selection: Selection,
    ) -> Result<CompositionDisplay, InputError> {
        let mut text = snapshot.text();
        if let Some(owner) = &self.owner {
            if owner.snapshot.document_id() != snapshot.document_id()
                || owner.snapshot.revision() != snapshot.revision()
                || owner.selection != selection
            {
                return Err(InputError::OwnershipChanged);
            }
            if let Some(preedit) = &self.preedit {
                let start = owner.replacement.start.0;
                text.replace_range(start..owner.replacement.end.0, &preedit.text);
                let scalar = preedit
                    .active_range_chars
                    .as_ref()
                    .map_or(preedit.text.chars().count(), |range| range.end);
                return Ok(CompositionDisplay {
                    text,
                    caret_byte: start + scalar_byte(&preedit.text, scalar),
                    preedit_range: Some(start..start + preedit.text.len()),
                });
            }
        }
        Ok(CompositionDisplay {
            text,
            caret_byte: selection.head.0,
            preedit_range: None,
        })
    }

    fn checked_owner(&mut self, editor: &Editor, epoch: InputEpoch) -> Result<Owner, InputError> {
        let Some(owner) = &self.owner else {
            return Err(InputError::InactiveEpoch);
        };
        if owner.epoch != epoch {
            return Err(InputError::StaleEpoch);
        }
        if !owner_matches(owner, editor) {
            self.interrupt();
            return Err(InputError::OwnershipChanged);
        }
        Ok(owner.clone())
    }

    fn refresh_owner(&mut self, editor: &Editor, epoch: InputEpoch) {
        let selection = editor.selection();
        self.owner = Some(Owner {
            epoch,
            snapshot: editor.snapshot(),
            selection,
            replacement: selection.range(),
        });
    }

    fn apply_pending_presentation(&mut self) {
        if let Some(mode) = self.pending_presentation.take() {
            self.presentation = mode;
        }
    }
}

fn owner_matches(owner: &Owner, editor: &Editor) -> bool {
    let snapshot = editor.snapshot();
    owner.snapshot.document_id() == snapshot.document_id()
        && owner.snapshot.revision() == snapshot.revision()
        && owner.selection == editor.selection()
}

fn scalar_byte(text: &str, scalar: usize) -> usize {
    text.char_indices()
        .nth(scalar)
        .map_or(text.len(), |(byte, _)| byte)
}
