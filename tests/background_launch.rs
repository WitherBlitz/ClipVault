use clipvault::clipboard::wide;
use std::{
    hash::{Hash, Hasher},
    process::{Child, Command},
    ptr, thread,
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::HWND,
    UI::WindowsAndMessaging::{FindWindowW, IsWindowVisible},
};

struct TestProcess(Child);
impl Drop for TestProcess {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn startup_and_repeat_background_launches_keep_the_popup_hidden() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("history");
    let exe = env!("CARGO_BIN_EXE_clipvault");
    let mut running = TestProcess(
        Command::new(exe)
            .arg("--data-dir")
            .arg(&data)
            .spawn()
            .unwrap(),
    );
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    data.hash(&mut hash);
    let class = wide(&format!("ClipVault-{:x}", hash.finish()));
    let deadline = Instant::now() + Duration::from_secs(10);
    let window: HWND = loop {
        assert!(
            running.0.try_wait().unwrap().is_none(),
            "background process exited early"
        );
        let hwnd = unsafe { FindWindowW(class.as_ptr(), ptr::null()) };
        if !hwnd.is_null() && data.join("items").is_dir() {
            break hwnd;
        }
        assert!(
            Instant::now() < deadline,
            "background process did not initialize"
        );
        thread::sleep(Duration::from_millis(25));
    };
    assert_eq!(
        unsafe { IsWindowVisible(window) },
        0,
        "startup must not open history"
    );
    let status = Command::new(exe)
        .arg("--data-dir")
        .arg(&data)
        .status()
        .unwrap();
    assert!(status.success());
    thread::sleep(Duration::from_millis(200));
    assert!(running.0.try_wait().unwrap().is_none());
    assert_eq!(
        unsafe { IsWindowVisible(window) },
        0,
        "a second background launch must not open history"
    );
}
