## Release Checklist

- `VERSION=<new version>` (including a `v` at the start)
- Update version in [`Cargo.toml`](./Cargo.toml).
- Run `cargo build` to update [`Cargo.lock`](./Cargo.lock).
- Add changes since last release to [`CHANGELOG.md`](./CHANGELOG.md). (You
  should do this with every commit!)
  - Update the top of the CHANGELOG to say the new version number with
    the release date, then start a new section for `main`
- Commit all changes with commit message: `vX.Y.Z Release`
- Tag commit and push it to GitHub: `git tag $VERSION && git push origin $VERSION`
- Publish new version to crates.io: `cargo publish`
- Generate new binaries:
  - macOS:
    - `cargo build --release`
    - `cd target/release`
    - `zip -r -X jless-$VERSION-x86_64-apple-darwin.zip jless`
  - Linux:
    - Make sure you can cross-compile for Linux:
      - `brew tap SergioBenitez/osxct`
      - `brew install x86_64-unknown-linux-gnu`
    - `CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=x86_64-unknown-linux-gnu-gcc cargo build --release --target=x86_64-unknown-linux-gnu`
    - `cd target/x86_64-unknown-linux-gnu/release`
    - `zip -r -X jless-$VERSION-x86_64-unknown-linux-gnu.zip target/x86_64-unknown-linux-gnu/release/jless`
- Create GitHub release
  - Click "Create new release"
  - Select tag
  - Copy stuff from `CHANGELOG.md` to description
  - Attach binaries generated above
- Update the [`website` branch](https://github.com/PaulJuliusMartinez/jless/tree/website)
  - Update [`releases_page.rb`](https://github.com/PaulJuliusMartinez/jless/blob/website/releases_page.rb) with the new release
  - Update [`user_guide_page.rb`](https://github.com/PaulJuliusMartinez/jless/blob/website/user_guide_page.rb) with any new commands
