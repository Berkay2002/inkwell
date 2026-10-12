//! Form data as CSV and tab-delimited text: Acrobat's Export Data and Import Data text formats. A header
//! row of full field names, then one row per record. The file is hostile input: it is read in one pass and
//! only the header, a short label per row and the one row asked for are kept.
//!
//! CSV follows RFC 4180 (quotes, doubled quotes, line breaks inside quotes), with the delimiter taken from the
//! header row (comma, semicolon for spreadsheets set to a Swedish locale, or tab). Tab text is read literally,
//! as it is written: it has no quoting, so a cell that Excel wrapped in quotes keeps them.

use crate::engine::EngineError;
use crate::exchange::{bad, MAX_ITEMS, MAX_STR};

type Result<T> = std::result::Result<T, EngineError>;

/// Records (rows under the header) in one file.
pub const MAX_ROWS: usize = MAX_ITEMS;
/// Longest label the row picker shows.
const LABEL: usize = 60;

/// What a table file holds.
#[derive(Debug, Default)]
pub struct Table {
    /// The field names, one per column.
    pub header: Vec<String>,
    /// The first cell that has text, per record (what the row picker lists).
    pub labels: Vec<String>,
    /// The line of each record, as a spreadsheet numbers rows: blank lines count, a line break inside quotes does not.
    pub rows: Vec<usize>,
    /// The record asked for: a cell is `None` when it is longer than 1 MB.
    pub picked: Option<Vec<Option<String>>>,
}

fn delimiter(text: &str) -> char {
    let (mut tab, mut comma, mut semi, mut quoted) = (0usize, 0usize, 0usize, false);
    for c in text.chars() {
        match c {
            '"' => quoted = !quoted,
            '\n' | '\r' if !quoted => break,
            '\t' if !quoted => tab += 1,
            ',' if !quoted => comma += 1,
            ';' if !quoted => semi += 1,
            _ => {}
        }
    }
    // The most common one; a tie goes to tab, then comma.
    if tab > 0 && tab >= comma && tab >= semi {
        '\t'
    } else if semi > comma {
        ';'
    } else {
        ','
    }
}

