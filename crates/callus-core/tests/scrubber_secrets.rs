//! Credential categories.

#![allow(clippy::unwrap_used)]

mod common;

use common::*;

#[test]
fn private_key_blocks() {
    for kind in ["RSA ", "EC ", "OPENSSH ", "ENCRYPTED ", ""] {
        let block = format!(
            "-----BEGIN {kind}{k}-----\r\nMIIEowIBAAKCAQEA0Z3VS5JJ\r\nc2VjcmV0\r\n-----END {kind}{k}-----",
            k = "PRIVATE KEY"
        );
        expect(&[(
            &format!("before\n{block}\nafter"),
            "before\n<REDACTED:PRIVATE_KEY>\nafter",
        )]);
    }
    // PGP spelling, two blocks, and a block cut off by output truncation.
    let pgp = concat!(
        "-----BEGIN PGP PRIVATE ",
        "KEY BLOCK-----\nabc\n-----END PGP PRIVATE KEY BLOCK-----"
    );
    expect(&[
        (
            &format!("{pgp} mid {pgp}"),
            "<REDACTED:PRIVATE_KEY> mid <REDACTED:PRIVATE_KEY>",
        ),
        (
            concat!("cat: -----BEGIN ", "PRIVATE KEY-----\nMIIEvQIBADAN\nBgkq"),
            "cat: <REDACTED:PRIVATE_KEY>",
        ),
    ]);
    keep(&[concat!(
        "-----BEGIN ",
        "PUBLIC KEY-----\nMIIBIjAN\n-----END PUBLIC KEY-----"
    )]);
}

#[test]
fn provider_keys() {
    for secret in [
        AWS,
        AWS_TEMP,
        GITHUB,
        GITHUB_FINE,
        GITLAB,
        OPENAI,
        OPENAI_PROJECT,
        ANTHROPIC,
        GOOGLE,
        SLACK,
        STRIPE,
        STRIPE_RESTRICTED,
    ] {
        expect(&[(
            &format!("invalid credential {secret} (401)."),
            "invalid credential <REDACTED:SECRET> (401).",
        )]);
    }
    for prefix in ["gho_", "ghu_", "ghs_", "ghr_"] {
        let token = format!("{prefix}{}", &GITHUB[4..]);
        expect(&[(&token, "<REDACTED:SECRET>")]);
    }
    for kind in ["xoxa", "xoxp", "xoxr", "xoxs"] {
        let token = format!("{kind}{}", &SLACK[4..]);
        expect(&[(&token, "<REDACTED:SECRET>")]);
    }
    keep(&[
        "task-management-system-for-teams failed",
        "the sk- prefix and ghp_ alone are not keys",
    ]);
}

#[test]
fn auth_headers_and_url_credentials() {
    expect(&[
        (
            "Authorization: Bearer abcdefghijklmnop",
            "Authorization: Bearer <REDACTED:AUTH>",
        ),
        (
            "> authorization: Basic YWxpY2U6aHVudGVyMg==",
            "> authorization: Basic <REDACTED:AUTH>",
        ),
        (
            r#"{"Authorization": "Token 9f8e7d6c5b4a"}"#,
            r#"{"Authorization": "Token <REDACTED:AUTH>"}"#,
        ),
        (
            "sent Bearer mF_9.B5f-4.1JqM twice",
            "sent Bearer <REDACTED:AUTH> twice",
        ),
        (
            "fatal: https://alice:s3cr3t@github.com/o/r.git not found",
            "fatal: https://<REDACTED:AUTH>@github.com/o/r.git not found",
        ),
        (
            "redis://:p%40ss@cache.example.com:6379/0",
            "redis://<REDACTED:AUTH>@cache.example.com:6379/0",
        ),
        (
            "git clone https://x-access-token:p@ss@w0rd@example.com/r",
            "git clone https://<REDACTED:AUTH>@example.com/r",
        ),
    ]);
    keep(&[
        "Basic authentication is not supported",
        "missing bearer token",
        "https://example.com/users/@scope/pkg",
        "https://example.com:8080/path?q=a",
    ]);
}

#[test]
fn json_web_tokens() {
    expect(&[
        (
            &format!("jwt expired: {JWT}."),
            "jwt expired: <REDACTED:JWT>.",
        ),
        // An unsigned token has an empty signature.
        (
            concat!("got eyJ", "hbGciOiJub25lIn0.eyJzdWIiOiIxIn0. ok"),
            "got <REDACTED:JWT> ok",
        ),
        // The header rule runs first.
        (
            &format!("Authorization: Bearer {JWT}"),
            "Authorization: Bearer <REDACTED:AUTH>",
        ),
    ]);
}

#[test]
fn assignments_keep_the_key_name() {
    for key in [
        "API_KEY",
        "SECRET",
        "TOKEN",
        "PASSWORD",
        "PASSWD",
        "PRIVATE_KEY",
        "ACCESS_KEY",
        "CLIENT_SECRET",
        "AUTH_TOKEN",
        "DATABASE_URL",
        "AWS_SECRET_ACCESS_KEY",
        "npm_config_token",
    ] {
        expect(&[
            (
                &format!("{key}=hunter2value"),
                &format!("{key}=<REDACTED:ENV_SECRET>"),
            ),
            (
                &format!("export {key}='two words'"),
                &format!("export {key}='<REDACTED:ENV_SECRET>'"),
            ),
            (
                &format!("$env:{key} = \"correct horse\"; ./run"),
                &format!("$env:{key} = \"<REDACTED:ENV_SECRET>\"; ./run"),
            ),
        ]);
    }
}

