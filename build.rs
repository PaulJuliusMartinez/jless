use anyhow::Result;
use vergen::EmitBuilder;

pub fn main() -> Result<()> {
    EmitBuilder::builder()
        .git_branch()
        .git_sha(/* short */ false)
        .git_dirty(/* include_untracked */ true)
        .emit()?;

    Ok(())
}
