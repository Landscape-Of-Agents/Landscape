// When to upload evidence to the DataSea ingestion service. Data leaves the machine at the moment it is
// complete: a tool call when it ends (post-tool hook), a session when it ends (stop and session-end
// hooks), a governance or other Crane command when it finishes, and everything else periodically
// (any Crane run or hook starts a sync when the last attempt is older than CRANE_SYNC_SECONDS). A sync runs as a detached 'crane agent sync' process with its
// output discarded, so a hook or command never waits for the network and a slow or unreachable
// service never delays or weakens enforcement. Nothing has to be configured: see ingest.rs.

use std::process::{Command, Stdio};

use super::{ingest, Stores};
use crate::governance::workspace::Workspace;
use crate::platform::now_millis;

/** Default interval of the periodic sync, in seconds */
pub(crate) const DEFAULT_INTERVAL_SECONDS: u64 = 60;

/** Report whether uploads are on (the upload key is created on first use, so nothing else is
 * needed)
 * Input
    - None
 * Output
    - bool
*/
pub(crate) fn configured() -> bool {
    ingest::endpoint().is_some()
}

/** Return the periodic sync interval: CRANE_SYNC_SECONDS, at least 5
 * Input
    - None
 * Output
    - u64 seconds
*/
pub(crate) fn interval_seconds() -> u64 {
    std::env::var("CRANE_SYNC_SECONDS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(DEFAULT_INTERVAL_SECONDS)
        .max(5)
}

/** Report whether the repository may have Brick monitors (a cheap file check; the sync decides)
 * Input
    - workspace: &Workspace - repository
 * Output
    - bool
*/
fn monitored(workspace: &Workspace) -> bool {
    workspace
        .file(crate::governance::state::BRICKS_FILE)
        .exists()
}

/** Start a detached sync now, when uploads are on; the sync reports what started it, so the service
 * records the trigger (DASHBOARD.md F34) without this process writing an event of its own
 * Input
    - workspace: &Workspace - repository
    - reason: &str - POST_TOOL, STOP, SESSION_END, COMMAND, or INTERVAL
 * Output
    - None (failures to start are ignored; the next trigger retries)
*/
pub(crate) fn trigger(workspace: &Workspace, reason: &str) {
    // Alert rules and Brick monitors are evaluated by the sync too, so it runs for them even when
    // uploads are off
    if !configured() && !super::rules::exist(workspace) && !monitored(workspace) {
        return;
    }
    let Ok(executable) = std::env::current_exe() else {
        return;
    };
    let mut command = Command::new(executable);
    command
        .args(["agent", "sync"])
        .env("CRANE_SYNC_BACKGROUND", "1")
        .env("CRANE_SYNC_TRIGGER", reason)
        .current_dir(&workspace.root)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        disinherit_standard_handles();
        // CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP: a hidden console, not stopped with the
        // parent. Not DETACHED_PROCESS: without any console, every git and curl the sync starts
        // would open (and flash) a console window of its own
        command.creation_flags(crate::platform::process::CREATE_NO_WINDOW | 0x0000_0200);
    }
    let _ = command.spawn();
}

/** Stop this process's standard handles from being inherited by the detached sync. Windows passes
 * every inheritable handle to a child, so without this the sync would keep the agent host's hook
 * pipes open and the host would wait for the sync to finish. Standard handles a later child is
 * explicitly given are duplicated for it by the standard library, so nothing else is affected.
 * Input
    - None
 * Output
    - None
*/
#[cfg(windows)]
fn disinherit_standard_handles() {
    use std::ffi::c_void;
    #[link(name = "kernel32")]
    extern "system" {
        /** Return a standard handle of this process (Win32) */
        fn GetStdHandle(kind: u32) -> *mut c_void;
        /** Change a handle's flags (Win32) */
        fn SetHandleInformation(handle: *mut c_void, mask: u32, flags: u32) -> i32;
    }
    /** The flag that lets a child process inherit a handle */
    const HANDLE_FLAG_INHERIT: u32 = 0x1;
    // STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE are -10, -11, -12 as DWORD
    for kind in [-10i32, -11, -12] {
        // SAFETY: GetStdHandle takes no pointers and returns a handle owned by this process (or
        // null or INVALID_HANDLE_VALUE, which are skipped); SetHandleInformation only changes that
        // handle's inheritance flag and never closes or invalidates it.
        unsafe {
            let handle = GetStdHandle(kind as u32);
            if !handle.is_null() && handle as isize != -1 {
                SetHandleInformation(handle, HANDLE_FLAG_INHERIT, 0);
            }
        }
    }
}

/** Start a detached sync when the last attempt is older than the interval
 * Input
    - workspace: &Workspace - repository
    - stores: &Stores - runtime stores
 * Output
    - None
*/
pub(crate) fn trigger_if_due(workspace: &Workspace, stores: &Stores) {
    if !configured() && !super::rules::exist(workspace) && !monitored(workspace) {
        return;
    }
    let last = ingest::state(stores).last_attempt_at;
    if now_millis().saturating_sub(last) >= interval_seconds() * 1000 {
        trigger(workspace, "INTERVAL");
    }
}
