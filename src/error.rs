use std::fmt::Display;

pub trait OutputLogIfFalse {
    fn log_err_if_failed(self);
}

impl<T, E: Display> OutputLogIfFalse for Result<T, E> {
    fn log_err_if_failed(self) {
        if let Err(e) = self {
            tracing::error!("{e}");
        }
    }
}
