//! Text output helpers: aligned tables, CSV, and number formatting.

/// `1234567` → `1,234,567`.
pub fn number(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

pub fn bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = n as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{n} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

fn is_numeric(cell: &str) -> bool {
    !cell.is_empty()
        && cell
            .chars()
            .all(|c| c.is_ascii_digit() || c == ',' || c == '.')
}

/// Prints rows as aligned columns. Numeric columns are right-aligned. Long
/// cells are cut so one wide value does not wreck the layout.
pub fn print_table(headers: &[&str], rows: &[Vec<String>]) {
    const MAX_WIDTH: usize = 80;
    let clip = |cell: &str| -> String {
        if cell.chars().count() > MAX_WIDTH {
            let cut: String = cell.chars().take(MAX_WIDTH - 1).collect();
            format!("{cut}…")
        } else {
            cell.to_owned()
        }
    };
    let rows: Vec<Vec<String>> = rows
        .iter()
        .map(|row| row.iter().map(|c| clip(c)).collect())
        .collect();
    let columns = headers.len();
    let mut widths: Vec<usize> = headers.iter().map(|h| h.chars().count()).collect();
    let mut numeric = vec![true; columns];
    for row in &rows {
        for (i, cell) in row.iter().enumerate().take(columns) {
            widths[i] = widths[i].max(cell.chars().count());
            if !cell.is_empty() && !is_numeric(cell) {
                numeric[i] = false;
            }
        }
    }
    let line = |cells: Vec<&str>| {
        let mut out = String::new();
        for (i, cell) in cells.iter().enumerate() {
            let pad = widths[i].saturating_sub(cell.chars().count());
            if i > 0 {
                out.push_str("  ");
            }
            if numeric[i] {
                out.push_str(&" ".repeat(pad));
                out.push_str(cell);
            } else {
                out.push_str(cell);
                if i + 1 < cells.len() {
                    out.push_str(&" ".repeat(pad));
                }
            }
        }
        println!("{out}");
    };
    line(headers.to_vec());
    let rules: Vec<String> = widths.iter().map(|w| "-".repeat(*w)).collect();
    line(rules.iter().map(String::as_str).collect());
    for row in &rows {
        line(row.iter().map(String::as_str).collect());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers() {
        assert_eq!(number(0), "0");
        assert_eq!(number(999), "999");
        assert_eq!(number(1_000), "1,000");
        assert_eq!(number(1_234_567), "1,234,567");
    }

    #[test]
    fn sizes() {
        assert_eq!(bytes(512), "512 B");
        assert_eq!(bytes(1536), "1.5 KB");
        assert_eq!(bytes(5 * 1024 * 1024 * 1024), "5.0 GB");
    }
}
