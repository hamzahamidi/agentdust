pub trait Step {
    fn name(&self) -> String;
    fn apply(&mut self) -> Result<(), String>;
    fn undo(&mut self) -> Result<(), String>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    pub step: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Undone {
    pub step: String,
    pub result: Result<(), String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Report {
    pub completed: Vec<String>,
    pub failure: Option<Failure>,
    pub rollback: Vec<Undone>,
}

impl Report {
    pub fn succeeded(&self) -> bool {
        self.failure.is_none()
    }

    pub fn rolled_back_cleanly(&self) -> bool {
        self.rollback.iter().all(|undone| undone.result.is_ok())
    }
}

pub fn run(steps: &mut [Box<dyn Step + '_>]) -> Report {
    let mut report = Report::default();
    let mut failed_at = None;
    for (position, step) in steps.iter_mut().enumerate() {
        match step.apply() {
            Ok(()) => report.completed.push(step.name()),
            Err(reason) => {
                report.failure = Some(Failure {
                    step: step.name(),
                    reason,
                });
                failed_at = Some(position);
                break;
            }
        }
    }
    if let Some(position) = failed_at {
        for step in steps[..position].iter_mut().rev() {
            report.rollback.push(Undone {
                step: step.name(),
                result: step.undo(),
            });
        }
    }
    report
}
