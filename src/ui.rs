use clipvault::{
    clipboard::{self, Capture, wide},
    model::{Kind, Preview, page_range},
    store::{Result, Store},
};
use std::{
    cell::RefCell,
    path::PathBuf,
    ptr,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    thread,
    time::Duration,
};
use windows_sys::Win32::{
    Foundation::*,
    Graphics::Gdi::*,
    System::{DataExchange::*, LibraryLoader::*, Registry::*, Threading::*},
    UI::{Controls::*, HiDpi::*, Input::KeyboardAndMouse::*, Shell::*, WindowsAndMessaging::*},
};

const TRAY: u32 = WM_APP + 1;
const OPEN: u32 = WM_APP + 2;
const KEY_SELECT: u32 = NIN_SELECT | NINF_KEY;
const DPI_UPDATE: u32 = WM_APP + 3;
const PREV: usize = 10;
const NEXT: usize = 11;
const MENU: usize = 12;
const PAUSE: usize = 20;
const STARTUP: usize = 21;
const CLEAR: usize = 22;
const QUIT: usize = 23;
const ROW: usize = 100;
const BG: u32 = 0x00231d18;
const CARD: u32 = 0x00332b24;
const INK: u32 = 0x00f8f3ee;
const MUTED: u32 = 0x00b9b2aa;
const ACCENT: u32 = 0x00b8e585;

#[derive(Clone)]
struct View {
    id: String,
    title: String,
    detail: String,
    kind: Kind,
    preview: Option<Preview>,
}
enum Command {
    Capture(Capture),
    Restore(String),
    Clear,
}
struct Update {
    entries: Option<Vec<View>>,
    status: String,
}
struct App {
    hwnd: HWND,
    buttons: Vec<HWND>,
    font: HFONT,
    heading: HFONT,
    icon: HICON,
    commands: Sender<Command>,
    updates: Receiver<Update>,
    entries: Vec<View>,
    page: usize,
    paused: bool,
    status: String,
    dpi: i32,
    row_height: i32,
    height: i32,
    capture_paused: Arc<AtomicBool>,
    taskbar: u32,
    startup: bool,
}
thread_local! { static APP: RefCell<Option<App>> = const { RefCell::new(None) }; }
thread_local! { static CAPTURE_REQUESTS: RefCell<Option<Sender<u32>>> = const { RefCell::new(None) }; }

