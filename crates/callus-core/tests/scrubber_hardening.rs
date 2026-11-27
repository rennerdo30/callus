#![allow(clippy::unwrap_used)]

use callus_core::{Scrubber, ScrubberConfig};

#[test]
fn redacts_pass_pwd_xml_secrets_and_encoded_user_slugs() {
    let scrubber = Scrubber::new(&ScrubberConfig {
        extra_usernames: vec!["jdoe_admin".to_owned()],
        ..ScrubberConfig::default()
    })
    .unwrap();

    // *_PASS and *_PWD keys are redacted while `bypass=true` and `pwd=/tmp` survive.
    let env_res = scrubber.scrub_text("DB_PASS=s3cr3t! REDIS_PWD=hunter2 bypass=true pwd=/tmp");
    assert_eq!(
        env_res.scrubbed,
        "DB_PASS=<REDACTED:ENV_SECRET> REDIS_PWD=<REDACTED:ENV_SECRET> bypass=true pwd=/tmp"
    );

    // XML config secret elements are redacted cleanly.
    let xml_res = scrubber.scrub_text(
        "<config><password>topsecret99</password><apiKey>k-abc123456</apiKey><host>localhost</host></config>",
    );
    assert_eq!(
        xml_res.scrubbed,
        "<config><password><REDACTED:ENV_SECRET></password><apiKey><REDACTED:ENV_SECRET></apiKey><host>localhost</host></config>"
    );

    // Encoded Claude/tool project folder slugs redact the user prefix while keeping repo tail.
    let slug_res = scrubber
        .scrub_text("dir C--Users-alice-projects-widget and -Users-bob-code and -home-carol-app");
    assert_eq!(
        slug_res.scrubbed,
        "dir <HOME>-projects-widget and <HOME>-code and <HOME>-app"
    );

    // Opt-in bare username redaction respects word boundaries.
    let user_res = scrubber.scrub_text("owner jdoe_admin ran jdoe_admin_extra");
    assert_eq!(
        user_res.scrubbed,
        "owner <REDACTED:USER> ran jdoe_admin_extra"
    );

    // Idempotent second pass.
    assert_eq!(
        scrubber.scrub_text(&env_res.scrubbed).scrubbed,
        env_res.scrubbed
    );
    assert_eq!(
        scrubber.scrub_text(&slug_res.scrubbed).scrubbed,
        slug_res.scrubbed
    );
}
