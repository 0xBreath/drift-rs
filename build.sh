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

#export CARGO_DRIFT_FFI_STATIC=1
# Provide a prebuilt drift_ffi_sys lib
#export CARGO_DRIFT_FFI_PATH="/target/release/libdrift_ffi_sys"

#CXX=/opt/homebrew/bin/c++-14 cargo build --release || exit 1
#cargo build -vv || exit 1
cargo build || exit 1
