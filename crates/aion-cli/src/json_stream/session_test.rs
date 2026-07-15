use super::content_with_attachment_paths;

#[test]
fn appends_protocol_file_paths_to_model_input() {
    let files = vec!["/tmp/image.png".to_owned(), "/tmp/notes.txt".to_owned()];

    let content = content_with_attachment_paths("inspect these", &files);

    assert_eq!(
        content,
        "inspect these\n\n[Attached files]\n/tmp/image.png\n/tmp/notes.txt"
    );
}

#[test]
fn preserves_text_when_protocol_has_no_files() {
    let content = "  keep surrounding whitespace  ";

    assert_eq!(content_with_attachment_paths(content, &[]), content);
}
