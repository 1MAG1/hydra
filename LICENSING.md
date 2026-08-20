# Licensing

| Crate | License | Why |
|---|---|---|
| `hydra-cli` (the `hydra` binary) | `GPL-3.0-or-later` | The product surface. Copyleft here means a modified hydra distributed to users must ship its source. |
| `hydra-core` | `MIT OR Apache-2.0` | An I/O-free scheduler built to be depended on. |
| `hya-net` | `MIT OR Apache-2.0` | Transport, same reasoning. |
| `hya-ffi` (`libhydra`) | `MIT OR Apache-2.0` | The embeddable C ABI. Its entire purpose is being linked into somebody else's application. |

Files: `LICENSE` (GPL-3.0 text, verbatim), `LICENSE-MIT`, `LICENSE-APACHE`,
`THIRD-PARTY-NOTICES.md`.

## Why the split is not uniform

Rust links statically. There is no dynamic-linking boundary of the kind LGPL was
written for, so a GPL library forces every crate that depends on it to become GPL
— there is no escape hatch and no "just link it" middle path. `hydra-core` is
described in its own manifest as a reusable scheduler; making it copyleft would
mean no permissively-licensed project could ever depend on it, and the
permissively-licensed corner of the ecosystem is exactly where a download
scheduler would get used.

The binary is the opposite case. Nobody links a CLI, so GPL on `hydra-cli` costs
nothing in adoption while still preventing a closed-source fork of the tool from
being shipped to users.

`hya-ffi` makes the same argument as sharply as it can be made. It exists so a
third party can embed the engine in an Android app, an iOS framework, a Go
program or a Flutter plugin; a copyleft `libhydra` would propagate into every
one of those and there would be no reason for the crate to exist. Two
consequences follow, and both are load-bearing rather than incidental:

* **`hya-ffi` must never depend on `hya-cli`, `hya-gui` or `hya-host`.** Those
  are GPL, and a single such dependency would relicense the embeddable library
  by accident. The dependency direction is `hya-ffi -> {hya-core, hya-net}` and
  nothing else from this workspace.
* **A libhydra distribution is a different artifact from a hydra distribution.**
  A package containing `libhydra.a` and `include/hydra.h` carries the MIT/Apache
  terms and the third-party notices for the library graph. A package containing
  the `hydra` binary carries GPL-3.0-or-later. Shipping an Android AAR or an
  iOS XCFramework means shipping the first kind, and its `NOTICE` must say so.

The cost of this split, stated plainly: someone may take `hydra-core`'s
scheduling algorithms into a proprietary product without contributing anything
back. Only the assembled tool is protected. If that trade is unacceptable, the
change is `license = "GPL-3.0-or-later"` in the two library manifests — but it
should be a deliberate decision, not a drift.

## Version 3, not version 2

Not a preference. `ring` (reached through `rustls`, and linked into the binary)
is `Apache-2.0 AND ISC`. Apache-2.0's patent-termination clause is an additional
restriction under GPL-2.0, so a GPL-2.0-only hydra could not legally ship TLS.
GPL-3.0 accommodates Apache-2.0 explicitly. `GPL-2.0-or-later` would technically
be satisfiable via its v3 branch, but it advertises a v2 option that cannot be
exercised here, so it is not used.

## What GPL-3.0 does not cover

Network use. hydra's own manifest describes it as "a server-side queue daemon
managing many concurrent transfers"; someone may run a modified hydra as a hosted
service without publishing changes, because that is not distribution. Closing
that gap is what AGPL-3.0 is for. It was considered and not adopted: many
organisations forbid AGPL dependencies outright, which would block benign
internal use and contribution along with the case it targets. Revisit if hydra is
ever actually deployed as a service others reach over a network.

## Third-party terms

Every one of the 109 crates linked into the binary is permissively licensed;
there is no copyleft crate in the graph, so nothing external constrains the
choice above. Four carry terms worth reading individually — `ring`,
`webpki-roots`, `unicode-ident`, `reed-solomon-simd` — and they are annotated in
`THIRD-PARTY-NOTICES.md`.

The `dirs` dependency was removed while auditing this: it was declared in
`hydra-cli` but never called, and it pulled in `option-ext`, the only
MPL-2.0-licensed crate in the graph. Its `LICENSE.txt` contains the Exhibit B
"Incompatible With Secondary Licenses" template, which is boilerplate present in
every copy of the MPL rather than an actual election, and its source files carry
no notice header — so it was likely compatible. Deleting unused code was the
cheaper resolution than depending on that reading.

## Contributions

Inbound contributions are taken under the license of the crate they touch: GPL
for `hydra-cli`, `MIT OR Apache-2.0` for the libraries. A patch that moves code
from a library crate into the binary is fine; the reverse — moving GPL code into
`hydra-core` — relicenses it and needs the author's agreement.

## Not legal advice

This file records the reasoning behind the choice; it is not a legal opinion, and
none of the analysis above was reviewed by a lawyer. Relicensing is
hard to undo once other people have copies, so if hydra acquires outside
contributors, users with contractual obligations, or an institutional owner with
an IP policy, get the arrangement checked by someone qualified.
