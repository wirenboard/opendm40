#!/bin/sh
# The English and Russian READMEs and user manuals stay in step (CI, the doc
# checks in .github/workflows/ci.yml; AGENTS.md "Docs in two languages"):
#  1. each pair has the same number of ## / ### / #### headings and the same
#     images, each as often (a section or a picture added in one language
#     only fails);
#  2. with BASE: a change since the merge base of BASE and HEAD that touches
#     one file of a pair touches the other too. Escape hatch for a typo fix
#     in one language: a line "Doc-lang: one-language" in a commit message
#     of the range or in $PR_BODY (CI passes the PR description).
#   tools/doc-lang-check.sh [BASE [HEAD]]   (HEAD default: HEAD)
set -e
cd "$(dirname "$0")/../.."
BASE=$1; HEAD=${2:-HEAD}
PAIRS="README.md:README.ru.md docs/manual/README.md:docs/manual/README.ru.md"
bad=0
# "<level> <count>" per heading level, outside ``` fences.
headings() {
    awk '/^[ \t]*```/ { f = !f; next } !f && /^##+ / && length($1) <= 4 { n[length($1)]++ }
         END { for (l = 2; l <= 4; l++) printf "%s %d\n", substr("####", 1, l), n[l] }' "$1"
}
# The image paths, one per use, sorted.
images() {
    grep -o '!\[[^]]*\]([^) ]*' "$1" | sed 's/.*](//' | sort
}
for p in $PAIRS; do
    en=${p%%:*}; ru=${p#*:}
    for f in "$en" "$ru"; do
        [ -f "$f" ] || { echo "doc-lang-check: $f is missing" >&2; bad=1; continue 2; }
    done
    if [ "$(headings "$en")" != "$(headings "$ru")" ]; then
        echo "doc-lang-check: $en and $ru have different headings (level count):" >&2
        headings "$en" | sed "s|^|  $en: |" >&2
        headings "$ru" | sed "s|^|  $ru: |" >&2
        bad=1
    fi
    d=$(images "$en" > "${TMPDIR:-/tmp}/dlc.$$.en"; images "$ru" > "${TMPDIR:-/tmp}/dlc.$$.ru"
        diff "${TMPDIR:-/tmp}/dlc.$$.en" "${TMPDIR:-/tmp}/dlc.$$.ru" | grep '^[<>]' || true)
    rm -f "${TMPDIR:-/tmp}/dlc.$$.en" "${TMPDIR:-/tmp}/dlc.$$.ru"
    if [ -n "$d" ]; then
        echo "doc-lang-check: $en (<) and $ru (>) show different images:" >&2
        echo "$d" | sed 's/^/  /' >&2
        bad=1
    fi
done
if [ -n "$BASE" ]; then
    MB=$(git merge-base "$BASE" "$HEAD")
    changed=$(git diff --name-only "$MB" "$HEAD")
    escape=no
    if git log --format=%B "$MB..$HEAD" | grep -q '^Doc-lang: one-language' ||
       printf '%s\n' "$PR_BODY" | tr -d '\r' | grep -q '^Doc-lang: one-language'; then
        escape=yes
    fi
    for p in $PAIRS; do
        en=${p%%:*}; ru=${p#*:}
        ce=$(echo "$changed" | grep -cxF "$en" || true)
        cr=$(echo "$changed" | grep -cxF "$ru" || true)
        [ "$ce" = "$cr" ] && continue
        one=$en; other=$ru; [ "$cr" = 1 ] && { one=$ru; other=$en; }
        if [ $escape = yes ]; then
            echo "doc-lang-check: only $one changed; allowed by \"Doc-lang: one-language\""
        else
            echo "doc-lang-check: $one changed since $(echo "$MB" | cut -c1-7) but $other did not." >&2
            echo "  Make the same change in both languages, or, for a typo fix in one language only," >&2
            echo "  add the line \"Doc-lang: one-language\" to a commit message or the PR description." >&2
            bad=1
        fi
    done
fi
[ $bad = 0 ] && echo "doc-lang-check: ok"
exit $bad
