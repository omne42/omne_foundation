#![forbid(unsafe_code)]

//! Low-level process/runtime primitives shared by higher-level runtime code.
//!
//! This crate owns platform-specific building blocks for:
//! - probing whether host commands are present and spawnable
//! - executing host commands with captured output and optional sudo-style escalation
//! - configuring spawned commands so they can be cleaned up as a process tree
//! - capturing process-tree cleanup handles/identities from a spawned child
//! - best-effort process-tree termination on Unix and Windows
//!
//! Unix uses per-child process groups. Cleanup capture fails closed unless the spawned child is
//! the leader of its own dedicated process group, so callers cannot accidentally arm cleanup
//! against the parent's process group by skipping setup. Linux and other Unix targets both stop
//! before `killpg` once the original leader identity can no longer be revalidated, so cleanup does
//! not trust a potentially reused PGID.
//!
//! Windows prefers Job Objects. When the current process cannot attach the child to a kill-on-close
//! job, cleanup falls back to best-effort tree cleanup rooted at the captured child PID:
//! `taskkill /T /F` while the leader is still alive, and a descendant sweep after the leader exits.

use std::io;
#[cfg(any(windows, test))]
use std::sync::{Mutex, MutexGuard};

mod command_path;
mod host_command;

pub use command_path::{
    resolve_command_path, resolve_command_path_or_standard_location,
    resolve_command_path_or_standard_location_os, resolve_command_path_os,
};
pub use host_command::{
    HostCommandError, HostCommandExecution, HostCommandOutput, HostCommandRequest,
    HostCommandSudoMode, HostRecipeError, HostRecipeRequest, command_available,
    command_available_os, command_exists, command_exists_os, command_path_exists,
    default_recipe_sudo_mode_for_program, run_host_command, run_host_recipe,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CleanupDisposition {
    TreeTerminationInitiated,
    DirectChildKillRequired,
}

pub fn configure_command_for_process_tree(command: &mut tokio::process::Command) {
    #[cfg(unix)]
    command.process_group(0);

    command.kill_on_drop(true);
}

pub struct ProcessTreeCleanup {
    #[cfg(windows)]
    windows_job: Option<win32job::Job>,
    #[cfg(windows)]
    windows_process_id: Mutex<Option<u32>>,
    #[cfg(unix)]
    unix_process_group: Option<UnixProcessGroupIdentity>,
}

impl ProcessTreeCleanup {
    #[cfg(windows)]
    pub fn new(child: &tokio::process::Child) -> io::Result<Self> {
        Ok(Self {
            windows_job: maybe_attach_windows_kill_job(child)?,
            windows_process_id: Mutex::new(child.id()),
        })
    }

    #[cfg(not(windows))]
    pub fn new(child: &tokio::process::Child) -> io::Result<Self> {
        Ok(Self {
            #[cfg(unix)]
            unix_process_group: capture_unix_process_group_identity(child)?,
        })
    }

    #[cfg(windows)]
    pub fn start_termination(&mut self) -> CleanupDisposition {
        if self.windows_job.take().is_some() {
            take_windows_process_id(&self.windows_process_id);
            CleanupDisposition::TreeTerminationInitiated
        } else {
            CleanupDisposition::DirectChildKillRequired
        }
    }

    #[cfg(not(windows))]
    pub fn start_termination(&mut self) -> CleanupDisposition {
        let _ = self;
        CleanupDisposition::DirectChildKillRequired
    }

    pub fn kill_tree(&self) {
        kill_process_tree(self);
    }
}

#[cfg(unix)]
#[derive(Clone, Copy, Debug)]
struct UnixProcessGroupIdentity {
    leader_pid: rustix::process::Pid,
    process_group_id: rustix::process::Pid,
    #[cfg(target_os = "linux")]
    leader_start_ticks: Option<u64>,
    #[cfg(target_os = "linux")]
    leader_session_id: Option<i32>,
}

#[cfg(all(unix, target_os = "linux"))]
#[derive(Clone, Copy, Debug)]
struct LinuxProcessIdentity {
    process_group_id: i32,
    session_id: i32,
    start_ticks: u64,
}

#[cfg(unix)]
fn capture_unix_process_group_identity(
    child: &tokio::process::Child,
) -> io::Result<Option<UnixProcessGroupIdentity>> {
    let leader_pid = child_process_pid(child).ok_or_else(|| {
        io::Error::other("cannot capture process-tree identity for a child without a pid")
    })?;
    let process_group_id = match rustix::process::getpgid(Some(leader_pid)) {
        Ok(process_group_id) => process_group_id,
        #[cfg(target_os = "linux")]
        // `configure_command_for_process_tree(process_group(0))` makes the child its own process
        // group leader, so Linux can still target the original group by `child pid` even if the
        // leader exits before callers finish capturing cleanup state.
        Err(rustix::io::Errno::SRCH) => leader_pid,
        #[cfg(not(target_os = "linux"))]
        Err(rustix::io::Errno::SRCH) => return Ok(None),
        Err(error) => return Err(io::Error::from(error)),
    };
    ensure_unix_process_group_is_dedicated(leader_pid, process_group_id)?;
    Ok(Some(UnixProcessGroupIdentity {
        leader_pid,
        process_group_id,
        #[cfg(target_os = "linux")]
        leader_start_ticks: match read_linux_process_identity(leader_pid) {
            Ok(identity) => Some(identity.start_ticks),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => return Err(error),
        },
        #[cfg(target_os = "linux")]
        leader_session_id: match read_linux_process_identity(leader_pid) {
            Ok(identity) => Some(identity.session_id),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => return Err(error),
        },
    }))
}

#[cfg(unix)]
fn child_process_pid(child: &tokio::process::Child) -> Option<rustix::process::Pid> {
    let raw_pid = i32::try_from(child.id()?).ok()?;
    rustix::process::Pid::from_raw(raw_pid)
}

#[cfg(unix)]
fn ensure_unix_process_group_is_dedicated(
    leader_pid: rustix::process::Pid,
    process_group_id: rustix::process::Pid,
) -> io::Result<()> {
    if process_group_id == leader_pid {
        return Ok(());
    }

    Err(io::Error::new(
        io::ErrorKind::InvalidInput,
        "process-tree cleanup requires the child to lead a dedicated process group; call configure_command_for_process_tree before spawning",
    ))
}

#[cfg(unix)]
fn kill_process_tree(cleanup: &ProcessTreeCleanup) {
    use rustix::io::Errno;
    use rustix::process::{Signal, kill_process_group};

    let Some(identity) = cleanup.unix_process_group else {
        return;
    };
    if !should_kill_unix_process_group(identity) {
        return;
    }

    match kill_process_group(identity.process_group_id, Signal::KILL) {
        Ok(()) | Err(Errno::SRCH) => {}
        Err(_) => {}
    }
}

#[cfg(all(unix, not(target_os = "linux")))]
fn should_kill_unix_process_group(identity: UnixProcessGroupIdentity) -> bool {
    match rustix::process::getpgid(Some(identity.leader_pid)) {
        Ok(current) => current == identity.process_group_id,
        Err(_) => false,
    }
}

#[cfg(target_os = "linux")]
fn should_kill_unix_process_group(identity: UnixProcessGroupIdentity) -> bool {
    should_kill_linux_process_group_with_group_probe(
        identity,
        read_linux_process_identity(identity.leader_pid),
        || linux_process_group_has_matching_member(identity),
    )
}

#[cfg(target_os = "linux")]
fn should_kill_linux_process_group_with_group_probe<F>(
    identity: UnixProcessGroupIdentity,
    current: io::Result<LinuxProcessIdentity>,
    group_exists: F,
) -> bool
where
    F: FnOnce() -> bool,
{
    match current {
        Ok(current) => {
            identity.leader_start_ticks.is_some_and(|start_ticks| {
                current.start_ticks == start_ticks
                    && current.process_group_id == identity.process_group_id.as_raw_pid()
            }) || (identity.leader_start_ticks.is_some() && group_exists())
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => group_exists(),
        Err(_) => false,
    }
}

#[cfg(all(unix, target_os = "linux"))]
fn linux_process_group_has_matching_member(identity: UnixProcessGroupIdentity) -> bool {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return false;
    };
    let target_group_id = identity.process_group_id.as_raw_pid();

    for entry in entries {
        let Ok(entry) = entry else {
            continue;
        };
        let file_name = entry.file_name();
        let Some(raw_pid) = file_name.to_str() else {
            continue;
        };
        let Ok(raw_pid) = raw_pid.parse::<i32>() else {
            continue;
        };
        let Some(pid) = rustix::process::Pid::from_raw(raw_pid) else {
            continue;
        };

        match read_linux_process_identity(pid) {
            Ok(process) if process.process_group_id == target_group_id => {
                if identity.leader_session_id.is_none() {
                    return true;
                }
                if Some(process.session_id) == identity.leader_session_id
                    && pid != identity.leader_pid
                {
                    return true;
                }
            }
            Ok(_) | Err(_) => {}
        }
    }

    false
}

#[cfg(all(unix, target_os = "linux"))]
fn read_linux_process_identity(pid: rustix::process::Pid) -> io::Result<LinuxProcessIdentity> {
    let stat = std::fs::read_to_string(format!("/proc/{}/stat", pid.as_raw_pid()))?;
    let tail = stat
        .rsplit_once(") ")
        .map(|(_, tail)| tail)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "invalid /proc stat"))?;
    let mut fields = tail.split_whitespace();
    let _state = fields
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing proc state"))?;
    let _parent_pid = fields
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing proc parent pid"))?;
    let process_group_id = parse_proc_stat_i32(fields.next(), "missing proc group id")?;
    let session_id = parse_proc_stat_i32(fields.next(), "missing proc session id")?;
    for _ in 0..15 {
        let _ = fields
            .next()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing proc stat field"))?;
    }
    let start_ticks = parse_proc_stat_u64(fields.next(), "missing proc start time")?;
    Ok(LinuxProcessIdentity {
        process_group_id,
        session_id,
        start_ticks,
    })
}

