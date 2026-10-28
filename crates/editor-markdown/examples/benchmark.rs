//! Measured grammar/projection costs; source copying/range shifts remain O(N).
use glyphfolio_editor_core::{ByteOffset, Editor, Transaction};
use glyphfolio_editor_markdown::{MarkdownParser, ParseMode, ProjectionOptions};
use std::time::Instant;

fn main() {
    let source = (0..10_000)
        .map(|index| format!("paragraph {index} stable words\n\n"))
        .collect::<String>();
    let mut editor = Editor::new(&source);
    let mut parser = MarkdownParser::new();
    let started = Instant::now();
    let first = parser.parse(&editor.snapshot());
    let full_parse_ms = started.elapsed().as_secs_f64() * 1000.0;
    let started = Instant::now();
    let projection = first.project(ProjectionOptions::default());
    let projection_ms = started.elapsed().as_secs_f64() * 1000.0;
    let mut full_samples = Vec::new();
    let mut projection_samples = Vec::new();
    for _ in 0..30 {
        let started = Instant::now();
        let fresh = MarkdownParser::new().parse(&editor.snapshot());
        full_samples.push(started.elapsed().as_secs_f64() * 1000.0);
        let started = Instant::now();
        let projected = fresh.project(ProjectionOptions::default());
        projection_samples.push(started.elapsed().as_secs_f64() * 1000.0);
        assert_eq!(projected.text(), projection.text());
    }
    full_samples.sort_by(f64::total_cmp);
    projection_samples.sort_by(f64::total_cmp);
    let at = source.find("paragraph 5000 ").unwrap() + "paragraph 5000 ".len();
    let mut measurements = Vec::new();
    for _ in 0..30 {
        let change = editor
            .apply(
                Transaction::new(&editor.snapshot()).replace(ByteOffset(at)..ByteOffset(at), "x"),
            )
            .unwrap();
        let started = Instant::now();
        let result = parser.parse_change(&change);
        measurements.push(started.elapsed().as_secs_f64() * 1000.0);
        assert_eq!(result.stats().mode, ParseMode::IncrementalParagraph);
        assert!(result.stats().parsed_bytes < 100);
        assert_eq!(result.blocks().len(), first.blocks().len());
    }
    measurements.sort_by(f64::total_cmp);
    println!(
        "MARKDOWN_BENCH source_bytes={} blocks={} display_bytes={} full_parse_ms={full_parse_ms:.3} projection_ms={projection_ms:.3} incremental_p50_ms={:.3} incremental_p95_ms={:.3} samples=30 source_copy_and_shift=O(N) grammar_parsed_bytes_per_edit_lt=100",
        source.len(),
        first.blocks().len(),
        projection.text().len(),
        measurements[15],
        measurements[28]
    );
    println!(
        "MARKDOWN_COMPONENT_DISTRIBUTION samples=30 full_parse_p50_ms={:.3} full_parse_p95_ms={:.3} full_parse_p99_ms={:.3} projection_build_p50_ms={:.3} projection_build_p95_ms={:.3} projection_build_p99_ms={:.3} incremental_p99_ms={:.3} includes_ui=false includes_shaping=false",
        full_samples[15],
        full_samples[28],
        full_samples[29],
        projection_samples[15],
        projection_samples[28],
        projection_samples[29],
        measurements[29]
    );
}
