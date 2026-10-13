### Changed

- **The family's scripts run in place from rust-fs-core, and this repository
  keeps no copy.** `scripts/core.sh` and `scripts/tier.sh` are gone; CI and
  chores run `../rust-fs-core/scripts/NAME.sh` at the pinned version
  (rust-fs-core 0.3.2, #212).
- **A release's notes are its CHANGELOG section.** The release workflow
  takes the GitHub release's body from `scripts/core.sh release-notes` and
  refuses a tag the CHANGELOG does not describe, before anything is
  published (rust-fs-core#209). It depends on rust-fs-core 0.3.1.
