//! Print the YAML template for a small Jira-shaped config.
//!
//! Run with: `cargo run -p fieldsmith --example template`

#![allow(dead_code)]

use fieldsmith::{Buildable, yaml_template};

#[derive(Buildable)]
struct DbCfg {
    /// Sea-orm-compatible cache URL.
    url: String,
}

/// Jira adapter configuration.
#[derive(Buildable)]
struct JiraConfig {
    /// Base URL of your Jira instance.
    #[builder(default = "https://your-jira.example.com")]
    url: String,
    /// Optional display name for this instance.
    #[builder(default = "My Jira")]
    name: Option<String>,
    /// Trust self-signed TLS certificates.
    #[builder(default = false)]
    accept_invalid_certs: bool,
    /// Optional cache DB override.
    db: Option<DbCfg>,
}

fn main() {
    print!("{}", yaml_template(&JiraConfig::schema()));
}
