# `jless`

`jless` is a command-line pager utility for viewing JSON files.

# Releases

## Build Linux release:

```
brew tap SergioBenitez/osxct
brew install x86_64-unknown-linux-gnu
CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=x86_64-unknown-linux-gnu-gcc cargo build --release --target=x86_64-unknown-linux-gnu
```