#[cfg(all(unix, target_os = "linux"))]
fn parse_proc_stat_i32(raw: Option<&str>, message: &'static str) -> io::Result<i32> {
    raw.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, message))?
        .parse::<i32>()
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))
}

#[cfg(all(unix, target_os = "linux"))]
fn parse_proc_stat_u64(raw: Option<&str>, message: &'static str) -> io::Result<u64> {
    raw.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, message))?
        .parse::<u64>()
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))
}

#[cfg(all(not(windows), not(unix)))]
fn kill_process_tree(_cleanup: &ProcessTreeCleanup) {}

#[cfg(windows)]
fn kill_process_tree(cleanup: &ProcessTreeCleanup) {
    kill_windows_process_tree(
        cleanup.windows_job.is_some(),
        &cleanup.windows_process_id,
        windows_taskkill_tree,
        kill_windows_orphan_descendants,
    );
}

#[cfg(any(windows, test))]
fn take_windows_process_id(process_id: &Mutex<Option<u32>>) -> Option<u32> {
    lock_windows_process_id(process_id).take()
}

#[cfg(any(windows, test))]
fn lock_windows_process_id(process_id: &Mutex<Option<u32>>) -> MutexGuard<'_, Option<u32>> {
    process_id
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(any(windows, test))]
fn kill_windows_process_tree<F, G>(
    has_windows_job: bool,
    process_id: &Mutex<Option<u32>>,
    taskkill_tree: F,
    fallback: G,
) where
    F: FnOnce(u32) -> io::Result<()>,
    G: FnOnce(u32),
{
    if has_windows_job {
        return;
    }

    let Some(pid) = take_windows_process_id(process_id) else {
        return;
    };

    if taskkill_tree(pid).is_err() {
        fallback(pid);
    }
}

