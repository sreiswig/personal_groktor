# CI flakes: infra vs. real failures

## 1. Zero steps + no runner = infra, not a test failure

If a job shows **cancelled** or **failed** and:

- no steps ran (the job page has no step list / logs), and
- no runner was assigned (`runner_name` is empty, `runner_id` is 0 in the
  job API), often with an annotation like
  *"The job was not acquired by Runner of type hosted even after multiple
  attempts"*,

then GitHub never gave the job a hosted runner. Nothing in this repo ran, so
it says nothing about the code. Example: on 2026-10-05 the `sbom` job on
`main` (run 37362689203) and the `test` job on PR #28 (run 37362846208) each
sat queued for ~15 minutes and were cancelled by GitHub with zero steps.

## 2. Habit: re-run failed jobs once before investigating

For a zero-step / no-runner failure, re-run once before digging in:

- Actions UI: open the run → **Re-run jobs** → **Re-run failed jobs**, or
- CLI: `gh run rerun <run-id> --failed`

If the re-run goes green, you're done. If it fails again *with steps that
ran*, that's a real failure: read the failing step's log. If it keeps dying
with zero steps, check <https://www.githubstatus.com/> (Actions).

We deliberately do **not** auto-retry: that would need a workflow with
`actions: write` or a PAT, which we avoid.

## 3. Why jobs have `timeout-minutes`

Each workflow job sets a job-level `timeout-minutes` sized to its normal
runtime plus headroom (`test`: 20, `sbom`: 15; both normally finish in
under 2 minutes). Without it a hung step can burn up to GitHub's 360-minute
default.

`timeout-minutes` only starts counting once a runner has picked the job up
and it is running. It caps hung **steps**. It does **not** bound time spent
queued waiting for a runner, so it will not prevent or shorten the
queue cancellations described above.
