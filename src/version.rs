const BRANCH: &'static str = env!("VERGEN_GIT_BRANCH");
const COMMIT: &'static str = env!("VERGEN_GIT_SHA");
const DIRTY: &'static str = env!("VERGEN_GIT_DIRTY");

pub fn for_version_command() -> String {
    let dirty = match DIRTY {
        "true" => "*",
        _ => "",
    };
    let commit = &COMMIT[..(usize::min(8, COMMIT.len()))];
    format!("{BRANCH}:{commit}{dirty}")
}
