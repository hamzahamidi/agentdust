use std::io;

use crate::classifier::Finding;
use crate::finding::ModelFinding;

pub trait Surveyor: Send + Sync {
    fn survey(&self) -> io::Result<Vec<Finding>>;

    fn describe(&self, finding: &Finding) -> ModelFinding;
}
