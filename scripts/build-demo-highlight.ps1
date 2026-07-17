param(
    [string]$Python = "python",
    [string]$Output = "docs/assets/kcoin-demo-highlight.gif"
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$outputPath = if ([System.IO.Path]::IsPathRooted($Output)) { $Output } else { Join-Path $root $Output }

& $Python (Join-Path $PSScriptRoot "build-demo-highlight.py") `
    --assets (Join-Path $root "docs/assets") `
    --output $outputPath

if ($LASTEXITCODE -ne 0) {
    throw "Demo highlight builder exited with code $LASTEXITCODE."
}
