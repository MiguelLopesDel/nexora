//! Nexora as a library: the binary in `main.rs` and developer tools under
//! `src/bin/` share the exact same modules, so benchmarks and integration
//! tests exercise the code paths the app ships with.

pub mod app;
pub mod config;
pub mod conversation;
pub mod hidden;
pub mod meeting;
pub mod providers;
pub mod screenshot;
pub mod ui;
pub mod vision;
pub mod whisper;

use std::sync::OnceLock;

/// Tokio runtime for network and portal I/O (GTK owns the main thread).
pub fn runtime() -> &'static tokio::runtime::Runtime {
    static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("failed to start tokio runtime")
    })
}
