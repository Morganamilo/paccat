use crate::args::Args;
use crate::pacman::{alpm_init, get_dbpkg, get_download_url};
use alpm::{Alpm, Package};
use alpm_utils::DbListExt;
use anyhow::{bail, ensure, Context, Error, Result};
use clap::Parser;
use compress_tools::{ArchiveContents, ArchiveIterator};
use nix::sys::stat::{umask, Mode, SFlag};
use nix::unistd::Uid;
use pacman::verify_packages;
use regex::RegexSet;
use std::fs::{create_dir_all, File};
use std::io::{
    self, stderr, stdin, BufRead, ErrorKind, IsTerminal, Read, Seek, Stdout, StdoutLock, Write,
};
use std::mem::take;
use std::os::unix::fs::fchown;
use std::os::unix::fs::OpenOptionsExt;
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

#[derive(Debug)]
struct Match {
    with: MatchWith,
    exact_file: bool,
    matched: Vec<usize>,
}

impl Match {
    fn new(regex: bool, files: Vec<String>) -> Result<Self> {
        let exact_file = files.iter().any(|f| f.contains('/'));
        let with = MatchWith::new(regex, files)?;
        let matched = Vec::new();
        Ok(Self {
            exact_file,
            with,
            matched,
        })
    }

    fn all_matched(&self) -> bool {
        match &self.with {
            MatchWith::Regex(r) => r.len() == self.matched.len(),
            MatchWith::Files(f) => f.len() == self.matched.len(),
        }
    }

    fn is_match(&mut self, file: &str, match_once: bool) -> bool {
        let file = if !self.exact_file {
            file.rsplit('/').next().unwrap()
        } else {
            file
        };

        if file.is_empty() {
            return false;
        }

        match self.with {
            MatchWith::Regex(ref mut r) => {
                let mut new_match = false;
                for m in r.matches(file) {
                    if !self.matched.contains(&m) {
                        self.matched.push(m);
                        new_match = true;
                    } else {
                        new_match = !match_once;
                    }
                }
                new_match
            }
            MatchWith::Files(ref mut f) => {
                if let Some(pos) = f.iter().position(|t| t == file || t == "*") {
                    if !self.matched.contains(&pos) {
                        self.matched.push(pos);
                        true
                    } else {
                        !match_once
                    }
                } else {
                    false
                }
            }
        }
    }
}

#[derive(Debug)]
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
    let mut stderr = stderr();
    let _ = write!(stderr, "error");
    for link in err.chain() {
        let _ = write!(stderr, ": {}", link);
    }
    let _ = writeln!(stderr);
}

fn main() {
    match run() {
        Ok(i) => std::process::exit(i),
        Err(e) => {
            if let Some(e) = e.downcast_ref::<io::Error>() {
                if e.kind() == ErrorKind::BrokenPipe {
                    std::process::exit(1);
                }
            }
            print_error(e);
            std::process::exit(1);
        }
    }
}

fn read_stdin(values: &mut Vec<String>) -> Result<()> {
    if let Some(index) = values.iter().position(|s| s == "-") {
        values.remove(index);

        if stdin().is_terminal() {
            bail!("argument '-' specified without input on stdin");
        }

        for line in stdin().lock().lines() {
            let line = line.context("failed to read stdin")?;
            values.push(line);
        }
    }

    Ok(())
}

fn run() -> Result<i32> {
    let mut args = args::Args::parse();
    let stdout = io::stdout();
    let is_tty = stdout.is_terminal();

    if !args.targets.is_empty() && args.files.is_empty() {
        if args.filedb || args.localdb {
            args.files = args.targets.split_off(0);
        } else {
            args.files = args.targets.split_off(1);
        }
    }

    if args.refresh == 0 && !args.localdb && !args.filedb && args.targets.is_empty() {
        bail!("no targets specified (use -h for help)");
    }
    if (args.refresh == 0 || !args.targets.is_empty()) && args.files.is_empty() {
        bail!("no files specified (use -h for help)");
    }

    read_stdin(&mut args.targets)?;
    read_stdin(&mut args.files)?;

    args.binary |= !is_tty;
    args.binary |= args.extract || args.install;

    let color = match args.color {
        args::ColorWhen::Auto => is_tty,
        args::ColorWhen::Always => true,
        args::ColorWhen::Never => false,
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
        dump_files(archive, &mut matcher, &args, color, &alpm)?;
    }

    match matcher.all_matched() {
        true => Ok(0),
        false => Ok(1),
    }
}

