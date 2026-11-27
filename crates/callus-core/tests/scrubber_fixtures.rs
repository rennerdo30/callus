//! Realistic multi-line tool output: nothing sensitive may survive.

#![allow(clippy::unwrap_used)]

mod common;

use common::*;

fn windows_pwsh() -> String {
    format!(
        r#"PS C:\Users\alice\code\widget> $env:OPENAI_API_KEY="{OPENAI}"; .\deploy.ps1 -Password S3cr3t!pw
Invoke-RestMethod: C:\Users\alice\code\widget\deploy.ps1:14:5
Line |
  14 |  Invoke-RestMethod -Uri $uri -Headers @{{Authorization="Bearer {JWT}"}}
     |  ~~~~~~~~~~~~~~~~~
     | Unable to connect to buildbox (192.168.4.20:8443)
VERBOSE: key file C:\Users\alice\.ssh\id_ed25519 for alice.smith@contoso.example
VERBOSE: profile c:/Users/Alice/Documents/PowerShell/profile.ps1, cache \\fileserver.corp\share
    at <ScriptBlock>, C:\Users\Jane Doe\AppData\Local\Temp\run.ps1: line 3
WARNING: Project Falcon uses {GITHUB} and {AWS}
"#
    )
}

fn linux_bash() -> String {
    format!(
        r"$ DATABASE_URL=postgresql://app:Pa55word@10.20.0.5:5432/app ./migrate --token {GITLAB}
./migrate: line 22: /home/deploy/.config/widget/env: Permission denied
fatal: unable to access 'https://oauth2:{GITHUB_FINE}@git.internal/team/widget.git/': Could not resolve host: git.internal
curl: (7) Failed to connect to fd12:3456::7 port 443 after 3 ms
export SLACK_TOKEN={SLACK}
declare -x STRIPE_KEY='{STRIPE}'
+ gcloud auth --key {GOOGLE}
ssh: connect to host nas.lan port 22: root@169.254.7.7 timed out
{pem}
",
        pem = concat!(
            "-----BEGIN OPENSSH ",
            "PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEAAAAA\n-----END OPENSSH PRIVATE KEY-----"
        )
    )
}

fn macos_zsh() -> String {
    format!(
        r#"zsh: no matches found: /Users/carol/Projects/widget/*.log
xcodebuild: error: '/Users/carol/Library/Developer/Xcode/DerivedData' on carols-mbp.local
npm ERR! //registry.npmjs.org/:_authToken={RANDOM}
anthropic.AuthenticationError: invalid x-api-key: {ANTHROPIC}
{{"client_secret": "zzTop-9000", "refresh_token": "{RANDOM}", "owner": "carol@icloud.example"}}
sudo: 100.101.102.103: temporary AWS_SECRET_ACCESS_KEY=wJalrXUtnFEMI/K7MDENG id {AWS_TEMP} {STRIPE_RESTRICTED} {OPENAI_PROJECT}
"#
    )
}

const SENSITIVE: &[&str] = &[
    OPENAI,
    OPENAI_PROJECT,
    ANTHROPIC,
    JWT,
    GITHUB,
    GITHUB_FINE,
    GITLAB,
    GOOGLE,
    SLACK,
    STRIPE,
    STRIPE_RESTRICTED,
    AWS,
    AWS_TEMP,
    RANDOM,
    "alice",
    "Alice",
    "Jane",
    "Doe",
    "deploy/",
    "carol",
    "contoso",
    "icloud",
    "S3cr3t!pw",
    "Pa55word",
    "zzTop-9000",
    "wJalrXUtnFEMI",
    "b3BlbnNzaC1rZXk",
    "buildbox",
    "fileserver",
    "git.internal",
    "nas.lan",
    "mbp.local",
    "192.168.4.20",
    "10.20.0.5",
    "fd12:3456",
    "169.254.7.7",
    "100.101.102.103",
    "Falcon",
];

#[test]
fn no_secret_or_identity_survives() {
    let scrubber = scrubber();
    for fixture in [windows_pwsh(), linux_bash(), macos_zsh()] {
        for text in [fixture.clone(), fixture.replace('\n', "\r\n")] {
            let once = scrubber.scrub_text(&text).scrubbed;
            for secret in SENSITIVE {
                assert!(!once.contains(secret), "leaked {secret:?} in:\n{once}");
            }
            assert!(once.lines().count() <= text.lines().count());
            let twice = scrubber.scrub_text(&once);
            assert_eq!(twice.scrubbed, once);
            assert_eq!(twice.redactions, []);
        }
    }
}

#[test]
fn fixtures_stay_useful_for_diagnosis() {
    let windows = scrub(&windows_pwsh());
    for kept in [
        r"Invoke-RestMethod: <REPO>\deploy.ps1:14:5",
        "Unable to connect to <REDACTED:HOST> (<REDACTED:IP>:8443)",
        r"<HOME>\.ssh\id_ed25519",
        r"<HOME>\AppData\Local\Temp\run.ps1: line 3",
        "$env:OPENAI_API_KEY=\"<REDACTED:",
    ] {
        assert!(windows.contains(kept), "missing {kept:?} in:\n{windows}");
    }
    let linux = scrub(&linux_bash());
    for kept in [
        "./migrate: line 22: <HOME>/.config/widget/env: Permission denied",
        "Could not resolve host: <REDACTED:HOST>",
        "port 443 after 3 ms",
        "DATABASE_URL=<REDACTED:ENV_SECRET> ./migrate --token <REDACTED:",
    ] {
        assert!(linux.contains(kept), "missing {kept:?} in:\n{linux}");
    }
    let macos = scrub(&macos_zsh());
    assert!(macos.contains("zsh: no matches found: <HOME>/Projects/widget/*.log"));
}

#[test]
fn ordinary_error_messages_are_untouched() {
    keep(&[
        "ParserError: \nLine |\n   2 |  python - << 'EOF'\n     |             ~\n     | Missing file specification after redirection operator.",
        "ParserError: The '<' operator is reserved for future use.",
        "At line:1 char:10\n+ python - <<'EOF'\n+          ~\nThe '<' operator is reserved for future use.\n    + FullyQualifiedErrorId : RedirectionNotSupported",
        "The token '&&' is not a valid statement separator in this version.",
        "SyntaxError: Unexpected token '}'",
        "SyntaxError: Unexpected token: '}'",
        "SyntaxError: Unexpected token: 'return'",
        "GET https://localhost:8080/@scope/pkg failed",
        "https://example.com:443/a?mail=x",
        "error: expected token: `;`",
        "TS1005: token: string expected",
        "error[E0425]: cannot find value `token` in this scope\n --> src/auth/token.rs:12:9",
        "if token == null { return Err(Token::Missing) }",
        "KeyError: 'password'",
        "max_tokens=4096 exceeds the limit; max_tokens: 4096",
        "bash: passwd: command not found",
        "git: 'comit' is not a git command. See 'git --help'.",
        "ModuleNotFoundError: No module named 'requests'",
        "Segmentation fault (core dumped) at 0x7ffd1234abcd",
        "zsh: no matches found: *.log",
        "E: Unable to locate package libssl-dev=3.0.2-0ubuntu1.18",
        "sha256:9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08",
    ]);
}

#[test]
fn large_input_is_handled() {
    let big = format!("{}{GITHUB}\n", "plain build output line 12\n".repeat(4000));
    let result = scrub(&big);
    assert!(result.ends_with("<REDACTED:SECRET>\n"));
}
