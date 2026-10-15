# RustWinUtil's stand-ins for WinUtil's GUI plumbing.
#
# WinUtil's own scripts (winutil\functions, copied unchanged) were written to run
# inside its WPF window: they log through Write-WinUtilLog, report progress through
# Step-WinUtilJob, ask questions through Show-WinUtilMessage, and run work through
# Start-WinUtilJob. RustWinUtil runs them headless instead, one PowerShell process
# per job, and turns their output into its own log. These definitions make that
# work; RustWinUtil asks its own questions before a script starts.

$ErrorActionPreference = 'Continue'
$ProgressPreference = 'SilentlyContinue'
[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new()

$sync = [hashtable]::Synchronized(@{})
$sync.winutildir = $env:RUSTWINUTIL_WORKDIR
$sync.configs = @{}
$sync.configs.dns = @'
{{DNS_JSON}}
'@ | ConvertFrom-Json
# Read by two tweak scripts that refresh WinUtil's own theme; nothing to refresh here.
$sync.ThemeButton = @{ Content = '' }

function Write-WinUtilLog {
    param([string]$Level = 'INFO', [string]$Component = '', [string]$Message = '')
    if ($Level -eq 'INFO') { Write-Host "[$Component] $Message" } else { Write-Host "$Level [$Component] $Message" }
}

function Step-WinUtilJob {
    param([string]$Status, [int]$Percent = -1, [string]$State, [string]$Overlay, [switch]$Hide)
    if ($Status) { Write-Host $Status }
}

# RustWinUtil confirms anything that needs it before the script runs.
function Show-WinUtilMessage {
    param($Message, $Title, $Button, $Icon)
    Write-Host "${Title}: $Message"
    return 'Yes'
}

function Start-WinUtilJob {
    param([string]$Name, [string]$Description, [hashtable]$Parameters = @{}, [scriptblock]$ScriptBlock, [switch]$DisableAppList)
    if ($Description) { Write-Host $Description }
    & $ScriptBlock @Parameters
}

function Invoke-WPFRunspace {
    param([scriptblock]$ScriptBlock, $ArgumentList, $ParameterList)
    if ($ParameterList) { & $ScriptBlock @ParameterList } elseif ($ArgumentList) { & $ScriptBlock @ArgumentList } else { & $ScriptBlock }
}

function Invoke-WPFUIThread { param([scriptblock]$ScriptBlock, $Parameters) }
function Invoke-WinutilThemeChange { param($theme) }
