//! One queue for every long task: batches, downloads, AI runs, exports.
//!
//! Each job has a lane. Jobs of the `Cpu` lane run one at a time because a
//! batch already uses every core; jobs of the `Gpu` lane run one at a time
//! because models share video memory; `Io` jobs (downloads) run side by side.
//! A job reports progress, can be paused and cancelled, and announces every
//! change through the listener.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use pixelit_core::{CancelToken, Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Finished jobs kept in the list.
const FINISHED_KEPT: usize = 50;

/// Shortest time between two progress announcements of one job.
const ANNOUNCE_EVERY: Duration = Duration::from_millis(100);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Lane {
    Cpu,
    Gpu,
    Io,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum JobState {
    Queued,
    Running,
    Paused,
    Done,
    Failed,
    Cancelled,
}

impl JobState {
    pub fn is_finished(self) -> bool {
        matches!(self, Self::Done | Self::Failed | Self::Cancelled)
    }
}

/// The public picture of a job.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JobInfo {
    pub id: u64,
    /// Stable kind, such as `batch`, `download` or `generate`.
    pub kind: String,
    pub title: String,
    pub lane: Lane,
    pub state: JobState,
    pub done: u64,
    pub total: u64,
    /// What is being worked on right now, such as a file name.
    pub current: String,
    /// Seconds left, once enough work is done to tell.
    pub eta_seconds: Option<u64>,
    pub error: Option<Error>,
    /// What the job produced.
    pub result: Option<Value>,
}

/// Called on every change of a job.
pub type Listener = Arc<dyn Fn(&JobInfo) + Send + Sync>;

#[derive(Default)]
struct PauseState {
    paused: Mutex<bool>,
    changed: Condvar,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

struct Entry {
    info: JobInfo,
    cancel: CancelToken,
    pause: Arc<PauseState>,
    /// Time spent running, without pauses and queueing.
    worked: Duration,
    resumed: Option<Instant>,
    announced: Option<Instant>,
}

#[derive(Default)]
struct LaneState {
    running: Option<u64>,
    waiting: VecDeque<u64>,
}

#[derive(Default)]
struct State {
    jobs: HashMap<u64, Entry>,
    order: Vec<u64>,
    lanes: HashMap<Lane, LaneState>,
}

struct Shared {
    state: Mutex<State>,
    lane_changed: Condvar,
    listener: Mutex<Option<Listener>>,
    next_id: AtomicU64,
}

impl Shared {
    fn announce(&self, info: &JobInfo) {
        let listener = lock(&self.listener).clone();
        if let Some(listener) = listener {
            listener(info);
        }
    }
}

/// The job queue. Cloning gives another handle to the same queue.
#[derive(Clone)]
pub struct Jobs {
    shared: Arc<Shared>,
}

impl Default for Jobs {
    fn default() -> Self {
        Self::new()
    }
}

/// What a running job uses to report progress and to notice pause and cancel.
#[derive(Clone)]
pub struct JobContext {
    id: u64,
    shared: Arc<Shared>,
    pub cancel: CancelToken,
    pause: Arc<PauseState>,
}

impl JobContext {
    /// A context outside any queue, for the command line and tests.
    pub fn detached() -> Self {
        Self {
            id: 0,
            shared: Jobs::new().shared,
            cancel: CancelToken::new(),
            pause: Arc::new(PauseState::default()),
        }
    }

    pub fn id(&self) -> u64 {
        self.id
    }

