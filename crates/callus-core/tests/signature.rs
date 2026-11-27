use callus_core::{compute_signature_hash, normalize_error_template};

fn same(first: &str, second: &str) {
    assert_eq!(
        normalize_error_template(first),
        normalize_error_template(second),
        "{first:?} vs {second:?}"
    );
}

fn differ(first: &str, second: &str) {
    assert_ne!(
        normalize_error_template(first),
        normalize_error_template(second),
        "{first:?} vs {second:?}"
    );
}

#[test]
fn line_and_column_numbers_are_volatile() {
    same("error at line 3 column 7", "error at line 812 column 1");
    same("At <HOME>\\x.ps1:3 char:5", "At <HOME>\\x.ps1:40 char:19");
    same("--> src/main.rs:3:5", "--> src/main.rs:120:33");
    same(
        "index.ts(12,5): error TS2304",
        "index.ts(1,99): error TS2304",
    );
    same("File \"app.py\", line 3", "File \"app.py\", line 77");
    same("Line |\n   3 |  python", "Line |\n  41 |  python");
}

#[test]
fn addresses_ids_times_pids_and_ports_are_volatile() {
    same("segfault at 0x7ffd1234abcd", "segfault at 0x000055d0c0de");
    same(
        "job 550e8400-e29b-41d4-a716-446655440000 failed",
        "job 123E4567-E89B-12D3-A456-426614174000 failed",
    );
    same(
        "event 01ARZ3NDEKTSV4RRFFQ69G5FAV lost",
        "event 01BX5ZZKBKACTAV9WEVGEMMVRZ lost",
    );
    same(
        "2026-11-27T13:00:00Z ERROR boom",
        "2025-01-02 04:05:06.789+09:00 ERROR boom",
    );
    same("[13:00:00] boom", "[04:05:06.123] boom");
    same("kill pid 4312 failed", "kill pid 7 failed");
    same("process id: 88 exited", "process id: 90210 exited");
    same("listen on port 3000 failed", "listen on port 54321 failed");
    same(
        "ECONNREFUSED 127.0.0.1:5432",
        "ECONNREFUSED 127.0.0.1:61000",
    );
    same("dial localhost:80: refused", "dial localhost:8080: refused");
}

#[test]
fn directories_are_machine_specific_but_file_names_are_not() {
    same(
        "cannot open C:\\Users\\a\\proj\\main.rs",
        "cannot open D:/work/main.rs",
    );
    same("cannot open <HOME>/x/main.rs", "cannot open /srv/main.rs");
    differ("cannot open /srv/main.rs", "cannot open /srv/lib.rs");
}

#[test]
fn meaningful_differences_survive() {
    differ("exit code 1", "exit code 127");
    differ("HTTP 404", "HTTP 500");
    differ("error 0x80070005", "error 0x80070002");
    differ("exit status 0xC0000005", "exit status 0xC0000409");
    differ("TS2304: not found", "TS2552: not found");
}

#[test]
fn whitespace_collapses_and_colours_vanish() {
    assert_eq!(
        normalize_error_template("  \x1b[31;1mParserError:\x1b[0m \r\n\t The  '<'  "),
        "ParserError: The '<'"
    );
    assert_eq!(normalize_error_template(""), "");
}

#[test]
fn normalization_is_stable() {
    let raw = "2026-11-27T13:00:00Z pid 4 C:\\a\\b.rs:3:5 at 0xdeadbeef port 80";
    let once = normalize_error_template(raw);
    assert_eq!(
        once,
        "<TIME> pid <PID> <PATH>/b.rs:<N>:<N> at <HEX> port <PORT>"
    );
    assert_eq!(normalize_error_template(&once), once);
}

#[test]
fn hash_is_pinned_sha256_over_tool_and_template() {
    // printf 'bash\0boom' | sha256sum
    assert_eq!(
        compute_signature_hash("bash", "boom"),
        "8693bf8565f7837eb51859334aca75862a840cb8086edc44469fa4d95072191a"
    );
    assert_ne!(
        compute_signature_hash("bash", "boom"),
        compute_signature_hash("edit", "boom")
    );
    // The separator keeps the two inputs apart.
    assert_ne!(
        compute_signature_hash("ab", "c"),
        compute_signature_hash("a", "bc")
    );
}
