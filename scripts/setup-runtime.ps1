param(
    [Parameter(Mandatory)]
    [string]$RuntimeRoot,
    [switch]$InstallPiper,
    [switch]$InstallWhisper,
    [switch]$InstallSilero,
    [string]$PiperWorkerSource
)

$ErrorActionPreference = "Stop"
$BinDir = Join-Path $RuntimeRoot "bin"
$ModelDir = Join-Path $RuntimeRoot "models"
$DownloadDir = Join-Path $RuntimeRoot "downloads"

function Write-ProgressEvent {
    param([int]$Percent, [string]$Message)
    Write-Output "[progress]$Percent|$Message"
}

function Get-VerifiedFile {
    param(
        [Parameter(Mandatory)][string]$Url,
        [Parameter(Mandatory)][string]$Destination,
        [string]$Sha256
    )

    if (Test-Path -LiteralPath $Destination) {
        if (-not $Sha256) { return }
        $CurrentHash = (Get-FileHash -LiteralPath $Destination -Algorithm SHA256).Hash.ToLowerInvariant()
        if ($CurrentHash -eq $Sha256) { return }
        Remove-Item -LiteralPath $Destination -Force
    }

    $Partial = "$Destination.part"
    & curl.exe --fail --location --ssl-no-revoke --retry 3 --retry-all-errors `
        --continue-at - --output $Partial $Url
    if ($LASTEXITCODE -ne 0) {
        throw "Download failed: $Url (curl: $LASTEXITCODE)"
    }
    Move-Item -LiteralPath $Partial -Destination $Destination -Force

    if ($Sha256) {
        $DownloadedHash = (Get-FileHash -LiteralPath $Destination -Algorithm SHA256).Hash.ToLowerInvariant()
        if ($DownloadedHash -ne $Sha256) {
            Remove-Item -LiteralPath $Destination -Force
            throw "SHA-256 mismatch: $Destination"
        }
    }
}

New-Item -ItemType Directory -Force -Path $RuntimeRoot, $BinDir, $ModelDir, $DownloadDir | Out-Null

if ($InstallPiper) {
    Write-ProgressEvent 8 "Downloading native Piper engine"
    $SherpaArchive = Join-Path $DownloadDir "sherpa-onnx-v1.13.4-win-x64-shared-MD-Release.tar.bz2"
    $PiperArchive = Join-Path $DownloadDir "vits-piper-ru_RU-dmitri-medium.tar.bz2"
    Get-VerifiedFile `
        -Url "https://github.com/k2-fsa/sherpa-onnx/releases/download/v1.13.4/sherpa-onnx-v1.13.4-win-x64-shared-MD-Release.tar.bz2" `
        -Destination $SherpaArchive `
        -Sha256 "d4dacc8be5afe03f22ade4d50cfd587c03a625eaca8c41f2d99a24d3db463eab"
    Get-VerifiedFile `
        -Url "https://github.com/k2-fsa/sherpa-onnx/releases/download/tts-models/vits-piper-ru_RU-dmitri-medium.tar.bz2" `
        -Destination $PiperArchive `
        -Sha256 "c86d0803737de13d441923ff3b3f309482fab8d7af3ec85949942809eb9a3660"

    Write-ProgressEvent 24 "Extracting Piper"
    $SherpaExtract = Join-Path $RuntimeRoot "sherpa-onnx-v1.13.4"
    if (-not (Test-Path -LiteralPath $SherpaExtract)) {
        & tar.exe -xjf $SherpaArchive -C $RuntimeRoot
        if ($LASTEXITCODE -ne 0) { throw "Failed to extract sherpa-onnx" }
        Move-Item `
            -LiteralPath (Join-Path $RuntimeRoot "sherpa-onnx-v1.13.4-win-x64-shared-MD-Release") `
            -Destination $SherpaExtract
    }
    $PiperExtract = Join-Path $RuntimeRoot "vits-piper-ru_RU-dmitri-medium"
    if (-not (Test-Path -LiteralPath $PiperExtract)) {
        & tar.exe -xjf $PiperArchive -C $RuntimeRoot
        if ($LASTEXITCODE -ne 0) { throw "Failed to extract the Piper model" }
    }

    $PiperRoot = Join-Path $RuntimeRoot "tts\piper-dmitri-fp32"
    $PiperBin = Join-Path $PiperRoot "bin"
    $PiperModel = Join-Path $PiperRoot "model"
    New-Item -ItemType Directory -Force -Path $PiperBin, $PiperModel | Out-Null
    Copy-Item (Join-Path $SherpaExtract "lib\sherpa-onnx-c-api.dll") $PiperBin -Force
    Copy-Item (Join-Path $SherpaExtract "bin\onnxruntime.dll") $PiperBin -Force
    Copy-Item (Join-Path $SherpaExtract "bin\onnxruntime_providers_shared.dll") $PiperBin -Force
    Copy-Item (Join-Path $PiperExtract "*") $PiperModel -Recurse -Force
    if ($PiperWorkerSource -and (Test-Path -LiteralPath $PiperWorkerSource)) {
        Copy-Item -LiteralPath $PiperWorkerSource -Destination (Join-Path $PiperBin "piper-worker.exe") -Force
    }
}

if ($InstallWhisper) {
    Write-ProgressEvent 38 "Downloading CPU whisper.cpp"
    $WhisperArchive = Join-Path $DownloadDir "whisper-bin-x64.zip"
    Get-VerifiedFile `
        -Url "https://github.com/ggml-org/whisper.cpp/releases/download/v1.9.1/whisper-bin-x64.zip" `
        -Destination $WhisperArchive `
        -Sha256 "7d8be46ecd31828e1eb7a2ecdd0d6b314feafd82163038ab6092594b0a063539"
    $WhisperExtract = Join-Path $RuntimeRoot "whisper-v1.9.1-cpu"
    if (-not (Test-Path -LiteralPath $WhisperExtract)) {
        Expand-Archive -LiteralPath $WhisperArchive -DestinationPath $WhisperExtract
    }
    $WhisperCli = Get-ChildItem $WhisperExtract -Recurse -Filter "whisper-cli.exe" | Select-Object -First 1
    if (-not $WhisperCli) { throw "whisper-cli.exe is missing from the archive" }
    Copy-Item $WhisperCli.FullName (Join-Path $BinDir "whisper-cli.exe") -Force
    Get-ChildItem $WhisperCli.Directory.FullName -Filter "*.dll" | Copy-Item -Destination $BinDir -Force

    Write-ProgressEvent 50 "Downloading speech model (about 1.6 GB)"
    Get-VerifiedFile `
        -Url "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo.bin?download=true" `
        -Destination (Join-Path $ModelDir "ggml-large-v3-turbo.bin") `
        -Sha256 "1fc70f774d38eb169993ac391eea357ef47c88757ef72ee5943879b7e8e2bc69"
    Get-VerifiedFile `
        -Url "https://huggingface.co/ggml-org/whisper-vad/resolve/main/ggml-silero-v6.2.0.bin?download=true" `
        -Destination (Join-Path $ModelDir "ggml-silero-v6.2.0.bin") `
        -Sha256 "2aa269b785eeb53a82983a20501ddf7c1d9c48e33ab63a41391ac6c9f7fb6987"
}

