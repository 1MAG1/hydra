# Frozen ABI baselines

`abi-1.manifest` is the layout of ABI version 1, as published: every
enumerator's value, every field's offset and width, every struct's size, and
every exported symbol. It is not documentation of the current header — it is
the promise the current header has to keep.

```text
abi 1
enum hydra_error_code_t.HYDRA_ERR_AGAIN 5
field hydra_engine_config_t.state_path 56 8
struct hydra_event_t 120
symbol hydra_job_create
```

`scripts/ffi-abi-compat.sh` derives the same facts from `include/hydra.h` — by
generating a C program that prints its own `sizeof` and `offsetof`, so the
answers come from a compiler rather than from a parser — and enforces the rules
in [`docs/ffi/ABI.md`](../../../docs/ffi/ABI.md) against this file.

It compares rather than diffs, and the asymmetry is the point: everything here
must still be true, and the header may say more. Appending an enumerator, a
function, or a field to one of the two size-prefixed configuration structs
passes. Moving a field, renumbering an enumerator, widening a type, dropping a
symbol or growing any other struct fails.

## Changing this file

`scripts/ffi-abi-compat.sh --update` rewrites it. There are exactly two reasons
to run that:

1. **An addition the contract permits.** The manifest gains lines; no existing
   line changes. If running `--update` changes or removes a line that was
   already here, the change was not an addition and this is not the fix.
2. **A new ABI version.** `HYDRA_FFI_ABI_VERSION` becomes 2, and the result is
   committed as `abi-2.manifest` alongside this one. ABI 1's baseline stays,
   so what ABI 1 promised remains inspectable after ABI 2 exists.

It is never the way to make a failing check pass. A failure here means a
program somebody compiled against a published header would read the wrong bytes
out of a struct at run time, and the correct response is either to undo the
change or to bump the ABI version.
