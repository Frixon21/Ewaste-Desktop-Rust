param([ValidateSet('test','build','fmt','check','clippy')][string]$Task='test',[switch]$ExtendedWorkflows)
$ErrorActionPreference='Stop'
$env:RUSTUP_HOME=Join-Path $PSScriptRoot '.toolchain/rustup'
$env:CARGO_HOME=Join-Path $PSScriptRoot '.toolchain/cargo'
$env:CARGO_TARGET_DIR=Join-Path $PSScriptRoot 'target'
$vs='C:/Program Files (x86)/Microsoft Visual Studio/18/BuildTools'
. (Join-Path $PSScriptRoot 'scripts/vs-build-environment.ps1')
Initialize-EwasteVisualStudioEnvironment -VsInstallPath $vs
Push-Location $PSScriptRoot
try {
 $cargo=Join-Path $env:CARGO_HOME 'bin/cargo.exe'
 $featureArgs=if($ExtendedWorkflows){@('--features','extended-workflows')}else{@()}
 switch ($Task) {
  'fmt' { & $cargo fmt --all }
  'test' { & $cargo test --locked @featureArgs }
  'build' { & $cargo build --release --locked @featureArgs }
  'check' { & $cargo check --locked @featureArgs }
  'clippy' { & $cargo clippy --locked --all-targets @featureArgs -- -D warnings }
 }
 exit $LASTEXITCODE
} finally { Pop-Location }
