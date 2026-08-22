//! Active window sampling on Windows, via the Win32 API.

use std::time::Duration;
use std::mem::size_of;
use std::path::PathBuf;
use crate::sampler::desktop::{Desktop, Window};
use anyhow::{bail, Context, Result};
use windows::core::PWSTR;
use windows::Win32::Foundation::{CloseHandle, HWND};
use windows::Win32::System::StationsAndDesktops::{GetThreadDesktop, SwitchDesktop};
use windows::Win32::System::SystemInformation::GetTickCount;
use windows::Win32::System::Threading::{GetCurrentThreadId, OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};
use windows::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId,
};

pub struct WindowsDesktop;
pub struct WindowsWindow { window_handle: HWND }

unsafe fn parse_lpwstr_from_buffer(buffer: &[u16]) -> String {
    // https://stackoverflow.com/questions/68185516/proper-handling-of-lpwstr-output-in-windows-rs
    let ptr = buffer.as_ptr();
    unsafe {
        let len = (0..buffer.len()).take_while(|&i| *ptr.offset(i as isize) != 0).count();
        let slice = std::slice::from_raw_parts(ptr, len);
        String::from_utf16_lossy(slice)
    }
}

impl Desktop for WindowsDesktop {
    fn implementation_name(&self) -> &'static str {
        "WindowsDesktop"
    }

    fn is_screen_locked(&self) -> bool {
        unsafe {
            let thread_id = GetCurrentThreadId();
            if let Ok(desktop_handle) = GetThreadDesktop(thread_id) {
                SwitchDesktop(desktop_handle).is_err()
            } else {
                false
            }
        }
    }

    fn get_idle_duration(&self) -> Result<Duration> {
        unsafe {
            let mut last_input_info = LASTINPUTINFO {
                cbSize: size_of::<LASTINPUTINFO>() as u32,
                dwTime: 0u32,
            };
            if !GetLastInputInfo(&mut last_input_info).as_bool() {
                bail!("GetLastInputInfo failed: {}", windows::core::Error::from_thread());
            }

            // wrapping_sub because GetTickCount wraps every ~49 days of uptime, and the
            // straight subtraction then underflows - which panics in a debug build.
            let ms = GetTickCount().wrapping_sub(last_input_info.dwTime);
            Ok(Duration::from_millis(ms as u64))
        }
    }

    fn get_active_window(&self) -> Result<Option<Box<dyn Window>>> {
        let window_handle = unsafe { GetForegroundWindow() };

        // NULL when no window has focus, eg. while a desktop switch is in progress. Routine,
        // so say "nothing focused" rather than handing back a handle that fails confusingly
        // in every call made on it.
        if window_handle.is_invalid() {
            return Ok(None);
        }

        Ok(Some(Box::new(WindowsWindow { window_handle })))
    }
}

impl Window for WindowsWindow {
    fn get_title(&self) -> Result<String> {
        unsafe {
            let text_length = GetWindowTextLengthW(self.window_handle) as usize;
            let mut buffer = vec![0u16; text_length+1];
            let returned_length = GetWindowTextW(self.window_handle, &mut buffer);
            if returned_length > 0 {
                let window_title = parse_lpwstr_from_buffer(&buffer);
                Ok(window_title)
            } else {
                bail!("GetWindowTextW returned 0")
            }
        }
    }

    fn get_process_id(&self) -> Result<u64> {
        unsafe {
            let mut process_id = 0u32;
            GetWindowThreadProcessId(self.window_handle, Some(&mut process_id));
            Ok(process_id as u64)
        }
    }

    /// The executable behind this window.
    ///
    /// Fails with access denied for any process running at a higher integrity level than us -
    /// an elevated Task Manager or editor, say. The caller records the event without a path
    /// rather than treating that as a malfunction.
    fn get_process_path(&self) -> Result<PathBuf> {
        let process_id = self.get_process_id()? as u32;

        unsafe {
            let process_handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, process_id)
                .with_context(|| format!("could not open process {process_id}"))?;

            let mut buffer_size = 1024u32;
            let mut buffer = vec![0u16; buffer_size as usize];
            let result = QueryFullProcessImageNameW(process_handle, PROCESS_NAME_WIN32,
                                                    PWSTR::from_raw(buffer.as_mut_ptr()),
                                                    &mut buffer_size);
            // Closed before the `?` below, so a failed query does not leak the handle. This
            // runs once per sample, so leaking would cost thousands of handles a day.
            let _ = CloseHandle(process_handle);
            result.with_context(|| format!("could not read the image name of process {process_id}"))?;

            Ok(PathBuf::from(parse_lpwstr_from_buffer(&buffer)))
        }
    }
}
