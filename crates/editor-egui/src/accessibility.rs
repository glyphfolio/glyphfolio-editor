//! Original semantic bridge from fully shaped text to editable AccessKit nodes.
//!
//! This module owns logical-line/run identity and coordinate conversion. It does
//! not use another editor's accessibility controller. Every shaped logical line
//! is retained, including offscreen lines; painting/scrolling belongs to the host.
//! Source uses canonical text coordinates. For Rich, the host maps these display
//! byte positions through its validated projection to the canonical document.

use std::{collections::HashMap, ops::Range, sync::Arc};

use accesskit::{Action, Node, NodeId, Role, TextDirection, TextPosition, TextSelection};
use egui::{Id, Pos2, Rect, pos2, vec2};
use glyphfolio_editor_core::{Affinity, ByteOffset, DocumentId, Selection};
use glyphfolio_editor_layout::{CaretStop, GlyphCluster, ShapedDocument, ShapedRow};
use glyphfolio_editor_markdown::{BlockKind, Projection};
use unicode_segmentation::UnicodeSegmentation;

const MAX_RUN_UNITS: usize = 255;

#[derive(Clone, Debug)]
struct RunIdentity {
    serial: u64,
    range: Range<usize>,
    text: String,
    rtl: bool,
}

#[derive(Clone, Debug)]
struct LineIdentity {
    serial: u64,
    text: String,
    runs: Vec<RunIdentity>,
}

#[derive(Clone, Debug)]
struct Binding {
    id: Id,
    source: Range<usize>,
    boundaries: Vec<usize>,
}

#[derive(Clone, Debug)]
struct Unit {
    source: Range<usize>,
    row: usize,
    rtl: bool,
    rect: Rect,
    fallback: bool,
}

#[derive(Clone, Debug)]
struct Run {
    source: Range<usize>,
    rtl: bool,
    units: Vec<Unit>,
    rect: Rect,
}

/// Cumulative semantic work for one native surface. Cache hits do not rebuild
/// source bindings, paragraph identity, word boundaries, or glyph geometry.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AccessibilityCacheStats {
    pub semantic_rebuilds: u64,
    pub cache_hits: u64,
    pub selection_updates: u64,
    pub cached_nodes: usize,
}

/// Counted lookup work, independent of native frame rate or allocator/RSS claims.
/// Index bytes are the last temporary vectors' payload capacities, excluding
/// allocator overhead; glyph/text/node storage is not counted here.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AccessibilityRowStats {
    pub index_builds: u64,
    pub indexed_rows: usize,
    pub index_payload_bytes: usize,
    pub lookups: u64,
    pub binary_probes: u64,
    pub candidates_visited: u64,
}

#[derive(Clone, Copy)]
struct RowEntry {
    row: usize,
    start: usize,
    end: usize,
    prefix_end: usize,
    prefix_end_row: usize,
}

/// Per-build interval index. Only overlapping candidates within the requested
/// logical line are examined; ordinary disjoint rows need one candidate.
struct RowLookup {
    entries: Vec<RowEntry>,
    lines: Vec<Range<usize>>,
    stats: AccessibilityRowStats,
}

impl RowLookup {
    fn new(document: &ShapedDocument, line_count: usize) -> Self {
        let mut entries: Vec<_> = document
            .rows
            .iter()
            .enumerate()
            .map(|(row, value)| RowEntry {
                row,
                start: value.source_range.start.0,
                end: value.source_range.end.0,
                prefix_end: 0,
                prefix_end_row: row,
            })
            .collect();
        entries.sort_unstable_by_key(|entry| {
            (
                document.rows[entry.row].logical_line,
                entry.start,
                entry.end,
                entry.row,
            )
        });
        let mut lines = Vec::with_capacity(line_count);
        let mut cursor = 0;
        for line in 0..line_count {
            let start = cursor;
            let mut furthest = 0;
            let mut furthest_row = 0;
            while cursor < entries.len() && document.rows[entries[cursor].row].logical_line == line
            {
                let entry = &mut entries[cursor];
                if cursor == start
                    || entry.end > furthest
                    || (entry.end == furthest && entry.row < furthest_row)
                {
                    furthest = entry.end;
                    furthest_row = entry.row;
                }
                entry.prefix_end = furthest;
                entry.prefix_end_row = furthest_row;
                cursor += 1;
            }
            lines.push(start..cursor);
        }
        debug_assert_eq!(
            cursor,
            entries.len(),
            "shaped rows must belong to a source line"
        );
        let stats = AccessibilityRowStats {
            index_builds: 1,
            indexed_rows: entries.len(),
            index_payload_bytes: entries.capacity() * std::mem::size_of::<RowEntry>()
                + lines.capacity() * std::mem::size_of::<Range<usize>>(),
            ..Default::default()
        };
        Self {
            entries,
            lines,
            stats,
        }
    }

    fn row<'a>(
        &mut self,
        document: &'a ShapedDocument,
        line: usize,
        byte: usize,
    ) -> (usize, &'a ShapedRow) {
        let entries = &self.entries[self.lines[line].clone()];
        assert!(
            !entries.is_empty(),
            "shaping includes every logical source line"
        );
        self.stats.lookups = self.stats.lookups.saturating_add(1);
        let upper = entries.partition_point(|entry| {
            self.stats.binary_probes = self.stats.binary_probes.saturating_add(1);
            entry.start <= byte
        });
        let lower = entries[..upper].partition_point(|entry| {
            self.stats.binary_probes = self.stats.binary_probes.saturating_add(1);
            entry.prefix_end <= byte
        });
        let mut chosen = None;
        for entry in &entries[lower..upper] {
            self.stats.candidates_visited = self.stats.candidates_visited.saturating_add(1);
            if byte < entry.end && chosen.is_none_or(|row: usize| entry.row < row) {
                chosen = Some(entry.row);
            }
        }
        let chosen = chosen.unwrap_or_else(|| {
            // A line ending or empty line may have no glyph range. Pick the
            // nearest actual source row, then use that row's exact caret below.
            let before = upper
                .checked_sub(1)
                .map(|index| entries[index].prefix_end_row);
            let after = entries.get(upper).map(|entry| entry.row);
            before
                .into_iter()
                .chain(after)
                .min_by_key(|&index| {
                    self.stats.candidates_visited = self.stats.candidates_visited.saturating_add(1);
                    let range = &document.rows[index].source_range;
                    let distance = if byte < range.start.0 {
                        range.start.0 - byte
                    } else {
                        byte.saturating_sub(range.end.0)
                    };
                    (distance, index)
                })
                .unwrap_or(entries[0].row)
        });
        (chosen, &document.rows[chosen])
    }
}

struct SharedSemanticTree {
    shaped: Arc<ShapedDocument>,
    id: Id,
    document_id: DocumentId,
    root_rect: Rect,
    origin: Pos2,
    selection: Selection,
    nodes: Vec<(Id, Node)>,
}

impl SharedSemanticTree {
    fn same_placement(
        &self,
        id: Id,
        root_rect: Rect,
        origin: Pos2,
        document_id: DocumentId,
    ) -> bool {
        self.id == id
            && self.document_id == document_id
            && self.root_rect == root_rect
            && self.origin == origin
    }
}

