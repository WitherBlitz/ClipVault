# ClipVault contributor checks

Keep changes scoped, read code before editing, reproduce failures, and verify claims with command output. Windows 10/11 x64 is the supported target. Real clipboard tests must run sequentially and restore prior supported clipboard content. Never include local history, screenshots of private clipboard content, or credentials in the repository.

- Install: `cargo fetch --locked`
- Narrow tests: `cargo test --lib model`
- Full tests: `cargo test --locked -- --test-threads=1`
- Format: `cargo fmt --check`
- Lint: `cargo clippy --locked --all-targets -- -D warnings`
- Build: `cargo build --locked --release`
- Installer: `powershell -NoProfile -File scripts/package.ps1`

Do not commit, push, or publish unless the user requested it. Do not change Windows privacy/security settings. Use an isolated `--data-dir` for desktop tests.
