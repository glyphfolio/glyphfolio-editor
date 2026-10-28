//! Coordinate conversion for the original UI-independent shaping geometry.
use egui::{Id, Pos2, Rect, pos2, vec2};
use glyphfolio_editor_core::{Affinity, ByteOffset};
use glyphfolio_editor_layout::{CaretStop, Point, ShapedDocument, ViewportLayout};
use glyphfolio_editor_markdown::{DisplayOffset, Projection, StyleKind};
use std::{ops::Range, sync::Arc};

#[derive(Clone, Debug)]
pub(crate) struct Geometry {
    pub document: Arc<ShapedDocument>,
    projection: Option<Arc<Projection>>,
    overlay: Option<OverlayMap>,
    viewport: Option<Arc<ViewportLayout>>,
}

/// Disposable preedit replacement over base display coordinates. It never owns
/// an editable document or authorizes a transaction against these coordinates.
#[derive(Clone, Debug)]
pub(crate) struct OverlayMap {
    pub replaced: Range<usize>,
    pub displayed: Range<usize>,
}

impl Geometry {
    pub fn from_shaped(_id: Id, document: Arc<ShapedDocument>) -> Self {
        Self {
            document,
            projection: None,
            overlay: None,
            viewport: None,
        }
    }
    pub fn projected(
        document: Arc<ShapedDocument>,
        projection: Option<Arc<Projection>>,
        overlay: Option<OverlayMap>,
        viewport: Option<Arc<ViewportLayout>>,
    ) -> Self {
        Self {
            document,
            projection,
            overlay,
            viewport,
        }
    }
    pub fn to_display(&self, source: usize, affinity: Affinity) -> usize {
        let base = self
            .projection
            .as_ref()
            .and_then(|p| p.source_to_display(ByteOffset(source), affinity).ok())
            .map_or(source, |at| at.0);
        self.overlay.as_ref().map_or(base, |overlay| {
            map_replacement(base, &overlay.replaced, &overlay.displayed, affinity)
        })
    }
    pub fn to_source(&self, display: usize, affinity: Affinity) -> usize {
        let base = self.overlay.as_ref().map_or(display, |overlay| {
            map_replacement(display, &overlay.displayed, &overlay.replaced, affinity)
        });
        self.projection
            .as_ref()
            .and_then(|p| p.display_to_source(DisplayOffset(base), affinity).ok())
            .map_or(base, |at| at.0)
    }
    pub fn projection(&self) -> Option<&Projection> {
        self.projection.as_deref()
    }
    fn source_caret(&self, mut caret: CaretStop) -> CaretStop {
        caret.byte = ByteOffset(self.to_source(caret.byte.0, caret.affinity));
        caret
    }
    pub fn display_caret(&self, byte: usize, affinity: Affinity) -> CaretStop {
        self.viewport
            .as_ref()
            .and_then(|view| view.caret(ByteOffset(byte), affinity))
            .unwrap_or_else(|| self.document.caret(ByteOffset(byte), affinity))
    }
    pub fn display_caret_rect(&self, byte: usize, affinity: Affinity) -> Rect {
        rect(self.display_caret(byte, affinity).rect())
    }
    pub fn display_selection_rects(&self, range: Range<usize>) -> Vec<Rect> {
        self.viewport
            .as_ref()
            .map_or_else(
                || {
                    self.document
                        .selection_rects(ByteOffset(range.start)..ByteOffset(range.end))
                },
                |view| view.selection_rects(ByteOffset(range.start)..ByteOffset(range.end)),
            )
            .into_iter()
            .map(rect)
            .collect()
    }
    pub fn caret(&self, byte: ByteOffset, affinity: Affinity) -> CaretStop {
        self.source_caret(self.display_caret(self.to_display(byte.0, affinity), affinity))
    }
    pub fn visual_neighbor(
        &self,
        byte: ByteOffset,
        affinity: Affinity,
        forward: bool,
    ) -> CaretStop {
        let at = ByteOffset(self.to_display(byte.0, affinity));
        let caret = self.viewport.as_ref().map_or_else(
            || self.document.visual_neighbor(at, affinity, forward),
            |view| {
                view.visual_neighbor(at, affinity, forward)
                    .unwrap_or_else(|| self.display_caret(at.0, affinity))
            },
        );
        self.source_caret(caret)
    }
    pub fn vertical_target(
        &self,
        byte: ByteOffset,
        affinity: Affinity,
        down: bool,
        preferred_x: f32,
    ) -> CaretStop {
        let at = ByteOffset(self.to_display(byte.0, affinity));
        let caret = self.viewport.as_ref().map_or_else(
            || {
                self.document
                    .vertical_target(at, affinity, down, preferred_x)
            },
            |view| {
                view.vertical_target(at, affinity, down, preferred_x)
                    .unwrap_or_else(|| self.display_caret(at.0, affinity))
            },
        );
        self.source_caret(caret)
    }
    pub fn row_edge(&self, byte: ByteOffset, affinity: Affinity, end: bool) -> CaretStop {
        let at = ByteOffset(self.to_display(byte.0, affinity));
        let caret = self.viewport.as_ref().map_or_else(
            || self.document.row_edge(at, affinity, end),
            |view| {
                view.row_edge(at, affinity, end)
                    .unwrap_or_else(|| self.display_caret(at.0, affinity))
            },
        );
        self.source_caret(caret)
    }
    pub fn hit_test_caret(&self, point: Pos2) -> CaretStop {
        let point = Point {
            x: point.x,
            y: point.y,
        };
        self.source_caret(
            self.viewport
                .as_ref()
                .and_then(|view| view.hit_test(point))
                .unwrap_or_else(|| self.document.hit_test(point)),
        )
    }
    pub fn caret_with_affinity(&self, byte: usize, affinity: Affinity) -> Rect {
        self.display_caret_rect(self.to_display(byte, affinity), affinity)
    }
    pub fn selection_rects(&self, range: Range<usize>) -> Vec<Rect> {
        if range.start >= range.end {
            return Vec::new();
        }
        self.display_selection_rects(
            self.to_display(range.start, Affinity::Before)
                ..self.to_display(range.end, Affinity::After),
        )
    }
    pub fn task_at(&self, point: Pos2) -> Option<ByteOffset> {
        let projection = self.projection.as_ref()?;
        projection.styles().iter().find_map(|span| {
            if matches!(span.kind, StyleKind::TaskCheckbox { .. })
                && self
                    .display_selection_rects(span.display_range.start.0..span.display_range.end.0)
                    .iter()
                    .any(|bounds| bounds.contains(point))
            {
                projection
                    .display_to_source(span.display_range.start, Affinity::Before)
                    .ok()
            } else {
                None
            }
        })
    }

