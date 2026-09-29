//! Folding (OV-00455). Expectations for manual folds were cross-checked
//! against `nvim --headless` (`2Gzf3j` creates a closed fold over lines 2-5).

mod helpers;

use helpers::EditorTest;

fn ten_lines() -> EditorTest {
    let text: String = (1..=10).map(|n| format!("line {n}\n")).collect();
    EditorTest::new(&text)
}

fn hidden(test: &EditorTest) -> Vec<usize> {
    (0..test.line_count())
        .filter(|&line| test.editor.buffer().is_line_folded(line))
        .collect()
}

#[test]
fn zf_creates_a_closed_fold_that_hides_its_body() {
    let mut test = ten_lines();
    test.keys("2Gzf3j");
    // Lines 2-5 (1-based): header stays, 3..=5 hidden (0-based 2..=4).
    assert_eq!(hidden(&test), vec![2, 3, 4]);
    test.assert_cursor(1, 0);
}

#[test]
fn j_and_k_treat_a_closed_fold_as_one_line() {
    let mut test = ten_lines();
    test.keys("2Gzf3jgg");
    test.keys("j");
    test.assert_cursor(1, 0); // onto the fold header (line 2)
    test.keys("j");
    test.assert_cursor(5, 0); // over the fold to line 6
    test.keys("k");
    test.assert_cursor(1, 0);
    test.keys("gg3j");
    test.assert_cursor(6, 0); // 1 -> fold -> 6 -> 7, like Vim's `3j`
}

#[test]
fn zo_zc_za_open_and_close_the_fold_under_the_cursor() {
    let mut test = ten_lines();
    test.keys("2Gzf3j");
    test.keys("zo");
    assert!(hidden(&test).is_empty());
    test.keys("4G");
    test.keys("zc");
    assert_eq!(hidden(&test), vec![2, 3, 4]);
    test.assert_cursor(1, 0); // cursor shown on the header
    test.keys("za");
    assert!(hidden(&test).is_empty());
    test.keys("za");
    assert_eq!(hidden(&test), vec![2, 3, 4]);
}

#[test]
fn zr_zm_and_ze_act_on_all_folds() {
    let mut test = ten_lines();
    test.keys("2Gzf3j7Gzf2j");
    assert_eq!(hidden(&test), vec![2, 3, 4, 7, 8]);
    test.keys("zR");
    assert!(hidden(&test).is_empty());
    test.keys("zM");
    assert_eq!(hidden(&test), vec![2, 3, 4, 7, 8]);
    test.keys("zE");
    assert!(hidden(&test).is_empty());
}

#[test]
fn dd_on_a_closed_fold_deletes_the_whole_fold() {
    let mut test = ten_lines();
    test.keys("2Gzf3jdd");
    test.assert_line_count(6);
    assert_eq!(test.line_text(0).as_deref(), Some("line 1"));
    assert_eq!(test.line_text(1).as_deref(), Some("line 6"));
    test.assert_cursor(1, 0);
    // The deleted text is one linewise register of four lines.
    let register = test.get_register_content('"').unwrap();
    assert_eq!(register.lines().count(), 4);
    // Undo brings all of it back.
    test.keys("u");
    test.assert_line_count(10);
}

#[test]
fn moving_horizontally_onto_a_closed_header_opens_it() {
    let mut test = ten_lines();
    test.keys("2Gzf3j");
    test.keys("l");
    assert!(hidden(&test).is_empty());
}

#[test]
fn zj_and_zk_jump_between_folds() {
    let mut test = ten_lines();
    test.keys("2Gzf3j7Gzf2jzRgg");
    test.keys("zj");
    test.assert_cursor(1, 0);
    test.keys("zj");
    test.assert_cursor(6, 0);
    test.keys("zk");
    test.assert_cursor(4, 0); // end of the first fold
}

#[test]
fn closed_folds_take_no_visual_rows_in_the_wrap_map() {
    let mut test = ten_lines();
    test.editor.options.wrap = true;
    test.editor.ensure_wrap_map(80);
    let before = test.editor.wrap_map().unwrap().total_visual_lines();
    test.keys("2Gzf3j");
    test.editor.ensure_wrap_map(80);
    let map = test.editor.wrap_map().unwrap();
    assert_eq!(map.total_visual_lines(), before - 3);
    // Visual row 2 is line 6 (index 5): the fold body is skipped.
    assert_eq!(map.visual_to_logical(2).0, 5);
    test.keys("zo");
    test.editor.ensure_wrap_map(80);
    assert_eq!(test.editor.wrap_map().unwrap().total_visual_lines(), before);
}

#[test]
fn lsp_folding_ranges_replace_indentation_folds_and_keep_state() {
    let mut test = EditorTest::new("a {\n  b {\n    c\n  }\n}\nd\n");
    test.editor
        .buffer_mut()
        .set_file_path("/tmp/fold_test.txt".to_string());
    test.keys("zM");
    // Indentation folds: headers `a {` (0..3) and `b {` (1..2).
    assert!(test.editor.buffer().is_line_folded(1));
    let version = test.editor.buffer().version();
    let range = |start, end| lsp_types::FoldingRange {
        start_line: start,
        end_line: end,
        start_character: None,
        end_character: None,
        kind: None,
        collapsed_text: None,
    };
    let applied = test.editor.apply_lsp_folding_ranges(
        "/tmp/fold_test.txt",
        version,
        &[range(0, 3), range(1, 2)],
    );
    assert!(applied);
    // `a {` stayed closed across the recompute (same header line).
    assert!(test.editor.buffer().is_line_folded(1));
    // Stale answers are ignored.
    assert!(!test.editor.apply_lsp_folding_ranges(
        "/tmp/fold_test.txt",
        version + 5,
        &[range(0, 1)]
    ));
}
