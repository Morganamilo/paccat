use crate::args::Args;
use alpm::{
    Alpm, AnyDownloadEvent, AnyEvent, DownloadEvent, DownloadResult, Event, LogLevel, Package,
};
use alpm_utils::DbListExt;
use alpm_utils::Targ;
use anyhow::{Context, Result};
use std::iter;
use std::os::unix::ffi::OsStrExt;

pub fn alpm_init(args: &Args) -> Result<Alpm> {
    let conf = pacmanconf::Config::with_opts(None, args.config.as_deref(), args.root.as_deref())?;
    let dbpath = args
        .dbpath
        .as_deref()
        .unwrap_or_else(|| conf.db_path.as_str());
    let mut alpm = Alpm::new(conf.root_dir.as_str(), dbpath)?;

    if args.filedb {
        alpm.set_dbext(".files");
    }

    alpm.set_dl_cb((), download_cb);
    alpm.set_log_cb((), log_cb);
    alpm.set_event_cb((), event_cb);

    alpm_utils::configure_alpm(&mut alpm, &conf)?;

    if let Some(dir) = args.cachedir.as_deref() {
        alpm.set_cachedirs(iter::once(dir))?;
    } else {
        alpm.add_cachedir(std::env::temp_dir().join("paccat").as_os_str().as_bytes())?;
    }

    if args.refresh > 0 {
        eprintln!("synchronising package databases...");
        alpm.syncdbs_mut().update(args.refresh > 1)?;
    }
    Ok(alpm)
}

pub fn get_dbpkg<'a>(alpm: &'a Alpm, target_str: &str) -> Result<Package<'a>> {
    let target = Targ::from(target_str);
    let pkg = alpm
        .syncdbs()
        .find_target_satisfier(target)
        .with_context(|| format!("could not find package: {}", target_str))?;
    Ok(pkg)
}

pub fn get_download_url(pkg: Package) -> Result<String> {
    let server = pkg
        .db()
        .unwrap()
        .servers()
        .first()
        .ok_or(alpm::Error::ServerNone)?;
    let url = format!("{}/{}", server, pkg.filename());
    Ok(url)
}

fn download_cb(file: &str, event: AnyDownloadEvent, _: &mut ()) {
    if file.ends_with(".sig") {
        return;
    }

    match event.event() {
        DownloadEvent::Completed(c) => match c.result {
            DownloadResult::Success => eprintln!("{} downloaded", file),
            DownloadResult::UpToDate => eprintln!("{} is up to date", file),
            DownloadResult::Failed => eprintln!("{} failed to download", file),
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

fn event_cb(event: AnyEvent, _: &mut ()) {
    if let Event::DatabaseMissing(e) = event.event() {
        eprintln!(
            "database file for {} does not exist (use pacman to download)",
            e.dbname()
        );
    }
}
