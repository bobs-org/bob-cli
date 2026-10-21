---
title: Update the Rust toolchain
description: Keep the pinned compiler, package minimum, and compatibility reference in sync.
---

# Update the Rust toolchain

Change the compiler floor as one coordinated update so local builds and release CI use
the same supported Rust version.

## Before you start

Choose the new stable toolchain version and check its availability on both Linux and
macOS GitHub runners.

## 1. Update the pin and package floor

Update rust-toolchain.toml and the rust-version field in Cargo.toml together. Keep the
toolchain's rustfmt and clippy components enabled.

## 2. Update the compatibility reference

Change the Rust minimum in docs/reference/compatibility.md to match the new package
minimum. The compatibility page describes the minimum supported version; it should not
drift from Cargo metadata.

## 3. Verify on a scratch branch

Push the change on a ci/** branch and wait for both Linux and macOS test jobs, lint, and
package checks. Read [Test release workflow changes](test-release-workflow-changes.md)
for scratch-branch cleanup.

## If it goes wrong

If a runner does not have the selected stable toolchain or component, choose a
published stable release supported by both operating systems and update the three
locations together.

## Related

- [bob-cli CI](../ci.md)
- [Requirements and compatibility](../../reference/compatibility.md)
- [Test release workflow changes](test-release-workflow-changes.md)
