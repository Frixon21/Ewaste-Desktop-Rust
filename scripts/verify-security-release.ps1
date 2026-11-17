param([Parameter(Mandatory=$true)][string]$PreviousExecutable,[ValidateSet(4,5,6,7)][int]$ExpectedSchema=7,[switch]$SameSchemaCompatibility,[string]$SourceDatabase)
$ErrorActionPreference='Stop'
$appRoot=Split-Path $PSScriptRoot -Parent
$release=Join-Path $appRoot 'target/release/ewaste-counter.exe'
$source=if([string]::IsNullOrWhiteSpace($SourceDatabase)){Join-Path $appRoot 'data/counter.sqlite3'}else{
    if(![System.IO.Path]::IsPathFullyQualified($SourceDatabase)){throw 'SourceDatabase must be an absolute path to an existing register or isolated snapshot.'}
    $sourceItem=Get-Item -LiteralPath $SourceDatabase -ErrorAction Stop
    if($sourceItem.PSIsContainer){throw 'SourceDatabase must name an existing register file.'}
    $sourceItem.FullName
}
$stage=Join-Path $appRoot ('.verification/security-release-'+[guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $stage | Out-Null
$backup=Join-Path $stage 'migrated-backup.sqlite3'
$restored=Join-Path $stage 'restored.sqlite3'
$sourceHash=(Get-FileHash -LiteralPath $source).Hash
$savedDb=$env:EWASTE_COUNTER_DB
$windowProcess=$null
function Invoke-Register([string]$Executable,[string[]]$CommandArguments){
    $info=[System.Diagnostics.ProcessStartInfo]::new()
    $info.FileName=$Executable
    $info.UseShellExecute=$false
    $info.CreateNoWindow=$true
    foreach($argument in $CommandArguments){$info.ArgumentList.Add($argument)}
    $process=[System.Diagnostics.Process]::Start($info)
    try {
        if(!$process.WaitForExit(10000)){throw 'Register command exceeded its smoke-test timeout.'}
        return $process.ExitCode
    } finally {
        if(!$process.HasExited){$process.Kill()}
        $process.Dispose()
    }
}
try {
    $env:EWASTE_COUNTER_DB=$source
    if((Invoke-Register $release @('--backup',$backup)) -ne 0){throw 'Release backup failed.'}
    if((Get-FileHash -LiteralPath $source).Hash -ne $sourceHash){throw 'Backup changed the source register.'}
    # Restore paths are explicit. Keep any expected refusal log in the isolated stage.
    $env:EWASTE_COUNTER_DB=$restored
    if((Invoke-Register $release @('--restore-copy',$backup,'--to',$restored)) -ne 0){throw 'Release restore failed.'}
    $before=(Get-FileHash -LiteralPath $restored).Hash
    if((Invoke-Register $release @('--restore-copy',$backup,'--to',$restored)) -eq 0){throw 'Restore overwrote an existing output.'}
    if((Get-FileHash -LiteralPath $restored).Hash -ne $before){throw 'Existing-output refusal changed the output.'}
    $oldBackup=Join-Path $stage 'old-executable-output.sqlite3'
    $previousExit=Invoke-Register $PreviousExecutable @('--backup',$oldBackup)
    if($SameSchemaCompatibility){
        if($previousExit -ne 0){throw 'The previous executable cannot read the compatible snapshot.'}
    }elseif($previousExit -eq 0){throw 'Previous executable accepted the new schema.'}
    if((Get-FileHash -LiteralPath $restored).Hash -ne $before){throw 'Previous executable changed the new schema.'}
    if(!$SameSchemaCompatibility -and (Test-Path -LiteralPath $oldBackup)){throw 'Previous executable created a backup despite rejecting the newer schema.'}
    # Inspect only the app-owned copy, never legacy records. No credentials are created.
    @'
import json, sqlite3, sys
from pathlib import Path
with sqlite3.connect(Path(sys.argv[3]).as_uri()+'?mode=ro',uri=True) as original:
    original_tickets = original.execute('select id,created_at,payload from tickets order by id').fetchall()
    original_state = dict(original.execute('select key,payload from app_state')) if original.execute("select 1 from sqlite_master where name='app_state'").fetchone() else {}
with sqlite3.connect(sys.argv[1]) as db:
    assert db.execute('pragma user_version').fetchone()[0] == int(sys.argv[2])
    assert db.execute('pragma quick_check').fetchone()[0] == 'ok'
    assert db.execute('select id,created_at,payload from tickets order by id').fetchall() == original_tickets
    raw = dict(db.execute('select key,payload from app_state'))
    for key,payload in original_state.items():
        assert raw[key] == payload, 'Migration changed existing app metadata'
    state = {key: json.loads(value)['data'] for key,value in raw.items()}
    if 'auth' not in original_state:
        assert state['auth_setup'] == 'NeedsBootstrap'
        assert state['auth']['operators'] == []
        assert state['security_audit']['events'] == []
        assert state['adjustments']['events'] == []
    if int(sys.argv[2]) >= 5:
        assert 'price_overrides' in state
        if 'price_overrides' not in original_state:
            assert state['price_overrides']['events'] == []
    if int(sys.argv[2]) >= 6:
        assert 'inventory_directory' in state and 'inventory_ledger' in state
        if 'inventory_ledger' not in original_state:
            assert state['inventory_ledger']['events'] == []
    if int(sys.argv[2]) >= 7:
        assert 'receipt_dispatches' in state and 'printer_settings' in state
        if 'receipt_dispatches' not in original_state:
            assert state['receipt_dispatches'] == {'jobs': [], 'next_id': 1}
        if 'printer_settings' not in original_state:
            assert state['printer_settings'] == {'queue': None}
print('Required metadata, preserved records and integrity verified; no credentials or events reset')
'@ | python - $restored $ExpectedSchema $source
    if($LASTEXITCODE){throw 'Schema metadata verification failed.'}
    $windowProcess=Start-Process -FilePath $release -PassThru -WindowStyle Hidden
    $opened=$false
    for($attempt=0;$attempt -lt 40;$attempt++){
        Start-Sleep -Milliseconds 250
        $windowProcess.Refresh()
        if($windowProcess.HasExited){throw 'Native register exited during startup.'}
        if($windowProcess.MainWindowTitle -eq 'E-waste Counter'){$opened=$true;break}
    }
    if(!$opened){throw 'Native register window was not observed.'}
    if((Get-FileHash -LiteralPath $source).Hash -ne $sourceHash){throw 'Smoke test changed the current register.'}
    $compatibilityCheck=if($SameSchemaCompatibility){'previous executable reads the compatible snapshot without changing it'}else{'old executable rejects new schema'}
    $summary=[ordered]@{
        release_sha256=(Get-FileHash -LiteralPath $release).Hash
        source_path=$source
        source_sha256=$sourceHash
        stage=$stage
        schema=$ExpectedSchema
        checks=@('read-only source backup','restore new copy','existing-output refusal',$compatibilityCheck,'required metadata and preserved records','native startup window')
        visual_acceptance=$false
    }
    $summary | ConvertTo-Json -Depth 3 | Set-Content -LiteralPath (Join-Path $stage 'verification.json')
    $stage | Set-Content -LiteralPath (Join-Path $appRoot '.verification/latest-security-release-smoke.txt')
    $summary | ConvertTo-Json -Depth 3
} finally {
    if($windowProcess -and !$windowProcess.HasExited){Stop-Process -Id $windowProcess.Id}
    $env:EWASTE_COUNTER_DB=$savedDb
}
