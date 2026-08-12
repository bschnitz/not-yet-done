//! Interactively build a small auth config on the terminal.
//!
//! Run with: `cargo run -p fieldsmith --example interactive --features stdin`

#![allow(dead_code)]

use fieldsmith::{Buildable, build_stdin};
use serde::Deserialize;

/// How a secret is obtained.
#[derive(Buildable, Deserialize, Debug)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Cred {
    /// Read the secret from an environment variable.
    Env {
        /// Name of the variable.
        var: String,
    },
    /// Run a command; its stdout is the secret.
    Command {
        /// Command line to run.
        script: String,
        /// Abort after this many seconds.
        timeout_secs: Option<u64>,
    },
}

/// Auth block with a credential and extra headers.
#[derive(Buildable, Deserialize, Debug)]
struct AuthConfig {
    /// How to obtain the secret.
    cred: Cred,
    /// Extra HTTP headers, verbatim.
    #[serde(default)]
    headers: Vec<String>,
}

fn main() {
    match build_stdin::<AuthConfig>() {
        Ok(cfg) => println!("\nBuilt: {cfg:#?}"),
        Err(e) => eprintln!("\naborted: {e}"),
    }
}
