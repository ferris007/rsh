//! One job.

use std::fmt;

use nix::unistd::Pid;
use whelk_process::ExitStatus;

/// The number a user sees in `[1]`.
///
/// Small and reused: when job 1 finishes, the next job started may be 1 again.
/// That is deliberate — job numbers are for typing, not for identity, and a
/// user should not have to type `%47` on a shell that has been open all day.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct JobId(pub usize);

impl fmt::Display for JobId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// What the shell last knew about a job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobState {
    /// Every process is running.
    Running,
    /// At least one process is stopped, and none have been resumed since.
    Stopped,
    /// Every process has ended.
    Done(ExitStatus),
}

impl JobState {
    /// Whether the job still exists as processes.
    pub fn is_alive(self) -> bool {
        !matches!(self, Self::Done(_))
    }

    /// The word `jobs` prints in the second column.
    pub fn describe(self) -> String {
        match self {
            Self::Running => "Running".to_owned(),
            Self::Stopped => "Stopped".to_owned(),
            Self::Done(ExitStatus::Exited(0)) => "Done".to_owned(),
            Self::Done(ExitStatus::Exited(code)) => format!("Exit {code}"),
            Self::Done(ExitStatus::Signaled(signal)) => format!("Killed ({signal})"),
        }
    }
}

/// A pipeline the shell is keeping track of.
#[derive(Debug, Clone)]
pub struct Job {
    id: JobId,
    pgid: Pid,
    /// Every process in the pipeline, and whether each has ended.
    ///
    /// Tracked per process rather than per job because the shell learns about
    /// them one at a time: `waitpid` reports a pid, not a pipeline, and a job
    /// is only finished when its last stage is.
    processes: Vec<Process>,
    command: String,
    state: JobState,
    /// Whether the user has been told this job finished.
    reported: bool,
    /// The terminal modes the job was using when it was suspended.
    ///
    /// A job stopped in the middle of `vim` left the terminal in raw mode, and
    /// the shell put its own modes back so it could print a prompt. Resuming it
    /// has to undo that undoing, or the editor comes back to a terminal that
    /// echoes and buffers lines — visibly broken, and not the job's fault.
    modes: Option<whelk_terminal::Modes>,
}

/// One process within a job.
#[derive(Debug, Clone)]
struct Process {
    pid: Pid,
    status: Option<ExitStatus>,
}

impl Job {
    /// Record a newly started pipeline.
    pub fn new(id: JobId, pgid: Pid, pids: Vec<Pid>, command: String) -> Self {
        Self {
            id,
            pgid,
            processes: pids
                .into_iter()
                .map(|pid| Process { pid, status: None })
                .collect(),
            command,
            state: JobState::Running,
            reported: false,
            modes: None,
        }
    }

    /// The number a user types after `%`.
    pub fn id(&self) -> JobId {
        self.id
    }

    /// The process group every stage shares.
    pub fn pgid(&self) -> Pid {
        self.pgid
    }

    /// The command line as the user wrote it.
    pub fn command(&self) -> &str {
        &self.command
    }

    /// What the shell last knew.
    pub fn state(&self) -> JobState {
        self.state
    }

    /// Whether the job contains this process.
    pub fn contains(&self, pid: Pid) -> bool {
        self.processes.iter().any(|process| process.pid == pid)
    }

    /// Record that one of the job's processes ended.
    ///
    /// The job's own status is the *last* stage's, matching how a foreground
    /// pipeline reports — so `false | true` is a job that succeeded.
    pub fn finished(&mut self, pid: Pid, status: ExitStatus) {
        if let Some(process) = self.processes.iter_mut().find(|process| process.pid == pid) {
            process.status = Some(status);
        }

        if self
            .processes
            .iter()
            .all(|process| process.status.is_some())
        {
            let last = self.processes.last().and_then(|process| process.status);
            self.state = JobState::Done(last.unwrap_or(ExitStatus::Exited(0)));
        }
    }

