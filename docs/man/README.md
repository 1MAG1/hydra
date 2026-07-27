# Manual pages

Seven section-1 pages, written in `man(7)` and verified against the release binary
rather than against the source comments. `mandoc -Tlint` is clean on all of them.

| Page | Covers |
|---|---|
| `hydra.1` | the download command: every top-level flag, protocols, wget/curl compatibility, JSON schema, exit codes, environment, files |
| `hydra-interactive.1` | the queue manager, its keys, and what backgrounding actually does |
| `hydra-checksum.1` | advertised-digest retrieval and the trust boundary around it |
| `hydra-parity.1` | at-rest Reed–Solomon parity, and why digests come first |
| `hydra-formats.1` | the format catalogue and the category→directory mapping |
| `hydra-bench.1` | the measurement harnesses |
| `hydra-completions.1` | shell completion scripts: `completions` (print) vs `install-completions` (write + report the remaining manual step per shell) |

## Portability

Plain `man(7)` macros only — no `mdoc`, no GNU extensions, no UTF-8 in the source.
The macro set used is `.TH .SH .SS .TP .PP .IP .RS .RE .RI .RB .BR .BI .IB .IR .B
.I .br .nf .fi .TS .TE`, which renders identically under groff (Linux) and mandoc
(macOS, \*BSD, illumos). `hydra.1` and `hydra-formats.1` contain `tbl` tables and
therefore start with the `'\" t` preprocessor line.

## Install

```sh
./install.sh                  # into /usr/local/share/man/man1
PREFIX=~/.local ./install.sh  # into a home prefix
./install.sh --check          # lint and render only, install nothing
```

`--check` fails non-zero on any non-STYLE diagnostic, so it is usable as a CI step.
The installer refuses to install pages that do not lint.

## Keeping them honest

The pages document **observed** behaviour. Every claim about exit codes, output
files, JSON fields, and flag effects was checked by running the release binary,
which is how the `BUGS` sections got written: nine flags parse but do nothing,
`--max-redirs` ignores its value, `--fail` still writes the error body, and
multi-file runs do not deduplicate colliding basenames. None of those are visible
from the `--help` text, and two of them (`--fail`, basename collision) are the
project's recurring failure shape — a file that exists and looks plausible.

When a flag is wired up, delete its "accepted but not implemented" note *and* its
`BUGS` paragraph. When a new flag is added, `hydra --help` is the starting point
but not the authority; run it before documenting what it does.
