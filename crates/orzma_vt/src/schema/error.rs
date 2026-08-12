//! Error channel for fallible [`OrzmaVt`](crate::vt::OrzmaVt) operations.

use thiserror::Error;

/// Result alias for [`OrzmaVt`](crate::vt::OrzmaVt) operations.
pub type VtResult<T = ()> = Result<T, VtError>;

/// An error raised by a VT operation.
///
/// No variants exist yet; the enum reserves the error channel that
/// [`VtResult`] carries.
#[derive(Error, Debug)]
pub enum VtError {}
