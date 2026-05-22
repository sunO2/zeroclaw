use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use similar::{ChangeTag, TextDiff};

// Color palette — dark-terminal analogues of Toad's $success/$error theme:
//   added lines   →  dark green bg, bright green fg
//   removed lines →  dark red bg,   bright red fg
//   context lines →  no bg,         medium-gray fg
//   separators    →  no bg,         dim gray fg

const ADD_BG: Color = Color::Rgb(0, 40, 0);
const ADD_FG: Color = Color::Rgb(140, 240, 140);
const DEL_BG: Color = Color::Rgb(55, 0, 0);
const DEL_FG: Color = Color::Rgb(240, 140, 140);
const CTX_FG: Color = Color::Rgb(130, 130, 130);
const SEP_FG: Color = Color::Rgb(70, 70, 70);

const DIFF_CONTEXT: usize = 3;
const MAX_WRITE_LINES: usize = 60;

/// Build ratatui Lines for a unified diff of `old` vs `new`.
///
/// Shows up to `DIFF_CONTEXT` unchanged lines around each changed region.
/// Groups are separated by a `⋯` marker. Each line carries a full-width
/// background style so the color band extends to the edge of the widget.
pub fn diff_lines(old: &str, new: &str) -> Vec<Line<'static>> {
    let diff = TextDiff::from_lines(old, new);
    let mut out: Vec<Line<'static>> = Vec::new();

    for (gi, group) in diff.grouped_ops(DIFF_CONTEXT).iter().enumerate() {
        if gi > 0 {
            out.push(Line::from(Span::styled(
                "  \u{22ef}".to_string(), // ⋯
                Style::default().fg(SEP_FG),
            )));
        }
        for op in group {
            for change in diff.iter_changes(op) {
                let text = change.value().trim_end_matches('\n').to_string();
                let line = match change.tag() {
                    ChangeTag::Delete => Line::from(vec![
                        Span::styled(
                            "- ".to_string(),
                            Style::default()
                                .bg(DEL_BG)
                                .fg(DEL_FG)
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(text, Style::default().bg(DEL_BG).fg(DEL_FG)),
                    ])
                    .style(Style::default().bg(DEL_BG)),

                    ChangeTag::Insert => Line::from(vec![
                        Span::styled(
                            "+ ".to_string(),
                            Style::default()
                                .bg(ADD_BG)
                                .fg(ADD_FG)
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(text, Style::default().bg(ADD_BG).fg(ADD_FG)),
                    ])
                    .style(Style::default().bg(ADD_BG)),

                    ChangeTag::Equal => {
                        Line::from(Span::styled(format!("  {text}"), Style::default().fg(CTX_FG)))
                    }
                };
                out.push(line);
            }
        }
    }

    if out.is_empty() {
        out.push(Line::from(Span::styled(
            "  (no changes)".to_string(),
            Style::default().fg(SEP_FG),
        )));
    }

    out
}

/// Build ratatui Lines showing `content` as entirely new (file_write).
///
/// Capped at `MAX_WRITE_LINES`; a `⋯ N more lines` trailer is appended
/// when the file is larger.
pub fn write_lines(content: &str) -> Vec<Line<'static>> {
    let all: Vec<&str> = content.lines().collect();
    let show = all.len().min(MAX_WRITE_LINES);
    let mut out: Vec<Line<'static>> = Vec::with_capacity(show + 1);

    for raw in &all[..show] {
        let text = (*raw).to_string();
        out.push(
            Line::from(vec![
                Span::styled(
                    "+ ".to_string(),
                    Style::default()
                        .bg(ADD_BG)
                        .fg(ADD_FG)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(text, Style::default().bg(ADD_BG).fg(ADD_FG)),
            ])
            .style(Style::default().bg(ADD_BG)),
        );
    }

    if all.len() > MAX_WRITE_LINES {
        out.push(Line::from(Span::styled(
            format!("  \u{22ef} {} more lines", all.len() - MAX_WRITE_LINES),
            Style::default().fg(SEP_FG),
        )));
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_produces_add_and_delete_lines() {
        let lines = diff_lines("foo\nbar\n", "foo\nbaz\n");
        let rendered: Vec<String> = lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();
        assert!(rendered.iter().any(|s| s.contains("- ") && s.contains("bar")));
        assert!(rendered.iter().any(|s| s.contains("+ ") && s.contains("baz")));
    }

    #[test]
    fn diff_no_changes_returns_placeholder() {
        let lines = diff_lines("same\n", "same\n");
        let all: String = lines.iter().flat_map(|l| l.spans.iter().map(|s| s.content.as_ref())).collect();
        assert!(all.contains("no changes"));
    }

    #[test]
    fn write_lines_caps_at_max() {
        let content: String = (0..100).map(|i| format!("line {i}\n")).collect();
        let lines = write_lines(&content);
        // Last line should be the "⋯ N more lines" trailer
        let last: String = lines.last().unwrap().spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(last.contains("more lines"), "expected trailer, got: {last}");
        assert_eq!(lines.len(), MAX_WRITE_LINES + 1);
    }

    #[test]
    fn diff_delete_line_has_red_bg() {
        let lines = diff_lines("old line\n", "new line\n");
        let del_line = lines
            .iter()
            .find(|l| l.spans.first().map(|s| s.content.as_ref()).unwrap_or("") == "- ")
            .expect("should have a delete line");
        assert_eq!(del_line.style.bg, Some(DEL_BG));
    }

    #[test]
    fn diff_insert_line_has_green_bg() {
        let lines = diff_lines("old line\n", "new line\n");
        let ins_line = lines
            .iter()
            .find(|l| l.spans.first().map(|s| s.content.as_ref()).unwrap_or("") == "+ ")
            .expect("should have an insert line");
        assert_eq!(ins_line.style.bg, Some(ADD_BG));
    }
}