    /// Remember the terminal modes this job was using.
    pub fn remember_modes(&mut self, modes: Option<whelk_terminal::Modes>) {
        self.modes = modes;
    }

    /// The terminal modes to put back before resuming this job.
    pub fn modes(&self) -> Option<&whelk_terminal::Modes> {
        self.modes.as_ref()
    }

    /// Record that the job was suspended.
    pub fn stopped(&mut self) {
        if self.state.is_alive() {
            self.state = JobState::Stopped;
        }
    }

    /// Record that the job was resumed.
    pub fn resumed(&mut self) {
        if self.state.is_alive() {
            self.state = JobState::Running;
        }
    }

    /// The status a finished job reports, if it has one.
    pub fn exit_status(&self) -> Option<ExitStatus> {
        match self.state {
            JobState::Done(status) => Some(status),
            _ => None,
        }
    }

    /// Whether the user still needs to be told about this job.
    pub fn needs_report(&self) -> bool {
        !self.reported
    }

    /// Mark the job as reported.
    pub fn mark_reported(&mut self) {
        self.reported = true;
    }

    /// Allow a job to be announced again, after it changes state.
    pub fn mark_unreported(&mut self) {
        self.reported = false;
    }

    /// The processes still running, for signalling.
    pub fn pids(&self) -> impl Iterator<Item = Pid> + '_ {
        self.processes
            .iter()
            .filter(|p| p.status.is_none())
            .map(|p| p.pid)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job() -> Job {
        Job::new(
            JobId(1),
            Pid::from_raw(100),
            vec![Pid::from_raw(100), Pid::from_raw(101)],
            "cat | grep x".to_owned(),
        )
    }

    #[test]
    fn a_job_is_not_done_until_every_stage_is() {
        let mut job = job();
        assert_eq!(job.state(), JobState::Running);

        job.finished(Pid::from_raw(100), ExitStatus::Exited(0));
        assert_eq!(job.state(), JobState::Running, "one stage left");

        job.finished(Pid::from_raw(101), ExitStatus::Exited(3));
        assert_eq!(job.state(), JobState::Done(ExitStatus::Exited(3)));
    }

    #[test]
    fn the_jobs_status_is_the_last_stages() {
        // Matching a foreground pipeline, where `false | true` succeeds.
        let mut job = job();
        job.finished(Pid::from_raw(100), ExitStatus::Exited(1));
        job.finished(Pid::from_raw(101), ExitStatus::Exited(0));
        assert_eq!(job.exit_status(), Some(ExitStatus::Exited(0)));
    }

    #[test]
    fn stopping_and_resuming_move_between_states() {
        let mut job = job();
        job.stopped();
        assert_eq!(job.state(), JobState::Stopped);
        job.resumed();
        assert_eq!(job.state(), JobState::Running);
    }

    #[test]
    fn a_finished_job_cannot_be_stopped_or_resumed() {
        // The events can genuinely arrive in this order: a stop notification
        // queued behind the exit that made it irrelevant.
        let mut job = job();
        job.finished(Pid::from_raw(100), ExitStatus::Exited(0));
        job.finished(Pid::from_raw(101), ExitStatus::Exited(0));

        job.stopped();
        assert_eq!(job.state(), JobState::Done(ExitStatus::Exited(0)));
        job.resumed();
        assert_eq!(job.state(), JobState::Done(ExitStatus::Exited(0)));
    }

    #[test]
    fn only_live_processes_are_signalled() {
        let mut job = job();
        job.finished(Pid::from_raw(100), ExitStatus::Exited(0));
        assert_eq!(job.pids().collect::<Vec<_>>(), [Pid::from_raw(101)]);
    }

    #[test]
    fn states_describe_themselves_the_way_jobs_prints_them() {
        assert_eq!(JobState::Running.describe(), "Running");
        assert_eq!(JobState::Stopped.describe(), "Stopped");
        assert_eq!(JobState::Done(ExitStatus::Exited(0)).describe(), "Done");
        assert_eq!(JobState::Done(ExitStatus::Exited(2)).describe(), "Exit 2");
    }
}