    pub fn link_at(&self, point: Pos2) -> Option<String> {
        let projection = self.projection.as_ref()?;
        let caret = self.document.hit_test(Point {
            x: point.x,
            y: point.y,
        });
        projection
            .styles_intersecting(
                DisplayOffset(caret.byte.0)..DisplayOffset(caret.byte.0.saturating_add(1)),
            )
            .find_map(|span| match &span.kind {
                StyleKind::Link { destination, .. }
                    if span.display_range.start.0 <= caret.byte.0
                        && caret.byte.0 < span.display_range.end.0 =>
                {
                    Some(destination.clone())
                }
                _ => None,
            })
    }
}

pub(crate) fn map_replacement(
    at: usize,
    old: &Range<usize>,
    new: &Range<usize>,
    affinity: Affinity,
) -> usize {
    if at < old.start {
        at
    } else if at > old.end {
        new.end.saturating_add(at - old.end)
    } else if at == old.end && old.start != old.end {
        new.end
    } else if affinity == Affinity::Before {
        new.start
    } else {
        new.end
    }
}
pub(crate) fn rect(value: glyphfolio_editor_layout::Rect) -> Rect {
    Rect::from_min_size(pos2(value.x, value.y), vec2(value.width, value.height))
}
pub(crate) fn translated(value: Rect, origin: Pos2) -> Rect {
    value.translate(origin.to_vec2())
}
pub(crate) fn relative(point: Pos2, origin: Pos2) -> Pos2 {
    (point - origin).to_pos2()
}