    /// Blocks while the job is paused. Returns an error once it is cancelled.
    pub fn checkpoint(&self) -> Result<()> {
        let mut paused = lock(&self.pause.paused);
        while *paused && !self.cancel.is_cancelled() {
            paused = self
                .pause
                .changed
                .wait_timeout(paused, Duration::from_millis(200))
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
        drop(paused);
        self.cancel.check()
    }

    /// Reports progress. Announcements are thinned out, except the last one.
    pub fn progress(&self, done: u64, total: u64, current: &str) {
        let info = {
            let mut state = lock(&self.shared.state);
            let Some(entry) = state.jobs.get_mut(&self.id) else {
                return;
            };
            entry.info.done = done;
            entry.info.total = total;
            entry.info.current.clear();
            entry.info.current.push_str(current);
            let worked = entry.worked
                + entry
                    .resumed
                    .map_or(Duration::ZERO, |resumed| resumed.elapsed());
            entry.info.eta_seconds = eta(worked, done, total);
            let now = Instant::now();
            let due = entry
                .announced
                .is_none_or(|last| now.duration_since(last) >= ANNOUNCE_EVERY);
            if !due && done < total {
                return;
            }
            entry.announced = Some(now);
            entry.info.clone()
        };
        self.shared.announce(&info);
    }

    /// Changes the title shown for the job.
    pub fn set_title(&self, title: &str) {
        let info = {
            let mut state = lock(&self.shared.state);
            let Some(entry) = state.jobs.get_mut(&self.id) else {
                return;
            };
            title.clone_into(&mut entry.info.title);
            entry.info.clone()
        };
        self.shared.announce(&info);
    }
}

/// Seconds left, judged by the average speed so far.
fn eta(worked: Duration, done: u64, total: u64) -> Option<u64> {
    if done == 0 || total <= done || worked < Duration::from_millis(500) {
        return None;
    }
    let per_unit = worked.as_secs_f64() / done as f64;
    Some((per_unit * (total - done) as f64).ceil() as u64)
}

impl Jobs {
    pub fn new() -> Self {
        Self {
            shared: Arc::new(Shared {
                state: Mutex::new(State::default()),
                lane_changed: Condvar::new(),
                listener: Mutex::new(None),
                next_id: AtomicU64::new(1),
            }),
        }
    }

    /// Sets the function called on every change of a job.
    pub fn set_listener(&self, listener: Listener) {
        *lock(&self.shared.listener) = Some(listener);
    }

    /// Jobs in the order they were added.
    pub fn list(&self) -> Vec<JobInfo> {
        let state = lock(&self.shared.state);
        state
            .order
            .iter()
            .filter_map(|id| state.jobs.get(id).map(|entry| entry.info.clone()))
            .collect()
    }

    pub fn get(&self, id: u64) -> Option<JobInfo> {
        lock(&self.shared.state)
            .jobs
            .get(&id)
            .map(|entry| entry.info.clone())
    }

    /// Whether any job is queued, running or paused.
    pub fn is_busy(&self) -> bool {
        lock(&self.shared.state)
            .jobs
            .values()
            .any(|entry| !entry.info.state.is_finished())
    }

    /// Adds a job and starts it as soon as its lane is free.
    pub fn spawn<F>(&self, kind: &str, title: &str, lane: Lane, work: F) -> u64
    where
        F: FnOnce(&JobContext) -> Result<Value> + Send + 'static,
    {
        let id = self.shared.next_id.fetch_add(1, Ordering::Relaxed);
        let cancel = CancelToken::new();
        let pause = Arc::new(PauseState::default());
        let info = JobInfo {
            id,
            kind: kind.to_owned(),
            title: title.to_owned(),
            lane,
            state: JobState::Queued,
            done: 0,
            total: 0,
            current: String::new(),
            eta_seconds: None,
            error: None,
            result: None,
        };
        {
            let mut state = lock(&self.shared.state);
            state.jobs.insert(
                id,
                Entry {
                    info: info.clone(),
                    cancel: cancel.clone(),
                    pause: Arc::clone(&pause),
                    worked: Duration::ZERO,
                    resumed: None,
                    announced: None,
                },
            );
            state.order.push(id);
            if lane != Lane::Io {
                state.lanes.entry(lane).or_default().waiting.push_back(id);
            }
            prune(&mut state);
        }
        self.shared.announce(&info);

        let context = JobContext {
            id,
            shared: Arc::clone(&self.shared),
            cancel,
            pause,
        };
        let shared = Arc::clone(&self.shared);
        let spawned = thread::Builder::new()
            .name(format!("job-{kind}-{id}"))
            .spawn(move || run(&shared, &context, lane, work));
        if let Err(error) = spawned {
            finish(&self.shared, id, lane, Err(Error::new("io").detail(error)));
        }
        id
    }

