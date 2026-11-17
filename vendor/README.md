# Vendored crates

Unmodified copies of upstream crates except for the small patches listed below
(each marked with a `rust-browser patch` comment). Wired up via `[patch.crates-io]` in `Cargo.toml`.

| Crate | Version | License | Patch |
| --- | --- | --- | --- |
| [wry](https://github.com/tauri-apps/wry) | 0.57.2 | Apache-2.0 OR MIT | WebView2: `AllowSingleSignOnUsingOSPrimaryAccount` (Entra ID SSO / Conditional Access) and password autosave enabled |
| [blitz-net](https://github.com/DioxusLabs/blitz) | 0.3.0-beta.4 | MIT OR Apache-2.0 | `Provider::with_cookie_provider` so login cookies can be persisted to disk |
