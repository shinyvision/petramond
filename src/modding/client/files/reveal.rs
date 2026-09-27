use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Condvar, LazyLock, Mutex, MutexGuard, PoisonError};

pub fn os_helper_turn() -> MutexGuard<'static, ()> {
    static TURN: Mutex<()> = Mutex::new(());
    TURN.lock().unwrap_or_else(PoisonError::into_inner)
}

static NO_FILE_MANAGER: AtomicBool = AtomicBool::new(false);

struct Waiting {
    path: Mutex<Option<PathBuf>>,
    wake: Condvar,
}

fn waiting() -> &'static Waiting {
    static WAITING: LazyLock<Waiting> = LazyLock::new(|| {
        std::thread::Builder::new()
            .name("client-mod-reveal".into())
            .spawn(helper)
            .expect("spawn the reveal helper");
        Waiting {
            path: Mutex::new(None),
            wake: Condvar::new(),
        }
    });
    &WAITING
}

fn helper() {
    let waiting = waiting();
    loop {
        let path = {
            let mut slot = waiting.path.lock().unwrap_or_else(PoisonError::into_inner);
            loop {
                if let Some(path) = slot.take() {
                    break path;
                }
                slot = waiting
                    .wake
                    .wait(slot)
                    .unwrap_or_else(PoisonError::into_inner);
            }
        };
        let _turn = os_helper_turn();
        if !petramond_util::process::reveal(&path) {
            NO_FILE_MANAGER.store(true, Ordering::Relaxed);
        }
    }
}

pub(super) fn reveal(path: PathBuf) -> bool {
    if !path.exists() {
        return false;
    }
    if cfg!(test) {
        return true;
    }
    if NO_FILE_MANAGER.load(Ordering::Relaxed) {
        return false;
    }
    let waiting = waiting();
    *waiting.path.lock().unwrap_or_else(PoisonError::into_inner) = Some(path);
    waiting.wake.notify_one();
    true
}
