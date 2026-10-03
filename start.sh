#!/bin/sh

cargo build --release --workspace
./target/release/spacetop --app=./target/release/spacelauncher