# Changelog

All notable changes to Bob are documented here.

## [0.3.1](https://github.com/bobs-org/bob-cli/compare/v0.3.0...v0.3.1) - 2026-10-23

### Added

- *(managed-log)* register Rotten Log kind with agenda and snapshot support
- *(ref)* expose annotation group pieces with seams in show
- *(highlights-ref)* render continuation pieces as tucked groups

### Fixed

- *(highlights-ref)* harden legacy migration coverage and grouping docs

## [0.3.0](https://github.com/bobs-org/bob-cli/compare/v0.2.0...v0.3.0) - 2026-10-22

### Added

- *(capture-related)* restore --exclude calibration pieces and regen docs
- *(capture-related)* calibrate similar-task bands from reviewed labels
- *(highlights-ref)* stabilize highlight IDs across sidecar order
- *(relations)* add pair planner, projection, and redundant warnings
- *(capture)* attach relation labels on typed pipe and expose relation in completions

### Breaking

- *(stats)* [**breaking**] make the net flow headline the window total so it matches its label

### Fixed

- *(ref)* launch discovered bundled Chromium for Mermaid

### Reverted

- *(relations)* back out bob-cli-am pair contract and planner

## [0.2.0] - 2026-10-20

### Added

- First versioned release of Bob and its companion command-line binaries.
