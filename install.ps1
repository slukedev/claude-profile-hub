# Installs claude-profile-hub from the latest GitHub release (Windows).
#
#   irm https://raw.githubusercontent.com/slukedev/claude-profile-hub/main/install.ps1 | iex
#
# Set $env:CPH_VERSION to a tag such as v0.1.0 to pin a version.
$ErrorActionPreference = 'Stop'
$repo = 'slukedev/claude-profile-hub'
$bin = 'claude-profile-hub'
$arch = if ($env:PROCESSOR_ARCHITECTURE -eq 'ARM64') { 'aarch64' } else { 'x86_64' }
$target = "$arch-pc-windows-msvc"
$base = if ($env:CPH_VERSION) { "https://github.com/$repo/releases/download/$($env:CPH_VERSION)" } else { "https://github.com/$repo/releases/latest/download" }
$dir = Join-Path $env:LOCALAPPDATA "Programs\$bin"
$tmp = Join-Path ([IO.Path]::GetTempPath()) ([Guid]::NewGuid())
New-Item -ItemType Directory -Force -Path $tmp, $dir | Out-Null

try {
    Write-Host "Downloading $bin for $target..."
    $sums = (Invoke-WebRequest -UseBasicParsing "$base/SHA256SUMS").Content
    if ($sums -is [byte[]]) { $sums = [Text.Encoding]::UTF8.GetString($sums) }
    foreach ($name in @("$bin-$target.exe", "$bin-w-$target.exe")) {
        $file = Join-Path $tmp $name
        Invoke-WebRequest -UseBasicParsing "$base/$name" -OutFile $file
        $want = ($sums -split "`n" | Where-Object { $_ -match "\s\*?$([regex]::Escape($name))\s*$" } | ForEach-Object { ($_ -split '\s+')[0] }) | Select-Object -First 1
        $got = (Get-FileHash -Algorithm SHA256 $file).Hash.ToLower()
        if (-not $want -or $want.ToLower() -ne $got) { throw "checksum mismatch for $name; nothing was installed" }
    }
    Copy-Item (Join-Path $tmp "$bin-$target.exe") (Join-Path $dir "$bin.exe") -Force
    Copy-Item (Join-Path $tmp "$bin-w-$target.exe") (Join-Path $dir "$bin-w.exe") -Force
} finally {
    Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
}

$userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
if (-not ($userPath -split ';' | Where-Object { $_ -eq $dir })) {
    [Environment]::SetEnvironmentVariable('Path', "$userPath;$dir", 'User')
    Write-Host "Added $dir to your PATH. Open a new terminal to use it."
}
Write-Host "Installed $dir\$bin.exe. Run '$bin' to set up your profiles."
