#!/bin/sh
# zig as the C compiler for x86_64-pc-windows-gnu (check.sh's Windows clippy: `ring` builds C).
# The `cc` crate passes its own --target, which zig does not understand: it is left out.
args=""
for a in "$@"; do
  case "$a" in --target=*|-target=*) ;; *) args="$args '$(printf %s "$a" | sed "s/'/'\\\\''/g")'";; esac
done
eval exec zig cc -target x86_64-windows-gnu $args
