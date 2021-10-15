use crate::args::Args;
use alpm::{Alpm, AnyDownloadEvent, DownloadEvent, DownloadResult, LogLevel, Package};
use alpm_utils::{DbListExt, Targ};
use anyhow::{bail, Context, Result};
use clap::Clap;
use compress_tools::{ArchiveContents, ArchiveIterator};
use nix::sys::signal::{signal, SigHandler, Signal};
use nix::unistd::isatty;
use regex::RegexSet;
use std::fs::File;
use std::io::{self, Read, Seek, Write};
use std::iter;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::io::AsRawFd;
use std::path::Path;

mod args;

#[derive(PartialEq, Eq)]
enum EntryState {
    Skip,
    FirstChunk,
    Reading,
}

struct Match<'a> {
    with: MatchWith<'a>,
    exact_file: bool,
}

impl<'a> Match<'a> {
    fn new(regex: bool, files: &'a [&'a str]) -> Result<Self> {
        let exact_file = files.iter().any(|f| f.contains('/'));
        let with = MatchWith::new(regex, files)?;
        Ok(Self { exact_file, with })
    }

    fn is_match(&self, file: &str) -> bool {
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
            MatchWith::Files(f) => f.iter().any(|&t| t == file),
        }
    }
}

enum MatchWith<'a> {
    Regex(RegexSet),
    Files(&'a [&'a str]),
}

impl<'a> MatchWith<'a> {
    fn new(regex: bool, files: &'a [&'a str]) -> Result<Self> {
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

    let files = args
        .files
        .iter()
        .map(|f| f.trim_start_matches('/'))
        .collect::<Vec<_>>();

    let matcher = Match::new(args.regex, &files)?;
    let alpm = alpm_init(&args)?;

    let pkgs = get_targets(&alpm, &args.targets)?;

    for pkg in pkgs {
        let file = File::open(&pkg).with_context(|| format!("failed to open {}", pkg))?;
        let archive = ArchiveIterator::from_read(file)?;
        ret |= dump_files(archive, &matcher, &args)?;
    }

    Ok(ret)
}

fn dump_files<R>(archive: ArchiveIterator<R>, matcher: &Match, args: &Args) -> Result<i32>
where
    R: Read + Seek,
{
    let stdout = io::stdout();
    let mut stdout = stdout.lock();
    let mut state = EntryState::Skip;
    let mut found = 0;
    let mut cur_file = String::new();

    for content in archive {
        match content {
            ArchiveContents::StartOfEntry(file) => {
                if matcher.is_match(&file) {
                    found += 1;
                    if args.quiet {
                        writeln!(stdout, "{}", file)?;
                    } else {
                        cur_file = file;
                        state = EntryState::FirstChunk;
                    }
                }
            }
            ArchiveContents::DataChunk(v) if state == EntryState::FirstChunk => {
                if is_binary(&v) && !args.binary {
                    state = EntryState::Skip;
                    eprintln!("{} is a binary file -- use --binary to print", cur_file)
                } else {
                    stdout.write_all(&v)?
                }
            }
            ArchiveContents::DataChunk(v) if state == EntryState::Reading => {
                stdout.write_all(&v)?
            }
            ArchiveContents::DataChunk(_) => (),
            ArchiveContents::EndOfEntry => state = EntryState::Skip,
            ArchiveContents::Err(e) => {
                return Err(e.into());
            }
        }
    }

    let ret = match matcher.with {
        MatchWith::Files(f) if f.len() as i32 == found => 0,
        MatchWith::Regex(_) if found != 0 => 0,
        _ => 1,
    };

    Ok(ret)
}

fn is_binary(data: &[u8]) -> bool {
    data.iter().take(512).any(|&b| b == 0)
}

fn get_targets(alpm: &Alpm, targs: &[String]) -> Result<Vec<String>> {
    let mut download = Vec::new();
    let mut repo = Vec::new();
    let mut files = Vec::new();

    for targ in targs {
        if let Ok(pkg) = get_dbpkg(alpm, targ) {
            repo.push(pkg);
        } else if targ.contains("://") {
            download.push(targ.clone());
        } else if Path::new(&targ).exists() {
            files.push(targ.to_string());
        } else {
            bail!("'{}' is not a package, file or url", targ);
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

fn get_download_url(pkg: Package) -> Result<String> {
    let server = pkg
        .db()
        .unwrap()
        .servers()
        .first()
        .ok_or(alpm::Error::ServerNone)?;
    let url = format!("{}/{}", server, pkg.filename());
    Ok(url)
}

fn alpm_init(args: &Args) -> Result<Alpm> {
    let conf = pacmanconf::Config::with_opts(None, args.config.as_deref(), args.root.as_deref())?;
    let dbpath = args
        .dbpath
        .as_deref()
        .unwrap_or_else(|| conf.db_path.as_str());
    let mut alpm = Alpm::new(conf.root_dir.as_str(), dbpath)?;
    alpm_utils::configure_alpm(&mut alpm, &conf)?;

    alpm.set_dl_cb((), download_cb);
    alpm.set_log_cb((), log_cb);

    if let Some(dir) = args.cachedir.as_deref() {
        alpm.set_cachedirs(iter::once(dir))?;
    } else {
        alpm.add_cachedir(std::env::temp_dir().as_os_str().as_bytes())?;
    }
    Ok(alpm)
}

fn get_dbpkg<'a>(alpm: &'a Alpm, target_str: &str) -> Result<Package<'a>> {
    let target = Targ::from(target_str);
    let pkg = alpm
        .syncdbs()
        .find_target_satisfier(target)
        .with_context(|| format!("could not find package: {}", target_str))?;
    Ok(pkg)
}

fn download_cb(file: &str, event: AnyDownloadEvent, _: &mut ()) {
    match event.event() {
        DownloadEvent::Init(_) => eprintln!("downloading {}...", file),
        DownloadEvent::Completed(e) => match e.result {
            DownloadResult::Failed => eprintln!("{} failed to download", file),
            DownloadResult::UpToDate => eprintln!("{} is up to date", file),
            _ => (),
        },
        _ => (),
    }
}

fn log_cb(level: LogLevel, msg: &str, _: &mut ()) {
    match level {
        LogLevel::WARNING => eprint!("warning: {}", msg),
        LogLevel::ERROR => eprint!("error: {}", msg),
        _ => (),
    }
}
