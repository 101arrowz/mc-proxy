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

pub fn handle_read_err(err: std::io::Error) -> Error {
    if err.kind() == std::io::ErrorKind::UnexpectedEof {
        Error::UnexpectedEof
    } else {
        todo!();
    }
}

pub fn handle_write_err(err: std::io::Error) -> Error {
    if err.kind() == std::io::ErrorKind::WriteZero {
        Error::NeedMore
    } else {
        todo!();
    }
}