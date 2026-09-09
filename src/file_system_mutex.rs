use anyhow::Result;
use std::{
    ops::DerefMut,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Mutex,
    },
};

use crate::FileSystemAPI;

#[cfg(not(debug_assertions))]
#[derive(thiserror::Error, Debug)]
pub enum FileSystemMutexError {
    #[cfg(not(debug_assertions))]
    #[error("File System mutex lock failed.")]
    LockFailed,
}

const UNOWNED: usize = 0;
/// `ThreadId` has no stable numeric form, so threads take a number from here the first time
/// they reach `with_fs`.
static NEXT_THREAD_KEY: AtomicUsize = AtomicUsize::new(1);

thread_local! {
    static THREAD_KEY: usize = NEXT_THREAD_KEY.fetch_add(1, Ordering::Relaxed);
}

#[derive(Debug)]
pub struct FileSystemMutex<F: FileSystemAPI> {
    fs: Mutex<F>,
    /// The thread inside `with_fs`, or `UNOWNED`. Only ever compared against the reading
    /// thread's own key, and a thread always observes its own writes, so this needs no
    /// ordering stronger than `Relaxed`.
    owner: AtomicUsize,
}

/// Clears the owner however `with_fs` leaves, unwinding included: a stale owner would make
/// that thread's next call look re-entrant.
struct OwnerGuard<'a>(&'a AtomicUsize);

impl Drop for OwnerGuard<'_> {
    fn drop(&mut self) {
        self.0.store(UNOWNED, Ordering::Relaxed);
    }
}

fn lock_failed<R>() -> Result<R> {
    #[cfg(debug_assertions)]
    panic!("File System Lock Failed");
    #[cfg(not(debug_assertions))]
    Err(FileSystemMutexError::LockFailed.into())
}

impl<F> FileSystemMutex<F>
where
    F: FileSystemAPI,
{
    pub fn init(fs: F) -> Self {
        Self {
            fs: Mutex::new(fs),
            owner: AtomicUsize::new(UNOWNED),
        }
    }

    /// Runs `f` with exclusive access to the file system, waiting for whichever thread holds
    /// it to finish.
    ///
    /// Re-entering from the thread already inside it is a caller bug rather than contention:
    /// the lock is not reentrant, so waiting would deadlock, and on wasm it traps. Nesting is
    /// refused instead, which is why every method needing file system work from inside a
    /// closure has an `fs`-taking twin - `object_path_impl`, `list_build_files_for_fs`,
    /// `fs_id_for_fs`.
    pub fn with_fs<R>(&self, f: impl FnOnce(&mut F) -> Result<R>) -> Result<R> {
        let me = THREAD_KEY.with(|key| *key);
        if self.owner.load(Ordering::Relaxed) == me {
            return lock_failed();
        }
        // Poisoning stays fail-stop: a panic inside `f` leaves the file system part-way
        // through a mutation, and handing that back out is worse than refusing it.
        let Ok(mut fs) = self.fs.lock() else {
            return lock_failed();
        };
        self.owner.store(me, Ordering::Relaxed);
        let _owner = OwnerGuard(&self.owner);
        f(fs.deref_mut())
    }

    pub fn take_fs(self) -> F {
        self.fs
            .into_inner()
            .expect("attempt to take a poisoned fs.")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MemoryFileSystem;
    use std::{
        path::Path,
        sync::{Arc, Barrier},
        thread,
        time::Duration,
    };

    /// Contention from another thread is legitimate and waits. The second caller must see
    /// the first one's write, which is what proves it waited rather than running alongside.
    #[test]
    fn a_second_thread_waits_rather_than_failing() {
        let mutex = Arc::new(FileSystemMutex::init(MemoryFileSystem::default()));
        let inside = Arc::new(Barrier::new(2));

        let holder = {
            let mutex = Arc::clone(&mutex);
            let inside = Arc::clone(&inside);
            thread::spawn(move || {
                mutex
                    .with_fs(|fs| {
                        inside.wait();
                        thread::sleep(Duration::from_millis(50));
                        fs.write_str("held.txt", "written".to_string())
                    })
                    .unwrap();
            })
        };

        inside.wait();
        let seen = mutex
            .with_fs(|fs| fs.read_to_string(Path::new("held.txt")))
            .expect("contention should wait, not fail");
        assert_eq!(seen.as_deref(), Some("written"));
        holder.join().unwrap();
    }

    /// Re-entering on the same thread is a caller bug, not contention: the lock is not
    /// reentrant, so it is refused rather than waited on.
    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "File System Lock Failed")]
    fn same_thread_reentry_is_refused() {
        let mutex = FileSystemMutex::init(MemoryFileSystem::default());
        let _ = mutex.with_fs(|_| mutex.with_fs(|_| Ok(())));
    }

    #[cfg(not(debug_assertions))]
    #[test]
    fn same_thread_reentry_is_refused() {
        let mutex = FileSystemMutex::init(MemoryFileSystem::default());
        assert!(mutex.with_fs(|_| mutex.with_fs(|_| Ok(()))).is_err());
    }

    /// The closure returning `Err` is an ordinary outcome, so the lock has to come back.
    #[test]
    fn an_error_from_the_closure_releases_the_lock() {
        let mutex = FileSystemMutex::init(MemoryFileSystem::default());
        assert!(mutex
            .with_fs(|_| -> Result<()> { Err(anyhow::anyhow!("nope")) })
            .is_err());
        assert!(mutex.with_fs(|_| Ok(())).is_ok());
    }
}
