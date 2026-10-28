use glyphfolio_editor_core::{
    ByteOffset, EditCommand, Editor, MarkdownTable, Selection, TableCommand,
};
fn select(editor: &mut Editor, a: usize, b: usize) {
    editor
        .set_selection(Selection {
            anchor: ByteOffset(a),
            head: ByteOffset(b),
            ..Selection::default()
        })
        .unwrap();
}
#[test]
fn marks_and_task_operations_share_exact_history() {
    let source = "\u{feff}日本\r\n- [X] done\r\n";
    let mut editor = Editor::new(source);
    select(&mut editor, 3, 9);
    editor.execute(EditCommand::ToggleStrikethrough).unwrap();
    assert!(editor.snapshot().text().starts_with("\u{feff}~~日本~~"));
    editor.execute(EditCommand::ToggleStrikethrough).unwrap();
    assert_eq!(editor.snapshot().text(), source);
    let at = source.find("done").unwrap();
    select(&mut editor, at, at);
    editor.execute(EditCommand::ToggleTaskChecked).unwrap();
    assert!(editor.snapshot().text().contains("- [ ] done"));
    editor.undo().unwrap();
    assert_eq!(editor.snapshot().text(), source);
    select(&mut editor, source.len() - 2, source.len() - 2);
    editor.execute(EditCommand::Enter).unwrap();
    assert!(editor.snapshot().text().contains("done\r\n- [ ] \r\n"));
    editor.undo().unwrap();
    assert_eq!(editor.snapshot().text(), source);
}
#[test]
fn tables_preserve_unedited_cells_and_undo_each_operation() {
    let source = "before\r\n\r\n| A | B |\r\n| :--- | ---: |\r\n| x\\|y | 日本 |\r\n\r\nafter";
    let mut editor = Editor::new(source);
    let at = source.find("日本").unwrap();
    select(&mut editor, at, at);
    let table = MarkdownTable::at(source, ByteOffset(at)).unwrap();
    assert_eq!(table.rows[1].cells.len(), 2);
    for command in [
        TableCommand::AddRow,
        TableCommand::AddColumn,
        TableCommand::DeleteColumn,
        TableCommand::DeleteRow,
    ] {
        select(&mut editor, at, at);
        let before = editor.snapshot().text();
        editor.edit_table(command).unwrap().expect("changes table");
        editor.undo().unwrap();
        assert_eq!(editor.snapshot().text(), before);
    }
    editor.edit_table(TableCommand::PreviousCell).unwrap();
    assert_eq!(
        editor.snapshot().slice(editor.selection().range()).unwrap(),
        "x\\|y"
    );
    assert!(MarkdownTable::at("```md\n| A | B |\n| --- | --- |\n```\n", ByteOffset(10)).is_none());
}

#[test]
fn table_mutations_change_the_intended_shape_and_keep_alignment_source() {
    let source = "| A | B |\r\n| :--- | ---: |\r\n| x | y |";
    for (command, rows, columns) in [
        (TableCommand::AddRow, 3, 2),
        (TableCommand::AddColumn, 2, 3),
        (TableCommand::DeleteColumn, 2, 1),
        (TableCommand::DeleteRow, 1, 2),
    ] {
        let mut editor = Editor::new(source);
        select(
            &mut editor,
            source.find('y').unwrap(),
            source.find('y').unwrap(),
        );
        editor.edit_table(command).unwrap().unwrap();
        let after = editor.snapshot().text();
        let table = MarkdownTable::at(&after, ByteOffset(0)).unwrap();
        assert_eq!(table.rows.len(), rows);
        assert_eq!(table.separator.cells.len(), columns);
        assert!(table.rows.iter().all(|row| row.cells.len() == columns));
        assert!(after.contains(":---"));
        if command == TableCommand::AddRow {
            assert!(
                table
                    .rows
                    .last()
                    .unwrap()
                    .cells
                    .iter()
                    .any(|cell| cell.start == editor.selection().head)
            );
        }
        editor.undo().unwrap();
        assert_eq!(editor.snapshot().text(), source);
    }
}