#[cfg(windows)]
fn windows_taskkill_program() -> std::path::PathBuf {
    std::env::var_os("SystemRoot")
        .or_else(|| std::env::var_os("WINDIR"))
        .map(|root| {
            std::path::PathBuf::from(root)
                .join("System32")
                .join("taskkill.exe")
        })
        .unwrap_or_else(|| std::path::PathBuf::from("taskkill"))
}

#[cfg(windows)]
fn windows_taskkill_tree(pid: u32) -> io::Result<()> {
    let snapshot = windows_process_snapshot();
    if !snapshot_contains_pid(&snapshot, pid) {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "root process already exited before taskkill",
        ));
    }

    let status = std::process::Command::new(windows_taskkill_program())
        .args(["/T", "/F", "/PID", &pid.to_string()])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()?;
    if status.success() {
        return Ok(());
    }

    Err(io::Error::other(format!(
        "taskkill exited unsuccessfully for pid {pid}: {status}"
    )))
}

#[cfg(windows)]
fn kill_windows_orphan_descendants(root_pid: u32) {
    use sysinfo::Pid;

    let snapshot = windows_process_snapshot();
    let descendants = collect_descendant_pids(
        snapshot
            .processes()
            .iter()
            .map(|(pid, process)| (pid.as_u32(), process.parent().map(|parent| parent.as_u32()))),
        root_pid,
    );

    for pid in descendants {
        if let Some(process) = snapshot.process(Pid::from_u32(pid)) {
            let _ = process.kill();
        }
    }
}

