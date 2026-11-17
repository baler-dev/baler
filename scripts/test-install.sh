#!/bin/bash
# Offline test for scripts/install.sh. It puts a fake `curl` (and, for the
# Windows case, a fake `uname`) first on PATH, so no request leaves the
# machine, and checks what the installer leaves in a throwaway $HOME.
#
#   bash scripts/test-install.sh

set -u

script_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
installer="$script_dir/install.sh"

root=$(mktemp -d)
trap 'rm -rf "$root"' EXIT

passed=0
failed=0

pass() {
    passed=$((passed + 1))
    echo "ok   - $1"
}

fail() {
    failed=$((failed + 1))
    echo "FAIL - $1"
    if [ -n "${2:-}" ]; then
        echo "       $2"
    fi
}

# ---- fixtures ----

arch=$(uname -m)
case "$arch" in
    arm64) arch="aarch64" ;;
    amd64) arch="x86_64" ;;
esac

tag="v9.9.9"
linux_name="baler-$tag-$arch-unknown-linux-gnu"
windows_name="baler-$tag-x86_64-pc-windows-msvc"
base_url="https://github.com/baler-dev/baler/releases/download/$tag"

fixtures="$root/fixtures"
mkdir -p "$fixtures/$linux_name" "$fixtures/windows"

printf '#!/bin/sh\necho "baler fake 9.9.9"\n' > "$fixtures/$linux_name/baler"
chmod +x "$fixtures/$linux_name/baler"
tar -czf "$fixtures/$linux_name.tar.gz" -C "$fixtures" "$linux_name"

printf 'fake windows binary\n' > "$fixtures/windows/baler.exe"
python3 - "$fixtures" "$windows_name" <<'PYTHON'
import sys
import zipfile

fixtures, name = sys.argv[1], sys.argv[2]
with zipfile.ZipFile(f"{fixtures}/{name}.zip", "w") as archive:
    archive.write(f"{fixtures}/windows/baler.exe", "baler.exe")
PYTHON

hash_of() {
    sha256sum "$1" | awk '{ print $1 }'
}

write_checksum() {
    printf '%s  %s\n' "$(hash_of "$fixtures/$1")" "$1" > "$fixtures/$1.sha256"
}

write_checksum "$linux_name.tar.gz"
write_checksum "$windows_name.zip"

# Substitute archives: valid, well-formed, but not what the checksum files
# describe. A swapped release asset looks like this, so a check that only
# caught corrupt archives would pass these.
substitute_dir="$root/substitute"
mkdir -p "$substitute_dir/$linux_name"
printf '#!/bin/sh\necho "malicious"\n' > "$substitute_dir/$linux_name/baler"
chmod +x "$substitute_dir/$linux_name/baler"
tar -czf "$substitute_dir/linux.tar.gz" -C "$substitute_dir" "$linux_name"
printf 'malicious windows binary\n' > "$substitute_dir/baler.exe"
python3 - "$substitute_dir" <<'PYTHON'
import sys
import zipfile

directory = sys.argv[1]
with zipfile.ZipFile(f"{directory}/windows.zip", "w") as archive:
    archive.write(f"{directory}/baler.exe", "baler.exe")
PYTHON

cat > "$fixtures/api.json" <<JSON
{"assets": [
  {"browser_download_url": "$base_url/$linux_name.tar.gz"},
  {"browser_download_url": "$base_url/$linux_name.tar.gz.sha256"},
  {"browser_download_url": "$base_url/$windows_name.zip"},
  {"browser_download_url": "$base_url/$windows_name.zip.sha256"}
]}
JSON

# A curl that serves the fixtures. It understands the only forms the
# installer uses: `curl [-fsSL] [-o file] url`. FAKE_MISSING lists file
# names that answer like a 404 (curl -f exits 22 for that).
fake_bin="$root/fake-bin"
mkdir -p "$fake_bin"
cat > "$fake_bin/curl" <<'CURL'
#!/bin/bash
output=""
url=""
while [ $# -gt 0 ]; do
    case "$1" in
        -o) output="$2"; shift 2 ;;
        -*) shift ;;
        *) url="$1"; shift ;;
    esac
