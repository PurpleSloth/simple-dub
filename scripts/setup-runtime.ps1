param(
    [switch]$SkipWhisperModel,
    [switch]$SkipSileroModel
)

$ErrorActionPreference = "Stop"

$ProjectRoot = Split-Path -Parent $PSScriptRoot
$RuntimeRoot = Join-Path $ProjectRoot "runtime"
$BinDir = Join-Path $RuntimeRoot "bin"
$ModelDir = Join-Path $RuntimeRoot "models"
$DownloadDir = Join-Path $RuntimeRoot "downloads"

$WhisperVersion = "v1.9.1"
$WhisperArchiveName = "whisper-cublas-12.4.0-bin-x64.zip"
$WhisperArchiveSha256 = "106a2030eff8998e4ef320fe72e263a78449e9040386ee27c41ea80b001b601b"
$WhisperArchiveUrl = "https://github.com/ggml-org/whisper.cpp/releases/download/$WhisperVersion/$WhisperArchiveName"

$WhisperModelName = "ggml-large-v3-turbo.bin"
$WhisperModelSha256 = "1fc70f774d38eb169993ac391eea357ef47c88757ef72ee5943879b7e8e2bc69"
$WhisperModelUrl = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/${WhisperModelName}?download=true"

$VadModelName = "ggml-silero-v6.2.0.bin"
$VadModelSha256 = "2aa269b785eeb53a82983a20501ddf7c1d9c48e33ab63a41391ac6c9f7fb6987"
$VadModelUrl = "https://huggingface.co/ggml-org/whisper-vad/resolve/main/${VadModelName}?download=true"

$SileroModelName = "v5_5_ru.pt"
$SileroModelUrl = "https://models.silero.ai/models/tts/ru/$SileroModelName"

function Get-VerifiedFile {
    param(
        [Parameter(Mandatory)]
        [string]$Url,
        [Parameter(Mandatory)]
        [string]$Destination,
        [string]$Sha256
    )

    if (Test-Path -LiteralPath $Destination) {
        if (-not $Sha256) {
            Write-Host "[skip] Уже существует: $Destination"
            return
        }

        $CurrentHash = (Get-FileHash -LiteralPath $Destination -Algorithm SHA256).Hash.ToLowerInvariant()
        if ($CurrentHash -eq $Sha256) {
            Write-Host "[skip] Проверенный файл уже существует: $Destination"
            return
        }

        throw "Контрольная сумма существующего файла не совпадает: $Destination"
    }

    Write-Host "[download] $Url"
    $PartialDestination = "$Destination.part"
    & curl.exe `
        --fail `
        --location `
        --ssl-no-revoke `
        --retry 3 `
        --retry-all-errors `
        --continue-at - `
        --output $PartialDestination `
        $Url
    if ($LASTEXITCODE -ne 0) {
        throw "curl завершился с кодом $LASTEXITCODE при загрузке $Url"
    }
    Move-Item -LiteralPath $PartialDestination -Destination $Destination -Force

    if ($Sha256) {
        $DownloadedHash = (Get-FileHash -LiteralPath $Destination -Algorithm SHA256).Hash.ToLowerInvariant()
        if ($DownloadedHash -ne $Sha256) {
            throw "Неверная SHA-256 для $Destination"
        }
    }
}

New-Item -ItemType Directory -Force -Path $BinDir, $ModelDir, $DownloadDir | Out-Null

$WhisperArchive = Join-Path $DownloadDir $WhisperArchiveName
Get-VerifiedFile -Url $WhisperArchiveUrl -Destination $WhisperArchive -Sha256 $WhisperArchiveSha256

$WhisperExtractDir = Join-Path $RuntimeRoot "whisper-$WhisperVersion"
if (-not (Test-Path -LiteralPath $WhisperExtractDir)) {
    Expand-Archive -LiteralPath $WhisperArchive -DestinationPath $WhisperExtractDir
}

$WhisperCli = Get-ChildItem -LiteralPath $WhisperExtractDir -Recurse -Filter "whisper-cli.exe" |
    Select-Object -First 1
if (-not $WhisperCli) {
    throw "В архиве whisper.cpp не найден whisper-cli.exe"
}

Copy-Item -LiteralPath $WhisperCli.FullName -Destination (Join-Path $BinDir "whisper-cli.exe") -Force
Get-ChildItem -LiteralPath $WhisperCli.Directory.FullName -Filter "*.dll" |
    Copy-Item -Destination $BinDir -Force

if (-not $SkipWhisperModel) {
    Get-VerifiedFile `
        -Url $WhisperModelUrl `
        -Destination (Join-Path $ModelDir $WhisperModelName) `
        -Sha256 $WhisperModelSha256
}

Get-VerifiedFile `
    -Url $VadModelUrl `
    -Destination (Join-Path $ModelDir $VadModelName) `
    -Sha256 $VadModelSha256

if (-not $SkipSileroModel) {
    Get-VerifiedFile `
        -Url $SileroModelUrl `
        -Destination (Join-Path $ModelDir $SileroModelName)
}

Write-Host "Runtime готов: $RuntimeRoot"
