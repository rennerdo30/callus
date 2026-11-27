//! Paths, addresses, hosts, deny rules and the API surface.

#![allow(clippy::unwrap_used)]

mod common;

use callus_core::*;
use common::*;

#[test]
fn repo_root_in_every_spelling() {
    expect(&[
        (r"C:\Users\alice\code\widget\src\a.rs", r"<REPO>\src\a.rs"),
        ("c:/users/ALICE/code/widget/src/a.rs", "<REPO>/src/a.rs"),
        (r"C:\\Users\\alice\\code\\widget\\a", r"<REPO>\\a"),
        ("/c/Users/alice/code/widget/a", "<REPO>/a"),
        ("/mnt/c/Users/alice/code/widget/a", "<REPO>/a"),
        (r"in C:\Users\alice\code\widget.", "in <REPO>."),
        // A sibling directory only shares the prefix.
        (
            r"C:\Users\alice\code\widget-old\a",
            r"<HOME>\code\widget-old\a",
        ),
    ]);
    let unix = Scrubber::new(&ScrubberConfig {
        repo_root: Some("/srv/ci/widget/".to_owned()),
        ..ScrubberConfig::default()
    })
    .unwrap();
    assert_eq!(
        unix.scrub_text("/srv/ci/widget/a.py").scrubbed,
        "<REPO>/a.py"
    );
    assert_eq!(unix.scrub_text(r"\srv\ci\widget\a").scrubbed, r"<REPO>\a");
    assert_eq!(
        unix.scrub_text("/srv/ci/widgets").scrubbed,
        "/srv/ci/widgets"
    );
}

#[test]
fn filesystem_roots_are_ignored_as_repo_root() {
    for root in ["", "/", r"C:\", "C:", "relative"] {
        let scrubber = Scrubber::new(&ScrubberConfig {
            repo_root: Some(root.to_owned()),
            ..ScrubberConfig::default()
        })
        .unwrap();
        let text = r"C:\work\relative /etc/hosts";
        assert_eq!(scrubber.scrub_text(text).scrubbed, text, "root {root:?}");
    }
}

#[test]
fn user_home_paths_keep_the_tail() {
    expect(&[
        (r"C:\Users\alice\.cargo\bin", r"<HOME>\.cargo\bin"),
        (r"C:\Users\bob\x.txt", r"<HOME>\x.txt"),
        ("D:/Users/bob/x.txt", "<HOME>/x.txt"),
        (r"c:\users\Bob.Smith-2\x", r"<HOME>\x"),
        (r"C:\Users\Jane Doe\x", r"<HOME>\x"),
        ("at C:\\Users\\Jane Doe\r\nnext", "at <HOME>\r\nnext"),
        ("home is /Users/Jane Doe", "home is <HOME>"),
        (r"dir 'C:\Users\Jane Doe' denied", "dir '<HOME>' denied"),
        (r"C:\Users\O'Brien\x", r"<HOME>\x"),
        (r"C:\Users\JANEDO~1\x", r"<HOME>\x"),
        (r"path 'C:\Users\bob' missing", "path '<HOME>' missing"),
        (r"C:\\Users\\bob\\x", r"<HOME>\\x"),
        (r"\\?\C:\Users\bob\x", r"\\?\<HOME>\x"),
        ("/mnt/c/Users/bob/x", "<HOME>/x"),
        ("/cygdrive/c/Users/bob/x", "<HOME>/x"),
        ("/Users/bob/Library/x", "<HOME>/Library/x"),
        (
            "/System/Volumes/Data/Users/bob/x",
            "/System/Volumes/Data<HOME>/x",
        ),
        ("/home/bob/.config/x", "<HOME>/.config/x"),
        ("file:///home/bob/x", "file://<HOME>/x"),
        ("cd /home/bob: denied", "cd <HOME>: denied"),
        ("cd /home/bob and retry", "cd <HOME> and retry"),
        ("I am in /home/bob.", "I am in <HOME>."),
    ]);
    let custom = Scrubber::new(&ScrubberConfig {
        home_dir: Some("/var/lib/build agent".to_owned()),
        ..ScrubberConfig::default()
    })
    .unwrap();
    assert_eq!(
        custom.scrub_text("/var/lib/build agent/.npmrc").scrubbed,
        "<HOME>/.npmrc"
    );
    keep(&[
        "/usr/lib/python3/site.py",
        r"C:\Program Files\Git\bin",
        "/homebrew/bin",
    ]);
}