done

name=$(basename "$url")
if [[ "$url" == https://api.github.com/* ]]; then
    source_file="$FAKE_DIR/api.json"
else
    case " ${FAKE_MISSING:-} " in
        *" $name "*) exit 22 ;;
    esac
    source_file="$FAKE_DIR/$name"
fi

[ -f "$source_file" ] || exit 22
if [ -n "$output" ]; then
    cp "$source_file" "$output"
else
    cat "$source_file"
fi
CURL
chmod +x "$fake_bin/curl"

cat > "$fake_bin/uname" <<'UNAME'
#!/bin/bash
case "${1:-}" in
    -s) echo "${FAKE_UNAME_S:-Linux}" ;;
    -m) echo "${FAKE_UNAME_M:-x86_64}" ;;
    *) /usr/bin/uname "$@" ;;
esac
UNAME
chmod +x "$fake_bin/uname"

# ---- runner ----

# Runs the installer in a fresh $HOME. Sets: home, status, output.
run_installer() {
    home=$(mktemp -d "$root/home.XXXXXX")
    if [ -n "${PRE_EXISTING_BINARY:-}" ]; then
        mkdir -p "$home/.local/bin"
        printf '%s' "$PRE_EXISTING_BINARY" > "$home/.local/bin/baler"
    fi
    output=$(
        HOME="$home" \
        SHELL=/bin/bash \
        PATH="$fake_bin:$PATH" \
        FAKE_DIR="$fixtures" \
        BALER_VERSION="$tag" \
        bash "$installer" 2>&1
    )
    status=$?
}

installed_binary() {
    echo "$home/.local/bin/baler"
}

# ---- cases ----

# 1. A good download installs and the binary runs.
run_installer
if [ "$status" -eq 0 ] && [ -x "$(installed_binary)" ] && [ "$("$(installed_binary)")" = "baler fake 9.9.9" ]; then
    pass "a matching checksum installs a working binary"
else
    fail "a matching checksum installs a working binary" "status=$status"
fi

if echo "$output" | grep -q "Checksum verified"; then
    pass "a successful install says the checksum was verified"
else
    fail "a successful install says the checksum was verified"
fi

leftovers=$(find "$home/.local/bin" -mindepth 1 ! -name baler | wc -l)
if [ "$leftovers" -eq 0 ]; then
    pass "nothing but the binary is left in ~/.local/bin"
else
    fail "nothing but the binary is left in ~/.local/bin" "$leftovers extra file(s)"
fi

# 2. An uppercase hash in the checksum file still matches.
cp "$fixtures/$linux_name.tar.gz.sha256" "$fixtures/saved.sha256"
printf '%s  %s\n' "$(hash_of "$fixtures/$linux_name.tar.gz" | tr 'a-f' 'A-F')" "$linux_name.tar.gz" \
    > "$fixtures/$linux_name.tar.gz.sha256"
run_installer
if [ "$status" -eq 0 ] && [ -x "$(installed_binary)" ]; then
    pass "an uppercase hash in the checksum file is accepted"
else
    fail "an uppercase hash in the checksum file is accepted" "status=$status"
fi
cp "$fixtures/saved.sha256" "$fixtures/$linux_name.tar.gz.sha256"

# 3. A swapped (valid but different) archive is refused and nothing is
# installed.
cp "$fixtures/$linux_name.tar.gz" "$fixtures/saved.tar.gz"
cp "$substitute_dir/linux.tar.gz" "$fixtures/$linux_name.tar.gz"
run_installer
if [ "$status" -ne 0 ] && [ ! -e "$(installed_binary)" ]; then
    pass "a swapped archive fails and installs nothing"
