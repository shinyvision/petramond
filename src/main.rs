#[cfg(not(target_arch = "wasm32"))]
fn main() {
    llamacraft::platform::native::run();
}

#[cfg(target_arch = "wasm32")]
fn main() {}
