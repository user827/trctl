pub use anyhow::{anyhow, bail, Context as _, Error, Result};

#[derive(Debug)]
pub struct NoMatches;
impl std::error::Error for NoMatches {}
impl std::fmt::Display for NoMatches {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Nothing found")
    }
}

#[derive(Debug)]
pub struct NothingToDo(pub &'static str);
impl std::error::Error for NothingToDo {}
impl std::fmt::Display for NothingToDo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug)]
pub struct Multiple(pub usize);
impl std::error::Error for Multiple {}
impl std::fmt::Display for Multiple {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Had {} errors", self.0)
    }
}

#[derive(Debug)]
pub struct NotEnoughSpace;
impl std::error::Error for NotEnoughSpace {}
impl std::fmt::Display for NotEnoughSpace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Not enought space")
    }
}

#[derive(Debug)]
pub struct MagnetURIError(pub magnet_uri::Error);
impl std::fmt::Display for MagnetURIError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match &self.0 {
            magnet_uri::Error::UrlEncode(e) => e.fmt(f),
            magnet_uri::Error::Scheme => write!(f, "Wrong scheme for a magnet uri"),
            magnet_uri::Error::Field(x, y) => write!(f, "Magnet URI: invalid format {x}: {y}"),
            magnet_uri::Error::ExactTopic(x) => write!(f, "Magnet URI exact topic error: {x}"),
        }
    }
}
impl std::error::Error for MagnetURIError {}
