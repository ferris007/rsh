//! The table of live jobs.

use nix::sys::signal::{killpg, Signal};
use nix::unistd::Pid;
use whelk_process::ChildEvent;

use crate::job::{Job, JobId, JobState};
use crate::spec::JobSpec;

/// Every job the shell is keeping track of.
#[derive(Debug, Default)]
pub struct JobTable {
    jobs: Vec<Job>,
    /// The job `%%` refers to, and the one `fg` picks with no argument.
    current: Option<JobId>,
    /// The job `%-` refers to.
    previous: Option<JobId>,
}

impl JobTable {
    /// An empty table.
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether anything is being tracked.
    pub fn is_empty(&self) -> bool {
        self.jobs.is_empty()
    }

    /// Record a newly started pipeline, returning its job number.
    pub fn add(&mut self, pgid: Pid, pids: Vec<Pid>, command: String) -> JobId {
        let id = self.next_id();
        self.jobs.push(Job::new(id, pgid, pids, command));
        self.promote(id);
        id
    }

    /// The lowest number not currently in use.
    ///
    /// Reusing numbers is what keeps `%1` and `%2` meaningful in a shell that
    /// has been open for hours. The alternative — counting up forever — would
    /// have a user typing `%143` to resume the only job they have.
    fn next_id(&self) -> JobId {
        (1..)
            .map(JobId)
            .find(|id| !self.jobs.iter().any(|job| job.id() == *id))
            .expect("unbounded")
    }

    /// Make a job the current one, pushing the old current to previous.
    ///
    /// This ordering is what makes `fg` with no argument do the obvious thing:
    /// the job you last touched is the one you get back.
    fn promote(&mut self, id: JobId) {
        if self.current == Some(id) {
            return;
        }
        self.previous = self.current;
        self.current = Some(id);
    }

    /// All jobs, in the order they were started.
    pub fn iter(&self) -> impl Iterator<Item = &Job> {
        self.jobs.iter()
    }

    /// Look up a job by specifier.
    pub fn find(&self, spec: JobSpec) -> Option<&Job> {
        let id = self.resolve(spec)?;
        self.jobs.iter().find(|job| job.id() == id)
    }

    /// Look up a job by specifier, mutably.
    pub fn find_mut(&mut self, spec: JobSpec) -> Option<&mut Job> {
        let id = self.resolve(spec)?;
        self.jobs.iter_mut().find(|job| job.id() == id)
    }

    fn resolve(&self, spec: JobSpec) -> Option<JobId> {
        match spec {
            JobSpec::Id(id) => Some(id),
            JobSpec::Current => self.current,
            JobSpec::Previous => self.previous,
        }
    }

    /// Apply a batch of child state changes.
    ///
    /// Events arrive per process, so a pipeline's job is updated several times
    /// and only becomes `Done` when its last stage reports.
    pub fn apply(&mut self, events: &[ChildEvent]) {
        for event in events {
            let pid = event.pid();
            let Some(job) = self.jobs.iter_mut().find(|job| job.contains(pid)) else {
                // A child nobody is tracking. Ordinary: a foreground command is
                // waited for directly and never enters the table.
                continue;
            };

            match *event {
                ChildEvent::Finished(_, status) => job.finished(pid, status),
                ChildEvent::Stopped(_, _) => {
                    job.stopped();
                    job.mark_unreported();
                }
                ChildEvent::Continued(_) => {
                    job.resumed();
                    job.mark_unreported();
                }
            }
        }
    }

    /// Jobs whose state the user has not been told about yet.
    ///
    /// Marks them reported, so each change is announced once.
    pub fn take_reportable(&mut self) -> Vec<Job> {
        let mut reportable = Vec::new();

        for job in &mut self.jobs {
            let interesting = matches!(job.state(), JobState::Done(_) | JobState::Stopped);
            if interesting && job.needs_report() {
                job.mark_reported();
                reportable.push(job.clone());
            }
        }

        reportable
    }

    /// Forget every job that has finished.
    ///
    /// Called after reporting, so a finished job is announced once and then
    /// stops occupying a number.
    pub fn forget_finished(&mut self) {
        self.jobs.retain(|job| job.state().is_alive());

        // A number that no longer exists must not stay as `%%`, or `fg` would
        // report "no such job" for a shell that plainly has one.
        let live = |id: &Option<JobId>| id.filter(|id| self.jobs.iter().any(|j| j.id() == *id));
        self.current = live(&self.current);
        self.previous = live(&self.previous);
    }

