$ErrorActionPreference = "Stop"

$ProjectRoot = Split-Path -Parent $PSScriptRoot
$CandidateFiles = & git -C $ProjectRoot ls-files --cached --others --exclude-standard
if ($LASTEXITCODE -ne 0) {
    throw "Не удалось получить список файлов Git"
}

$SecretPattern = 'sk-or-v[0-9]+-[A-Za-z0-9_-]{16,}'
$Findings = [System.Collections.Generic.List[string]]::new()

foreach ($RelativePath in $CandidateFiles) {
    $AbsolutePath = Join-Path $ProjectRoot $RelativePath
    if (-not (Test-Path -LiteralPath $AbsolutePath -PathType Leaf)) {
        continue
    }

    try {
        $LineNumber = 0
        foreach ($Line in [System.IO.File]::ReadLines($AbsolutePath)) {
            $LineNumber++
            if ($Line -match $SecretPattern) {
                $Findings.Add("${RelativePath}:${LineNumber}")
            }
        }
    }
    catch {
        # Бинарные ресурсы не содержат проверяемого исходного текста.
        continue
    }
}

if ($Findings.Count -gt 0) {
    Write-Error (
        "Обнаружены возможные ключи OpenRouter:`n" +
        ($Findings -join [Environment]::NewLine)
    )
    exit 1
}

Write-Host "Секреты OpenRouter в файлах будущего коммита не обнаружены."
