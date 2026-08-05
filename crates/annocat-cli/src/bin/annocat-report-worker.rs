#[path = "../report_import.rs"]
mod report_import;

#[cfg(windows)]
fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

#[cfg(windows)]
fn run() -> Result<(), String> {
    use std::os::windows::io::FromRawHandle;
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::Security::{GetTokenInformation, TOKEN_QUERY, TokenIsAppContainer};
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    let mut token = std::ptr::null_mut();
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return Err(format!(
            "cannot inspect result validation worker token: {}",
            std::io::Error::last_os_error()
        ));
    }
    let mut is_appcontainer = 0_u32;
    let mut returned = 0_u32;
    let inspected = unsafe {
        GetTokenInformation(
            token,
            TokenIsAppContainer,
            &mut is_appcontainer as *mut _ as *mut std::ffi::c_void,
            std::mem::size_of::<u32>() as u32,
            &mut returned,
        )
    };
    unsafe { CloseHandle(token) };
    if inspected == 0 {
        return Err(format!(
            "cannot verify result validation worker AppContainer token: {}",
            std::io::Error::last_os_error()
        ));
    }
    if is_appcontainer == 0 {
        return Err(
            "result validation worker refused to parse untrusted data outside AppContainer".into(),
        );
    }

    let mut args = std::env::args().skip(1);
    if args.next().as_deref() != Some("validate-handle") {
        return Err("invalid result validation worker request".into());
    }
    let handle = args
        .next()
        .ok_or("missing inherited result archive handle")?
        .parse::<usize>()
        .map_err(|_| "invalid inherited result archive handle")?;
    if handle == 0 || args.next().is_some() {
        return Err("invalid result validation worker request".into());
    }
    let file = unsafe { std::fs::File::from_raw_handle(handle as _) };
    let report = report_import::validate_archive_file(file)?;
    println!(
        "Valid AnnoCAT result: {} (schema {}, {} files, {} bytes)",
        report.run_id, report.schema_version, report.file_count, report.uncompressed_bytes
    );
    Ok(())
}

#[cfg(not(windows))]
fn main() {
    eprintln!("annocat-report-worker is currently supported only on Windows");
    std::process::exit(1);
}
