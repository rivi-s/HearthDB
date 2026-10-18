# HearthDB 0.4.1-exp.3

Experimental fork: https://github.com/rivi-s/HearthDB
Upstream: https://github.com/copypasteonly/HearthDB

## Changes

- Convert asynchronous native query results into Lua tables incrementally
  across frames, using row/cell and elapsed-time budgets.
- Release converted native row contents as delivery progresses.
- Dispatch ready callbacks in stable ticket order.
- Retain bounded, in-memory query phase and delivery timings. No raw SQL text
  or quest titles are included in those diagnostic records.

Callbacks retain the existing API and receive a complete result once delivery
finishes. Cancellation, reset, errors, Lua registry references, and callback
reentry are covered by the included modeled-Lua tests. A single Lua allocation
or garbage collection can exceed the best-effort time budget.

## Compatibility

The numeric HDB_GetVersion API remains 0.4.1. Diagnostic delivery stats identify
the experimental build. pfQuest-HDB 0.2.0-beta.5 supports both this DLL and the
existing upstream DLL; this experiment is optional.

## Install and roll back

1. Fully exit the game. A /reload does not replace a loaded DLL.
2. Back up your existing mods/HearthDB.dll outside the mods directory.
3. Extract this package so mods/HearthDB.dll replaces the existing file.
4. Restart the game and verify pfQuest-HDB with /pfqhdb.

To roll back, fully exit, restore the backed-up DLL, then restart. Keep only
one HearthDB DLL in mods. Settings and SQLite databases do not need replacing.

## Diagnostics

HDB_DumpDeliveryDiagnostics() writes Logs/hdb_delivery_exp3.log on demand.
There is no automatic log-writing loop in the DLL. A separate development
profiler may explicitly request a dump; that addon is not shipped here.

## Validation

The included tests execute the actual delivery and trace modules against a
modeled Lua stack, including 11,153-row delivery, cancellation, reset, reentry,
error handling, registry cleanup, and bounded diagnostic retention.

Run the host tests with:

```sh
CARGO_PKG_VERSION=0.4.1-exp.3 rustc --edition=2021 --test tests/delivery_mock.rs -o /tmp/hdb-delivery-tests
/tmp/hdb-delivery-tests
```

Build the Windows i686 DLL using the repository Makefile and its documented
Rust/LLVM/xwin SDK prerequisites. Live pfQuest-HDB login tests exercised this
build, including dense quest results and route/map readiness. Wider gameplay
and quest-completion testing remains desirable before a stable release.

## License and source

The upstream GNU GPL version 3 license is retained in LICENSE. Release assets
include the corresponding source archive and identify its commit. No new
license or upstream attribution is substituted.
