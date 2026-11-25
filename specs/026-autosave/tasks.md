# Tasks: Autosave and crash recovery

Plan: [plan.md](plan.md)

- [x] T1. Backup on a timer, written on its own thread; removed when nothing is unsaved and at quit — verify: unit test A1
- [x] T2. Recovery at launch, taking the backup over — verify: unit tests A2
- [x] T3. Take marker and header repair — verify: unit tests A3
- [x] T4. Benchmark of the write and the clone — verify: `cargo bench -p dstudio-core --bench save`
- [x] T5. Docs: README Project files, roadmap S0.3 — verify: read them against the change