    /// Asks a job to stop. A queued job never starts.
    pub fn cancel(&self, id: u64) {
        let state = lock(&self.shared.state);
        if let Some(entry) = state.jobs.get(&id) {
            entry.cancel.cancel();
            entry.pause.changed.notify_all();
        }
        drop(state);
        self.shared.lane_changed.notify_all();
    }

    /// Asks every unfinished job to stop.
    pub fn cancel_all(&self) {
        let ids: Vec<u64> = lock(&self.shared.state).order.clone();
        for id in ids {
            self.cancel(id);
        }
    }

    /// Pauses or resumes a job at its next checkpoint.
    pub fn set_paused(&self, id: u64, paused: bool) {
        let info = {
            let mut state = lock(&self.shared.state);
            let Some(entry) = state.jobs.get_mut(&id) else {
                return;
            };
            if entry.info.state.is_finished() {
                return;
            }
            *lock(&entry.pause.paused) = paused;
            entry.pause.changed.notify_all();
            match (entry.info.state, paused) {
                (JobState::Running, true) => {
                    if let Some(resumed) = entry.resumed.take() {
                        entry.worked += resumed.elapsed();
                    }
                    entry.info.state = JobState::Paused;
                    entry.info.eta_seconds = None;
                }
                (JobState::Paused, false) => {
                    entry.resumed = Some(Instant::now());
                    entry.info.state = JobState::Running;
                }
                _ => return,
            }
            entry.info.clone()
        };
        self.shared.announce(&info);
    }

    /// Removes finished jobs from the list.
    pub fn clear_finished(&self) {
        let mut state = lock(&self.shared.state);
        let finished: Vec<u64> = state
            .jobs
            .iter()
            .filter(|(_, entry)| entry.info.state.is_finished())
            .map(|(id, _)| *id)
            .collect();
        for id in finished {
            state.jobs.remove(&id);
            state.order.retain(|known| *known != id);
        }
    }

