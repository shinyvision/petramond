// A shipped Windows build is a GUI app: without this, double-clicking
// petramond.exe opens a console window behind the game that stays for the
// whole session. Only the optimized profiles take it, so a debug build still
// has somewhere to print — and a windowed build has nowhere at all, since
// logging goes to stderr. The dedicated server is deliberately NOT given this
// attribute: its console IS its interface (stop / save / say / op).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

/// The gen/light/mesh worker pools allocate large short-lived buffers from many
/// threads at once; mimalloc's per-thread heaps keep that churn off the system
/// allocator's shared arena locks (measured as residual frame-time spikes).
#[global_allocator]
static GLOBAL_ALLOC: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn main() {
    petramond_client::native::run();
}
