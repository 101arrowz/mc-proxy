use thiserror::Error;

#[derive(Debug, PartialEq, Error)]
pub enum Error {
    #[error("unexpected end of input")]
    UnexpectedEof,
    #[error("output buffer too small")]
    NeedMore,
    #[error("malformed data")]
    Malformed,
}
