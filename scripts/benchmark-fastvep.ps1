[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$InputVcf,

    [string]$FastVep,
    [string]$Fasta,
    [string]$TranscriptCache,

    [ValidateRange(1, 20)]
    [int]$Iterations = 3,

    [ValidateRange(0, [long]::MaxValue)]
    [long]$RecordCount = 0,

    [string]$JsonOutput
)

$ErrorActionPreference = "Stop"
$projectRoot = Split-Path -Parent $PSScriptRoot

function Resolve-RequiredFile {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Path,
        [Parameter(Mandatory = $true)]
        [string]$Label
    )

    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
        throw "$Label was not found: $Path"
    }
    (Resolve-Path -LiteralPath $Path).Path
}

function Find-DefaultFasta {
    $root = Join-Path $projectRoot "target\debug\resources\reference\grch38"
    $candidate = Get-ChildItem -LiteralPath $root -Filter "*.fna" -File -Recurse |
        Where-Object { Test-Path -LiteralPath "$($_.FullName).fai" -PathType Leaf } |
        Sort-Object LastWriteTimeUtc -Descending |
        Select-Object -First 1
    if ($null -eq $candidate) {
        throw "No indexed GRCh38 FASTA was found under $root"
    }
    $candidate.FullName
}

function Get-VcfRecordCount {
    param([Parameter(Mandatory = $true)][string]$Path)

    $file = [System.IO.File]::OpenRead($Path)
    try {
        $stream = $file
        if ($Path.EndsWith(".gz", [System.StringComparison]::OrdinalIgnoreCase) -or
            $Path.EndsWith(".bgz", [System.StringComparison]::OrdinalIgnoreCase)) {
            $stream = [System.IO.Compression.GZipStream]::new(
                $file,
                [System.IO.Compression.CompressionMode]::Decompress,
                $false
            )
        }
        try {
            $reader = [System.IO.StreamReader]::new($stream)
            try {
                [long]$records = 0
                while (($line = $reader.ReadLine()) -ne $null) {
                    if ($line.Length -gt 0 -and $line[0] -ne "#") {
                        $records++
                    }
                }
                return $records
            }
            finally {
                $reader.Dispose()
            }
        }
        finally {
            if ($stream -ne $file) {
                $stream.Dispose()
            }
        }
    }
    finally {
        $file.Dispose()
    }
}

function Get-Median {
    param([Parameter(Mandatory = $true)][double[]]$Values)

    $ordered = @($Values | Sort-Object)
    $middle = [math]::Floor($ordered.Count / 2)
    if (($ordered.Count % 2) -eq 1) {
        return $ordered[$middle]
    }
    ($ordered[$middle - 1] + $ordered[$middle]) / 2
}

if ([string]::IsNullOrWhiteSpace($FastVep)) {
    $FastVep = Join-Path $projectRoot "tools\fastvep\fastvep.exe"
}
if ([string]::IsNullOrWhiteSpace($Fasta)) {
    $Fasta = Find-DefaultFasta
}
if ([string]::IsNullOrWhiteSpace($TranscriptCache)) {
    $TranscriptCache = Join-Path $projectRoot "target\debug\resources\transcript-cache\ensembl-115.cache"
}

$inputPath = Resolve-RequiredFile -Path $InputVcf -Label "Input VCF"
$fastVepPath = Resolve-RequiredFile -Path $FastVep -Label "fastVEP executable"
$fastaPath = Resolve-RequiredFile -Path $Fasta -Label "Reference FASTA"
$transcriptCachePath = Resolve-RequiredFile -Path $TranscriptCache -Label "Transcript cache"

if ($RecordCount -eq 0) {
    Write-Host "Counting VCF records outside the timed benchmark..."
    $RecordCount = Get-VcfRecordCount -Path $inputPath
}
if ($RecordCount -eq 0) {
    throw "The input VCF contains no variant records"
}

$temporaryRoot = [System.IO.Path]::GetFullPath([System.IO.Path]::GetTempPath())
$benchmarkRoot = Join-Path $temporaryRoot "annocat-fastvep-benchmark-$([guid]::NewGuid().ToString('N'))"
[System.IO.Directory]::CreateDirectory($benchmarkRoot) | Out-Null
$runs = [System.Collections.Generic.List[object]]::new()

