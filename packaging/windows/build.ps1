# Builds the release exe, a portable zip and (if Inno Setup is installed) the installer.
# Run from the repository root:   powershell -File packaging\windows\build.ps1
$ErrorActionPreference = "Stop"
Set-Location (Join-Path $PSScriptRoot "..\..")

$version = (Select-String -Path "Cargo.toml" -Pattern '^version\s*=\s*"([^"]+)"' | Select-Object -First 1).Matches.Groups[1].Value
Write-Host "Alfadoc $version"

cargo build --release -p alfadoc-app -j 2
if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }

$exe = "target\release\alfadoc.exe"
New-Item -ItemType Directory -Force -Path dist | Out-Null
$zip = "dist\Alfadoc-$version-windows-portable.zip"
if (Test-Path $zip) { Remove-Item $zip }
Compress-Archive -Path $exe -DestinationPath $zip
Write-Host "Portable: $zip"

$iscc = @(
  "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe",
  "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe",
  "$env:ProgramFiles\Inno Setup 6\ISCC.exe"
) | Where-Object { Test-Path $_ } | Select-Object -First 1

if ($iscc) {
  & $iscc "/DAppVersion=$version" "/DExePath=$((Resolve-Path $exe).Path)" "packaging\windows\alfadoc.iss"
  if ($LASTEXITCODE -ne 0) { throw "Inno Setup failed" }
  Write-Host "Installer: dist\Alfadoc-$version-setup.exe"
} else {
  Write-Host "Inno Setup 6 not found: installer skipped. Install it from https://jrsoftware.org/isinfo.php and run this script again."
}
