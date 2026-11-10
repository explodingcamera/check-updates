use bpaf::Bpaf;
use owo_colors::{OwoColorize, Stream, Style};

use crate::{cmd, interactive};

/// Check for updates in your dependencies.
#[derive(Bpaf, Clone, Debug)]
#[bpaf(options, version(env!("CARGO_PKG_VERSION")))]
pub struct Args {
    /// Interactive mode
    #[bpaf(short('i'), long)]
    pub interactive: bool,

    /// Root directory to search from
    #[bpaf(argument("DIR"))]
    pub root: Option<std::path::PathBuf>,

    /// Enable verbose output
    pub verbose: bool,

    /// Exit with a non-zero status when updates are available
    pub fail_on_updates: bool,

    /// Cache mode: prefer-local, refresh, or no-cache
    #[bpaf(argument("MODE"), fallback(RegistryCacheMode::Refresh))]
    pub cache: RegistryCacheMode,

    /// Update non-breaking version requirements in project manifests
    #[bpaf(short('u'), long)]
    pub update: bool,

    /// Update non-breaking requirements and installed dependencies
    #[bpaf(short('U'), long)]
    pub upgrade: bool,

    /// Include breaking requirement updates
    #[bpaf(short('b'), long)]
    pub breaking: bool,

    /// With --upgrade, update lockfiles without installing JavaScript dependencies
    pub lockfile_only: bool,

    /// Only use versions allowed by current requirements
    #[bpaf(short('c'), long)]
    pub compatible: bool,

    /// Compact interactive mode (fewer spacing lines)
    pub compact: bool,

    /// Include pre-release/alpha/beta versions
    pub pre: bool,

    /// Ignore package toolchain version requirements
    #[bpaf(long, long("ignore-rust-version"))]
    pub ignore_toolchain_version: bool,

    /// Only check specific workspace packages (can be repeated)
    #[bpaf(short('p'), long, argument("NAME"))]
    pub package: Vec<String>,

    #[bpaf(external(command), optional)]
    pub cmd: Option<Command>,
}

impl Args {
    pub fn version_strategy(&self) -> crate::VersionStrategy {
        crate::VersionStrategy {
            compatible: self.compatible,
            pre: self.pre,
            ignore_toolchain_version: self.ignore_toolchain_version,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum RegistryCacheMode {
    PreferLocal,
    Refresh,
    NoCache,
}

impl std::str::FromStr for RegistryCacheMode {
    type Err = &'static str;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "prefer-local" => Ok(Self::PreferLocal),
            "refresh" => Ok(Self::Refresh),
            "no-cache" => Ok(Self::NoCache),
            _ => Err("expected prefer-local, refresh, or no-cache"),
        }
    }
}

#[derive(Bpaf, Clone, Debug)]
pub enum Command {
    /// Generate shell completion scripts
    #[bpaf(command("generate-shell-completion"))]
    GenerateShellCompletion {
        /// Shell to generate completions for: bash, fish, zsh, or elvish
        #[bpaf(positional("SHELL"), guard(|shell| ["bash", "fish", "zsh", "elvish"].contains(&shell.as_str()), "expected bash, fish, zsh, or elvish"))]
        shell: String,
    },
}

/// Run the standalone command.
pub fn main() {
    execute(false);
}

/// Run the Cargo subcommand.
pub fn cargo_main() {
    execute(true);
}

fn execute(cargo: bool) {
    let mut input: Vec<_> = std::env::args_os().skip(1).collect();
    if cargo && input.first().is_some_and(|arg| arg == "check-updates") {
        input.remove(0);
    }
    let args = args()
        .run_inner(bpaf::Args::from(input.as_slice()).set_name("check-updates"))
        .unwrap_or_else(|error| {
            error.print_message(80);
            std::process::exit(if error.exit_code() == 0 { 0 } else { 2 });
        });
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("failed to initialize async runtime")
        .block_on(run(args));
}

/// Run the CLI with parsed arguments.
pub async fn run(args: Args) {
    if let Some(Command::GenerateShellCompletion { shell }) = &args.cmd {
        let input = [format!("--bpaf-complete-style-{shell}")];
        let _ =
            self::args().run_inner(bpaf::Args::from(input.as_slice()).set_name("check-updates"));
        return;
    }
    if args.lockfile_only && !args.upgrade {
        eprintln!(
            "{} --lockfile-only requires --upgrade",
            "error:"
                .if_supports_color(Stream::Stderr, |text| text.style(Style::new().red().bold()))
        );
        std::process::exit(2);
    }
    log::set_logger(&interactive::Logger).expect("failed to initialize logger");
    log::set_max_level(if args.verbose {
        log::LevelFilter::Debug
    } else {
        log::LevelFilter::Info
    });
    let root = args
        .root
        .clone()
        .unwrap_or_else(|| std::env::current_dir().expect("current directory"));
    let has_updates = match cmd::run(&root, &args).await {
        Ok(has_updates) => has_updates,
        Err(error) => {
            log::error!("Failed to check dependencies: {error}");
            std::process::exit(1);
        }
    };
    if args.fail_on_updates && has_updates {
        std::process::exit(2);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_grouped_flags_and_values() {
        let parser = args();
        parser.check_invariants(false);
        for input in [
            &[
                "-iU",
                "-p",
                "app",
                "--package",
                "lib",
                "--ignore-rust-version",
            ][..],
            &["-iUpapp", "-plib", "--ignore-toolchain-version"][..],
        ] {
            let args = parser.run_inner(input).unwrap();
            assert!(args.interactive && args.upgrade);
            assert_eq!(args.package, ["app", "lib"]);
            assert!(args.version_strategy().ignore_toolchain_version);
        }
    }
}