/// State belonging to one semantic surface. Removed node IDs are never recycled.
///
/// AccessKit action requests carry no document revision. IDs that survive a text
/// update therefore name their current logical run; removed IDs are rejected.
/// The caller must additionally reject actions while composing or changing host
/// sessions and apply decoded selections against its current document snapshot.
#[derive(Default)]
pub struct AccessibilityState {
    document: Option<DocumentId>,
    root: Option<Id>,
    text: String,
    lines: Vec<LineIdentity>,
    next_serial: u64,
    bindings: Vec<Binding>,
    bounds: HashMap<NodeId, Rect>,
    legal_boundaries: Vec<usize>,
    scalar_fallback_runs: usize,
    shared: Option<SharedSemanticTree>,
    cache_stats: AccessibilityCacheStats,
    row_stats: AccessibilityRowStats,
}

impl AccessibilityState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cache_stats(&self) -> AccessibilityCacheStats {
        AccessibilityCacheStats {
            cached_nodes: self.shared.as_ref().map_or(0, |cache| cache.nodes.len()),
            ..self.cache_stats
        }
    }

    pub fn row_lookup_stats(&self) -> AccessibilityRowStats {
        self.row_stats
    }

    /// Add Rich presentation semantics to a cloned emission tree. The editable
    /// root, text runs, IDs, selection bindings and cached semantic tree remain
    /// intact; only matching logical paragraphs gain heading role/level.
    ///
    /// AccessKit levels are zero-based: Markdown H1 is level 0 and H6 level 5.
    /// This returns false without mutation for a foreign document/tree or a
    /// display mismatch, including a disposable IME overlay. Callers can still
    /// give an overlay root a Rich label, but must not guess block coordinates.
    pub fn decorate_rich_nodes(&self, nodes: &mut [(Id, Node)], projection: &Projection) -> bool {
        let Some(((root_id, root), descendants)) = nodes.split_first_mut() else {
            return false;
        };
        if self.document != Some(projection.document_id())
            || self.root != Some(*root_id)
            || root.role() != Role::MultilineTextInput
            || root.value() != Some(projection.text())
            || self.text != projection.text()
        {
            return false;
        }
        root.set_label("Rich Markdown editor");
        let mut lines = self.lines.iter();
        let mut current = lines.next();
        let mut line_start = 0;
        let mut block_index = 0;
        let blocks = projection.blocks();
        // Emission order is root, then each logical paragraph and its runs.
        // Walk that order once; no map, reparse, reshaping or cache rebuild.
        for (node_id, node) in descendants {
            let Some(line) = current else {
                break;
            };
            if *node_id != root_id.with(("logical-source-line", line.serial)) {
                continue;
            }
            while block_index < blocks.len()
                && blocks[block_index].display_range.end.0 <= line_start
            {
                block_index += 1;
            }
            node.set_role(Role::Paragraph);
            node.clear_level();
            if let Some(block) = blocks.get(block_index)
                && block.display_range.start.0 <= line_start
                && let BlockKind::Heading(level @ 1..=6) = block.kind
            {
                node.set_role(Role::Heading);
                node.set_level(usize::from(level - 1));
            }
            line_start += line.text.len();
            current = lines.next();
        }
        true
    }

    /// Prepare current source/action bindings without cloning a return tree.
    /// Retaining the immutable Arc prevents an address from being recycled into
    /// a false cache hit; `Arc::make_mut` necessarily creates different geometry.
    pub fn prepare_shared(
        &mut self,
        id: Id,
        root_rect: Rect,
        origin: Pos2,
        document_id: DocumentId,
        shaped: &Arc<ShapedDocument>,
        selection: Selection,
    ) {
        let hit = self.shared.as_ref().is_some_and(|cache| {
            cache.same_placement(id, root_rect, origin, document_id)
                && Arc::ptr_eq(&cache.shaped, shaped)
        });
        if hit {
            self.shared_hit(selection);
            return;
        }
        self.shared = None;
        self.cache_stats.semantic_rebuilds = self.cache_stats.semantic_rebuilds.saturating_add(1);
        let nodes = self.rebuild_nodes(id, root_rect, origin, document_id, shaped, selection);
        self.shared = Some(SharedSemanticTree {
            shaped: Arc::clone(shaped),
            id,
            document_id,
            root_rect,
            origin,
            selection,
            nodes,
        });
    }

    /// Return the current whole semantic tree for emission. Stable geometry
    /// reuses all nodes/bindings and updates only the editable root selection.
    pub fn build_shared(
        &mut self,
        id: Id,
        root_rect: Rect,
        origin: Pos2,
        document_id: DocumentId,
        shaped: &Arc<ShapedDocument>,
        selection: Selection,
    ) -> Vec<(Id, Node)> {
        self.prepare_shared(id, root_rect, origin, document_id, shaped, selection);
        self.shared
            .as_ref()
            .expect("prepare installs the shared tree")
            .nodes
            .clone()
    }

    fn shared_hit(&mut self, selection: Selection) {
        self.cache_stats.cache_hits = self.cache_stats.cache_hits.saturating_add(1);
        if self
            .shared
            .as_ref()
            .is_some_and(|cache| cache.selection == selection)
        {
            return;
        }
        let mapped = self
            .position(selection.anchor)
            .zip(self.position(selection.head));
        let cache = self.shared.as_mut().expect("cache hit has a tree");
        if let Some((anchor, focus)) = mapped {
            cache.nodes[0]
                .1
                .set_text_selection(TextSelection { anchor, focus });
        } else {
            cache.nodes[0].1.clear_text_selection();
        }
        cache.selection = selection;
        self.cache_stats.selection_updates = self.cache_stats.selection_updates.saturating_add(1);
    }

    /// Build a whole-document semantic tree in logical source order.
    ///
    /// Directional runs are split at soft wraps and bidi-direction changes.
    /// Their screen geometry may be nonmonotonic in source order. The root is an
    /// editable text input, containing Paragraph nodes, containing TextRun nodes.
    pub fn build(
        &mut self,
        id: Id,
        root_rect: Rect,
        origin: Pos2,
        document_id: DocumentId,
        shaped: &ShapedDocument,
        selection: Selection,
    ) -> Vec<(Id, Node)> {
        let hit = self.shared.as_ref().is_some_and(|cache| {
            cache.same_placement(id, root_rect, origin, document_id)
                && std::ptr::eq(cache.shaped.as_ref(), shaped)
        });
        if hit {
            self.shared_hit(selection);
            return self
                .shared
                .as_ref()
                .expect("cache hit has a tree")
                .nodes
                .clone();
        }
        self.shared = None;
        self.cache_stats.semantic_rebuilds = self.cache_stats.semantic_rebuilds.saturating_add(1);
        self.rebuild_nodes(id, root_rect, origin, document_id, shaped, selection)
    }

    fn rebuild_nodes(
        &mut self,
        id: Id,
        root_rect: Rect,
        origin: Pos2,
        document_id: DocumentId,
        shaped: &ShapedDocument,
        selection: Selection,
    ) -> Vec<(Id, Node)> {
        let same_document = self.document == Some(document_id) && self.root == Some(id);
        let old_lines = if same_document {
            std::mem::take(&mut self.lines)
        } else {
            self.lines.clear();
            Vec::new()
        };
        self.document = Some(document_id);
        self.root = Some(id);
        self.bindings.clear();
        self.bounds.clear();
        self.scalar_fallback_runs = 0;
        self.legal_boundaries = shaped
            .text
            .grapheme_indices(true)
            .map(|(byte, _)| byte)
            .chain([shaped.text.len()])
            .collect();

        let source_lines = logical_lines(&shaped.text);
        let mut row_lookup = RowLookup::new(shaped, source_lines.len());
        let old_indices = reconcile_lines(&old_lines, &source_lines, &shaped.text);
        let mut paragraph_nodes = Vec::new();
        let mut children = Vec::new();

        for (line_index, source) in source_lines.iter().enumerate() {
            let old = old_indices[line_index].map(|index| &old_lines[index]);
            let line_serial = old.map_or_else(|| self.allocate_serial(), |line| line.serial);
            let paragraph_id = id.with(("logical-source-line", line_serial));
            let line_text = &shaped.text[source.clone()];
            // Word boundaries belong to the whole logical paragraph, not each
            // wrapped/directional run. Continuations must not invent word starts.
            let word_starts: Vec<_> = line_text
                .split_word_bound_indices()
                .filter(|(_, segment)| !segment.chars().all(char::is_whitespace))
                .map(|(offset, _)| source.start + offset)
                .filter(|offset| self.legal_boundaries.binary_search(offset).is_ok())
                .collect();
            let runs = shape_runs(shaped, line_index, source.clone(), &mut row_lookup);
            let mut previous_runs_used = vec![false; old.map_or(0, |line| line.runs.len())];
            let mut run_identities = Vec::new();
            let mut run_nodes = Vec::new();
            let mut run_children = Vec::new();
            let mut paragraph_rect = Rect::NOTHING;

            for run in runs {
                let relative = (run.source.start - source.start)..(run.source.end - source.start);
                let value = &shaped.text[run.source.clone()];
                let serial = old
                    .and_then(|line| {
                        match_run(line, &relative, value, run.rtl, &previous_runs_used)
                    })
                    .map(|index| {
                        previous_runs_used[index] = true;
                        old.expect("matched an existing line").runs[index].serial
                    })
                    .unwrap_or_else(|| self.allocate_serial());
                let run_id = id.with(("logical-source-run", serial));
                let screen_rect = run.rect.translate(origin.to_vec2());
                paragraph_rect = paragraph_rect.union(screen_rect);
                let mut node = Node::new(Role::TextRun);
                node.set_value(value);
                node.set_bounds(access_rect(screen_rect));
                node.set_text_direction(if run.rtl {
                    TextDirection::RightToLeft
                } else {
                    TextDirection::LeftToRight
                });
                node.add_action(Action::ScrollIntoView);
                let lengths: Vec<_> = run
                    .units
                    .iter()
                    .map(|unit| {
                        u8::try_from(unit.source.len())
                            .expect("transport units are bounded before node creation")
                    })
                    .collect();
                node.set_character_lengths(lengths);
                node.set_word_starts(
                    run.units
                        .iter()
                        .enumerate()
                        .filter(|(_, unit)| word_starts.binary_search(&unit.source.start).is_ok())
                        .map(|(index, _)| {
                            u8::try_from(index).expect("text runs have at most 255 units")
                        })
                        .collect::<Vec<_>>(),
                );
                node.set_character_positions(
                    run.units
                        .iter()
                        .map(|unit| {
                            if run.rtl {
                                (run.rect.right() - unit.rect.right()).max(0.0)
                            } else {
                                (unit.rect.left() - run.rect.left()).max(0.0)
                            }
                        })
                        .collect::<Vec<_>>(),
                );
                node.set_character_widths(
                    run.units
                        .iter()
                        .map(|unit| unit.rect.width())
                        .collect::<Vec<_>>(),
                );
                if run.units.iter().any(|unit| unit.fallback) {
                    self.scalar_fallback_runs += 1;
                }
                self.bindings.push(Binding {
                    id: run_id,
                    source: run.source.clone(),
                    boundaries: run
                        .units
                        .iter()
                        .map(|unit| unit.source.start)
                        .chain([run.source.end])
                        .collect(),
                });
                self.bounds.insert(run_id.accesskit_id(), screen_rect);
                run_children.push(run_id.accesskit_id());
                run_identities.push(RunIdentity {
                    serial,
                    range: relative,
                    text: value.to_owned(),
                    rtl: run.rtl,
                });
                run_nodes.push((run_id, node));
            }

            let mut paragraph = Node::new(Role::Paragraph);
            paragraph.set_children(run_children);
            paragraph.set_bounds(access_rect(paragraph_rect));
            paragraph.add_action(Action::ScrollIntoView);
            self.bounds
                .insert(paragraph_id.accesskit_id(), paragraph_rect);
            children.push(paragraph_id.accesskit_id());
            self.lines.push(LineIdentity {
                serial: line_serial,
                text: line_text.to_owned(),
                runs: run_identities,
            });
            paragraph_nodes.push((paragraph_id, paragraph));
            paragraph_nodes.extend(run_nodes);
        }

        self.text.clone_from(&shaped.text);
        self.row_stats.index_builds = self
            .row_stats
            .index_builds
            .saturating_add(row_lookup.stats.index_builds);
        self.row_stats.indexed_rows = row_lookup.stats.indexed_rows;
        self.row_stats.index_payload_bytes = row_lookup.stats.index_payload_bytes;
        self.row_stats.lookups = self
            .row_stats
            .lookups
            .saturating_add(row_lookup.stats.lookups);
        self.row_stats.binary_probes = self
            .row_stats
            .binary_probes
            .saturating_add(row_lookup.stats.binary_probes);
        self.row_stats.candidates_visited = self
            .row_stats
            .candidates_visited
            .saturating_add(row_lookup.stats.candidates_visited);
        self.bounds.insert(id.accesskit_id(), root_rect);
        let mut root = Node::new(Role::MultilineTextInput);
        root.set_label("Markdown source editor");
        root.set_value(shaped.text.as_str());
        root.set_bounds(access_rect(root_rect));
        root.set_children(children);
        for action in [
            Action::Focus,
            Action::SetTextSelection,
            Action::ReplaceSelectedText,
            Action::SetValue,
            Action::ScrollIntoView,
        ] {
            root.add_action(action);
        }
        if let (Some(anchor), Some(focus)) = (
            self.position(selection.anchor),
            self.position(selection.head),
        ) {
            root.set_text_selection(TextSelection { anchor, focus });
        }
        let mut nodes = vec![(id, root)];
        nodes.extend(paragraph_nodes);
        nodes
    }

    /// Canonical logical caret association. A source position inside a grapheme
    /// is not an accessible caret and returns None rather than silently moving it.
    pub fn position(&self, byte: ByteOffset) -> Option<TextPosition> {
        if self.legal_boundaries.binary_search(&byte.0).is_err() {
            return None;
        }
        // Source order is monotonic even when bidi geometry is not. Prefer the
        // final run beginning at a shared boundary, matching canonical affinity.
        let index = self
            .bindings
            .partition_point(|binding| binding.source.start <= byte.0)
            .checked_sub(1)?;
        let binding = &self.bindings[index];
        let character_index = binding.boundaries.binary_search(&byte.0).ok()?;
        Some(TextPosition {
            node: binding.id.accesskit_id(),
            character_index,
        })
    }

    /// Decode only IDs emitted by the current tree and exact legal source stops.
    /// A caller passing old text must rebuild the tree before accepting actions.
    pub fn source_position(&self, position: TextPosition, text: &str) -> Option<ByteOffset> {
        if text != self.text {
            return None;
        }
        let binding = self
            .bindings
            .iter()
            .find(|binding| binding.id.accesskit_id() == position.node)?;
        let byte = *binding.boundaries.get(position.character_index)?;
        if !binding.source.contains(&byte) && byte != binding.source.end {
            return None;
        }
        self.legal_boundaries
            .binary_search(&byte)
            .ok()
            .map(|_| ByteOffset(byte))
    }

    pub fn bounds_for_node(&self, node: NodeId) -> Option<Rect> {
        self.bounds.get(&node).copied()
    }

    pub fn contains_node(&self, node: NodeId) -> bool {
        self.bounds.contains_key(&node)
    }

    pub fn root_id(&self) -> Option<NodeId> {
        self.root.map(|id| id.accesskit_id())
    }

    /// Structural transport fallback for graphemes exceeding AccessKit's u8
    /// byte-length capacity. Interior scalar positions remain rejected on input.
    pub fn scalar_fallback_runs(&self) -> usize {
        self.scalar_fallback_runs
    }

    fn allocate_serial(&mut self) -> u64 {
        let serial = self.next_serial;
        self.next_serial = self
            .next_serial
            .checked_add(1)
            .expect("accessibility identity space exhausted");
        serial
    }
}