else
    fail "a swapped archive fails and installs nothing" "status=$status"
fi
if echo "$output" | grep -q "checksum mismatch"; then
    pass "a mismatch is reported as a checksum mismatch"
else
    fail "a mismatch is reported as a checksum mismatch" "$output"
fi

# 4. A failed run leaves an existing install exactly as it was.
PRE_EXISTING_BINARY="old binary"
run_installer
if [ "$status" -ne 0 ] && [ "$(cat "$(installed_binary)")" = "old binary" ]; then
    pass "a failed install keeps the previously installed binary"
else
    fail "a failed install keeps the previously installed binary" "status=$status"
fi
PRE_EXISTING_BINARY=""
cp "$fixtures/saved.tar.gz" "$fixtures/$linux_name.tar.gz"

# 5. A good install replaces an existing binary.
PRE_EXISTING_BINARY="old binary"
run_installer
if [ "$status" -eq 0 ] && [ "$("$(installed_binary)")" = "baler fake 9.9.9" ]; then
    pass "a good install replaces the previous binary"
else
    fail "a good install replaces the previous binary" "status=$status"
fi
PRE_EXISTING_BINARY=""

# 6. A release without a checksum file is refused.
export FAKE_MISSING="$linux_name.tar.gz.sha256"
run_installer
unset FAKE_MISSING
if [ "$status" -ne 0 ] && [ ! -e "$(installed_binary)" ] && echo "$output" | grep -q "cannot be verified"; then
    pass "a missing checksum file fails and installs nothing"
else
    fail "a missing checksum file fails and installs nothing" "status=$status"
fi

# 7. A malformed checksum file is refused.
printf 'not a hash\n' > "$fixtures/$linux_name.tar.gz.sha256"
run_installer
if [ "$status" -ne 0 ] && [ ! -e "$(installed_binary)" ]; then
    pass "a malformed checksum file fails and installs nothing"
else
    fail "a malformed checksum file fails and installs nothing" "status=$status"
fi
: > "$fixtures/$linux_name.tar.gz.sha256"
run_installer
if [ "$status" -ne 0 ] && [ ! -e "$(installed_binary)" ]; then
    pass "an empty checksum file fails and installs nothing"
else
    fail "an empty checksum file fails and installs nothing" "status=$status"
fi
cp "$fixtures/saved.sha256" "$fixtures/$linux_name.tar.gz.sha256"

# 8. A failed archive download fails and installs nothing.
export FAKE_MISSING="$linux_name.tar.gz"
run_installer
unset FAKE_MISSING
if [ "$status" -ne 0 ] && [ ! -e "$(installed_binary)" ]; then
    pass "a failed archive download fails and installs nothing"
else
    fail "a failed archive download fails and installs nothing" "status=$status"
fi

# 9. The Windows (Git Bash) zip path verifies and installs too.
if command -v unzip >/dev/null 2>&1; then
    export FAKE_UNAME_S="MINGW64_NT-10.0" FAKE_UNAME_M="x86_64"
    run_installer
    if [ "$status" -eq 0 ] && [ "$(cat "$(installed_binary)")" = "fake windows binary" ]; then
        pass "the Windows zip is verified and installed"
    else
        fail "the Windows zip is verified and installed" "status=$status"
    fi

    cp "$substitute_dir/windows.zip" "$fixtures/$windows_name.zip"
    run_installer
    if [ "$status" -ne 0 ] && [ ! -e "$(installed_binary)" ]; then
        pass "a swapped Windows zip fails and installs nothing"
    else
        fail "a swapped Windows zip fails and installs nothing" "status=$status"
    fi
    unset FAKE_UNAME_S FAKE_UNAME_M
else
    echo "skip - unzip is not installed, so the Windows zip path was not tested"
fi

echo
echo "$passed passed, $failed failed"
[ "$failed" -eq 0 ]
