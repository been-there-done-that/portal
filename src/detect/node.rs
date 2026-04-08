// stub — implemented in a later task
use std::path::Path;

pub const KNOWN_RUNNERS: &[&str] = &[];

pub fn is_known_runner(_cmd: &str) -> bool { false }

pub fn resolve_run_args(_cwd: &Path, args: Vec<String>) -> Vec<String> { args }
