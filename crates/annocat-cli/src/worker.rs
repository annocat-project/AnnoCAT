use std::path::Path;

#[cfg(not(windows))]
pub fn validate_report(path: &Path) -> Result<String, String> {
    use std::process::{Command, Stdio};
    let executable = std::env::current_exe()
        .map_err(|error| format!("cannot locate AnnoCAT executable: {error}"))?;
    let output = Command::new(executable)
        .arg("report-worker")
        .arg("validate")
        .arg(path)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("cannot run report validation worker: {error}"))?;
    worker_output(output.status.success(), &output.stdout, &output.stderr)
}

#[cfg(windows)]
pub fn validate_report(path: &Path) -> Result<String, String> {
    appcontainer::validate_report(path)
}

fn worker_output(success: bool, stdout: &[u8], stderr: &[u8]) -> Result<String, String> {
    if !success {
        let message = String::from_utf8_lossy(stderr).trim().to_owned();
        return Err(if message.is_empty() {
            "report validation worker failed without an error message".into()
        } else {
            message
        });
    }
    String::from_utf8(stdout.to_vec())
        .map(|value| value.trim().to_owned())
        .map_err(|_| "report validation worker returned non-UTF-8 output".into())
}

#[cfg(windows)]
pub fn require_appcontainer() -> Result<(), String> {
    appcontainer::require_current_process()
}

#[cfg(not(windows))]
pub fn require_appcontainer() -> Result<(), String> {
    Err("sandboxed report import is currently available only on Windows".into())
}

#[cfg(windows)]
mod appcontainer {
    use super::worker_output;
    use std::ffi::c_void;
    use std::fs::File;
    use std::io::Read;
    use std::mem::{size_of, zeroed};
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use std::os::windows::io::{AsRawHandle, FromRawHandle};
    use std::path::Path;
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::Foundation::{
        CloseHandle, HANDLE, HANDLE_FLAG_INHERIT, LocalFree, SetHandleInformation, WAIT_OBJECT_0,
    };
    use windows_sys::Win32::Security::Authorization::ConvertSidToStringSidW;
    use windows_sys::Win32::Security::Isolation::{
        CreateAppContainerProfile, DeriveAppContainerSidFromAppContainerName,
        GetAppContainerFolderPath,
    };
    use windows_sys::Win32::Security::{
        FreeSid, GetTokenInformation, PSID, SECURITY_ATTRIBUTES, SECURITY_CAPABILITIES,
        TOKEN_QUERY, TokenIsAppContainer,
    };
    use windows_sys::Win32::System::Com::CoTaskMemFree;
    use windows_sys::Win32::System::JobObjects::{
        CreateJobObjectW, JOB_OBJECT_LIMIT_ACTIVE_PROCESS, JOB_OBJECT_LIMIT_JOB_MEMORY,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JobObjectExtendedLimitInformation, SetInformationJobObject,
    };
    use windows_sys::Win32::System::Pipes::CreatePipe;
    use windows_sys::Win32::System::Threading::{
        CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT, CreateMutexW, CreateProcessW,
        DeleteProcThreadAttributeList, EXTENDED_STARTUPINFO_PRESENT, GetCurrentProcess,
        GetExitCodeProcess, INFINITE, InitializeProcThreadAttributeList, OpenProcessToken,
        PROC_THREAD_ATTRIBUTE_CHILD_PROCESS_POLICY, PROC_THREAD_ATTRIBUTE_HANDLE_LIST,
        PROC_THREAD_ATTRIBUTE_JOB_LIST, PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES,
        PROCESS_INFORMATION, ReleaseMutex, ResumeThread, STARTF_USESTDHANDLES, STARTUPINFOEXW,
        TerminateProcess, UpdateProcThreadAttribute, WaitForSingleObject,
    };
    use windows_sys::Win32::System::WindowsProgramming::PROCESS_CREATION_CHILD_PROCESS_RESTRICTED;

    const PROFILE_NAME: &str = "OpenAI.AnnoCat.ReportWorker";
    const LAUNCH_MUTEX_NAME: &str = "Local\\OpenAI.AnnoCat.ReportWorker.Launch";
    const HRESULT_ALREADY_EXISTS: i32 = 0x8007_00b7_u32 as i32;

