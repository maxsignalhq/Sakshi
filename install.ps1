# Install Sakshi on Windows.
#
#   irm https://raw.githubusercontent.com/maxsignalhq/Sakshi/main/install.ps1 | iex
#
# Downloads the release zip for this machine, checks its SHA-256 against the
# published checksum, puts sakshi.exe in %LOCALAPPDATA%\Programs\sakshi and adds
# that folder to your user PATH. Settings, all optional:
#
#   $env:SAKSHI_VERSION         tag to install, e.g. v0.1.0 (default: latest)
#   $env:SAKSHI_INSTALL_DIR     where to put sakshi.exe
#   $env:SAKSHI_NO_MODIFY_PATH  set to 1 to leave your PATH alone

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

$Repo = 'maxsignalhq/Sakshi'
$Base = if ($env:SAKSHI_DOWNLOAD_BASE) { $env:SAKSHI_DOWNLOAD_BASE } else { "https://github.com/$Repo/releases" }
$InstallDir = if ($env:SAKSHI_INSTALL_DIR) { $env:SAKSHI_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA 'Programs\sakshi' }

function Fail($message) {
    Write-Error "sakshi: $message"
    exit 1
}

# Which build fits this machine. OSArchitecture is the real CPU, even when
# PowerShell itself runs emulated.
$osArch = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString()
switch ($osArch) {
    'X64'   { $arch = 'x86_64' }
    'Arm64' { $arch = 'aarch64' }
    default { Fail "no prebuilt binary for CPU '$osArch'; install from source with: cargo install --git https://github.com/$Repo sakshi" }
}
$target = "$arch-pc-windows-msvc"

# Which release.
$version = $env:SAKSHI_VERSION
if (-not $version) {
    try {
        $release = Invoke-RestMethod -Uri "https://api.github.com/repos/$Repo/releases/latest" -Headers @{ 'User-Agent' = 'sakshi-installer' }
        $version = $release.tag_name
    } catch {
        Fail "could not find the latest release; set `$env:SAKSHI_VERSION, e.g. v0.1.0"
    }
}
if (-not $version.StartsWith('v')) { $version = "v$version" }

$name = "sakshi-$version-$target"
$url = "$Base/download/$version/$name.zip"
$tmp = Join-Path ([System.IO.Path]::GetTempPath()) ("sakshi-" + [System.Guid]::NewGuid())
New-Item -ItemType Directory -Path $tmp | Out-Null

try {
    Write-Host "sakshi: downloading $name"
    $zip = Join-Path $tmp "$name.zip"
    Invoke-WebRequest -Uri $url -OutFile $zip -UseBasicParsing
    $expected = ((Invoke-WebRequest -Uri "$url.sha256" -UseBasicParsing).Content -split '\s+')[0].Trim().ToLower()
    $actual = (Get-FileHash $zip -Algorithm SHA256).Hash.ToLower()
    if (-not $expected -or $expected -ne $actual) {
        Fail "checksum mismatch for $name.zip; not installing"
    }

    Expand-Archive -Path $zip -DestinationPath (Join-Path $tmp 'out') -Force
    $exe = Join-Path $tmp 'out\sakshi.exe'
    if (-not (Test-Path $exe)) { Fail "archive did not contain sakshi.exe" }

    New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
    Copy-Item $exe (Join-Path $InstallDir 'sakshi.exe') -Force
    Write-Host "sakshi: installed $version to $InstallDir\sakshi.exe"
} finally {
    Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
}

$userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
$onPath = ($userPath -split ';') -contains $InstallDir
if ($onPath) {
    Write-Host "sakshi: run 'sakshi' to start"
} elseif ($env:SAKSHI_NO_MODIFY_PATH -eq '1') {
    Write-Host "sakshi: add $InstallDir to your PATH to run 'sakshi' from anywhere"
} else {
    $newPath = if ($userPath) { "$userPath;$InstallDir" } else { $InstallDir }
    [Environment]::SetEnvironmentVariable('Path', $newPath, 'User')
    Write-Host "sakshi: added $InstallDir to your user PATH; open a new terminal and run 'sakshi'"
}
