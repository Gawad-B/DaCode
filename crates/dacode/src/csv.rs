//! Rainbow CSV: each column of a `.csv`, `.tsv` or `.psv` file in a colour of its own.
//!
//! Computed for each row as it is drawn, and never stored. That is the whole design: a table is
//! read by its columns, which a parse of the file would give, but a parse of the file is a
//! document-sized walk and a stored answer is an eighth thing `EditorState::mutate` would have to
//! move. A row's columns are known from its own line alone, so drawing asks the line and the cost
//! is the viewport's.
//!
//! The limitation that buys, stated rather than hidden: a quoted field that runs over a line break
//! is coloured from its own line's start, so the line after the break begins in the first column's
//! colour. And a line longer than [`SCAN_LIMIT`] characters is drawn plain past that point, so a
//! one-line export of a million fields costs a frame what any other long line costs.

use std::ops::Range;

/// How far into a line its columns are worked out. Past this the rest of the line is plain.
pub const SCAN_LIMIT: usize = 16 * 1024;

/// The separator a file's name says its fields use, or `None` for a file that is not a table.
pub fn delimiter_for(name: &str) -> Option<char> {
    let extension = name.rsplit_once('.')?.1.to_ascii_lowercase();
    match extension.as_str() {
        "csv" => Some(','),
        "tsv" | "tab" => Some('\t'),
        "psv" => Some('|'),
        _ => None,
    }
}

/// The fields of `line`, as char ranges in it, each with its column number. A delimiter belongs to
/// no field, so it is drawn plain and the columns read as separated.
///
/// Quotes are RFC 4180's: a field that starts with `"` runs to the next `"` not doubled, so
/// `"a,b"` is one field and `""` inside it is a quote, not its end.
pub fn fields(line: &str, delimiter: char) -> Vec<(usize, Range<usize>)> {
    let mut out = Vec::new();
    let mut column = 0;
    let mut start = 0;
    let mut end = 0;
    let mut quoted = false;
    let mut at_field_start = true;
    let mut chars = line.chars().take(SCAN_LIMIT).enumerate().peekable();
    while let Some((index, c)) = chars.next() {
        if c != '\r' && c != '\n' {
            end = index + 1;
        }
        if quoted {
            if c == '"' {
                if chars.peek().map(|(_, next)| *next) == Some('"') {
                    chars.next();
                    end = index + 2;
                } else {
                    quoted = false;
                }
            }
            continue;
        }
        if c == '"' && at_field_start {
            quoted = true;
            at_field_start = false;
            continue;
        }
        if c == delimiter {
            out.push((column, start..index));
            column += 1;
            start = index + 1;
            at_field_start = true;
            continue;
        }
        if c != '\r' && c != '\n' {
            at_field_start = false;
        }
    }
    out.push((column, start..end.max(start)));
    out
}

/// The column the char at `index` of `line` is in. A delimiter counts as the column before it,
/// which is where a caret just after a field's last character is.
pub fn column_at(line: &str, delimiter: char, index: usize) -> usize {
    let found = fields(line, delimiter);
    found
        .iter()
        .find(|(_, range)| index <= range.end)
        .or(found.last())
        .map_or(0, |(column, _)| *column)
}

/// The name of column `column`: the header row's field, unquoted, or `None` when the header has no
/// such field or it is empty.
pub fn header_name(header: &str, delimiter: char, column: usize) -> Option<String> {
    let (_, range) = fields(header, delimiter)
        .into_iter()
        .find(|(index, _)| *index == column)?;
    let text: String = header
        .chars()
        .skip(range.start)
        .take(range.end - range.start)
        .collect();
    let text = text.trim();
    let text = text
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
        .map(|inner| inner.replace("\"\"", "\""))
        .unwrap_or_else(|| text.to_string());
    (!text.is_empty()).then_some(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn columns(line: &str, delimiter: char) -> Vec<(usize, String)> {
        fields(line, delimiter)
            .into_iter()
            .map(|(column, range)| {
                let text = line
                    .chars()
                    .skip(range.start)
                    .take(range.end - range.start)
                    .collect();
                (column, text)
            })
            .collect()
    }

    fn expect(pairs: &[(usize, &str)]) -> Vec<(usize, String)> {
        pairs
            .iter()
            .map(|(column, text)| (*column, text.to_string()))
            .collect()
    }

    #[test]
    fn a_files_extension_names_its_delimiter() {
        assert_eq!(delimiter_for("data.csv"), Some(','));
        assert_eq!(delimiter_for("DATA.TSV"), Some('\t'));
        assert_eq!(delimiter_for("x.psv"), Some('|'));
        assert_eq!(delimiter_for("main.rs"), None);
        assert_eq!(delimiter_for("csv"), None, "a name, not an extension");
    }

    #[test]
    fn each_field_is_numbered_and_the_delimiters_belong_to_none() {
        assert_eq!(
            columns("id,name,,age\n", ','),
            expect(&[(0, "id"), (1, "name"), (2, ""), (3, "age")])
        );
    }

    #[test]
    fn a_quoted_delimiter_and_a_doubled_quote_stay_inside_their_field() {
        // The whole reason this is not `split(',')`: an address with a comma in it is one field,
        // and so is a quotation with quotes in it.
        assert_eq!(
            columns(r#"1,"Smith, Jane","She said ""hi""",x"#, ','),
            expect(&[
                (0, "1"),
                (1, r#""Smith, Jane""#),
                (2, r#""She said ""hi""""#),
                (3, "x")
            ])
        );
        // A quote in the middle of a field is a character, not the start of quoting.
        assert_eq!(columns("a\"b,c", ','), expect(&[(0, "a\"b"), (1, "c")]));
    }

    #[test]
    fn a_tab_separated_line_splits_on_tabs_only() {
        assert_eq!(
            columns("a,b\tc d\r\n", '\t'),
            expect(&[(0, "a,b"), (1, "c d")])
        );
    }

    #[test]
    fn the_caret_names_its_column_from_the_header() {
        let header = r#"id,"full name",age"#;
        let row = "7,Ada Lovelace,36";
        assert_eq!(column_at(row, ',', 0), 0);
        assert_eq!(column_at(row, ',', 4), 1);
        assert_eq!(
            column_at(row, ',', 1),
            0,
            "just after a field is still that field"
        );
        assert_eq!(
            column_at(row, ',', 99),
            2,
            "past the end is the last column"
        );
        assert_eq!(
            header_name(header, ',', column_at(row, ',', 4)).as_deref(),
            Some("full name")
        );
        assert_eq!(header_name(header, ',', 9), None);
    }

    #[test]
    fn a_line_past_the_scan_limit_is_split_only_as_far_as_the_limit() {
        // Bounded however long the line: a one-line export costs what any long line does.
        let line = "ab,".repeat(SCAN_LIMIT);
        let found = fields(&line, ',');
        assert!(found.len() <= SCAN_LIMIT / 3 + 1);
        assert!(found.last().unwrap().1.end <= SCAN_LIMIT);
    }
}