pub fn run() -> Result<()> {
    let args: Vec<_> = std::env::args_os().collect();
    let data = args
        .windows(2)
        .find(|v| v[0] == "--data-dir")
        .map(|v| PathBuf::from(&v[1]))
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("LOCALAPPDATA").unwrap_or_else(|| ".".into()))
                .join("ClipVault")
        });
    let show = args.iter().any(|v| v == "--show");
    let isolated = args.iter().any(|v| v == "--data-dir");
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    use std::hash::{Hash, Hasher};
    data.hash(&mut hasher);
    unsafe {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let mutex = CreateMutexW(
            ptr::null(),
            0,
            wide(&format!("Local\\ClipVault-{:x}", hasher.finish())).as_ptr(),
        );
        if mutex.is_null() {
            return Err("Cannot create application lock".into());
        }
        if GetLastError() == ERROR_ALREADY_EXISTS {
            let other = FindWindowW(
                wide(&format!("ClipVault-{:x}", hasher.finish())).as_ptr(),
                ptr::null(),
            );
            if !other.is_null() {
                PostMessageW(other, OPEN, 0, 0);
            }
            CloseHandle(mutex);
            return Ok(());
        }
        let instance = GetModuleHandleW(ptr::null());
        let class = wide(&format!("ClipVault-{:x}", hasher.finish()));
        let icon = LoadImageW(instance, 1usize as _, IMAGE_ICON, 32, 32, 0) as HICON;
        let mut wc: WNDCLASSW = std::mem::zeroed();
        wc.lpfnWndProc = Some(wndproc);
        wc.hInstance = instance;
        wc.lpszClassName = class.as_ptr();
        wc.hCursor = LoadCursorW(ptr::null_mut(), IDC_ARROW);
        wc.hIcon = icon;
        wc.style = CS_DROPSHADOW;
        if RegisterClassW(&wc) == 0 {
            CloseHandle(mutex);
            return Err("Cannot register popup window".into());
        }
        let hwnd = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
            class.as_ptr(),
            wide("ClipVault").as_ptr(),
            WS_POPUP | WS_BORDER | WS_CLIPCHILDREN,
            0,
            0,
            420,
            740,
            ptr::null_mut(),
            ptr::null_mut(),
            instance,
            ptr::null(),
        );
        if hwnd.is_null() {
            CloseHandle(mutex);
            return Err("Cannot create popup window".into());
        }
        let dpi = GetDpiForWindow(hwnd) as i32;
        let font = CreateFontW(
            -14 * dpi / 96,
            0,
            0,
            0,
            400,
            0,
            0,
            0,
            DEFAULT_CHARSET as u32,
            0,
            0,
            CLEARTYPE_QUALITY as u32,
            0,
            wide("Segoe UI").as_ptr(),
        );
        let heading = CreateFontW(
            -23 * dpi / 96,
            0,
            0,
            0,
            600,
            0,
            0,
            0,
            DEFAULT_CHARSET as u32,
            0,
            0,
            CLEARTYPE_QUALITY as u32,
            0,
            wide("Segoe UI").as_ptr(),
        );
        let (tx, rx) = mpsc::channel();
        let (utx, urx) = mpsc::channel();
        let window = hwnd as usize;
        let capture_paused = Arc::new(AtomicBool::new(false));
        let (capture_tx, capture_rx) = mpsc::channel();
        CAPTURE_REQUESTS.with(|c| *c.borrow_mut() = Some(capture_tx));
        let capture_commands = tx.clone();
        let capture_updates = utx.clone();
        let paused = capture_paused.clone();
        let capture_thread = thread::spawn(move || {
            capture_worker(
                window,
                capture_rx,
                capture_commands,
                capture_updates,
                paused,
            )
        });
        let storage_thread = thread::spawn(move || worker(data, window, rx, utx));
        let mut app = App {
            hwnd,
            buttons: vec![],
            font,
            heading,
            icon,
            commands: tx,
            updates: urx,
            entries: vec![],
            page: 0,
            paused: false,
            status: "Starting encrypted history…".into(),
            dpi,
            row_height: 56,
            height: 740,
            capture_paused,
            taskbar: RegisterWindowMessageW(wide("TaskbarCreated").as_ptr()),
            startup: if isolated { false } else { startup_enabled() },
        };
        for n in 0..10 {
            app.add_button(ROW + n, "", true);
        }
        app.add_button(PREV, "← Previous", false);
        app.add_button(NEXT, "Next →", false);
        app.add_button(MENU, "•••", false);
        app.layout();
        if AddClipboardFormatListener(hwnd) == 0 {
            return Err("Windows refused the clipboard listener".into());
        }
        if RegisterHotKey(hwnd, 1, MOD_CONTROL | MOD_SHIFT | MOD_NOREPEAT, 0x56) == 0 {
            app.status = "Ctrl+Shift+V is in use. Open ClipVault from its tray icon.".into();
        }
        app.tray(NIM_ADD);
        SetTimer(hwnd, 1, 100, None);
        APP.with(|v| *v.borrow_mut() = Some(app));
        if show {
            PostMessageW(hwnd, OPEN, 0, 0);
        }
        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, ptr::null_mut(), 0, 0) > 0 {
            if msg.message == WM_KEYDOWN
                && IsWindowVisible(hwnd) != 0
                && (msg.hwnd == hwnd || IsChild(hwnd, msg.hwnd) != 0)
            {
                match msg.wParam as u16 {
                    VK_ESCAPE => {
                        ShowWindow(hwnd, SW_HIDE);
                        continue;
                    }
                    VK_LEFT => {
                        PostMessageW(hwnd, WM_COMMAND, PREV, 0);
                        continue;
                    }
                    VK_RIGHT => {
                        PostMessageW(hwnd, WM_COMMAND, NEXT, 0);
                        continue;
                    }
                    _ => {}
                }
            }
            if IsDialogMessageW(hwnd, &msg) == 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        APP.with(|v| {
            if let Some(app) = v.borrow_mut().take() {
                app.tray(NIM_DELETE);
                DeleteObject(app.font);
                DeleteObject(app.heading);
                if !app.icon.is_null() {
                    DestroyIcon(app.icon);
                }
            }
        });
        CAPTURE_REQUESTS.with(|c| *c.borrow_mut() = None);
        let _ = capture_thread.join();
        let _ = storage_thread.join();
        CloseHandle(mutex);
    }
    Ok(())
}

