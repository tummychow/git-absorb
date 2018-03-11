# git absorb

[![crates.io badge](https://img.shields.io/crates/v/git-absorb.svg)](https://crates.io/crates/git-absorb)

This is a port of Facebook's [`hg absorb`](https://bitbucket.org/facebook/hg-experimental/src/default/hgext3rd/absorb/__init__.py?at=default&fileviewer=file-view-default), which I first read about on [mozilla.dev.version-control](https://groups.google.com/forum/#!msg/mozilla.dev.version-control/nh4fITFlEMk/ZNXgnAzxAQAJ).

## Elevator Pitch

You have a feature branch with a few commits. Your teammate reviewed the branch and pointed out a few bugs. You have fixes for the bugs, but you don't want to shove them all into an opaque commit that says `fixes`, because you believe in atomic commits. Instead of manually finding commit SHAs for `git commit --fixup`, or running a manual interactive rebase, do this:

```
git add $FILES_YOU_FIXED
git absorb
git rebase -i --autosquash
```

`git absorb` will automatically identify which commits are safe to modify, and which indexed changes belong to each of those commits. It will then write `fixup!` commits for each of those changes. You can check its output manually if you don't trust it, and then fold the fixups into your feature branch with git's built-in autosquash functionality.

## Installing

You will need the following:

- [cargo](https://github.com/rust-lang/cargo)
- [CMake](https://cmake.org)
- libgit2 [optional dependencies](https://github.com/libgit2/libgit2#optional-dependencies)

Then `cargo install git-absorb`. Make sure that `$CARGO_HOME/bin` is on your `$PATH` so that git can find the command. (`$CARGO_HOME` defaults to `~/.cargo`.)

Note that `git absorb` does _not_ use the system libgit2. This means you do not need to have libgit2 installed to build or run it. However, this does mean you have to be able to build libgit2, hence the build-time dependency on CMake.
