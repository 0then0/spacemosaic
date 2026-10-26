# Verify and install the pinned compiler in a temporary build-tools directory.
$ErrorActionPreference = 'Stop'
$innoVersion = '7.1.0'
$toolsRoot = if ($env:RUNNER_TEMP) { $env:RUNNER_TEMP } else { [System.IO.Path]::GetTempPath() }
$compilerRoot = Join-Path $toolsRoot "spacemosaic-inno-$innoVersion"
$installerPath = Join-Path $toolsRoot "innosetup-$innoVersion-x64.exe"
Invoke-WebRequest -UseBasicParsing -Uri "https://github.com/jrsoftware/issrc/releases/download/is-7_1_0/innosetup-$innoVersion-x64.exe" -OutFile $installerPath
$expectedHash = '0362a383ed217d4c4239b5933866dd96d3eb2102737da92f80f6057a4b40df2f'
if ((Get-FileHash $installerPath -Algorithm SHA256).Hash.ToLowerInvariant() -ne $expectedHash) {
    throw 'Inno Setup installer SHA-256 mismatch'
}
$process = Start-Process -FilePath $installerPath -ArgumentList @('/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', '/SP-', "/DIR=`"$compilerRoot`"") -Wait -PassThru
if ($process.ExitCode -ne 0) { throw "Inno Setup installation failed: $($process.ExitCode)" }
$compilerPath = Join-Path $compilerRoot 'ISCC.exe'
if (!(Test-Path $compilerPath)) { throw 'Inno Setup compiler is missing' }
Write-Host "Inno Setup compiler: $((Get-Item $compilerPath).VersionInfo.FileVersion)"
$env:INNO_SETUP_COMPILER = $compilerPath
if ($env:GITHUB_ENV) {
    "INNO_SETUP_COMPILER=$compilerPath" | Out-File -FilePath $env:GITHUB_ENV -Encoding utf8 -Append
}
