# Callus installer for Windows (PowerShell 5.1 or 7).
#
#   irm https://raw.githubusercontent.com/rennerdo30/callus/main/install.ps1 | iex
#   & ([scriptblock]::Create((irm https://raw.githubusercontent.com/rennerdo30/callus/main/install.ps1))) update
#   & ([scriptblock]::Create((irm https://raw.githubusercontent.com/rennerdo30/callus/main/install.ps1))) uninstall
#       (arguments: install, update or uninstall, then claude codex opencode antigravity, --yes, --dry-run, --packs)
#
# Zero Python or Node.js required: downloads the single prebuilt `callus.exe` release binary
# (or uses $env:CALLUS_ARCHIVE / $env:CALLUS_BIN for offline or pre-release testing), installs it
# into %LOCALAPPDATA%\Programs\Callus (or $env:CALLUS_APP), adds that folder to your user PATH,
# and runs `callus install|update|uninstall` which shows exact per-tool steps and asks before each one.

& {
    $ErrorActionPreference = "Stop"
    $repo = "rennerdo30/callus"
    $target = "x86_64-pc-windows-msvc"
    $appDir = if ($env:CALLUS_APP) {
        $env:CALLUS_APP
    } else {
        $localBase = if ($env:LOCALAPPDATA) { $env:LOCALAPPDATA } else { Join-Path $HOME "AppData\Local" }
        Join-Path $localBase "Programs\Callus"
    }
    $markerPath = Join-Path $appDir ".callus-app.json"
    $installedBin = Join-Path $appDir "callus.exe"

    $rest = @($args)
    $action = "install"
    if ($rest.Count -gt 0 -and @("install", "update", "uninstall") -contains $rest[0]) {
        $action = $rest[0]
        $rest = @($rest | Select-Object -Skip 1)
    }
    $isDryRun = ($rest -contains "--dry-run")

    function Add-UserPath([string]$dir) {
        $current = [Environment]::GetEnvironmentVariable("Path", "User")
        $entries = @()
        if ($current) {
            $entries = @($current -split ";" | Where-Object { $_ -and $_.Trim() -ne "" })
        }
        $normTarget = $dir.TrimEnd("\").ToLowerInvariant()
        foreach ($e in $entries) {
            if ($e.Trim().TrimEnd("\").ToLowerInvariant() -eq $normTarget) {
                return $false
            }
        }
        $updated = (@($entries) + @($dir)) -join ";"
        [Environment]::SetEnvironmentVariable("Path", $updated, "User")
        if ($env:Path -notlike "*$dir*") {
            $env:Path = "$dir;$env:Path"
        }
        return $true
    }

    function Remove-UserPath([string]$dir) {
        $current = [Environment]::GetEnvironmentVariable("Path", "User")
        if (-not $current) { return }
        $normTarget = $dir.TrimEnd("\").ToLowerInvariant()
        $kept = @($current -split ";" | Where-Object {
            $_ -and $_.Trim() -ne "" -and ($_.Trim().TrimEnd("\").ToLowerInvariant() -ne $normTarget)
        })
        [Environment]::SetEnvironmentVariable("Path", ($kept -join ";"), "User")
    }

    function Assert-OwnedAppDir([string]$dir) {
        if ((Test-Path $dir) -and -not (Test-Path $markerPath)) {
            $items = @(Get-ChildItem -Path $dir -Force -ErrorAction SilentlyContinue)
            if ($items.Count -gt 0) {
                throw "$dir exists and was not created by the Callus installer; set `$env:CALLUS_APP to another folder."
            }
        }
    }

    if ($action -eq "uninstall") {
        $runner = if ($env:CALLUS_BIN -and (Test-Path $env:CALLUS_BIN)) {
            $env:CALLUS_BIN
        } elseif (Test-Path $installedBin) {
            $installedBin
        } elseif (Get-Command "callus" -ErrorAction SilentlyContinue) {
            (Get-Command "callus").Source
        } else {
            $null
        }
        if ($runner) {
            & $runner uninstall @rest
            if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
        } else {
            Write-Host "callus binary not found; skipping agent config cleanup."
        }
        if (Test-Path $markerPath) {
            if ($isDryRun) {
                Write-Host "[dry-run] would remove $appDir and clean user PATH entry"
            } else {
                Remove-UserPath $appDir
                Remove-Item -Recurse -Force $appDir -ErrorAction SilentlyContinue
                Write-Host "Removed $appDir (your ~/.callus database was left untouched)."
            }
        }
        return
    }

    Assert-OwnedAppDir $appDir

    $tmp = Join-Path ([IO.Path]::GetTempPath()) ("callus-" + [Guid]::NewGuid().ToString("N"))
    New-Item -ItemType Directory -Path $tmp | Out-Null
    try {
        $sourceBin = $null
        $sourcePacks = $null
        $versionLabel = "local"

        if ($env:CALLUS_BIN -and (Test-Path $env:CALLUS_BIN)) {
            $sourceBin = (Resolve-Path $env:CALLUS_BIN).Path
            $candidatePacks = Join-Path (Split-Path $sourceBin -Parent) "packs"
            if (Test-Path $candidatePacks) { $sourcePacks = $candidatePacks }
            Write-Host "Using local binary $sourceBin"
        } else {
            $zip = Join-Path $tmp "callus.zip"
            if ($env:CALLUS_ARCHIVE -and (Test-Path $env:CALLUS_ARCHIVE)) {
                Copy-Item $env:CALLUS_ARCHIVE $zip
                $versionLabel = "archive"
            } else {
                [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
                $ver = $env:CALLUS_VERSION
                if (-not $ver) {
                    $releaseUrl = "https://api.github.com/repos/$repo/releases/latest"
                    Write-Host "Checking latest Callus release from $repo..."
                    try {
                        $rel = Invoke-RestMethod -Uri $releaseUrl -Headers @{ "User-Agent" = "callus-installer" }
                        $ver = $rel.tag_name
                    } catch {
                        throw "No published GitHub release found yet for $repo. Build from source with `cargo install --path crates/callus-cli --locked` or set `$env:CALLUS_BIN / `$env:CALLUS_ARCHIVE."
                    }
                }
                $versionLabel = $ver
                $url = if ($env:CALLUS_ARCHIVE) {
                    $env:CALLUS_ARCHIVE
                } else {
                    "https://github.com/$repo/releases/download/$ver/callus-$ver-$target.zip"
                }
                Write-Host "Downloading $url"
                Invoke-WebRequest -Uri $url -OutFile $zip -UseBasicParsing
            }
            Expand-Archive -Path $zip -DestinationPath $tmp -Force
            $binItem = Get-ChildItem -Path $tmp -Recurse -Filter "callus.exe" | Select-Object -First 1
            if (-not $binItem) {
                throw "The downloaded archive did not contain callus.exe."
            }
            $sourceBin = $binItem.FullName
            $packsItem = Join-Path $binItem.DirectoryName "packs"
            if (Test-Path $packsItem) { $sourcePacks = $packsItem }
        }

        if ($isDryRun) {
            Write-Host "[dry-run] would install callus.exe to $installedBin and add $appDir to user PATH"
            & $sourceBin $action @rest
            if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
            return
        }

        New-Item -ItemType Directory -Path $appDir -Force | Out-Null
        Get-ChildItem -Path $appDir -Filter "callus.exe.old.*" -ErrorAction SilentlyContinue | Remove-Item -Force -ErrorAction SilentlyContinue
        if (Test-Path $installedBin) {
            try {
                Copy-Item -Path $sourceBin -Destination $installedBin -Force -ErrorAction Stop
            } catch {
                # Windows locks running .exe files (e.g. an active `callus mcp` server),
                # but allows renaming them aside so the new binary can take its place cleanly.
                $staleBin = Join-Path $appDir ("callus.exe.old." + [Guid]::NewGuid().ToString("N"))
                Move-Item -Path $installedBin -Destination $staleBin -Force
                Copy-Item -Path $sourceBin -Destination $installedBin -Force
                Remove-Item -Path $staleBin -Force -ErrorAction SilentlyContinue
            }
        } else {
            Copy-Item -Path $sourceBin -Destination $installedBin -Force
        }
        if ($sourcePacks) {
            $destPacks = Join-Path $appDir "packs"
            if (Test-Path $destPacks) { Remove-Item -Recurse -Force $destPacks }
            Copy-Item -Path $sourcePacks -Destination $destPacks -Recurse -Force
        }
        $marker = @{
            managed_by = "callus-installer"
            version    = $versionLabel
            installed  = (Get-Date).ToUniversalTime().ToString("yyyy-MM-ddTHH:mm:ssZ")
        } | ConvertTo-Json -Depth 3
        Set-Content -Path $markerPath -Value $marker -Encoding UTF8

        if (Add-UserPath $appDir) {
            Write-Host "Added $appDir to your user PATH (open a new terminal to use `callus` directly)."
        }
        Write-Host "Installed Callus to $installedBin"

        & $installedBin $action @rest
        if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
    } finally {
        Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
    }
} @args