try {
    for ($iteration = 1; $iteration -le $Iterations; $iteration++) {
        foreach ($mode in @("vcf-only", "vcf-and-structured")) {
            $prefix = "$mode-$iteration"
            $vcfOutput = Join-Path $benchmarkRoot "$prefix.vcf"
            $structuredOutput = Join-Path $benchmarkRoot "$prefix.ndjson"
            $stdoutLog = Join-Path $benchmarkRoot "$prefix.stdout.log"
            $stderrLog = Join-Path $benchmarkRoot "$prefix.stderr.log"
            $arguments = @(
                "annotate",
                "--input", $inputPath,
                "--output", $vcfOutput,
                "--output-format", "vcf",
                "--fasta", $fastaPath,
                "--transcript-cache", $transcriptCachePath,
                "--symbol",
                "--hgvs",
                "--canonical",
                "--no-progress"
            )
            if ($mode -eq "vcf-and-structured") {
                $arguments += @("--structured-output", $structuredOutput)
            }

            $timer = [System.Diagnostics.Stopwatch]::StartNew()
            $previousErrorActionPreference = $ErrorActionPreference
            try {
                # Windows PowerShell 5 wraps native stderr as NativeCommandError when Stop is active.
                $ErrorActionPreference = "Continue"
                & $fastVepPath @arguments 1> $stdoutLog 2> $stderrLog
                $exitCode = $LASTEXITCODE
            }
            finally {
                $ErrorActionPreference = $previousErrorActionPreference
            }
            $timer.Stop()
            if ($exitCode -ne 0) {
                $stderr = Get-Content -LiteralPath $stderrLog -Raw
                throw "fastVEP $mode iteration $iteration failed with exit code ${exitCode}: $stderr"
            }

            [long]$outputBytes = (Get-Item -LiteralPath $vcfOutput).Length
            if ($mode -eq "vcf-and-structured") {
                $outputBytes += (Get-Item -LiteralPath $structuredOutput).Length
            }
            $seconds = $timer.Elapsed.TotalSeconds
            $runs.Add([pscustomobject]@{
                mode = $mode
                iteration = $iteration
                elapsedSeconds = $seconds
                variantsPerSecond = $RecordCount / $seconds
                outputBytes = $outputBytes
                outputMiBPerSecond = ($outputBytes / 1MB) / $seconds
            })

            Remove-Item -LiteralPath $vcfOutput -Force
            if (Test-Path -LiteralPath $structuredOutput) {
                Remove-Item -LiteralPath $structuredOutput -Force
            }
            Remove-Item -LiteralPath $stdoutLog, $stderrLog -Force
        }
    }

    $summaries = @(
        foreach ($mode in @("vcf-only", "vcf-and-structured")) {
            $modeRuns = @($runs | Where-Object mode -eq $mode)
            [pscustomobject]@{
                mode = $mode
                iterations = $modeRuns.Count
                medianSeconds = Get-Median -Values @($modeRuns.elapsedSeconds)
                medianVariantsPerSecond = Get-Median -Values @($modeRuns.variantsPerSecond)
                medianOutputMiBPerSecond = Get-Median -Values @($modeRuns.outputMiBPerSecond)
                outputBytes = $modeRuns[0].outputBytes
            }
        }
    )

    $result = [pscustomobject]@{
        schemaVersion = 1
        generatedAt = [DateTimeOffset]::Now.ToString("o")
        input = $inputPath
        records = $RecordCount
        executable = $fastVepPath
        fasta = $fastaPath
        transcriptCache = $transcriptCachePath
        summaries = $summaries
        runs = @($runs)
    }

    $summaries |
        Select-Object mode, iterations,
            @{Name = "medianSeconds"; Expression = { "{0:N3}" -f $_.medianSeconds }},
            @{Name = "medianVariantsPerSecond"; Expression = { "{0:N1}" -f $_.medianVariantsPerSecond }},
            @{Name = "medianOutputMiBPerSecond"; Expression = { "{0:N1}" -f $_.medianOutputMiBPerSecond }} |
        Format-Table -AutoSize

    if (-not [string]::IsNullOrWhiteSpace($JsonOutput)) {
        $jsonPath = [System.IO.Path]::GetFullPath($JsonOutput)
        $jsonParent = Split-Path -Parent $jsonPath
        if (-not [string]::IsNullOrWhiteSpace($jsonParent)) {
            [System.IO.Directory]::CreateDirectory($jsonParent) | Out-Null
        }
        $result | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath $jsonPath -Encoding utf8
        Write-Host "Wrote benchmark summary to $jsonPath"
    }
}
finally {
    $resolvedBenchmarkRoot = [System.IO.Path]::GetFullPath($benchmarkRoot)
    if (-not $resolvedBenchmarkRoot.StartsWith(
        $temporaryRoot,
        [System.StringComparison]::OrdinalIgnoreCase
    )) {
        throw "Refusing to clean benchmark output outside the operating-system temporary directory"
    }
    if (Test-Path -LiteralPath $resolvedBenchmarkRoot) {
        Remove-Item -LiteralPath $resolvedBenchmarkRoot -Recurse -Force
    }
}
