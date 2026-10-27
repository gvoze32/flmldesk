//! CSV export of player lists.
//!
//! RFC 4180 quoting, CRLF line ends and a UTF-8 byte order mark, so Excel on
//! Windows opens the file with accented names intact.

const BOM: &str = "\u{FEFF}";

fn field(out: &mut String, value: &str) {
    if value.contains([',', '"', '\n', '\r']) {
        out.push('"');
        out.push_str(&value.replace('"', "\"\""));
        out.push('"');
    } else {
        out.push_str(value);
    }
}

/// CSV text for `rows`; the first row is normally the header.
pub fn csv<R, S>(rows: impl IntoIterator<Item = R>) -> String
where
    R: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut out = String::from(BOM);
    for row in rows {
        for (i, value) in row.into_iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            field(&mut out, value.as_ref());
        }
        out.push_str("\r\n");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_only_fields_that_need_it() {
        let text = csv([vec!["Name", "Club"], vec!["Džeko, Edin", "Club \"A\""], vec!["Line\nbreak", "plain"]]);
        assert_eq!(text, "\u{FEFF}Name,Club\r\n\"Džeko, Edin\",\"Club \"\"A\"\"\"\r\n\"Line\nbreak\",plain\r\n");
    }
}