fn reconcile_lines(old: &[LineIdentity], new: &[Range<usize>], text: &str) -> Vec<Option<usize>> {
    let mut result = vec![None; new.len()];
    let mut prefix = 0;
    while prefix < old.len() && prefix < new.len() && old[prefix].text == text[new[prefix].clone()]
    {
        result[prefix] = Some(prefix);
        prefix += 1;
    }
    let mut old_end = old.len();
    let mut new_end = new.len();
    while old_end > prefix
        && new_end > prefix
        && old[old_end - 1].text == text[new[new_end - 1].clone()]
    {
        old_end -= 1;
        new_end -= 1;
        result[new_end] = Some(old_end);
    }
    // A changed paragraph remains a paragraph at this logical editing location;
    // run identity below is stricter and never reuses unrelated deleted text.
    for (offset, index) in result[prefix..new_end].iter_mut().enumerate() {
        if prefix + offset < old_end {
            *index = Some(prefix + offset);
        }
    }
    result
}

fn match_run(
    old_line: &LineIdentity,
    range: &Range<usize>,
    text: &str,
    rtl: bool,
    used: &[bool],
) -> Option<usize> {
    old_line
        .runs
        .iter()
        .enumerate()
        .find(|(index, run)| {
            !used[*index] && run.rtl == rtl && run.range == *range && run.text == text
        })
        .or_else(|| {
            // Keep the first run's logical anchor through a soft-wrap split or
            // merge. A changed/unrelated value cannot claim the retired run ID.
            old_line.runs.iter().enumerate().find(|(index, run)| {
                !used[*index]
                    && run.rtl == rtl
                    && run.range.start == range.start
                    && (run.text.starts_with(text) || text.starts_with(&run.text))
            })
        })
        .map(|(index, _)| index)
}

