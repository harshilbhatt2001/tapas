//! Store sync between machines. For now only the pure merge core; the Drive transport lives
//! in `google::drive`, and the orchestration (sync state, base document) builds on both.

pub mod merge;