#[test]
fn assignment_spellings() {
    expect(&[
        // Letters-only values: quoted literals and compound key names.
        (
            r#"apiKey: "abcdefgh""#,
            r#"apiKey: "<REDACTED:ENV_SECRET>""#,
        ),
        (
            r#"  token: "abcdefgh""#,
            r#"  token: "<REDACTED:ENV_SECRET>""#,
        ),
        ("secret_key: abcdefgh", "secret_key: <REDACTED:ENV_SECRET>"),
        (
            "postgres://admin:s3cr#t/x?y@db.example.com/app",
            "postgres://<REDACTED:AUTH>@db.example.com/app",
        ),
        // JSON that was encoded into a string once more.
        (
            r#"{\"password\":\"abc\",\"Authorization\": \"Bearer abcdef123\"}"#,
            r#"{\"password\":\"<REDACTED:ENV_SECRET>\",\"Authorization\": \"Bearer <REDACTED:AUTH>\"}"#,
        ),
        (
            r#"{"password": "letmein", "user": "x"}"#,
            r#"{"password": "<REDACTED:ENV_SECRET>", "user": "x"}"#,
        ),
        (
            "{'client_secret': 'abcdef'}",
            "{'client_secret': '<REDACTED:ENV_SECRET>'}",
        ),
        ("  password: hunter2", "  password: <REDACTED:ENV_SECRET>"),
        ("API_KEY: abcdefgh", "API_KEY: <REDACTED:ENV_SECRET>"),
        ("x-api-key: 0a1b2c3d4e", "x-api-key: <REDACTED:ENV_SECRET>"),
        (
            "Server=db;Password=Sup3r;Timeout=5",
            "Server=db;Password=<REDACTED:ENV_SECRET>",
        ),
        ("PASSWORD=-dash-first", "PASSWORD=<REDACTED:ENV_SECRET>"),
        ("TOKEN=`backticked`", "TOKEN=`<REDACTED:ENV_SECRET>`"),
        ("TOKEN=\"unterminated", "TOKEN=\"<REDACTED:ENV_SECRET>"),
        ("'token' => 'abc'", "'token' => '<REDACTED:ENV_SECRET>'"),
        (
            "DATABASE_URL=postgres://u:p@db.example.com/app",
            "DATABASE_URL=<REDACTED:ENV_SECRET>",
        ),
        // A known secret followed by more of the value hides the whole value.
        (
            &format!("TOKEN={GITHUB},tail"),
            "TOKEN=<REDACTED:ENV_SECRET>",
        ),
        (&format!("TOKEN={GITHUB}"), "TOKEN=<REDACTED:SECRET>"),
    ]);
}

#[test]
fn command_line_flags() {
    expect(&[
        (
            "cli --token abc123 --verbose",
            "cli --token <REDACTED:ENV_SECRET> --verbose",
        ),
        (
            "cli --password=\"a b\" x",
            "cli --password=\"<REDACTED:ENV_SECRET>\" x",
        ),
        (
            "cli --api-key 'k' --client-secret s3",
            "cli --api-key '<REDACTED:ENV_SECRET>' --client-secret <REDACTED:ENV_SECRET>",
        ),
        (
            "Connect-Thing -Password hunter2",
            "Connect-Thing -Password <REDACTED:ENV_SECRET>",
        ),
    ]);
    keep(&[
        "cli --token --verbose",
        "kubectl get my-secret -n default",
        "cli --token-file ./t",
    ]);
}

#[test]
fn high_entropy_strings_next_to_keywords() {
    expect(&[
        (
            &format!("invalid token {RANDOM} supplied"),
            "invalid token <REDACTED:HIGH_ENTROPY> supplied",
        ),
        (
            &format!("auth failed for [{RANDOM}]"),
            "auth failed for [<REDACTED:HIGH_ENTROPY>]",
        ),
        (
            &format!("{RANDOM} is not a valid key"),
            "<REDACTED:HIGH_ENTROPY> is not a valid key",
        ),
        (
            concat!(
                "credential wJalrXUtnFEMI/",
                "K7MDENG/bPxRfiCYEXAMPLEKEY rejected"
            ),
            "credential <REDACTED:HIGH_ENTROPY> rejected",
        ),
        // Letters only, but far too irregular for an identifier.
        (
            concat!("secret qZrLmXvTbNcK", "dPwHsJfGyAeUiO"),
            "secret <REDACTED:HIGH_ENTROPY>",
        ),
    ]);
    keep(&[
        // No keyword nearby, or only on another line.
        &format!("commit {RANDOM} not found"),
        &format!("token\n{RANDOM}"),
        // Low entropy, short, or a plain identifier.
        "token aaaaaaaaaaaaaaaaaaaaaaaa1",
        "token q8Zr2Lm9Xv4Tb7N",
        "auth error: InvalidAuthenticationTokenException",
        "key error in get_user_authentication_settings",
    ]);
}
