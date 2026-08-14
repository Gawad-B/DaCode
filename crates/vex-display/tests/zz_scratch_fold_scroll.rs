//! Scratch: replicate App::scroll_by (main.rs:1621) against a closed fold.

use vex_display::{DisplayMap, ScrollAnchor, WrapMode};
use vex_text::Text;

fn scroll_by(display: &DisplayMap, text: &Text, anchor: &mut ScrollAnchor, rows: isize) {
    for _ in 0..rows.unsigned_abs() {
        if rows > 0 {
            let in_line = display.rows_in_line(text, anchor.line);
            if anchor.sub_row + 1 < in_line {
                anchor.sub_row += 1;
            } else if anchor.line < text.last_line() {
                anchor.line += 1;
                anchor.sub_row = 0;
            }
        } else if anchor.sub_row > 0 {
            anchor.sub_row -= 1;
        } else if anchor.line > 0 {
            anchor.line -= 1;
            anchor.sub_row = display.rows_in_line(text, anchor.line).saturating_sub(1);
        }
    }
}

#[test]
fn wheel_over_a_closed_fold() {
    let body: String = (0..200)
        .map(|n| format!("line {n}\n"))
        .collect::<Vec<_>>()
        .join("");
    let text = Text::from_str(&body);
    let mut display = DisplayMap::new(WrapMode::None, 80);
    display.folds.close(10..150);

    // Downward from the fold head.
    let mut anchor = ScrollAnchor::at_line(10);
    let mut seen = Vec::new();
    for notch in 0..60 {
        scroll_by(&display, &text, &mut anchor, 3);
        let first = display.rows_from(&text, anchor, 5)[0].line;
        seen.push((notch, anchor.line, first));
    }
    let stalled_down = seen.windows(2).filter(|w| w[0].2 == w[1].2).count();
    println!("down: first 8 = {:?}", &seen[..8]);
    println!("down: stalled notches = {stalled_down}");

    // Upward from just past the fold.
    let mut anchor = ScrollAnchor::at_line(151);
    let mut up = Vec::new();
    for notch in 0..60 {
        scroll_by(&display, &text, &mut anchor, -3);
        let first = display.rows_from(&text, anchor, 5)[0].line;
        up.push((notch, anchor.line, first));
    }
    let stalled_up = up.windows(2).filter(|w| w[0].2 == w[1].2).count();
    println!("up: first 8 = {:?}", &up[..8]);
    println!("up: stalled notches = {stalled_up}");
    panic!("see output");
}
