#!/bin/bash

# Test for https://github.com/tummychow/git-absorb/issues/6:
# - Hunks with line removals
# - Multiple hunks removing lines in the same file
# - Multiple hunks adding lines in the same file

# Decide which git-absorb to use: the one provided via $GIT_ABSORB or the default target/debug/git-absorb
DEFAULT_GIT_ABSORB=`dirname $0`/../target/debug/git-absorb
RELATIVE_GIT_ABSORB="${GIT_ABSORB:-$DEFAULT_GIT_ABSORB}"
GIT_ABSORB=$(cd `dirname $RELATIVE_GIT_ABSORB` && echo `pwd`/`basename $RELATIVE_GIT_ABSORB`)

function error_exit {
    echo "ERROR: $1" >&2
    exit "${2:-1}"         # Return a code specified by $2 or 1 by default.
}

TESTDIR=`dirname $0`/data-issue-006/
rm -rf $TESTDIR &&  mkdir $TESTDIR  &&  cd $TESTDIR     &&  git init     || error_exit "Unable to init repository $?"
# the "base" commit and its parents (if any) will not be considered as fixup targets by git-absorb
git commit --allow-empty -m "base"  &&  
echo -e "line1\nline2\nline3" > test-file-old  &&  git add test-file-old  &&   git commit -m "an old commit that cannot be fixed up" &&
git tag base &&

# the commits that can be fixed up:
echo -e "line1\nline2\nline3\nline4\nline5" > test-file  &&  git add test-file  &&   git commit -m "commit1"           &&
echo -e "line1_2\nline2_3\nline3_5" > test-file2  &&  git add test-file2  &&   git commit -m "commit2"           &&
echo -e "line1\nline2\nline3\nline4\nline5\nline6\nline7" > test-file-old  &&  git add test-file-old  &&   git commit -m "commit3"           &&
git checkout -b expected    &&

# the fixup commits that git-absorb is expected to generate:
echo -e "line1\nline3\nline4\nline5" > test-file                &&  git add test-file  &&   git commit -m "fixup! commit1"    &&
echo -e "line1\nline3\nline4" > test-file                       &&  git add test-file  &&   git commit -m "fixup! commit1"    &&
echo -e "line1\nline2\nline3\nline4\nline ADDED after 4\nline5\nline6\nline7" > test-file-old  &&  git add test-file-old  &&   git commit -m "fixup! commit3"           &&
echo -e "line_added_1\nline1_2\nline2_3\nline3_5" > test-file2  &&  git add test-file2  &&   git commit -m "fixup! commit2"           &&
echo -e "line_added_1\nline1_2\nline2_3\nline_added_4\nline3_5" > test-file2  &&  git add test-file2  &&   git commit -m "fixup! commit2"           &&
echo -e "line1\nline ADDED\nADDED\nline2\nline3\nline4\nline ADDED after 4\nline5\nline6\nline7" > test-file-old  &&  git add test-file-old  &&   git commit -m "left unabsorbed"           &&

# undo all the fixup commits and put changes from them into the index for git-absorb to process:
git checkout -b actual      &&
git reset --hard expected && git reset --soft master || error_exit "Testcase setup failure"

#exit 0

echo "======== Running git-absorb"
RUST_BACKTRACE=1 $GIT_ABSORB -v -b base || error_exit "git-absorb exited with error $?"

if [ -n "$(git status --porcelain)" ]; then
    git commit -m "left unabsorbed" || error_exit "Committing unabsorbed files failed"
    if [ -n "$(git status --porcelain)" ]; then
        # probably untracked files?
        error_exit "Working directory not clean!"
    fi
fi

# Uncomment to compare the fixup results instead of the fixup commits:
#
# GIT_SEQUENCE_EDITOR=: git rebase --interactive --autosquash base &&
# git checkout expected && GIT_SEQUENCE_EDITOR=: git rebase --interactive --autosquash base || error_exit "squashing failed"

echo "======== Checking results"
n=0
for expected_commit in $(git rev-list base..expected)  # list commits from `expected` up to, but not including, `base`
do
    actual_commit="actual~${n}"
    echo "==== Comparing ${n}-th expected to actual fixup commit's contents: $expected_commit..$actual_commit"
    git diff $expected_commit..$actual_commit --exit-code || error_exit "Commit contents differ"
    echo "OK.. comparing message:"
    diff -U1 \
         <(git show -s --format=%B $expected_commit) \
         <(git show -s --format=%B $actual_commit) || error_exit "Commit messages differ"
    n=$((n+1))
done

echo "OK!"
exit 0
