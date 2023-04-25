#!/bin/sh

set -e

cargo clean
exec cargo lockbud "$@"
