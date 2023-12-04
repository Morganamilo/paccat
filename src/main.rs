use crate::args::Args;
use crate::pacman::{alpm_init, get_dbpkg, get_download_url};
use alpm::{Alpm, Package};
use alpm_utils::DbListExt;
use anyhow::{bail, ensure, Context, Error, Result};
use clap::Parser;
use compress_tools::{ArchiveContents, ArchiveIterator};
use nix::sys::signal::{signal, SigHandler, Signal};
use nix::sys::stat::{umask, Mode};
use nix::unistd::{isatty, Uid};
use pacman::verify_packages;
use regex::RegexSet;
use std::fs::{create_dir_all, File, OpenOptions};
use std::io::{self, Read, Seek, Stdout, StdoutLock, Write};
use std::mem::take;
use std::os::unix::fs::fchown;
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::io::AsRawFd;
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};

mod args;
mod pacman;

#[derive(Default)]
enum Output<'a> {
    Stdout(StdoutLock<'a>),
    Bat(Child, ChildStdin),
    File(File),
    #[default]
    None,
}

#[derive(PartialEq, Eq)]
enum EntryState {
    Skip,
    FirstChunk,
    Reading,
}

struct Match {
    with: MatchWith,
    exact_file: bool,
}

impl Match {
    fn new(regex: bool, files: Vec<String>) -> Result<Self> {
        let exact_file = files.iter().any(|f| f.contains('/'));
        let with = MatchWith::new(regex, files)?;
        Ok(Self { exact_file, with })
    }

    fn is_match(&mut self, file: &str, remove: bool) -> bool {
        let file = if !self.exact_file {
            file.rsplit('/').next().unwrap()
        } else {
            file
        };

        if file.is_empty() {
            return false;
        }

        match self.with {
            MatchWith::Regex(ref r) => r.is_match(file),
            MatchWith::Files(ref mut f) => {
                if let Some(pos) = f.iter().position(|t| t == file) {
                    if remove {
                        f.remove(pos);
                    }
                    true
                } else {
                    false
                }
            }
        }
    }
}

enum MatchWith {
    Regex(RegexSet),
    Files(Vec<String>),
}

impl MatchWith {
    fn new(regex: bool, files: Vec<String>) -> Result<Self> {
        let match_with = if regex {
            let regex = RegexSet::new(files)?;
            MatchWith::Regex(regex)
        } else {
            MatchWith::Files(files)
        };

        Ok(match_with)
    }
}

fn print_error(err: Error) {
    eprint!("error");
    for link in err.chain() {
        eprint!(": {}", link);
    }
    eprintln!();
}

fn main() {
    unsafe { signal(Signal::SIGPIPE, SigHandler::SigDfl).unwrap() };

    match run() {
        Ok(i) => std::process::exit(i),
        Err(e) => {
            print_error(e);
            std::process::exit(1);
        }
    }
}

fn run() -> Result<i32> {
    let mut args = args::Args::parse();
    let mut ret = 0;
    let stdout = io::stdout();
    let is_tty = isatty(stdout.as_raw_fd()).unwrap_or(false);

    if !args.targets.is_empty() && args.files.is_empty() {
        args.files = args.targets.split_off(1);
    }

    if !args.localdb && !args.filedb && args.targets.is_empty() {
        bail!("no targets specified (use -h for help)");
    }
    if args.files.is_empty() {
        bail!("no files specified (use -h for help)");
    }

    args.binary |= !is_tty;
    args.binary |= args.extract || args.install;

    let color = match args.color {
        args::ColorWhen::Auto => is_tty,
        args::ColorWhen::Always => false,
        args::ColorWhen::Never => true,
    };

    let files = args
        .files
        .iter()
        .map(|f| f.trim_start_matches('/').to_string())
        .collect::<Vec<_>>();

    let mut matcher = Match::new(args.regex, files)?;
    let alpm = alpm_init(&args)?;

    let pkgs = get_targets(&alpm, &args, &mut matcher)?;

    if args.install {
        umask(Mode::empty());
    }

    for pkg in pkgs {
        let file = File::open(&pkg).with_context(|| format!("failed to open {}", pkg))?;
        let archive = ArchiveIterator::from_read(file)?;
        ret |= dump_files(archive, &mut matcher, &args, color, &alpm)?;
    }

    Ok(ret)
}

fn open_output(
    output: &mut Output,
    stdout: &mut Stdout,
    filename: &str,
    use_bat: bool,
) -> Result<()> {
    match (output, use_bat) {
        (Output::Stdout(_), _) => (),
        (Output::File(_), _) => (),
        (output @ Output::Bat(_, _), _) | (output @ Output::None, true) => {
            let mut child = Command::new("bat")
                .arg("-pp")
                .arg("--file-name")
                .arg(filename)
                .stdin(Stdio::piped())
                .spawn()?;

            let stdin = child.stdin.take().unwrap();
            *output = Output::Bat(child, stdin);
        }
        (output @ Output::None, false) => *output = Output::Stdout(stdout.lock()),
    };
    Ok(())
}

fn close_outout(output: &mut Output) -> Result<()> {
    if let Output::Bat(mut child, stdin) = take(output) {
        drop(stdin);
        let status = child.wait().context("failed to wait for bat")?;
        ensure!(
            status.success(),
            "bat failed to run (exited {})",
            status.code().unwrap_or(1),
        );
    }
    Ok(())
}

