#!/bin/bash
set -e

REPO="baler-dev/baler"
DEFAULT_VERSION="__VERSION__"

bin_dir="$HOME/.local/bin"
mkdir -p "$bin_dir"

# Everything is downloaded and checked here first, and nothing reaches
# bin_dir unless the checksum matches. The directory is removed on exit,
# whether the install worked or not.
work_dir=$(mktemp -d)
trap 'rm -rf "$work_dir"' EXIT

os=$(uname -s | tr '[:upper:]' '[:lower:]')
arch=$(uname -m)
case "$arch" in
  arm64) arch="aarch64" ;;
  amd64) arch="x86_64" ;;
esac

case "$os" in
  darwin) os_pattern="apple-darwin"; ext="tar.gz" ;;
  linux)  os_pattern="unknown-linux-gnu"; ext="tar.gz" ;;
  mingw*|msys*|cygwin*)
    echo "Detected Windows (via Git Bash/MSYS)."
    os_pattern="pc-windows-msvc"
    ext="zip"
    if ! command -v unzip >/dev/null 2>&1; then
      echo "Error: 'unzip' not found in this Git Bash environment." >&2
      exit 1
    fi
    ;;
  *) echo "Unsupported OS: $os" >&2; exit 1 ;;
esac

# A release renders DEFAULT_VERSION to its tag (v0.1.0). Anything else,
# such as the untouched placeholder, means "no pinned version".
version="${BALER_VERSION:-}"
case "$DEFAULT_VERSION" in
  v[0-9]*) [ -n "$version" ] || version="$DEFAULT_VERSION" ;;
esac

if [ -n "$version" ]; then
    echo "Fetching release $version..."
    api_url="https://api.github.com/repos/$REPO/releases/tags/$version"
else
    echo "Fetching latest release..."
    api_url="https://api.github.com/repos/$REPO/releases/latest"
fi

echo "Fetching download URL for $arch-$os_pattern..."
github_response=$(curl -fsSL "$api_url") || {
    echo "Error: could not read $api_url" >&2
    exit 1
}
asset_url=$(echo "$github_response" | grep -o "https://github.com/$REPO/releases/download/[^\"]*$arch-$os_pattern.$ext\"" | head -n 1)
asset_url="${asset_url%\"}"

if [ -z "$asset_url" ]; then
    echo "Error: no release asset found for $arch-$os_pattern in $REPO ${version:-(latest)}." >&2
    exit 1
fi

archive_file="$work_dir/baler.$ext"
checksum_file="$work_dir/baler.$ext.sha256"

echo "Downloading baler from $asset_url"
curl -fSL -o "$archive_file" "$asset_url" || {
    echo "Error: could not download $asset_url" >&2
    exit 1
}

# Every release publishes a .sha256 file next to each archive, in the
# format `sha256sum` writes: the hash, then the file name.
curl -fsSL -o "$checksum_file" "$asset_url.sha256" || {
    echo "Error: could not download $asset_url.sha256, so the download cannot be verified. Nothing was installed." >&2
    exit 1
}
expected_hash=$(awk '{ print tolower($1); exit }' "$checksum_file")
case "$expected_hash" in
    *[!0-9a-f]*|"")
        echo "Error: the checksum file for $asset_url is not valid. Nothing was installed." >&2
        exit 1
        ;;
esac
if [ "${#expected_hash}" -ne 64 ]; then
    echo "Error: the checksum file for $asset_url is not valid. Nothing was installed." >&2
    exit 1
fi

if command -v sha256sum >/dev/null 2>&1; then
    actual_hash=$(sha256sum "$archive_file" | awk '{ print tolower($1) }')
elif command -v shasum >/dev/null 2>&1; then
    actual_hash=$(shasum -a 256 "$archive_file" | awk '{ print tolower($1) }')
else
    echo "Error: neither 'sha256sum' nor 'shasum' was found, so the download cannot be verified. Nothing was installed." >&2
    exit 1
fi

if [ "$actual_hash" != "$expected_hash" ]; then
    echo "Error: checksum mismatch for $asset_url" >&2
    echo "  expected: $expected_hash" >&2
    echo "  actual:   $actual_hash" >&2
    echo "Nothing was installed." >&2
    exit 1
fi
echo "Checksum verified."

if [ "$ext" = "zip" ]; then
    unzip -o "$archive_file" -d "$work_dir/unpacked" >/dev/null
    unpacked_binary="$work_dir/unpacked/baler.exe"
else
    mkdir -p "$work_dir/unpacked"
    tar -xzf "$archive_file" -C "$work_dir/unpacked" --strip-components=1
    unpacked_binary="$work_dir/unpacked/baler"
fi

if [ ! -f "$unpacked_binary" ]; then
    echo "Error: the archive does not contain the baler binary. Nothing was installed." >&2
    exit 1
fi

mv -f "$unpacked_binary" "$bin_dir/baler"
chmod +x "$bin_dir/baler" 2>/dev/null || true
echo "baler installed to ~/.local/bin"

if [[ ":$PATH:" != *":$HOME/.local/bin:"* ]]; then
    if [[ "$SHELL" == *"bash"* ]]; then
        printf '\n%s\n' 'export PATH="$HOME/.local/bin:$PATH"' >> ~/.bashrc
    elif [[ "$SHELL" == *"zsh"* ]]; then
        printf '\n%s\n' 'export PATH="$HOME/.local/bin:$PATH"' >> ~/.zshrc
    elif [[ "$SHELL" == *"fish"* ]]; then
        printf '\n%s\n' 'fish_add_path "$HOME/.local/bin"' >> ~/.config/fish/config.fish
    fi
    echo "Please restart your terminal, or source your shell config."
else
    echo "~/.local/bin is already in your PATH."
fi