if ($InstallSilero) {
    Write-ProgressEvent 62 "Installing private Python runtime for Silero"
    $SileroRoot = Join-Path $RuntimeRoot "tts\silero-v5-5-eugene"
    $PythonDir = Join-Path $SileroRoot "python"
    $WorkerDir = Join-Path $SileroRoot "worker"
    $SileroModels = Join-Path $SileroRoot "models"
    New-Item -ItemType Directory -Force -Path $SileroRoot, $WorkerDir, $SileroModels | Out-Null

    if (-not (Test-Path -LiteralPath (Join-Path $PythonDir "python.exe"))) {
        $PythonInstaller = Join-Path $DownloadDir "python-3.12.10-amd64.exe"
        Get-VerifiedFile `
            -Url "https://www.python.org/ftp/python/3.12.10/python-3.12.10-amd64.exe" `
            -Destination $PythonInstaller
        $Arguments = "/quiet InstallAllUsers=0 Include_launcher=0 Include_test=0 Include_doc=0 Include_tcltk=0 Include_pip=1 Include_dev=0 PrependPath=0 TargetDir=`"$PythonDir`""
        $Process = Start-Process -FilePath $PythonInstaller -ArgumentList $Arguments -Wait -PassThru
        if ($Process.ExitCode -ne 0) { throw "Python installer failed with code $($Process.ExitCode)" }
    }

    Write-ProgressEvent 74 "Installing CPU PyTorch"
    & (Join-Path $PythonDir "python.exe") -m pip install --disable-pip-version-check `
        "torch==2.7.1" --index-url "https://download.pytorch.org/whl/cpu"
    if ($LASTEXITCODE -ne 0) { throw "Failed to install PyTorch for Silero" }

    Write-ProgressEvent 88 "Downloading Silero 5.5 and its worker"
    Get-VerifiedFile `
        -Url "https://models.silero.ai/models/tts/ru/v5_5_ru.pt" `
        -Destination (Join-Path $SileroModels "v5_5_ru.pt")
    Get-VerifiedFile `
        -Url "https://github.com/PurpleSloth/simple-dub/releases/download/v0.1.0/silero-worker.py" `
        -Destination (Join-Path $WorkerDir "silero_worker.py")
}

Write-ProgressEvent 100 "Runtime components are ready"
