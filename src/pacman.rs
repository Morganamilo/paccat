use std::io::{stderr, Write};

use crate::args::Args;
use alpm::SigList;
use alpm::{
    Alpm, AnyDownloadEvent, AnyEvent, DownloadEvent, DownloadResult, Event, LogLevel, Package,
    SigLevel,
};
use alpm_utils::DbListExt;
use alpm_utils::Targ;
use anyhow::anyhow;
use anyhow::{Context, Result};
use nix::unistd::Uid;

pub fn alpm_init(args: &Args) -> Result<Alpm> {
    let mut conf =
        pacmanconf::Config::with_opts(None, args.config.as_deref(), args.root.as_deref())?;
    if let Some(dbpath) = args.dbpath.clone() {
        conf.db_path = dbpath;
    }
    let mut alpm = Alpm::new(conf.root_dir.as_str(), conf.db_path.as_str()).with_context(|| {
        format!(
            "failed to initialize alpm (root: {}, dbpath: {})",
            conf.root_dir.as_str(),
            conf.db_path,
        )
    })?;

    if args.filedb {
        alpm.set_dbext(".files");
    }

    alpm.set_dl_cb((), download_cb);
    alpm.set_log_cb(args.debug, log_cb);
    alpm.set_event_cb(args.refresh != 0, event_cb);

    alpm_utils::configure_alpm(&mut alpm, &conf)?;
    if !Uid::current().is_root() {
        alpm.set_sandbox_user(Option::<&str>::None)?;
    }

    if let Some(dir) = args.cachedir.as_deref() {
        alpm.add_cachedir(dir)?;
    } else {
        let tmp = std::env::temp_dir()
            .join("paccat")
            .to_str()
            .context("tempdir is not a str")?
            .to_string();
        alpm.add_cachedir(tmp)?;
    }

    if args.refresh > 0 {
        writeln!(stderr(), "synchronising package databases...")?;
        let res = alpm.syncdbs_mut().update(args.refresh > 1);

        if !Uid::current().is_root() {
            res.map_err(|e| anyhow!("are you root?").context(e))?;
        }

        res?;
    }

    for db in alpm.syncdbs() {
        db.is_valid()
            .with_context(|| format!("database {}{} is not valid", db.name(), alpm.dbext()))?
    }

    Ok(alpm)
}

pub fn get_dbpkg<'a>(alpm: &'a Alpm, target_str: &str, localdb: bool) -> Result<&'a Package> {
    let pkg = if localdb {
        alpm.localdb().pkg(target_str).ok()
    } else {
        let target = Targ::from(target_str);
        alpm.syncdbs().find_target_satisfier(target)
    };
    let pkg = pkg.with_context(|| format!("could not find package: {}", target_str))?;
    Ok(pkg)
}

pub fn verify_packages<'a, I>(alpm: &Alpm, siglevel: SigLevel, files: I) -> Result<()>
where
    I: IntoIterator<Item = &'a str>,
{
    if !siglevel.contains(SigLevel::PACKAGE) {
        return Ok(());
    }

    let mut siglist = SigList::new();

    for file in files {
        if let Err(e) = alpm
            .pkg_load(file, false, alpm.remote_file_siglevel())?
            .check_signature(&mut siglist)
        {
            if e == alpm::Error::SigMissing && siglevel.contains(SigLevel::PACKAGE_OPTIONAL) {
                continue;
            }

            Err(e).with_context(|| format!("failed to verify package {}", file))?;
        }
    }

    Ok(())
}

pub fn get_download_url(pkg: &Package) -> Result<String> {
    let server = pkg
        .db()
        .unwrap()
        .servers()
        .first()
        .ok_or(alpm::Error::ServerNone)?;
    let url = format!("{}/{}", server, pkg.filename().unwrap_or("unknown"));
    Ok(url)
}

fn download_cb(file: &str, event: AnyDownloadEvent, _: &mut ()) {
    if file.ends_with(".sig") {
        return;
    }

    if let DownloadEvent::Completed(c) = event.event() {
        let _ = match c.result {
            DownloadResult::Success => writeln!(stderr(), "  {} downloaded", file),
            DownloadResult::UpToDate => writeln!(stderr(), "  {} is up to date", file),
            DownloadResult::Failed => writeln!(stderr(), "  {} failed to download", file),
        };
    }
}

fn log_cb(level: LogLevel, msg: &str, &mut debug: &mut bool) {
    match level {
        LogLevel::WARNING => {
            let _ = write!(stderr(), "warning: {}", msg);
        }
        LogLevel::ERROR => {
            let _ = write!(stderr(), "error: {}", msg);
        }
        LogLevel::DEBUG if debug => {
            let _ = write!(stderr(), "debug: {}", msg);
        }
        _ => (),
    }
}

fn event_cb(event: AnyEvent, &mut refresh: &mut bool) {
    match event.event() {
        Event::DatabaseMissing(e) if !refresh => {
            let _ = writeln!(
                stderr(),
                "database file for {} does not exist (use '-Fy 'to download)",
                e.dbname()
            );
        }
        Event::PkgRetrieveStart(_) => {
            let _ = writeln!(stderr(), "downloading packages...");
        }
        _ => (),
    }
}
