#!/bin/bash
# Pull request test builds (.github/workflows/pr-build.yml, firmware/README.md
# "PR builds"): one shared prerelease, tag `pr-builds`, holds the current
# build of every open PR; one comment per PR links it.
#   tools/pr-build.sh publish RUN_ID [--dry-run]   after a CI run of a PR
#   tools/pr-build.sh remove PR                      the PR was closed
# Needs gh with a token that may read the run's artifacts, write releases
# and PR comments (GH_TOKEN, GH_REPO).
# The CI run built untrusted PR code: its artifact is only data here. The
# script never runs anything from it; it takes the PR number from the
# artifact name, checks it against the run's head commit, unzips into an
# empty directory and hashes the .bin itself. A run that is not the PR's
# current head (an older push) publishes nothing.
set -euo pipefail
usage() { echo "usage: pr-build.sh publish RUN_ID [--dry-run] | remove PR" >&2; exit 2; }
CMD=${1:-}; ARG=${2:-}; DRY=${3:-}
[[ $ARG =~ ^[0-9]+$ ]] || usage
REPO=${GH_REPO:?GH_REPO (owner/repo)}
SERVER=${GITHUB_SERVER_URL:-https://github.com}
# The comment's author: the workflow's token posts as github-actions[bot].
AUTHOR=${COMMENT_AUTHOR:-github-actions[bot]}
MARK='<!-- opendm40-pr-build -->'
TAG=pr-builds
MAN=$SERVER/$REPO/blob/main/docs/manual/README.md
NOTES="Test builds of open pull requests, one per PR: built by CI from unreviewed, unmerged changes and not tested on a meter. Each PR's comment names its file, SHA-256 and commit; a PR's file is replaced on every push and removed when the PR is closed.

Install like an update ([manual, Update]($MAN#update)). To go back, install the [latest release]($SERVER/$REPO/releases/latest); if the meter does not start: [Backup, recovery and back to stock]($MAN#backup-recovery-and-back-to-stock)."

# This PR's assets on the release: dm40_opendm40_v<version>-pr<N>.<commit>.bin
assets() { # PR -> "id name" lines
    local rel
    rel=$(gh api "repos/$REPO/releases/tags/$TAG" --jq .id 2>/dev/null) || return 0
    gh api --paginate "repos/$REPO/releases/$rel/assets?per_page=100" --jq \
        ".[] | select(.name | test(\"^dm40_opendm40_v[0-9A-Za-z.-]+-pr$1\\\\.[0-9a-f]{7}\\\\.bin\$\")) | \"\\(.id) \\(.name)\"" 2>/dev/null || true
}
comment_id() { # PR -> id of this script's comment
    gh api --paginate "repos/$REPO/issues/$1/comments" \
        --jq ".[] | select(.user.login == \"$AUTHOR\") | select(.body | startswith(\"$MARK\")) | .id" | head -1
}
comment() { # PR BODY: edit this script's comment, or post it
    local id
    id=$(comment_id "$1")
    if [ -n "$id" ]; then
        gh api -X PATCH "repos/$REPO/issues/comments/$id" -f body="$2" --jq .html_url
    else
        gh api -X POST "repos/$REPO/issues/$1/comments" -f body="$2" --jq .html_url
    fi
}

if [ "$CMD" = remove ]; then
    PR=$ARG
    assets "$PR" | while read -r id name; do
        echo "pr-build: delete $name"
        gh api -X DELETE "repos/$REPO/releases/assets/$id"
    done
    id=$(comment_id "$PR")
    if [ -n "$id" ]; then
        gh api -X PATCH "repos/$REPO/issues/comments/$id" --jq .html_url \
            -f body="$MARK
### Test build of this pull request

Removed: the pull request is closed."
    fi
    exit 0
fi
[ "$CMD" = publish ] || usage
RUN=$ARG

read -r EVENT HEAD CONCL RUNNUM < <(gh api "repos/$REPO/actions/runs/$RUN" \
    --jq '[.event, .head_sha, (.conclusion // "running"), .run_number] | @tsv')
[ "$EVENT" = pull_request ] || { echo "pr-build: run $RUN is a $EVENT run, nothing to do"; exit 0; }

# The image artifact: dm40_opendm40_v<version>-pr<N>.<head7> (ci.yml).
ART=$(gh api "repos/$REPO/actions/runs/$RUN/artifacts" \
    --jq '.artifacts[] | select(.expired | not) | select(.name | test("^dm40_opendm40_v[0-9A-Za-z.-]+-pr[0-9]+\\.[0-9a-f]{7}$")) | .name' | head -1)
[ -n "$ART" ] || { echo "pr-build: run $RUN has no image artifact (docs-only change or a failed build)"; exit 0; }
NAME=$ART
[[ $NAME =~ -pr([0-9]+)\.([0-9a-f]{7})$ ]]
PR=${BASH_REMATCH[1]}; SHORT=${BASH_REMATCH[2]}
VER=${NAME#dm40_opendm40_v}

# The PR named in the artifact name must be open, with this run's commit as
# its head.
read -r STATE PRHEAD < <(gh api "repos/$REPO/pulls/$PR" --jq '[.state, .head.sha] | @tsv')
if [ "$STATE" != open ] || [ "$PRHEAD" != "$HEAD" ] || [ "${HEAD:0:7}" != "$SHORT" ]; then
    echo "pr-build: PR #$PR is $STATE at $PRHEAD, run $RUN built $HEAD ($SHORT): not the current head, nothing published"
    exit 0
fi

DIR=$(mktemp -d)
trap 'rm -rf "$DIR"' EXIT
gh run download "$RUN" -R "$REPO" -n "$NAME" -D "$DIR/a"
BIN=$DIR/a/$NAME.bin
[ -f "$BIN" ] && [ ! -L "$BIN" ] || { echo "pr-build: $NAME.bin not in the artifact" >&2; exit 1; }
SUM=$(sha256sum "$BIN" | cut -d' ' -f1)
# BUILD.txt (ci.yml): the commit CI built, the PR head merged into the base.
MERGE=$(sed -n 's/^merge \([0-9a-f]\{40\}\)$/\1/p' "$DIR/a/BUILD.txt" 2>/dev/null || true)
BASE=$(sed -n 's/^base \([0-9a-f]\{40\}\)$/\1/p' "$DIR/a/BUILD.txt" 2>/dev/null || true)

C=$SERVER/$REPO/commit
built="PR head [\`${HEAD:0:7}\`]($C/$HEAD)"
[ -n "$BASE" ] && built="$built merged into \`main\` at [\`${BASE:0:7}\`]($C/$BASE)"
[ -n "$MERGE" ] && built="$built (merge commit \`${MERGE:0:7}\`)"
# The asset's public URL (no login), as GitHub forms it.
URL=$SERVER/$REPO/releases/download/$TAG/$NAME.bin
BODY=$(cat <<EOF
$MARK
### Test build of this pull request

[\`$NAME.bin\`]($URL) (no GitHub account needed): the meter shows the version \`$VER\` (start screen, Settings > More > More > Device, \`*IDN?\`).

| | |
|---|---|
| SHA-256 | \`$SUM\` |
| Built from | $built |
| CI | [run #$RUNNUM]($SERVER/$REPO/actions/runs/$RUN): $CONCL |

> [!WARNING]
> A test build of an unreviewed, unmerged change: not a release, not tested on a meter. Install it like an update ([manual, Update]($MAN#update)). To go back, install the [latest release]($SERVER/$REPO/releases/latest); if the meter does not start, see [Backup, recovery and back to stock]($MAN#backup-recovery-and-back-to-stock).

Replaced on every push to this pull request ([all PR builds]($SERVER/$REPO/releases/tag/$TAG)); removed when it is closed.
EOF
)
if [ "$DRY" = --dry-run ]; then
    echo "$BODY"
    exit 0
fi

# The shared prerelease, created once (idempotent: a parallel run may have
# made it meanwhile). A prerelease is never "Latest"; --latest=false too.
if ! gh release view "$TAG" -R "$REPO" > /dev/null 2>&1; then
    gh release create "$TAG" -R "$REPO" --prerelease --latest=false --target main \
        --title "PR test builds (untested)" --notes "$NOTES" > /dev/null 2>&1 ||
        gh release view "$TAG" -R "$REPO" > /dev/null
fi
OLD=$(assets "$PR")
gh release upload "$TAG" -R "$REPO" --clobber "$BIN"
# This PR's earlier builds: one current build per PR.
echo "$OLD" | while read -r id name; do
    [ -n "$id" ] && [ "$name" != "$NAME.bin" ] || continue
    echo "pr-build: delete $name"
    gh api -X DELETE "repos/$REPO/releases/assets/$id"
done
comment "$PR" "$BODY"
