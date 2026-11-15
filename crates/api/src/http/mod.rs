//! HTTP plumbing: the middleware, and what the handlers share (extractors,
//! answers, pagination).

pub mod extract;
pub mod middleware;
pub mod pagination;
pub mod reply;
