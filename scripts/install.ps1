$ErrorActionPreference = "Stop"

$Repo = "baler-dev/baler"
$DefaultVersion = "__VERSION__"

$InstallDir = "$env:USERPROFILE\.local\bin"
New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null

$arch = if ([System.Environment]::Is64BitOperatingSystem) { "x86_64" } else {
    Write-Error "Unsupported architecture."
    exit 1
}
$target = "$arch-pc-windows-msvc"

# A release renders DefaultVersion to its tag (v0.1.0). Anything else,
# such as the untouched placeholder, means "no pinned version".
$version = $env:BALER_VERSION
if (-not $version -and $DefaultVersion -match '^v\d') {
    $version = $DefaultVersion
}

if ($version) {
    Write-Host "Fetching release $version..."
    $apiUrl = "https://api.github.com/repos/$Repo/releases/tags/$version"
} else {
    Write-Host "Fetching latest release..."
    $apiUrl = "https://api.github.com/repos/$Repo/releases/latest"
}

Write-Host "Fetching download URL for $target..."
$release = Invoke-RestMethod -Uri $apiUrl
$asset = $release.assets | Where-Object { $_.name -like "*$target.zip" } | Select-Object -First 1

if (-not $asset) {
    Write-Error "No release asset found for $target."
    exit 1
}

$tmp = New-Item -ItemType Directory -Path ([System.IO.Path]::GetTempPath()) -Name "baler-install" -Force

Write-Host "Downloading baler from $($asset.browser_download_url)..."
Invoke-WebRequest -Uri $asset.browser_download_url -OutFile "$tmp\baler.zip"

# Every release publishes a .sha256 file next to each archive, in the
# format `sha256sum` writes: the hash, then the file name. Nothing is
# unpacked or installed unless the archive matches it.
$checksumAsset = $release.assets | Where-Object { $_.name -eq "$($asset.name).sha256" } | Select-Object -First 1
if (-not $checksumAsset) {
    Remove-Item -Recurse -Force $tmp
    Write-Error "No checksum file found for $($asset.name), so the download cannot be verified. Nothing was installed."
    exit 1
}

Invoke-WebRequest -Uri $checksumAsset.browser_download_url -OutFile "$tmp\baler.zip.sha256"
$checksumText = (Get-Content -Raw -Path "$tmp\baler.zip.sha256").Trim()
$expectedHash = ($checksumText -split '\s+')[0].ToLowerInvariant()
if ($expectedHash -notmatch '^[0-9a-f]{64}$') {
    Remove-Item -Recurse -Force $tmp
    Write-Error "The checksum file for $($asset.name) is not valid. Nothing was installed."
    exit 1
}

$actualHash = (Get-FileHash -Algorithm SHA256 -Path "$tmp\baler.zip").Hash.ToLowerInvariant()
if ($actualHash -ne $expectedHash) {
    Remove-Item -Recurse -Force $tmp
    Write-Error "Checksum mismatch for $($asset.name). Expected $expectedHash but got $actualHash. Nothing was installed."
    exit 1
}
Write-Host "Checksum verified."

Expand-Archive -Path "$tmp\baler.zip" -DestinationPath $tmp -Force

Move-Item -Path "$tmp\baler.exe" -Destination "$InstallDir\baler.exe" -Force
Remove-Item -Recurse -Force $tmp

Write-Host "baler installed to $InstallDir\baler.exe"

$userPath = [Environment]::GetEnvironmentVariable("Path", "User")
if ($userPath -notlike "*$InstallDir*") {
    Write-Host "Adding $InstallDir to your PATH..."
    [Environment]::SetEnvironmentVariable("Path", "$userPath;$InstallDir", "User")
    Write-Host "Please open a new terminal for the PATH change to take effect."
} else {
    Write-Host "$InstallDir is already in your PATH."
}
