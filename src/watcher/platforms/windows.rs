use std::process::Command;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::time::Duration;
use std::mem::size_of;
use std::path::{Path, PathBuf};
use crate::watcher::UiRefresh;
use crate::watcher::core::{Window, Desktop, MoonwatcherSignal, WorkerHandle};
use anyhow::{anyhow, bail, Context, Result};
use windows::core::{w, PWSTR};
use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::StationsAndDesktops::{GetThreadDesktop, SwitchDesktop};
use windows::Win32::System::SystemInformation::GetTickCount;
use windows::Win32::System::Threading::{GetCurrentThreadId, OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetForegroundWindow, GetMessageW,
    GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId, PostMessageW, PostQuitMessage,
    RegisterClassW, TranslateMessage, CW_USEDEFAULT, MSG, WM_APP, WM_CLOSE, WM_DESTROY,
    WM_ENDSESSION, WM_QUERYENDSESSION, WNDCLASSW, WS_EX_TOOLWINDOW, WS_OVERLAPPED,
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

/// Ctrl-C only reaches us when we inherited a console from the shell that launched us,
/// which never happens for the daemon started at login. It is kept for the convenience of
/// running the binary from a terminal; the reliable shutdown path on Windows is the
/// session-end handling in [`run_event_loop`].
pub fn install_signal_handlers(worker: WorkerHandle) -> Result<()> {
    ctrlc::set_handler(move || {
        log::info!("Received Ctrl-C");
        worker.send(MoonwatcherSignal::Terminate { done: None });
    })?;

    Ok(())
}

/// Posted to the UI window by the worker thread when it has stopped, so that the message
/// loop knows to give up too.
const WM_MOONWATCHER_WORKER_EXITED: u32 = WM_APP + 1;

/// Posted to the UI window to wake the message loop when the tray needs repainting. The
/// window procedure ignores it; it exists purely to get `GetMessageW` to return.
const WM_MOONWATCHER_REFRESH_UI: u32 = WM_APP + 2;

/// How long we let the worker finish its final write before giving up on it. Windows
/// allows roughly five seconds between `WM_QUERYENDSESSION` and killing the process; the
/// write itself is a single small file, so this budget is never expected to be used up.
const FLUSH_TIMEOUT: Duration = Duration::from_secs(5);

/// Set once before the UI window is created, so that `wnd_proc` (which cannot be a
/// closure) can reach the worker. There is exactly one UI window per process.
static UI_WORKER: OnceLock<WorkerHandle> = OnceLock::new();

/// Handle of the UI window, as an `isize` so other threads can read it. Zero until the
/// window exists.
static UI_HWND: AtomicIsize = AtomicIsize::new(0);

/// Set by [`request_ui_quit`], so that a worker which stops before the window exists does
/// not leave the message loop running with nothing left to do.
static UI_QUIT_REQUESTED: AtomicBool = AtomicBool::new(false);

/// Set by [`request_ui_refresh`] and cleared by the message loop once it has repainted.
static UI_REFRESH_REQUESTED: AtomicBool = AtomicBool::new(false);

/// Run the Win32 message loop on the calling thread.
///
/// Besides pumping messages for the tray icon, this creates a hidden top-level window
/// whose sole purpose is to receive `WM_QUERYENDSESSION` / `WM_ENDSESSION`. Without a
/// window the process gets no notice at all of a logoff or shutdown, which is why
/// buffered events used to be lost. The window must be a real top-level window: a
/// message-only window (parented to `HWND_MESSAGE`) does not receive session-end messages.
///
/// `build_tray` is called once the window exists, may be `None`, and is allowed to fail —
/// the session-end handling is worth having on its own, so the loop runs either way.
pub fn run_event_loop<T: UiRefresh>(worker: WorkerHandle,
                                    build_tray: Option<impl FnOnce() -> Result<T>>) -> Result<()> {
    let _ = UI_WORKER.set(worker);

    let hwnd = unsafe { create_ui_window() }?;
    UI_HWND.store(hwnd.0 as isize, Ordering::SeqCst);

    // Keep the tray icon alive for as long as the loop runs; dropping it removes the icon.
    let mut tray = match build_tray.map(|build| build()) {
        Some(Ok(tray)) => Some(tray),
        None => None,
        Some(Err(e)) => {
            log::warn!("Could not create tray icon (continuing without it): {e:?}");
            None
        }
    };

    log::info!("Entering Win32 message loop");
    let mut msg = MSG::default();
    while !UI_QUIT_REQUESTED.load(Ordering::SeqCst) {
        let result = unsafe { GetMessageW(&mut msg, None, 0, 0) };
        match result.0 {
            0 => break, // WM_QUIT
            -1 => bail!("GetMessageW failed: {}", windows::core::Error::from_thread()),
            _ => unsafe {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }

        // Repainting here rather than in the window procedure keeps the tray out of
        // `wnd_proc`, which has no way to reach it. Any message wakes us, so no timer is
        // needed and the process stays idle while nothing changes.
        if UI_REFRESH_REQUESTED.swap(false, Ordering::SeqCst) {
            if let Some(tray) = tray.as_mut() {
                tray.refresh();
            }
        }
    }
    log::info!("Left Win32 message loop");

    UI_HWND.store(0, Ordering::SeqCst);
    Ok(())
}

unsafe fn create_ui_window() -> Result<HWND> {
    let class_name = w!("MoonwatcherSessionWindow");
    let instance = unsafe { GetModuleHandleW(None) }?;

    let window_class = WNDCLASSW {
        lpfnWndProc: Some(wnd_proc),
        hInstance: instance.into(),
        lpszClassName: class_name,
        ..Default::default()
    };
    if unsafe { RegisterClassW(&window_class) } == 0 {
        return Err(anyhow!("RegisterClassW failed: {}", windows::core::Error::from_thread()));
    }

    // WS_EX_TOOLWINDOW plus never calling ShowWindow() keeps this out of the taskbar and
    // out of Alt-Tab, while still being a top-level window as far as shutdown is concerned.
    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_TOOLWINDOW,
            class_name,
            w!("Moonwatch.rs"),
            WS_OVERLAPPED,
            CW_USEDEFAULT, CW_USEDEFAULT, 0, 0,
            None,
            None,
            Some(instance.into()),
            None,
        )
    }?;

    log::debug!("Created hidden session window {:?}", hwnd.0);
    Ok(hwnd)
}

