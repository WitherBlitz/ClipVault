# ClipVault v0.1.1 — automatic background startup

- Installation now enables startup for the current Windows user automatically.
- ClipVault starts in the background immediately after installation, including silent installation, and at Windows sign-in.
- Background launches do not open the history popup, including when an instance is already running. Click the tray icon or press Ctrl+Shift+V to open it.
- The tray menu still lets you disable start-at-sign-in. Uninstall removes the startup registration.

Download **ClipVault-Setup-0.1.1-x64.exe** to install or upgrade. Existing history is retained. The portable EXE does not register startup by itself.

Validation includes a real-process regression test for quiet first/repeat launches, the existing 13 Rust tests, formatting/lint, and an isolated installer smoke test for automatic startup, background process launch and uninstall cleanup. Binaries remain unsigned previews; previous clipboard format and interactive visual QA limits still apply. See README and docs/verification-0.1.1.md.
