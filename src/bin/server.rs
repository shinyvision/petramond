/// The gen/light/mesh worker pools allocate large short-lived buffers from many
/// threads at once; mimalloc's per-thread heaps keep that churn off the system
/// allocator's shared arena locks (measured as residual frame-time spikes).
#[global_allocator]
static GLOBAL_ALLOC: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn main() {
    petramond::platform::server::run();
}
