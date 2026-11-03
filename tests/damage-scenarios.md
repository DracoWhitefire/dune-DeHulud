# Damage scenario contract

`damage_scenarios.json` is the shared Python/Rust input. Every scenario starts a
fresh model: own health 100/100, enemy health 100/100 and shield 50, aim at enemy,
no soft lock, image/world 1, all sections enabled. Frames are applied atomically
before one poll at `at_ms`; omitted state persists except `fail_reads`, which is
cleared each frame. `expected` contains numbers emitted in that interval, not
the accumulated visible history. Drops from multiple targets in one poll merge
into one number per channel. Display ordering is tested separately.
Switch state persists. `no_reads` names routes with zero attempted reads, including
failed reads. `DamageModel.feature_ranges()` is the single address-range
definition, returned in initial metadata, `damage-info`, and successful frame
acknowledgements. `LoggedReader.assert_no_reads(disabled, feature_ranges)` rejects
every logged access overlapping an exclusive range, regardless of route label.
The Rust adapter must use the same overlap rule and returned ranges. Labels are
diagnostics only. Shared identity and controller/pawn reads stay allowed.
Worm ranges cover value/threshold fields in existing threat components; dealt
ranges cover object/chunk tables, chunks, non-own objects and aim/soft-lock fields;
taken ranges cover own vitals.

The arena's default offsets are explicitly synthetic. They are not assertions
about any supported build. `--build 26333948 --damage` selects `BoundDamageModel`
in `damage_build.py`: actual GWorld/controller/pawn hops are shared with the threat
arena; attribute-set health follows a payload pointer and maximum/power are
inline; weak handles are on the own character and resolve through the image's
chunk table and count slots. Other builds retain the synthetic model. Expected
damage numbers are independent of offsets.

`fake_game.py --damage` adds metadata under `damage` in its first JSON line.
`damage <JSON frame>` acknowledges with JSON containing `damage` metadata,
including `failure_ranges` and `feature_ranges`. Python uses
`FakeGame.damage_frame(frame, logged_reader)` to install the returned failure
ranges in the parent reader. Rust uses `Fake::damage_frame` and must install the
returned ranges in its consumer hook. A successful frame without `fail_reads`
clears failure ranges. Plain `FakeGame.cmd` rejects `fail_reads` to prevent
discarding failure metadata; Rust's plain `cmd` rejects all damage frames.
`damage-info` returns current metadata. Synthetic-mode threat objects and commands
retain their addresses and meaning. Bound mode uses 0x3000-byte object spacing
and gives the vehicle its native vehicle class; pointer commands retain their
meaning, with addresses supplied in the initial metadata.

In the bound model, `world` swaps the actual GWorld pointer to a typed world with
the same game instance. A changed `image` token maps a new image with the same
supported PE identity and invalidates the old header. Its acknowledgement returns
`image_base`: Python's game helper updates its source maps; Rust's fake helper
updates its base and the adapter must refresh control-file base before stepping.
The reader must actually reattach; metadata tokens alone never reset history.
Synthetic transport world/image keys remain rejected before any mutation.
`items` can replace table entries by index, object name (or null), serial and
flags; `spawn` also accepts flags and class depth. This exposes each weak-pointer
guard without changing the scenario expectations.

Reader instrumentation belongs at the consumer's memory-read boundary. External
`process_vm_readv` calls are invisible to the fake process. Python's `LoggedReader`
wraps either the arena or the actual process reader, tags attempted reads with
`route`, and injects failures by overlapping address range. Rust needs the same
address/size/route trace and failure hook in its test source. Both adapters check
disabled product routes by address overlap, including failed read attempts.

## Running the scenarios

Both applications now run all shared detector scenarios:
steady hits, killing blow during role change, healing mixed with a hit, target
swap, serial reuse, vehicle target, read failure, world/image changes, 499/500 ms
gap boundary, maximum change, death/respawn, each feature off, and all off.
Python's `test_damage_scenarios.py` exercises the public Meter with DamageReader
and a real child-process reader. Rust's `damage_scenarios.rs` exercises the built
binary with the `test-source` feature. The ordinary Rust suite still checks the
fixture schema; release feature tests run every scenario. No detector
implementation is duplicated in the adapters.

Python adapter: create the arena and real reader, apply a frame, update identity,
time and switches, inject failures, poll once, compare emitted numbers and the
route read log. Rust adapter: consume the same JSON through `dehulud::json`, drive
`Run`/`Fake::damage_frame`, install the returned failure/range metadata, and
compare the poll trace after exactly one frame. `Run::damage_step` writes an
atomic control update with damage_step, at_ms, enabled flags, current image base
and replacement failure ranges. The binary polls once for each distinct step;
read and poll events carry that step. Ordinary `Run::poll` remains available for
the older free-running tests. Both adapters reset every changed feature as a
header click would, including transitions to all-off. All-off additionally asserts
that the entire attempted-read log is empty.

Follow-up cases for the detector suite: bounded class walk at depths 4/5, invalid
health/max/shield channels (NaN, infinity, negative and health above maximum),
independent channel resets, recent-target expiry at 18 seconds, four-target
eviction, deduplication across roles and the six-target ceiling, switch re-enable
baseline, own-character vehicle fallback, and fading/rounding display behavior.
The own-death frame expects the final positive-to-zero drop once, followed by a
cleared baseline; this and per-channel merging are confirmed behavior.
