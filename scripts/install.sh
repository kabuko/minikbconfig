#!/usr/bin/env bash
#
# Install minikbconfig, the TUI, and the udev rule.
#
# Everything here is idempotent: run it again after a pull to update.
#
#   ./scripts/install.sh              install binaries, prompt for the udev rule
#   ./scripts/install.sh --no-udev    binaries only, no privilege prompts
#   ./scripts/install.sh --help
#
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
rule_src="$repo_root/udev/99-minikbconfig.rules"
rule_dst="/etc/udev/rules.d/99-minikbconfig.rules"

do_udev=1
for arg in "$@"; do
    case "$arg" in
        --no-udev) do_udev=0 ;;
        -h|--help) sed -n '3,10p' "${BASH_SOURCE[0]}" | sed 's/^#\{1\} \{0,1\}//'; exit 0 ;;
        *) echo "unknown option '$arg'; try --help" >&2; exit 2 ;;
    esac
done

say() { printf '\n== %s\n' "$1"; }

say "Building and installing binaries"
# Both binaries. `cargo install --path` installs one crate at a time, so
# forgetting the TUI is easy - which is exactly what had happened.
cargo install --path "$repo_root/crates/minikbconfig" --force --quiet
cargo install --path "$repo_root/crates/minikbconfig-tui" --force --quiet

for bin in minikbconfig minikbconfig-tui; do
    if command -v "$bin" >/dev/null 2>&1; then
        printf '   %-18s %s\n' "$bin" "$(command -v "$bin")"
    else
        printf '   %-18s NOT ON PATH - is ~/.cargo/bin in your PATH?\n' "$bin"
    fi
done

if [ "$do_udev" -eq 0 ]; then
    say "Skipping the udev rule (--no-udev)"
    echo "   The pad will stay root-owned. Either run this without --no-udev,"
    echo "   or use sudo when running minikbconfig."
    exit 0
fi

say "Installing the udev rule"
if [ ! -f "$rule_src" ]; then
    echo "   error: $rule_src not found; is this the repository root?" >&2
    exit 1
fi
# Skip if identical, so re-running does not need sudo for nothing.
if [ -f "$rule_dst" ] && cmp -s "$rule_src" "$rule_dst"; then
    echo "   $rule_dst is already up to date"
else
    echo "   This needs sudo, and will ask for your password."
    sudo install -m 0644 "$rule_src" "$rule_dst"
    sudo udevadm control --reload-rules
    sudo udevadm trigger
    echo "   installed and reloaded"
fi

say "Replug the pad"
echo "   A pad that was already attached keeps its old permissions until it is"
echo "   unplugged and plugged back in. This is the step that usually looks like"
echo "   the rule did not take effect."

say "Done"
cat <<'EOF'
   Verify with:   minikbconfig info
   Then:          minikbconfig snapshot pad.json
                  minikbconfig apply pad.json --dry-run

   If `info` reports the pad as forbidden rather than missing, you are not yet
   in the plugdev group. Group membership is read at login, so log out and back
   in - that is the step people miss. Until then, `sudo minikbconfig` works.
EOF