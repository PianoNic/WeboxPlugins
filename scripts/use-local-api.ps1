<#
.SYNOPSIS
  Builds the plugins against a local Webox checkout instead of the git dependency.

.DESCRIPTION
  Writes .cargo/config.toml (git-ignored) with a [patch] that points collabhost-plugin-api at <Webox checkout>/plugin-api.
  Pass the path relative to this repository or absolute; it never ends up in a commit. Run with -Off to go back to the
  git dependency.

.EXAMPLE
  pwsh scripts/use-local-api.ps1 ../Webox
  pwsh scripts/use-local-api.ps1 -Off
#>
param([string]$Webox, [switch]$Off)
$ErrorActionPreference = 'Stop'
$root = Join-Path $PSScriptRoot '..'
$cfg = Join-Path $root '.cargo/config.toml'
if ($Off) { Remove-Item -ErrorAction SilentlyContinue $cfg; 'Using the git dependency.'; return }
if (-not $Webox) { throw 'Pass the path of a Webox checkout, e.g. pwsh scripts/use-local-api.ps1 ../Webox' }
$api = Join-Path $Webox 'plugin-api'
if (-not (Test-Path (Join-Path $api 'Cargo.toml'))) { throw "$api/Cargo.toml not found" }
$api = (Resolve-Path $api).Path -replace '\\', '/'
New-Item -ItemType Directory -Force (Split-Path $cfg) | Out-Null
@"
# Written by scripts/use-local-api.ps1 (git-ignored): build against a local Webox checkout.
[patch.crates-io]
collabhost-plugin-api = { path = "$api" }
"@ | Set-Content -Encoding utf8 $cfg
"Using $api"
