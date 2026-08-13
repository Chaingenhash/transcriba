//! Tracks the cancel flag for each running job so a `cancel_job` command can
//! reach a transcription already in flight.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Default)]
pub struct Jobs {
    inner: Mutex<HashMap<String, Arc<AtomicBool>>>,
}

impl Jobs {
    /// Registers `id` and returns its cancel flag, replacing any prior entry.
    pub fn flag(&self, id: &str) -> Arc<AtomicBool> {
        let flag = Arc::new(AtomicBool::new(false));
        self.inner
            .lock()
            .expect("jobs mutex poisoned")
            .insert(id.to_string(), Arc::clone(&flag));
        flag
    }

    /// Requests cancellation. Unknown ids are ignored — the job may have just finished.
    pub fn cancel(&self, id: &str) {
        if let Some(flag) = self.inner.lock().expect("jobs mutex poisoned").get(id) {
            flag.store(true, Ordering::Relaxed);
        }
    }

    pub fn finish(&self, id: &str) {
        self.inner.lock().expect("jobs mutex poisoned").remove(id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancelling_a_registered_job_sets_its_flag() {
        let jobs = Jobs::default();
        let flag = jobs.flag("a");
        assert!(!flag.load(Ordering::Relaxed));
        jobs.cancel("a");
        assert!(flag.load(Ordering::Relaxed));
    }

    #[test]
    fn cancelling_an_unknown_job_is_a_no_op() {
        let jobs = Jobs::default();
        jobs.cancel("nope");
    }

    #[test]
    fn finished_jobs_are_forgotten_and_no_longer_cancellable() {
        let jobs = Jobs::default();
        let flag = jobs.flag("a");
        jobs.finish("a");
        jobs.cancel("a");
        assert!(!flag.load(Ordering::Relaxed));
    }

    #[test]
    fn two_jobs_have_independent_flags() {
        let jobs = Jobs::default();
        let a = jobs.flag("a");
        let b = jobs.flag("b");
        jobs.cancel("a");
        assert!(a.load(Ordering::Relaxed));
        assert!(!b.load(Ordering::Relaxed));
    }
}
