param([switch]$ExtendedWorkflows,[switch]$PrinterPdf)
$ErrorActionPreference='Stop'
if($ExtendedWorkflows -and $PrinterPdf){throw 'The PDF printer check belongs to the basic walkthrough build.'}
$env:RUSTUP_HOME=Join-Path $PSScriptRoot '.toolchain/rustup'
$env:CARGO_HOME=Join-Path $PSScriptRoot '.toolchain/cargo'
$env:CARGO_TARGET_DIR=Join-Path $PSScriptRoot 'target'
$coverageDir=Join-Path $PSScriptRoot '.verification/coverage'
New-Item -ItemType Directory -Force $coverageDir | Out-Null
$coverageRun=Join-Path $coverageDir ([guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory $coverageRun | Out-Null
$env:LLVM_PROFILE_FILE=Join-Path $coverageRun '%p.profraw'
$vs='C:/Program Files (x86)/Microsoft Visual Studio/18/BuildTools'
. (Join-Path $PSScriptRoot 'scripts/vs-build-environment.ps1')
Initialize-EwasteVisualStudioEnvironment -VsInstallPath $vs
Push-Location $PSScriptRoot
try {
 $cargo=Join-Path $env:CARGO_HOME 'bin/cargo.exe'
 $featureArgs=if($ExtendedWorkflows){@('--features','extended-workflows')}else{@()}
 $output=& $cargo rustc --locked --tests --message-format=json @featureArgs -- -C instrument-coverage
 if($LASTEXITCODE){exit $LASTEXITCODE}
 $artifacts=$output | ForEach-Object { try {$_ | ConvertFrom-Json} catch {} } | Where-Object {$_.reason -eq 'compiler-artifact' -and $_.executable -and $_.profile.test}
 $testExe=($artifacts | Select-Object -Last 1).executable
 if(!$testExe){throw 'Coverage test executable was not returned.'}
 & $testExe
 if($LASTEXITCODE){exit $LASTEXITCODE}
 if($PrinterPdf){
  & $testExe --exact printer_windows::tests::microsoft_pdf_only_to_new_workspace_verification_file --ignored --nocapture
  if($LASTEXITCODE){exit $LASTEXITCODE}
 }
 $llvmBin=Join-Path $env:RUSTUP_HOME 'toolchains/stable-x86_64-pc-windows-msvc/lib/rustlib/x86_64-pc-windows-msvc/bin'
 $rawFiles=@(Get-ChildItem -LiteralPath $coverageRun -Filter '*.profraw' | ForEach-Object FullName)
 $profile=Join-Path $coverageRun 'coverage.profdata'
 & (Join-Path $llvmBin 'llvm-profdata.exe') merge -sparse @rawFiles -o $profile
 if($LASTEXITCODE){exit $LASTEXITCODE}
 & (Join-Path $llvmBin 'llvm-cov.exe') report $testExe "-instr-profile=$profile" '-ignore-filename-regex=(_tests\.rs|\.toolchain)' | Tee-Object -FilePath (Join-Path $coverageRun 'summary.txt')
 & (Join-Path $llvmBin 'llvm-cov.exe') export $testExe "-instr-profile=$profile" '-ignore-filename-regex=(_tests\.rs|\.toolchain)' | Set-Content (Join-Path $coverageRun 'coverage.json')
 & (Join-Path $llvmBin 'llvm-cov.exe') export $testExe "-instr-profile=$profile" -format=lcov '-ignore-filename-regex=(_tests\.rs|\.toolchain)' | Set-Content (Join-Path $coverageRun 'coverage.lcov')
 if($LASTEXITCODE){exit $LASTEXITCODE}
 $summaryArgs=@(if($ExtendedWorkflows){'--extended'})
 python (Join-Path $PSScriptRoot 'scripts/coverage-summary.py') (Join-Path $coverageRun 'coverage.lcov') @summaryArgs
 if($LASTEXITCODE){exit $LASTEXITCODE}
 $coverageRun | Set-Content (Join-Path $coverageDir 'latest.txt')
 $mode=if($ExtendedWorkflows){'extended'}else{'walkthrough'}
 $coverageRun | Set-Content (Join-Path $coverageDir "latest-$mode.txt")
} finally { Pop-Location }