fn capture_worker(
    window: usize,
    requests: Receiver<u32>,
    commands: Sender<Command>,
    updates: Sender<Update>,
    paused: Arc<AtomicBool>,
) {
    for sequence in requests {
        if paused.load(Ordering::Relaxed) {
            continue;
        }
        if unsafe { GetClipboardOwner() } == window as HWND {
            continue;
        }
        if unsafe { GetClipboardSequenceNumber() } != sequence {
            continue;
        }
        match clipboard::read(window as HWND, true) {
            Ok(Some(capture)) => {
                if commands.send(Command::Capture(capture)).is_err() {
                    break;
                }
            }
            Ok(None) => {}
            Err(error) => {
                let _ = updates.send(Update {
                    entries: None,
                    status: error,
                });
            }
        }
    }
}

fn worker(root: PathBuf, window: usize, commands: Receiver<Command>, updates: Sender<Update>) {
    let mut store = match Store::open(root) {
        Ok(v) => v,
        Err(e) => {
            let _ = updates.send(Update {
                entries: Some(vec![]),
                status: format!("History could not open: {e}"),
            });
            return;
        }
    };
    fn publish(store: &Store, updates: &Sender<Update>, status: String) {
        let entries = store
            .entries
            .iter()
            .map(|r| View {
                id: r.id.clone(),
                title: r.title.clone(),
                detail: r.detail.clone(),
                kind: r.kind.clone(),
                preview: r.preview.clone(),
            })
            .collect();
        let _ = updates.send(Update {
            entries: Some(entries),
            status,
        });
    }
    publish(
        &store,
        &updates,
        store
            .warnings
            .first()
            .cloned()
            .unwrap_or_else(|| "Ready · history stays on this PC".into()),
    );
    for command in commands {
        let result: Result<String> = (|| match command {
            Command::Capture(capture) => {
                if !capture.sources.is_empty() {
                    publish(&store, &updates, "Saving file snapshot…".into());
                }
                let sources = capture.sources.clone();
                store.insert(capture.into_record(), &sources)?;
                Ok("Ready · history stays on this PC".into())
            }
            Command::Restore(id) => {
                publish(&store, &updates, "Preparing item for the clipboard…".into());
                let paths = store.materialize(&id)?;
                let record = store
                    .entries
                    .iter()
                    .find(|r| r.id == id)
                    .ok_or("Entry was removed")?;
                let mut result = Err("Clipboard is busy".into());
                for _ in 0..12 {
                    result = clipboard::write(window as HWND, &record.formats, &paths);
                    if result.is_ok() {
                        break;
                    }
                    thread::sleep(Duration::from_millis(40));
                }
                result?;
                Ok("Copied · paste with Ctrl+V".into())
            }
            Command::Clear => {
                // Staged file paths must not outlive their payloads on the clipboard.
                if let Some(current) = clipboard::read(window as HWND, false)?
                    && current
                        .sources
                        .iter()
                        .any(|p| p.starts_with(store.root().join("staging")))
                {
                    clipboard::write(window as HWND, &[], &[])?;
                }
                store.clear()?;
                Ok("History cleared".into())
            }
        })();
        publish(
            &store,
            &updates,
            result.unwrap_or_else(|e| format!("Could not complete: {e}")),
        );
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        if msg == WM_CLIPBOARDUPDATE {
            CAPTURE_REQUESTS.with(|c| {
                if let Some(sender) = c.borrow().as_ref() {
                    let _ = sender.send(GetClipboardSequenceNumber());
                }
            });
            return 0;
        }
        if msg == WM_DPICHANGED {
            PostMessageW(hwnd, DPI_UPDATE, wp, 0);
            return 0;
        }
        if msg == WM_DESTROY {
            RemoveClipboardFormatListener(hwnd);
            UnregisterHotKey(hwnd, 1);
            PostQuitMessage(0);
            return 0;
        }
        APP.with(|cell| {
            // Win32 may send synchronous messages while a control is being updated.
            // Never create overlapping mutable references during that reentrancy.
            let Ok(mut slot)=cell.try_borrow_mut() else {return DefWindowProcW(hwnd,msg,wp,lp)};
            let Some(app)=slot.as_mut() else {return DefWindowProcW(hwnd,msg,wp,lp)};
            if msg==app.taskbar { app.tray(NIM_ADD); return 0; }
            match msg {
                OPEN|WM_HOTKEY=>{app.toggle();0}
                TRAY=>{match lp as u32 & 0xffff {NIN_SELECT|KEY_SELECT=>app.toggle(),WM_RBUTTONUP|WM_CONTEXTMENU=>app.menu(),_=>{}}0}
                WM_TIMER=>{
                    while let Ok(update)=app.updates.try_recv() { if let Some(entries)=update.entries {app.entries=entries;} app.status=update.status; app.refresh(); }
                    0
                }
                WM_COMMAND=>{
                    let command=wp & 0xffff;
                    match command {
                        PREV=>{app.page=app.page.saturating_sub(1);app.refresh();}
                        NEXT=>{app.page=(app.page+1).min(9);app.refresh();}
                        MENU=>app.menu(),
                        PAUSE=>{app.paused = !app.paused;app.capture_paused.store(app.paused,Ordering::Relaxed);app.status=if app.paused {"Capture paused".into()} else {"Capture resumed".into()};app.refresh();}
                        STARTUP=>{match set_startup(!app.startup) {Ok(())=>{app.startup = !app.startup;app.status="Sign-in preference updated".into();},Err(e)=>app.status=e}app.refresh();}
                        CLEAR=>{
                            let answer=MessageBoxW(hwnd,wide("Permanently clear all saved clipboard history and file snapshots?").as_ptr(),wide("Clear ClipVault history").as_ptr(),MB_YESNO|MB_ICONQUESTION|MB_DEFBUTTON2);
                            if answer==IDYES {let _=app.commands.send(Command::Clear);}
                        }
                        QUIT=>{DestroyWindow(hwnd);}
                        n if (ROW..ROW+10).contains(&n)=>{
                            if let Some(entry)=app.entries.get(app.page*10+n-ROW) {let _=app.commands.send(Command::Restore(entry.id.clone())); ShowWindow(hwnd,SW_HIDE);}
                        }
                        _=>{}
                    }0
                }
                WM_ACTIVATE=>{if wp & 0xffff == WA_INACTIVE as usize {ShowWindow(hwnd,SW_HIDE);}0}
                WM_CLOSE=>{ShowWindow(hwnd,SW_HIDE);0}
                DPI_UPDATE=>{app.update_dpi((wp&0xffff) as i32);0}
                WM_ERASEBKGND=>1,
                WM_PAINT=>{let mut ps=std::mem::zeroed();let dc=BeginPaint(hwnd,&mut ps);app.paint(dc);EndPaint(hwnd,&ps);0}
                WM_DRAWITEM=>{app.draw_button(&*(lp as *const DRAWITEMSTRUCT));1}
                _=>DefWindowProcW(hwnd,msg,wp,lp),
            }
        })
    }
}

