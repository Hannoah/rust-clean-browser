#[cfg(target_os = "windows")]
mod win_mem {
    use core::ffi::c_void;

    #[repr(C)]
    #[allow(non_snake_case)]
    pub struct ProcessMemoryCounters {
        pub cb: u32,
        pub page_fault_count: u32,
        pub peak_working_set_size: usize,
        pub working_set_size: usize,
        pub quota_peak_paged_pool_usage: usize,
        pub quota_paged_pool_usage: usize,
        pub quota_peak_non_paged_pool_usage: usize,
        pub quota_non_paged_pool_usage: usize,
        pub pagefile_usage: usize,
        pub peak_pagefile_usage: usize,
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        pub fn GetCurrentProcess() -> *mut c_void;
        pub fn K32GetProcessMemoryInfo(
            process: *mut c_void,
            ppsmem_counters: *mut ProcessMemoryCounters,
            cb: u32,
        ) -> i32;
        pub fn SetProcessWorkingSetSize(
            h_process: *mut c_void,
            dw_minimum_working_set_size: usize,
            dw_maximum_working_set_size: usize,
        ) -> i32;
    }
}

/// Process working set (resident memory) in MB; 0 if unavailable.
#[cfg(target_os = "windows")]
pub fn working_set_mb() -> f64 {
    // SAFETY: plain-old-data struct; all-zero is a valid value.
    let mut counters: win_mem::ProcessMemoryCounters = unsafe { std::mem::zeroed() };
    counters.cb = std::mem::size_of::<win_mem::ProcessMemoryCounters>() as u32;

    let ok = unsafe {
        win_mem::K32GetProcessMemoryInfo(
            win_mem::GetCurrentProcess(),
            &mut counters as *mut win_mem::ProcessMemoryCounters,
            counters.cb,
        )
    };

    if ok != 0 {
        counters.working_set_size as f64 / (1024.0 * 1024.0)
    } else {
        0.0
    }
}

/// macOS: resident size from `proc_pidinfo(PROC_PIDTASKINFO)`.
#[cfg(target_os = "macos")]
pub fn working_set_mb() -> f64 {
    let mut info: libc::proc_taskinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_taskinfo>() as libc::c_int;
    let n = unsafe {
        libc::proc_pidinfo(
            libc::getpid(),
            libc::PROC_PIDTASKINFO,
            0,
            &mut info as *mut _ as *mut libc::c_void,
            size,
        )
    };
    if n == size {
        info.pti_resident_size as f64 / (1024.0 * 1024.0)
    } else {
        0.0
    }
}

/// Linux & other Unix: resident pages from `/proc/self/statm`.
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub fn working_set_mb() -> f64 {
    let resident_pages = std::fs::read_to_string("/proc/self/statm")
        .ok()
        .and_then(|s| s.split_whitespace().nth(1)?.parse::<f64>().ok())
        .unwrap_or(0.0);
    let page = unsafe { libc_page_size() }.max(4096) as f64;
    resident_pages * page / (1024.0 * 1024.0)
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
unsafe fn libc_page_size() -> i64 {
    const SC_PAGESIZE: i32 = 30; // _SC_PAGESIZE on Linux
    unsafe extern "C" {
        fn sysconf(name: i32) -> i64;
    }
    unsafe { sysconf(SC_PAGESIZE) }
}

/// Ask Windows to reclaim freed heap pages from the process working set immediately
/// after dropping a heavy tab's DOM/layout/image buffers.
#[cfg(target_os = "windows")]
pub fn trim_process_working_set() {
    unsafe {
        let _ = win_mem::SetProcessWorkingSetSize(win_mem::GetCurrentProcess(), usize::MAX, usize::MAX);
    }
}

/// macOS: ask libmalloc to return freed pages to the OS.
#[cfg(target_os = "macos")]
pub fn trim_process_working_set() {
    unsafe extern "C" {
        fn malloc_zone_pressure_relief(zone: *mut core::ffi::c_void, goal: usize) -> usize;
    }
    unsafe {
        malloc_zone_pressure_relief(std::ptr::null_mut(), 0);
    }
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub fn trim_process_working_set() {}
