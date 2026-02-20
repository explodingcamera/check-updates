use clap::{Parser, builder::styling};

const STYLES: clap::builder::Styles = styling::Styles::styled()
    .header(styling::AnsiColor::Yellow.on_default().bold())
    .usage(styling::AnsiColor::Green.on_default().bold())
    .literal(styling::AnsiColor::Blue.on_default().bold())
    .placeholder(styling::AnsiColor::Green.on_default());

#[derive(Parser, Debug)]
#[command(
    name = "check-updates",
    about = "Check for updates in your dependencies",
    version,
    styles(STYLES)
)]
pub struct Args {
    #[arg(short, long, help = "Interactive mode")]
    pub interactive: bool,

    #[arg(long, help = "Root directory to search from", value_name = "DIR")]
    pub root: Option<std::path::PathBuf>,

    #[arg(long, help = "Enable verbose output")]
    pub verbose: bool,

    #[arg(short = 'u', long, help = "Upgrade version requirements in Cargo.toml")]
    pub upgrade: bool,

    #[arg(
        short = 'U',
        long = "update",
        help = "Upgrade requirements and run cargo update",
        conflicts_with = "upgrade"
    )]
    pub update: bool,

    #[arg(long, help = "Only upgrade to semver-compatible versions")]
    pub compatible: bool,

    #[arg(long, help = "Include pre-release/alpha/beta versions")]
    pub pre: bool,

    #[arg(
        short,
        long,
        help = "Only check specific packages (can be specified multiple times)"
    )]
    pub package: Vec<String>,
}

// TODO
// #[arg(
//     long,
//     help = "Ignore minimum rust version when checking compatible versions"
// )]
// pub ignore_rust_version: bool,