    /// Blocks until the job finishes and returns its final picture.
    pub fn wait(&self, id: u64) -> Option<JobInfo> {
        let mut state = lock(&self.shared.state);
        loop {
            let info = state.jobs.get(&id).map(|entry| entry.info.clone())?;
            if info.state.is_finished() {
                return Some(info);
            }
            state = self
                .shared
                .lane_changed
                .wait_timeout(state, Duration::from_millis(100))
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }
}

/// Drops the oldest finished jobs above the kept number.
fn prune(state: &mut State) {
    let finished: Vec<u64> = state
        .order
        .iter()
        .copied()
        .filter(|id| {
            state
                .jobs
                .get(id)
                .is_some_and(|entry| entry.info.state.is_finished())
        })
        .collect();
    let extra = finished.len().saturating_sub(FINISHED_KEPT);
    for id in finished.into_iter().take(extra) {
        state.jobs.remove(&id);
        state.order.retain(|known| *known != id);
    }
}

/// Waits for the turn of a job in its lane. `false` means the job was
/// cancelled while waiting.
fn wait_turn(shared: &Shared, id: u64, lane: Lane, cancel: &CancelToken) -> bool {
    if lane == Lane::Io {
        return !cancel.is_cancelled();
    }
    let mut state = lock(&shared.state);
    loop {
        if cancel.is_cancelled() {
            if let Some(lane_state) = state.lanes.get_mut(&lane) {
                lane_state.waiting.retain(|known| *known != id);
            }
            return false;
        }
        let lane_state = state.lanes.entry(lane).or_default();
        if lane_state.running.is_none() && lane_state.waiting.front() == Some(&id) {
            lane_state.waiting.pop_front();
            lane_state.running = Some(id);
            return true;
        }
        state = shared
            .lane_changed
            .wait_timeout(state, Duration::from_millis(200))
            .unwrap_or_else(PoisonError::into_inner)
            .0;
    }
}

fn finish(shared: &Shared, id: u64, lane: Lane, result: Result<Value>) {
    let info = {
        let mut state = lock(&shared.state);
        if let Some(lane_state) = state.lanes.get_mut(&lane) {
            if lane_state.running == Some(id) {
                lane_state.running = None;
            }
            lane_state.waiting.retain(|known| *known != id);
        }
        let Some(entry) = state.jobs.get_mut(&id) else {
            shared.lane_changed.notify_all();
            return;
        };
        if let Some(resumed) = entry.resumed.take() {
            entry.worked += resumed.elapsed();
        }
        entry.info.eta_seconds = None;
        match result {
            Ok(value) => {
                entry.info.state = JobState::Done;
                entry.info.result = Some(value);
            }
            Err(error) if error.is_cancelled() => entry.info.state = JobState::Cancelled,
            Err(error) => {
                entry.info.state = JobState::Failed;
                entry.info.error = Some(error);
            }
        }
        entry.info.clone()
    };
    shared.lane_changed.notify_all();
    shared.announce(&info);
}

fn run<F>(shared: &Arc<Shared>, context: &JobContext, lane: Lane, work: F)
where
    F: FnOnce(&JobContext) -> Result<Value>,
{
    let id = context.id;
    if !wait_turn(shared, id, lane, &context.cancel) {
        finish(shared, id, lane, Err(Error::cancelled()));
        return;
    }
    let started = {
        let mut state = lock(&shared.state);
        state.jobs.get_mut(&id).map(|entry| {
            let paused = *lock(&entry.pause.paused);
            entry.info.state = if paused {
                JobState::Paused
            } else {
                entry.resumed = Some(Instant::now());
                JobState::Running
            };
            entry.info.clone()
        })
    };
    if let Some(info) = started {
        shared.announce(&info);
    }
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| work(context)))
        .unwrap_or_else(|_| Err(Error::new("internal").detail("job panicked")));
    let result = match result {
        Err(_) if context.cancel.is_cancelled() => Err(Error::cancelled()),
        other => other,
    };
    finish(shared, id, lane, result);
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicBool;
    use std::sync::mpsc;

    use serde_json::json;

    use super::*;

    #[test]
    fn job_runs_and_reports() {
        let jobs = Jobs::new();
        let (sender, receiver) = mpsc::channel();
        jobs.set_listener(Arc::new(move |info: &JobInfo| {
            let _ = sender.send(info.state);
        }));
        let id = jobs.spawn("test", "Test", Lane::Cpu, |context| {
            context.progress(1, 2, "a");
            context.checkpoint()?;
            context.progress(2, 2, "b");
            Ok(json!({ "files": 2 }))
        });
        let info = jobs.wait(id).unwrap();
        assert_eq!(info.state, JobState::Done);
        assert_eq!(info.done, 2);
        assert_eq!(info.result, Some(json!({ "files": 2 })));
        assert!(!jobs.is_busy());
        let states: Vec<JobState> = receiver.try_iter().collect();
        assert_eq!(states.first(), Some(&JobState::Queued));
        assert_eq!(states.last(), Some(&JobState::Done));
    }

