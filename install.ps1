$ErrorActionPreference = 'Stop'

$Repo = if ($env:RUST_DBVIEWER_REPO) { $env:RUST_DBVIEWER_REPO } else { 'rafadepaula-olist/rust-dbviewer' }
if ($Repo -notmatch '^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$') {
    throw "Invalid repository format '$Repo'. Expected 'owner/repo'."
}

$AssetName = 'rust-dbviewer-windows-x86_64.exe'
$ChecksumAsset = "$AssetName.sha256"
$InstallDir = Join-Path $env:LOCALAPPDATA 'Programs\rust-dbviewer'
$BinPath = Join-Path $InstallDir 'rust-dbviewer.exe'
$ShortcutDir = Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs'
$ShortcutPath = Join-Path $ShortcutDir 'Rust DBViewer.lnk'

Write-Host 'Installing Rust DBViewer...'

New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null

$Token = if ($env:GITHUB_TOKEN) { $env:GITHUB_TOKEN } elseif ($env:GH_TOKEN) { $env:GH_TOKEN } else { $null }
$TmpFile = Join-Path ([System.IO.Path]::GetTempPath()) ("rust-dbviewer-" + [System.Guid]::NewGuid().ToString('N') + '.exe')
$TmpSha = Join-Path ([System.IO.Path]::GetTempPath()) ("rust-dbviewer-" + [System.Guid]::NewGuid().ToString('N') + '.sha256')

try {
    if ($Token) {
        $ApiUrl = "https://api.github.com/repos/$Repo/releases/latest"
        $ApiHeaders = @{
            'User-Agent'    = 'rust-dbviewer-installer'
            'Authorization' = "Bearer $Token"
            'Accept'        = 'application/vnd.github+json'
        }
        Write-Host "Fetching latest release metadata from $ApiUrl..."
        $Release = Invoke-RestMethod -Uri $ApiUrl -Headers $ApiHeaders
        $BinAsset = $Release.assets | Where-Object { $_.name -eq $AssetName } | Select-Object -First 1
        $ShaAsset = $Release.assets | Where-Object { $_.name -eq $ChecksumAsset } | Select-Object -First 1
        if (-not $BinAsset -or -not $ShaAsset) {
            throw "Release asset '$AssetName' or '$ChecksumAsset' not found in $Repo."
        }
        $AssetHeaders = @{
            'User-Agent'    = 'rust-dbviewer-installer'
            'Authorization' = "Bearer $Token"
            'Accept'        = 'application/octet-stream'
        }
        Write-Host "Downloading $AssetName (authenticated)..."
        Invoke-WebRequest -Uri $BinAsset.url -Headers $AssetHeaders -OutFile $TmpFile -UseBasicParsing
        Invoke-WebRequest -Uri $ShaAsset.url -Headers $AssetHeaders -OutFile $TmpSha -UseBasicParsing
    } else {
        $DownloadUrl = "https://github.com/$Repo/releases/latest/download/$AssetName"
        $ChecksumUrl = "https://github.com/$Repo/releases/latest/download/$ChecksumAsset"
        $Headers = @{ 'User-Agent' = 'rust-dbviewer-installer' }
        Write-Host "Downloading $AssetName from $DownloadUrl..."
        Invoke-WebRequest -Uri $DownloadUrl -Headers $Headers -OutFile $TmpFile -UseBasicParsing
        Invoke-WebRequest -Uri $ChecksumUrl -Headers $Headers -OutFile $TmpSha -UseBasicParsing
    }

    if (-not (Test-Path $TmpFile) -or ((Get-Item $TmpFile).Length -eq 0)) {
        throw "Downloaded binary is empty."
    }

    $ExpectedHash = ((Get-Content -Path $TmpSha -Raw).Trim() -split '\s+')[0].ToLowerInvariant()
    if ($ExpectedHash -notmatch '^[0-9a-f]{64}$') {
        throw "Invalid SHA-256 checksum file."
    }

    $ActualHash = (Get-FileHash -Path $TmpFile -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($ActualHash -ne $ExpectedHash) {
        throw "SHA-256 checksum mismatch for $AssetName. Expected $ExpectedHash, got $ActualHash."
    }

    Move-Item -Force -Path $TmpFile -Destination $BinPath
} finally {
    if (Test-Path $TmpFile) {
        Remove-Item -Force -Path $TmpFile -ErrorAction SilentlyContinue
    }
    if (Test-Path $TmpSha) {
        Remove-Item -Force -Path $TmpSha -ErrorAction SilentlyContinue
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