#[test]
fn adding_tasks_keeps_quote_and_fence_structure() {
    let mut editor = Editor::new("> quoted\n\n```\n- [ ] literal\n```\n");
    select(&mut editor, 2, 2);
    editor.execute(EditCommand::ToggleTaskList).unwrap();
    assert!(editor.snapshot().text().starts_with("> - [ ] quoted\n"));
    editor.undo().unwrap();
    let source = editor.snapshot().text();
    let at = source.find("literal").unwrap();
    select(&mut editor, at, at);
    assert!(
        editor
            .execute(EditCommand::ToggleTaskChecked)
            .unwrap()
            .is_none()
    );
    assert_eq!(editor.snapshot().text(), source);
}

#[test]
fn mixed_formatting_selections_apply_consistently_instead_of_inverting_each_line() {
    for (source, command, expected) in [
        (
            "~~done~~\nnew",
            EditCommand::ToggleStrikethrough,
            "~~done~~\n~~new~~",
        ),
        (
            "- [x] done\n- new",
            EditCommand::ToggleTaskList,
            "- [x] done\n- [ ] new",
        ),
    ] {
        let mut editor = Editor::new(source);
        select(&mut editor, 0, source.len());
        editor.execute(command).unwrap().unwrap();
        assert_eq!(editor.snapshot().text(), expected);
        editor.undo().unwrap();
        assert_eq!(editor.snapshot().text(), source);
    }
}

#[test]
fn table_alignment_clear_header_promotion_and_final_column_deletion_undo_exactly() {
    use glyphfolio_editor_core::ColumnAlignment;
    let source = "| A | B |\r\n| --- | --- |\r\n| x | y |\r\n";
    for command in [
        TableCommand::Align(ColumnAlignment::Center),
        TableCommand::ClearCell,
        TableCommand::DeleteRow,
        TableCommand::DeleteTable,
    ] {
        let mut editor = Editor::new(source);
        let at = if command == TableCommand::DeleteRow {
            2
        } else {
            source.find('y').unwrap()
        };
        select(&mut editor, at, at);
        editor.edit_table(command).unwrap().unwrap();
        let after = editor.snapshot().text();
        match command {
            TableCommand::Align(_) => assert_eq!(
                MarkdownTable::at(&after, ByteOffset(0))
                    .unwrap()
                    .alignments(&after)[1],
                ColumnAlignment::Center
            ),
            TableCommand::ClearCell => assert!(!after.contains('y')),
            TableCommand::DeleteRow => {
                let table = MarkdownTable::at(&after, ByteOffset(0)).unwrap();
                assert_eq!(table.rows.len(), 1);
                assert_eq!(
                    &after[table.rows[0].cells[0].start.0..table.rows[0].cells[0].end.0],
                    "x"
                );
            }
            TableCommand::DeleteTable => assert_eq!(after, ""),
            _ => unreachable!(),
        }
        editor.undo().unwrap();
        assert_eq!(editor.snapshot().text(), source);
    }
    let mut editor = Editor::new("| A |\n| --- |\n");
    editor
        .edit_table(TableCommand::DeleteColumn)
        .unwrap()
        .unwrap();
    assert_eq!(editor.snapshot().text(), "");
    editor.undo().unwrap();
    assert_eq!(editor.snapshot().text(), "| A |\n| --- |\n");
}

#[test]
fn table_bom_and_unsupported_ragged_alignment_are_safe() {
    use glyphfolio_editor_core::ColumnAlignment;
    let source = "\u{feff}| A | B |\r\n| --- | --- |\r\n| x | y |\r\n";
    let mut editor = Editor::new(source);
    select(&mut editor, 5, 5);
    assert!(MarkdownTable::at(source, ByteOffset(5)).is_some());
    editor.edit_table(TableCommand::DeleteRow).unwrap().unwrap();
    assert!(editor.snapshot().text().starts_with('\u{feff}'));
    editor.undo().unwrap();
    assert_eq!(editor.snapshot().text(), source);
    editor
        .edit_table(TableCommand::DeleteTable)
        .unwrap()
        .unwrap();
    assert_eq!(editor.snapshot().text(), "\u{feff}");
    editor.undo().unwrap();
    assert_eq!(editor.snapshot().text(), source);
    let source = "| A | B |\n| --- | --- |\n| x | y | z |\n";
    let mut editor = Editor::new(source);
    let at = source.find('z').unwrap();
    select(&mut editor, at, at);
    assert!(
        editor
            .edit_table(TableCommand::Align(ColumnAlignment::Right))
            .unwrap()
            .is_none()
    );
    assert_eq!(editor.snapshot().text(), source);
}
