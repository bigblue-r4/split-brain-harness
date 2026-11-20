# Forge isolation and lifecycle

The real compiler/executor now require Linux. Operators must provision
bubblewrap (`/usr/bin/bwrap`), util-linux prlimit (`/usr/bin/prlimit`),
`/bin/kill`, an installed Rust WASI target, and wasmtime on a trusted PATH.
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

The supervisor drains output with bounded readers and writes stdin concurrently
under a common deadline. It terminates/reaps process groups before cleanup.
Compiler and runtime cleanup failures propagate explicitly and cannot admit a
successful capability memory entry. `destroyed` reports on-disk artifact cleanup,
not secure erasure from RAM, swap, backups or storage media. A failed cleanup
report requires operator inspection of the named `sbh-forge-*` scratch directory;
do not create an executable reuse pool or sweep unrelated `/tmp` entries.

## Validation

Default workspace tests cover output flooding, blocked stdin/process descendants,
exclusive file creation, truthful cleanup failure reports and request-limit
propagation. The namespace integration test requires external tools and enabled
user namespaces, so it is explicitly ignored in the default suite. Run it in a
prepared Linux environment:

```
cargo test --locked -p sbh-forge process_boundary::tests::sandbox_is_empty_and_does_not_inherit_host_markers -- --ignored --exact
```

This integration check compiles ordinary native-target metadata and rejects only
synthetic environment/file canaries. It does not install a WASI target, prove an
end-to-end Wasmtime run, or replace adversarial runtime tests. Before release,
exercise a supported WASI target and Wasmtime build, including timeout, memory
and malformed-module tests. Existing callers matching outcome enums must handle
the new `CleanupFailed` variants. No wire-data migration is required.
