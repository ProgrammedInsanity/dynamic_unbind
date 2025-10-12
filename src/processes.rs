use log_derive::logfn;
use std::path::{Path, PathBuf};
use sysinfo::Pid;
use tracing::instrument;

use procfs::process::{FDTarget, all_processes};

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("Could not kill these processes: {processes:#?}")]
    CouldNotKillAllProcesses { processes: Vec<u32> },
}

/// Error means that the target_file does not exist.
#[instrument]
#[logfn(err = "Error")]
fn get_pids_via_procfs(target_file: &Path) -> anyhow::Result<Vec<sysinfo::Pid>> {
    let absolute_target_file = target_file.canonicalize()?;
    Ok(all_processes()
        .expect("Failed to iterate /proc processes")
        .filter_map(|proc| {
            let proc = proc.ok()?;
            proc.fd()
                .ok()?
                .flatten()
                .any(|fd| match fd.target {
                    FDTarget::Path(path) => {
                        if let Ok(path) = path.canonicalize()
                            && path == absolute_target_file
                        {
                            true
                        } else {
                            false
                        }
                    }
                    _ => false,
                })
                .then_some(Pid::from_u32(proc.pid.abs().cast_unsigned()))
        })
        .collect())
}

#[instrument]
pub fn kill_processes_using_files(files: &Vec<&PathBuf>) -> Result<(), Error> {
    use std::thread;
    use std::time::Duration;
    use sysinfo::{Signal, System};
    let mut s = System::new_all();

    let pids: Vec<Pid> = files
        .iter()
        .flat_map(|file| get_pids_via_procfs(file))
        .flatten()
        .collect();

    log::info!(
        "Gracefully killing these processes: {:#?}",
        pids.iter()
            .map(|pid| format!(
                "{}:'{}'",
                pid.as_u32(),
                s.process(*pid)
                    .map_or("Could not get name".to_string(), |process| process
                        .name()
                        .to_string_lossy()
                        .to_string())
            ))
            .collect::<Vec<String>>()
    );

    pids.iter()
        .flat_map(|pid| s.process(*pid))
        .for_each(|proc| {
            proc.kill_with(Signal::Term);
        });

    // give some time to gracefully shutdown
    thread::sleep(Duration::from_secs(3));

    // This is for processes that restart after being killed
    const TRIES: u32 = 30;
    for try_of_kill in 1..=TRIES {
        let pids: Vec<Pid> = files
            .iter()
            .flat_map(|file| get_pids_via_procfs(file))
            .flatten()
            .collect();

        s.refresh_processes_specifics(
            sysinfo::ProcessesToUpdate::Some(&pids),
            true,
            sysinfo::ProcessRefreshKind::everything().without_tasks(),
        );

        if pids.is_empty() {
            break;
        }

        log::info!(
            "Force killing these processes try:{try_of_kill}/{TRIES}: {:#?}",
            pids.iter()
                .map(|pid| format!(
                    "{}:'{}'",
                    pid.as_u32(),
                    s.process(*pid)
                        .map_or("Could not get name".to_string(), |process| process
                            .name()
                            .to_os_string()
                            .to_str()
                            .expect("Process name is not valid UTF-8")
                            .to_string())
                ))
                .collect::<Vec<String>>()
        );

        pids.iter()
            .flat_map(|pid| s.process(*pid))
            .for_each(|proc| {
                proc.kill_with(Signal::Kill);
            });

        thread::sleep(Duration::from_secs(1));
    }

    let pids: Vec<Pid> = files
        .iter()
        .flat_map(|file| get_pids_via_procfs(file))
        .flatten()
        .collect();

    if pids.is_empty() {
        Ok(())
    } else {
        Err(Error::CouldNotKillAllProcesses {
            processes: pids.iter().map(|pid| pid.as_u32()).collect(),
        })
    }
}
