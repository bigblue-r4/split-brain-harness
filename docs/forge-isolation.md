# Forge isolation and lifecycle

The real compiler/executor now require Linux. Operators must provision
bubblewrap (`/usr/bin/bwrap`), util-linux prlimit (`/usr/bin/prlimit`),
an installed Rust WASI target, and wasmtime on a trusted PATH.
The application never installs these tools. Unsupported platforms, disabled user
namespaces, missing tools or sandbox errors fail the operation without an
unrestricted fallback. Existing mock compiler/executor interfaces remain usable
for unit tests; custom executors must honor `execute_limited` constraints.

Compilation clears inherited environment and runs with isolated namespaces,
no network, no capabilities, a read-only toolchain/system-library view, an
otherwise empty filesystem, and one private writable workdir. Generated code
cannot read the service's home/configuration or host environment at compile time.
Static macro screening is defense in depth; it is not the isolation boundary.
The toolchain directory and system libraries are trusted operator installations.
Do not put credentials in them or grant generated code extra mounts.

Compiler limits: 60 seconds wall time, 60 CPU seconds, 2 GiB virtual address
space, 16 MiB artifact/file ceiling, 128 descriptors, no core dumps, bounded
8 KiB stdout/stderr streams, and a 1 MiB source ceiling. The workspace uses
random exclusive 0700 directories and exclusive 0600 source/artifact staging.

Runtime uses the request's time and linear-memory limits (maximum 10 seconds and
256 MiB), one memory/table, at most 10000 table elements, a finite 16 GiB host
virtual-address limit, and bounded 64 KiB stdout/stderr and 1 MiB stdin.
The host address limit accommodates Wasmtime's virtual memory reservations; it
is not a 16 GiB physical-memory reservation or a dedicated cgroup quota.
No WASI filesystem preopens, socket permissions or inherited guest environment
are supplied. Unknown CLI flags fail closed; operators must validate the selected
Wasmtime version before deployment. See the [official Wasmtime CLI option
reference](https://docs.wasmtime.dev/cli-options.html) and [Wasm options](https://docs.wasmtime.dev/api/wasmtime_cli_flags/struct.WasmOptions.html).

Runtime explicitly disables compiled-module caching and parallel compilation,
and confines HOME/XDG_CACHE_HOME to the disposable workdir. This avoids default
user-cache writes and bounds compilation concurrency.

The supervisor multiplexes nonblocking stdin/stdout/stderr under a common
deadline, including EOF after the leader exits. It creates no I/O worker threads.
It terminates/reaps process groups before cleanup; compiler PID namespaces add
containment for descendants that create their own sessions inside the sandbox.
A detached pipe holder outside those namespaces can no longer prolong a call or
produce a late success response. The Unix implementation uses the already-locked
libc package directly for fcntl/kill, with documented descriptor/PID ownership.
Compiler and runtime cleanup failures propagate explicitly and cannot admit a
successful capability memory entry. `destroyed` reports on-disk artifact cleanup,
not secure erasure from RAM, swap, backups or storage media. A failed cleanup
report requires operator inspection of the named `sbh-forge-*` scratch directory;
do not create an executable reuse pool or sweep unrelated `/tmp` entries.

## Validation

Default workspace tests cover output flooding, blocked stdin/process descendants,
a detached pipe holder retaining output after leader exit, exclusive file
creation, cleanup failure reporting and request-limit propagation. Three external
integration tests are explicitly ignored by default: native namespace canaries,
an unprivileged permission-denied cleanup fixture, and actual WASI runtime cases.
Run them in a prepared Linux environment:

```
cargo test --locked -p sbh-forge --lib -- --ignored --nocapture
```

When validating with a separate Rust toolchain copy, Cargo may prepend its own
host toolchain to PATH. Build with `--no-run`, then execute the emitted test binary
directly with the isolated rustc/wasmtime directory first in PATH. The WASI suite
covers a normal run, compile-time and guest host-file/environment canaries,
infinite-loop timeout, memory over-allocation, output flooding, malformed modules
and absence of a persistent module cache. Tests use synthetic data only.

The suite passed locally with Rust 1.99.1 / wasm32-wasip1 and Wasmtime 50.0.3.
Revalidate the selected runtime/toolchain on the target deployment platform;
these local checks are not independent approval or staging validation. Existing
callers matching outcome enums must handle `CleanupFailed`. No wire-data
migration is required.
