# Rich editing and native tables

Rich presentation keeps Markdown delimiters folded while the caret moves through bold, italic, strike, code and links. Moving the caret does not change text geometry. Source presentation remains the exact canonical Markdown, with the same selection and undo history.

## Table editing

Tables use native shaped cells, wrapped text, bold headers, grid borders and a focused-cell outline. Separator colons control left, center or right alignment. Equal-width columns have a minimum width; wide tables scroll horizontally.

- Tab / Shift-Tab: next / previous cell; Tab beyond the final cell adds a row.
- Enter: next row in the same column; adds a row at the end.
- Shift-Enter: a cell line break, stored as `<br>`.
- Cmd/Ctrl-Enter: leave the table for a paragraph. Arrow Down from its final visual row also exits.
- Table toolbar menu or cell context menu: add/delete rows or columns, clear a cell, change alignment, leave or delete the table.

Deleting the header promotes the first body row. Removing the final column removes the table. Boundary Backspace/Delete protects structural pipes. Text insertion escapes pipes within a cell. Each structural operation uses the original transaction engine and shared undo history. BOM and CRLF regression cases are covered.

## Inline and paragraph behavior

Formatting buttons reflect the caret's marks and heading. Choosing a mark at an ordinary collapsed caret sets a pending typing style without inserting empty Markdown delimiters. Actual text, including committed composition text, materializes the mark in one transaction. Trailing spaces stay outside the closing delimiter.

Visible-text replacement protects hidden syntax and removes wrappers whose entire content was deleted. Enter creates a real paragraph break, balances active inline formatting and preserves list/quote behavior. Backspace can join ordinary paragraphs and merge matching marks. Pure edit planning lives in the egui-free `glyphfolio_editor_markdown::editing` module; the egui adapter supplies interaction and rendering.

## Evidence and limits

The local workspace passes 230 tests, including the official 652-example CommonMark corpus within one test, native widget pointer/key regressions, source-map replacement/undo checks and table geometry/navigation checks. Rendering captures and validation logs are in [the evidence directory](evidence/rich-stable/).

These checks do not qualify real OS IME, clipboard transport, screen readers or Windows/Linux runtime behavior. Rich preedit retains its temporary Source safeguard, as do very large styled paragraphs. Tables are shaped as whole blocks; per-row virtualization, user-resizable columns, merged cells and rectangular spreadsheet selection are future work. Ragged and unsupported container tables retain editable source. This increment changes only the independent embeddable editor, not either GlyphFolio application.