fn logical_lines(text: &str) -> Vec<Range<usize>> {
    let bytes = text.as_bytes();
    let mut result = Vec::new();
    let mut start = 0;
    let mut at = 0;
    while at < bytes.len() {
        if matches!(bytes[at], b'\r' | b'\n') {
            let cr = bytes[at] == b'\r';
            at += 1;
            if cr && bytes.get(at) == Some(&b'\n') {
                at += 1;
            }
            result.push(start..at);
            start = at;
        } else {
            at += 1;
        }
    }
    result.push(start..text.len());
    result
}

fn shape_runs(
    document: &ShapedDocument,
    line: usize,
    source: Range<usize>,
    rows: &mut RowLookup,
) -> Vec<Run> {
    let value = &document.text[source.clone()];
    let mut units = Vec::new();
    for (offset, grapheme) in value.grapheme_indices(true) {
        let start = source.start + offset;
        if u8::try_from(grapheme.len()).is_ok() {
            units.push(unit(
                document,
                line,
                start..start + grapheme.len(),
                false,
                rows,
            ));
        } else {
            for (scalar, character) in grapheme.char_indices() {
                let at = start + scalar;
                units.push(unit(
                    document,
                    line,
                    at..at + character.len_utf8(),
                    true,
                    rows,
                ));
            }
        }
    }
    if units.is_empty() {
        let (_, row) = rows.row(document, line, source.start);
        let rect = row_rect(row, source.start);
        return vec![Run {
            source,
            rtl: row.rtl,
            units,
            rect,
        }];
    }
    let mut runs: Vec<Run> = Vec::new();
    for unit in units {
        if let Some(run) = runs.last_mut().filter(|run| {
            run.rtl == unit.rtl
                && run.units.last().is_some_and(|last| last.row == unit.row)
                && run.units.len() < MAX_RUN_UNITS
        }) {
            run.source.end = unit.source.end;
            run.rect = run.rect.union(unit.rect);
            run.units.push(unit);
        } else {
            runs.push(Run {
                source: unit.source.clone(),
                rtl: unit.rtl,
                rect: unit.rect,
                units: vec![unit],
            });
        }
    }
    runs
}

fn unit(
    document: &ShapedDocument,
    line: usize,
    source: Range<usize>,
    fallback: bool,
    rows: &mut RowLookup,
) -> Unit {
    let (row_index, row) = rows.row(document, line, source.start);
    let mut rect = Rect::NOTHING;
    let mut rtl = row.rtl;
    let mut found_direction = false;
    for cluster in &row.clusters {
        if cluster.source_range.start.0 < source.end && source.start < cluster.source_range.end.0 {
            if !found_direction {
                rtl = cluster.rtl;
                found_direction = true;
            }
            rect = rect.union(cluster_unit_rect(
                cluster,
                source.clone(),
                &document.text,
                row,
            ));
        }
    }
    if !rect.is_finite() {
        let caret = row_caret(row, source.start, Affinity::Before);
        let ending = document.text[source.clone()].contains(['\r', '\n']);
        rect = Rect::from_min_size(
            pos2(caret.x, row.bounds.y),
            vec2(if ending { 3.0 } else { 0.0 }, row.bounds.height),
        );
    }
    Unit {
        source,
        row: row_index,
        rtl,
        rect,
        fallback,
    }
}

fn cluster_unit_rect(
    cluster: &GlyphCluster,
    source: Range<usize>,
    text: &str,
    row: &ShapedRow,
) -> Rect {
    let boundaries: Vec<_> = text[cluster.source_range.start.0..cluster.source_range.end.0]
        .grapheme_indices(true)
        .map(|(offset, _)| cluster.source_range.start.0 + offset)
        .chain([cluster.source_range.end.0])
        .collect();
    let count = boundaries.len().saturating_sub(1).max(1) as f32;
    let start = boundaries
        .partition_point(|byte| *byte <= source.start)
        .saturating_sub(1);
    let end = boundaries
        .partition_point(|byte| *byte < source.end)
        .max(start + 1);
    let (left, right) = if cluster.rtl {
        (count - end as f32, count - start as f32)
    } else {
        (start as f32, end as f32)
    };
    Rect::from_min_max(
        pos2(
            cluster.bounds.x + cluster.bounds.width * left / count,
            row.bounds.y,
        ),
        pos2(
            cluster.bounds.x + cluster.bounds.width * right / count,
            row.bounds.y + row.bounds.height,
        ),
    )
}

fn row_caret(row: &ShapedRow, byte: usize, affinity: Affinity) -> &CaretStop {
    let exact = if affinity == Affinity::After {
        row.carets
            .iter()
            .rev()
            .find(|caret| caret.byte.0 == byte && caret.affinity == affinity)
            .or_else(|| row.carets.iter().rev().find(|caret| caret.byte.0 == byte))
    } else {
        row.carets
            .iter()
            .find(|caret| caret.byte.0 == byte && caret.affinity == affinity)
            .or_else(|| row.carets.iter().find(|caret| caret.byte.0 == byte))
    };
    exact
        .or_else(|| {
            row.carets
                .iter()
                .min_by_key(|caret| caret.byte.0.abs_diff(byte))
        })
        .expect("the chosen source row has an exact caret geometry")
}

