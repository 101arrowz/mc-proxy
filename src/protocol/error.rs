use thiserror::Error;

#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum Error {
    #[error("unexpected end of input")]
    UnexpectedEof,
    #[error("output buffer too small")]
    NeedMore,
    #[error("malformed data")]
    Malformed,
}
