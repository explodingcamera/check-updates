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

    #[arg(
        short = 'u',
        long,
        help = "Update version requirements in Cargo.toml and run cargo update"
    )]
    pub update: bool,

    #[arg(short = 'U', long, help = "Update version requirements in Cargo.toml")]
    pub upgrade: bool,

    #[arg(long, help = "Only upgrade to semver-compatible versions")]
    pub compatible: bool,

    #[arg(long, help = "Compact interactive mode (fewer spacing lines)")]
    pub compact: bool,

    #[arg(long, help = "Include pre-release/alpha/beta versions")]
    pub pre: bool,

    #[arg(
        short,
        long,
        help = "Only check specific packages (can be specified multiple times)"
    )]
    pub package: Vec<String>,

    #[command(subcommand)]
    pub cmd: Option<Command>,
}

#[derive(Parser, Debug)]
pub enum Command {
    #[command(
        about = "Generate shell completion scripts",
        long_about = "Generate shell completion scripts for check-updates.\nCan be used like `check-updates generate-completion bash > check-updates.bash`"
    )]
    GenerateShellCompletion {
        #[clap(value_name = "SHELL")]
        shell: clap_complete::Shell,
    },
}