impl App {
    fn update_dpi(&mut self, dpi: i32) {
        if self.dpi == dpi {
            return;
        }
        self.dpi = dpi;
        unsafe {
            DeleteObject(self.font);
            DeleteObject(self.heading);
            self.font = CreateFontW(
                -14 * dpi / 96,
                0,
                0,
                0,
                400,
                0,
                0,
                0,
                DEFAULT_CHARSET as u32,
                0,
                0,
                CLEARTYPE_QUALITY as u32,
                0,
                wide("Segoe UI").as_ptr(),
            );
            self.heading = CreateFontW(
                -23 * dpi / 96,
                0,
                0,
                0,
                600,
                0,
                0,
                0,
                DEFAULT_CHARSET as u32,
                0,
                0,
                CLEARTYPE_QUALITY as u32,
                0,
                wide("Segoe UI").as_ptr(),
            );
            for button in &self.buttons {
                SendMessageW(*button, WM_SETFONT, self.font as usize, 1);
            }
        }
        self.layout();
    }
    fn px(&self, v: i32) -> i32 {
        v * self.dpi / 96
    }
    fn add_button(&mut self, id: usize, label: &str, _row: bool) {
        unsafe {
            let button = CreateWindowExW(
                0,
                wide("BUTTON").as_ptr(),
                wide(label).as_ptr(),
                WS_CHILD | WS_VISIBLE | WS_TABSTOP | BS_OWNERDRAW as u32,
                0,
                0,
                0,
                0,
                self.hwnd,
                id as _,
                GetModuleHandleW(ptr::null()),
                ptr::null(),
            );
            SendMessageW(button, WM_SETFONT, self.font as usize, 1);
            self.buttons.push(button);
        }
    }
    fn layout(&mut self) {
        unsafe {
            let monitor = MonitorFromWindow(self.hwnd, MONITOR_DEFAULTTONEAREST);
            let mut info: MONITORINFO = std::mem::zeroed();
            info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
            GetMonitorInfoW(monitor, &mut info);
            let available = (info.rcWork.bottom - info.rcWork.top) * 96 / self.dpi;
            self.row_height = ((available - 180) / 10).clamp(30, 56);
            self.height = 150 + self.row_height * 10;
            SetWindowPos(
                self.hwnd,
                ptr::null_mut(),
                0,
                0,
                self.px(420),
                self.px(self.height),
                SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
            );
            for n in 0..10 {
                MoveWindow(
                    self.buttons[n],
                    self.px(12),
                    self.px(88 + n as i32 * self.row_height),
                    self.px(394),
                    self.px(self.row_height - 4),
                    1,
                );
            }
            MoveWindow(
                self.buttons[10],
                self.px(12),
                self.px(self.height - 49),
                self.px(110),
                self.px(32),
                1,
            );
            MoveWindow(
                self.buttons[11],
                self.px(296),
                self.px(self.height - 49),
                self.px(110),
                self.px(32),
                1,
            );
            MoveWindow(
                self.buttons[12],
                self.px(366),
                self.px(18),
                self.px(40),
                self.px(32),
                1,
            );
            self.refresh();
        }
    }
    fn refresh(&self) {
        unsafe {
            for n in 0..10 {
                if let Some(item) = self.entries.get(self.page * 10 + n) {
                    SetWindowTextW(
                        self.buttons[n],
                        wide(&format!(
                            "{}: {}. {}",
                            self.page * 10 + n + 1,
                            item.title,
                            item.detail
                        ))
                        .as_ptr(),
                    );
                    ShowWindow(self.buttons[n], SW_SHOWNA);
                } else {
                    ShowWindow(self.buttons[n], SW_HIDE);
                }
            }
            EnableWindow(self.buttons[10], (self.page > 0) as i32);
            EnableWindow(self.buttons[11], (self.page < 9) as i32);
            InvalidateRect(self.hwnd, ptr::null(), 0);
            for button in &self.buttons {
                InvalidateRect(*button, ptr::null(), 0);
            }
        }
    }
    fn tray(&self, action: u32) {
        unsafe {
            let mut tray: NOTIFYICONDATAW = std::mem::zeroed();
            tray.cbSize = std::mem::size_of_val(&tray) as u32;
            tray.hWnd = self.hwnd;
            tray.uID = 1;
            tray.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
            tray.uCallbackMessage = TRAY;
            tray.hIcon = self.icon;
            let tip = wide("ClipVault · Ctrl+Shift+V");
            tray.szTip[..tip.len()].copy_from_slice(&tip);
            Shell_NotifyIconW(action, &tray);
            if action == NIM_ADD {
                tray.Anonymous.uVersion = NOTIFYICON_VERSION_4;
                Shell_NotifyIconW(NIM_SETVERSION, &tray);
            }
        }
    }
    fn toggle(&mut self) {
        unsafe {
            if IsWindowVisible(self.hwnd) != 0 {
                ShowWindow(self.hwnd, SW_HIDE);
                return;
            }
            self.page = 0;
            self.layout();
            let mut anchor: RECT = std::mem::zeroed();
            let mut id: NOTIFYICONIDENTIFIER = std::mem::zeroed();
            id.cbSize = std::mem::size_of_val(&id) as u32;
            id.hWnd = self.hwnd;
            id.uID = 1;
            if Shell_NotifyIconGetRect(&id, &mut anchor) < 0 {
                let mut p = POINT { x: 0, y: 0 };
                GetCursorPos(&mut p);
                anchor = RECT {
                    left: p.x,
                    right: p.x,
                    top: p.y,
                    bottom: p.y,
                };
            }
            let monitor = MonitorFromRect(&anchor, MONITOR_DEFAULTTONEAREST);
            let mut info: MONITORINFO = std::mem::zeroed();
            info.cbSize = std::mem::size_of_val(&info) as u32;
            GetMonitorInfoW(monitor, &mut info);
            let area = info.rcWork;
            let width = self.px(420);
            let height = self.px(self.height);
            let x = (anchor.right - width).clamp(area.left, (area.right - width).max(area.left));
            let y = if anchor.top - height >= area.top {
                anchor.top - height - self.px(8)
            } else {
                anchor.bottom + self.px(8)
            };
            let y = y.clamp(area.top, (area.bottom - height).max(area.top));
            SetWindowPos(self.hwnd, HWND_TOPMOST, x, y, width, height, SWP_SHOWWINDOW);
            SetForegroundWindow(self.hwnd);
            SetFocus(self.buttons[0]);
            self.refresh();
        }
    }
    fn menu(&self) {
        unsafe {
            let menu = CreatePopupMenu();
            for (id, label, checked) in [
                (
                    PAUSE,
                    if self.paused {
                        "Resume capture"
                    } else {
                        "Pause capture"
                    },
                    self.paused,
                ),
                (STARTUP, "Start at sign-in", self.startup),
                (CLEAR, "Clear history…", false),
                (QUIT, "Exit ClipVault", false),
            ] {
                AppendMenuW(
                    menu,
                    MF_STRING | if checked { MF_CHECKED } else { 0 },
                    id,
                    wide(label).as_ptr(),
                );
            }
            let mut p = POINT { x: 0, y: 0 };
            GetCursorPos(&mut p);
            SetForegroundWindow(self.hwnd);
            let chosen = TrackPopupMenu(
                menu,
                TPM_RETURNCMD | TPM_RIGHTBUTTON,
                p.x,
                p.y,
                0,
                self.hwnd,
                ptr::null(),
            );
            DestroyMenu(menu);
            if chosen > 0 {
                PostMessageW(self.hwnd, WM_COMMAND, chosen as usize, 0);
            }
            PostMessageW(self.hwnd, WM_NULL, 0, 0);
        }
    }
    fn paint(&self, dc: HDC) {
        let mut client = RECT {
            left: 0,
            top: 0,
            right: self.px(420),
            bottom: self.px(self.height),
        };
        fill(dc, &client, BG);
        text(
            dc,
            "ClipVault",
            RECT {
                left: self.px(18),
                top: self.px(14),
                right: self.px(350),
                bottom: self.px(46),
            },
            self.heading,
            INK,
            DT_SINGLELINE,
        );
        text(
            dc,
            &format!(
                "{} / 100 items  ·  {}",
                self.entries.len(),
                if self.paused {
                    "Paused"
                } else {
                    "Clipboard history"
                }
            ),
            RECT {
                left: self.px(18),
                top: self.px(47),
                right: self.px(400),
                bottom: self.px(67),
            },
            self.font,
            ACCENT,
            DT_SINGLELINE,
        );
        let status = if self.paused {
            "Capture paused · resume from the ••• menu"
        } else {
            &self.status
        };
        text(
            dc,
            status,
            RECT {
                left: self.px(18),
                top: self.px(68),
                right: self.px(401),
                bottom: self.px(86),
            },
            self.font,
            MUTED,
            DT_SINGLELINE | DT_END_ELLIPSIS,
        );
        text(
            dc,
            &format!("{} / 10", self.page + 1),
            RECT {
                left: self.px(130),
                top: self.px(self.height - 43),
                right: self.px(288),
                bottom: self.px(self.height - 20),
            },
            self.font,
            MUTED,
            DT_SINGLELINE | DT_CENTER,
        );
        if page_range(self.page, self.entries.len()).is_empty() {
            client.top = self.px(135);
            client.bottom = self.px(self.height - 80);
            client.left = self.px(35);
            client.right = self.px(385);
            text(
                dc,
                if self.entries.is_empty() {
                    "Your clipboard, remembered.\n\nCopy text, an image, or files.\nThey will appear here.\n\nClick an item to copy it again.\nThen paste with Ctrl+V."
                } else {
                    "No items on this page yet.\n\nUse the left arrow for newer items."
                },
                client,
                self.font,
                MUTED,
                DT_CENTER | DT_WORDBREAK,
            );
        }
    }
    fn draw_button(&self, draw: &DRAWITEMSTRUCT) {
        let dc = draw.hDC;
        let r = draw.rcItem;
        let selected = draw.itemState & ODS_SELECTED != 0;
        fill(dc, &r, if selected { 0x00524636 } else { CARD });
        let id = draw.CtlID as usize;
        if (ROW..ROW + 10).contains(&id) {
            if let Some(entry) = self.entries.get(self.page * 10 + id - ROW) {
                let pad = self.px(10);
                let box_size = (r.bottom - r.top - self.px(10)).min(self.px(42));
                let left = r.left + pad;
                let top = r.top + (r.bottom - r.top - box_size) / 2;
                if let Some(preview) = &entry.preview {
                    thumbnail(dc, preview, left, top, box_size);
                } else {
                    text(
                        dc,
                        match entry.kind {
                            Kind::Text => "T",
                            Kind::Files => "FILE",
                            Kind::Image => "IMG",
                        },
                        RECT {
                            left,
                            top: top + self.px(7),
                            right: left + box_size,
                            bottom: top + box_size,
                        },
                        self.font,
                        ACCENT,
                        DT_CENTER | DT_SINGLELINE,
                    );
                }
                let title = entry.title.replace(['\n', '\r', '\t'], " ");
                let text_left = left + box_size + pad;
                text(
                    dc,
                    &title,
                    RECT {
                        left: text_left,
                        top: r.top + self.px(5),
                        right: r.right - pad,
                        bottom: r.top + self.px(26),
                    },
                    self.font,
                    INK,
                    DT_SINGLELINE | DT_END_ELLIPSIS,
                );
                if self.row_height >= 42 {
                    text(
                        dc,
                        &entry.detail,
                        RECT {
                            left: text_left,
                            top: r.top + self.px(27),
                            right: r.right - pad,
                            bottom: r.bottom - self.px(3),
                        },
                        self.font,
                        MUTED,
                        DT_SINGLELINE | DT_END_ELLIPSIS,
                    );
                }
            }
        } else {
            text(
                dc,
                match id {
                    PREV => "← Previous",
                    NEXT => "Next →",
                    _ => "•••",
                },
                RECT {
                    left: r.left,
                    top: r.top + self.px(6),
                    right: r.right,
                    bottom: r.bottom,
                },
                self.font,
                if draw.itemState & ODS_DISABLED != 0 {
                    MUTED
                } else {
                    INK
                },
                DT_CENTER | DT_SINGLELINE,
            );
        }
        if draw.itemState & ODS_FOCUS != 0 {
            unsafe {
                DrawFocusRect(dc, &r);
            }
        }
    }
}

