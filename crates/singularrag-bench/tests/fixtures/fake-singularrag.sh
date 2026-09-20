#!/bin/sh
# Stand-in for `singularrag` in tests: answers --version, records an `index` call.
if [ "$1" = "--version" ]; then echo "singularrag 0.1.0-fake"; exit 0; fi
if [ "$1" = "index" ] && [ -n "$FAKE_SINGULARRAG_MARK" ]; then echo "$@" > "$FAKE_SINGULARRAG_MARK"; fi
exit 0
