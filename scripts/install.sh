#!/bin/bash
set -e

REPO="baler-dev/baler"
DEFAULT_VERSION="__VERSION__"

mkdir -p ~/.local/bin && cd ~/.local/bin

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

echo "Downloading baler from $asset_url"
if [ "$ext" = "zip" ]; then
    curl -L -o baler_latest.zip "$asset_url"
    unzip -o baler_latest.zip -d .
    rm baler_latest.zip
    mv baler.exe baler 2>/dev/null || true
else
    curl -L -o baler_latest.tar.gz "$asset_url"
    tar -xzf baler_latest.tar.gz --strip-components=1
    rm baler_latest.tar.gz
fi
chmod +x baler* 2>/dev/null || true
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
