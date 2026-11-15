The real-document harness inventories 32 operation families and implements 23
bounded workflows. Tool availability is distinct from execution. Nine families
still lack workloads or a usable transport, and the manifest retains the
unmeasured parts of the two older combined hierarchy/variable families.

Use a verified completed build manifest, immutable complete document inventory
and matching font-environment contract. Every case opens a fresh complete copy;
the harness never substitutes an empty board or edits the original fixture.
The one-client prerequisite must pass before offering larger counts. Run through
the shared resource guard appropriate to the measurement; this script does not
acquire that lease itself.

Existing calls retain a global 64-mutation reservation by default. To offer all
writers in each selected family, explicitly select independent family budgets:

```sh
python3 scripts/bench/real-document-operations.py \
  --build-manifest "$BENCH_BUILD" --engine-commit "$BENCH_COMMIT" \
  --server-sha "$BENCH_SERVER_SHA" --fixture "$BENCH_FIXTURE" \
  --font-environment "$BENCH_FONTS" --output "$BENCH_FRESH_OUTPUT" \
  --clients 1 --max-inflight 32 --timeout 90 --max-bytes 1000000000 \
  --family reparent reorder group-ungroup group-ungroup-nested align \
  --write-budget-scope family --render-budget 0 \
  --managed-primary-save-before-ack
```

After a successful prerequisite, separate fresh cases can offer `--clients 10`,
`100` or `1000`. Family mode reserves `clients × workflow mutations` independently;
`--family-write-budget N` caps each family instead. Global mode uses
`--write-budget N`. Mixing the two cap flags is rejected. A whole workflow must
fit its reservation; spare requests do not become partially scheduled writers.
Deadlines, available distinct original targets and refusals can prevent completion.

| New family | Mutation requests per complete writer | Control and remaining limits |
| --- | ---: | --- |
| `reparent` | 13 | Owned copies of actual imported nodes; explicit parent/index/order checks |
| `reorder` | 10 | Up/down inside an owned sibling array |
| `group-ungroup` | 8 | Root sibling round trip |
| `group-ungroup-nested` | 10 | Same-parent nested round trip; no arbitrary authored transform/effect flattening claim |
| `align` | 10 | Left/top only; other align/distribute modes remain unmeasured |
| `variables` | 24 | Four scalar kinds, exact signed zero and escaped Unicode; no reference binding/bulk replacement claim |
| `themes` | 2 | Own-axis merge and undo; no active-axis resolution/preset claim |

Hierarchy workflows create per-agent copies of actual imported targets and remove
only their owned copies/frames/groups. Metadata workflows operate on the actual
full document, with distinct owned variable/axis names. Variables and themes
serialize each complete workflow because undo history is shared. Own immediate
undo restores the original optional maps; the final verifier checks absent versus
empty fields. An uncertain mutation stops the workflow; cleanup never replays it.

Reports distinguish scheduled, started, accepted and rejected workflows; actual
mutation requests and unique mutation agents; cancellation and unconfirmed calls;
logical clients and peak active requests. A scheduled 1,000 writers is not a
1,000-writer result. Refusals remain `measured-with-refusals`, and a whole case
with refusals is not marked passed. A failed workflow cancels queued work while
retaining every in-flight request's uncertainty journal.

The full-document verifier retains every field/array and numeric f64 bit, with
only the documented accepted actor's saved active-page metadata change modeled.
Object keys are normalized, so raw bytes/map order are unverified. Fonts, PNG
dimensions and asset closure do not establish arbitrary Figma pixel parity.

Checkpoint 14's original 27-family results remain frozen history. Five new
hierarchy controls passed on full MUI at one client in a separately recorded
private calibration. Variables/themes and higher-count new-family runs remain
unmeasured until released product runs complete. Admission-only Python tests are
not product throughput evidence:

```sh
python3 -m unittest discover -s scripts/bench -p test_real_document_plan.py
```
