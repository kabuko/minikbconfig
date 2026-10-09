# Snapshots of a pad's configuration, one file per board.
#
# Committed on purpose: a snapshot is the only restore path that is guaranteed to
# match the board it came from. tools/restore_device.py hardcodes the 3-key
# board's factory bindings and would overwrite the 6-key board's defaults with
# the wrong model's.
#
# Take one with:  minikbconfig snapshot snapshots/<name>
# Restore with:  minikbconfig apply snapshots/<name> --dry-run
#
# Each file records the vendor/product ID it was captured from, so apply can warn
# if the pad in hand is a different model.