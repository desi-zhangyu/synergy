# Decision Record: Prepare controlled temporary directories on the execution host

Status: implemented

## Problem

A remote sandbox policy can name a session temporary child before that directory exists on its physical host. Creating only the shared temporary parent leaves bubblewrap unable to bind the session write root, so the user command never starts.

## Decision

The Linux helper prepares its controlled parent and the direct session child selected by `TMPDIR` immediately before building the sandbox plan. It runs under the execution user after durable Workspace admission. Directory-relative `mkdirat` and `openat` with `O_NOFOLLOW` prevent preparation from following workspace symlinks. External or nested temporary paths are not created. The existing policy, syscall filters and user identity remain authoritative.

## Alternatives considered

Creating the directory in the calling Agent cannot prepare a remote filesystem. Creating it while compiling a sandbox would mutate a Workspace before writer admission. Removing the session write root or disabling containment would hide the initialization defect and change the execution policy.

## Consequences

Native and remote Linux commands share one initialization path. Regression tests cover first use, repeated use, external paths and each symlink component; the Docker boundary uses a missing session directory and writes a SQLite database through Python temporary-file APIs while retaining external-write, credential-read and network denial assertions.
