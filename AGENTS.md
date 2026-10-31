# Notes for coding agents

Build, tests and CI: [firmware/README.md](firmware/README.md). Contribution
rules: [CONTRIBUTING.md](CONTRIBUTING.md).

## Docs in two languages

The README and the user manual exist in English and Russian:

| English | Russian |
|---|---|
| [README.md](README.md) | [README.ru.md](README.ru.md) |
| [docs/manual/README.md](docs/manual/README.md) | [docs/manual/README.ru.md](docs/manual/README.ru.md) |

- Every change to one of them is made in both languages in the same commit
  or pull request. Translate it; do not leave a TODO or English text in the
  Russian file.
- Same structure in both: the same headings at the same levels and the same
  images. Intra-document links in a Russian file point to its Russian
  headings; links to English-only documents say «(на английском)».
- Keep the terms of the glossary at the end of the Russian manual (an HTML
  comment); on-screen labels, keys, SCPI commands and file names stay in
  English.
- [`firmware/tools/doc-lang-check.sh`](firmware/tools/doc-lang-check.sh)
  enforces this in CI (Doc checks): run
  `firmware/tools/doc-lang-check.sh origin/main` before a push. For a typo
  fix in one language only, add the line `Doc-lang: one-language` to the
  commit message or the PR description.
