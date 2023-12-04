use clap::{ArgAction, Parser, ValueEnum, ValueHint};

const TEMPLATE: &str = "usage:
    paccat [options] <target> <files>
    paccat [options] <targets> -- <files>
    paccat [options] -<Q|F> [targets] -- <files>

a target can be specified as:
    <pkgname>, <repo>/<pkgname>, <url> or <file>.

files can be specified as just the filename or the full path.

{about}

{options}";

#[derive(Copy, Clone, Default, Debug, ValueEnum)]
pub enum ColorWhen {
    #[default]
    Auto,
    Always,
    Never,
}

#[derive(Parser, Debug)]
#[command(
    help_template(TEMPLATE),
    version = concat!("v", clap::crate_version!()),
    args_override_self = true,
    arg_required_else_help = true,
)]
/// Print pacman package files
pub struct Args {
    #[arg(short = 'F', long = "files")]
    /// Use files database to search for files before deciding to download
    pub filedb: bool,
    #[arg(short = 'Q', conflicts_with = "filedb", long = "query")]
    /// Use local database to search for files before deciding to download
    pub localdb: bool,
    #[arg(short, long, value_name = "path")]
    /// Set an alternative root directory
    pub root: Option<String>,
    #[arg(short = 'b', long, value_name = "path")]
    /// Set an alternative database location
    pub dbpath: Option<String>,
    #[arg(long, short = 'y', action = ArgAction::Count)]
    /// Download fresh package databases from the server
    pub refresh: u8,
    #[arg(long, value_name = "path")]
    /// Set an alternative cache directory
    pub cachedir: Option<String>,
    /// Specify when to enable coloring
    #[arg(long, value_name = "when", value_enum, default_value_t = ColorWhen::Auto)]
    pub color: ColorWhen,
    #[arg(short = 'x', long)]
    /// Enable searching using regular expressions
    pub regex: bool,
    #[arg(short, long)]
    /// Print all matches of files instead of just the first
    pub all: bool,
    #[arg(short = 'e', long)]
    /// Extract matched files to the current directory
    pub extract: bool,
    #[arg(long, short, conflicts_with = "extract")]
    /// Install matched files to the system
    pub install: bool,
    #[arg(short, long)]
    /// Print file names instead of file content
    pub quiet: bool,
    #[arg(long)]
    ///Print binary files
    pub binary: bool,
    #[arg(long, value_name = "file")]
    /// Use an alternative pacman.conf
    pub config: Option<String>,
    #[arg(
        value_name = "targets",
        value_hint = ValueHint::AnyPath,
    )]
    /// List of packages, package files, or package urls
    pub targets: Vec<String>,
    #[arg(
        last = true,
        value_name = "files",
        value_hint = ValueHint::AnyPath,
    )]
    pub files: Vec<String>,
}