    pub fn validate_report(path: &Path) -> Result<String, String> {
        let application = std::env::current_exe()
            .map_err(|error| format!("cannot locate AnnoCAT executable: {error}"))?;
        let trusted_executable = application
            .parent()
            .ok_or("cannot locate AnnoCAT application directory")?
            .join("annocat-report-worker.exe");
        if !trusted_executable.is_file() {
            return Err(format!(
                "AnnoCAT report worker is missing: {}",
                trusted_executable.display()
            ));
        }
        let archive = File::open(path)
            .map_err(|error| format!("cannot open report archive {}: {error}", path.display()))?;
        let archive_handle = archive.as_raw_handle() as HANDLE;
        if unsafe { SetHandleInformation(archive_handle, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT) }
            == 0
        {
            return Err(format!(
                "cannot prepare the report archive for sandboxed validation: {}",
                std::io::Error::last_os_error()
            ));
        }

        let _launch_lock = NamedMutex::lock()?;
        let profile_sid = AppContainerSid::open_or_create()?;
        let executable = profile_sid.stage_worker(&trusted_executable)?;
        let job = WindowsJob::new()?;
        let pipe = OutputPipe::new()?;
        let mut inherited_handles = [archive_handle, pipe.write];
        let mut job_handles = [job.0];
        let mut child_policy = PROCESS_CREATION_CHILD_PROCESS_RESTRICTED;
        let mut security_capabilities = SECURITY_CAPABILITIES {
            AppContainerSid: profile_sid.0,
            Capabilities: null_mut(),
            CapabilityCount: 0,
            Reserved: 0,
        };
        let mut attributes = AttributeList::new(4)?;
        attributes.set(
            PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES,
            &mut security_capabilities,
        )?;
        attributes.set_slice(PROC_THREAD_ATTRIBUTE_HANDLE_LIST, &mut inherited_handles)?;
        attributes.set_slice(PROC_THREAD_ATTRIBUTE_JOB_LIST, &mut job_handles)?;
        attributes.set(
            PROC_THREAD_ATTRIBUTE_CHILD_PROCESS_POLICY,
            &mut child_policy,
        )?;

        let executable_wide = wide(executable.0.as_os_str());
        let mut command_line = wide(std::ffi::OsStr::new(&format!(
            "\"{}\" validate-handle {}",
            executable.0.display(),
            archive_handle as usize
        )));
        let mut startup = STARTUPINFOEXW::default();
        startup.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
        startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
        startup.StartupInfo.hStdOutput = pipe.write;
        startup.StartupInfo.hStdError = pipe.write;
        startup.StartupInfo.hStdInput = null_mut();
        startup.lpAttributeList = attributes.pointer();
        let mut process = PROCESS_INFORMATION::default();
        let environment = minimal_environment()?;
        let created = unsafe {
            CreateProcessW(
                executable_wide.as_ptr(),
                command_line.as_mut_ptr(),
                null(),
                null(),
                1,
                EXTENDED_STARTUPINFO_PRESENT | CREATE_SUSPENDED | CREATE_UNICODE_ENVIRONMENT,
                environment.as_ptr() as *const c_void,
                null(),
                &startup.StartupInfo,
                &mut process,
            )
        };
        unsafe {
            SetHandleInformation(archive_handle, HANDLE_FLAG_INHERIT, 0);
        }
        if created == 0 {
            return Err(format!(
                "cannot start AppContainer report worker {}: {}",
                executable.0.display(),
                std::io::Error::last_os_error()
            ));
        }
        let process_handles = ProcessHandles(process);
        pipe.close_parent_write();
        if !process_is_appcontainer(process_handles.0.hProcess)? {
            unsafe { TerminateProcess(process_handles.0.hProcess, 1) };
            return Err("Windows created the report worker without AppContainer isolation".into());
        }
        if unsafe { ResumeThread(process_handles.0.hThread) } == u32::MAX {
            unsafe { TerminateProcess(process_handles.0.hProcess, 1) };
            return Err(format!(
                "cannot resume AppContainer report worker: {}",
                std::io::Error::last_os_error()
            ));
        }

        let reader = pipe.start_reader();
        let waited = unsafe { WaitForSingleObject(process_handles.0.hProcess, INFINITE) };
        if waited != WAIT_OBJECT_0 {
            unsafe { TerminateProcess(process_handles.0.hProcess, 1) };
            return Err(format!(
                "cannot wait for AppContainer report worker: {}",
                std::io::Error::last_os_error()
            ));
        }
        let mut exit_code = 1_u32;
        if unsafe { GetExitCodeProcess(process_handles.0.hProcess, &mut exit_code) } == 0 {
            return Err(format!(
                "cannot read AppContainer report worker status: {}",
                std::io::Error::last_os_error()
            ));
        }
        let output = reader
            .join()
            .map_err(|_| "AppContainer report worker output reader panicked".to_string())??;
        if exit_code == 0 {
            worker_output(true, &output, &[])
        } else if output.is_empty() {
            Err(format!(
                "report validation worker exited with status 0x{exit_code:08x}"
            ))
        } else {
            worker_output(false, &[], &output)
        }
    }

