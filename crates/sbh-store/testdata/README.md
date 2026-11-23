# Legacy Dream memory database

`dream-memory-rusqlite-0.32.1.sqlite` was generated from SBH master
`9400d55` using rusqlite 0.32.1 / libsqlite3-sys 0.30.1 (bundled
SQLite 3.46.0). It contains only synthetic data from the existing unit-test
helpers: `seed(&store, "m")`, `evidence(&store)`, then
`apply(&store, "a", promotion("m", 0, "a", "evidence-run"))`.
The generator opened the file through `MemoryStore::open` with the tests'
`Verifier` and closed it normally. Schema version is 1.

The compatibility test copies this immutable old-format fixture before opening
it with the current library, checking history and validated selection.

SHA-256: `36b3b7be14e212d419bdbd619b0302e19120eec8049d6b49e80b22a3af7b1265`
