$ErrorActionPreference = "Stop"

$Repo = if ($env:PATCHBAY_REPO) { $env:PATCHBAY_REPO } else { "lifuyue/patchbay-cli" }
$Version = if ($env:PATCHBAY_VERSION) { $env:PATCHBAY_VERSION } else { "latest" }
$InstallDir = if ($env:PATCHBAY_INSTALL_DIR) {
    $env:PATCHBAY_INSTALL_DIR
} elseif ($env:LOCALAPPDATA) {
    Join-Path $env:LOCALAPPDATA "Patchbay\bin"
} else {
    Join-Path $HOME ".patchbay\bin"
}

$Arch = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString()
switch ($Arch) {
    "X64" { $Asset = "patchbay-x86_64-pc-windows-msvc.zip" }
    default {
        throw "patchbay installer: unsupported Windows architecture: $Arch. Download a release manually from https://github.com/$Repo/releases"
    }
}

if ($Version -eq "latest") {
    $BaseUrl = "https://github.com/$Repo/releases/latest/download"
} else {
    $BaseUrl = "https://github.com/$Repo/releases/download/$Version"
}

$TempDir = Join-Path ([System.IO.Path]::GetTempPath()) ("patchbay-" + [System.Guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Path $TempDir | Out-Null

try {
    $Archive = Join-Path $TempDir $Asset
    $Checksums = Join-Path $TempDir "SHA256SUMS"

    Write-Host "Downloading Patchbay CLI $Version for Windows $Arch..."
    Invoke-WebRequest -Uri "$BaseUrl/$Asset" -OutFile $Archive
    Invoke-WebRequest -Uri "$BaseUrl/SHA256SUMS" -OutFile $Checksums

    $Pattern = "\s$([System.Text.RegularExpressions.Regex]::Escape($Asset))$"
    $ExpectedLine = Get-Content $Checksums | Where-Object { $_ -match $Pattern } | Select-Object -First 1
    if (-not $ExpectedLine) {
        throw "patchbay installer: SHA256SUMS does not include $Asset"
    }

    $ExpectedHash = (($ExpectedLine -split "\s+")[0]).ToUpperInvariant()
    $ActualHash = (Get-FileHash -Path $Archive -Algorithm SHA256).Hash.ToUpperInvariant()
    if ($ActualHash -ne $ExpectedHash) {
        throw "patchbay installer: checksum mismatch for $Asset"
    }

    Expand-Archive -Path $Archive -DestinationPath $TempDir -Force
    $Binary = Join-Path $TempDir "patchbay.exe"
    if (-not (Test-Path $Binary)) {
        throw "patchbay installer: archive did not contain patchbay.exe"
    }

    New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
    $InstalledPath = Join-Path $InstallDir "patchbay.exe"
    Copy-Item -Path $Binary -Destination $InstalledPath -Force

    Write-Host "Installed patchbay to $InstalledPath"

    $PathEntries = $env:PATH -split ";"
    if ($PathEntries -notcontains $InstallDir) {
        Write-Host "Add $InstallDir to your PATH to run patchbay from any shell."
    }
} finally {
    Remove-Item -Path $TempDir -Recurse -Force -ErrorAction SilentlyContinue
}
