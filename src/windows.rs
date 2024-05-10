use windows::Win32::{
    System::{
        Power::{SetThreadExecutionState, ES_AWAYMODE_REQUIRED, ES_CONTINUOUS, ES_SYSTEM_REQUIRED},
        Threading::{GetCurrentProcess, SetPriorityClass, HIGH_PRIORITY_CLASS},
    },
    UI::HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2},
};

pub(crate) struct System;

impl System {
    pub(crate) fn new() -> eyre::Result<Self> {
        unsafe {
            // Set DPI Awareness for our process
            SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2)?;

            // Make our process high priority.
            let this_process = GetCurrentProcess();
            SetPriorityClass(this_process, HIGH_PRIORITY_CLASS)?;

            // Make sure that we keep the display and system timers reset.
            SetThreadExecutionState(ES_AWAYMODE_REQUIRED | ES_SYSTEM_REQUIRED | ES_CONTINUOUS);
        }

        Ok(System)
    }
}

impl Drop for System {
    fn drop(&mut self) {
        unsafe {
            // Allow computer to sleep / toggle display again
            SetThreadExecutionState(ES_CONTINUOUS);
        }
    }
}