fn row_rect(row: &ShapedRow, byte: usize) -> Rect {
    let caret = row_caret(row, byte, Affinity::After);
    Rect::from_min_size(pos2(caret.x, row.bounds.y), vec2(1.5, row.bounds.height))
}

fn access_rect(rect: Rect) -> accesskit::Rect {
    accesskit::Rect {
        x0: f64::from(rect.left()),
        y0: f64::from(rect.top()),
        x1: f64::from(rect.right()),
        y1: f64::from(rect.bottom()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glyphfolio_editor_core::Editor;
    use glyphfolio_editor_layout::{LayoutOptions, ShapingEngine};

    fn build(
        state: &mut AccessibilityState,
        document: DocumentId,
        text: &str,
        width: f32,
    ) -> Vec<(Id, Node)> {
        let shaped = ShapingEngine::new().shape(
            text,
            LayoutOptions {
                width,
                ..Default::default()
            },
        );
        state.build(
            Id::unique("test-editor"),
            Rect::from_min_size(Pos2::ZERO, vec2(width, 100.0)),
            pos2(5.0, 7.0),
            document,
            &shaped,
            Selection::caret(ByteOffset(0)),
        )
    }

    fn shared_shape(text: &str, width: f32) -> Arc<ShapedDocument> {
        Arc::new(ShapingEngine::new().shape(
            text,
            LayoutOptions {
                width,
                ..LayoutOptions::default()
            },
        ))
    }

    #[test]
    fn projected_headings_gain_levels_without_changing_editable_text_contract() {
        use glyphfolio_editor_markdown::{MarkdownParser, ProjectionOptions};
        let editor = Editor::new(
            "# Top\n\n## **日本** &amp; Title\r\n\r\nbody\n\n###### Small\n\nSetext title\n------------\n",
        );
        let projection = MarkdownParser::new()
            .parse(&editor.snapshot())
            .project(ProjectionOptions::default());
        let shaped = shared_shape(projection.text(), 300.0);
        let id = Id::unique("rich-heading-semantics");
        let rect = Rect::from_min_size(Pos2::ZERO, vec2(300.0, 100.0));
        let mut state = AccessibilityState::new();
        let original = state.build_shared(
            id,
            rect,
            Pos2::ZERO,
            editor.snapshot().document_id(),
            &shaped,
            Selection::default(),
        );
        let cache_stats = state.cache_stats();
        let row_stats = state.row_lookup_stats();
        let mut decorated = original.clone();
        assert!(state.decorate_rich_nodes(&mut decorated, &projection));
        assert_eq!(decorated[0].1.label(), Some("Rich Markdown editor"));
        assert_eq!(decorated[0].1.role(), Role::MultilineTextInput);
        assert_eq!(decorated[0].1.children(), original[0].1.children());
        assert_eq!(
            decorated[0].1.text_selection(),
            original[0].1.text_selection()
        );
        for action in [
            Action::Focus,
            Action::SetTextSelection,
            Action::ReplaceSelectedText,
            Action::SetValue,
            Action::ScrollIntoView,
        ] {
            assert!(decorated[0].1.supports_action(action));
        }
        let headings: Vec<_> = decorated
            .iter()
            .filter(|(_, node)| node.role() == Role::Heading)
            .collect();
        assert_eq!(
            headings
                .iter()
                .map(|(_, node)| node.level().unwrap())
                .collect::<Vec<_>>(),
            vec![0, 1, 5, 1]
        );
        let heading_values: Vec<_> = headings
            .iter()
            .map(|(_, node)| {
                assert!(node.supports_action(Action::ScrollIntoView));
                node.children()
                    .iter()
                    .map(|child| {
                        let (_, run) = decorated
                            .iter()
                            .find(|(id, _)| id.accesskit_id() == *child)
                            .unwrap();
                        assert_eq!(run.role(), Role::TextRun);
                        run.value().unwrap_or_default()
                    })
                    .collect::<String>()
                    .trim_end_matches(['\r', '\n'])
                    .to_owned()
            })
            .collect();
        assert_eq!(
            heading_values,
            ["Top", "日本 & Title", "Small", "Setext title"]
        );
        for ((before_id, before), (after_id, after)) in original.iter().zip(&decorated) {
            assert_eq!(before_id, after_id);
            if before.role() == Role::TextRun {
                assert_eq!(before, after);
            }
        }
        let once = decorated.clone();
        assert!(state.decorate_rich_nodes(&mut decorated, &projection));
        assert_eq!(decorated, once, "decoration is idempotent");
        assert_eq!(state.cache_stats(), cache_stats);
        assert_eq!(state.row_lookup_stats(), row_stats);
        let cached = state.shared.as_ref().unwrap();
        assert_eq!(
            cached.nodes, original,
            "the cached emission remains untouched"
        );
        for (byte, _) in projection.text().grapheme_indices(true) {
            let at = state.position(ByteOffset(byte)).unwrap();
            assert_eq!(
                state.source_position(at, projection.text()),
                Some(ByteOffset(byte))
            );
        }
    }

    #[test]
    fn rich_decoration_rejects_foreign_projection_and_preedit_geometry() {
        use glyphfolio_editor_markdown::{MarkdownParser, ProjectionOptions};
        let editor = Editor::new("# Heading\n");
        let foreign = Editor::new("# Heading\n");
        let mut parser = MarkdownParser::new();
        let projection = parser
            .parse(&editor.snapshot())
            .project(ProjectionOptions::default());
        let foreign_projection = parser
            .parse(&foreign.snapshot())
            .project(ProjectionOptions::default());
        let mut state = AccessibilityState::new();
        let mut nodes = build(
            &mut state,
            editor.snapshot().document_id(),
            projection.text(),
            300.0,
        );
        let before = nodes.clone();
        assert!(!state.decorate_rich_nodes(&mut nodes, &foreign_projection));
        assert_eq!(nodes, before);
        let mut overlay_nodes = build(
            &mut state,
            editor.snapshot().document_id(),
            &format!("前{}", projection.text()),
            300.0,
        );
        let overlay_before = overlay_nodes.clone();
        assert!(!state.decorate_rich_nodes(&mut overlay_nodes, &projection));
        assert_eq!(overlay_nodes, overlay_before);
    }

    #[test]
    fn shared_semantics_reuse_bindings_and_only_refresh_root_selection() {
        let text = "alpha e\u{301}\n日本 beta";
        let document = Editor::new(text).snapshot().document_id();
        let shaped = shared_shape(text, 300.0);
        let id = Id::unique("cached-editor");
        let rect = Rect::from_min_size(Pos2::ZERO, vec2(300.0, 100.0));
        let origin = pos2(5.0, 7.0);
        let mut state = AccessibilityState::new();
        state.prepare_shared(id, rect, origin, document, &shaped, Selection::default());
        let initial_row_work = state.row_lookup_stats();
        assert_eq!(state.cache_stats().semantic_rebuilds, 1);
        assert_eq!(state.cache_stats().cache_hits, 0);
        assert!(state.cache_stats().cached_nodes > 1);
        let first = state.build_shared(id, rect, origin, document, &shaped, Selection::default());
        let bindings: Vec<_> = state
            .bindings
            .iter()
            .map(|binding| {
                (
                    binding.id,
                    binding.source.clone(),
                    binding.boundaries.clone(),
                )
            })
            .collect();
        let selected = Selection {
            anchor: ByteOffset(0),
            head: ByteOffset(text.find(" beta").unwrap()),
            ..Selection::default()
        };
        let next = state.build_shared(id, rect, origin, document, &shaped, selected);
        assert_eq!(
            first[1..],
            next[1..],
            "unchanged geometry keeps all non-root semantic data"
        );
        assert_eq!(first[0].0, next[0].0);
        assert_ne!(first[0].1.text_selection(), next[0].1.text_selection());
        assert_eq!(
            next[0].1.text_selection().unwrap().focus,
            state.position(selected.head).unwrap()
        );
        assert_eq!(
            bindings,
            state
                .bindings
                .iter()
                .map(|binding| (
                    binding.id,
                    binding.source.clone(),
                    binding.boundaries.clone()
                ))
                .collect::<Vec<_>>()
        );
        assert_eq!(state.cache_stats().semantic_rebuilds, 1);
        assert_eq!(state.cache_stats().selection_updates, 1);
        let _ = state.build_shared(id, rect, origin, document, &shaped, selected);
        assert_eq!(
            state.cache_stats().selection_updates,
            1,
            "identical selection does not mutate the cached root"
        );
        let invalid = Selection::caret(ByteOffset(text.find('e').unwrap() + 1));
        let after = state.build_shared(id, rect, origin, document, &shaped, invalid);
        assert!(
            after[0].1.text_selection().is_none(),
            "an illegal combining boundary cannot retain an older visible selection"
        );
        assert_eq!(state.cache_stats().semantic_rebuilds, 1);
        assert_eq!(state.cache_stats().selection_updates, 2);
        assert_eq!(
            state.row_lookup_stats(),
            initial_row_work,
            "selection-only cache hits perform no row indexing or unit lookups"
        );
    }

    #[test]
    fn indexed_rows_match_containment_geometry_for_unicode_bidi_and_wrapped_endings() {
        let text = format!(
            "office e\u{301} नमस्ते 👩‍👩‍👧‍👦 English العربية עברית repeated long words\r\ne{}\n\nlast",
            "\u{301}".repeat(300)
        );
        let document = shared_shape(&text, 85.0);
        let lines = logical_lines(&text);
        let mut index = RowLookup::new(&document, lines.len());
        for (line, source) in lines.iter().enumerate() {
            for (offset, grapheme) in text[source.clone()].grapheme_indices(true) {
                let byte = source.start + offset;
                let selected = index.row(&document, line, byte);
                let reference = document.rows.iter().enumerate().find(|(_, row)| {
                    row.logical_line == line
                        && row.source_range.start.0 <= byte
                        && byte < row.source_range.end.0
                });
                if let Some((expected, _)) = reference {
                    assert_eq!(
                        selected.0, expected,
                        "indexed glyph row differs at source byte {byte}"
                    );
                }
                let semantic = unit(
                    &document,
                    line,
                    byte..byte + grapheme.len(),
                    false,
                    &mut index,
                );
                assert_eq!(semantic.row, selected.0);
                assert!(semantic.rect.is_finite());
                assert!((semantic.rect.top() - selected.1.bounds.y).abs() < 0.01);
            }
        }
        let ending = text.find("\r\n").unwrap();
        let last_row = document
            .rows
            .iter()
            .enumerate()
            .filter(|(_, row)| row.logical_line == 0)
            .max_by_key(|(_, row)| row.visual_index)
            .unwrap();
        let ending_unit = unit(&document, 0, ending..ending + 2, false, &mut index);
        assert_eq!(ending_unit.row, last_row.0);
        assert_eq!(
            ending_unit.rect.top(),
            last_row.1.bounds.y,
            "wrapped CRLF belongs to the final exact row"
        );
        let caret = row_caret(last_row.1, ending, Affinity::Before);
        assert_eq!(ending_unit.rect.left(), caret.x);
    }

    fn fixed_cell_document(count: usize) -> Arc<ShapedDocument> {
        use glyphfolio_editor_layout::Rect as LayoutRect;
        let mut text = String::with_capacity(count * 4);
        let mut rows = Vec::with_capacity(count + 1);
        for line in 0..=count {
            let start = text.len();
            let value = if line == count { "" } else { "row\n" };
            text.push_str(value);
            let y = line as f32 * 20.0;
            let letters = value.len().saturating_sub(1);
            let carets = (0..=letters)
                .map(|column| CaretStop {
                    byte: ByteOffset(start + column),
                    affinity: if column == letters && letters > 0 {
                        Affinity::Before
                    } else {
                        Affinity::After
                    },
                    x: column as f32 * 8.0,
                    y,
                    height: 20.0,
                })
                .collect();
            let clusters = (0..letters)
                .map(|column| GlyphCluster {
                    source_range: ByteOffset(start + column)..ByteOffset(start + column + 1),
                    bounds: LayoutRect {
                        x: column as f32 * 8.0,
                        y,
                        width: 8.0,
                        height: 20.0,
                    },
                    rtl: false,
                })
                .collect();
            rows.push(ShapedRow {
                source_range: ByteOffset(start)..ByteOffset(text.len()),
                logical_line: line,
                visual_index: 0,
                bounds: LayoutRect {
                    x: 0.0,
                    y,
                    width: (letters as f32 * 8.0).max(1.0),
                    height: 20.0,
                },
                rtl: false,
                carets,
                clusters,
            });
        }
        Arc::new(ShapedDocument {
            text,
            rows,
            glyphs: Vec::new(),
            width: 320.0,
            height: (count + 1) as f32 * 20.0,
            diagnostics: Default::default(),
            table_cells: Vec::new(),
        })
    }

    #[test]
    fn row_index_preserves_original_priority_for_overlaps_and_nearest_gaps() {
        let mut document = (*fixed_cell_document(1)).clone();
        let template = document.rows[0].clone();
        document.text = "abcdefghijkl".to_owned();
        document.rows = [(5, 7), (0, 10), (1, 2), (12, 12)]
            .into_iter()
            .map(|(start, end)| {
                let mut row = template.clone();
                row.source_range = ByteOffset(start)..ByteOffset(end);
                row
            })
            .collect();
        let mut index = RowLookup::new(&document, 1);
        for (byte, expected) in [(0, 1), (1, 1), (5, 0), (6, 0), (7, 1), (9, 1)] {
            assert_eq!(index.row(&document, 0, byte).0, expected);
        }
        assert_eq!(index.row(&document, 0, 10).0, 1);
        assert_eq!(index.row(&document, 0, 11).0, 1);
        assert_eq!(index.row(&document, 0, 12).0, 3);
    }

    #[test]
    fn ten_thousand_line_semantics_have_bounded_row_index_work_and_reuse() {
        // Exact synthetic fixed-cell geometry isolates semantic construction;
        // real native font coverage is qualified by the separate shaping suite.
        let shaped = fixed_cell_document(10_000);
        let document = Editor::new(&shaped.text).snapshot().document_id();
        let root = Id::unique("large-semantic-counter-fixture");
        let rect = Rect::from_min_size(Pos2::ZERO, vec2(shaped.width, shaped.height));
        let mut state = AccessibilityState::new();
        let started = std::time::Instant::now();
        state.prepare_shared(
            root,
            rect,
            Pos2::ZERO,
            document,
            &shaped,
            Selection::default(),
        );
        let cold = started.elapsed();
        let work = state.row_lookup_stats();
        assert_eq!(work.index_builds, 1);
        assert_eq!(work.indexed_rows, 10_001);
        assert_eq!(work.lookups, 40_001);
        assert!(
            work.binary_probes <= work.lookups * 4,
            "each unit searches only its exact logical-line range"
        );
        assert!(work.candidates_visited <= work.lookups * 2);
        assert!(
            work.index_payload_bytes <= (shaped.rows.len() + 10_001) * 64,
            "row-index vector payload is bounded by rows plus logical lines"
        );
        let started = std::time::Instant::now();
        for line in 0..100 {
            state.prepare_shared(
                root,
                rect,
                Pos2::ZERO,
                document,
                &shaped,
                Selection::caret(ByteOffset(line * 400)),
            );
        }
        let warm = started.elapsed();
        assert_eq!(
            state.row_lookup_stats(),
            work,
            "warm selection frames allocate no new row index and perform no geometry lookup"
        );
        assert_eq!(state.cache_stats().semantic_rebuilds, 1);
        assert_eq!(state.cache_stats().cache_hits, 100);
        let nodes = state.build_shared(
            root,
            rect,
            Pos2::ZERO,
            document,
            &shaped,
            Selection::default(),
        );
        assert_eq!(nodes[0].1.children().len(), 10_001);
        let last = *nodes[0].1.children().last().unwrap();
        assert_eq!(
            state.bounds_for_node(last).unwrap().top(),
            200_000.0,
            "offscreen content retains exact fixture geometry"
        );
        eprintln!(
            "SEMANTIC_ROW_BENCH logical_lines=10001 nodes={} cold_ms={:.3} warm_prepare_100_ms={:.3} index_payload_bytes={} lookups={} probes={} candidates={}",
            nodes.len(),
            cold.as_secs_f64() * 1000.0,
            warm.as_secs_f64() * 1000.0,
            work.index_payload_bytes,
            work.lookups,
            work.binary_probes,
            work.candidates_visited
        );
    }

    #[test]
    fn shared_cache_owns_shape_and_arc_make_mut_cannot_reuse_stale_geometry() {
        let text = "geometry";
        let document = Editor::new(text).snapshot().document_id();
        let mut shaped = shared_shape(text, 300.0);
        let id = Id::unique("mutable-shape-editor");
        let rect = Rect::from_min_size(Pos2::ZERO, vec2(300.0, 100.0));
        let mut state = AccessibilityState::new();
        let initial = state.build_shared(
            id,
            rect,
            Pos2::ZERO,
            document,
            &shaped,
            Selection::default(),
        );
        let old_pointer = Arc::as_ptr(&shaped);
        let old = Arc::downgrade(&shaped);
        assert_eq!(
            Arc::strong_count(&shaped),
            2,
            "the cache owns its immutable geometry"
        );
        Arc::make_mut(&mut shaped).rows[0].clusters[0].bounds.x += 40.0;
        assert!(!std::ptr::eq(old_pointer, Arc::as_ptr(&shaped)));
        assert!(
            old.upgrade().is_some(),
            "old allocation remains alive until cache invalidation"
        );
        let updated = state.build_shared(
            id,
            rect,
            Pos2::ZERO,
            document,
            &shaped,
            Selection::default(),
        );
        assert!(old.upgrade().is_none());
        assert_eq!(state.cache_stats().semantic_rebuilds, 2);
        assert_eq!(initial[0].1.children(), updated[0].1.children());
        let old_bounds: Vec<_> = initial
            .iter()
            .filter(|(_, node)| node.role() == Role::TextRun)
            .map(|(_, node)| node.bounds())
            .collect();
        let new_bounds: Vec<_> = updated
            .iter()
            .filter(|(_, node)| node.role() == Role::TextRun)
            .map(|(_, node)| node.bounds())
            .collect();
        assert_ne!(
            old_bounds, new_bounds,
            "fresh shape geometry must reach native semantics"
        );
    }

    #[test]
    fn shared_cache_invalidates_placement_surface_and_document_identity() {
        let text = "stable text\nsecond line";
        let document = Editor::new(text).snapshot().document_id();
        let other_document = Editor::new(text).snapshot().document_id();
        let shaped = shared_shape(text, 300.0);
        let id = Id::unique("placement-editor");
        let other_id = Id::unique("other-placement-editor");
        let rect = Rect::from_min_size(Pos2::ZERO, vec2(300.0, 100.0));
        let mut state = AccessibilityState::new();
        let first = state.build_shared(
            id,
            rect,
            Pos2::ZERO,
            document,
            &shaped,
            Selection::default(),
        );
        let old_caret = state.position(ByteOffset(0)).unwrap();
        let resized = Rect::from_min_size(Pos2::ZERO, vec2(320.0, 120.0));
        let changed_bounds = state.build_shared(
            id,
            resized,
            Pos2::ZERO,
            document,
            &shaped,
            Selection::default(),
        );
        assert_ne!(first[0].1.bounds(), changed_bounds[0].1.bounds());
        let moved = state.build_shared(
            id,
            resized,
            pos2(20.0, 30.0),
            document,
            &shaped,
            Selection::default(),
        );
        assert_eq!(
            first[0].1.children(),
            moved[0].1.children(),
            "repositioning preserves logical paragraph IDs"
        );
        assert_ne!(first[1].1.bounds(), moved[1].1.bounds());
        state.prepare_shared(
            other_id,
            resized,
            pos2(20.0, 30.0),
            document,
            &shaped,
            Selection::default(),
        );
        assert!(state.source_position(old_caret, text).is_none());
        let old_surface = state.position(ByteOffset(0)).unwrap();
        state.prepare_shared(
            other_id,
            resized,
            pos2(20.0, 30.0),
            other_document,
            &shaped,
            Selection::default(),
        );
        assert!(state.source_position(old_surface, text).is_none());
        assert_eq!(state.cache_stats().semantic_rebuilds, 5);
    }

    #[test]
    fn borrowed_build_invalidates_foreign_geometry_but_can_reuse_owned_arc() {
        let text = "original";
        let document = Editor::new(text).snapshot().document_id();
        let shaped = shared_shape(text, 300.0);
        let different = shared_shape("different", 300.0);
        let id = Id::unique("borrowed-cache-editor");
        let rect = Rect::from_min_size(Pos2::ZERO, vec2(300.0, 100.0));
        let mut state = AccessibilityState::new();
        state.prepare_shared(
            id,
            rect,
            Pos2::ZERO,
            document,
            &shaped,
            Selection::default(),
        );
        let old = state.position(ByteOffset(0)).unwrap();
        let _ = state.build(
            id,
            rect,
            Pos2::ZERO,
            document,
            &different,
            Selection::default(),
        );
        assert_eq!(state.cache_stats().cached_nodes, 0);
        assert!(state.source_position(old, "different").is_none());
        state.prepare_shared(
            id,
            rect,
            Pos2::ZERO,
            document,
            &shaped,
            Selection::default(),
        );
        assert_ne!(
            old.node,
            state.position(ByteOffset(0)).unwrap().node,
            "retired run IDs are not recycled when old bytes return"
        );
        let _ = state.build(
            id,
            rect,
            Pos2::ZERO,
            document,
            &shaped,
            Selection::default(),
        );
        assert_eq!(state.cache_stats().semantic_rebuilds, 3);
        assert_eq!(state.cache_stats().cache_hits, 1);
        assert!(state.cache_stats().cached_nodes > 0);
    }

    #[test]
    fn source_order_units_and_crlf_roundtrip_without_duplicate_parentage() {
        let text = "a e\u{301}\r\nאבג Latin\n";
        let document = Editor::new(text).snapshot().document_id();
        let mut state = AccessibilityState::new();
        let nodes = build(&mut state, document, text, 170.0);
        let flattened = nodes
            .iter()
            .filter(|(_, node)| node.role() == Role::TextRun)
            .map(|(_, node)| node.value().unwrap_or_default())
            .collect::<String>();
        assert_eq!(flattened, text);
        for (id, node) in nodes.iter().skip(1) {
            assert_eq!(
                nodes
                    .iter()
                    .filter(|(_, parent)| parent.children().contains(&id.accesskit_id()))
                    .count(),
                1
            );
            if node.role() == Role::TextRun {
                assert_eq!(
                    node.character_lengths()
                        .iter()
                        .map(|length| usize::from(*length))
                        .sum::<usize>(),
                    node.value().unwrap_or_default().len()
                );
                assert_eq!(
                    node.character_lengths().len(),
                    node.character_positions()
                        .expect("positions supplied")
                        .len()
                );
                assert_eq!(
                    node.character_lengths().len(),
                    node.character_widths().expect("widths supplied").len()
                );
            }
        }
        for (byte, _) in text.grapheme_indices(true).chain([(text.len(), "")]) {
            let position = state.position(ByteOffset(byte)).unwrap();
            assert_eq!(
                state.source_position(position, text),
                Some(ByteOffset(byte))
            );
        }
        assert!(
            state.position(ByteOffset(3)).is_none(),
            "inside combining grapheme is not a caret"
        );
        assert!(
            state
                .source_position(state.position(ByteOffset(0)).unwrap(), "old text")
                .is_none()
        );
    }
    #[test]
    fn accessible_word_starts_cross_soft_wraps_without_whitespace_words() {
        let text = "alpha supercalifragilistic beta";
        let editor = Editor::new(text);
        let mut state = AccessibilityState::default();
        let nodes = build(&mut state, editor.snapshot().document_id(), text, 45.0);
        let mut starts = Vec::new();
        let mut continued_word_run = false;
        for (id, node) in nodes
            .iter()
            .filter(|(_, node)| node.role() == Role::TextRun)
        {
            let binding = state
                .bindings
                .iter()
                .find(|binding| binding.id == *id)
                .unwrap();
            starts.extend(
                node.word_starts()
                    .iter()
                    .map(|index| binding.boundaries[*index as usize]),
            );
            if binding.source.start > 6 && binding.source.end < text.find(" beta").unwrap() {
                continued_word_run = true;
                assert!(node.word_starts().is_empty());
            }
        }
        assert!(
            continued_word_run,
            "fixture must split the long word across runs"
        );
        assert_eq!(starts, vec![0, 6, text.find("beta").unwrap()]);
    }

    #[test]
    fn stable_line_and_run_identity_survives_prefix_insertion_and_soft_reflow() {
        let document = Editor::new("").snapshot().document_id();
        let mut state = AccessibilityState::new();
        let text = "keep this long source line across soft wrapping changes\nsecond stable line";
        let nodes = build(&mut state, document, text, 900.0);
        let paragraph_ids = nodes[0].1.children().to_vec();
        let old_first_run = state.position(ByteOffset(0)).unwrap().node;
        let narrow = build(&mut state, document, text, 120.0);
        assert_eq!(paragraph_ids, narrow[0].1.children());
        assert_eq!(old_first_run, state.position(ByteOffset(0)).unwrap().node);
        let prefixed = format!("inserted\n{text}");
        let after = build(&mut state, document, &prefixed, 120.0);
        assert_eq!(paragraph_ids, &after[0].1.children()[1..]);
        assert_eq!(
            old_first_run,
            state.position(ByteOffset("inserted\n".len())).unwrap().node
        );
    }

    #[test]
    fn removed_and_foreign_document_ids_cannot_be_reused_for_actions() {
        let first = Editor::new("old").snapshot().document_id();
        let second = Editor::new("new").snapshot().document_id();
        let mut state = AccessibilityState::new();
        build(&mut state, first, "old", 500.0);
        let old = state.position(ByteOffset(0)).unwrap();
        build(&mut state, first, "unrelated", 500.0);
        assert!(state.source_position(old, "unrelated").is_none());
        let replaced = state.position(ByteOffset(0)).unwrap();
        build(&mut state, second, "unrelated", 500.0);
        assert!(state.source_position(replaced, "unrelated").is_none());
    }

    #[test]
    fn logical_paragraph_identity_survives_an_edit_without_reusing_unrelated_runs() {
        let original = "first edit me\nsecond keep me\n";
        let changed = "first edited by a host\nsecond keep me\n";
        let document = Editor::new(original).snapshot().document_id();
        let mut state = AccessibilityState::new();
        let before = build(&mut state, document, original, 900.0);
        let old_first = state.position(ByteOffset(0)).unwrap();
        let unchanged = state
            .position(ByteOffset(original.find("second").unwrap()))
            .unwrap();
        let after = build(&mut state, document, changed, 900.0);
        assert_eq!(before[0].1.children(), after[0].1.children());
        assert!(state.source_position(old_first, changed).is_none());
        assert_eq!(
            state.source_position(unchanged, changed),
            Some(ByteOffset(changed.find("second").unwrap()))
        );
    }

    #[test]
    fn oversized_grapheme_transport_is_structural_and_rejects_scalar_interiors() {
        let text = format!("e{}\r\n", "\u{301}".repeat(300));
        let document = Editor::new(&text).snapshot().document_id();
        let mut state = AccessibilityState::new();
        let nodes = build(&mut state, document, &text, 500.0);
        assert!(state.scalar_fallback_runs() > 0);
        let first = state.position(ByteOffset(0)).unwrap();
        let inside = TextPosition {
            character_index: 1,
            ..first
        };
        assert!(state.source_position(inside, &text).is_none());
        let runs: Vec<_> = nodes
            .iter()
            .filter(|(_, node)| node.role() == Role::TextRun)
            .collect();
        assert_eq!(
            runs.iter()
                .map(|(_, node)| node.value().unwrap_or_default())
                .collect::<String>(),
            text
        );
        assert!(runs.iter().any(|(_, node)| {
            node.value().unwrap_or_default().ends_with("\r\n")
                && node.character_lengths().last() == Some(&2)
        }));
        assert!(
            state.position(ByteOffset(text.len() - 1)).is_none(),
            "CRLF is one selectable unit"
        );
    }

    #[test]
    fn mixed_direction_runs_use_directional_positions_and_keep_all_offscreen_text() {
        let text = format!(
            "Latin אבג العربية end\n{}final offscreen",
            "source line\n".repeat(30)
        );
        let document = Editor::new(&text).snapshot().document_id();
        let mut state = AccessibilityState::new();
        let nodes = build(&mut state, document, &text, 180.0);
        let rtl_runs: Vec<_> = nodes
            .iter()
            .filter(|(_, node)| {
                node.role() == Role::TextRun
                    && node.text_direction() == Some(TextDirection::RightToLeft)
            })
            .collect();
        assert!(!rtl_runs.is_empty());
        for (_, run) in rtl_runs {
            assert!(
                run.character_positions()
                    .expect("positions supplied")
                    .windows(2)
                    .all(|pair| pair[1] + 0.01 >= pair[0])
            );
        }
        let final_position = state.position(ByteOffset(text.len())).unwrap();
        assert!(state.bounds_for_node(final_position.node).unwrap().top() > 100.0);
        assert!(state.contains_node(final_position.node));
        let source_text = nodes
            .iter()
            .filter(|(_, node)| node.role() == Role::TextRun)
            .map(|(_, node)| node.value().unwrap_or_default())
            .collect::<String>();
        assert_eq!(source_text, text);
    }
}
