home() {
    cd "$(git rev-parse --show-toplevel)" || exit 1
}

home

dev=false

usage() {
  if [[ -n $1 ]]; then
    echo "$*"
    echo
  fi
  cat <<EOF

usage: $0 [OPTIONS]

Bootstrap a validator to start a network.
Gossip host must be set to a private or public IP to communicate beyond localhost.

OPTIONS:
  --dev             - Symlink to local dependencies

EOF
  exit 1
}

positional_args=()
while [[ -n $1 ]]; do
  if [[ ${1:0:1} = - ]]; then
    if [[ $1 = --dev ]]; then
      dev=true
      shift 1
    elif [[ $1 = -h ]]; then
      usage "$@"
    else
      echo "Unknown argument: $1"
      exit 1
    fi
  else
    positional_args+=("$1")
    shift
  fi
done

if [[ $(uname -m) == "arm64" ]]; then
    echo "Running on Apple Silicon"
    rustup toolchain install 1.81.0-x86_64-apple-darwin || exit 1
    rustup override set 1.81.0-x86_64-apple-darwin || exit 1
else
    echo "Not running on Apple Silicon"
    rustup toolchain install 1.81.0 || exit 1
    rustup override set 1.81.0 || exit 1
fi

agave-install init 2.0.8

avm use 0.30.0

CXX=/opt/homebrew/bin/c++-14 cargo build || exit 1

cargo fmt || exit 1

cargo check || exit 1