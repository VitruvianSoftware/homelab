# CLA signatures

Storage branch for the CLA Assistant action configured in
`.github/workflows/cla.yml` on `main`. Signatures are committed here by the
action as `signatures/version1/cla.json`.

This branch is intentionally an **orphan**: it shares no history with `main`
and holds no project source.

It must exist and must **not** be protected, or the action fails with:

    Error occurred when creating the signed contributors file:
    Branch cla-signatures not found. Make sure the branch where signatures
    are stored is NOT protected.

That was the state until 2026-08-23 -- the branch had never been created, so no
external contributor could complete CLA signing. It stayed invisible because the
workflow only runs on pull requests, and this mirror receives few.