# Fetches each app's icon once, at development time, and bundles them into
# assets\app-icons.json, which is built into RustWinUtil.exe.
#
#   powershell -ExecutionPolicy Bypass -File tools\fetch-icons.ps1
#
# WinUtil loads these live from Google's favicon service every time it opens.
# RustWinUtil fetches them here instead, once, from the same service, so the app
# itself never goes online for them and never sends the app list anywhere. Run it
# again after updating winutil\config\applications.json.
#
# Only genuine PNGs under 64 KB are kept, and Google's generic "no icon" globe is
# skipped so those apps keep their lettered tile. The logos belong to their
# owners and are used only to show which app is which, as WinUtil does.

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path)
$apps = Get-Content -Raw (Join-Path $root 'winutil\config\applications.json') | ConvertFrom-Json
$out  = Join-Path $root 'assets\app-icons.json'

function Get-Icon([string]$link) {
    $uri = "https://www.google.com/s2/favicons?sz=64&domain_url=$([uri]::EscapeDataString($link))"
    try {
        $resp = Invoke-WebRequest -Uri $uri -UseBasicParsing -TimeoutSec 20
        return [byte[]]$resp.Content
    } catch {
        return $null
    }
}

function Get-Sha([byte[]]$bytes) {
    $sha = [System.Security.Cryptography.SHA256]::Create()
    return [BitConverter]::ToString($sha.ComputeHash($bytes))
}

# What Google answers for a site it has no icon for.
$globe = Get-Icon 'https://no-such-site.invalid'
$globeHash = if ($globe) { Get-Sha $globe } else { '' }

$icons = [ordered]@{}
$skipped = @()
foreach ($p in $apps.PSObject.Properties) {
    $bytes = Get-Icon $p.Value.link
    $png = $bytes -and $bytes.Length -gt 8 -and $bytes.Length -lt 65536 -and
           $bytes[0] -eq 0x89 -and $bytes[1] -eq 0x50 -and $bytes[2] -eq 0x4E -and $bytes[3] -eq 0x47
    if ($png -and (Get-Sha $bytes) -ne $globeHash) {
        $icons[$p.Name] = 'data:image/png;base64,' + [Convert]::ToBase64String($bytes)
    } else {
        $skipped += $p.Name
    }
}

[IO.File]::WriteAllText($out, ($icons | ConvertTo-Json -Compress), (New-Object Text.UTF8Encoding $false))
$size = [Math]::Round((Get-Item $out).Length / 1KB)
Write-Output "Wrote $($icons.Count) icons ($size KB) to $out; $($skipped.Count) apps keep their letter tile."