#[cfg(windows)]
fn windows_process_snapshot() -> sysinfo::System {
    use sysinfo::{ProcessRefreshKind, RefreshKind, System};

    System::new_with_specifics(RefreshKind::nothing().with_processes(ProcessRefreshKind::nothing()))
}

#[cfg(windows)]
fn snapshot_contains_pid(snapshot: &sysinfo::System, pid: u32) -> bool {
    use sysinfo::Pid;

    snapshot.process(Pid::from_u32(pid)).is_some()
}

#[cfg(any(windows, test))]
fn collect_descendant_pids(
    processes: impl IntoIterator<Item = (u32, Option<u32>)>,
    root_pid: u32,
) -> Vec<u32> {
    use std::collections::{BTreeMap, BTreeSet};

    let mut children_by_parent = BTreeMap::<u32, Vec<u32>>::new();
    for (pid, parent) in processes {
        if let Some(parent) = parent {
            children_by_parent.entry(parent).or_default().push(pid);
        }
    }

    let mut descendants = Vec::new();
    let mut stack = vec![(root_pid, false)];
    let mut visited = BTreeSet::new();

    while let Some((pid, expanded)) = stack.pop() {
        if expanded {
            if pid != root_pid {
                descendants.push(pid);
            }
            continue;
        }

        if !visited.insert(pid) {
            continue;
        }

        stack.push((pid, true));
        if let Some(children) = children_by_parent.get(&pid) {
            for &child in children.iter().rev() {
                stack.push((child, false));
            }
        }
    }

    descendants
}

#[cfg(test)]
mod descendant_tests {
    use std::io;
    use std::sync::Mutex;

    use super::{collect_descendant_pids, kill_windows_process_tree};

    #[test]
    fn collect_descendant_pids_returns_postorder_descendants_only() {
        let processes = [
            (10, None),
            (11, Some(10)),
            (12, Some(10)),
            (13, Some(11)),
            (14, Some(13)),
            (20, None),
        ];

        assert_eq!(collect_descendant_pids(processes, 10), vec![14, 13, 11, 12]);
    }

    #[test]
    fn collect_descendant_pids_ignores_unrelated_cycles() {
        let processes = [(10, None), (11, Some(10)), (21, Some(22)), (22, Some(21))];

        assert_eq!(collect_descendant_pids(processes, 10), vec![11]);
    }

    #[test]
    fn windows_fallback_runs_when_taskkill_returns_error() {
        let process_id = Mutex::new(Some(42));
        let mut taskkill_attempts = Vec::new();
        let mut fallback_pids = Vec::new();

        kill_windows_process_tree(
            false,
            &process_id,
            |pid| {
                taskkill_attempts.push(pid);
                Err(io::Error::other("taskkill failed"))
            },
            |pid| fallback_pids.push(pid),
        );

        assert_eq!(taskkill_attempts, vec![42]);
        assert_eq!(fallback_pids, vec![42]);
        assert_eq!(*process_id.lock().expect("lock pid"), None);
    }

    #[test]
    fn windows_fallback_skips_when_taskkill_succeeds() {
        let process_id = Mutex::new(Some(42));
        let mut taskkill_attempts = Vec::new();
        let mut fallback_pids = Vec::new();

        kill_windows_process_tree(
            false,
            &process_id,
            |pid| {
                taskkill_attempts.push(pid);
                Ok(())
            },
            |pid| fallback_pids.push(pid),
        );

        assert_eq!(taskkill_attempts, vec![42]);
        assert!(fallback_pids.is_empty());
        assert_eq!(*process_id.lock().expect("lock pid"), None);
    }
}

#[cfg(windows)]
fn maybe_attach_windows_kill_job(
    child: &tokio::process::Child,
) -> io::Result<Option<win32job::Job>> {
    use win32job::{ExtendedLimitInfo, Job};

    let Some(process_handle) = child.raw_handle() else {
        return Ok(None);
    };
    if process_handle.is_null() {
        return Ok(None);
    }

    let job = Job::create().map_err(io::Error::from)?;
    let mut limits = ExtendedLimitInfo::new();
    limits.limit_kill_on_job_close();
    job.set_extended_limit_info(&limits)
        .map_err(io::Error::from)?;

    match job.assign_process(process_handle as isize) {
        Ok(()) => Ok(Some(job)),
        Err(error) => {
            let error = io::Error::from(error);
            match error.raw_os_error() {
                Some(WINDOWS_ERROR_ACCESS_DENIED) | Some(WINDOWS_ERROR_NOT_SUPPORTED) => Ok(None),
                _ => Err(error),
            }
        }
    }
}

