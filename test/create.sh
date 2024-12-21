#!/bin/bash

# git-absorb binary path
if [ $# -eq 0 ]; then
    GIT_ABSORB_BIN="git absorb"
else
    GIT_ABSORB_BIN=$1
fi


README_FILE=README.md
GIT_ANNOTATED_TAG="annotated-tag"

# initialize a git repository
git init

# commit 1: create a sample file
touch ${README_FILE}
# commit 1: stage change
git add ${README_FILE}
# commit 1: create initial commit
git commit -m 'Initial commit'

# commit 2: update file
cat <<EOF >| ${README_FILE}
# readme

Testing

## header 2: part 1

Header 2 testing. part 1

## header 2: part 2

Header 2 testing. part 2
EOF
# commit 2: stage updates
git add ${README_FILE}
# commit 2: create commit
git commit -m 'Update readme'

# commit 2: make this commit as annotated commit
git tag -a ${GIT_ANNOTATED_TAG} -m 'my annotated tag'

# commit 3: insert lines in the middle
ed ${README_FILE} <<< '9i
### header 3: part 1

this is header 3

.
w
q
'
# commit 3: stage changes
git add ${README_FILE}
# commit 3: create commit
git commit -m 'More updates'

# commit 4: insert additional lines
ed ${README_FILE} <<< '7d
7i
Header 2 testing.

foo

part 1
.
w
q'
# commit 4: stage changes
git add ${README_FILE}
# commit 4: create commit
git commit -m 'Commute commit'

# commit 5: create the diff that'll commute
ed ${README_FILE} <<< '9d
9i
foobar
.
w
q'
# commit 5: stage change
git add -u ${README_FILE}
# commit 5: try to absorb
${GIT_ABSORB_BIN} -v --base ${GIT_ANNOTATED_TAG}
