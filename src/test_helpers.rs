use unicode_width::UnicodeWidthStr;

fn check_sorted(rows: &Vec<Vec<String>>) {
    let mut lengths: Vec<usize> = rows.iter().map(Vec::len).collect::<Vec<_>>();
    lengths.sort();
    if lengths[0] != lengths[lengths.len() - 1] {
        panic!("jagged rows passed to `format_grid`: {:?}", rows);
    }
}

fn empty_string_of_length_n(n: usize) -> String {
    format!("{:1$}", "", n)
}

pub fn pad_right_with_spaces(s: &str, width: usize) -> String {
    let len = UnicodeWidthStr::width(s);
    let num_spaces = width.saturating_sub(len);
    format!("{}{:num_spaces$}", s, "")
}

pub fn format_table(rows: &Vec<Vec<String>>, with_borders: bool) -> String {
    if rows.is_empty() {
        return "".to_string();
    }

    check_sorted(&rows);

    // If the table is:
    //
    // +---+----+
    // | 1 |  2 |
    // |   |  3 |
    // +---+----+
    // | a | bc |
    // | d |    |
    // +---+----+
    //
    // after the process a row we'll have:
    //
    // [ [ Some "1" ]
    // , [ Some "2", Some "3" ]
    // ]
    //
    // and we'll make them all the same length to contain:
    //
    // [ [ Some "1", Some "" ]
    // , [ Some "2", Some "3" ]
    // ]
    //
    // and then eventually it will be:
    //
    // [ [ Some "1", Some "",  None, Some "a", Some "d" ]
    // , [ Some "2", Some "3", None, Some "bc", Some "" ]
    // ]
    //
    // Then we'll pad the Somes in each column to be the same width.
    let mut column_lines = vec![];
    for _ in 0..rows[0].len() {
        column_lines.push(vec![]);
    }

    let num_cols = rows[0].len();

    for (y, row) in rows.iter().enumerate() {
        // Add None in between rows.
        if y != 0 {
            for x in 0..num_cols {
                column_lines[x].push(None);
            }
        }

        // Add the lines for each cell
        for (x, value) in row.iter().enumerate() {
            for line in value.lines() {
                column_lines[x].push(Some(line.to_string()));
            }
        }

        // Find tallest column
        let max_height = column_lines.iter().map(Vec::len).max().unwrap();

        // Pad each column so they're the same height
        for column in column_lines.iter_mut() {
            let needed_lines = max_height - column.len();
            for _ in 0..needed_lines {
                column.push(Some("".to_string()));
            }
        }
    }

    // Now figure out the width of line in the columns and pad the strings so
    // they're all the same width.
    let mut column_widths = vec![];
    for column in column_lines.iter_mut() {
        let max_width = column
            .iter()
            .filter_map(|s| s.as_deref().map(UnicodeWidthStr::width))
            .max()
            .unwrap();
        column_widths.push(max_width);

        for row_line in column.iter_mut() {
            if let Some(s) = row_line {
                let needed_padding = max_width - UnicodeWidthStr::width(s.as_str());
                for _ in 0..needed_padding {
                    s.push(' ');
                }
            }
        }
    }

    let mut top_border = "┌".to_string();
    let mut inner_border = "├".to_string();
    let mut bottom_border = "└".to_string();

    for (i, column_width) in column_widths.iter().enumerate() {
        if i != 0 {
            top_border.push('┬');
            inner_border.push('┼');
            bottom_border.push('┴');
        }

        // Two spaces of padding on each side
        for _ in 0..(column_width + 2) {
            top_border.push('─');
            inner_border.push('─');
            bottom_border.push('─');
        }
    }

    top_border.push('┐');
    inner_border.push('┤');
    bottom_border.push('┘');

    let mut output = String::new();

    if with_borders {
        output.push_str(&top_border);
        output.push('\n');
    }

    for row in 0..(column_lines[0].len()) {
        if column_lines[0][row].is_none() {
            if with_borders {
                output.push_str(&inner_border);
                output.push('\n');
            }
            continue;
        }

        if with_borders {
            output.push('│');
        }

        for (x, column) in column_lines.iter().enumerate() {
            // With borders, always print a space before a cell line,
            // but without borders, only add one after the first column.
            if with_borders {
                output.push(' ');
            } else {
                if x != 0 {
                    output.push(' ');
                }
            }

            output.push_str(column[row].as_ref().unwrap());

            if with_borders {
                output.push_str(" │");
            }
        }

        // Truncate trailing whitespace if we don't have borders.
        if !with_borders {
            output.truncate(output.trim_end().len());
        }

        output.push('\n');
    }

    if with_borders {
        output.push_str(&bottom_border);
        output.push('\n');
    }

    output
}

#[cfg(test)]
mod test {
    use super::*;

    use insta::assert_snapshot;

    #[test]
    fn test_format_table() {
        let table = vec![
            vec!["1".to_string(), "2\n3".to_string(), "🦀".to_string()],
            vec!["a\nbc".to_string(), "d".to_string(), "x".to_string()],
        ];

        assert_snapshot!(format_table(&table, true), @r"
        ┌────┬───┬────┐
        │ 1  │ 2 │ 🦀 │
        │    │ 3 │    │
        ├────┼───┼────┤
        │ a  │ d │ x  │
        │ bc │   │    │
        └────┴───┴────┘
        ");

        assert_snapshot!(format_table(&table, false), @r"
        1  2 🦀
           3
        a  d x
        bc
        ");
    }
}
