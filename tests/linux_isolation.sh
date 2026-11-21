#!/bin/sh
# Run in a disposable Linux VM with writable cgroup v2 and unshare. No network.
set -eu
case "$(uname -s)" in Linux) ;; *) echo 'Linux required' >&2; exit 2;; esac
binary=${1:?usage: linux_isolation.sh /absolute/path/to/execscope}
case "$binary" in /*) ;; *) echo 'Absolute executable path required' >&2; exit 2;; esac
base=${EXECSCOPE_TEST_CGROUP_ROOT:-/sys/fs/cgroup}
[ -f "$base/cgroup.controllers" ] || { echo 'cgroup v2 required' >&2; exit 2; }
group="$base/execscope-fixture-$$"
mkdir "$group"
trap 'rmdir "$group"' EXIT
printf '50000 100000\n' > "$group/cpu.max"
printf '134217728\n' > "$group/memory.max"
# Expand $$ and the arguments in the guest shell, not this shell.
# shellcheck disable=SC2016
result=$(unshare --mount --pid --fork --mount-proc /bin/sh -ec '
  echo "$$" > "$1/cgroup.procs"
  exec "$2" inspect --scope all --detail full
' sh "$group" "$binary")
if command -v jq >/dev/null 2>&1; then
  printf '%s\n' "$result" | jq -e '.system.os == "linux" and .hardware.cgroup.hierarchy == "v2" and .hardware.effective_cpus == 0.5 and .hardware.effective_memory_bytes == 134217728 and (.active_network | not)' >/dev/null
else
  printf '%s\n' "$result" | grep -Eq '"effective_cpus"[[:space:]]*:[[:space:]]*0\.5([,[:space:]]|$)'
  printf '%s\n' "$result" | grep -Eq '"effective_memory_bytes"[[:space:]]*:[[:space:]]*134217728([,[:space:]]|$)'
  printf '%s\n' "$result" | grep -Eq '"hierarchy"[[:space:]]*:[[:space:]]*"v2"'
  if printf '%s\n' "$result" | grep -q '"active_network"'; then exit 1; fi
fi
printf '%s\n' 'Linux PID/mount namespace and cgroup v2: 0.5 CPU, 128 MiB memory, passive all scope.'