    #[test]
    fn failure_and_cancel_are_told_apart() {
        let jobs = Jobs::new();
        let failed = jobs.spawn("test", "Fails", Lane::Io, |_| Err(Error::new("io")));
        assert_eq!(jobs.wait(failed).unwrap().state, JobState::Failed);

        let (started, wait_started) = mpsc::channel();
        let cancelled = jobs.spawn("test", "Stops", Lane::Io, move |context| {
            let _ = started.send(());
            loop {
                context.checkpoint()?;
                thread::sleep(Duration::from_millis(5));
            }
        });
        wait_started.recv().unwrap();
        jobs.cancel(cancelled);
        assert_eq!(jobs.wait(cancelled).unwrap().state, JobState::Cancelled);

        let panicked = jobs.spawn("test", "Panics", Lane::Io, |_| panic!("boom"));
        let info = jobs.wait(panicked).unwrap();
        assert_eq!(info.state, JobState::Failed);
        assert_eq!(info.error.unwrap().code, "internal");
    }

    #[test]
    fn cpu_lane_runs_one_job_at_a_time() {
        let jobs = Jobs::new();
        let active = Arc::new(AtomicU64::new(0));
        let overlap = Arc::new(AtomicBool::new(false));
        let ids: Vec<u64> = (0..4)
            .map(|_| {
                let active = Arc::clone(&active);
                let overlap = Arc::clone(&overlap);
                jobs.spawn("test", "Lane", Lane::Cpu, move |_| {
                    if active.fetch_add(1, Ordering::SeqCst) > 0 {
                        overlap.store(true, Ordering::SeqCst);
                    }
                    thread::sleep(Duration::from_millis(20));
                    active.fetch_sub(1, Ordering::SeqCst);
                    Ok(Value::Null)
                })
            })
            .collect();
        for id in ids {
            assert_eq!(jobs.wait(id).unwrap().state, JobState::Done);
        }
        assert!(!overlap.load(Ordering::SeqCst));
    }

    #[test]
    fn queued_job_can_be_cancelled() {
        let jobs = Jobs::new();
        let (release, wait_release) = mpsc::channel::<()>();
        let first = jobs.spawn("test", "Holds", Lane::Gpu, move |_| {
            let _ = wait_release.recv();
            Ok(Value::Null)
        });
        let second = jobs.spawn("test", "Waits", Lane::Gpu, |_| Ok(Value::Null));
        jobs.cancel(second);
        assert_eq!(jobs.wait(second).unwrap().state, JobState::Cancelled);
        release.send(()).unwrap();
        assert_eq!(jobs.wait(first).unwrap().state, JobState::Done);
    }

    #[test]
    fn pause_blocks_checkpoints() {
        let jobs = Jobs::new();
        let (started, wait_started) = mpsc::channel();
        let steps = Arc::new(AtomicU64::new(0));
        let counter = Arc::clone(&steps);
        let id = jobs.spawn("test", "Pauses", Lane::Io, move |context| {
            let _ = started.send(());
            for _ in 0..1000 {
                context.checkpoint()?;
                counter.fetch_add(1, Ordering::SeqCst);
                thread::sleep(Duration::from_millis(2));
            }
            Ok(Value::Null)
        });
        wait_started.recv().unwrap();
        jobs.set_paused(id, true);
        thread::sleep(Duration::from_millis(50));
        let before = steps.load(Ordering::SeqCst);
        thread::sleep(Duration::from_millis(80));
        let after = steps.load(Ordering::SeqCst);
        assert!(after <= before + 1, "{before} {after}");
        assert_eq!(jobs.get(id).unwrap().state, JobState::Paused);
        jobs.set_paused(id, false);
        jobs.cancel(id);
        assert_eq!(jobs.wait(id).unwrap().state, JobState::Cancelled);
    }

    #[test]
    fn eta_needs_some_progress() {
        assert_eq!(eta(Duration::from_secs(10), 0, 10), None);
        assert_eq!(eta(Duration::from_millis(100), 1, 10), None);
        assert_eq!(eta(Duration::from_secs(10), 5, 10), Some(10));
        assert_eq!(eta(Duration::from_secs(10), 10, 10), None);
    }
}