    /// Send a signal to every process in a job.
    ///
    /// To the *group*, not to the processes one at a time: that is the whole
    /// reason jobs have process groups. One call reaches every stage of a
    /// pipeline, including any children they started themselves.
    pub fn signal(&self, job: &Job, signal: Signal) -> Result<(), nix::errno::Errno> {
        killpg(job.pgid(), signal)
    }

    /// Mark a job as the one `%%` refers to.
    pub fn make_current(&mut self, id: JobId) {
        self.promote(id);
    }

    /// The current job's number, if there is one.
    pub fn current(&self) -> Option<JobId> {
        self.current
    }

    /// The marker `jobs` prints after the number: `+` for current, `-` for
    /// previous.
    pub fn marker(&self, id: JobId) -> char {
        if self.current == Some(id) {
            '+'
        } else if self.previous == Some(id) {
            '-'
        } else {
            ' '
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use whelk_process::ExitStatus;

    fn pid(n: i32) -> Pid {
        Pid::from_raw(n)
    }

    fn table_with_two() -> JobTable {
        let mut table = JobTable::new();
        table.add(pid(100), vec![pid(100)], "sleep 30".to_owned());
        table.add(pid(200), vec![pid(200)], "vim notes".to_owned());
        table
    }

    #[test]
    fn jobs_are_numbered_from_one() {
        let table = table_with_two();
        let ids: Vec<_> = table.iter().map(|job| job.id()).collect();
        assert_eq!(ids, [JobId(1), JobId(2)]);
    }

    #[test]
    fn the_newest_job_becomes_current_and_the_old_one_previous() {
        let table = table_with_two();
        assert_eq!(table.current(), Some(JobId(2)));
        assert_eq!(table.marker(JobId(2)), '+');
        assert_eq!(table.marker(JobId(1)), '-');
    }

    #[test]
    fn specifiers_resolve_to_the_right_job() {
        let table = table_with_two();
        assert_eq!(table.find(JobSpec::Current).unwrap().id(), JobId(2));
        assert_eq!(table.find(JobSpec::Previous).unwrap().id(), JobId(1));
        assert_eq!(table.find(JobSpec::Id(JobId(1))).unwrap().id(), JobId(1));
        assert!(table.find(JobSpec::Id(JobId(9))).is_none());
    }

    #[test]
    fn numbers_are_reused_once_a_job_is_gone() {
        // Otherwise a long-lived shell has a user typing `%143`.
        let mut table = table_with_two();
        table.apply(&[ChildEvent::Finished(pid(100), ExitStatus::Exited(0))]);
        table.take_reportable();
        table.forget_finished();

        let id = table.add(pid(300), vec![pid(300)], "cat".to_owned());
        assert_eq!(id, JobId(1), "the freed number should come back");
    }

    #[test]
    fn a_finished_job_stops_being_current() {
        let mut table = table_with_two();
        table.apply(&[ChildEvent::Finished(pid(200), ExitStatus::Exited(0))]);
        table.take_reportable();
        table.forget_finished();

        // `%%` must not point at a job that no longer exists.
        assert_ne!(table.current(), Some(JobId(2)));
        assert!(table.find(JobSpec::Current).is_none() || table.current() == Some(JobId(1)));
    }

    #[test]
    fn a_state_change_is_reported_once() {
        let mut table = table_with_two();
        table.apply(&[ChildEvent::Stopped(pid(100), Signal::SIGTSTP)]);

        assert_eq!(table.take_reportable().len(), 1);
        assert!(table.take_reportable().is_empty(), "reported twice");
    }

    #[test]
    fn resuming_makes_the_job_reportable_again() {
        let mut table = table_with_two();
        table.apply(&[ChildEvent::Stopped(pid(100), Signal::SIGTSTP)]);
        table.take_reportable();

        table.apply(&[ChildEvent::Continued(pid(100))]);
        assert_eq!(
            table.find(JobSpec::Id(JobId(1))).unwrap().state(),
            JobState::Running
        );
    }

    #[test]
    fn events_for_untracked_children_are_ignored() {
        // A foreground command is waited for directly and never enters the
        // table, so its death arrives here with nowhere to go.
        let mut table = table_with_two();
        table.apply(&[ChildEvent::Finished(pid(999), ExitStatus::Exited(0))]);
        assert_eq!(table.iter().count(), 2);
    }
}
