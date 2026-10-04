#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Point {
    AfterOpen,
    BeforeWrite,
    AfterWrite,
    ReaderAfterActive,
    ReaderAfterList,
}

pub trait Probe: Sync {
    fn reached(&self, point: Point);
}

pub struct NoProbe;

impl Probe for NoProbe {
    fn reached(&self, _point: Point) {}
}