fn open_output(
    output: &mut Output,
    stdout: &mut Stdout,
    filename: &str,
    use_bat: bool,
) -> Result<()> {
    match (output, use_bat) {
        (Output::File(_), _) => (),
        (output @ Output::Bat(_, _), _)
        | (output @ Output::None | output @ Output::Stdout(_), true) => {
            let mut child = Command::new("bat")
                .arg("-pp")
                .arg("--color=always")
                .arg("--file-name")
                .arg(filename)
                .stdin(Stdio::piped())
                .spawn()?;

            let stdin = child.stdin.take().unwrap();
            *output = Output::Bat(child, stdin);
        }
        (output @ Output::None | output @ Output::Stdout(_), false) => {
            *output = Output::Stdout(stdout.lock())
        }
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
) -> Result<()>
where
    R: Read + Seek,
{
    let mut stdout = io::stdout();
    let mut output = Output::default();
    let mut state = EntryState::Skip;
    let mut filename = String::new();

    let use_bat = color
        && !args.list
        && !args.extract
        && !args.install
        && Command::new("bat").arg("-h").output().is_ok();

    for content in archive {
        match content {
            ArchiveContents::StartOfEntry(mut file, stat) => {
                let mode = Mode::from_bits_truncate(stat.st_mode);
                let kind = SFlag::from_bits_truncate(stat.st_mode);

                if kind != SFlag::S_IFREG {
                    continue;
                }

                if args.executable && !mode.contains(Mode::S_IXUSR) {
                    continue;
                }

                filename = file.rsplit('/').next().unwrap().to_string();

                if matcher.is_match(&file, !args.all) {
                    if args.list || args.extract || args.install {
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

                            let extract_file = File::options()
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
                        let file = "/".to_string() + &file;
                        open_output(&mut output, &mut stdout, &file, use_bat)?;
                        state = EntryState::FirstChunk;
                    }
                }
            }
            ArchiveContents::DataChunk(data) if state == EntryState::FirstChunk => {
                if is_binary(&data) && matches!(output, Output::Bat(_, _)) {
                    output = Output::Stdout(stdout.lock());

                    if args.binary {
                        read_chunk(&mut state, &mut output, &data)?;
                    } else {
                        state = EntryState::Skip;
                        writeln!(
                            stderr(),
                            "{} is a binary file use --binary to print",
                            filename
                        )?;
                    }
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

    Ok(())
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
                .filter(|pkg| want_pkg(args.all, pkg, matcher))
                .filter_map(|p| dbs.pkg(p.name()).ok());
            repo.extend(pkgs);
        } else if args.filedb {
            let pkgs = dbs
                .iter()
                .flat_map(|db| db.pkgs())
                .filter(|pkg| want_pkg(args.all, pkg, matcher));
            repo.extend(pkgs);
        }

        if !args.all && !args.executable {
            repo.truncate(1);
        }
    } else {
        for targ in &args.targets {
            if let Ok(pkg) = get_dbpkg(alpm, targ, args.localdb) {
                if pkg.files().files().is_empty() || want_pkg(args.all, pkg, matcher) {
                    repo.push(pkg);
                }
            } else if targ.contains("://") {
                url.push(targ.clone());
            } else if Path::new(&targ).is_file() {
                files.push(targ.to_string());
            } else {
                bail!("'{}' is not a package, file or url", targ);
            }
        }
    }

    matcher.matched.clear();

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

fn want_pkg(all: bool, pkg: &Package, matcher: &mut Match) -> bool {
    let files = pkg.files();
    if !all && matcher.all_matched() {
        return false;
    }
    files
        .files()
        .iter()
        .any(|f| matcher.is_match(f.name(), false))
}