#[cfg(windows)]
const WINDOWS_ERROR_ACCESS_DENIED: i32 = 5;

#[cfg(windows)]
const WINDOWS_ERROR_NOT_SUPPORTED: i32 = 50;

#[cfg(all(test, unix, target_os = "linux"))]
mod tests {
    use super::{
        CleanupDisposition, LinuxProcessIdentity, ProcessTreeCleanup, UnixProcessGroupIdentity,
        configure_command_for_process_tree, ensure_unix_process_group_is_dedicated,
        should_kill_linux_process_group_with_group_probe,
    };
    use rustix::process::Pid;
    use std::io;
    use std::path::Path;
    use std::process::Stdio;
    use std::time::Duration;

    #[test]
    fn reused_leader_pid_still_allows_killing_surviving_linux_process_group() {
        let identity = UnixProcessGroupIdentity {
            leader_pid: Pid::from_raw(4242).expect("leader pid must be non-zero"),
            process_group_id: Pid::from_raw(31337).expect("process group id must be non-zero"),
            leader_start_ticks: Some(11),
            leader_session_id: Some(7),
        };

        assert!(should_kill_linux_process_group_with_group_probe(
            identity,
            Ok(LinuxProcessIdentity {
                process_group_id: 9999,
                session_id: 7,
                start_ticks: 22,
            }),
            || true,
        ));
    }

    #[test]
    fn reused_leader_pid_fails_closed_when_linux_process_group_is_gone() {
        let identity = UnixProcessGroupIdentity {
            leader_pid: Pid::from_raw(4242).expect("leader pid must be non-zero"),
            process_group_id: Pid::from_raw(31337).expect("process group id must be non-zero"),
            leader_start_ticks: Some(11),
            leader_session_id: Some(7),
        };

        assert!(!should_kill_linux_process_group_with_group_probe(
            identity,
            Err(io::ErrorKind::NotFound.into()),
            || false,
        ));
    }

    #[test]
    fn orphaned_group_is_killed_when_leader_exited_before_capture_completed() {
        let identity = UnixProcessGroupIdentity {
            leader_pid: Pid::from_raw(4242).expect("leader pid must be non-zero"),
            process_group_id: Pid::from_raw(31337).expect("process group id must be non-zero"),
            leader_start_ticks: None,
            leader_session_id: None,
        };

        assert!(should_kill_linux_process_group_with_group_probe(
            identity,
            Err(io::ErrorKind::NotFound.into()),
            || true,
        ));
    }

    #[test]
    fn reused_leader_pid_still_fails_closed_when_capture_lost_start_ticks() {
        let identity = UnixProcessGroupIdentity {
            leader_pid: Pid::from_raw(4242).expect("leader pid must be non-zero"),
            process_group_id: Pid::from_raw(31337).expect("process group id must be non-zero"),
            leader_start_ticks: None,
            leader_session_id: None,
        };

        assert!(!should_kill_linux_process_group_with_group_probe(
            identity,
            Ok(LinuxProcessIdentity {
                process_group_id: 31337,
                session_id: 7,
                start_ticks: 22,
            }),
            || true,
        ));
    }

    #[test]
    fn leader_exit_after_capture_still_allows_killing_surviving_linux_process_group() {
        let identity = UnixProcessGroupIdentity {
            leader_pid: Pid::from_raw(4242).expect("leader pid must be non-zero"),
            process_group_id: Pid::from_raw(31337).expect("process group id must be non-zero"),
            leader_start_ticks: Some(11),
            leader_session_id: Some(7),
        };

        assert!(should_kill_linux_process_group_with_group_probe(
            identity,
            Err(io::ErrorKind::NotFound.into()),
            || true,
        ));
    }

    #[test]
    fn dedicated_process_group_validation_rejects_shared_group() {
        let leader_pid = Pid::from_raw(4242).expect("leader pid must be non-zero");
        let shared_group = Pid::from_raw(77).expect("shared group id must be non-zero");

        let err = ensure_unix_process_group_is_dedicated(leader_pid, shared_group)
            .expect_err("shared process group must be rejected");
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        assert!(
            err.to_string()
                .contains("configure_command_for_process_tree"),
            "unexpected error: {err}"
        );
    }

