$ErrorActionPreference = 'Stop'

$Repo = if ($env:RUST_DBVIEWER_REPO) { $env:RUST_DBVIEWER_REPO } else { 'rafadepaula-olist/rust-dbviewer' }
$AssetName = 'rust-dbviewer-windows-x86_64.exe'
$InstallDir = Join-Path $env:LOCALAPPDATA 'Programs\rust-dbviewer'
$BinPath = Join-Path $InstallDir 'rust-dbviewer.exe'
$ShortcutDir = Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs'
$ShortcutPath = Join-Path $ShortcutDir 'Rust DBViewer.lnk'

Write-Host 'Installing Rust DBViewer...'

New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null

$DownloadUrl = "https://github.com/$Repo/releases/latest/download/$AssetName"
$Headers = @{ 'User-Agent' = 'rust-dbviewer-installer' }
$Token = if ($env:GITHUB_TOKEN) { $env:GITHUB_TOKEN } elseif ($env:GH_TOKEN) { $env:GH_TOKEN } else { $null }
if ($Token) {
    $Headers['Authorization'] = "Bearer $Token"
}

$TmpFile = Join-Path ([System.IO.Path]::GetTempPath()) ("rust-dbviewer-" + [System.Guid]::NewGuid().ToString('N') + '.exe')

try {
    Write-Host "Downloading $AssetName from $DownloadUrl..."
    Invoke-WebRequest -Uri $DownloadUrl -Headers $Headers -OutFile $TmpFile -UseBasicParsing

    if (-not (Test-Path $TmpFile) -or ((Get-Item $TmpFile).Length -eq 0)) {
        throw "Downloaded binary is empty."
    }

    Move-Item -Force -Path $TmpFile -Destination $BinPath
} finally {
    if (Test-Path $TmpFile) {
        Remove-Item -Force -Path $TmpFile -ErrorAction SilentlyContinue
    }
}

# Idempotently add install directory to User Path
$UserPath = [Environment]::GetEnvironmentVariable('Path', 'User')
$PathEntries = @()
if ($UserPath) {
    $PathEntries = $UserPath.Split(';', [System.StringSplitOptions]::RemoveEmptyEntries)
}

$NormalizedInstall = $InstallDir.TrimEnd('\').ToLowerInvariant()
$AlreadyInUserPath = $false
foreach ($Entry in $PathEntries) {
    if ($Entry.TrimEnd('\').ToLowerInvariant() -eq $NormalizedInstall) {
        $AlreadyInUserPath = $true
        break
    }
}

if (-not $AlreadyInUserPath) {
    $NewUserPath = if ($UserPath) { "$UserPath;$InstallDir" } else { $InstallDir }
    [Environment]::SetEnvironmentVariable('Path', $NewUserPath, 'User')
    Write-Host "Added $InstallDir to your User PATH."
}

$SessionEntries = if ($env:Path) { $env:Path.Split(';', [System.StringSplitOptions]::RemoveEmptyEntries) } else { @() }
$AlreadyInSessionPath = $false
foreach ($Entry in $SessionEntries) {
    if ($Entry.TrimEnd('\').ToLowerInvariant() -eq $NormalizedInstall) {
        $AlreadyInSessionPath = $true
        break
    }
}
if (-not $AlreadyInSessionPath) {
    $env:Path = "$env:Path;$InstallDir"
}

# Create Start Menu shortcut
New-Item -ItemType Directory -Force -Path $ShortcutDir | Out-Null
$WshShell = New-Object -ComObject WScript.Shell
$Shortcut = $WshShell.CreateShortcut($ShortcutPath)
$Shortcut.TargetPath = $BinPath
$Shortcut.WorkingDirectory = $InstallDir
$Shortcut.Description = 'Fast native Rust MySQL and MariaDB viewer'
$Shortcut.Save()

Write-Host ''
Write-Host 'Rust DBViewer installed successfully!'
Write-Host "  Binary:   $BinPath"
Write-Host "  Shortcut: $ShortcutPath"
Write-Host ''
Write-Host 'Launch it by running "rust-dbviewer" in a new terminal or opening "Rust DBViewer" from the Start Menu.'