/// Reads `text`, keeping the record numbered `pick` (0 is the first row under the header). More than
/// [`MAX_ROWS`] rows or [`MAX_ITEMS`] columns, and a quote that never closes, refuse the file whole: a part of a
/// file would be a wrong answer. A row is dropped once read, so memory is the header, the labels and one row.
pub fn read(text: &str, pick: Option<usize>) -> Result<Table> {
    let delim = delimiter(text);
    let quoting = delim != '\t';
    let mut table = Table::default();
    let mut rows = 0usize;
    // Records seen, blank ones too: the row number a spreadsheet shows.
    let mut line = 0usize;
    let mut header_done = false;
    let mut record: Vec<String> = Vec::new();
    let mut cell = String::new();
    let mut quoted = false;
    // A cell began with a quote and was closed: what follows it up to the delimiter is kept as it is.
    let mut was_quoted = false;
    let mut at_start = true;

    // A cell stops growing once it is over the limit (it is "too long" from then on): a file whose quote never
    // closes is one cell, and must not be copied whole.
    let grow = |cell: &mut String, c: char| {
        if cell.len() <= MAX_STR {
            cell.push(c);
        }
    };
    let end_record = |record: &mut Vec<String>, table: &mut Table, rows: &mut usize, line: &mut usize, header_done: &mut bool| -> Result<()> {
        let done = std::mem::take(record);
        *line += 1;
        // A blank line, or a row of nothing but empty cells (spreadsheets pad with them), is not a record.
        if done.iter().all(String::is_empty) {
            return Ok(());
        }
        if !*header_done {
            *header_done = true;
            table.header = done;
            return Ok(());
        }
        if *rows >= MAX_ROWS {
            return Err(bad(format!("This file has more than {MAX_ROWS} rows, so Inkwell does not read it.")));
        }
        // Control and bidirectional formatting characters are spaces: a label is shown in a list.
        let plain = |c: char| if c.is_control() || matches!(c, '\u{200E}' | '\u{200F}' | '\u{061C}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}') { ' ' } else { c };
        let label: String = done.iter().find(|c| !c.is_empty()).map_or(String::new(), |c| c.chars().map(plain).take(LABEL).collect());
        table.labels.push(label);
        table.rows.push(*line);
        if pick == Some(*rows) {
            table.picked = Some(done.into_iter().map(|c| (c.len() <= MAX_STR).then_some(c)).collect());
        }
        *rows += 1;
        Ok(())
    };

    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        let push_cell = |record: &mut Vec<String>, cell: &mut String| -> Result<()> {
            if record.len() >= MAX_ITEMS {
                return Err(bad(format!("This file has more than {MAX_ITEMS} columns, so Inkwell does not read it.")));
            }
            record.push(std::mem::take(cell));
            Ok(())
        };
        match c {
            '"' if quoting && quoted => {
                if chars.peek() == Some(&'"') {
                    chars.next();
                    grow(&mut cell, '"');
                } else {
                    quoted = false;
                    was_quoted = true;
                }
            }
            '"' if quoting && at_start && !was_quoted => {
                quoted = true;
                at_start = false;
            }
            _ if quoted => grow(&mut cell, c),
            c if c == delim => {
                push_cell(&mut record, &mut cell)?;
                at_start = true;
                was_quoted = false;
            }
            '\r' | '\n' => {
                if c == '\r' && chars.peek() == Some(&'\n') {
                    chars.next();
                }
                push_cell(&mut record, &mut cell)?;
                end_record(&mut record, &mut table, &mut rows, &mut line, &mut header_done)?;
                at_start = true;
                was_quoted = false;
            }
            c => {
                grow(&mut cell, c);
                at_start = false;
            }
        }
    }
    if quoted {
        return Err(bad("This file is cut off: a quoted value never ends."));
    }
    if !cell.is_empty() || !record.is_empty() || was_quoted {
        if record.len() >= MAX_ITEMS {
            return Err(bad(format!("This file has more than {MAX_ITEMS} columns, so Inkwell does not read it.")));
        }
        record.push(cell);
        end_record(&mut record, &mut table, &mut rows, &mut line, &mut header_done)?;
    }
    if !header_done {
        return Err(bad("This file is empty: a data file has a row of field names, then a row of values."));
    }
    if rows == 0 {
        return Err(bad("This file has a row of field names but no row of values."));
    }
    Ok(table)
}

/// A CSV cell: quoted when it holds a comma, a quote or a line break (RFC 4180).
fn csv_cell(s: &str) -> String {
    if s.contains([',', '"', '\r', '\n']) { format!("\"{}\"", s.replace('"', "\"\"")) } else { s.to_string() }
}

/// UTF-8 text starts with a byte order mark so a spreadsheet opens it as UTF-8 and not as a code page.
const BOM: &str = "\u{FEFF}";

/// The file text: a header row and one record row (CRLF, as RFC 4180 has it). Every value is one cell.
pub fn csv(names: &[&str], values: &[String]) -> String {
    let row = |v: &mut dyn Iterator<Item = String>| v.collect::<Vec<_>>().join(",");
    format!("{BOM}{}\r\n{}\r\n", row(&mut names.iter().map(|n| csv_cell(n))), row(&mut values.iter().map(|v| csv_cell(v))))
}

/// Tab text has no quoting, so a tab or line break inside a value is written as a space: returns the text and how
/// many values that changed.
pub fn tab(names: &[&str], values: &[String]) -> (String, usize) {
    let clean = |s: &str| s.replace(['\t', '\r', '\n'], " ");
    let changed = names.iter().copied().chain(values.iter().map(String::as_str)).filter(|s| s.contains(['\t', '\r', '\n'])).count();
    let row = |v: Vec<String>| v.join("\t");
    (format!("{BOM}{}\r\n{}\r\n", row(names.iter().map(|n| clean(n)).collect()), row(values.iter().map(|v| clean(v)).collect())), changed)
}
