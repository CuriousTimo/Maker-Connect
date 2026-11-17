#!/bin/sh
# zig as the archiver for check.sh's Windows clippy.
exec zig ar "$@"