fn fill(dc: HDC, r: &RECT, color: u32) {
    unsafe {
        let brush = CreateSolidBrush(color);
        FillRect(dc, r, brush);
        DeleteObject(brush);
    }
}
fn text(dc: HDC, value: &str, mut r: RECT, font: HFONT, color: u32, flags: u32) {
    unsafe {
        let old = SelectObject(dc, font);
        SetBkMode(dc, TRANSPARENT as i32);
        SetTextColor(dc, color);
        let w = wide(value);
        DrawTextW(
            dc,
            w.as_ptr(),
            (w.len() - 1) as i32,
            &mut r,
            flags | DT_NOPREFIX,
        );
        SelectObject(dc, old);
    }
}
fn thumbnail(dc: HDC, p: &Preview, x: i32, y: i32, size: i32) {
    let mut pixels = Vec::with_capacity(p.rgba.len());
    for (n, rgba) in p.rgba.chunks_exact(4).enumerate() {
        let bg = if (n as u32 % p.width / 8 + n as u32 / p.width / 8).is_multiple_of(2) {
            210u32
        } else {
            170
        };
        let a = rgba[3] as u32;
        for c in [rgba[2], rgba[1], rgba[0]] {
            pixels.push(((c as u32 * a + bg * (255 - a)) / 255) as u8);
        }
        pixels.push(255);
    }
    unsafe {
        let mut bmi: BITMAPINFO = std::mem::zeroed();
        bmi.bmiHeader.biSize = 40;
        bmi.bmiHeader.biWidth = p.width as i32;
        bmi.bmiHeader.biHeight = -(p.height as i32);
        bmi.bmiHeader.biPlanes = 1;
        bmi.bmiHeader.biBitCount = 32;
        let ratio = size as f64 / p.width.max(p.height) as f64;
        let w = (p.width as f64 * ratio) as i32;
        let h = (p.height as f64 * ratio) as i32;
        StretchDIBits(
            dc,
            x + (size - w) / 2,
            y + (size - h) / 2,
            w,
            h,
            0,
            0,
            p.width as i32,
            p.height as i32,
            pixels.as_ptr().cast(),
            &bmi,
            DIB_RGB_COLORS,
            SRCCOPY,
        );
    }
}
fn startup_enabled() -> bool {
    unsafe {
        let mut key = ptr::null_mut();
        if RegOpenKeyExW(
            HKEY_CURRENT_USER,
            wide("Software\\Microsoft\\Windows\\CurrentVersion\\Run").as_ptr(),
            0,
            KEY_QUERY_VALUE,
            &mut key,
        ) != 0
        {
            return false;
        }
        let ok = RegQueryValueExW(
            key,
            wide("ClipVault").as_ptr(),
            ptr::null(),
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
        ) == 0;
        RegCloseKey(key);
        ok
    }
}
fn set_startup(enabled: bool) -> Result<()> {
    unsafe {
        let mut key = ptr::null_mut();
        let code = RegCreateKeyExW(
            HKEY_CURRENT_USER,
            wide("Software\\Microsoft\\Windows\\CurrentVersion\\Run").as_ptr(),
            0,
            ptr::null(),
            0,
            KEY_SET_VALUE,
            ptr::null(),
            &mut key,
            ptr::null_mut(),
        );
        if code != 0 {
            return Err(format!("Cannot change sign-in setting: {code}"));
        }
        let code = if enabled {
            let exe = std::env::current_exe().map_err(|e| e.to_string())?;
            let value = wide(&format!("\"{}\"", exe.display()));
            RegSetValueExW(
                key,
                wide("ClipVault").as_ptr(),
                0,
                REG_SZ,
                value.as_ptr().cast(),
                (value.len() * 2) as u32,
            )
        } else {
            RegDeleteValueW(key, wide("ClipVault").as_ptr())
        };
        RegCloseKey(key);
        if code != 0 {
            Err(format!("Cannot change sign-in setting: {code}"))
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_popup_pages_ten_rows_and_restores_the_selected_entry() {
        unsafe {
            let instance = GetModuleHandleW(ptr::null());
            let class = wide("ClipVaultHiddenUiTest");
            let mut wc: WNDCLASSW = std::mem::zeroed();
            wc.hInstance = instance;
            wc.lpfnWndProc = Some(wndproc);
            wc.lpszClassName = class.as_ptr();
            assert_ne!(RegisterClassW(&wc), 0);
            let hwnd = CreateWindowExW(
                WS_EX_TOOLWINDOW,
                class.as_ptr(),
                wide("ClipVault UI test").as_ptr(),
                WS_POPUP,
                0,
                0,
                420,
                740,
                ptr::null_mut(),
                ptr::null_mut(),
                instance,
                ptr::null(),
            );
            assert!(!hwnd.is_null());
            let (tx, commands) = mpsc::channel();
            let (_, rx) = mpsc::channel();
            let mut app = App {
                hwnd,
                buttons: vec![],
                font: ptr::null_mut(),
                heading: ptr::null_mut(),
                icon: ptr::null_mut(),
                commands: tx,
                updates: rx,
                entries: (0..100)
                    .map(|n| View {
                        id: format!("id{n}"),
                        title: format!("Test item {n}"),
                        detail: "text".into(),
                        kind: Kind::Text,
                        preview: None,
                    })
                    .collect(),
                page: 0,
                paused: false,
                status: "Test".into(),
                dpi: 96,
                row_height: 56,
                height: 710,
                capture_paused: Arc::new(AtomicBool::new(false)),
                taskbar: u32::MAX,
                startup: false,
            };
            for n in 0..10 {
                app.add_button(ROW + n, "", true);
            }
            app.add_button(PREV, "Previous", false);
            app.add_button(NEXT, "Next", false);
            app.add_button(MENU, "Menu", false);
            app.layout();
            APP.with(|c| *c.borrow_mut() = Some(app));
            for _ in 0..12 {
                SendMessageW(hwnd, WM_COMMAND, NEXT, 0);
            }
            APP.with(|c| {
                let borrow = c.borrow();
                let app = borrow.as_ref().unwrap();
                assert_eq!(app.page, 9);
                assert_eq!(IsWindowEnabled(app.buttons[11]), 0);
                for n in 0..10 {
                    let mut text = [0u16; 256];
                    let len = GetWindowTextW(app.buttons[n], text.as_mut_ptr(), 256);
                    assert!(
                        String::from_utf16_lossy(&text[..len as usize])
                            .contains(&format!("Test item {}", 90 + n))
                    );
                }
            });
            SendMessageW(hwnd, WM_COMMAND, ROW + 2, 0);
            match commands.try_recv().unwrap() {
                Command::Restore(id) => assert_eq!(id, "id92"),
                _ => panic!("expected restore"),
            }
            for _ in 0..12 {
                SendMessageW(hwnd, WM_COMMAND, PREV, 0);
            }
            APP.with(|c| assert_eq!(c.borrow().as_ref().unwrap().page, 0));
            APP.with(|c| *c.borrow_mut() = None);
            DestroyWindow(hwnd);
            UnregisterClassW(class.as_ptr(), instance);
        }
    }
    #[test]
    fn clipboard_notifications_survive_modal_reentrancy() {
        let (tx, rx) = mpsc::channel();
        CAPTURE_REQUESTS.with(|c| *c.borrow_mut() = Some(tx));
        APP.with(|c| {
            let _modal_borrow = c.borrow_mut();
            unsafe {
                wndproc(ptr::null_mut(), WM_CLIPBOARDUPDATE, 0, 0);
            }
        });
        assert!(
            rx.try_recv().is_ok(),
            "a menu's nested message loop must not drop clipboard notifications"
        );
        CAPTURE_REQUESTS.with(|c| *c.borrow_mut() = None);
    }
}
