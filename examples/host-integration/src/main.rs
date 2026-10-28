fn main() -> Result<(), Box<dyn std::error::Error>> {
    let report = glyphfolio_editor_host_example::exercise_public_api()?;
    println!(
        "Host fixture PASS: revision={}, committed_events={}, bytes={}, clean={}",
        report.revision, report.committed_events, report.bytes, report.clean,
    );
    let mut arguments = std::env::args().skip(1);
    if let Some(argument) = arguments.next() {
        if argument != "--save-reopen" {
            return Err("Expected --save-reopen /explicit/host-owned/path.md".into());
        }
        let path = arguments.next().ok_or("Missing save/reopen destination")?;
        if arguments.next().is_some() {
            return Err("Unexpected additional argument".into());
        }
        use glyphfolio_editor_core::{ByteOffset, Editor, Selection, TransactionOrigin};
        use glyphfolio_editor_host_example::{reopen_utf8, save_snapshot_atomic};
        let original = "\u{feff}# Generic host\r\n\r\nNative 日本 😀  \r\n";
        let mut editor = Editor::new(original);
        editor.set_selection(Selection::caret(ByteOffset(editor.snapshot().byte_len())))?;
        editor.insert_text("temporary edit", TransactionOrigin::Typing)?;
        editor.undo()?.ok_or("Expected edit history")?;
        let receipt = save_snapshot_atomic(editor.snapshot(), &path)?;
        receipt.acknowledge(&mut editor)?;
        let reopened = reopen_utf8(&path)?;
        if reopened.snapshot().text() != original
            || editor.is_dirty()
            || reopened.snapshot().document_id() == receipt.document_id()
        {
            return Err("Host edit/undo/save/reopen fidelity failed".into());
        }
        println!(
            "Host disk fixture PASS: revision={}, bytes={}, exact_utf8=true, fresh_session=true, path={path}",
            receipt.revision().0,
            reopened.snapshot().byte_len()
        );
    }
    Ok(())
}