#[test]
fn email_addresses() {
    expect(&[
        (
            "Author: A <alice.o+ci@sub.example.co.uk>",
            "Author: A <<REDACTED:EMAIL>>",
        ),
        ("mail bob_1@example.com.", "mail <REDACTED:EMAIL>."),
    ]);
    keep(&[
        "npm ERR! react@18.3.1 and pkg@1.0.0-beta.rc",
        "assets/logo@2x.png missing",
        "node_modules/@types/node",
        "actions/checkout@v7",
    ]);
}

#[test]
fn private_addresses_only() {
    for ip in [
        "10.0.0.1",
        "10.255.255.255",
        "172.16.0.1",
        "172.31.9.9",
        "192.168.1.10",
        "169.254.1.1",
        "100.64.0.1",
        "100.127.255.255",
        "fc00::1",
        "fd12:3456:789a::1",
        "fe80::1ff:fe23:4567:890a",
        "fe80::1%eth0",
        "FD00:0:0:0:0:0:0:1",
    ] {
        expect(&[
            (
                &format!("connect {ip} refused"),
                "connect <REDACTED:IP> refused",
            ),
            (&format!("host={ip}, retry"), "host=<REDACTED:IP>, retry"),
        ]);
    }
    expect(&[
        ("tcp 10.1.2.3:5432.", "tcp <REDACTED:IP>:5432."),
        ("http://[fd00::5]:8080/", "http://[<REDACTED:IP>]:8080/"),
        ("dial fd00::5: timeout", "dial <REDACTED:IP>: timeout"),
        ("::ffff:192.168.0.1", "::ffff:<REDACTED:IP>"),
    ]);
    keep(&[
        "127.0.0.1:8080 and ::1 and [::1]:80 and 0.0.0.0",
        "8.8.8.8 172.15.0.1 172.32.0.1 100.63.0.1 100.128.0.1 192.169.0.1",
        "2001:db8::1 and 2606:4700::1111",
        "version 10.1.2.3.4 and 1.10.0.0.1",
        "std::fs::read at 12:30:45, ratio 16:9",
        "error[E0433]: use of undeclared `Foo::bar`",
    ]);
}

#[test]
fn internal_and_configured_hosts() {
    expect(&[
        ("ssh nas.local failed", "ssh <REDACTED:HOST> failed"),
        ("db.prod.internal:5432", "<REDACTED:HOST>:5432"),
        (
            "https://git.corp/x http://a.lan. printer.home.arpa",
            "https://<REDACTED:HOST>/x http://<REDACTED:HOST>. <REDACTED:HOST>",
        ),
        ("ping BuildBox: down", "ping <REDACTED:HOST>: down"),
        ("ci.buildbox.example.com", "<REDACTED:HOST>"),
    ]);
    keep(&[
        "localhost and foo.localhost",
        "cp .env.local .env && cat settings.local.json",
        "AttributeError: threading.local",
        "the buildboxes and my-buildbox-2",
        "a.internal.example.com",
    ]);
}

