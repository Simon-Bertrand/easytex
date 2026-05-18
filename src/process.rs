use std::process::Stdio;
use tokio::process::{Child, Command};

#[derive(Debug)]
pub struct RunningProcess {
    pid: u32,
    tree: ProcessTree,
}

impl RunningProcess {
    pub fn pid(&self) -> u32 {
        self.pid
    }

    pub async fn terminate(&self) {
        self.tree.terminate().await;
    }
}

pub fn prepare_command(cmd: &mut Command) {
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

    #[cfg(unix)]
    unsafe {
        cmd.pre_exec(|| {
            libc::setpgid(0, 0);
            Ok(())
        });
    }
}

pub fn track_child(child: &Child) -> std::io::Result<Option<RunningProcess>> {
    let Some(pid) = child.id() else {
        return Ok(None);
    };
    let tree = ProcessTree::for_child(child, pid)?;
    Ok(Some(RunningProcess { pid, tree }))
}

#[derive(Debug)]
enum ProcessTree {
    #[cfg(unix)]
    Unix { pid: u32 },
    #[cfg(windows)]
    Windows(windows_impl::JobObject),
    #[cfg(not(any(unix, windows)))]
    Single { pid: u32 },
}

impl ProcessTree {
    fn for_child(child: &Child, _pid: u32) -> std::io::Result<Self> {
        #[cfg(unix)]
        {
            let _ = child;
            Ok(Self::Unix { pid: _pid })
        }

        #[cfg(windows)]
        {
            windows_impl::JobObject::for_child(child).map(Self::Windows)
        }

        #[cfg(not(any(unix, windows)))]
        {
            let _ = child;
            Ok(Self::Single { pid })
        }
    }

    async fn terminate(&self) {
        match self {
            #[cfg(unix)]
            Self::Unix { pid } => {
                unsafe {
                    libc::kill(-(*pid as i32), libc::SIGTERM);
                }
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                unsafe {
                    libc::kill(-(*pid as i32), libc::SIGKILL);
                }
            }
            #[cfg(windows)]
            Self::Windows(job) => job.terminate(),
            #[cfg(not(any(unix, windows)))]
            Self::Single { pid } => {
                let _ = pid;
            }
        }
    }
}

#[cfg(windows)]
mod windows_impl {
    use std::{
        io,
        mem::{size_of, zeroed},
        ptr::null_mut,
    };
    use tokio::process::Child;
    use windows_sys::Win32::{
        Foundation::{CloseHandle, HANDLE},
        System::JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
            SetInformationJobObject, TerminateJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        },
    };

    #[derive(Debug)]
    pub struct JobObject {
        handle: HANDLE,
    }

    impl JobObject {
        pub fn for_child(child: &Child) -> io::Result<Self> {
            unsafe {
                let handle = CreateJobObjectW(null_mut(), null_mut());
                if handle.is_null() {
                    return Err(io::Error::last_os_error());
                }

                let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = zeroed();
                info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                let ok = SetInformationJobObject(
                    handle,
                    JobObjectExtendedLimitInformation,
                    &mut info as *mut _ as *mut _,
                    size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                );
                if ok == 0 {
                    let error = io::Error::last_os_error();
                    CloseHandle(handle);
                    return Err(error);
                }

                let child_handle = child.raw_handle().ok_or_else(|| {
                    io::Error::new(io::ErrorKind::Other, "child process handle is unavailable")
                })? as HANDLE;
                let ok = AssignProcessToJobObject(handle, child_handle);
                if ok == 0 {
                    let error = io::Error::last_os_error();
                    CloseHandle(handle);
                    return Err(error);
                }

                Ok(Self { handle })
            }
        }

        pub fn terminate(&self) {
            unsafe {
                TerminateJobObject(self.handle, 1);
            }
        }
    }

    impl Drop for JobObject {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.handle);
            }
        }
    }

    unsafe impl Send for JobObject {}
    unsafe impl Sync for JobObject {}
}
