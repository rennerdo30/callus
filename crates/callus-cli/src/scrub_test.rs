//! `callus scrub-test`: try the redaction rules on sample text.

use std::{
    io::{Read as _, Write as _},
    path::PathBuf,
};

use callus_core::{CallusError, Scrubber, ScrubberConfig};

#[derive(Debug, clap::Args)]
pub(crate) struct Args {
    /// Text to scrub; read from stdin without TEXT and --file
    #[arg(conflicts_with = "file", allow_hyphen_values = true)]
    text: Option<String>,
    /// Read the text from a file
    #[arg(long, value_name = "PATH")]
    file: Option<PathBuf>,
    /// Repository root, shown as <REPO>
    #[arg(long, value_name = "PATH")]
    repo_root: Option<String>,
    /// Extra pattern to redact (repeatable)
    #[arg(long, value_name = "REGEX")]
    deny_regex: Vec<String>,
    /// Extra hostname to redact (repeatable)
    #[arg(long, value_name = "HOST")]
    hostname: Vec<String>,
    /// Print JSON with `scrubbed` and `redactions`
    #[arg(long)]
    json: bool,
}

fn io_error(context: impl Into<String>) -> impl FnOnce(std::io::Error) -> CallusError {
    let context = context.into();
    move |source| CallusError::Io { context, source }
}

pub(crate) fn run(args: Args) -> Result<(), CallusError> {
    let scrubber = Scrubber::new(&ScrubberConfig {
        repo_root: args.repo_root,
        home_dir: std::env::home_dir().map(|home| home.to_string_lossy().into_owned()),
        extra_hostnames: args.hostname,
        deny_regexes: args.deny_regex,
    })?;
    let input = match (args.text, args.file) {
        (Some(text), _) => text,
        (None, Some(path)) => {
            let context = format!("cannot read {}", path.display());
            decode(&std::fs::read(path).map_err(io_error(context))?)
        }
        (None, None) => {
            let mut bytes = Vec::new();
            std::io::stdin()
                .read_to_end(&mut bytes)
                .map_err(io_error("cannot read stdin"))?;
            decode(&bytes)
        }
    };
    let result = scrubber.scrub_text(&input);
    let mut output = if args.json {
        serde_json::to_string_pretty(&result)?
    } else {
        result.scrubbed
    };
    if !output.ends_with('\n') {
        output.push('\n');
    }
    std::io::stdout()
        .write_all(output.as_bytes())
        .map_err(io_error("cannot write stdout"))
}

/// Accepts UTF-8 and the BOM-marked UTF-16 that Windows tools write. Other
/// bytes are replaced so one bad byte cannot hide the rest from the rules.
fn decode(bytes: &[u8]) -> String {
    let utf16 = |rest: &[u8], read: fn([u8; 2]) -> u16| {
        let (pairs, _) = rest.as_chunks::<2>();
        String::from_utf16_lossy(&pairs.iter().map(|&pair| read(pair)).collect::<Vec<_>>())
    };
    match bytes {
        [0xFF, 0xFE, rest @ ..] => utf16(rest, u16::from_le_bytes),
        [0xFE, 0xFF, rest @ ..] => utf16(rest, u16::from_be_bytes),
        [0xEF, 0xBB, 0xBF, rest @ ..] => String::from_utf8_lossy(rest).into_owned(),
        _ => String::from_utf8_lossy(bytes).into_owned(),
    }
}
