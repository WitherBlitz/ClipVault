use clipvault::{
    clipboard::{self, Capture, wide},
    model::{Kind, Preview, Record, page_range},
    store::{Result, Store},
    sync::{self, PeerHistory, SyncEvent, SyncHandle},
    transfer::{self, EntrySummary as View},
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
const SYNC: usize = 24;
const ROW: usize = 100;
const TAB: usize = 200;
const TAB_PREV: usize = 203;
const TAB_NEXT: usize = 204;
const BG: u32 = 0x00231d18;
const CARD: u32 = 0x00332b24;
const INK: u32 = 0x00f8f3ee;
const MUTED: u32 = 0x00b9b2aa;
const ACCENT: u32 = 0x00b8e585;

#[derive(Default)]
struct Sources {
    local_name: String,
    local: Vec<View>,
    peers: Vec<PeerHistory>,
    selected: Option<String>,
    tab_page: usize,
}
impl Sources {
    fn entries(&self) -> &[View] {
        self.selected
            .as_ref()
            .and_then(|id| self.peers.iter().find(|p| &p.id == id))
            .map(|p| p.entries.as_slice())
            .unwrap_or(&self.local)
    }
    fn name(&self) -> &str {
        self.selected
            .as_ref()
            .and_then(|id| self.peers.iter().find(|p| &p.id == id))
            .map(|p| p.name.as_str())
            .unwrap_or(&self.local_name)
    }
    fn count(&self) -> usize {
        self.peers.len() + 1
    }
    fn selected_index(&self) -> usize {
        self.selected
            .as_ref()
            .and_then(|id| self.peers.iter().position(|p| &p.id == id))
            .map(|n| n + 1)
            .unwrap_or(0)
    }
    fn select(&mut self, index: usize) {
        if index < self.count() {
            self.selected = if index == 0 {
                None
            } else {
                Some(self.peers[index - 1].id.clone())
            };
        }
    }
    fn update(&mut self, peers: Vec<PeerHistory>) {
        self.peers = peers;
        if self
            .selected
            .as_ref()
            .is_some_and(|id| !self.peers.iter().any(|p| &p.id == id))
        {
            self.selected = None;
        }
        self.tab_page = self.tab_page.min((self.count() - 1) / 3);
    }
    fn tab_name(&self, index: usize) -> String {
        if index == 0 {
            format!("{} (this PC)", self.local_name)
        } else {
            let peer = &self.peers[index - 1];
            format!(
                "{}{}",
                peer.name,
                if peer.online { "" } else { " · offline" }
            )
        }
    }
}
enum Command {
    Capture(Capture),
    Restore(String),
    RestoreRemote(String, String),
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
    sources: Sources,
    network: Option<SyncHandle>,
    sync_updates: Receiver<SyncEvent>,
    control_updates: Sender<Update>,
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
            if show && !other.is_null() {
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
        let default_sync = !isolated && std::env::var_os("CLIPVAULT_NO_SYNC").is_none();
        let (network, sync_updates) = match sync::start(data.clone(), default_sync) {
            Ok((network, events)) => (Some(network), events),
            Err(error) => {
                let (sender, events) = mpsc::channel();
                let _ = sender.send(SyncEvent::Status(format!("Sync unavailable: {error}")));
                (None, events)
            }
        };
        let local_name = network
            .as_ref()
            .map(|n| n.local_name())
            .unwrap_or_else(|| std::env::var("COMPUTERNAME").unwrap_or_else(|_| "This PC".into()));
        let worker_network = network.clone();
        let control_updates = utx.clone();
        let storage_thread = thread::spawn(move || worker(data, window, rx, utx, worker_network));
        let mut app = App {
            hwnd,
            buttons: vec![],
            font,
            heading,
            icon,
            commands: tx,
            updates: urx,
            sources: Sources {
                local_name,
                ..Sources::default()
            },
            network,
            sync_updates,
            control_updates,
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
        app.add_tabs();
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
                if let Some(network) = &app.network {
                    network.shutdown();
                }
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

fn worker(
    root: PathBuf,
    window: usize,
    commands: Receiver<Command>,
    updates: Sender<Update>,
    network: Option<SyncHandle>,
) {
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
                created: r.created,
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
            .unwrap_or_else(|| "Ready · select an item to copy".into()),
    );
    if let Some(network) = &network {
        network.publish(transfer::summaries(&store));
    }
    for command in commands {
        let result: Result<String> = (|| match command {
            Command::Capture(capture) => {
                if !capture.sources.is_empty() {
                    publish(&store, &updates, "Saving file snapshot…".into());
                }
                let sources = capture.sources.clone();
                store.insert(capture.into_record(), &sources)?;
                Ok("Ready · select an item to copy".into())
            }
            Command::Restore(id) => {
                publish(&store, &updates, "Preparing item for the clipboard…".into());
                let paths = store.materialize(&id)?;
                let record = store
                    .entries
                    .iter()
                    .find(|r| r.id == id)
                    .ok_or("Entry was removed")?;
                restore_clipboard(window, record, &paths)?;
                Ok("Copied · paste with Ctrl+V".into())
            }
            Command::RestoreRemote(peer, id) => {
                publish(
                    &store,
                    &updates,
                    "Downloading item from the selected PC…".into(),
                );
                let network = network.as_ref().ok_or("Network sync is unavailable")?;
                let (record, paths) = network.restore(&peer, &id)?;
                restore_clipboard(window, &record, &paths)?;
                Ok("Copied from remote PC · paste with Ctrl+V".into())
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
        if let Some(network) = &network {
            network.publish(transfer::summaries(&store));
        }
    }
}

fn restore_clipboard(window: usize, record: &Record, paths: &[PathBuf]) -> Result<()> {
    let mut result = Err("Clipboard is busy".into());
    for _ in 0..12 {
        result = clipboard::write(window as HWND, &record.formats, paths);
        if result.is_ok() {
            break;
        }
        thread::sleep(Duration::from_millis(40));
    }
    result.map(|_| ())
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
                    while let Ok(update)=app.updates.try_recv() { if let Some(entries)=update.entries {app.sources.local=entries;} app.status=update.status; app.refresh(); }
                    while let Ok(update)=app.sync_updates.try_recv() {
                        match update {SyncEvent::Peers(peers)=>app.sources.update(peers),SyncEvent::Status(status)=>app.status=status}
                        app.refresh();
                    }
                    0
                }
                WM_COMMAND=>{
                    let command=wp & 0xffff;
                    match command {
                        PREV=>{app.page=app.page.saturating_sub(1);app.refresh();}
                        NEXT=>{app.page=(app.page+1).min(9);app.refresh();}
                        TAB_PREV=>{app.sources.tab_page=app.sources.tab_page.saturating_sub(1);app.refresh();}
                        TAB_NEXT=>{app.sources.tab_page=(app.sources.tab_page+1).min((app.sources.count()-1)/3);app.refresh();}
                        n if (TAB..TAB+3).contains(&n)=>{app.sources.select(app.sources.tab_page*3+n-TAB);app.page=0;app.refresh();}
                        MENU=>app.menu(),
                        PAUSE=>{app.paused = !app.paused;app.capture_paused.store(app.paused,Ordering::Relaxed);app.status=if app.paused {"Capture paused".into()} else {"Capture resumed".into()};app.refresh();}
                        STARTUP=>{match set_startup(!app.startup) {Ok(())=>{app.startup = !app.startup;app.status="Sign-in preference updated".into();},Err(e)=>app.status=e}app.refresh();}
                        SYNC=>app.toggle_sync(),
                        CLEAR=>{
                            let answer=MessageBoxW(hwnd,wide("Permanently clear this computer's saved clipboard history and file snapshots?").as_ptr(),wide("Clear local ClipVault history").as_ptr(),MB_YESNO|MB_ICONQUESTION|MB_DEFBUTTON2);
                            if answer==IDYES {let _=app.commands.send(Command::Clear);}
                        }
                        QUIT=>{DestroyWindow(hwnd);}
                        n if (ROW..ROW+10).contains(&n)=>{
                            if let Some(entry)=app.sources.entries().get(app.page*10+n-ROW) {
                                let command=if let Some(peer)=&app.sources.selected {Command::RestoreRemote(peer.clone(),entry.id.clone())} else {Command::Restore(entry.id.clone())};
                                let _=app.commands.send(command); ShowWindow(hwnd,SW_HIDE);
                            }
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
    fn toggle_sync(&self) {
        if let Some(network) = self.network.clone() {
            let enabled = !network.is_enabled();
            let updates = self.control_updates.clone();
            // This control path must not queue behind an active file download.
            thread::spawn(move || {
                let status = match network.set_enabled(enabled) {
                    Ok(()) => {
                        if enabled {
                            "All LAN/Tailscale PCs can read this history".into()
                        } else {
                            "Network sharing disabled".into()
                        }
                    }
                    Err(error) => format!("Could not save sharing preference: {error}"),
                };
                let _ = updates.send(Update {
                    entries: None,
                    status,
                });
            });
        }
    }
    fn add_tabs(&mut self) {
        for n in 0..3 {
            self.add_button(TAB + n, "", false);
        }
        self.add_button(TAB_PREV, "‹", false);
        self.add_button(TAB_NEXT, "›", false);
    }
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
            self.row_height = ((available - 216) / 10).clamp(30, 56);
            self.height = 186 + self.row_height * 10;
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
                    self.px(124 + n as i32 * self.row_height),
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
            for n in 0..3 {
                MoveWindow(
                    self.buttons[13 + n],
                    self.px(12 + n as i32 * 112),
                    self.px(90),
                    self.px(108),
                    self.px(28),
                    1,
                );
            }
            MoveWindow(
                self.buttons[16],
                self.px(350),
                self.px(90),
                self.px(26),
                self.px(28),
                1,
            );
            MoveWindow(
                self.buttons[17],
                self.px(380),
                self.px(90),
                self.px(26),
                self.px(28),
                1,
            );
            self.refresh();
        }
    }
    fn refresh(&self) {
        unsafe {
            for n in 0..10 {
                if let Some(item) = self.sources.entries().get(self.page * 10 + n) {
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
            for n in 0..3 {
                let index = self.sources.tab_page * 3 + n;
                if index < self.sources.count() {
                    SetWindowTextW(
                        self.buttons[13 + n],
                        wide(&self.sources.tab_name(index)).as_ptr(),
                    );
                    ShowWindow(self.buttons[13 + n], SW_SHOWNA);
                } else {
                    ShowWindow(self.buttons[13 + n], SW_HIDE);
                }
            }
            EnableWindow(self.buttons[16], (self.sources.tab_page > 0) as i32);
            EnableWindow(
                self.buttons[17],
                ((self.sources.tab_page + 1) * 3 < self.sources.count()) as i32,
            );
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
                (
                    SYNC,
                    "Share history with ALL LAN / Tailscale PCs",
                    self.network.as_ref().is_some_and(|n| n.is_enabled()),
                ),
                (CLEAR, "Clear local history…", false),
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
                self.sources.entries().len(),
                if self.paused {
                    "Paused"
                } else {
                    self.sources.name()
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
        let status = format!(
            "Sharing {} · {}",
            if self.network.as_ref().is_some_and(|n| n.is_enabled()) {
                "ON: all network PCs"
            } else {
                "OFF"
            },
            if self.paused {
                "Capture paused"
            } else {
                &self.status
            }
        );
        text(
            dc,
            &status,
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
        if page_range(self.page, self.sources.entries().len()).is_empty() {
            client.top = self.px(168);
            client.bottom = self.px(self.height - 80);
            client.left = self.px(35);
            client.right = self.px(385);
            text(
                dc,
                if self.sources.entries().is_empty() {
                    if self.sources.selected.is_some() {
                        "No saved items from this PC yet.\n\nBoth computers need ClipVault running,\nwith network sharing enabled.\n\nOffline PCs reconnect automatically."
                    } else {
                        "Your clipboard, remembered.\n\nCopy text, an image, or files.\nSelect a PC tab to browse its history.\n\nClick an item, then paste with Ctrl+V."
                    }
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
            if let Some(entry) = self.sources.entries().get(self.page * 10 + id - ROW) {
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
        } else if (TAB..TAB + 3).contains(&id) {
            let index = self.sources.tab_page * 3 + id - TAB;
            if index < self.sources.count() {
                let active = index == self.sources.selected_index();
                if active {
                    fill(dc, &r, 0x00524636);
                }
                text(
                    dc,
                    &self.sources.tab_name(index),
                    RECT {
                        left: r.left + self.px(5),
                        top: r.top + self.px(5),
                        right: r.right - self.px(5),
                        bottom: r.bottom,
                    },
                    self.font,
                    if active { ACCENT } else { MUTED },
                    DT_CENTER | DT_SINGLELINE | DT_END_ELLIPSIS,
                );
            }
        } else {
            text(
                dc,
                match id {
                    PREV => "← Previous",
                    NEXT => "Next →",
                    TAB_PREV => "‹",
                    TAB_NEXT => "›",
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
    if p.width == 0
        || p.height == 0
        || p.width > 80
        || p.height > 80
        || p.rgba.len() != p.width as usize * p.height as usize * 4
    {
        return;
    }
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
    unsafe fn render_preview(app: &App, path: &std::path::Path) {
        unsafe {
            let width = app.px(420);
            let height = app.px(app.height);
            let screen = GetDC(ptr::null_mut());
            let dc = CreateCompatibleDC(screen);
            let mut info: BITMAPINFO = std::mem::zeroed();
            info.bmiHeader.biSize = 40;
            info.bmiHeader.biWidth = width;
            info.bmiHeader.biHeight = -height;
            info.bmiHeader.biPlanes = 1;
            info.bmiHeader.biBitCount = 32;
            let mut bits = ptr::null_mut();
            let bitmap =
                CreateDIBSection(screen, &info, DIB_RGB_COLORS, &mut bits, ptr::null_mut(), 0);
            assert!(!bitmap.is_null());
            let previous = SelectObject(dc, bitmap);
            app.paint(dc);
            for button in &app.buttons {
                if GetWindowLongPtrW(*button, GWL_STYLE) as u32 & WS_VISIBLE == 0 {
                    continue;
                }
                let mut rect: RECT = std::mem::zeroed();
                GetWindowRect(*button, &mut rect);
                MapWindowPoints(
                    ptr::null_mut(),
                    app.hwnd,
                    (&mut rect as *mut RECT).cast(),
                    2,
                );
                SetViewportOrgEx(dc, rect.left, rect.top, ptr::null_mut());
                let mut draw: DRAWITEMSTRUCT = std::mem::zeroed();
                draw.CtlID = GetDlgCtrlID(*button) as u32;
                draw.hDC = dc;
                draw.rcItem = RECT {
                    left: 0,
                    top: 0,
                    right: rect.right - rect.left,
                    bottom: rect.bottom - rect.top,
                };
                if IsWindowEnabled(*button) == 0 {
                    draw.itemState = ODS_DISABLED;
                }
                app.draw_button(&draw);
            }
            GdiFlush();
            let buffer =
                std::slice::from_raw_parts(bits.cast::<u8>(), width as usize * height as usize * 4);
            let mut rgba = Vec::with_capacity(buffer.len());
            for pixel in buffer.chunks_exact(4) {
                rgba.extend_from_slice(&[pixel[2], pixel[1], pixel[0], 255]);
            }
            image::RgbaImage::from_raw(width as u32, height as u32, rgba)
                .unwrap()
                .save(path)
                .unwrap();
            SelectObject(dc, previous);
            DeleteObject(bitmap);
            DeleteDC(dc);
            ReleaseDC(ptr::null_mut(), screen);
        }
    }
    #[test]
    fn pc_tabs_show_separate_histories_and_preserve_local_updates() {
        let local = View {
            id: "local".into(),
            created: 0,
            title: "Local text".into(),
            detail: String::new(),
            kind: Kind::Text,
            preview: None,
        };
        let remote = View {
            id: "remote".into(),
            created: 0,
            title: "Plex text".into(),
            detail: String::new(),
            kind: Kind::Text,
            preview: None,
        };
        let mut sources = Sources {
            local_name: "My PC".into(),
            local: vec![local.clone()],
            peers: vec![PeerHistory {
                id: "plex".into(),
                name: "PlexPC".into(),
                online: true,
                entries: vec![remote],
            }],
            selected: Some("plex".into()),
            tab_page: 0,
        };
        assert_eq!(sources.entries()[0].title, "Plex text");
        sources.local.push(local);
        assert_eq!(
            sources.entries().len(),
            1,
            "new local copies must not replace selected PC history"
        );
        sources.selected = None;
        assert_eq!(sources.entries().len(), 2);
    }
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
                sources: Sources {
                    local_name: "My PC".into(),
                    local: (0..100)
                        .map(|n| View {
                            id: format!("id{n}"),
                            created: 0,
                            title: format!("Test item {n}"),
                            detail: "text".into(),
                            kind: Kind::Text,
                            preview: None,
                        })
                        .collect(),
                    peers: vec![PeerHistory {
                        id: "plex".into(),
                        name: "PlexPC".into(),
                        online: true,
                        entries: vec![View {
                            id: "remote1".into(),
                            created: 0,
                            title: "Plex text".into(),
                            detail: String::new(),
                            kind: Kind::Text,
                            preview: None,
                        }],
                    }],
                    ..Sources::default()
                },
                network: None,
                sync_updates: mpsc::channel().1,
                control_updates: mpsc::channel().0,
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
            app.add_tabs();
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
            SendMessageW(hwnd, WM_COMMAND, TAB + 1, 0);
            APP.with(|c| {
                let slot = c.borrow();
                let app = slot.as_ref().unwrap();
                assert_eq!(app.sources.name(), "PlexPC");
                assert_eq!(app.sources.entries()[0].title, "Plex text");
                assert_eq!(app.page, 0);
                let mut tab: RECT = std::mem::zeroed();
                let mut row: RECT = std::mem::zeroed();
                GetWindowRect(app.buttons[13], &mut tab);
                GetWindowRect(app.buttons[0], &mut row);
                assert!(
                    row.top >= tab.bottom,
                    "PC tabs must sit above the item list"
                );
                if let Some(path) = std::env::var_os("CLIPVAULT_RENDER_PATH") {
                    render_preview(app, &PathBuf::from(path));
                }
            });
            SendMessageW(hwnd, WM_COMMAND, ROW, 0);
            match commands.try_recv().unwrap() {
                Command::RestoreRemote(peer, id) => {
                    assert_eq!(peer, "plex");
                    assert_eq!(id, "remote1");
                }
                _ => panic!("expected remote restore"),
            }
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