    fn process_terminated_or_zombie(pid: u32) -> bool {
        let status_path = format!("/proc/{pid}/status");
        match std::fs::read_to_string(status_path) {
            Ok(status) => status
                .lines()
                .find(|line| line.starts_with("State:"))
                .map(|line| line.contains("\tZ") || line.contains(" zombie"))
                .unwrap_or(false),
            Err(err) => err.kind() == io::ErrorKind::NotFound,
        }
    }

    async fn wait_for_pid(path: &Path) -> Option<u32> {
        for _ in 0..100 {
            if let Ok(raw) = tokio::fs::read_to_string(path).await
                && let Ok(pid) = raw.trim().parse::<u32>()
            {
                return Some(pid);
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        None
    }

    #[tokio::test]
    async fn cleanup_kills_child_process_group() -> io::Result<()> {
        let dir = tempfile::tempdir()?;
        let pid_file = dir.path().join("background.pid");
        let script = format!("sleep 30 & echo $! > '{}'; wait", pid_file.display());

        let mut command = tokio::process::Command::new("sh");
        command
            .arg("-c")
            .arg(script)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        configure_command_for_process_tree(&mut command);

        let mut child = command.spawn()?;
        let mut cleanup = ProcessTreeCleanup::new(&child)?;
        let pid = wait_for_pid(&pid_file)
            .await
            .expect("background pid file should be written");

        assert_eq!(
            cleanup.start_termination(),
            CleanupDisposition::DirectChildKillRequired
        );
        cleanup.kill_tree();
        let _ = child.kill().await;
        let _ = child.wait().await;

        let mut gone = false;
        for _ in 0..300 {
            if process_terminated_or_zombie(pid) {
                gone = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        assert!(gone, "background process group should be terminated");
        Ok(())
    }

    #[tokio::test]
    async fn cleanup_kills_orphaned_process_group_after_linux_leader_exit() -> io::Result<()> {
        let dir = tempfile::tempdir()?;
        let shell_pid_file = dir.path().join("shell.pid");
        let bg_pid_file = dir.path().join("background.pid");
        let script = format!(
            "echo $$ > '{shell}'; sleep 30 & echo $! > '{background}'; exit 0",
            shell = shell_pid_file.display(),
            background = bg_pid_file.display()
        );

        let mut command = tokio::process::Command::new("sh");
        command
            .arg("-c")
            .arg(script)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        configure_command_for_process_tree(&mut command);

        let mut child = command.spawn()?;
        let shell_pid = wait_for_pid(&shell_pid_file)
            .await
            .expect("shell pid file should be written");
        let bg_pid = wait_for_pid(&bg_pid_file)
            .await
            .expect("background pid file should be written");

        let mut leader_exited = false;
        for _ in 0..300 {
            if process_terminated_or_zombie(shell_pid) {
                leader_exited = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(leader_exited, "shell leader should exit before cleanup");

        let mut cleanup = ProcessTreeCleanup::new(&child)?;
        assert!(
            cleanup.unix_process_group.is_some(),
            "linux cleanup should keep orphan-group state after the leader exits"
        );
        assert_eq!(
            cleanup.start_termination(),
            CleanupDisposition::DirectChildKillRequired
        );
        cleanup.kill_tree();
        let _ = child.kill().await;
        let _ = child.wait().await;

        let mut gone = false;
        for _ in 0..300 {
            if process_terminated_or_zombie(bg_pid) {
                gone = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        assert!(
            gone,
            "linux cleanup should still kill orphaned process groups after the leader exits"
        );
        Ok(())
    }

    #[tokio::test]
    async fn cleanup_kills_orphaned_process_group_when_leader_exits_after_capture() -> io::Result<()>
    {
        let dir = tempfile::tempdir()?;
        let shell_pid_file = dir.path().join("shell.pid");
        let bg_pid_file = dir.path().join("background.pid");
        let script = format!(
            "echo $$ > '{shell}'; sleep 30 & echo $! > '{background}'; sleep 0.2; exit 0",
            shell = shell_pid_file.display(),
            background = bg_pid_file.display()
        );

        let mut command = tokio::process::Command::new("sh");
        command
            .arg("-c")
            .arg(script)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        configure_command_for_process_tree(&mut command);

        let mut child = command.spawn()?;
        let mut cleanup = ProcessTreeCleanup::new(&child)?;
        let shell_pid = wait_for_pid(&shell_pid_file)
            .await
            .expect("shell pid file should be written");
        let bg_pid = wait_for_pid(&bg_pid_file)
            .await
            .expect("background pid file should be written");

        let mut leader_exited = false;
        for _ in 0..1000 {
            if process_terminated_or_zombie(shell_pid) {
                leader_exited = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(
            leader_exited,
            "shell leader should exit after cleanup capture"
        );

        assert_eq!(
            cleanup.start_termination(),
            CleanupDisposition::DirectChildKillRequired
        );
        cleanup.kill_tree();
        let _ = child.kill().await;
        let _ = child.wait().await;

        let mut gone = false;
        for _ in 0..1000 {
            if process_terminated_or_zombie(bg_pid) {
                gone = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        assert!(
            gone,
            "linux cleanup should still kill orphaned process groups when the leader exits after capture"
        );
        Ok(())
    }

    #[tokio::test]
    async fn cleanup_kills_child_process_group_even_if_leader_dies_before_kill_tree()
    -> io::Result<()> {
        let dir = tempfile::tempdir()?;
        let pid_file = dir.path().join("background.pid");
        let script = format!("sleep 30 & echo $! > '{}'; wait", pid_file.display());

        let mut command = tokio::process::Command::new("sh");
        command
            .arg("-c")
            .arg(script)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        configure_command_for_process_tree(&mut command);

        let mut child = command.spawn()?;
        let mut cleanup = ProcessTreeCleanup::new(&child)?;
        let pid = wait_for_pid(&pid_file)
            .await
            .expect("background pid file should be written");

        assert_eq!(
            cleanup.start_termination(),
            CleanupDisposition::DirectChildKillRequired
        );
        let _ = child.kill().await;
        let _ = child.wait().await;
        cleanup.kill_tree();

        let mut gone = false;
        for _ in 0..1000 {
            if process_terminated_or_zombie(pid) {
                gone = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        assert!(
            gone,
            "background process group should still be terminated after the leader dies before kill_tree"
        );
        Ok(())
    }

    #[tokio::test]
    async fn cleanup_rejects_child_without_dedicated_process_group() -> io::Result<()> {
        let mut command = tokio::process::Command::new("sh");
        command
            .arg("-c")
            .arg("sleep 30")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());

        let mut child = command.spawn()?;
        let err = ProcessTreeCleanup::new(&child)
            .err()
            .expect("non-isolated child must not arm process-tree cleanup");
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        let _ = child.kill().await;
        let _ = child.wait().await;
        Ok(())
    }
}

#[cfg(all(test, unix, not(target_os = "linux")))]
mod unix_tests {
    use super::{CleanupDisposition, ProcessTreeCleanup, configure_command_for_process_tree};
    use rustix::io::Errno;
    use rustix::process::{Pid, Signal, kill_process, test_kill_process_group};
    use std::io;
    use std::path::Path;
    use std::process::Stdio;
    use std::time::Duration;

    async fn wait_for_pid(path: &Path) -> Option<u32> {
        for _ in 0..100 {
            if let Ok(raw) = tokio::fs::read_to_string(path).await
                && let Ok(pid) = raw.trim().parse::<u32>()
            {
                return Some(pid);
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        None
    }

    fn process_group_gone(process_group: Pid) -> bool {
        matches!(test_kill_process_group(process_group), Err(Errno::SRCH))
    }

    fn pid_to_process_group(pid: u32) -> Pid {
        Pid::from_raw(i32::try_from(pid).expect("pid should fit in i32"))
            .expect("process group id must be non-zero")
    }

    #[tokio::test]
    async fn cleanup_kills_child_process_group() -> io::Result<()> {
        let dir = tempfile::tempdir()?;
        let pid_file = dir.path().join("background.pid");
        let script = format!("sleep 30 & echo $! > '{}'; wait", pid_file.display());

        let mut command = tokio::process::Command::new("sh");
        command
            .arg("-c")
            .arg(script)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        configure_command_for_process_tree(&mut command);

        let mut child = command.spawn()?;
        let mut cleanup = ProcessTreeCleanup::new(&child)?;
        let process_group = pid_to_process_group(child.id().expect("child pid should exist"));
        let _bg_pid = wait_for_pid(&pid_file)
            .await
            .expect("background pid file should be written");

        assert_eq!(
            cleanup.start_termination(),
            CleanupDisposition::DirectChildKillRequired
        );
        cleanup.kill_tree();
        tokio::time::timeout(Duration::from_secs(5), child.wait())
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "child did not exit in time"))??;

        let mut gone = false;
        for _ in 0..300 {
            if process_group_gone(process_group) {
                gone = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        assert!(gone, "background process group should be terminated");
        Ok(())
    }

    #[tokio::test]
    async fn cleanup_does_not_kill_orphaned_process_group_after_leader_exit() -> io::Result<()> {
        let dir = tempfile::tempdir()?;
        let shell_pid_file = dir.path().join("shell.pid");
        let bg_pid_file = dir.path().join("background.pid");
        let script = format!(
            "echo $$ > '{shell}'; sleep 30 & echo $! > '{background}'; exit 0",
            shell = shell_pid_file.display(),
            background = bg_pid_file.display()
        );

        let mut command = tokio::process::Command::new("sh");
        command
            .arg("-c")
            .arg(script)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        configure_command_for_process_tree(&mut command);

        let mut child = command.spawn()?;
        let mut cleanup = ProcessTreeCleanup::new(&child)?;
        let shell_pid = wait_for_pid(&shell_pid_file)
            .await
            .expect("shell pid file should be written");
        let process_group = pid_to_process_group(shell_pid);
        let bg_pid = wait_for_pid(&bg_pid_file)
            .await
            .expect("background pid file should be written");
        let background_pid =
            Pid::from_raw(i32::try_from(bg_pid).expect("background pid should fit in i32"))
                .expect("background pid must be non-zero");

        tokio::time::timeout(Duration::from_secs(5), child.wait())
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "shell did not exit in time"))??;

        assert_eq!(
            cleanup.start_termination(),
            CleanupDisposition::DirectChildKillRequired
        );
        cleanup.kill_tree();

        let mut still_present = false;
        for _ in 0..300 {
            if !process_group_gone(process_group) {
                still_present = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        assert!(
            still_present,
            "cleanup must fail closed instead of killing an orphaned process group after the leader exits"
        );

        let _ = kill_process(background_pid, Signal::KILL);
        for _ in 0..300 {
            if process_group_gone(process_group) {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "background process group did not exit after explicit cleanup",
        ))
    }
}

#[cfg(all(test, windows))]
mod windows_tests {
    use super::{CleanupDisposition, ProcessTreeCleanup, configure_command_for_process_tree};
    use std::io;
    use std::process::Stdio;
    use std::time::Duration;

    #[tokio::test]
    async fn cleanup_terminates_direct_child_or_attached_job() -> io::Result<()> {
        let mut command = tokio::process::Command::new("cmd");
        command
            .arg("/C")
            .arg("ping -n 30 127.0.0.1 >NUL")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        configure_command_for_process_tree(&mut command);

        let mut child = command.spawn()?;
        let mut cleanup = ProcessTreeCleanup::new(&child)?;
        let disposition = cleanup.start_termination();
        cleanup.kill_tree();
        if matches!(disposition, CleanupDisposition::DirectChildKillRequired) {
            let _ = child.kill().await;
        }

        tokio::time::timeout(Duration::from_secs(5), child.wait())
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "child did not exit in time"))??;
        Ok(())
    }

    #[tokio::test]
    async fn cleanup_is_safe_after_child_exit() -> io::Result<()> {
        let mut command = tokio::process::Command::new("cmd");
        command
            .arg("/C")
            .arg("exit /B 0")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        configure_command_for_process_tree(&mut command);

        let mut child = command.spawn()?;
        let mut cleanup = ProcessTreeCleanup::new(&child)?;
        let _ = child.wait().await?;

        let _ = cleanup.start_termination();
        cleanup.kill_tree();
        Ok(())
    }

    #[tokio::test]
    async fn cleanup_allows_repeated_termination_requests() -> io::Result<()> {
        let mut command = tokio::process::Command::new("cmd");
        command
            .arg("/C")
            .arg("ping -n 30 127.0.0.1 >NUL")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        configure_command_for_process_tree(&mut command);

        let mut child = command.spawn()?;
        let mut cleanup = ProcessTreeCleanup::new(&child)?;

        let _ = cleanup.start_termination();
        cleanup.kill_tree();
        cleanup.kill_tree();

        let _ = cleanup.start_termination();
        cleanup.kill_tree();

        let _ = child.kill().await;
        let _ = child.wait().await?;
        Ok(())
    }
}
