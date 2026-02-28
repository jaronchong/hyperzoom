use std::sync::atomic::{AtomicBool, Ordering};

/// Call from within a cpal callback to promote the current thread to real-time
/// priority. The `AtomicBool` guard ensures this only runs once per stream.
pub fn promote_once(done: &AtomicBool, label: &str) {
    if done.swap(true, Ordering::SeqCst) {
        return;
    }
    match promote_current_thread() {
        Ok(()) => log::info!("Promoted audio thread to real-time priority ({label})"),
        Err(e) => log::warn!("Failed to promote audio thread ({label}): {e}"),
    }
}

#[cfg(target_os = "macos")]
pub fn promote_current_thread() -> Result<(), String> {
    use mach2::kern_return::KERN_SUCCESS;
    use mach2::mach_time::mach_timebase_info;
    use mach2::thread_policy::{
        thread_policy_set, thread_time_constraint_policy, THREAD_TIME_CONSTRAINT_POLICY,
    };

    unsafe {
        let thread_port = libc::pthread_mach_thread_np(libc::pthread_self());

        // Get timebase to convert nanoseconds → Mach absolute time units
        let mut timebase = mach_timebase_info { numer: 0, denom: 0 };
        mach_timebase_info(&mut timebase);
        let ns_to_abs = |ns: u32| -> u32 {
            (ns as u64 * timebase.denom as u64 / timebase.numer as u64) as u32
        };

        // ~5.33ms period (≈187.5 Hz tick), 2.66ms computation, 10ms constraint
        let policy = thread_time_constraint_policy {
            period: ns_to_abs(5_333_333),
            computation: ns_to_abs(2_666_666),
            constraint: ns_to_abs(10_000_000),
            preemptible: 1,
        };

        let ret = thread_policy_set(
            thread_port,
            THREAD_TIME_CONSTRAINT_POLICY,
            &policy as *const _ as *mut _,
            std::mem::size_of::<thread_time_constraint_policy>() as u32
                / std::mem::size_of::<i32>() as u32,
        );

        if ret == KERN_SUCCESS {
            Ok(())
        } else {
            Err(format!("thread_policy_set returned {ret}"))
        }
    }
}

#[cfg(target_os = "windows")]
pub fn promote_current_thread() -> Result<(), String> {
    #[link(name = "avrt")]
    extern "system" {
        fn AvSetMmThreadCharacteristicsW(
            task_name: *const u16,
            task_index: *mut u32,
        ) -> isize;
    }

    let task_name: Vec<u16> = "Pro Audio\0".encode_utf16().collect();
    let mut task_index: u32 = 0;
    let handle =
        unsafe { AvSetMmThreadCharacteristicsW(task_name.as_ptr(), &mut task_index) };
    if handle == 0 {
        Err("AvSetMmThreadCharacteristicsW failed".into())
    } else {
        Ok(())
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn promote_current_thread() -> Result<(), String> {
    log::debug!("RT thread promotion not implemented on this platform");
    Ok(())
}