#[test]
fn custom_deny_rules() {
    expect(&[(
        "Project Falcon and project-falcon failed",
        "<REDACTED:CUSTOM> and <REDACTED:CUSTOM> failed",
    )]);
    let greedy = |pattern: &str, text: &str| {
        let scrubber = Scrubber::new(&ScrubberConfig {
            deny_regexes: vec![pattern.to_owned()],
            ..ScrubberConfig::default()
        })
        .unwrap();
        let once = scrubber.scrub_text(text).scrubbed;
        assert_eq!(scrubber.scrub_text(&once).scrubbed, once, "{pattern}");
        once
    };
    // Rules that would match placeholders or nothing must still terminate.
    assert_eq!(greedy("SECRET", GITHUB), "<REDACTED:SECRET>");
    assert_eq!(greedy("x*", "abc"), "abc");
    assert_eq!(greedy(".+", "abc /home/bob"), "<REDACTED:CUSTOM>");
    assert_eq!(
        greedy("<|>", "10.0.0.1 a<b"),
        "<REDACTED:IP> a<REDACTED:CUSTOM>b"
    );
    // A rule enabled by an earlier redaction applies in the same call.
    assert_eq!(
        greedy(r"\bfoo", "bob@example.comfoo foo"),
        "<REDACTED:EMAIL> <REDACTED:CUSTOM>"
    );
    // Deny rules cannot weaken built-in detection.
    assert_eq!(greedy("ghp_", GITHUB), "<REDACTED:SECRET>");
    assert_eq!(greedy("Users", r"C:\Users\bob"), "<HOME>");
}

#[test]
fn invalid_deny_regex_is_an_error() {
    let result = Scrubber::new(&ScrubberConfig {
        deny_regexes: vec!["(".to_owned()],
        ..ScrubberConfig::default()
    });
    assert!(matches!(result, Err(CallusError::Pattern(_))));
}

#[test]
fn hits_hold_categories_and_counts_only() {
    let result = scrubber().scrub_text(&format!(
        "{GITHUB} {AWS} a@example.com 10.0.0.1 PASSWORD=zzz9zzz"
    ));
    let hit = |category, count| RedactionHit { category, count };
    assert_eq!(
        result.redactions,
        [
            hit(RedactionCategory::Secret, 2),
            hit(RedactionCategory::EnvSecret, 1),
            hit(RedactionCategory::Email, 1),
            hit(RedactionCategory::Ip, 1),
        ]
    );
    let json = serde_json::to_string(&result).unwrap();
    assert!(json.contains(r#"{"category":"env_secret","count":1}"#));
    for leaked in [GITHUB, AWS, "a@example.com", "10.0.0.1", "zzz9zzz"] {
        assert!(!json.contains(leaked));
        assert!(!format!("{result:?}").contains(leaked));
    }
}

#[test]
fn events_are_scrubbed_in_every_free_form_field() {
    let mut event = ToolEvent {
        event_id: "01J".to_owned(),
        timestamp: "2026-11-27T13:00:00Z".to_owned(),
        cli: CliKind::Codex,
        cli_version: Some("1.0 /home/bob/bin".to_owned()),
        model: Some("model on nas.local".to_owned()),
        session_id: "hashed".to_owned(),
        os: OsKind::Linux,
        shell: ShellKind::Bash,
        repo_hash: Some("abc".to_owned()),
        tool_name: "mcp:buildbox".to_owned(),
        phase: ToolPhase::PostFailure,
        command_or_input: Some(format!("curl -H 'Authorization: Bearer {JWT}' 10.0.0.1")),
        exit_code: Some(7),
        error_text: Some("curl: (7) /home/bob/x on 10.0.0.1".to_owned()),
        duration_ms: Some(3),
    };
    let hits = scrubber().scrub_event(&mut event);
    assert_eq!(event.tool_name, "mcp:<REDACTED:HOST>");
    assert_eq!(event.cli_version.as_deref(), Some("1.0 <HOME>/bin"));
    assert_eq!(event.model.as_deref(), Some("model on <REDACTED:HOST>"));
    assert_eq!(
        event.command_or_input.as_deref(),
        Some("curl -H 'Authorization: Bearer <REDACTED:AUTH>' <REDACTED:IP>")
    );
    assert_eq!(
        event.error_text.as_deref(),
        Some("curl: (7) <HOME>/x on <REDACTED:IP>")
    );
    let ips = hits.iter().find(|h| h.category == RedactionCategory::Ip);
    assert_eq!(ips.map(|h| h.count), Some(2));
    assert_eq!(count(&scrubber(), "nothing here", RedactionCategory::Ip), 0);
    assert_eq!(scrubber().scrub_event(&mut event), []);
}
