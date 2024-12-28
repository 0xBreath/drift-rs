#!/bin/bash

home() {
    cd "$(git rev-parse --show-toplevel)" || exit 1
}

home

if [[ $(uname -m) == "arm64" ]]; then
    echo "Running on Apple Silicon"
    rustup override set 1.81.0-x86_64-apple-darwin || exit 1
else
    echo "Not running on Apple Silicon"
    rustup override set 1.81.0 || exit 1
fi

agave-install init 2.0.8 || exit 1

avm use 0.30.0

export TEST_MAINNET_RPC_ENDPOINT="https://mainnet.helius-rpc.com/?api-key=0b810c4e-acb6-49a3-b2cd-90e671480ca8"
export GRPC="https://grpc.us.shyft.to"
export X_TOKEN="b8ad3c28-45ed-470b-91ed-8c2dd32f0859"
export CXX=/opt/homebrew/bin/c++-14

# Provide a prebuilt drift_ffi_sys lib
export CARGO_DRIFT_FFI_PATH="/usr/local/lib/libdrift_ffi_sys.dylib"
#export CARGO_DRIFT_FFI_STATIC=1

cargo build > build.log 2>&1
echo "Finished building"

cargo test -p drift-rs --lib geyser::dlob::tests::test_orderbook > test.log 2>&1
echo "Finished test"

CARGO_DRIFT_FFI_PATH="/usr/local/lib" cargo test -p drift-rs --lib geyser::dlob::tests::test_orderbook