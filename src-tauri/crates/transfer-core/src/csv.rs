//! CSV cell quoting shared by the saved-report export and `NOT-COPIED.csv`.

/// Quotes a cell when it needs it and defuses a leading `= + - @` (or tab / CR), which Excel
/// would run as a formula.
pub fn csv_cell(cell: &str) -> String {
    let cell = if cell.starts_with(['=', '+', '-', '@', '\t', '\r']) {
        format!("'{cell}")
    } else {
        cell.to_string()
    };
    if cell.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", cell.replace('"', "\"\""))
    } else {
        cell
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_and_defuses_formulas() {
        assert_eq!(csv_cell("plain.txt"), "plain.txt");
        assert_eq!(csv_cell("a, \"b\""), "\"a, \"\"b\"\"\"");
        assert_eq!(csv_cell("=HYPERLINK(x)"), "'=HYPERLINK(x)");
        assert_eq!(csv_cell("\tx"), "'\tx");
        assert_eq!(csv_cell("\rx"), "\"'\rx\"");
    }
}