    pub fn require_current_process() -> Result<(), String> {
        if process_is_appcontainer(unsafe { GetCurrentProcess() })? {
            Ok(())
        } else {
            Err("report worker refused to parse untrusted data outside AppContainer".into())
        }
    }

    fn process_is_appcontainer(process: HANDLE) -> Result<bool, String> {
        let mut token = null_mut();
        if unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) } == 0 {
            return Err(format!(
                "cannot inspect report worker token: {}",
                std::io::Error::last_os_error()
            ));
        }
        let token = OwnedHandle(token);
        let mut value = 0_u32;
        let mut returned = 0_u32;
        let result = unsafe {
            GetTokenInformation(
                token.0,
                TokenIsAppContainer,
                &mut value as *mut _ as *mut c_void,
                size_of::<u32>() as u32,
                &mut returned,
            )
        };
        if result == 0 {
            return Err(format!(
                "cannot verify AppContainer token: {}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(value != 0)
    }

    struct AppContainerSid(PSID);

    impl AppContainerSid {
        fn open_or_create() -> Result<Self, String> {
            let name = wide(std::ffi::OsStr::new(PROFILE_NAME));
            let mut sid = null_mut();
            let display = wide(std::ffi::OsStr::new("AnnoCAT report validator"));
            let description = wide(std::ffi::OsStr::new(
                "Networkless sandbox for validating untrusted AnnoCAT reports",
            ));
            let created = unsafe {
                CreateAppContainerProfile(
                    name.as_ptr(),
                    display.as_ptr(),
                    description.as_ptr(),
                    null(),
                    0,
                    &mut sid,
                )
            };
            if created == HRESULT_ALREADY_EXISTS {
                let retry =
                    unsafe { DeriveAppContainerSidFromAppContainerName(name.as_ptr(), &mut sid) };
                if retry < 0 {
                    return Err(format!(
                        "cannot resolve the existing AnnoCAT report AppContainer profile (HRESULT 0x{:08x})",
                        retry as u32
                    ));
                }
            } else if created < 0 {
                return Err(format!(
                    "cannot create the AnnoCAT report AppContainer profile (HRESULT 0x{:08x})",
                    created as u32
                ));
            }
            if sid.is_null() {
                return Err(
                    "Windows returned no SID for the AnnoCAT report AppContainer profile".into(),
                );
            }
            Ok(Self(sid))
        }

        fn stage_worker(&self, source: &Path) -> Result<StagedWorker, String> {
            let mut sid_text = null_mut();
            if unsafe { ConvertSidToStringSidW(self.0, &mut sid_text) } == 0 {
                return Err(format!(
                    "cannot resolve report AppContainer identity: {}",
                    std::io::Error::last_os_error()
                ));
            }
            let sid_text = LocalWideString(sid_text);
            let mut folder_text = null_mut();
            let result = unsafe { GetAppContainerFolderPath(sid_text.0, &mut folder_text) };
            if result < 0 {
                return Err(format!(
                    "cannot locate report AppContainer profile (HRESULT 0x{:08x})",
                    result as u32
                ));
            }
            let folder_text = TaskWideString(folder_text);
            let folder = wide_path(folder_text.0);
            std::fs::create_dir_all(&folder).map_err(|error| {
                format!("cannot initialize report AppContainer profile: {error}")
            })?;
            let target = folder.join("annocat-report-worker.exe");
            let temporary = folder.join(format!(
                ".annocat-report-worker-{}.partial",
                std::process::id()
            ));
            if temporary.exists() {
                std::fs::remove_file(&temporary)
                    .map_err(|error| format!("cannot replace temporary report worker: {error}"))?;
            }
            std::fs::copy(source, &temporary)
                .map_err(|error| format!("cannot stage sandboxed report worker: {error}"))?;
            if target.exists() {
                std::fs::remove_file(&target)
                    .map_err(|error| format!("cannot update sandboxed report worker: {error}"))?;
            }
            std::fs::rename(&temporary, &target)
                .map_err(|error| format!("cannot publish sandboxed report worker: {error}"))?;
            Ok(StagedWorker(target))
        }
    }

    impl Drop for AppContainerSid {
        fn drop(&mut self) {
            unsafe { FreeSid(self.0) };
        }
    }

    struct StagedWorker(std::path::PathBuf);

    impl Drop for StagedWorker {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    struct NamedMutex(HANDLE);

    impl NamedMutex {
        fn lock() -> Result<Self, String> {
            let name = wide(std::ffi::OsStr::new(LAUNCH_MUTEX_NAME));
            let handle = unsafe { CreateMutexW(null(), 0, name.as_ptr()) };
            if handle.is_null() {
                return Err(format!(
                    "cannot create report sandbox launch lock: {}",
                    std::io::Error::last_os_error()
                ));
            }
            let mutex = Self(handle);
            let waited = unsafe { WaitForSingleObject(mutex.0, INFINITE) };
            if waited != WAIT_OBJECT_0 && waited != 0x0000_0080 {
                return Err(format!(
                    "cannot acquire report sandbox launch lock: {}",
                    std::io::Error::last_os_error()
                ));
            }
            Ok(mutex)
        }
    }

    impl Drop for NamedMutex {
        fn drop(&mut self) {
            unsafe {
                ReleaseMutex(self.0);
                CloseHandle(self.0);
            }
        }
    }

    struct AttributeList {
        storage: Vec<usize>,
    }

    impl AttributeList {
        fn new(count: u32) -> Result<Self, String> {
            let mut bytes = 0_usize;
            unsafe { InitializeProcThreadAttributeList(null_mut(), count, 0, &mut bytes) };
            if bytes == 0 {
                return Err(format!(
                    "cannot size AppContainer process attributes: {}",
                    std::io::Error::last_os_error()
                ));
            }
            let words = bytes.div_ceil(size_of::<usize>());
            let mut list = Self {
                storage: vec![0_usize; words],
            };
            if unsafe { InitializeProcThreadAttributeList(list.pointer(), count, 0, &mut bytes) }
                == 0
            {
                return Err(format!(
                    "cannot initialize AppContainer process attributes: {}",
                    std::io::Error::last_os_error()
                ));
            }
            Ok(list)
        }

        fn pointer(
            &mut self,
        ) -> windows_sys::Win32::System::Threading::LPPROC_THREAD_ATTRIBUTE_LIST {
            self.storage.as_mut_ptr() as _
        }

        fn set<T>(&mut self, attribute: u32, value: &mut T) -> Result<(), String> {
            self.set_raw(attribute, value as *mut T as *const c_void, size_of::<T>())
        }

        fn set_slice<T>(&mut self, attribute: u32, value: &mut [T]) -> Result<(), String> {
            self.set_raw(
                attribute,
                value.as_mut_ptr() as *const c_void,
                std::mem::size_of_val(value),
            )
        }

        fn set_raw(
            &mut self,
            attribute: u32,
            value: *const c_void,
            bytes: usize,
        ) -> Result<(), String> {
            if unsafe {
                UpdateProcThreadAttribute(
                    self.pointer(),
                    0,
                    attribute as usize,
                    value,
                    bytes,
                    null_mut(),
                    null(),
                )
            } == 0
            {
                return Err(format!(
                    "cannot configure AppContainer process attribute {attribute}: {}",
                    std::io::Error::last_os_error()
                ));
            }
            Ok(())
        }
    }

    impl Drop for AttributeList {
        fn drop(&mut self) {
            unsafe { DeleteProcThreadAttributeList(self.pointer()) };
        }
    }

    struct WindowsJob(HANDLE);

    impl WindowsJob {
        fn new() -> Result<Self, String> {
            let handle = unsafe { CreateJobObjectW(null(), null()) };
            if handle.is_null() {
                return Err(format!(
                    "cannot create report worker Job Object: {}",
                    std::io::Error::last_os_error()
                ));
            }
            let job = Self(handle);
            let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { zeroed() };
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_ACTIVE_PROCESS
                | JOB_OBJECT_LIMIT_JOB_MEMORY
                | JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            limits.BasicLimitInformation.ActiveProcessLimit = 1;
            limits.JobMemoryLimit = 1024 * 1024 * 1024;
            if unsafe {
                SetInformationJobObject(
                    job.0,
                    JobObjectExtendedLimitInformation,
                    &limits as *const _ as *const c_void,
                    size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                )
            } == 0
            {
                return Err(format!(
                    "cannot configure report worker Job Object: {}",
                    std::io::Error::last_os_error()
                ));
            }
            Ok(job)
        }
    }

    impl Drop for WindowsJob {
        fn drop(&mut self) {
            unsafe { CloseHandle(self.0) };
        }
    }

    struct OutputPipe {
        read: HANDLE,
        write: HANDLE,
    }

    impl OutputPipe {
        fn new() -> Result<Self, String> {
            let security = SECURITY_ATTRIBUTES {
                nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: null_mut(),
                bInheritHandle: 1,
            };
            let mut read = null_mut();
            let mut write = null_mut();
            if unsafe { CreatePipe(&mut read, &mut write, &security, 0) } == 0 {
                return Err(format!(
                    "cannot create report worker output pipe: {}",
                    std::io::Error::last_os_error()
                ));
            }
            if unsafe { SetHandleInformation(read, HANDLE_FLAG_INHERIT, 0) } == 0 {
                unsafe {
                    CloseHandle(read);
                    CloseHandle(write);
                }
                return Err(format!(
                    "cannot secure report worker output pipe: {}",
                    std::io::Error::last_os_error()
                ));
            }
            Ok(Self { read, write })
        }

        fn close_parent_write(&self) {
            unsafe { CloseHandle(self.write) };
        }

        fn start_reader(&self) -> std::thread::JoinHandle<Result<Vec<u8>, String>> {
            let read = self.read as usize;
            std::thread::spawn(move || {
                let mut file = unsafe { File::from_raw_handle(read as _) };
                let mut bytes = Vec::new();
                file.read_to_end(&mut bytes)
                    .map_err(|error| format!("cannot read report worker output: {error}"))?;
                Ok(bytes)
            })
        }
    }

    struct ProcessHandles(PROCESS_INFORMATION);

    impl Drop for ProcessHandles {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.0.hThread);
                CloseHandle(self.0.hProcess);
            }
        }
    }

    struct OwnedHandle(HANDLE);

    impl Drop for OwnedHandle {
        fn drop(&mut self) {
            unsafe { CloseHandle(self.0) };
        }
    }

    struct LocalWideString(windows_sys::core::PWSTR);

    impl Drop for LocalWideString {
        fn drop(&mut self) {
            unsafe { LocalFree(self.0 as _) };
        }
    }

    struct TaskWideString(windows_sys::core::PWSTR);

    impl Drop for TaskWideString {
        fn drop(&mut self) {
            unsafe { CoTaskMemFree(self.0 as _) };
        }
    }

    fn wide(value: &std::ffi::OsStr) -> Vec<u16> {
        value.encode_wide().chain(std::iter::once(0)).collect()
    }

    fn wide_path(pointer: windows_sys::core::PCWSTR) -> std::path::PathBuf {
        let mut length = 0;
        while unsafe { *pointer.add(length) } != 0 {
            length += 1;
        }
        let units = unsafe { std::slice::from_raw_parts(pointer, length) };
        std::path::PathBuf::from(std::ffi::OsString::from_wide(units))
    }

    fn minimal_environment() -> Result<Vec<u16>, String> {
        const ALLOWED: &[&str] = &[
            "APPDATA",
            "ComSpec",
            "LOCALAPPDATA",
            "Path",
            "PATHEXT",
            "SystemDrive",
            "SystemRoot",
            "TEMP",
            "TMP",
            "USERPROFILE",
            "windir",
        ];
        let mut entries = ALLOWED
            .iter()
            .filter_map(|name| std::env::var_os(name).map(|value| ((*name).to_owned(), value)))
            .collect::<Vec<_>>();
        if !entries
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case("SystemRoot"))
        {
            return Err("Windows did not provide SystemRoot for the report worker".into());
        }
        entries.sort_by_key(|(name, _)| name.to_ascii_uppercase());
        let mut block = Vec::new();
        for (name, value) in entries {
            block.extend(std::ffi::OsStr::new(&name).encode_wide());
            block.push(b'=' as u16);
            block.extend(value.encode_wide());
            block.push(0);
        }
        block.push(0);
        Ok(block)
    }
}
