use std::time::Duration;
use std::mem::size_of;
use std::path::PathBuf;
use crate::watcher::core::{Window, Desktop, MoonwatcherSignal};
use anyhow::{bail, Result};
use windows::core::PWSTR;
use windows::Win32::Foundation::HWND;
use windows::Win32::System::StationsAndDesktops::{GetThreadDesktop, SwitchDesktop};
use windows::Win32::System::SystemInformation::GetTickCount;
use windows::Win32::System::Threading::{GetCurrentThreadId, OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};
use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId};

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

    fn get_idle_duration(&self) -> Duration {
        unsafe {
            let mut last_input_info = LASTINPUTINFO {
                cbSize: size_of::<LASTINPUTINFO>() as u32,
                dwTime: 0u32,
            };
            let success = GetLastInputInfo(&mut last_input_info);
            if success.as_bool() {
                let current_tick_count = GetTickCount();
                let ms = current_tick_count - last_input_info.dwTime;
                Duration::from_millis(ms as u64)
            } else {
                Duration::from_millis(0)
            }
        }
    }

    fn get_active_window(&self) -> Result<Box<dyn Window>> {
        unsafe {
            let window_handle = GetForegroundWindow();
            Ok( Box::new(WindowsWindow { window_handle }) )
        }
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

    fn get_process_path(&self) -> Result<PathBuf> {
        let process_id = self.get_process_id()? as u32;

        unsafe {
            let process_handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, process_id)?;
            let mut buffer_size = 1024u32;
            let mut buffer = vec![0u16; buffer_size as usize];
            let success = QueryFullProcessImageNameW(process_handle, PROCESS_NAME_WIN32, PWSTR::from_raw(buffer.as_mut_ptr()), &mut buffer_size);
            if success.is_ok() {
                let process_path_str = parse_lpwstr_from_buffer(&buffer);
                let process_path = PathBuf::from(process_path_str);
                Ok(process_path)
            } else {
                bail!("QueryFullProcessImageNameW returned zero")
            }
        }
    }
}

pub fn get_signal_channel() -> Result<crossbeam_channel::Receiver<MoonwatcherSignal>> {
    let (sender, receiver) = crossbeam_channel::bounded(100);

    ctrlc::set_handler(move || {
        println!("Received OS signal Ctrl-C");
        sender.send(MoonwatcherSignal::Terminate).expect("failed to send signal over crossbeam_channel");
    })?;

    Ok(receiver)
}
