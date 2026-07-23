param(
    [string]$PythonLauncher = "py",
    [string]$OutputDirectory
)

$ErrorActionPreference = "Stop"
$ProjectRoot = Split-Path -Parent $PSScriptRoot
$BuildRoot = Join-Path $ProjectRoot "output\silero-worker-packaging"
$EnvironmentRoot = Join-Path $BuildRoot "env"

if (-not $OutputDirectory) {
    $OutputDirectory = Join-Path $ProjectRoot "output\silero-worker-dist"
}
$OutputDirectory = [System.IO.Path]::GetFullPath($OutputDirectory)

if (-not (Test-Path -LiteralPath (Join-Path $EnvironmentRoot "Scripts\python.exe"))) {
    & $PythonLauncher -3.12 -m venv $EnvironmentRoot
    if ($LASTEXITCODE -ne 0) {
        throw "Для сборки требуется CPython 3.12 и Windows Python Launcher."
    }
}

$Python = Join-Path $EnvironmentRoot "Scripts\python.exe"
& $Python -m pip install `
    --disable-pip-version-check `
    --requirement (Join-Path $ProjectRoot "workers\requirements-build.txt")
if ($LASTEXITCODE -ne 0) {
    throw "Не удалось установить закреплённые зависимости Silero-worker."
}

New-Item -ItemType Directory -Force -Path $OutputDirectory, $BuildRoot | Out-Null
& $Python -m PyInstaller `
    --noconfirm `
    --clean `
    --onefile `
    --name "silero-worker" `
    --distpath $OutputDirectory `
    --workpath (Join-Path $BuildRoot "work") `
    --specpath $BuildRoot `
    (Join-Path $ProjectRoot "workers\silero_worker.py")
if ($LASTEXITCODE -ne 0) {
    throw "Сборка Silero-worker завершилась с ошибкой."
}

$Worker = Join-Path $OutputDirectory "silero-worker.exe"
$Hash = (Get-FileHash -LiteralPath $Worker -Algorithm SHA256).Hash.ToLowerInvariant()
Write-Host "Silero-worker готов: $Worker"
Write-Host "SHA-256: $Hash"
