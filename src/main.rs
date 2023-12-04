use crate::args::Args;
use crate::pacman::{alpm_init, get_dbpkg, get_download_url};
use alpm::{Alpm, Package};
use alpm_utils::DbListExt;
use anyhow::{bail, Context, Result};
use clap::Parser;
use compress_tools::{ArchiveContents, ArchiveIterator};
use nix::sys::signal::{signal, SigHandler, Signal};
use nix::unistd::isatty;
use regex::RegexSet;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, Write};
use std::os::unix::io::AsRawFd;
use std::path::Path;

mod args;
mod pacman;

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

fn main() {
    unsafe { signal(Signal::SIGPIPE, SigHandler::SigDfl).unwrap() };

    match run() {
        Ok(i) => std::process::exit(i),
        Err(e) => {
            eprintln!("error: {}", e);
            std::process::exit(1);
        }
    }
}

fn run() -> Result<i32> {
    let mut args = args::Args::parse();
    let mut ret = 0;
    let stdout = io::stdout();

    args.binary |= !isatty(stdout.as_raw_fd()).unwrap_or(false);
    args.binary |= args.extract;

    let files = args
        .files
        .iter()
        .map(|f| f.trim_start_matches('/').to_string())
        .collect::<Vec<_>>();

    let mut matcher = Match::new(args.regex, files)?;
    let alpm = alpm_init(&args)?;

    let pkgs = get_targets(&alpm, &args, &mut matcher)?;

    for pkg in pkgs {
        let file = File::open(&pkg).with_context(|| format!("failed to open {}", pkg))?;
        let archive = ArchiveIterator::from_read(file)?;
        ret |= dump_files(archive, &mut matcher, &args)?;
    }

    Ok(ret)
}

fn dump_files<R>(archive: ArchiveIterator<R>, matcher: &mut Match, args: &Args) -> Result<i32>
where
    R: Read + Seek,
{
    let stdout = io::stdout();
    let mut stdout = stdout.lock();
    let mut state = EntryState::Skip;
    let mut found = 0;
    let mut cur_file = String::new();
    let mut cur_extract_file: Option<File> = None;

    for content in archive {
        match content {
            ArchiveContents::StartOfEntry(file, _) => {
                if matcher.is_match(&file, !args.all) {
                    found += 1;

                    if args.quiet || args.extract {
                        writeln!(stdout, "{}", file)?;

                        if args.extract {
                            state = EntryState::FirstChunk;
                            cur_file = file;

                            let filename = cur_file.rsplit('/').next().unwrap();
                            let extract_file = OpenOptions::new()
                                .write(true)
                                .create(true)
                                .truncate(true)
                                .open(filename)
                                .with_context(|| format!("failed to open target {}", filename))?;
                            cur_extract_file = Some(extract_file);
                        }
                    } else {
                        state = EntryState::FirstChunk;
                        cur_file = file;
                    }
                }
            }
            ArchiveContents::DataChunk(v) if state == EntryState::FirstChunk => {
                if !args.binary && is_binary(&v) {
                    state = EntryState::Skip;
                    eprintln!("{} is a binary file -- use --binary to print", cur_file);
                } else {
                    state = EntryState::Reading;

                    if let Some(extract_file) = &mut cur_extract_file {
                        extract_file.write_all(&v)?;
                    } else {
                        stdout.write_all(&v)?;
                    }
                }
            }
            ArchiveContents::DataChunk(v) if state == EntryState::Reading => {
                if let Some(extract_file) = &mut cur_extract_file {
                    extract_file.write_all(&v)?;
                } else {
                    stdout.write_all(&v)?;
                }
            }
            ArchiveContents::DataChunk(_) => (),
            ArchiveContents::EndOfEntry => state = EntryState::Skip,
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

fn is_binary(data: &[u8]) -> bool {
    data.iter().take(512).any(|&b| b == 0)
}

fn get_targets(alpm: &Alpm, args: &Args, matcher: &mut Match) -> Result<Vec<String>> {
    let mut download = Vec::new();
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
            if let Ok(pkg) = get_dbpkg(alpm, targ) {
                if pkg.files().files().is_empty() || want_pkg(alpm, pkg, matcher) {
                    repo.push(pkg);
                }
            } else if targ.contains("://") {
                download.push(targ.clone());
            } else if Path::new(&targ).exists() {
                files.push(targ.to_string());
            } else {
                bail!("'{}' is not a package, file or url", targ);
            }
        }
    }

    // todo filter repopkg files

    for pkg in repo {
        download.push(get_download_url(pkg)?);
    }

    let downloaded = alpm.fetch_pkgurl(download.into_iter())?;
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