/// Ask the worker to write everything it has and stop, and wait for it to finish.
fn flush_and_terminate_worker() {
    match UI_WORKER.get() {
        Some(worker) => { worker.terminate_and_wait(FLUSH_TIMEOUT); }
        None => log::error!("No worker handle available, buffered events will be lost"),
    }
}

unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        // The system is asking whether it may end the session. The documented contract is
        // to answer immediately and do the work in WM_ENDSESSION, so we only take a
        // non-destructive snapshot here: if some other application vetoes the shutdown we
        // must still be running afterwards.
        WM_QUERYENDSESSION => {
            log::info!("WM_QUERYENDSESSION (lparam={:#x}), writing buffered events", lparam.0);
            if let Some(worker) = UI_WORKER.get() {
                worker.flush_and_wait(FLUSH_TIMEOUT);
            }
            LRESULT(1) // TRUE: we do not block the shutdown
        }
        // The session really is ending (or the shutdown was cancelled, wparam == FALSE).
        WM_ENDSESSION => {
            if wparam.0 != 0 {
                log::info!("WM_ENDSESSION, session is ending - shutting down");
                flush_and_terminate_worker();
            } else {
                log::info!("WM_ENDSESSION, session end was cancelled - carrying on");
            }
            LRESULT(0)
        }
        WM_CLOSE => {
            log::info!("WM_CLOSE - shutting down");
            flush_and_terminate_worker();
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        WM_MOONWATCHER_WORKER_EXITED => {
            log::debug!("Worker thread has exited, leaving the message loop");
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        WM_DESTROY => {
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

/// Leave the message loop. Must be called from the UI thread (eg. from a tray menu callback).
pub fn quit_event_loop() {
    unsafe { PostQuitMessage(0) };
}

/// Wake the UI thread and ask it to leave the message loop. Safe to call from any thread.
pub fn request_ui_quit() {
    UI_QUIT_REQUESTED.store(true, Ordering::SeqCst);
    post_to_ui_window(WM_MOONWATCHER_WORKER_EXITED);
}

/// Wake the UI thread and ask it to repaint the tray. Safe to call from any thread.
pub fn request_ui_refresh() {
    UI_REFRESH_REQUESTED.store(true, Ordering::SeqCst);
    post_to_ui_window(WM_MOONWATCHER_REFRESH_UI);
}

fn post_to_ui_window(message: u32) {
    let hwnd = UI_HWND.load(Ordering::SeqCst);
    if hwnd != 0 {
        let hwnd = HWND(hwnd as *mut std::ffi::c_void);
        let _ = unsafe { PostMessageW(Some(hwnd), message, WPARAM(0), LPARAM(0)) };
    }
}

/// Show `path` in Explorer. Its exit code is not meaningful, so it is not checked.
pub fn open_path(path: &Path) -> Result<()> {
    Command::new("explorer").arg(path).spawn()?;
    Ok(())
}
