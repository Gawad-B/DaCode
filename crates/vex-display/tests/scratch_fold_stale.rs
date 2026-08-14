use vex_display::{DisplayMap, ScrollAnchor, WrapMode};
use vex_text::Text;

fn doc(lines: usize) -> Text {
    let mut s = String::new();
    for i in 0..lines {
        s.push_str(&format!("line{i}\n"));
    }
    Text::from_str(&s)
}

#[test]
fn stale_fold_hides_the_tail_of_a_shrunk_document() {
    let text = doc(12); // lines 0..=11 plus a trailing empty line 12
    let mut map = DisplayMap {
        mode: WrapMode::None,
        ..Default::default()
    };
    // A fold closed when the document was 200 lines long.
    map.folds.close(5..100);

    let rows = map.rows_from(&text, ScrollAnchor::default(), 40);
    let lines: Vec<usize> = rows.iter().map(|r| r.line).collect();
    println!("last_line = {}", text.last_line());
    println!("rows from 0: {lines:?}");
    println!("folded count on head: {}", rows.last().unwrap().folded);

    let anchor = ScrollAnchor {
        line: 6,
        sub_row: 0,
        column: 0,
    };
    let rows2 = map.rows_from(&text, anchor, 40);
    let lines2: Vec<(usize, usize, usize)> = rows2
        .iter()
        .map(|r| (r.line, r.range.start.get(), r.range.end.get()))
        .collect();
    println!("rows from anchor line 6: {lines2:?}");
}
