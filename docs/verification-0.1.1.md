# v0.1.1 verification — automatic background startup

The user requested that installation enable Windows startup and that the app run in the background. Installation now registers the current user's Run entry without requiring an installer task, and launches the application without `--show`. Repeating a background launch leaves the existing popup hidden. The tray menu can still disable startup. Reinstalling or upgrading re-enables it.

## Reproduced before the fix

- Real-process regression: a first launch stayed hidden, but a second launch opened history (`IsWindowVisible = 1`, expected 0).
- Updated installer smoke: v0.1.0 did not register startup automatically.

## Passing verification

- `cargo test --test background_launch -- --test-threads=1`: passes; a real process initializes its isolated history, remains running with no visible popup, and stays hidden after a second invocation.
- `powershell -NoProfile -File scripts/package.ps1`: format check, clippy with warnings denied, full suite (14 tests), optimized build, and EXE installer compile all pass.
- `powershell -NoProfile -File scripts/smoke-installer.ps1`: passes; empty installer task selection still enables startup, the quoted Run command has no popup argument, installation starts exactly one background process, and a sign-in-equivalent launch stays running without a window. Uninstall removes files, registration and startup entry.
- Fresh read-only review found no material issues after bounded process-readiness and installer-only waiting were added to the smoke script.

Smoke tests refuse to replace an existing installation/startup value, isolate LOCALAPPDATA, stop only the test executable, uninstall their temporary installation and remove only their checked workspace directory. They do not reboot the machine; sign-in is verified through its registered command and an equivalent process launch.

The v0.1.0 interactive visual QA and clipboard-format limits remain unchanged. No further computer-use automation was performed for this change.
