# git absorb

[![crates.io badge](https://img.shields.io/crates/v/git-absorb.svg)](https://crates.io/crates/git-absorb)

This is a port of Facebook's [`hg absorb`](https://bitbucket.org/facebook/hg-experimental/src/default/hgext3rd/absorb/__init__.py?at=default&fileviewer=file-view-default), which I first read about on [mozilla.dev.version-control](https://groups.google.com/forum/#!msg/mozilla.dev.version-control/nh4fITFlEMk/ZNXgnAzxAQAJ).

## Elevator Pitch

You have a feature branch with a few commits. Your teammate reviewed the branch and pointed out a few bugs. You have fixes for the bugs, but you don't want to shove them all into an opaque commit that says `fixes`, because you believe in atomic commits. Instead of manually finding commit SHAs for `git commit --fixup`, or running a manual interactive rebase, do this:

```
git add $FILES_YOU_FIXED
git absorb --and-rebase
# or: git absorb && git rebase -i --autosquash master
```

`git absorb` will automatically identify which commits are safe to modify, and which staged changes belong to each of those commits. It will then write `fixup!` commits for each of those changes.

With the `--and-rebase` flag, these fixup commits will be automatically integrated into the corresponding ones. Alternatively, you can check its output manually if you don't trust it, and then fold the fixups into your feature branch with git's built-in [autosquash](https://git-scm.com/docs/git-rebase#Documentation/git-rebase.txt-rebaseautoStash) functionality:

```
git add $FILES_YOU_FIXED
git absorb
git log # check the auto-generated fixup commits
git rebase -i --autosquash master
```

## Installing

You will need the following:

- [cargo](https://github.com/rust-lang/cargo)

Then `cargo install git-absorb`. Make sure that `$CARGO_HOME/bin` is on your `$PATH` so that git can find the command. (`$CARGO_HOME` defaults to `~/.cargo`.)

Note that `git absorb` does _not_ use the system libgit2. This means you do not need to have libgit2 installed to build or run it. However, this does mean you have to be able to build libgit2. (Due to [recent changes](https://github.com/alexcrichton/git2-rs/commit/76f4b74aef2bc2a54906ddcbf7fbe0018936a69d) in the git2 crate, CMake is no longer needed to build it.)

Alternatively, `git absorb` is available in the following system package managers:

| Repository                  | Command                                      |
| --------------------------- | -------------------------------------------- |
| AUR                         | `yay -S git-absorb`                          |
| DPorts                      | `pkg install git-absorb`                     |
| FreeBSD Ports               | `pkg install git-absorb`                     |
| Homebrew and Linuxbrew      | `brew install git-absorb`                    |
| nixpkgs stable and unstable | `nix-env -iA nixpkgs.gitAndTools.git-absorb` |
| Void Linux                  | `xbps-install -S git-absorb`                 |

## Usage

1. `git add` any changes that you want to absorb. By design, `git absorb` will only consider content in the git index (staging area).
2. `git absorb`. This will create a sequence of commits on `HEAD`. Each commit will have a `fixup!` message indicating the message (if unique) or SHA of the commit it should be squashed into.
3. If you are satisfied with the output, `git rebase -i --autosquash` to squash the `fixup!` commits into their predecessors. You can set the [`GIT_SEQUENCE_EDITOR`](https://stackoverflow.com/a/29094904) environment variable if you don't need to edit the rebase TODO file.
4. If you are not satisfied (or if something bad happened), `git reset --soft` to the pre-absorption commit to recover your old state. (You can find the commit in question with `git reflog`.) And if you think `git absorb` is at fault, please [file an issue](https://github.com/tummychow/git-absorb/issues/new).

## Configuration

### Stack size

When run without `--base`, git-absorb will only search for candidate commits to fixup within a certain range (by default 10). If you get an error like this:

```
WARN stack limit reached, limit: 10
```

edit your local or global `.gitconfig` and add the following section

```ini
[absorb]
    maxStack=50 # Or any other reasonable value for your project
```

## TODO

- implement force flag
- implement remote default branch check
- add smaller force flags to disable individual safety checks
- stop using `failure::err_msg` and ensure all error output is actionable by the user
- slightly more log output in the success case
- more tests (esp main module and integration tests)
- travis
- windows support and appveyor
- document stack and commute details
- more commutation cases (esp copy/rename detection)
- don't load all hunks in memory simultaneously because they could be huge
- implement some kind of index locking to protect against concurrent modifications
