#!/bin/sh
# Uplink preflight. Build and test the product here; a non-zero exit fails
# preflight. See toolchain-hook.md on this branch.
#
# - Run as `sh preflight.sh` from the root of the tree under test: the export
#   tree (public upstream plus the change), or the current checkout for
#   `git uplink preflight --command-only`.
# - This branch is checked out beside the script. Reach its other files with
#   "$(dirname "$0")".
# - GITHUB_TOKEN, GH_TOKEN, and UPLINK_*_TOKEN / UPLINK_*_KEY are not set.
# - In the forge pack it runs in a job with a read-only token and no other
#   credential, never in the job that writes.
set -eu
