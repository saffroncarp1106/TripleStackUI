param([Parameter(Mandatory = $true)][string]$SkinsDirectory)
$ErrorActionPreference = 'Stop'
$projectDirectory = Split-Path -Parent $PSScriptRoot
$sourceDirectory = (Resolve-Path -LiteralPath $SkinsDirectory).Path
& cargo build --locked --manifest-path (Join-Path $projectDirectory 'Cargo.toml')
if ($LASTEXITCODE -ne 0) { throw 'Could not build the preset importer.' }
$executable = Join-Path $projectDirectory 'target/debug/triplestack.exe'
$reportDirectory = Join-Path $env:LOCALAPPDATA 'TripleStack'
New-Item -ItemType Directory -Path $reportDirectory -Force | Out-Null
$reportPath = Join-Path $reportDirectory ('stable-import-' + [guid]::NewGuid().ToString('N') + '.json')
$process = Start-Process -FilePath $executable -ArgumentList @('--compile-stable-presets', ('"{0}"' -f $sourceDirectory), ('"{0}"' -f $reportPath)) -WindowStyle Hidden -Wait -PassThru
if (!(Test-Path -LiteralPath $reportPath)) { throw "Importer exited with code $($process.ExitCode) without a report." }
$report = Get-Content -LiteralPath $reportPath -Raw -Encoding UTF8 | ConvertFrom-Json
if ($report.error) { throw $report.error }
Write-Output ("{0} added, {1} unchanged, {2} without audio, {3} failed" -f @($report.created).Count, @($report.unchanged).Count, @($report.skipped).Count, @($report.errors).Count)
foreach ($preset in $report.created) { Write-Output ("{0}: {1} sounds" -f $preset.name, $preset.sounds) }
foreach ($failure in $report.errors) { Write-Warning $failure }
Write-Output "Report: $reportPath"
if ($process.ExitCode -ne 0) { throw 'Some skins could not be imported; other presets were saved. See the report.' }
