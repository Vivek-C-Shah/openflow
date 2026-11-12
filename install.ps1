# OpenFlow installer for Windows.
#   irm https://raw.githubusercontent.com/Vivek-C-Shah/openflow/main/install.ps1 | iex
$ErrorActionPreference = "Stop"
$Repo = if ($env:OPENFLOW_REPO) { $env:OPENFLOW_REPO } else { "Vivek-C-Shah/openflow" }
$Dir  = Join-Path $env:LOCALAPPDATA "Programs\OpenFlow"

Write-Host "`n  Installing OpenFlow to $Dir" -ForegroundColor Cyan
$rel   = Invoke-RestMethod "https://api.github.com/repos/$Repo/releases/latest" -Headers @{ "User-Agent" = "openflow-install" }
$asset = $rel.assets | Where-Object { $_.name -like "*windows-x64.zip" } | Select-Object -First 1
if (-not $asset) { throw "No Windows build found in the latest release of $Repo" }

$tmp = Join-Path $env:TEMP $asset.name
Invoke-WebRequest $asset.browser_download_url -OutFile $tmp -UseBasicParsing

# Stop a running copy so its files can be replaced.
if (Test-Path "$Dir\openflow.exe") { & "$Dir\openflow.exe" stop 2>$null | Out-Null; Start-Sleep -Milliseconds 500 }
New-Item -ItemType Directory -Force $Dir | Out-Null
Expand-Archive $tmp -DestinationPath $Dir -Force
Remove-Item $tmp

$userPath = [Environment]::GetEnvironmentVariable("Path", "User")
if (($userPath -split ";") -notcontains $Dir) {
    [Environment]::SetEnvironmentVariable("Path", "$userPath;$Dir", "User")
    $env:Path += ";$Dir"
    Write-Host "  Added to PATH (open a new terminal to use 'openflow' anywhere)."
}

& "$Dir\openflow.exe" setup