fn dump_files<R>(
    archive: ArchiveIterator<R>,
    matcher: &mut Match,
    args: &Args,
    color: bool,
    alpm: &Alpm,
) -> Result<i32>
where
    R: Read + Seek,
{
    let mut stdout = io::stdout();
    let mut output = Output::default();
    let mut state = EntryState::Skip;
    let mut found = 0;
    let mut filename = String::new();

    let use_bat = color
        && !args.quiet
        && !args.extract
        && !args.install
        && Command::new("bat").arg("-h").status().is_ok();

    for content in archive {
        match content {
            ArchiveContents::StartOfEntry(mut file, stat) => {
                filename = file.rsplit('/').next().unwrap().to_string();

                if matcher.is_match(&file, !args.all) {
                    found += 1;

                    if args.quiet || args.extract || args.install {
                        writeln!(stdout, "{}", file)?;

                        if args.extract || args.install {
                            state = EntryState::FirstChunk;
                            let open_file = if args.install {
                                file.insert_str(0, alpm.root());
                                Path::new(&file)
                            } else {
                                Path::new(&filename)
                            };

                            let exists = !args.install || open_file.exists();

                            if !exists {
                                if let Some(parent) = open_file.parent() {
                                    create_dir_all(parent).with_context(|| {
                                        format!("failed to mkdir {}", parent.display())
                                    })?;
                                }
                            }

                            let extract_file = OpenOptions::new()
                                .write(true)
                                .create(true)
                                .truncate(true)
                                .mode(stat.st_mode)
                                .open(open_file)
                                .with_context(|| {
                                    format!("failed to open {}", open_file.display())
                                })?;

                            if !exists && Uid::current().is_root() {
                                fchown(&extract_file, Some(stat.st_uid), Some(stat.st_gid))
                                    .with_context(|| {
                                        format!("failed to chown {}", open_file.display())
                                    })?;
                            }

                            output = Output::File(extract_file);
                        }
                    } else {
                        open_output(&mut output, &mut stdout, &filename, use_bat)?;
                        state = EntryState::FirstChunk;
                    }
                }
            }
            ArchiveContents::DataChunk(data) if state == EntryState::FirstChunk => {
                if !args.binary && is_binary(&data) {
                    state = EntryState::Skip;
                    eprintln!("{} is a binary file -- use --binary to print", filename);
                } else {
                    read_chunk(&mut state, &mut output, &data)?;
                }
            }
            ArchiveContents::DataChunk(v) if state == EntryState::Reading => {
                read_chunk(&mut state, &mut output, &v)?;
            }
            ArchiveContents::DataChunk(_) => (),
            ArchiveContents::EndOfEntry => {
                state = EntryState::Skip;
                close_outout(&mut output)?;
            }
            ArchiveContents::Err(e) => {
                return Err(e.into());
            }
        }
    }

    let ret = match &matcher.with {
        MatchWith::Files(f) if f.is_empty() => 0,
        MatchWith::Regex(_) if found != 0 => 0,
        _ => 1,
    };

    Ok(ret)
}

fn read_chunk(
    state: &mut EntryState,
    output: &mut Output,
    data: &[u8],
) -> Result<(), anyhow::Error> {
    *state = EntryState::Reading;
    match output {
        Output::Stdout(stdout) => stdout.write_all(data)?,
        Output::Bat(_, stdin) => stdin.write_all(data)?,
        Output::File(file) => file.write_all(data)?,
        Output::None => (),
    };
    Ok(())
}

fn is_binary(data: &[u8]) -> bool {
    data.iter().take(512).any(|&b| b == 0)
}

fn get_targets(alpm: &Alpm, args: &Args, matcher: &mut Match) -> Result<Vec<String>> {
    let mut download = Vec::new();
    let mut url = Vec::new();
    let mut repo = Vec::new();
    let mut files = Vec::new();
    let dbs = alpm.syncdbs();

    if args.targets.is_empty() {
        if args.localdb {
            let pkgs = alpm.localdb().pkgs();
            let pkgs = pkgs
                .iter()
                .filter(|pkg| want_pkg(alpm, *pkg, matcher))
                .filter_map(|p| dbs.pkg(p.name()).ok());
            repo.extend(pkgs);
        } else if args.filedb {
            let pkgs = dbs
                .iter()
                .flat_map(|db| db.pkgs())
                .filter(|pkg| want_pkg(alpm, *pkg, matcher));
            repo.extend(pkgs);
        }
    } else {
        for targ in &args.targets {
            if let Ok(pkg) = get_dbpkg(alpm, targ, args.localdb) {
                if pkg.files().files().is_empty() || want_pkg(alpm, pkg, matcher) {
                    repo.push(pkg);
                }
            } else if targ.contains("://") {
                url.push(targ.clone());
            } else if Path::new(&targ).exists() {
                files.push(targ.to_string());
            } else {
                bail!("'{}' is not a package, file or url", targ);
            }
        }
    }

    // todo filter repopkg files

    for &pkg in &repo {
        download.push(get_download_url(pkg)?);
    }
    download.extend(url.clone());

    let downloaded = alpm.fetch_pkgurl(download.into_iter())?;
    let mut iter = downloaded.iter();

    verify_packages(
        alpm,
        alpm.local_file_siglevel(),
        files.iter().map(|s| s.as_str()),
    )?;

    verify_packages(
        alpm,
        alpm.default_siglevel(),
        iter.by_ref().take(repo.len()),
    )?;
    verify_packages(alpm, alpm.remote_file_siglevel(), iter)?;

    files.extend(downloaded);

    Ok(files)
}

fn want_pkg(_alpm: &Alpm, pkg: Package, matcher: &mut Match) -> bool {
    let files = pkg.files();
    if matches!(matcher.with, MatchWith::Files(ref f) if f.is_empty()) {
        return false;
    }
    files
        .files()
        .iter()
        .any(|f| matcher.is_match(f.name(), false))
}
